//! macOS video texture import: wraps a CVPixelBuffer as a Metal texture
//! (zero-copy when the buffer is GPU-backed) and blits BGRA → RGBA into a
//! texture Vello can register (Vello's `register_texture` requires
//! Rgba8Unorm + COPY_SRC). Lives behind the renderer trait's
//! `import_video_frame` (macOS); the AVFoundation media backend delivers
//! `PixelBufferFrame` events and the renderer hands each frame's pixel
//! buffer here.

use ipc_messages::media::VideoPaintId;
use log::error;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer, kCVReturnSuccess,
};
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture};
use std::collections::HashMap;
use vello::Renderer as VelloRenderer;
use wgpu::{
    CommandEncoderDescriptor, ComputePipelineDescriptor, Extent3d, Texture, TextureDescriptor,
    TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
};

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
struct VideoTexture {
    /// RGBA8 conversion target; its contents are what Vello samples.
    texture: Texture,
    /// Fake image data referencing `texture` via Vello's override_image;
    /// composed scenes draw this as a plain `Paint::Image` brush.
    image: peniko::ImageData,
    /// The pixel buffer whose contents are in `texture`; keeps the
    /// source alive until the next frame replaces it.
    pixel_buffer: Retained<CVPixelBuffer>,
    width: u32,
    height: u32,
}

struct VideoImport {
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
    fn new(
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
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
    fn blit_bgra_into_rgba(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pixel_buffer: &Retained<CVPixelBuffer>,
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

/// The per-renderer video texture state: the Metal texture cache + blit
/// pipeline, and one RGBA target texture per video paint id. The AVFoundation
/// media backend delivers `PixelBufferFrame` events; each frame's pixel
/// buffer is blitted into its paint's target texture and registered with the
/// renderer's Vello via `override_image`.
pub(super) struct VideoTextures {
    import: Option<VideoImport>,
    textures: HashMap<VideoPaintId, VideoTexture>,
}

/// The renderer's GPU resources the video texture manager borrows for a
/// frame import.
pub(super) struct RenderResources<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub vello_renderer: &'a mut VelloRenderer,
    pub raw_device: &'a Retained<ProtocolObject<dyn MTLDevice>>,
}

impl VideoTextures {
    pub(super) fn new() -> Self {
        Self {
            import: None,
            textures: HashMap::new(),
        }
    }

    /// Register a video frame: wrap `pixel_buffer` as a Metal texture and
    /// blit it into an RGBA texture Vello can sample. Returns the fake
    /// `ImageData` referencing the texture (via `override_image`) to embed
    /// in composed scenes as a plain image brush. The same image data is
    /// reused while the frame size is unchanged.
    pub(super) fn import_frame(
        &mut self,
        resources: RenderResources,
        paint_id: VideoPaintId,
        pixel_buffer: &Retained<CVPixelBuffer>,
        width: u32,
        height: u32,
    ) -> Option<peniko::ImageData> {
        let RenderResources {
            device,
            queue,
            vello_renderer,
            raw_device,
        } = resources;
        let video_import = match &self.import {
            Some(video_import) => video_import,
            None => {
                let video_import = VideoImport::new(device, raw_device)?;
                self.import = Some(video_import);
                self.import.as_ref()?
            }
        };

        let needs_new = match self.textures.get(&paint_id) {
            Some(existing) => existing.width != width || existing.height != height,
            None => true,
        };
        if needs_new {
            if let Some(old) = self.textures.remove(&paint_id) {
                // Drop the old override so its blob id no longer samples.
                vello_renderer.override_image(&old.image, None);
            }
            let target = device.create_texture(&TextureDescriptor {
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
                device,
                queue,
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
            vello_renderer.override_image(
                &image,
                Some(wgpu::TexelCopyTextureInfoBase {
                    texture: target.clone(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                }),
            );
            self.textures.insert(
                paint_id,
                VideoTexture {
                    texture: target,
                    image: image.clone(),
                    pixel_buffer: pixel_buffer.clone(),
                    width,
                    height,
                },
            );
            return Some(image);
        }

        let state = self.textures.get_mut(&paint_id)?;
        // Same-size frame: blit into the existing target and keep the
        // pixel buffer alive for the frame's lifetime.
        let _source = video_import.blit_bgra_into_rgba(
            device,
            queue,
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
    pub(super) fn mark_dirty(&self, vello_renderer: &mut VelloRenderer) {
        for state in self.textures.values() {
            vello_renderer.mark_override_image_dirty(&state.image);
        }
    }
}
