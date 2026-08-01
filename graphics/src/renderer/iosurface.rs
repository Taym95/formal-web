//! Zero-copy IOSurface surface backend (the default on macOS): renders Vello
//! directly into a shared IOSurface texture from the webview's ring; the
//! embedder imports the same surface and blits it, with no readback and no
//! IPC pixel bytes.

use super::{FrameMetadata, GpuRenderer, PollRequest, RenderSubmit, SurfaceRenderer};
use anyrender::PaintScene;
use ipc_messages::content::WebviewId;
use ipc_messages::graphics::{GraphicsEvent, SurfacePayload};
use kurbo::Affine;
use log::{debug, error};
use vello::{AaConfig, RenderParams};
use wgpu::TextureViewDescriptor;

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

impl SurfaceRenderer for GpuRenderer {
    type RenderData = SharedRenderData;

    fn render(
        &mut self,
        scene: &anyrender::Scene,
        width: u32,
        height: u32,
        buffers: &mut crate::SurfaceBuffers,
        buffer_index: usize,
        metadata: FrameMetadata,
    ) -> Option<RenderSubmit> {
        let (width, height) = (width.max(1), height.max(1));
        self.mark_video_textures_dirty();

        self.vello_scene.reset();
        {
            let mut painter = anyrender_vello::VelloScenePainter::new(&mut self.vello_scene);
            painter.append_scene(scene.clone(), Affine::IDENTITY);
        }

        // The shared target texture comes from the webview's IOSurface ring,
        // selected by `buffer_index`.
        let crate::SurfaceBuffers::Iosurface(buffers) = buffers;
        let target = &buffers.textures[buffer_index].texture;
        let view = target.create_view(&TextureViewDescriptor::default());
        if let Err(e) = self.vello_renderer.render_to_texture(
            &self.device_handle.device,
            &self.device_handle.queue,
            &self.vello_scene,
            &view,
            &RenderParams {
                base_color: vello::peniko::Color::TRANSPARENT,
                width,
                height,
                antialiasing_method: AaConfig::Area,
            },
        ) {
            error!(
                "[gpu-renderer] Vello render into shared texture failed: {:?}",
                e
            );
            return None;
        }

        self.generation += 1;
        let generation = self.generation;
        // Vello's render_to_texture submits internally; waiting for "all
        // submitted work" (submission_index: None) covers that submission.
        let done = SharedRenderData {
            webview_id: metadata.webview_id,
            generation,
            width,
            height,
            buffer_index,
            metadata,
        };
        if let Err(send_error) = self.channels.poll_tx.send(PollRequest {
            device: self.device_handle.clone(),
            submission_index: None,
            done: Some(done),
        }) {
            error!("[gpu-renderer] failed to queue shared render poll: {send_error}");
        }
        debug!(
            "[gpu-renderer] rendered into shared texture {}x{} gen={} buffer={}",
            width, height, generation, buffer_index
        );
        Some(RenderSubmit { generation })
    }

    fn handle_render_done(
        &mut self,
        data: SharedRenderData,
        buffers: &mut crate::SurfaceBuffers,
        sender: &ipc::IpcSender<GraphicsEvent>,
        tla_tracer: &mut verification::TLATracer,
    ) {
        let crate::SurfaceBuffers::Iosurface(buffers) = buffers;
        let SharedRenderData {
            webview_id,
            generation,
            width,
            height,
            buffer_index,
            metadata,
        } = data;
        let Some(texture) = buffers.textures.get(buffer_index) else {
            error!(
                "[graphics] bad buffer index {} for render done {:?} gen={}",
                buffer_index, webview_id, generation
            );
            return;
        };
        buffers.ring.mark_pending(buffer_index, generation);

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
            payload: SurfacePayload::SharedTexture {
                texture_id: texture.texture_id,
                port: texture.port_for_frame(),
            },
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
}
