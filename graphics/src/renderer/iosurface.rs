//! Zero-copy IOSurface surface backend (the default on macOS): renders Vello
//! directly into a shared IOSurface texture from the webview's ring; the
//! embedder imports the same surface and blits it, with no readback and no
//! IPC pixel bytes.

use super::{
    FrameDelivery, FrameMetadata, GpuContext, PollRequest, ReadbackChannels, RenderError,
    SurfaceBuffers, SurfaceRenderer, SurfaceRingState, frame_metadata, render_size,
};
use crate::iosurface::{IosurfaceTexture, create_shared_texture};
use ipc_messages::content::WebviewId;
use ipc_messages::graphics::{GraphicsEvent, SurfacePayload};
use ipc_messages::media::VideoPaintId;
use log::{debug, error, info};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_video::CVPixelBuffer;
use objc2_metal::MTLDevice;

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
/// shared IOSurface double buffer and the shared-texture id counter.
pub struct IosurfaceRenderer {
    gpu: GpuContext,
    channels: ReadbackChannels<SharedRenderData>,
    /// The webview's shared IOSurface double buffer (two textures),
    /// reallocated on resize.
    buffers: Option<SurfaceBuffers<[IosurfaceTexture; 2]>>,
    /// Monotonic identity for shared IOSurface textures; changes on resize.
    texture_id_counter: u64,
}

impl IosurfaceRenderer {
    /// Allocate the two shared IOSurface textures for the double buffer,
    /// using `first_texture_id` as the id of the first texture.
    fn allocate_textures(
        renderer: &IosurfaceRenderer,
        width: u32,
        height: u32,
        first_texture_id: u64,
    ) -> Option<[IosurfaceTexture; 2]> {
        Some([
            create_shared_texture(renderer, width, height, first_texture_id)?,
            create_shared_texture(renderer, width, height, first_texture_id + 1)?,
        ])
    }

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
        })
    }

    fn submit_scene(&mut self, composed: ComposedScene) -> Result<(), RenderError> {
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
            let first_texture_id = self.texture_id_counter;
            let payload = Self::allocate_textures(self, width, height, first_texture_id)
                .ok_or_else(|| {
                    error!(
                        "[graphics] allocate shared textures {}x{}: failed",
                        width, height
                    );
                    RenderError::Failed
                })?;
            self.texture_id_counter += 2;
            self.buffers = Some(SurfaceBuffers::new(
                SurfaceRingState::new(width, height),
                payload,
            ));
        }
        let buffers = self.buffers.as_mut().ok_or(RenderError::Failed)?;
        // Double buffering: each cycle renders into the buffer the last
        // render did not use. The embedder's FrameNeeded pacing guarantees
        // that buffer is free (it holds the frame from two cycles ago).
        let buffer_index = buffers.next_buffer();

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

        Ok(())
    }

    fn handle_render_done(
        &mut self,
        data: SharedRenderData,
        sender: &ipc::IpcSender<GraphicsEvent>,
    ) -> FrameDelivery {
        let SharedRenderData {
            webview_id,
            generation,
            width,
            height,
            buffer_index,
            metadata,
        } = data;
        let mut delivery = FrameDelivery {
            graphics_computed: false,
        };
        let Some(buffers) = self.buffers.as_mut() else {
            error!(
                "[graphics] no surface buffers for render done {:?}",
                webview_id
            );
            return delivery;
        };
        let Some(texture) = buffers.payload().get(buffer_index) else {
            error!(
                "[graphics] bad buffer index {} for render done {:?} gen={}",
                buffer_index, webview_id, generation
            );
            return delivery;
        };
        let texture_id = texture.texture_id;
        let port = texture.port_for_frame();

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
            return delivery;
        }
        delivery.graphics_computed = true;
        delivery
    }

    fn render_done_webview_id(data: &SharedRenderData) -> WebviewId {
        data.webview_id
    }

    fn import_video_frame(
        &mut self,
        paint_id: VideoPaintId,
        pixel_buffer: &Retained<CVPixelBuffer>,
        width: u32,
        height: u32,
    ) -> Option<peniko::ImageData> {
        self.gpu
            .import_video_frame(paint_id, pixel_buffer, width, height)
    }
}
