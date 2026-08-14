//! Facade over the shared embedder plumbing (`embedder-core`): the CLI
//! entry points for the `formal-web-embedder` binary and the windowed
//! backend installation used by this crate and the root `formal-web`
//! binary.

pub use embedder_core::*;

/// Install the default windowed embedder backend for the current platform
/// and build config (AppKit on macOS by default, winit elsewhere, winit
/// whenever the `winit_embedder` config is enabled). Call once at startup,
/// before running the headed app.
pub fn install_default_windowed_backend() {
    embedder_backend::install_default_windowed_backend();
}
