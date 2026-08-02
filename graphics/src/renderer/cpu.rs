//! CPU readback surface backend: renders Vello into an intermediate texture,
//! submits a GPU → CPU readback, and copies the pixels into the webview's
//! shared-memory ring once the readback completes. This is the backend off
//! macOS (GStreamer media backend) and on macOS when built with the
//! `cpu_readback` feature.

use super::{
    FrameDelivery, FrameMetadata, GpuContext, PollRequest, ReadbackChannels, RenderError,
    RenderSubmit, SurfaceBuffers, SurfaceRenderer, SurfaceRingState, frame_metadata, render_size,
};
use ipc_messages::content::WebviewId;
use ipc_messages::graphics::{GraphicsEvent, SurfacePayload};
use log::{debug, error, info};
use std::collections::HashMap;
use wgpu::{
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d, Origin3d,
    TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};

#[cfg(target_os = "macos")]
use ipc_messages::media::VideoPaintId;
#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_core_video::CVPixelBuffer;

use crate::ComposedScene;

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

/// The CPU readback renderer: a [`GpuContext`] plus the intermediate
/// texture, the readback staging pool, and the webview's shared-memory ring.
pub struct CpuRenderer {
    gpu: GpuContext,
    channels: ReadbackChannels<CpuRenderData>,
    /// Intermediate texture for Vello compute (has STORAGE_BINDING +
    /// COPY_SRC); the CPU readback source.
    render_tex: Option<(wgpu::Texture, u32, u32)>,
    /// Staging buffers for GPU → CPU readback, one per in-flight frame.
    readback_buffers: [Option<(wgpu::Buffer, u32, u32)>; READBACK_SLOTS],
    /// Generation of the frame whose readback is in flight per slot.
    inflight_readbacks: [Option<u64>; READBACK_SLOTS],
    /// The webview's shared-memory ring (three regions), reallocated on
    /// resize.
    buffers: Option<SurfaceBuffers<[ipc::IpcSharedRegion; 3]>>,
    /// The most recent composed scene that could not be submitted because
    /// every ring buffer was still awaiting the embedder's ack.
    deferred_scene: Option<ComposedScene>,
}

