//! Per-webview GPU renderers, one per surface backend, behind a common
//! [`SurfaceRenderer`] trait. Each backend defines its own [`RenderData`]
//! associated type — the per-frame payload produced at submit time and
//! consumed by [`SurfaceRenderer::handle_render_done`] when the GPU
//! completes the frame.
//!
//! The CPU path (readback + shared memory) renders to an intermediate
//! texture and submits a GPU → CPU readback; the pixels are copied into the
//! webview's shared-memory ring once the readback completes. It is the
//! backend off macOS (GStreamer media backend) and on macOS when built with
//! the `cpu_readback` feature.
//!
//! The zero-copy path renders Vello directly into a shared IOSurface
//! texture (macOS, the default); the embedder imports the same surface and
//! blits it, with no readback and no IPC pixel bytes. See graphics/README.md.

use ipc_messages::content::{FrameId, WebviewId};
use ipc_messages::graphics::{ChildViewport, FrameHitInfo, GraphicsEvent};
use ipc_messages::media::VideoPaintId;
use log::error;
use std::collections::HashMap;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::ProtocolObject;
#[cfg(target_os = "macos")]
use objc2_metal::MTLDevice;
use vello::{AaSupport, Renderer as VelloRenderer, RendererOptions, Scene as VelloScene};
use wgpu::{
    CommandEncoderDescriptor, ComputePipelineDescriptor, Extent3d, Texture, TextureDescriptor,
    TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
};

/// Frame metadata captured at submit time and delivered with the frame when
/// the GPU completes it.
#[derive(Clone)]
pub struct FrameMetadata {
    pub webview_id: WebviewId,
    pub frame_hit_info: Vec<FrameHitInfo>,
    pub child_viewports: Vec<ChildViewport>,
    pub child_frame_to_webview: HashMap<FrameId, WebviewId>,
    pub animating: bool,
}

/// The result of submitting a frame: the generation now in flight.
pub struct RenderSubmit {
    pub generation: u64,
}

/// A request for the GPU poll thread to block until the given device
/// submission completes. The readback map callbacks (CPU path) fire there
/// and deliver `RenderDone`; when `done` is present (zero-copy path, which
/// has no map callback) the poll thread delivers it after the poll.
pub struct PollRequest {
    pub device: wgpu_context::DeviceHandle,
    /// The submission to wait for; `None` waits for all submitted work
    /// (used by the shared-texture path, where Vello submits internally).
    pub submission_index: Option<wgpu::SubmissionIndex>,
    /// Zero-copy path: delivered after the GPU work completes.
    pub done: Option<RenderDone>,
}

/// The channels connecting the renderers to the GPU poll thread and the
/// main loop, created once at graphics-process startup.
#[derive(Clone)]
pub struct ReadbackChannels {
    /// Requests for the poll thread to block on a device submission.
    pub poll_tx: crossbeam_channel::Sender<PollRequest>,
    /// Completed frames: delivered by the readback map callbacks (CPU path)
    /// and by the poll thread (zero-copy path).
    pub render_done_tx: crossbeam_channel::Sender<RenderDone>,
}

/// A per-webview GPU renderer: submits a frame's render and, when the GPU
/// completes it, delivers the pixels to the embedder. The associated
/// `RenderData` is the per-frame payload produced at submit time and
/// consumed by [`handle_render_done`](Self::handle_render_done); each
/// surface backend provides its own implementation (CPU readback + shared
/// memory, or zero-copy IOSurface).
pub(crate) trait SurfaceRenderer {
    /// Per-frame data produced at submit time, consumed at GPU completion.
    type RenderData: Send + 'static;

    /// Render `scene` at `width`×`height` into the ring buffer at
    /// `buffer_index` and submit. Each backend picks its render target from
    /// `buffers` (the shared IOSurface texture on the zero-copy path; the
    /// internal intermediate texture on the CPU path). The GPU completion is
    /// delivered on `ReadbackChannels::render_done_tx` as `Self::RenderData`.
    fn render(
        &mut self,
        scene: &anyrender::Scene,
        width: u32,
        height: u32,
        buffers: &mut crate::SurfaceBuffers,
        buffer_index: usize,
        metadata: FrameMetadata,
    ) -> Option<RenderSubmit>;

