//! GPU renderer — renders scenes to a CPU-readable RGBA8 buffer via Vello,
//! and (macOS) directly into shared IOSurface textures.
//!
//! The CPU path renders to an intermediate GPU texture (STORAGE_BINDING),
//! then a GPU → CPU readback copies the pixels to a staging buffer.  The
//! pixel data is shipped to the embedder via IPC shared memory.
//!
//! The zero-copy path (macOS) renders Vello directly into a wgpu texture
//! backed by a cross-process IOSurface; the embedder imports the same
//! surface and blits it, with no readback and no IPC pixel bytes.  See
//! graphics/README.md.

use anyrender::PaintScene;
use ipc_messages::content::{FrameId, WebviewId};
use ipc_messages::graphics::{ChildViewport, FrameHitInfo};
use ipc_messages::media::VideoPaintId;
use kurbo::Affine;
use log::{debug, error};
use std::collections::HashMap;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::ProtocolObject;
#[cfg(target_os = "macos")]
use objc2_metal::MTLDevice;
use vello::{
    AaConfig, AaSupport, RenderParams, Renderer as VelloRenderer, RendererOptions,
    Scene as VelloScene,
};
use wgpu::{
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, ComputePipelineDescriptor, Extent3d,
    Origin3d, TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture,
    TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    TextureViewDescriptor,
};

/// The number of readback buffers kept per renderer; must be >= the number
/// of shared-memory surface buffers so each in-flight frame has its own
/// staging buffer.
pub const READBACK_SLOTS: usize = 3;

/// A request for the GPU poll thread to block until the given device
/// submission completes. The map callbacks registered with
/// `map_buffer_on_submit` fire there and deliver `ReadbackReady`; when
/// `done` is present (zero-copy path, which has no map callback) the poll
/// thread instead delivers `RenderDone` once the submission completes.
pub struct PollRequest {
    pub device: wgpu_context::DeviceHandle,
    /// The submission to wait for; `None` waits for all submitted work
    /// (used by the shared-texture path, where Vello submits internally).
    pub submission_index: Option<wgpu::SubmissionIndex>,
    /// Zero-copy path: delivered after the GPU work completes.
    pub done: Option<RenderDone>,
}

/// A render into a shared texture completed on the GPU: the embedder may now
/// safely blit the shared surface (the coarse GPU sync for the zero-copy
/// path).
pub struct RenderDone {
    pub webview_id: WebviewId,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub buffer_index: usize,
    pub completion: ReadbackCompletion,
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
    /// Completed shared-texture renders delivered from the poll thread to
    /// the main loop (zero-copy path).
    pub render_done_tx: crossbeam_channel::Sender<RenderDone>,
}

