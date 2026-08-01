use crate::content::{FrameId, PaintFrame, WebviewId};
use crate::media::{MediaPipelineId, VideoPaintId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[cfg(target_os = "macos")]
use ipc_channel::platform::OsMachPort;

/// Identifies a per-webview compositor slot within the graphics process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CompositorSlotId(pub Uuid);

impl CompositorSlotId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CompositorSlotId {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// GraphicsCommand — messages from user agent → graphics process
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphicsCommand {
    /// Register a new webview compositor slot.
    RegisterWebview { webview_id: WebviewId },
    /// Unregister a webview compositor slot.
    UnregisterWebview { webview_id: WebviewId },
    /// A paint frame (scene + composition metadata) from a content process.
    /// The full PaintFrame with its shmem regions is reconstructed before sending.
    PaintFrame { frame: PaintFrame },
    /// The embedder has finished consuming the pixels of a rendered surface
    /// frame (generation) for a webview; its shared-memory buffer is now
    /// free to be reused. The graphics process must not rewrite a buffer
    /// until this ack arrives for the frame that used it.
    TextureConsumed {
        webview_id: WebviewId,
        generation: u64,
    },
    /// Remove a video frame slot (pipeline destroyed).
    RemoveVideoFrame {
        webview_id: WebviewId,
        paint_id: VideoPaintId,
    },
    /// Create a media pipeline (video playback) internally in the graphics process.
    CreateMediaPipeline {
        pipeline_id: MediaPipelineId,
        url: String,
        webview_id: WebviewId,
        video_paint_id: VideoPaintId,
    },
    /// Start or resume playback of a media pipeline.
    MediaPlay { pipeline_id: MediaPipelineId },
    /// Pause playback of a media pipeline.
    MediaPause { pipeline_id: MediaPipelineId },
    /// Seek a media pipeline to a position.
    MediaSeek {
        pipeline_id: MediaPipelineId,
        position_secs: f64,
    },
    /// Destroy a media pipeline.
    MediaDestroy { pipeline_id: MediaPipelineId },
    /// Register a child navigable host mapping.
    RegisterChildNavigableHost {
        child_webview_id: WebviewId,
        parent_traversable_id: WebviewId,
        content_frame_id: FrameId,
    },
    /// Notify the compositor that a child navigation was finalized.
    ChildNavigationFinalized {
        parent_traversable_id: WebviewId,
        content_frame_id: FrameId,
    },
    /// Notify the compositor that a top-level navigation finalized.
    /// Resets the compositor so the next PaintFrame replaces the old root scene.
    NavigationFinalized { webview_id: WebviewId },
    /// Forward a TLA+ trace sender (dev only, ipc-channel mode).
    /// Sent by the UA right after launch, before any other commands.
    SetTraceSender(Option<verification::TraceSender>),
    /// Shut down the graphics process.
    Shutdown,
}

// ---------------------------------------------------------------------------
// GraphicsEvent — messages from graphics process → user agent
// ---------------------------------------------------------------------------

/// Frame tree node layout data — published by the graphics process for the UA
/// to do hit-testing and event routing. Each node represents one frame (root,
/// iframe child, or video frame slot) with its position and clip rect in root
/// coordinates, plus the transform from child local space to parent space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameHitInfo {
    pub frame_id: FrameId,
    /// The webview that owns this frame.
    pub webview_id: WebviewId,
    /// Parent frame, if this is a child frame.
    pub parent_frame_id: Option<FrameId>,
    /// Viewport width in logical pixels.
    pub viewport_width: u32,
    /// Viewport height in logical pixels.
    pub viewport_height: u32,
    /// Clip rectangle in root coordinates [x0, y0, x1, y1].
    /// The UA checks if a pointer event falls within this rect
    /// to determine which frame the event targets.
    pub root_clip_bounds: [f64; 4],
    /// Affine transform [a, b, c, d, tx, ty] from this frame's local
    /// coordinate space to its parent frame's space. The UA uses this
    /// to convert pointer coordinates when traversing the frame tree.
    pub child_to_parent_transform: [f64; 6],
    /// IDs of direct child frames in this frame's embed tree.
    pub child_frame_ids: Vec<FrameId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphicsEvent {
    /// A rendered surface frame is ready for one webview. The pixels are
    /// delivered according to `payload`:
    ///
    /// - `CpuShmem`: bytes live in the IPC shared memory region carried
    ///   alongside the message; the embedder uploads them in place into its
    ///   persistent per-webview texture.
    /// - `SharedTexture` (macOS): the frame was rendered directly into a
    ///   shared IOSurface; the embedder imports the surface and blits it.
    PixelFrameReady {
        webview_id: WebviewId,
        /// How the rendered pixels are delivered to the embedder.
        payload: SurfacePayload,
        /// True when the composed scene contains animated content (video)
        /// that requires the UA to re-note a rendering opportunity even
        /// without user input.
        animating: bool,
        width: u32,
        height: u32,
        generation: u64,
        frame_hit_info: Vec<FrameHitInfo>,
        child_viewports: Vec<ChildViewport>,
        child_frame_to_webview: HashMap<FrameId, WebviewId>,
    },

    /// A media pipeline reached end of stream. The UA should forward
    /// this to the relevant content process so it can unset any
    /// animating flags.
    VideoEnded {
        webview_id: WebviewId,
        video_paint_id: VideoPaintId,
    },
    /// The graphics process is shutting down.
    ShutdownComplete,
}

/// How a rendered frame's pixels are delivered to the embedder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SurfacePayload {
    /// CPU readback pixels in the IPC shared-memory map (cross-platform).
    CpuShmem {
        /// Key into the IPC shared memory map for the rendered RGBA pixel buffer.
        shmem_key: usize,
    },
    /// macOS zero-copy: the frame was rendered directly into a shared
    /// IOSurface, whose Mach port travels in this variant. The embedder
    /// looks the surface up from the port and blits it.
    #[cfg(target_os = "macos")]
    SharedTexture {
        /// Stable identity of the shared IOSurface; changes on resize.
        texture_id: u64,
        /// Mach port (send right) to the IOSurface.
        port: OsMachPort,
    },
}

/// Viewport data for a child frame (iframe), used by the UA to publish
/// viewport dimensions to child traversables via set_traversable_viewport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildViewport {
    /// The child webview that owns this frame.
    pub child_webview_id: WebviewId,
    /// Clip rectangle in root coordinates [x0, y0, x1, y1].
    pub root_clip_bounds: [f64; 4],
}
