pub mod compositor;
// The zero-copy IOSurface surface backend: the default on macOS; disabled
// by the `cpu_readback` feature. IOSurface sharing is macOS-only.
#[cfg(all(target_os = "macos", not(feature = "cpu_readback")))]
pub mod iosurface;
pub mod renderer;

#[cfg(all(not(target_os = "macos"), feature = "zero_copy"))]
compile_error!(
    "the `zero_copy` feature requires macOS: IOSurface texture sharing is not available on this platform (use `cpu_readback` or no features)"
);

use std::collections::HashMap;

use crate::renderer::{ReadbackChannels, RenderError, SurfaceRenderer};
use compositor::{Compositor, CompositorVideoFrame};
use crossbeam_channel::{select, tick};
use ipc_messages::content::{FrameId, WebviewId};
use ipc_messages::graphics::{FrameHitInfo, GraphicsCommand, GraphicsEvent};
use ipc_messages::media::{MediaPipelineId, VideoPaintId};
use log::{debug, error, info};
use media::backend::{MediaBackend, MediaBackendEvent, PipelineHandle};
use verification::TLATracer;

/// The composed scene for one webview — the final result after compositing
/// all iframe and video embed sites into the root scene.
#[derive(Clone)]
pub struct ComposedScene {
    pub webview_id: WebviewId,
    pub scene: anyrender::Scene,
    pub frame_hit_info: Vec<FrameHitInfo>,
    /// Viewport data for child frames, keyed by child webview_id.
    /// Populated during compose_scene from the compositor's visible_frame_viewports.
    pub child_viewports: HashMap<WebviewId, [f64; 4]>,
    /// Mapping from child frame_id (the content_frame_id used in embed sites)
    /// to the child webview_id. Used by the UA to route UI events to the
    /// correct child traversable instead of the root.
    pub child_frame_to_webview: HashMap<FrameId, WebviewId>,
    /// True when the composed scene contains animated content (video)
    /// that requires the UA to re-note a rendering opportunity even
    /// without user input.
    pub animating: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisibleFrameViewport {
    pub frame_id: FrameId,
    pub offset_x: f32,
    pub offset_y: f32,
    pub width: u32,
    pub height: u32,
}

/// A webview's per-webview state: the compositor (scene assembly, fonts,
/// video frames) and the renderer (Vello + surface delivery, the ring, the
/// deferred scene).
struct WebviewState<R> {
    compositor: Compositor,
    renderer: R,
}

impl<R: SurfaceRenderer> WebviewState<R> {
    fn new(channels: ReadbackChannels<R::RenderData>) -> Self {
        Self {
            compositor: Compositor::default(),
            renderer: R::new(channels).unwrap_or_else(|error| panic!("renderer init: {error}")),
        }
    }
}

/// Run the graphics process event loop.
/// The media backend (if provided) runs directly in this loop — no separate IPC.
/// The pipeline_to_webview mapping is managed via RegisterMediaPipeline from content.
/// The surface renderer is chosen at compile time by feature and passed as
/// the generic `R` (see graphics/README.md); the loop interacts with it only
/// through the `SurfaceRenderer` trait.
pub fn run_graphics_process<B: MediaBackend + 'static, R: SurfaceRenderer>(
    cmd_rx: crossbeam_channel::Receiver<ipc::IpcIncoming<GraphicsCommand>>,
    graphics_event_tx: ipc::IpcSender<GraphicsEvent>,
    media_backend: Option<B>,
) {
    let mut webviews: HashMap<WebviewId, WebviewState<R>> = HashMap::new();
    let event_sender = graphics_event_tx;

    // Readback plumbing created once at startup: a native poll thread blocks
    // on device.poll(Wait) for each submitted frame (firing the readback map
    // callbacks on the CPU path), so the select! loop never has to poll at an
    // interval or busy-wait. Completed frames arrive on the single
    // `render_done` channel, whose message type is the renderer's RenderData.
    let (channels, poll_rx, render_done_rx) = ReadbackChannels::<R::RenderData>::new();
    let render_done_tx = channels.render_done_tx.clone();
    let poll_thread = std::thread::Builder::new()
        .name(String::from("formal-web:gpu-poll"))
        .spawn(move || {
            while let Ok(request) = poll_rx.recv() {
                // Block until the requested submission(s) complete; the map
                // callbacks registered with map_buffer_on_submit fire here,
                // delivering the completed frame to the main loop.
                if let Err(poll_error) = request.device.device.poll(wgpu::PollType::Wait {
                    submission_index: request.submission_index,
                    timeout: None,
                }) {
                    error!("[graphics] gpu poll failed: {poll_error:?}");
                }
                // Zero-copy path: no map callback exists; deliver the done
                // notice directly.
                if let Some(done) = request.done
                    && let Err(send_error) = render_done_tx.send(done)
                {
                    error!("[graphics] failed to deliver render done: {send_error}");
                }
            }
        })
        .expect("failed to spawn gpu poll thread");

    // Drain the first message to check for SetTraceSender.
    let mut tla_tracer = TLATracer::new("Navigation", "formal-web:graphics", None);
    if let Ok(incoming) = cmd_rx.try_recv() {
        if let GraphicsCommand::SetTraceSender(sender) = incoming.payload {
            tla_tracer.set_sender(sender);
        } else {
            // Not a trace sender; process it as a normal command.
            handle_command(
                incoming.payload,
                &mut webviews,
                &incoming.shmem_regions,
                &mut HashMap::new(),
                &mut HashMap::new(),
                None::<&mut B>,
                &mut HashMap::new(),
                &mut tla_tracer,
                &channels,
            );
        }
    }

    // Media pipeline state.
    let mut pipelines: HashMap<MediaPipelineId, B::Pipeline> = HashMap::new();
    let mut pipeline_webview_map: HashMap<MediaPipelineId, (WebviewId, VideoPaintId)> =
        HashMap::new();
    // Sample tick: dynamically switches between never() (no active pipelines)
    // and tick(8ms) (at least one pipeline needs sampling). This avoids waking
    // the select! loop every 8ms when everything is idle.
    let mut sample_tick: crossbeam_channel::Receiver<std::time::Instant> =
        crossbeam_channel::never();
    let mut had_active_pipelines = false;

    // Reverse mapping from child webview -> (parent webview, content_frame_id).
    // Populated by RegisterChildNavigableHost and used in PaintFrame to remap
    // child PaintFrames into the parent's compositor slot.
    let mut child_webview_to_parent: HashMap<WebviewId, (WebviewId, FrameId)> = HashMap::new();

    // Use crossbeam's never() channel when there's no backend so the select! loop
    // has a single uniform structure regardless of whether a backend exists.
    let (mut backend, media_event_rx) = match media_backend {
        Some(b) => {
            let rx = b.event_receiver();
            (Some(b), rx)
        }
        None => (None, crossbeam_channel::never()),
    };

    loop {
        // Dynamically switch the sample tick: when at least one pipeline
        // needs sampling, use an 8ms tick. When none do, use never() so the
        // select! loop doesn't wake up at all for sampling.
        let has_active_pipelines = pipelines.values().any(|p| !p.is_done());
        if has_active_pipelines && !had_active_pipelines {
            sample_tick = tick(std::time::Duration::from_millis(8));
            had_active_pipelines = true;
        } else if !has_active_pipelines && had_active_pipelines {
            sample_tick = crossbeam_channel::never();
            had_active_pipelines = false;
        }

        select! {
            recv(cmd_rx) -> cmd => {
                let Ok(incoming) = cmd else { break };
                if handle_command(
                    incoming.payload,
                    &mut webviews,
                    &incoming.shmem_regions,
                    &mut pipelines,
                    &mut pipeline_webview_map,
                    backend.as_mut(),
                    &mut child_webview_to_parent,
                    &mut tla_tracer,
                    &channels,
                ) {
                    break;
                }
            }
            recv(media_event_rx) -> event => {
                let Ok(event) = event else { break };
                handle_media_event(
                    event,
                    &pipeline_webview_map,
                    &mut webviews,
                    &event_sender,
                );
            }
            recv(sample_tick) -> _ => {
                // Only sample pipelines that haven't reached end-of-stream.
                // Sampling an idle pipeline burns CPU running the AVFoundation
                // run loop drain (NSRunLoop::runUntilDate with 8ms timeout).
                for pipeline in pipelines.values() {
                    if !pipeline.is_done() {
                        pipeline.sample();
                    }
                }
            }
            recv(render_done_rx) -> done => {
                let Ok(done) = done else { break };
                handle_render_done(&mut webviews, &event_sender, &mut tla_tracer, done);
            }
        }
    }

    // Shutdown: drop every poll sender (the renderers' ReadbackChannels clones
    // and the main channels) so the poll thread's channel closes, then join it.
    // The poll thread may still be blocked in device.poll(Wait) for the current
    // submission; that completes, the next recv() sees the closed channel, and
    // the thread exits.
    drop(webviews);
    drop(channels);
    if let Err(error) = poll_thread.join() {
        error!("[graphics] gpu poll thread panicked: {error:?}");
    }
}

