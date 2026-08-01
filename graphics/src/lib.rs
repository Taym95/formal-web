pub mod compositor;
#[cfg(target_os = "macos")]
pub mod iosurface;
pub mod renderer;

use std::collections::HashMap;

use crate::renderer::SurfaceRenderer;
use compositor::{Compositor, CompositorVideoFrame};
use crossbeam_channel::{select, tick};
use ipc_messages::content::{FontTransportReceiver, FrameId, WebviewId};
use ipc_messages::graphics::{FrameHitInfo, GraphicsCommand, GraphicsEvent};
use ipc_messages::media::{MediaPipelineId, VideoPaintId};
use log::{debug, error, info};
use verification::TLATracer;

use media::backend::{MediaBackend, MediaBackendEvent, PipelineHandle};

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

struct WebviewCompositorSlot {
    compositor: Compositor,
    gpu_renderer: crate::renderer::GpuRenderer,
    font_receiver: FontTransportReceiver,
    child_frame_to_parent: HashMap<FrameId, WebviewId>,
    /// Persistent per-webview frame buffers, a three-slot ring. A buffer is
    /// only written when the embedder has acked the previous frame that used
    /// it (TextureConsumed); otherwise the composed scene is deferred here
    /// until a buffer frees up.
    surface_buffers: Option<SurfaceBuffers>,
    /// Monotonic identity for shared IOSurface textures; changes on resize.
    #[cfg(target_os = "macos")]
    iosurface_texture_id_counter: u64,
    /// The most recent composed scene that could not be rendered because
    /// every surface buffer was still awaiting the embedder's ack.
    deferred_scene: Option<ComposedScene>,
}

/// State of one surface buffer in the ring.
#[derive(Debug, Clone, Copy, PartialEq)]
enum BufferState {
    /// Free to be picked for a new frame's submission.
    Free,
    /// Picked at submit time; the render is in flight and the pixels have
    /// not been delivered yet.
    Reserved(u64),
    /// Delivered (PixelFrameReady sent); awaiting the embedder's ack.
    Pending(u64),
}

/// The ring lifecycle shared by both backends: a buffer is picked at submit
/// time (Reserved), becomes Pending when the pixels are delivered, and is
/// freed by the embedder's TextureConsumed ack. The graphics process never
/// writes a buffer that is Reserved or Pending, so the embedder is
/// guaranteed to have consumed the previous frame's pixels before the buffer
/// is reused.
struct SurfaceRingState {
    state: [BufferState; 3],
    /// Ring index where the next free buffer search starts.
    write_index: usize,
    width: u32,
    height: u32,
}

impl SurfaceRingState {
    fn new(width: u32, height: u32) -> Self {
        Self {
            state: [BufferState::Free, BufferState::Free, BufferState::Free],
            write_index: 0,
            width,
            height,
        }
    }

    /// Index of the next free buffer to pick, scanning the ring from
    /// `write_index`; None when every buffer is reserved or pending.
    fn next_free(&self) -> Option<usize> {
        (0..3)
            .map(|offset| (self.write_index + offset) % 3)
            .find(|index| self.state[*index] == BufferState::Free)
    }

    /// Mark the buffer picked for a submitted frame.
    fn reserve(&mut self, index: usize, generation: u64) {
        debug_assert!(self.state[index] == BufferState::Free);
        self.state[index] = BufferState::Reserved(generation);
        self.write_index = (index + 1) % 3;
    }

    /// Move the buffer from Reserved to Pending once its pixels are delivered.
    fn mark_pending(&mut self, index: usize, generation: u64) {
        debug_assert!(self.state[index] == BufferState::Reserved(generation));
        self.state[index] = BufferState::Pending(generation);
    }

    /// Free the buffer holding `generation` (the embedder's ack). Returns
    /// true when a pending buffer was found and freed.
    fn ack(&mut self, generation: u64) -> bool {
        for index in 0..3 {
            if self.state[index] == BufferState::Pending(generation) {
                self.state[index] = BufferState::Free;
                return true;
            }
        }
        false
    }
}

