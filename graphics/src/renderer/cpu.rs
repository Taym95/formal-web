//! CPU readback surface backend: renders Vello into an intermediate texture,
//! submits a GPU → CPU readback, and copies the pixels into the webview's
//! shared-memory ring once the readback completes. This is the backend off
//! macOS (GStreamer media backend) and on macOS when built with the
//! `cpu_readback` feature.

use super::{FrameMetadata, GpuRenderer, PollRequest, RenderSubmit, SurfaceRenderer};
use anyrender::PaintScene;
use ipc_messages::content::WebviewId;
use ipc_messages::graphics::{GraphicsEvent, SurfacePayload};
use kurbo::Affine;
use log::{debug, error};
use vello::{AaConfig, RenderParams};
use wgpu::{
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d, Origin3d,
    TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
};

/// The number of readback buffers kept per renderer; must be >= the number
/// of shared-memory surface buffers so each in-flight frame has its own
/// staging buffer.
pub const READBACK_SLOTS: usize = 3;

/// Per-frame data for the CPU readback path: delivered by the readback map
/// callback when the GPU completes the copy.
pub struct CpuRenderData {
    pub webview_id: WebviewId,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub shmem_index: usize,
    pub readback_index: usize,
    pub result: Result<(), wgpu::BufferAsyncError>,
    pub metadata: FrameMetadata,
}

impl GpuRenderer {
    fn ensure_render_tex(&mut self, width: u32, height: u32) {
        if self
            .render_tex
            .as_ref()
            .map(|(_, w, h)| *w == width && *h == height)
            .unwrap_or(false)
        {
            return;
        }
        let tex = self
            .device_handle
            .device
            .create_texture(&TextureDescriptor {
                label: Some("vello-intermediate"),
                size: Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba8Unorm,
                usage: TextureUsages::STORAGE_BINDING
                    | TextureUsages::TEXTURE_BINDING
                    | TextureUsages::COPY_SRC,
                view_formats: &[],
            });
        self.render_tex = Some((tex, width, height));
    }

    /// Drop the in-flight marker for a readback slot (map failure path).
    fn release_readback(&mut self, readback_index: usize) {
        if let Some(generation) = self.inflight_readbacks[readback_index].take() {
            debug!(
                "[gpu-renderer] released readback slot {} gen={}",
                readback_index, generation
            );
        }
    }

    /// Copy the completed readback's pixels into `pixels` (tightly packed,
    /// `width * height * 4` bytes) and release the readback slot.
    /// Returns false when the slot is not in flight.
    fn copy_readback(
        &mut self,
        readback_index: usize,
        pixels: &mut [u8],
        width: u32,
        height: u32,
    ) -> bool {
        let Some(generation) = self.inflight_readbacks[readback_index].take() else {
            error!(
                "[gpu-renderer] readback slot {} not in flight",
                readback_index
            );
            return false;
        };
        let Some((buf, _, _)) = &self.readback_buffers[readback_index] else {
            error!(
                "[gpu-renderer] readback slot {} has no buffer",
                readback_index
            );
            return false;
        };
        let data = buf.slice(..).get_mapped_range();
        // Strip alignment padding — write only the actual pixel data into
        // the destination slice, which is tightly packed (width * 4 bytes
        // per row).
        let pixel_count = (width * height * 4) as usize;
        if pixels.len() < pixel_count {
            error!(
                "[gpu-renderer] destination too small: {}B for {}x{} (need {}B)",
                pixels.len(),
                width,
                height,
                pixel_count
            );
            drop(data);
            buf.unmap();
            return false;
        }
        let padded_bytes_per_row = ((width * 4) as usize).div_ceil(256) * 256;
        let row_bytes = (width * 4) as usize;
        if padded_bytes_per_row == row_bytes {
            pixels[..pixel_count].copy_from_slice(&data[..pixel_count]);
        } else {
            for (row_index, row) in data.chunks(padded_bytes_per_row).enumerate() {
                let start = row_index * row_bytes;
                pixels[start..start + row_bytes].copy_from_slice(&row[..row_bytes]);
            }
        }
        drop(data);
        buf.unmap();
        debug!(
            "[gpu-renderer] readback complete slot={} gen={} pixels={}B",
            readback_index, generation, pixel_count
        );
        true
    }