#[cfg(target_os = "macos")]
mod video_texture {
    //! macOS video texture import: wraps a CVPixelBuffer as a Metal texture
    //! (zero-copy when the buffer is GPU-backed) and blits BGRA → RGBA into
    //! a texture Vello can register (Vello's `register_texture` requires
    //! Rgba8Unorm + COPY_SRC).

    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_core_video::{
        CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, kCVReturnSuccess,
    };
    use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture};

    const BGRA_TO_RGBA_WGSL: &str = r#"
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var dst_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = textureDimensions(src_tex);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    // wgpu presents a bgra8unorm texel in RGBA semantic order (the Metal
    // backend applies the format's channel swizzle), so the copy is direct.
    let c = textureLoad(src_tex, vec2<i32>(gid.xy), 0);
    textureStore(dst_tex, vec2<i32>(gid.xy), c);
}
"#;

    /// A video frame delivered as a GPU texture on this renderer's device.
    pub(super) struct VideoTexture {
        /// RGBA8 conversion target; its contents are what Vello samples.
        pub(super) texture: Texture,
        /// Fake image data referencing `texture` via Vello's override_image;
        /// composed scenes draw this as a plain `Paint::Image` brush.
        pub(super) image: peniko::ImageData,
        /// The pixel buffer whose contents are in `texture`; keeps the
        /// source alive until the next frame replaces it.
        pub(super) pixel_buffer: Retained<objc2_core_video::CVPixelBuffer>,
        pub(super) width: u32,
        pub(super) height: u32,
    }

    pub(super) struct VideoImport {
        /// Metal texture cache for this renderer's device.
        cache: Retained<CVMetalTextureCache>,
        /// The BGRA→RGBA blit pipeline.
        pipeline: wgpu::ComputePipeline,
        bind_group_layout: wgpu::BindGroupLayout,
    }

    impl VideoImport {
        /// Create the Metal texture cache and blit pipeline on this
        /// renderer's device. Requires the raw Metal device from
        /// `wgpu::hal::metal::Device::raw_device`.
        pub(super) fn new(
            device: &wgpu::Device,
            raw_device: &Retained<ProtocolObject<dyn MTLDevice>>,
        ) -> Option<VideoImport> {
            let mut cache_ptr = std::ptr::null_mut();
            let cache_result = unsafe {
                CVMetalTextureCache::create(
                    None,
                    None,
                    raw_device,
                    None,
                    std::ptr::NonNull::new(&mut cache_ptr)?,
                )
            };
            if cache_result != kCVReturnSuccess {
                error!("[gpu-renderer] CVMetalTextureCacheCreate failed: {cache_result}");
                return None;
            }
            // SAFETY: the create call above wrote a valid retained cache.
            let cache = unsafe { Retained::from_raw(cache_ptr) }?;

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("bgra-to-rgba"),
                source: wgpu::ShaderSource::Wgsl(BGRA_TO_RGBA_WGSL.into()),
            });
            let bind_group_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("bgra-to-rgba-layout"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::StorageTexture {
                                access: wgpu::StorageTextureAccess::WriteOnly,
                                format: wgpu::TextureFormat::Rgba8Unorm,
                                view_dimension: wgpu::TextureViewDimension::D2,
                            },
                            count: None,
                        },
                    ],
                });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("bgra-to-rgba-pipeline-layout"),
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
                label: Some("bgra-to-rgba"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
            Some(VideoImport {
                cache,
                pipeline,
                bind_group_layout,
            })
        }

        /// Wrap `pixel_buffer` (BGRA) as a wgpu texture on this device and
        /// blit it into `target` (RGBA8Unorm storage), returning the wrapped
        /// source texture. The caller must keep `pixel_buffer` alive while
        /// the source texture is in use.
        pub(super) fn blit_bgra_into_rgba(
            &self,
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            pixel_buffer: &Retained<objc2_core_video::CVPixelBuffer>,
            width: u32,
            height: u32,
            target: &Texture,
        ) -> Option<Texture> {
            let mut texture_ptr = std::ptr::null_mut();
            let result = unsafe {
                CVMetalTextureCache::create_texture_from_image(
                    None,
                    &self.cache,
                    pixel_buffer,
                    None,
                    MTLPixelFormat::BGRA8Unorm,
                    width as usize,
                    height as usize,
                    0,
                    std::ptr::NonNull::new(&mut texture_ptr)?,
                )
            };
            if result != kCVReturnSuccess {
                error!("[gpu-renderer] create_texture_from_image failed: {result}");
                return None;
            }
            // SAFETY: the create call above wrote a valid retained texture.
            let cv_metal_texture: Retained<CVMetalTexture> =
                unsafe { Retained::from_raw(texture_ptr) }?;
            // SAFETY: CVMetalTextureGetTexture returns a +1 retained Metal
            // texture referencing the pixel buffer.
            let raw_texture: Retained<ProtocolObject<dyn MTLTexture>> =
                CVMetalTextureGetTexture(&cv_metal_texture)?;

            let hal_texture = unsafe {
                wgpu::hal::metal::Device::texture_from_raw(
                    raw_texture,
                    wgpu::TextureFormat::Bgra8Unorm,
                    objc2_metal::MTLTextureType::Type2D,
                    1,
                    1,
                    wgpu::hal::CopyExtent {
                        width,
                        height,
                        depth: 1,
                    },
                )
            };
            let source = unsafe {
                device.create_texture_from_hal::<wgpu::hal::metal::Api>(
                    hal_texture,
                    &TextureDescriptor {
                        label: Some("video-bgra-source"),
                        size: Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: TextureDimension::D2,
                        format: TextureFormat::Bgra8Unorm,
                        usage: TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                )
            };

            let source_view = source.create_view(&TextureViewDescriptor::default());
            let target_view = target.create_view(&TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bgra-to-rgba-bind"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&target_view),
                    },
                ],
            });
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
                label: Some("bgra-to-rgba"),
            });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("bgra-to-rgba"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
            }
            queue.submit([encoder.finish()]);
            Some(source)
        }
    }
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
    /// Per-video-paint GPU texture state (macOS).
    #[cfg(target_os = "macos")]
    video_textures: HashMap<VideoPaintId, video_texture::VideoTexture>,
    #[cfg(target_os = "macos")]
    video_import: Option<video_texture::VideoImport>,
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
            #[cfg(target_os = "macos")]
            video_textures: HashMap::new(),
            #[cfg(target_os = "macos")]
            video_import: None,
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

    /// The raw Metal device backing this renderer (macOS), needed to create
    /// IOSurface-backed Metal textures and the video Metal texture cache.
    #[cfg(target_os = "macos")]
    pub fn raw_metal_device(&self) -> Option<Retained<ProtocolObject<dyn MTLDevice>>> {
        // SAFETY: the hal device is this renderer's own device; the returned
        // raw Metal device is used only to create textures on it.
        let hal_device = unsafe { self.device_handle.device.as_hal::<wgpu::hal::metal::Api>() }?;
        Some(hal_device.raw_device().clone())
    }

    /// The wgpu device this renderer composites with.
    pub fn device(&self) -> &wgpu::Device {
        &self.device_handle.device
    }

    /// Register a video frame: wrap `pixel_buffer` as a Metal texture and
    /// blit it into an RGBA texture Vello can sample. Returns the fake
    /// `ImageData` referencing the texture (via `override_image`) to embed in
    /// composed scenes as a plain image brush. The same image data is reused
    /// while the frame size is unchanged.
    #[cfg(target_os = "macos")]
    pub fn import_video_frame(
        &mut self,
        paint_id: VideoPaintId,
        pixel_buffer: &Retained<objc2_core_video::CVPixelBuffer>,
        width: u32,
        height: u32,
    ) -> Option<peniko::ImageData> {
        let video_import = match &self.video_import {
            Some(video_import) => video_import,
            None => {
                let raw_device = self.raw_metal_device()?;
                let video_import =
                    video_texture::VideoImport::new(&self.device_handle.device, &raw_device)?;
                self.video_import = Some(video_import);
                self.video_import.as_ref()?
            }
        };

        let needs_new = match self.video_textures.get(&paint_id) {
            Some(existing) => existing.width != width || existing.height != height,
            None => true,
        };
        if needs_new {
            if let Some(old) = self.video_textures.remove(&paint_id) {
                // Drop the old override so its blob id no longer samples.
                self.vello_renderer.override_image(&old.image, None);
            }
            let target = self
                .device_handle
                .device
                .create_texture(&TextureDescriptor {
                    label: Some("video-rgba"),
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
            // The blit source references the pixel buffer; keep it alive in
            // the video texture state until the next frame replaces it.
            let _source = video_import.blit_bgra_into_rgba(
                &self.device_handle.device,
                &self.device_handle.queue,
                pixel_buffer,
                width,
                height,
                &target,
            )?;
            // Fake image: empty blob, real size. Vello never reads the blob;
            // the override below makes it sample `target` instead.
            let image = peniko::ImageData {
                data: peniko::Blob::new(std::sync::Arc::new(&[])),
                format: peniko::ImageFormat::Rgba8,
                alpha_type: peniko::ImageAlphaType::Alpha,
                width,
                height,
            };
            self.vello_renderer.override_image(
                &image,
                Some(wgpu::TexelCopyTextureInfoBase {
                    texture: target.clone(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                }),
            );
            self.video_textures.insert(
                paint_id,
                video_texture::VideoTexture {
                    texture: target,
                    image: image.clone(),
                    pixel_buffer: pixel_buffer.clone(),
                    width,
                    height,
                },
            );
            return Some(image);
        }

        let state = self.video_textures.get_mut(&paint_id)?;
        // Same-size frame: blit into the existing target and keep the
        // pixel buffer alive for the frame's lifetime.
        let _source = video_import.blit_bgra_into_rgba(
            &self.device_handle.device,
            &self.device_handle.queue,
            pixel_buffer,
            width,
            height,
            &state.texture,
        )?;
        state.pixel_buffer = pixel_buffer.clone();
        Some(state.image.clone())
    }

    /// Mark every registered video texture dirty so Vello recopies their
    /// (updated) contents into its atlas on the next render.
    pub fn mark_video_textures_dirty(&mut self) {
        #[cfg(target_os = "macos")]
        for state in self.video_textures.values() {
            self.vello_renderer.mark_override_image_dirty(&state.image);
        }
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
        let bytes_per_row = (width * 4).div_ceil(alignment) * alignment;
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
            let aligned_bytes_per_row = (width * 4).div_ceil(alignment) * alignment;
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
                submission_index: Some(submission_index),
                done: None,
            });
            self.inflight_readbacks[readback_index] = Some(generation);
            debug!(
                "[gpu-renderer] submitted {}x{} gen={} readback={}",
                width, height, generation, readback_index
            );
            Some(RenderSubmit { generation })
        }
    }

    /// Render a scene directly into a shared texture (macOS zero-copy path)
    /// without a readback. Vello computes into `target`'s view; when the
    /// submission completes, the poll thread sends `RenderDone` so the main
    /// loop can deliver `PixelFrameReady` with the shared-surface payload.
    /// The embedder's blit of the shared surface is then GPU-safe.
    #[cfg(target_os = "macos")]
    pub fn render_scene_shared(
        &mut self,
        scene: &anyrender::Scene,
        width: u32,
        height: u32,
        target: &Texture,
        completion: ReadbackCompletion,
    ) -> Option<RenderSubmit> {
        let (width, height) = (width.max(1), height.max(1));
        self.mark_video_textures_dirty();

        self.vello_scene.reset();
        {
            let mut painter = anyrender_vello::VelloScenePainter::new(&mut self.vello_scene);
            painter.append_scene(scene.clone(), Affine::IDENTITY);
        }

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
        let buffer_index = completion.shmem_index;
        // Vello's render_to_texture submits internally; waiting for "all
        // submitted work" (submission_index: None) covers that submission.
        let done = RenderDone {
            webview_id: completion.webview_id,
            generation,
            width,
            height,
            buffer_index,
            completion,
        };
        let _ = self.channels.poll_tx.send(PollRequest {
            device: self.device_handle.clone(),
            submission_index: None,
            done: Some(done),
        });
        debug!(
            "[gpu-renderer] rendered into shared texture {}x{} gen={} buffer={}",
            width, height, generation, buffer_index
        );
        Some(RenderSubmit { generation })
    }
}