/// Three shared-memory pixel buffers per webview used as a ring (CPU path,
/// non-macOS).
#[cfg(not(target_os = "macos"))]
struct SurfaceShmemBuffers {
    regions: [ipc::IpcSharedRegion; 3],
    ring: SurfaceRingState,
}

#[cfg(not(target_os = "macos"))]
impl SurfaceShmemBuffers {
    fn allocate(width: u32, height: u32) -> Result<Self, ipc::IpcError> {
        let byte_count = (width as usize) * (height as usize) * 4;
        let region_zero = ipc::IpcSharedRegion::allocate(byte_count)?;
        let region_one = ipc::IpcSharedRegion::allocate(byte_count)?;
        let region_two = ipc::IpcSharedRegion::allocate(byte_count)?;
        Ok(Self {
            regions: [region_zero, region_one, region_two],
            ring: SurfaceRingState::new(width, height),
        })
    }
}

/// Three shared IOSurface textures per webview used as a ring (macOS
/// zero-copy path).
#[cfg(target_os = "macos")]
struct IosurfaceBuffers {
    textures: [iosurface::IosurfaceTexture; 3],
    ring: SurfaceRingState,
}

#[cfg(target_os = "macos")]
impl IosurfaceBuffers {
    fn allocate(
        renderer: &crate::renderer::GpuRenderer,
        width: u32,
        height: u32,
        texture_id_counter: &mut u64,
    ) -> Option<Self> {
        let texture_zero =
            iosurface::create_shared_texture(renderer, width, height, *texture_id_counter)?;
        *texture_id_counter += 1;
        let texture_one =
            iosurface::create_shared_texture(renderer, width, height, *texture_id_counter)?;
        *texture_id_counter += 1;
        let texture_two =
            iosurface::create_shared_texture(renderer, width, height, *texture_id_counter)?;
        *texture_id_counter += 1;
        Some(Self {
            textures: [texture_zero, texture_one, texture_two],
            ring: SurfaceRingState::new(width, height),
        })
    }
}

/// The per-webview frame buffers, one implementation per backend. The
/// variant — and with it the whole delivery path — is chosen at compile time:
/// the CPU readback path off macOS, the zero-copy IOSurface path on macOS.
/// The zero-copy variant is boxed to keep the enum small (each texture holds
/// a wgpu texture plus the IOSurface and its Mach port).
enum SurfaceBuffers {
    #[cfg(not(target_os = "macos"))]
    Cpu(SurfaceShmemBuffers),
    #[cfg(target_os = "macos")]
    Iosurface(Box<IosurfaceBuffers>),
}

impl SurfaceBuffers {
    fn ring(&self) -> &SurfaceRingState {
        match self {
            #[cfg(not(target_os = "macos"))]
            SurfaceBuffers::Cpu(buffers) => &buffers.ring,
            #[cfg(target_os = "macos")]
            SurfaceBuffers::Iosurface(buffers) => &buffers.ring,
        }
    }

    fn ring_mut(&mut self) -> &mut SurfaceRingState {
        match self {
            #[cfg(not(target_os = "macos"))]
            SurfaceBuffers::Cpu(buffers) => &mut buffers.ring,
            #[cfg(target_os = "macos")]
            SurfaceBuffers::Iosurface(buffers) => &mut buffers.ring,
        }
    }

    fn next_free(&self) -> Option<usize> {
        self.ring().next_free()
    }

    fn reserve(&mut self, index: usize, generation: u64) {
        self.ring_mut().reserve(index, generation);
    }

    fn ack(&mut self, generation: u64) -> bool {
        self.ring_mut().ack(generation)
    }
}