fn handle_media_event<R: SurfaceRenderer>(
    event: MediaBackendEvent,
    pipeline_webview_map: &HashMap<MediaPipelineId, (WebviewId, VideoPaintId)>,
    webviews: &mut HashMap<WebviewId, WebviewState<R>>,
    composed_scene_sender: &ipc::IpcSender<GraphicsEvent>,
) {
    match event {
        MediaBackendEvent::Frame(mut video_frame) => {
            let pipeline_id = video_frame.pipeline_id;
            let Some(&(webview_id, paint_id)) = pipeline_webview_map.get(&pipeline_id) else {
                debug!("[graphics] frame for unknown pipeline {:?}", pipeline_id);
                return;
            };
            debug!(
                "[graphics] video frame arrived pipeline={:?} webview={:?}",
                pipeline_id, webview_id
            );
            let pixel_bytes: std::sync::Arc<[u8]> = std::mem::take(&mut video_frame.data).into();
            let cf = CompositorVideoFrame {
                video_paint_id: paint_id,
                width: video_frame.width,
                height: video_frame.height,
                content: compositor::VideoFrameContent::Bytes(pixel_bytes),
            };
            if let Some(slot) = webviews.get_mut(&webview_id) {
                // Store the video frame. It will be included in the next normal
                // composition (triggered by a content PaintFrame via the standard
                // render cycle). The compositor's compose_frame already checks
                // self.video_frames when rendering EmbedSite::Video embed sites.
                // Never compose independently from the video handler — doing so
                // creates an orphan composition with no corresponding
                // UpdateTheRendering, violating the TLA+ pipeline model.
                slot.compositor.update_video_frame(cf);
            }
        }
        // AVFoundation video frames arrive as GPU pixel buffers and are
        // composited as textures; GStreamer delivers CPU bytes (Frame
        // above). The variant exists only when AVFoundation is the active
        // media backend (explicit feature, or the default on macOS when no
        // backend feature is selected).
        #[cfg(all(
            target_os = "macos",
            any(feature = "backend-avfoundation", not(feature = "backend-gstreamer"))
        ))]
        MediaBackendEvent::PixelBufferFrame(frame) => {
            let pipeline_id = frame.pipeline_id;
            let Some(&(webview_id, paint_id)) = pipeline_webview_map.get(&pipeline_id) else {
                debug!(
                    "[graphics] pixel buffer frame for unknown pipeline {:?}",
                    pipeline_id
                );
                return;
            };
            let Some(slot) = webviews.get_mut(&webview_id) else {
                return;
            };
            let Some(resource_id) = slot.renderer.import_video_frame(
                paint_id,
                &frame.pixel_buffer,
                frame.width,
                frame.height,
            ) else {
                error!(
                    "[graphics] failed to import video texture pipeline={:?}",
                    pipeline_id
                );
                return;
            };
            let cf = CompositorVideoFrame {
                video_paint_id: paint_id,
                width: frame.width,
                height: frame.height,
                content: compositor::VideoFrameContent::Texture(resource_id),
            };
            slot.compositor.update_video_frame(cf);
        }
        MediaBackendEvent::Eos { pipeline_id } => {
            if let Some(&(webview_id, paint_id)) = pipeline_webview_map.get(&pipeline_id) {
                // Keep the last video frame in the compositor so it continues
                // to render as a static image. The PaintFrame from content will
                // carry animating=false, so the UA stops re-noting rendering
                // opportunities for video animation.
                if let Err(send_error) = composed_scene_sender.send(GraphicsEvent::VideoEnded {
                    webview_id,
                    video_paint_id: paint_id,
                }) {
                    error!("[graphics] failed to send VideoEnded: {send_error}");
                }
            }
        }
        MediaBackendEvent::Error {
            pipeline_id,
            message,
        } => {
            error!("[graphics] pipeline {:?} error: {}", pipeline_id, message);
        }
        MediaBackendEvent::DurationChanged {
            pipeline_id,
            duration_secs,
        } => {
            debug!(
                "[graphics] pipeline {:?} duration: {}s",
                pipeline_id, duration_secs
            );
        }
    }
}

