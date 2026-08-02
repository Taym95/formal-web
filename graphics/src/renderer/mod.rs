//! Per-webview GPU renderers behind a common [`SurfaceRenderer`] trait. The
//! graphics process event loop operates on the renderer only through this
//! trait; the concrete implementation is chosen at compile time by the
//! surface backend features (see graphics/README.md): the CPU readback
//! backend (`renderer/cpu.rs`) off macOS and with the `cpu_readback`
//! feature, the zero-copy IOSurface backend (`renderer/iosurface.rs`) on
//! macOS by default.
//!
//! Each backend defines its own [`SurfaceRenderer::RenderData`] associated
//! type — the per-frame payload produced at submit time and consumed by
//! [`SurfaceRenderer::handle_render_done`] when the GPU completes the frame.
//! The shared [`GpuContext`] holds what every backend needs (the wgpu
//! device, the Vello renderer, the video texture machinery); each backend
//! owns its surface buffers ([`SurfaceBuffers`]) and its delivery path.

use anyrender::PaintScene;
use ipc_messages::content::{FrameId, WebviewId};
use ipc_messages::graphics::{ChildViewport, FrameHitInfo, GraphicsEvent};
use ipc_messages::media::VideoPaintId;
use kurbo::Affine;
use std::collections::HashMap;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::ProtocolObject;
#[cfg(target_os = "macos")]
use objc2_core_video::CVPixelBuffer;
#[cfg(target_os = "macos")]
use objc2_metal::MTLDevice;
use vello::{
    AaConfig, AaSupport, RenderParams, Renderer as VelloRenderer, RendererOptions,
    Scene as VelloScene,
};
use wgpu::{Texture, TextureViewDescriptor};

use crate::ComposedScene;
use verification::TLATracer;

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

/// The outcome of submitting a composed scene.
#[derive(Debug)]
pub enum RenderError {
    /// Every surface buffer was reserved or pending; the scene was retained
    /// for submission once an ack frees a buffer.
    Deferred,
    /// Buffer allocation or GPU failure; the scene was dropped.
    Failed,
}

/// A request for the GPU poll thread to block until the given device
/// submission completes. The readback map callbacks (CPU path) fire there
/// and deliver the backend's `RenderData`; when `done` is present
/// (zero-copy path, which has no map callback) the poll thread delivers it
/// after the poll.
pub struct PollRequest<D> {
    pub device: wgpu_context::DeviceHandle,
    /// The submission to wait for; `None` waits for all submitted work
    /// (used by the shared-texture path, where Vello submits internally).
    pub submission_index: Option<wgpu::SubmissionIndex>,
    /// Zero-copy path: delivered after the GPU work completes.
    pub done: Option<D>,
}

/// The channels connecting the renderers to the GPU poll thread and the
/// main loop, created once at graphics-process startup. `D` is the surface
/// backend's `RenderData`.
pub struct ReadbackChannels<D> {
    /// Requests for the poll thread to block on a device submission.
    pub poll_tx: crossbeam_channel::Sender<PollRequest<D>>,
    /// Completed frames: delivered by the readback map callbacks (CPU path)
    /// and by the poll thread (zero-copy path).
    pub render_done_tx: crossbeam_channel::Sender<D>,
}

impl<D> ReadbackChannels<D> {
    /// Create the channels plus the receivers, for the poll thread and the
    /// main loop.
    pub fn new() -> (
        Self,
        crossbeam_channel::Receiver<PollRequest<D>>,
        crossbeam_channel::Receiver<D>,
    ) {
        let (poll_tx, poll_rx) = crossbeam_channel::unbounded();
        let (render_done_tx, render_done_rx) = crossbeam_channel::unbounded();
        (
            Self {
                poll_tx,
                render_done_tx,
            },
            poll_rx,
            render_done_rx,
        )
    }
}

impl<D> Clone for ReadbackChannels<D> {
    fn clone(&self) -> Self {
        Self {
            poll_tx: self.poll_tx.clone(),
            render_done_tx: self.render_done_tx.clone(),
        }
    }
}

/// The shared per-webview GPU state every surface backend needs: the wgpu
/// device, the Vello renderer, the video texture machinery, and the frame
/// generation counter. Backend-specific state (surface buffers, staging
/// buffers) lives on the concrete renderers.
pub(crate) struct GpuContext {
    pub(crate) device_handle: wgpu_context::DeviceHandle,
    vello_renderer: VelloRenderer,
    vello_scene: VelloScene,
    pub(crate) generation: u64,
    #[cfg(target_os = "macos")]
    video: video::VideoTextures,
}

