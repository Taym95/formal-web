//! Zero-copy IOSurface surface backend (the default on macOS): renders Vello
//! directly into a shared IOSurface texture from the webview's ring; the
//! embedder imports the same surface and blits it, with no readback and no
//! IPC pixel bytes.

use super::{
    FrameMetadata, GpuContext, PollRequest, ReadbackChannels, RenderError, SurfaceBuffers,
    SurfaceRenderer, SurfaceRingState, frame_metadata, render_size,
};
use ipc_messages::content::WebviewId;
use ipc_messages::graphics::{GraphicsEvent, SurfacePayload};
use log::{debug, error, info};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_video::CVPixelBuffer;
use objc2_metal::MTLDevice;
use verification::TLATracer;

use crate::ComposedScene;

/// Per-frame data for the zero-copy path: delivered by the poll thread once
/// the render into the shared texture completes (the embedder's blit of the
/// shared surface is then GPU-safe).
pub struct SharedRenderData {
    pub webview_id: WebviewId,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub buffer_index: usize,
    pub metadata: FrameMetadata,
}

/// The zero-copy IOSurface renderer: a [`GpuContext`] plus the webview's
/// shared IOSurface ring and the shared-texture id counter.
pub struct IosurfaceRenderer {
    gpu: GpuContext,
    channels: ReadbackChannels<SharedRenderData>,
    /// The webview's shared IOSurface ring (three textures), reallocated on
    /// resize.
    buffers: Option<SurfaceBuffers<[crate::iosurface::IosurfaceTexture; 3]>>,
    /// Monotonic identity for shared IOSurface textures; changes on resize.
    texture_id_counter: u64,
    /// The most recent composed scene that could not be submitted because
    /// every ring buffer was still awaiting the embedder's ack.
    deferred_scene: Option<ComposedScene>,
}

impl IosurfaceRenderer {
    /// The wgpu device this renderer composites with (also used to create
    /// the shared IOSurface textures).
    pub(crate) fn device(&self) -> &wgpu::Device {
        &self.gpu.device_handle.device
    }

    /// The raw Metal device backing this renderer, needed to create
    /// IOSurface-backed Metal textures.
    pub(crate) fn raw_metal_device(&self) -> Option<Retained<ProtocolObject<dyn MTLDevice>>> {
        self.gpu.raw_metal_device()
    }
}

impl SurfaceRenderer for IosurfaceRenderer {
    type RenderData = SharedRenderData;

    fn new(channels: ReadbackChannels<SharedRenderData>) -> Result<Self, String> {
        Ok(Self {
            gpu: GpuContext::new()?,
            channels,
            buffers: None,
            texture_id_counter: 1,
            deferred_scene: None,
        })
    }

    fn submit_scene(
        &mut self,
        composed: ComposedScene,
        tla_tracer: &mut TLATracer,
    ) -> Result<(), RenderError> {
        let ComposedScene {
            webview_id,
            scene,
            frame_hit_info,
            child_viewports,
            child_frame_to_webview,
            animating,
        } = composed;
        let (width, height) = render_size(&frame_hit_info);
        info!(
            "[render-pipe] Graphics GPU render webview={} {}x{} {} child_frames animating={}",
            webview_id.0,
            width,
            height,
            child_viewports.len(),
            animating,
        );

        // Reuse the per-webview frame buffers across frames, reallocating
        // only when the viewport size changes.
        let needs_new = self
            .buffers
            .as_ref()
            .is_none_or(|buffers| buffers.ring().width != width || buffers.ring().height != height);
        if needs_new {
            let mut textures: [Option<crate::iosurface::IosurfaceTexture>; 3] = [None, None, None];
            for slot in textures.iter_mut() {
                let texture = crate::iosurface::create_shared_texture(
                    self,
                    width,
                    height,
                    self.texture_id_counter,
                )
                .ok_or_else(|| {
                    error!(
                        "[graphics] allocate shared textures {}x{}: failed",
                        width, height
                    );
                    RenderError::Failed
                })?;
                self.texture_id_counter += 1;
                *slot = Some(texture);
            }
            let payload = textures.map(|texture| texture.expect("filled above"));
            self.buffers = Some(SurfaceBuffers::new(
                SurfaceRingState::new(width, height),
                payload,
            ));
        }
        let buffers = self.buffers.as_mut().ok_or(RenderError::Failed)?;
        let Some(buffer_index) = buffers.next_free() else {
            // Every buffer is reserved or awaiting the embedder's ack: hold
            // the composed scene and submit it once a buffer frees. This
            // keeps the rendering-opportunity cycle alive instead of
            // dropping the frame.
            info!(
                "[render-pipe] Graphics defer scene webview={} (all {} buffers busy)",
                webview_id.0, 3
            );
            self.deferred_scene = Some(ComposedScene {
                webview_id,
                scene,
                frame_hit_info,
                child_viewports,
                child_frame_to_webview,
                animating,
            });
            return Err(RenderError::Deferred);
        };

        let metadata = frame_metadata(
            webview_id,
            frame_hit_info,
            child_viewports,
            child_frame_to_webview,
            animating,
        );

        // The shared target texture comes from the webview's IOSurface ring,
        // selected by `buffer_index`.
        self.gpu.mark_video_textures_dirty();
        let target = &buffers.payload()[buffer_index].texture;
        if let Err(error) = self.gpu.render_into(&scene, target, width, height) {
            error!("[gpu-renderer] {error}");
            return Err(RenderError::Failed);
        }

        self.gpu.generation += 1;
        let generation = self.gpu.generation;
        // Vello's render_to_texture submits internally; waiting for "all
        // submitted work" (submission_index: None) covers that submission.
        let done = SharedRenderData {
            webview_id,
            generation,
            width,
            height,
            buffer_index,
            metadata,
        };
        if let Err(send_error) = self.channels.poll_tx.send(PollRequest {
            device: self.gpu.device_handle.clone(),
            submission_index: None,
            done: Some(done),
        }) {
            error!("[gpu-renderer] failed to queue shared render poll: {send_error}");
        }
        debug!(
            "[gpu-renderer] rendered into shared texture {}x{} gen={} buffer={}",
            width, height, generation, buffer_index
        );
        buffers.reserve(buffer_index, generation);

        verification::tla_log!(
            *tla_tracer,
            -> "GPURendering",
            "SurfaceFrameSubmitted",
            webview_id.0,
            generation,
            format!("{}x{}", width, height),
            buffer_index
        );
        Ok(())
    }