fn handle_command<B: MediaBackend + 'static, R: SurfaceRenderer>(
    cmd: GraphicsCommand,
    webviews: &mut HashMap<WebviewId, WebviewState<R>>,
    shmem_regions: &HashMap<usize, ipc::IpcSharedRegion>,
    pipelines: &mut HashMap<MediaPipelineId, B::Pipeline>,
    pipeline_webview_map: &mut HashMap<MediaPipelineId, (WebviewId, VideoPaintId)>,
    media_backend: Option<&mut B>,
    child_webview_to_parent: &mut HashMap<WebviewId, (WebviewId, FrameId)>,
    tla_tracer: &mut TLATracer,
    channels: &ReadbackChannels<R::RenderData>,
) -> bool {
    match cmd {
        GraphicsCommand::RegisterWebview { webview_id } => {
            debug!("[graphics] registering webview {:?}", webview_id);
            webviews
                .entry(webview_id)
                .or_insert_with(|| WebviewState::new(channels.clone()));
        }
        GraphicsCommand::UnregisterWebview { webview_id } => {
            debug!("[graphics] unregistering webview {:?}", webview_id);
            webviews.remove(&webview_id);
        }
        GraphicsCommand::PaintFrame { frame } => {
            // Remap child PaintFrames into the parent's compositor slot.
            let (target_webview_id, actual_frame_id, is_root_candidate) =
                if let Some(&(parent_id, content_frame_id)) =
                    child_webview_to_parent.get(&frame.traversable_id)
                {
                    (parent_id, content_frame_id, false)
                } else {
                    (frame.traversable_id, frame.frame_id, true)
                };
            let webview_id = target_webview_id;
            let slot = webviews
                .entry(webview_id)
                .or_insert_with(|| WebviewState::new(channels.clone()));
            let composition = frame.composition.clone();
            let viewport_width = frame.viewport_width;
            let viewport_height = frame.viewport_height;
            let frame_id = actual_frame_id;
            let animating = frame.animating;
            let recorded_scene = match slot.compositor.decode_frame(frame, shmem_regions) {
                Ok(scene) => scene,
                Err(error) => {
                    error!("[graphics] deserialize paint frame: {error}");
                    return false;
                }
            };
            info!(
                "[render-pipe] Graphics store frame id={} webview={} root_candidate={} viewport={}x{} children={}",
                frame_id.0,
                webview_id.0,
                is_root_candidate,
                viewport_width,
                viewport_height,
                composition.embed_sites.len()
            );
            slot.compositor.store_frame(
                frame_id,
                viewport_width,
                viewport_height,
                composition,
                recorded_scene,
                is_root_candidate,
            );
            // Compose whenever the root frame arrives, even at a zero-sized
            // viewport: the frame is rendered at a minimum size so the UA's
            // rendering cycle always completes (a skipped frame would leave
            // the cycle open and stall all future renders).
            // Only compose and produce a texture when the top-level (root)
            // frame arrives. Child PaintFrames and video frames are buffered
            // locally — their LATEST data is included when the root triggers
            // composition. This ensures exactly one texture per render cycle
            // regardless of how many embedded frames exist.
            let should_compose = is_root_candidate;
            if should_compose {
                info!(
                    "[render-pipe] Graphics compose scene webview={} root_frame={}",
                    webview_id.0, frame_id.0
                );
                if let Some(mut composed) = slot.compositor.compose_scene(webview_id) {
                    // Populate child data for the UA to publish and route.
                    let (cv, cftw) =
                        build_child_data(&mut slot.compositor, child_webview_to_parent);
                    composed.child_viewports = cv;
                    composed.child_frame_to_webview = cftw;
                    // animating comes from the content-process PaintFrame flag.
                    // Content knows what's animating (video, CSS animations)
                    // and sets this. Graphics just passes it through.
                    composed.animating = animating;
                    match slot.renderer.submit_scene(composed) {
                        Ok(submit) => {
                            verification::tla_log!(
                                *tla_tracer,
                                -> "GPURendering",
                                "SurfaceFrameSubmitted",
                                webview_id.0,
                                submit.generation,
                                format!("{}x{}", submit.width, submit.height),
                                submit.buffer_index
                            );
                        }
                        Err(RenderError::Deferred) => {}
                        Err(RenderError::Failed) => {
                            error!(
                                "[graphics] submit composed scene failed for {:?}",
                                webview_id
                            );
                        }
                    }
                }
            }
        }
        GraphicsCommand::RemoveVideoFrame {
            webview_id,
            paint_id,
        } => {
            if let Some(slot) = webviews.get_mut(&webview_id) {
                slot.compositor.remove_video_frame(paint_id);
            }
        }
        GraphicsCommand::TextureConsumed {
            webview_id,
            generation,
        } => {
            let Some(slot) = webviews.get_mut(&webview_id) else {
                debug!(
                    "[graphics] texture consumed for unknown webview {:?}",
                    webview_id
                );
                return false;
            };
            if !slot.renderer.ack(generation) {
                debug!(
                    "[graphics] texture consumed for unknown generation {} (webview={:?})",
                    generation, webview_id
                );
            } else {
                debug!(
                    "[graphics] texture consumed webview={:?} gen={}",
                    webview_id.0, generation
                );
                verification::tla_log!(
                    *tla_tracer,
                    -> "GPURendering",
                    "TextureConsumed",
                    webview_id.0,
                    generation
                );
            }
            // A composed scene deferred for lack of a free buffer can now be
            // submitted: an ack freed a buffer.
            if let Some(submit) = slot.renderer.submit_deferred() {
                verification::tla_log!(
                    *tla_tracer,
                    -> "GPURendering",
                    "SurfaceFrameSubmitted",
                    webview_id.0,
                    submit.generation,
                    format!("{}x{}", submit.width, submit.height),
                    submit.buffer_index
                );
            }
        }
        GraphicsCommand::RegisterChildNavigableHost {
            child_webview_id,
            parent_traversable_id,
            content_frame_id,
        } => {
            child_webview_to_parent
                .insert(child_webview_id, (parent_traversable_id, content_frame_id));
        }
        GraphicsCommand::ChildNavigationFinalized {
            parent_traversable_id,
            content_frame_id,
        } => {
            if let Some(slot) = webviews.get_mut(&parent_traversable_id) {
                slot.compositor
                    .note_child_navigation_finalized(content_frame_id);
            }
        }
        GraphicsCommand::NavigationFinalized { webview_id } => {
            if let Some(slot) = webviews.get_mut(&webview_id) {
                slot.compositor.note_navigation_finalized();
            }
        }
        GraphicsCommand::CreateMediaPipeline {
            pipeline_id,
            url,
            webview_id,
            video_paint_id,
        } => {
            debug!(
                "[graphics:media] create pipeline {:?} url={} webview={:?} paint={:?}",
                pipeline_id, url, webview_id, video_paint_id
            );
            pipeline_webview_map.insert(pipeline_id, (webview_id, video_paint_id));
            if let Some(backend) = media_backend {
                match backend.create_pipeline(pipeline_id, url) {
                    Ok(pipeline) => {
                        pipelines.insert(pipeline_id, pipeline);
                    }
                    Err(e) => error!("[graphics:media] create failed: {e}"),
                }
            }
        }
        GraphicsCommand::MediaPlay { pipeline_id } => {
            if let Some(p) = pipelines.get(&pipeline_id)
                && let Err(e) = p.play()
            {
                error!("[graphics:media] play: {e}");
            }
        }
        GraphicsCommand::MediaPause { pipeline_id } => {
            if let Some(p) = pipelines.get(&pipeline_id)
                && let Err(e) = p.pause()
            {
                error!("[graphics:media] pause: {e}");
            }
        }
        GraphicsCommand::MediaSeek {
            pipeline_id,
            position_secs,
        } => {
            if let Some(p) = pipelines.get(&pipeline_id)
                && let Err(e) = p.seek(position_secs)
            {
                error!("[graphics:media] seek: {e}");
            }
        }
        GraphicsCommand::MediaDestroy { pipeline_id } => {
            if let Some(p) = pipelines.remove(&pipeline_id)
                && let Err(e) = p.destroy()
            {
                error!("[graphics:media] destroy: {e}");
            }
        }

        GraphicsCommand::SetTraceSender(sender) => {
            tla_tracer.set_sender(sender);
        }
        GraphicsCommand::Shutdown => return true,
    }
    false
}