impl GpuContext {
    pub(crate) fn new() -> Result<Self, String> {
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
            generation: 0,
            #[cfg(target_os = "macos")]
            video: video::VideoTextures::new(),
        })
    }

    /// The raw Metal device backing this renderer (macOS), needed to create
    /// IOSurface-backed Metal textures and the video Metal texture cache.
    #[cfg(target_os = "macos")]
    pub(crate) fn raw_metal_device(&self) -> Option<Retained<ProtocolObject<dyn MTLDevice>>> {
        // SAFETY: the hal device is this renderer's own device; the returned
        // raw Metal device is used only to create textures on it.
        let hal_device = unsafe { self.device_handle.device.as_hal::<wgpu::hal::metal::Api>() }?;
        Some(hal_device.raw_device().clone())
    }

    /// Paint `scene` into `target` with Vello's compute renderer.
    pub(crate) fn render_into(
        &mut self,
        scene: &anyrender::Scene,
        target: &Texture,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        self.vello_scene.reset();
        {
            let mut painter = anyrender_vello::VelloScenePainter::new(&mut self.vello_scene);
            painter.append_scene(scene.clone(), Affine::IDENTITY);
        }

        let view = target.create_view(&TextureViewDescriptor::default());
        self.vello_renderer
            .render_to_texture(
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
            )
            .map_err(|e| format!("Vello render failed: {e:?}"))
    }

    /// Mark every registered video texture dirty so Vello recopies their
    /// (updated) contents into its atlas on the next render.
    pub(crate) fn mark_video_textures_dirty(&mut self) {
        #[cfg(target_os = "macos")]
        self.video.mark_dirty(&mut self.vello_renderer);
    }

    /// Register a video frame: wrap `pixel_buffer` as a Metal texture and
    /// blit it into an RGBA texture Vello can sample. Returns the fake
    /// `ImageData` referencing the texture (via `override_image`) to embed
    /// in composed scenes as a plain image brush.
    #[cfg(target_os = "macos")]
    pub(crate) fn import_video_frame(
        &mut self,
        paint_id: VideoPaintId,
        pixel_buffer: &Retained<CVPixelBuffer>,
        width: u32,
        height: u32,
    ) -> Option<peniko::ImageData> {
        let raw_device = self.raw_metal_device()?;
        self.video.import_frame(
            video::RenderResources {
                device: &self.device_handle.device,
                queue: &self.device_handle.queue,
                vello_renderer: &mut self.vello_renderer,
                raw_device: &raw_device,
            },
            paint_id,
            pixel_buffer,
            width,
            height,
        )
    }
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
pub(crate) struct SurfaceRingState {
    state: [BufferState; 3],
    /// Ring index where the next free buffer search starts.
    write_index: usize,
    width: u32,
    height: u32,
}

impl SurfaceRingState {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            state: [BufferState::Free, BufferState::Free, BufferState::Free],
            write_index: 0,
            width,
            height,
        }
    }

    /// Index of the next free buffer to pick, scanning the ring from
    /// `write_index`; None when every buffer is reserved or pending.
    pub(crate) fn next_free(&self) -> Option<usize> {
        (0..3)
            .map(|offset| (self.write_index + offset) % 3)
            .find(|index| self.state[*index] == BufferState::Free)
    }

    /// Mark the buffer picked for a submitted frame.
    pub(crate) fn reserve(&mut self, index: usize, generation: u64) {
        debug_assert!(self.state[index] == BufferState::Free);
        self.state[index] = BufferState::Reserved(generation);
        self.write_index = (index + 1) % 3;
    }

    /// Move the buffer from Reserved to Pending once its pixels are delivered.
    pub(crate) fn mark_pending(&mut self, index: usize, generation: u64) {
        debug_assert!(self.state[index] == BufferState::Reserved(generation));
        self.state[index] = BufferState::Pending(generation);
    }

    /// Free the buffer holding `generation` (the embedder's ack). Returns
    /// true when a pending buffer was found and freed.
    pub(crate) fn ack(&mut self, generation: u64) -> bool {
        for index in 0..3 {
            if self.state[index] == BufferState::Pending(generation) {
                self.state[index] = BufferState::Free;
                return true;
            }
        }
        false
    }
}

/// The per-webview frame buffers for one surface backend: the shared ring
/// lifecycle plus the backend's buffer payloads (shared-memory regions on
/// the CPU path, IOSurface textures on the zero-copy path).
pub(crate) struct SurfaceBuffers<P> {
    ring: SurfaceRingState,
    payload: P,
}

impl<P> SurfaceBuffers<P> {
    pub(crate) fn new(ring: SurfaceRingState, payload: P) -> Self {
        Self { ring, payload }
    }

    pub(crate) fn ring(&self) -> &SurfaceRingState {
        &self.ring
    }

    pub(crate) fn ring_mut(&mut self) -> &mut SurfaceRingState {
        &mut self.ring
    }

    pub(crate) fn payload(&self) -> &P {
        &self.payload
    }

    /// Mutable payload access; only the CPU readback backend writes into its
    /// shared-memory regions when a readback completes.
    #[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
    pub(crate) fn payload_mut(&mut self) -> &mut P {
        &mut self.payload
    }