impl CpuRenderer {
    fn ensure_render_tex(&mut self, width: u32, height: u32) {
        if self
            .render_tex
            .as_ref()
            .map(|(_, render_width, render_height)| {
                *render_width == width && *render_height == height
            })
            .unwrap_or(false)
        {
            return;
        }
        let tex = self
            .gpu
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

    /// Three shared-memory pixel buffers for the ring, sized for `width`×
    /// `height` RGBA8.
    fn allocate_shmem(width: u32, height: u32) -> Result<[ipc::IpcSharedRegion; 3], ipc::IpcError> {
        let byte_count = (width as usize) * (height as usize) * 4;
        let region_zero = ipc::IpcSharedRegion::allocate(byte_count)?;
        let region_one = ipc::IpcSharedRegion::allocate(byte_count)?;
        let region_two = ipc::IpcSharedRegion::allocate(byte_count)?;
        Ok([region_zero, region_one, region_two])
    }

    /// Drop the in-flight marker for a readback slot (map failure path).
    fn release_readback(
        inflight_readbacks: &mut [Option<u64>; READBACK_SLOTS],
        readback_index: usize,
    ) {
        if let Some(generation) = inflight_readbacks[readback_index].take() {
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
        inflight_readbacks: &mut [Option<u64>; READBACK_SLOTS],
        readback_buffers: &mut [Option<(wgpu::Buffer, u32, u32)>; READBACK_SLOTS],
        readback_index: usize,
        pixels: &mut [u8],
        width: u32,
        height: u32,
    ) -> bool {
        let Some(generation) = inflight_readbacks[readback_index].take() else {
            error!(
                "[gpu-renderer] readback slot {} not in flight",
                readback_index
            );
            return false;
        };
        let Some((buf, _, _)) = &readback_buffers[readback_index] else {
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

    fn ensure_readback_buffer<'a>(
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
            Some((_, buffer_width, buffer_height)) => {
                *buffer_width != width || *buffer_height != height
            }
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

impl SurfaceRenderer for CpuRenderer {
    type RenderData = CpuRenderData;

    fn new(channels: ReadbackChannels<CpuRenderData>) -> Result<Self, String> {
        Ok(Self {
            gpu: GpuContext::new()?,
            channels,
            render_tex: None,
            readback_buffers: [None, None, None],
            inflight_readbacks: [None, None, None],
            buffers: None,
            deferred_scene: None,
        })
    }

    fn submit_scene(&mut self, composed: ComposedScene) -> Result<RenderSubmit, RenderError> {
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

        // The intermediate render target must match the current size before
        // the buffers borrow below.
        self.ensure_render_tex(width, height);

        // Reuse the per-webview frame buffers across frames, reallocating
        // only when the viewport size changes.
        let needs_new = self
            .buffers
            .as_ref()
            .is_none_or(|buffers| buffers.ring().width != width || buffers.ring().height != height);
        if needs_new {
            let payload = Self::allocate_shmem(width, height).map_err(|error| {
                error!(
                    "[graphics] allocate surface shmem {}x{}: {error}",
                    width, height
                );
                RenderError::Failed
            })?;
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

        // Step 1: Vello compute render into the intermediate texture.
        let (src_tex, _, _) = self.render_tex.as_ref().ok_or(RenderError::Failed)?;
        self.gpu.mark_video_textures_dirty();
        if let Err(error) = self.gpu.render_into(&scene, src_tex, width, height) {
            error!("[gpu-renderer] {error}");
            return Err(RenderError::Failed);
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
            return Err(RenderError::Failed);
        };
        let device_handle = &self.gpu.device_handle;
        let readback_buf = Self::ensure_readback_buffer(
            &mut self.readback_buffers[readback_index],
            device_handle,
            width,
            height,
        )
        .ok_or(RenderError::Failed)?;
        // bytes_per_row must be a multiple of COPY_BYTES_PER_ROW_ALIGNMENT.
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let aligned_bytes_per_row = (width * 4).div_ceil(alignment) * alignment;
        let aligned_size = aligned_bytes_per_row * height;

        let mut encoder = device_handle
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("surface-readback"),
            });
        let (src_tex, _, _) = self.render_tex.as_ref().ok_or(RenderError::Failed)?;
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

        self.gpu.generation += 1;
        let generation = self.gpu.generation;
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
        let submission_index = self.gpu.device_handle.queue.submit([encoder.finish()]);
        // Ask the poll thread to block until this submission completes; it
        // fires the map callback above when the GPU is done.
        if let Err(send_error) = self.channels.poll_tx.send(PollRequest {
            device: self.gpu.device_handle.clone(),
            submission_index: Some(submission_index),
            done: None,
        }) {
            error!("[gpu-renderer] failed to queue poll request: {send_error}");
        }
        self.inflight_readbacks[readback_index] = Some(generation);
        buffers.reserve(buffer_index, generation);
        debug!(
            "[gpu-renderer] submitted {}x{} gen={} readback={}",
            width, height, generation, readback_index
        );

        Ok(RenderSubmit {
            generation,
            width,
            height,
            buffer_index,
        })
    }

    fn handle_render_done(
        &mut self,
        data: CpuRenderData,
        sender: &ipc::IpcSender<GraphicsEvent>,
    ) -> FrameDelivery {
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
        let mut delivery = FrameDelivery {
            generation,
            width,
            height,
            buffer_index: shmem_index,
            surface_frame_sent: false,
            graphics_computed: false,
        };
        if let Err(error) = result {
            error!(
                "[graphics] readback map failed for {:?} gen={}: {error:?}",
                webview_id, generation
            );
            Self::release_readback(&mut self.inflight_readbacks, readback_index);
            return delivery;
        }
        let Some(buffers) = self.buffers.as_mut() else {
            error!(
                "[graphics] no surface buffers for render done {:?}",
                webview_id
            );
            return delivery;
        };
        let Some(region) = buffers.payload_mut().get_mut(shmem_index) else {
            error!(
                "[graphics] bad shmem index {} for readback {:?} gen={}",
                shmem_index, webview_id, generation
            );
            Self::release_readback(&mut self.inflight_readbacks, readback_index);
            return delivery;
        };
        // SAFETY: this buffer was reserved at submit time and its pixels are
        // delivered exactly once here, before it is marked pending; no other
        // party reads or writes these pages in between.
        let pixel_slice = unsafe { region.as_mut_slice() };
        if !Self::copy_readback(
            &mut self.inflight_readbacks,
            &mut self.readback_buffers,
            readback_index,
            pixel_slice,
            width,
            height,
        ) {
            error!(
                "[graphics] readback copy failed for {:?} gen={}",
                webview_id, generation
            );
            return delivery;
        }
        buffers.ring_mut().mark_pending(shmem_index, generation);
        delivery.surface_frame_sent = true;

        let shmem_key = generation as usize;
        let mut shmem_map = HashMap::new();
        shmem_map.insert(shmem_key, buffers.payload()[shmem_index].clone());

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
            return delivery;
        }
        delivery.graphics_computed = true;
        delivery
    }

    fn render_done_webview_id(data: &CpuRenderData) -> WebviewId {
        data.webview_id
    }

    fn ack(&mut self, generation: u64) -> bool {
        self.buffers
            .as_mut()
            .is_some_and(|buffers| buffers.ack(generation))
    }

    fn submit_deferred(&mut self) -> Option<RenderSubmit> {
        let composed = self.deferred_scene.take()?;
        let webview_id = composed.webview_id;
        match self.submit_scene(composed) {
            Ok(submit) => Some(submit),
            Err(RenderError::Deferred) => None,
            Err(RenderError::Failed) => {
                error!(
                    "[graphics] submit deferred scene failed for {:?}",
                    webview_id
                );
                None
            }
        }
    }

    #[cfg(target_os = "macos")]
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
