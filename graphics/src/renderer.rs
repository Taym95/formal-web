//! GPU renderer — renders scenes to a CPU-readable RGBA8 buffer via Vello.
//! Vello renders to an intermediate GPU texture (STORAGE_BINDING), then a
//! GPU → CPU readback copies the pixels to a staging buffer.  The pixel data
//! is shipped to the embedder via IPC shared memory.
//!
//! Cross-process IOSurface sharing is not viable on modern macOS
//! (IOSurfaceLookup is deprecated/inoperative cross-process, and Mach-port
//! bootstrap registration is unreliable).  See graphics/README.md.

use anyrender::PaintScene;
use ipc_messages::content::{FrameId, WebviewId};
use ipc_messages::graphics::{ChildViewport, FrameHitInfo};
use kurbo::Affine;
use log::{debug, error};
use std::collections::HashMap;

use vello::{
    AaConfig, AaSupport, RenderParams, Renderer as VelloRenderer, RendererOptions,
    Scene as VelloScene,
};
use wgpu::{
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d, Origin3d,
    TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
};

/// The number of readback buffers kept per renderer; must be >= the number
/// of shared-memory surface buffers so each in-flight frame has its own
/// staging buffer.
pub const READBACK_SLOTS: usize = 3;

/// A request for the GPU poll thread to block until the given device
/// submission completes. Sent after each readback submission; the map
/// callbacks then fire on the poll thread and deliver `ReadbackReady`.
pub struct PollRequest {
    pub device: wgpu_context::DeviceHandle,
    pub submission_index: wgpu::SubmissionIndex,
}

/// Frame metadata captured at submit time and delivered to the main loop
/// when the GPU completes the readback copy. The shared-memory buffer index
/// is pre-selected here; the pixels are written into it only at completion.
#[derive(Clone)]
pub struct ReadbackCompletion {
    pub webview_id: WebviewId,
    pub shmem_index: usize,
    pub frame_hit_info: Vec<FrameHitInfo>,
    pub child_viewports: Vec<ChildViewport>,
    pub child_frame_to_webview: HashMap<FrameId, WebviewId>,
    pub animating: bool,
}

/// A completed readback, sent from the map callback to the main loop.
pub struct ReadbackReady {
    pub webview_id: WebviewId,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub shmem_index: usize,
    pub readback_index: usize,
    pub result: Result<(), wgpu::BufferAsyncError>,
    pub frame_hit_info: Vec<FrameHitInfo>,
    pub child_viewports: Vec<ChildViewport>,
    pub child_frame_to_webview: HashMap<FrameId, WebviewId>,
    pub animating: bool,
}

/// The result of submitting a frame's readback: the generation of the frame
/// now in flight.
pub struct RenderSubmit {
    pub generation: u64,
}

/// The channels that connect the readback pipeline to the GPU poll thread
/// and the main loop. Created once at graphics-process startup.
#[derive(Clone)]
pub struct ReadbackChannels {
    /// Requests for the poll thread to block on a device submission.
    pub poll_tx: crossbeam_channel::Sender<PollRequest>,
    /// Completed readbacks delivered from the map callbacks to the main loop.
    pub readback_ready_tx: crossbeam_channel::Sender<ReadbackReady>,
}

pub struct GpuRenderer {
    device_handle: wgpu_context::DeviceHandle,
    vello_renderer: VelloRenderer,
    vello_scene: VelloScene,
    /// Intermediate texture for Vello compute (has STORAGE_BINDING + COPY_SRC).
    render_tex: Option<(Texture, u32, u32)>,
    /// Staging buffers for GPU → CPU readback, one per in-flight frame.
    /// Each slot is resized on demand and reused once its readback completes.
    readback_buffers: [Option<(wgpu::Buffer, u32, u32)>; READBACK_SLOTS],
    /// Generation of the frame whose readback is in flight per slot; None
    /// when the slot is free to be reused.
    inflight_readbacks: [Option<u64>; READBACK_SLOTS],
    channels: ReadbackChannels,
    generation: u64,
}