/// Extract child frame data from the compositor and match against the
/// child_webview_to_parent mapping. Returns (child_viewports, child_frame_to_webview).
fn build_child_data(
    compositor: &mut Compositor,
    child_webview_to_parent: &HashMap<WebviewId, (WebviewId, FrameId)>,
) -> (HashMap<WebviewId, [f64; 4]>, HashMap<FrameId, WebviewId>) {
    let mut viewports = HashMap::new();
    let mut frame_to_webview = HashMap::new();
    // Build a reverse lookup: content_frame_id -> child_webview_id
    let frame_to_child: HashMap<FrameId, WebviewId> = child_webview_to_parent
        .iter()
        .map(|(child, &(_, content_fid))| (content_fid, *child))
        .collect();
    for vp in compositor.visible_frame_viewports() {
        let Some(&child_wv) = frame_to_child.get(&vp.frame_id) else {
            continue;
        };
        frame_to_webview.insert(vp.frame_id, child_wv);
        viewports.insert(
            child_wv,
            [
                f64::from(vp.offset_x),
                f64::from(vp.offset_y),
                f64::from(vp.offset_x) + f64::from(vp.width),
                f64::from(vp.offset_y) + f64::from(vp.height),
            ],
        );
    }
    (viewports, frame_to_webview)
}

