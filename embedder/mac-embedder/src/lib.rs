#![cfg(target_os = "macos")]
//! The AppKit windowed embedder backend: the default macOS embedder.
//! Runs an NSApplication with NSWindow/NSView/CALayer display, presents
//! composited web content zero-copy by setting the content layer's
//! `contents` to the shared IOSurface from the graphics process, and paces
//! animated content with a CVDisplayLink that requests the next frame via
//! `WebviewProvider::frame_needed`.

mod app;
mod input;
mod window;

pub use app::run_windowed_app;