impl GpuRenderer {
    pub fn new(channels: ReadbackChannels) -> Result<Self, String> {
        let features = wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE;
        let context = wgpu_context::WGPUContext::with_features_and_limits(Some(features), None);
        let device_handle = pollster::block_on(context.create_device_handle(None))
            .map_err(|e| format!("failed to create wgpu device: {e}"))?;

        let vello_renderer = VelloRenderer::new(
            &device_handle.device,
            RendererOptions {
                use_cpu: false,
                num_init_threads: None,
                antialiasing_support: AaSupport::area_only(),
                pipeline_cache: None,
            },
        )
        .map_err(|e| format!("failed to create Vello renderer: {e}"))?;

        Ok(Self {
            device_handle,
            vello_renderer,
            vello_scene: VelloScene::new(),
            render_tex: None,
            readback_buffers: [None, None, None],
            inflight_readbacks: [None, None, None],
            channels,
            generation: 0,
        })
    }

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

    #[allow(dead_code)]
    fn ensure_readback_buffer(&mut self, width: u32, height: u32) -> Option<&wgpu::Buffer> {
        Self::ensure_readback_buffer_inner(
            &mut self.readback_buffers[0],
            &self.device_handle,
            width,
            height,
        )
    }

    /// True when any readback submission is still waiting for the GPU to
    /// finish. The main loop uses this to decide whether to keep polling.
    /// Drop the in-flight marker for a readback slot (map failure path).
    pub fn release_readback(&mut self, readback_index: usize) {
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
    pub fn copy_readback(
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
        let bytes_per_row = ((width * 4 + alignment - 1) / alignment) * alignment;
        let size = (bytes_per_row * height) as u64;
        // Check if existing buffer matches size (drop the borrow before mutation).
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

    /// Render a scene and submit the GPU → CPU readback without blocking.
    /// The pixels are delivered asynchronously: the buffer is mapped via
    /// `map_buffer_on_submit`, so when the GPU finishes the copy the map
    /// callback sends `ReadbackReady` on the renderer's channel with the
    /// pre-selected `completion` metadata (including the shared-memory
    /// buffer index the pixels must be written into). A poll request is sent
    /// to the dedicated poll thread so the GPU is waited on without blocking
    /// the main loop. Returns the frame generation on success.
    pub fn render_scene(
        &mut self,
        scene: &anyrender::Scene,
        width: u32,
        height: u32,
        completion: ReadbackCompletion,
    ) -> Option<RenderSubmit> {
        let (width, height) = (width.max(1), height.max(1));
        self.ensure_render_tex(width, height);

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
        {
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
            let aligned_bytes_per_row = ((width * 4 + alignment - 1) / alignment) * alignment;
            let aligned_size = aligned_bytes_per_row * height;

            let mut encoder =
                device_handle
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
            let webview_id = completion.webview_id;
            let shmem_index = completion.shmem_index;
            let frame_hit_info = completion.frame_hit_info;
            let child_viewports = completion.child_viewports;
            let child_frame_to_webview = completion.child_frame_to_webview;
            let animating = completion.animating;
            // The map is scheduled to complete after this submission finishes
            // on the GPU; the callback fires on the poll thread and carries
            // everything needed to deliver the pixels.
            let readback_ready_tx = self.channels.readback_ready_tx.clone();
            encoder.map_buffer_on_submit(
                readback_buf,
                wgpu::MapMode::Read,
                0..aligned_size as u64,
                move |result| {
                    let _ = readback_ready_tx.send(ReadbackReady {
                        webview_id,
                        generation,
                        width,
                        height,
                        shmem_index,
                        readback_index,
                        result,
                        frame_hit_info,
                        child_viewports,
                        child_frame_to_webview,
                        animating,
                    });
                },
            );
            let submission_index = device_handle.queue.submit([encoder.finish()]);
            // Ask the poll thread to block until this submission completes; it
            // fires the map callback above when the GPU is done.
            let _ = self.channels.poll_tx.send(PollRequest {
                device: self.device_handle.clone(),
                submission_index,
            });
            self.inflight_readbacks[readback_index] = Some(generation);
            debug!(
                "[gpu-renderer] submitted {}x{} gen={} readback={}",
                width, height, generation, readback_index
            );
            Some(RenderSubmit { generation })
        }
    }
}