    /// The GPU completed a frame: mark the ring buffer pending and deliver
    /// the frame to the embedder.
    fn handle_render_done(
        &mut self,
        data: Self::RenderData,
        buffers: &mut crate::SurfaceBuffers,
        sender: &ipc::IpcSender<GraphicsEvent>,
        tla_tracer: &mut verification::TLATracer,
    );

    /// The webview a completed frame belongs to (used to look up the slot).
    fn render_done_webview_id(data: &Self::RenderData) -> WebviewId;
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
        /// `wgpu_hal::metal::Device::raw_device`.
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

/// The per-webview GPU renderer: Vello rendering plus the surface backend's
/// delivery machinery. The `SurfaceRenderer` impl is chosen at compile time
/// by features: readback + shared memory off macOS (and on macOS with
/// `cpu_readback`), zero-copy IOSurface on macOS by default.
pub struct GpuRenderer {
    device_handle: wgpu_context::DeviceHandle,
    vello_renderer: VelloRenderer,
    vello_scene: VelloScene,
    channels: ReadbackChannels,
    generation: u64,
    /// Per-video-paint GPU texture state (macOS).
    #[cfg(target_os = "macos")]
    video_textures: HashMap<VideoPaintId, video_texture::VideoTexture>,
    #[cfg(target_os = "macos")]
    video_import: Option<video_texture::VideoImport>,
    /// Intermediate texture for Vello compute (has STORAGE_BINDING +
    /// COPY_SRC); the CPU readback source.
    #[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
    render_tex: Option<(Texture, u32, u32)>,
    /// Staging buffers for GPU → CPU readback, one per in-flight frame.
    #[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
    readback_buffers: [Option<(wgpu::Buffer, u32, u32)>; cpu::READBACK_SLOTS],
    /// Generation of the frame whose readback is in flight per slot.
    #[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
    inflight_readbacks: [Option<u64>; cpu::READBACK_SLOTS],
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
            channels,
            generation: 0,
            #[cfg(target_os = "macos")]
            video_textures: HashMap::new(),
            #[cfg(target_os = "macos")]
            video_import: None,
            #[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
            render_tex: None,
            #[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
            readback_buffers: [None, None, None],
            #[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
            inflight_readbacks: [None, None, None],
        })
    }

    /// The wgpu device this renderer composites with.
    pub fn device(&self) -> &wgpu::Device {
        &self.device_handle.device
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

    /// Mark every registered video texture dirty so Vello recopies their
    /// (updated) contents into its atlas on the next render.
    pub fn mark_video_textures_dirty(&mut self) {
        #[cfg(target_os = "macos")]
        for state in self.video_textures.values() {
            self.vello_renderer.mark_override_image_dirty(&state.image);
        }
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
}

// ── Backend implementations ───────────────────────────────────────────────

/// The CPU readback backend: renders into an intermediate texture and ships
/// pixels through the webview's shared-memory ring. The backend off macOS
/// (GStreamer media backend) and on macOS when built with `cpu_readback`.
#[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
pub(crate) mod cpu;
/// The zero-copy IOSurface backend (macOS default): renders directly into a
/// shared IOSurface texture.
#[cfg(all(target_os = "macos", not(feature = "cpu_readback")))]
mod iosurface;

/// Per-frame data for the CPU readback path: delivered by the readback map
/// callback when the GPU completes the copy.
#[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
pub use cpu::CpuRenderData;
/// Per-frame data for the zero-copy path: delivered by the poll thread once
/// the render into the shared texture completes (the embedder's blit of the
/// shared surface is then GPU-safe).
#[cfg(all(target_os = "macos", not(feature = "cpu_readback")))]
pub use iosurface::SharedRenderData;

/// A completed frame, delivered on the renderer's single done channel. The
/// message type is chosen at compile time by the surface backend.
#[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
pub type RenderDone = CpuRenderData;
#[cfg(all(target_os = "macos", not(feature = "cpu_readback")))]
pub type RenderDone = SharedRenderData;