impl WebviewCompositorSlot {
    fn new(channels: crate::renderer::ReadbackChannels) -> Self {
        Self {
            compositor: Compositor::default(),
            gpu_renderer: match crate::renderer::GpuRenderer::new(channels) {
                Ok(r) => r,
                Err(e) => panic!("GpuRenderer init: {e}"),
            },
            font_receiver: FontTransportReceiver::default(),
            child_frame_to_parent: HashMap::new(),
            surface_buffers: None,
            #[cfg(target_os = "macos")]
            iosurface_texture_id_counter: 1,
            deferred_scene: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisibleFrameViewport {
    pub frame_id: FrameId,
    pub offset_x: f32,
    pub offset_y: f32,
    pub width: u32,
    pub height: u32,
}

/// Run the graphics process event loop.
/// The media backend (if provided) runs directly in this loop — no separate IPC.
/// The pipeline_to_webview mapping is managed via RegisterMediaPipeline from content.
pub fn run_graphics_process<B: MediaBackend + 'static>(
    cmd_rx: crossbeam_channel::Receiver<ipc::IpcIncoming<GraphicsCommand>>,
    graphics_event_tx: ipc::IpcSender<GraphicsEvent>,
    media_backend: Option<B>,
) {
    let mut webviews: HashMap<WebviewId, WebviewCompositorSlot> = HashMap::new();
    let event_sender = graphics_event_tx;

    // Readback plumbing created once at startup: a native poll thread blocks
    // on device.poll(Wait) for each submitted frame (firing the readback map
    // callbacks on the CPU path), so the select! loop never has to poll at an
    // interval or busy-wait. Completed frames arrive on the single
    // `render_done` channel, whose message type is the backend's RenderData
    // (chosen at compile time).
    let (poll_tx, poll_rx) = crossbeam_channel::unbounded::<crate::renderer::PollRequest>();
    let (render_done_tx, render_done_rx) =
        crossbeam_channel::unbounded::<crate::renderer::RenderDone>();
    let channels = crate::renderer::ReadbackChannels {
        poll_tx,
        render_done_tx,
    };
    let render_done_tx = channels.render_done_tx.clone();
    let poll_thread = std::thread::Builder::new()
        .name(String::from("formal-web:gpu-poll"))
        .spawn(move || {
            let render_done_tx = render_done_tx;
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
                if let Some(done) = request.done {
                    if let Err(send_error) = render_done_tx.send(done) {
                        error!("[graphics] failed to deliver render done: {send_error}");
                    }
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

    // Completed GPU readbacks arrive here from the poll thread's map
    // callbacks; the poll thread blocks on the device until a submission
    // completes.

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

fn handle_media_event(
    event: MediaBackendEvent,
    pipeline_webview_map: &HashMap<MediaPipelineId, (WebviewId, VideoPaintId)>,
    webviews: &mut HashMap<WebviewId, WebviewCompositorSlot>,
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
        #[cfg(target_os = "macos")]
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
            let Some(resource_id) = slot.gpu_renderer.import_video_frame(
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

fn handle_command<B: MediaBackend + 'static>(
    cmd: GraphicsCommand,
    webviews: &mut HashMap<WebviewId, WebviewCompositorSlot>,
    shmem_regions: &HashMap<usize, ipc::IpcSharedRegion>,
    pipelines: &mut HashMap<MediaPipelineId, B::Pipeline>,
    pipeline_webview_map: &mut HashMap<MediaPipelineId, (WebviewId, VideoPaintId)>,
    media_backend: Option<&mut B>,
    child_webview_to_parent: &mut HashMap<WebviewId, (WebviewId, FrameId)>,
    tla_tracer: &mut TLATracer,
    channels: &crate::renderer::ReadbackChannels,
) -> bool {
    match cmd {
        GraphicsCommand::RegisterWebview { webview_id } => {
            debug!("[graphics] registering webview {:?}", webview_id);
            webviews
                .entry(webview_id)
                .or_insert_with(|| WebviewCompositorSlot::new(channels.clone()));
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
                .or_insert_with(|| WebviewCompositorSlot::new(channels.clone()));
            let composition = frame.composition.clone();
            let viewport_width = frame.viewport_width;
            let viewport_height = frame.viewport_height;
            let frame_id = actual_frame_id;
            let animating = frame.animating;
            let recorded_scene =
                match frame.into_recorded_scene(&mut slot.font_receiver, shmem_regions) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("[graphics] deserialize paint frame: {e}");
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
                if let Some(mut composed) = slot
                    .compositor
                    .compose_scene(&slot.font_receiver, webview_id)
                {
                    // Populate child data for the UA to publish and route.
                    let (cv, cftw) = build_child_data(
                        &mut slot.compositor,
                        child_webview_to_parent,
                        &slot.font_receiver,
                    );
                    composed.child_viewports = cv;
                    composed.child_frame_to_webview = cftw;
                    // animating comes from the content-process PaintFrame flag.
                    // Content knows what's animating (video, CSS animations)
                    // and sets this. Graphics just passes it through.
                    composed.animating = animating;
                    if let Err(()) = submit_composed_scene(slot, composed, tla_tracer) {
                        error!(
                            "[graphics] submit composed scene failed for {:?}",
                            webview_id
                        );
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
            if !slot
                .surface_buffers
                .as_mut()
                .is_some_and(|buffers| buffers.ack(generation))
            {
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
            if slot.deferred_scene.is_some() {
                let composed = slot.deferred_scene.take().expect("checked above");
                if let Err(()) = submit_composed_scene(slot, composed, tla_tracer) {
                    error!(
                        "[graphics] submit deferred scene failed for {:?}",
                        webview_id
                    );
                }
            }
        }
        GraphicsCommand::RegisterChildNavigableHost {
            child_webview_id,
            parent_traversable_id,
            content_frame_id,
        } => {
            if let Some(slot) = webviews.get_mut(&parent_traversable_id) {
                slot.child_frame_to_parent
                    .insert(content_frame_id, parent_traversable_id);
            }
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
    font_receiver: &FontTransportReceiver,
) -> (HashMap<WebviewId, [f64; 4]>, HashMap<FrameId, WebviewId>) {
    let mut viewports = HashMap::new();
    let mut frame_to_webview = HashMap::new();
    // Build a reverse lookup: content_frame_id -> child_webview_id
    let frame_to_child: HashMap<FrameId, WebviewId> = child_webview_to_parent
        .iter()
        .map(|(child, &(_, content_fid))| (content_fid, *child))
        .collect();
    for vp in compositor.visible_frame_viewports(font_receiver) {
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

fn submit_composed_scene(
    slot: &mut WebviewCompositorSlot,
    composed: ComposedScene,
    tla_tracer: &mut verification::TLATracer,
) -> Result<(), ()> {
    let ComposedScene {
        webview_id,
        scene,
        frame_hit_info,
        child_viewports,
        child_frame_to_webview,
        animating,
    } = composed;

    // Render at a minimum size even when the viewport is zero-sized (e.g.
    // the window shrank below the chrome): a PixelFrameReady must always be
    // produced for a root frame, otherwise the UA's rendering cycle never
    // completes and all subsequent renders stall. The clamped size is used
    // for both the render and the message, so the embedder always receives
    // a valid frame to upload and ack.
    let width = frame_hit_info
        .first()
        .map(|h| h.viewport_width)
        .unwrap_or(0)
        .max(1);
    let height = frame_hit_info
        .first()
        .map(|h| h.viewport_height)
        .unwrap_or(0)
        .max(1);
    info!(
        "[render-pipe] Graphics GPU render webview={} {}x{} {} child_frames animating={}",
        webview_id.0,
        width,
        height,
        child_viewports.len(),
        animating,
    );

    // Reuse the per-webview frame buffers across frames, reallocating only
    // when the viewport size changes. The buffer is picked here (at submit
    // time) and reserved; the pixels are delivered later, when the GPU
    // completes the render. The buffer implementation — and with it the whole
    // delivery path — is chosen at compile time.
    let needs_new = slot
        .surface_buffers
        .as_ref()
        .is_none_or(|buffers| buffers.ring().width != width || buffers.ring().height != height);
    if needs_new {
        #[cfg(not(target_os = "macos"))]
        match SurfaceShmemBuffers::allocate(width, height) {
            Ok(buffers) => slot.surface_buffers = Some(SurfaceBuffers::Cpu(buffers)),
            Err(error) => {
                error!(
                    "[graphics] allocate surface shmem {}x{}: {error}",
                    width, height
                );
                return Err(());
            }
        }
        #[cfg(target_os = "macos")]
        match IosurfaceBuffers::allocate(
            &slot.gpu_renderer,
            width,
            height,
            &mut slot.iosurface_texture_id_counter,
        ) {
            Some(buffers) => {
                slot.surface_buffers = Some(SurfaceBuffers::Iosurface(Box::new(buffers)));
            }
            None => {
                error!(
                    "[graphics] allocate shared textures {}x{}: failed",
                    width, height
                );
                return Err(());
            }
        }
    }
    let buffers = slot.surface_buffers.as_mut().expect("allocated above");
    let Some(buffer_index) = buffers.next_free() else {
        // Every buffer is reserved or awaiting the embedder's ack: hold the
        // composed scene and submit it once a buffer frees. This keeps the
        // rendering-opportunity cycle alive instead of dropping the frame.
        info!(
            "[render-pipe] Graphics defer scene webview={} (all {} buffers busy)",
            webview_id.0, 3
        );
        slot.deferred_scene = Some(ComposedScene {
            webview_id,
            scene,
            frame_hit_info,
            child_viewports,
            child_frame_to_webview,
            animating,
        });
        return Ok(());
    };

    // Render and submit. The frame buffer was pre-selected above; its pixels
    // are delivered once the GPU completes the render. The renderer's backend
    // picks its target from `buffers` (the shared IOSurface texture on the
    // zero-copy path; the internal intermediate texture on the CPU path).
    let child_ports: Vec<ipc_messages::graphics::ChildViewport> = child_viewports
        .into_iter()
        .map(|(cwv, b)| ipc_messages::graphics::ChildViewport {
            child_webview_id: cwv,
            root_clip_bounds: b,
        })
        .collect();
    let metadata = crate::renderer::FrameMetadata {
        webview_id,
        frame_hit_info,
        child_viewports: child_ports,
        child_frame_to_webview,
        animating,
    };
    let submit = slot
        .gpu_renderer
        .render(&scene, width, height, buffers, buffer_index, metadata);
    let Some(submit) = submit else {
        error!("[graphics] render failed for {:?}", webview_id);
        return Err(());
    };
    buffers.reserve(buffer_index, submit.generation);

    verification::tla_log!(
        *tla_tracer,
        -> "GPURendering",
        "SurfaceFrameSubmitted",
        webview_id.0,
        submit.generation,
        format!("{}x{}", width, height),
        buffer_index
    );

    Ok(())
}

/// Deliver a completed frame: look up the webview's slot and hand the
/// backend's `RenderData` to the renderer's `handle_render_done`, which marks
/// the ring buffer pending and sends `PixelFrameReady`.
fn handle_render_done(
    webviews: &mut HashMap<WebviewId, WebviewCompositorSlot>,
    sender: &ipc::IpcSender<GraphicsEvent>,
    tla_tracer: &mut verification::TLATracer,
    done: crate::renderer::RenderDone,
) {
    let webview_id = crate::renderer::GpuRenderer::render_done_webview_id(&done);
    let Some(slot) = webviews.get_mut(&webview_id) else {
        debug!(
            "[graphics] render done for unknown webview {:?}",
            webview_id
        );
        return;
    };
    let Some(buffers) = slot.surface_buffers.as_mut() else {
        error!(
            "[graphics] no surface buffers for render done {:?}",
            webview_id
        );
        return;
    };
    slot.gpu_renderer
        .handle_render_done(done, buffers, sender, tla_tracer);
}