    fn ensure_readback_buffer_inner<'a>(
        readback_buffer: &'a mut Option<(wgpu::Buffer, u32, u32)>,
        device_handle: &wgpu_context::DeviceHandle,
        width: u32,
        height: u32,
    ) -> Option<&'a wgpu::Buffer> {
        // bytes_per_row must be a multiple of COPY_BYTES_PER_ROW_ALIGNMENT (256).
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let bytes_per_row = (width * 4).div_ceil(alignment) * alignment;
        let size = (bytes_per_row * height) as u64;
        let needs_new = match readback_buffer {
            Some((_, w, h)) => *w != width || *h != height,
            None => true,
        };
        if !needs_new {
            return readback_buffer.as_ref().map(|(b, _, _)| b);
        }
        let buf = device_handle.device.create_buffer(&BufferDescriptor {
            label: Some("surface-readback"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        *readback_buffer = Some((buf, width, height));
        readback_buffer.as_ref().map(|(b, _, _)| b)
    }
}

impl SurfaceRenderer for GpuRenderer {
    type RenderData = CpuRenderData;

    fn render(
        &mut self,
        scene: &anyrender::Scene,
        width: u32,
        height: u32,
        _buffers: &mut crate::SurfaceBuffers,
        buffer_index: usize,
        metadata: FrameMetadata,
    ) -> Option<RenderSubmit> {
        let (width, height) = (width.max(1), height.max(1));
        self.ensure_render_tex(width, height);
        self.mark_video_textures_dirty();

        // Step 1: Vello compute render into intermediate texture.
        self.vello_scene.reset();
        {
            let mut painter = anyrender_vello::VelloScenePainter::new(&mut self.vello_scene);
            painter.append_scene(scene.clone(), Affine::IDENTITY);
        }

        let view = self
            .render_tex
            .as_ref()
            .map(|(tex, _, _)| tex.create_view(&TextureViewDescriptor::default()))?;

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
            error!("[gpu-renderer] Vello render failed: {:?}", e);
            return None;
        }

        // Step 2: pick the next free readback slot and ensure its staging
        // buffer matches the current size.
        let Some(readback_index) =
            (0..READBACK_SLOTS).find(|index| self.inflight_readbacks[*index].is_none())
        else {
            error!(
                "[gpu-renderer] no free readback slot for {}x{}",
                width, height
            );
            return None;
        };
        let device_handle = &self.device_handle;
        let readback_buffers = &mut self.readback_buffers;
        let readback_buf = Self::ensure_readback_buffer_inner(
            &mut readback_buffers[readback_index],
            device_handle,
            width,
            height,
        )?;
        // bytes_per_row must be a multiple of COPY_BYTES_PER_ROW_ALIGNMENT.
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let aligned_bytes_per_row = (width * 4).div_ceil(alignment) * alignment;
        let aligned_size = aligned_bytes_per_row * height;

        let mut encoder = device_handle
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("surface-readback"),
            });
        let (src_tex, _, _) = self.render_tex.as_ref()?;
        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: src_tex,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            TexelCopyBufferInfo {
                buffer: readback_buf,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(aligned_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.generation += 1;
        let generation = self.generation;
        let webview_id = metadata.webview_id;
        let shmem_index = buffer_index;
        let frame_hit_info = metadata.frame_hit_info;
        let child_viewports = metadata.child_viewports;
        let child_frame_to_webview = metadata.child_frame_to_webview;
        let animating = metadata.animating;
        // The map is scheduled to complete after this submission finishes on
        // the GPU; the callback fires on the poll thread and delivers the
        // completed frame to the main loop.
        let render_done_tx = self.channels.render_done_tx.clone();
        encoder.map_buffer_on_submit(
            readback_buf,
            wgpu::MapMode::Read,
            0..aligned_size as u64,
            move |result| {
                if let Err(send_error) = render_done_tx.send(CpuRenderData {
                    webview_id,
                    generation,
                    width,
                    height,
                    shmem_index,
                    readback_index,
                    result,
                    metadata: FrameMetadata {
                        webview_id,
                        frame_hit_info,
                        child_viewports,
                        child_frame_to_webview,
                        animating,
                    },
                }) {
                    error!("[gpu-renderer] failed to deliver readback ready: {send_error}");
                }
            },
        );
        let submission_index = device_handle.queue.submit([encoder.finish()]);
        // Ask the poll thread to block until this submission completes; it
        // fires the map callback above when the GPU is done.
        if let Err(send_error) = self.channels.poll_tx.send(PollRequest {
            device: self.device_handle.clone(),
            submission_index: Some(submission_index),
            done: None,
        }) {
            error!("[gpu-renderer] failed to queue poll request: {send_error}");
        }
        self.inflight_readbacks[readback_index] = Some(generation);
        debug!(
            "[gpu-renderer] submitted {}x{} gen={} readback={}",
            width, height, generation, readback_index
        );
        Some(RenderSubmit { generation })
    }

    fn handle_render_done(
        &mut self,
        data: CpuRenderData,
        buffers: &mut crate::SurfaceBuffers,
        sender: &ipc::IpcSender<GraphicsEvent>,
        tla_tracer: &mut verification::TLATracer,
    ) {
        let crate::SurfaceBuffers::Cpu(buffers) = buffers;
        let CpuRenderData {
            webview_id,
            generation,
            width,
            height,
            shmem_index,
            readback_index,
            result,
            metadata,
        } = data;
        if let Err(error) = result {
            error!(
                "[graphics] readback map failed for {:?} gen={}: {error:?}",
                webview_id, generation
            );
            self.release_readback(readback_index);
            return;
        }
        let Some(region) = buffers.regions.get_mut(shmem_index) else {
            error!(
                "[graphics] bad shmem index {} for readback {:?} gen={}",
                shmem_index, webview_id, generation
            );
            self.release_readback(readback_index);
            return;
        };
        // SAFETY: this buffer was reserved at submit time and its pixels are
        // delivered exactly once here, before it is marked pending; no other
        // party reads or writes these pages in between.
        let pixel_slice = unsafe { region.as_mut_slice() };
        if !self.copy_readback(readback_index, pixel_slice, width, height) {
            error!(
                "[graphics] readback copy failed for {:?} gen={}",
                webview_id, generation
            );
            return;
        }
        buffers.ring.mark_pending(shmem_index, generation);

        verification::tla_log!(
            *tla_tracer,
            -> "GPURendering",
            "SurfaceFrameSent",
            webview_id.0,
            generation,
            format!("{}x{}", width, height),
            shmem_index
        );

        let shmem_key = generation as usize;
        let mut shmem_map = std::collections::HashMap::new();
        shmem_map.insert(shmem_key, buffers.regions[shmem_index].clone());

        if sender
            .send_with_shmem_map(
                GraphicsEvent::PixelFrameReady {
                    webview_id,
                    payload: SurfacePayload::CpuShmem { shmem_key },
                    animating: metadata.animating,
                    width,
                    height,
                    generation,
                    frame_hit_info: metadata.frame_hit_info,
                    child_viewports: metadata.child_viewports,
                    child_frame_to_webview: metadata.child_frame_to_webview,
                },
                shmem_map,
            )
            .is_err()
        {
            error!(
                "[graphics] failed to send PixelFrameReady for {:?} gen={}",
                webview_id, generation
            );
            return;
        }

        // The graphical output for the webview is done: the pixels were sent.
        // Traced with the webview's navigable id so it matches the per-frame
        // RenderingOpportunity model (the webview id is the root navigable).
        verification::tla_log!(
            *tla_tracer,
            -> "RenderingOpportunity",
            "GraphicsComputed",
            webview_id.0
        );
    }

    fn render_done_webview_id(data: &CpuRenderData) -> WebviewId {
        data.webview_id
    }
}
