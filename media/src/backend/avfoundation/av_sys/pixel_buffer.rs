//! Safe wrappers around `CVPixelBuffer` dimension queries.
//!
//! The decoded pixel buffers are delivered as-is to the graphics process,
//! which wraps them as Metal textures (zero-copy when GPU-backed) instead of
//! reading pixels back to CPU.

use objc2_core_video::{CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth};

/// Width of a pixel buffer in pixels (no lock required).
pub(crate) fn pixel_buffer_width(buf: &CVPixelBuffer) -> u32 {
    CVPixelBufferGetWidth(buf) as u32
}

/// Height of a pixel buffer in pixels (no lock required).
pub(crate) fn pixel_buffer_height(buf: &CVPixelBuffer) -> u32 {
    CVPixelBufferGetHeight(buf) as u32
}