    pub(crate) fn next_free(&self) -> Option<usize> {
        self.ring.next_free()
    }

    pub(crate) fn reserve(&mut self, index: usize, generation: u64) {
        self.ring.reserve(index, generation);
    }

    pub(crate) fn ack(&mut self, generation: u64) -> bool {
        self.ring.ack(generation)
    }
}

/// The render size for a composed scene: the root frame's viewport, clamped
/// to at least 1×1 so a frame is always produced (a skipped frame would
/// leave the UA's rendering cycle open and stall all future renders).
pub(crate) fn render_size(frame_hit_info: &[FrameHitInfo]) -> (u32, u32) {
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
    (width, height)
}

/// Build the frame metadata delivered with the completed frame, converting
/// the child viewport map to the wire `ChildViewport` list.
pub(crate) fn frame_metadata(
    webview_id: WebviewId,
    frame_hit_info: Vec<FrameHitInfo>,
    child_viewports: HashMap<WebviewId, [f64; 4]>,
    child_frame_to_webview: HashMap<FrameId, WebviewId>,
    animating: bool,
) -> FrameMetadata {
    let child_ports: Vec<ChildViewport> = child_viewports
        .into_iter()
        .map(|(child_webview_id, root_clip_bounds)| ChildViewport {
            child_webview_id,
            root_clip_bounds,
        })
        .collect();
    FrameMetadata {
        webview_id,
        frame_hit_info,
        child_viewports: child_ports,
        child_frame_to_webview,
        animating,
    }
}

/// A per-webview GPU renderer: submits a composed scene's render and, when
/// the GPU completes it, delivers the pixels to the embedder. The associated
/// `RenderData` is the per-frame payload produced at submit time and
/// consumed by [`handle_render_done`](Self::handle_render_done); each
/// surface backend provides its own implementation (CPU readback + shared
/// memory, or zero-copy IOSurface).
pub trait SurfaceRenderer {
    /// Per-frame data produced at submit time, consumed at GPU completion.
    type RenderData: Send + 'static;

    /// Create a renderer for one webview: the wgpu device, Vello renderer,
    /// and readback plumbing.
    fn new(channels: ReadbackChannels<Self::RenderData>) -> Result<Self, String>
    where
        Self: Sized;

    /// Submit `composed` for rendering. The renderer derives the render
    /// size, (re)allocates its surface buffers on resize, picks a free ring
    /// buffer — or retains the scene when every buffer is busy — and submits
    /// the render. The GPU completion is delivered on
    /// `ReadbackChannels::render_done_tx` as `Self::RenderData`.
    fn submit_scene(
        &mut self,
        composed: ComposedScene,
        tla_tracer: &mut TLATracer,
    ) -> Result<(), RenderError>;

    /// The GPU completed a frame: mark the ring buffer pending and deliver
    /// the frame to the embedder.
    fn handle_render_done(
        &mut self,
        data: Self::RenderData,
        sender: &ipc::IpcSender<GraphicsEvent>,
        tla_tracer: &mut TLATracer,
    );

    /// The webview a completed frame belongs to (used to look up the
    /// webview's renderer).
    fn render_done_webview_id(data: &Self::RenderData) -> WebviewId;

    /// Free the ring buffer holding `generation` (the embedder's ack).
    /// Returns true when a pending buffer was found and freed.
    fn ack(&mut self, generation: u64) -> bool;

    /// Submit the scene that was retained because every ring buffer was
    /// busy. Returns true when there was a deferred scene.
    fn submit_deferred(&mut self, tla_tracer: &mut TLATracer) -> bool;

    /// Register a video frame: wrap `pixel_buffer` as a Metal texture and
    /// blit it into an RGBA texture Vello can sample. Returns the fake
    /// `ImageData` referencing the texture to embed in composed scenes.
    #[cfg(target_os = "macos")]
    fn import_video_frame(
        &mut self,
        paint_id: VideoPaintId,
        pixel_buffer: &Retained<CVPixelBuffer>,
        width: u32,
        height: u32,
    ) -> Option<peniko::ImageData>;
}

#[cfg(target_os = "macos")]
mod video;

/// The CPU readback backend: renders into an intermediate texture and ships
/// pixels through the webview's shared-memory ring. The backend off macOS
/// (GStreamer media backend) and on macOS when built with `cpu_readback`.
#[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
pub(crate) mod cpu;
/// The zero-copy IOSurface backend (macOS default): renders directly into a
/// shared IOSurface texture.
#[cfg(all(target_os = "macos", not(feature = "cpu_readback")))]
mod iosurface;

#[cfg(any(not(target_os = "macos"), feature = "cpu_readback"))]
pub use cpu::{CpuRenderData, CpuRenderer};
#[cfg(all(target_os = "macos", not(feature = "cpu_readback")))]
pub use iosurface::{IosurfaceRenderer, SharedRenderData};