    fn handle_render_done(
        &mut self,
        data: SharedRenderData,
        sender: &ipc::IpcSender<GraphicsEvent>,
        tla_tracer: &mut TLATracer,
    ) {
        let SharedRenderData {
            webview_id,
            generation,
            width,
            height,
            buffer_index,
            metadata,
        } = data;
        let Some(buffers) = self.buffers.as_mut() else {
            error!(
                "[graphics] no surface buffers for render done {:?}",
                webview_id
            );
            return;
        };
        let Some(texture) = buffers.payload().get(buffer_index) else {
            error!(
                "[graphics] bad buffer index {} for render done {:?} gen={}",
                buffer_index, webview_id, generation
            );
            return;
        };
        let texture_id = texture.texture_id;
        let port = texture.port_for_frame();
        buffers.ring_mut().mark_pending(buffer_index, generation);

        verification::tla_log!(
            *tla_tracer,
            -> "GPURendering",
            "SurfaceFrameSent",
            webview_id.0,
            generation,
            format!("{}x{}", width, height),
            buffer_index
        );

        let frame_event = GraphicsEvent::PixelFrameReady {
            webview_id,
            payload: SurfacePayload::SharedTexture { texture_id, port },
            animating: metadata.animating,
            width,
            height,
            generation,
            frame_hit_info: metadata.frame_hit_info,
            child_viewports: metadata.child_viewports,
            child_frame_to_webview: metadata.child_frame_to_webview,
        };
        if let Err(send_error) = sender.send(frame_event) {
            error!(
                "[graphics] failed to send PixelFrameReady for {:?} gen={}: {send_error}",
                webview_id, generation
            );
            return;
        }

        verification::tla_log!(
            *tla_tracer,
            -> "RenderingOpportunity",
            "GraphicsComputed",
            webview_id.0
        );
    }

    fn render_done_webview_id(data: &SharedRenderData) -> WebviewId {
        data.webview_id
    }

    fn ack(&mut self, generation: u64) -> bool {
        self.buffers
            .as_mut()
            .is_some_and(|buffers| buffers.ack(generation))
    }

    fn submit_deferred(&mut self, tla_tracer: &mut TLATracer) -> bool {
        let Some(composed) = self.deferred_scene.take() else {
            return false;
        };
        let webview_id = composed.webview_id;
        if let Err(error) = self.submit_scene(composed, tla_tracer) {
            match error {
                RenderError::Deferred => {}
                RenderError::Failed => {
                    error!(
                        "[graphics] submit deferred scene failed for {:?}",
                        webview_id
                    );
                }
            }
        }
        true
    }

    fn import_video_frame(
        &mut self,
        paint_id: ipc_messages::media::VideoPaintId,
        pixel_buffer: &Retained<CVPixelBuffer>,
        width: u32,
        height: u32,
    ) -> Option<peniko::ImageData> {
        self.gpu
            .import_video_frame(paint_id, pixel_buffer, width, height)
    }
}