/// Deliver a completed frame: look up the webview's renderer, hand the
/// backend's `RenderData` to its `handle_render_done` (which marks the ring
/// buffer pending and sends `PixelFrameReady`), then emit the trace events
/// the delivery reports.
fn handle_render_done<R: SurfaceRenderer>(
    webviews: &mut HashMap<WebviewId, WebviewState<R>>,
    sender: &ipc::IpcSender<GraphicsEvent>,
    tla_tracer: &mut TLATracer,
    done: R::RenderData,
) {
    let webview_id = R::render_done_webview_id(&done);
    let Some(slot) = webviews.get_mut(&webview_id) else {
        debug!(
            "[graphics] render done for unknown webview {:?}",
            webview_id
        );
        return;
    };
    let delivery = slot.renderer.handle_render_done(done, sender);
    if delivery.surface_frame_sent {
        verification::tla_log!(
            *tla_tracer,
            -> "GPURendering",
            "SurfaceFrameSent",
            webview_id.0,
            delivery.generation,
            format!("{}x{}", delivery.width, delivery.height),
            delivery.buffer_index
        );
    }
    // The graphical output for the webview is done: the pixels were sent.
    // Traced with the webview's navigable id so it matches the per-frame
    // RenderingOpportunity model (the webview id is the root navigable).
    if delivery.graphics_computed {
        verification::tla_log!(
            *tla_tracer,
            -> "RenderingOpportunity",
            "GraphicsComputed",
            webview_id.0
        );
    }
}
