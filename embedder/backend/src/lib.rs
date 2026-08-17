//! Windowed embedder backend selection.
//!
//! On macOS the AppKit backend (`mac-embedder`) is the default and the only
//! windowed backend compiled unless the `winit_embedder` feature is
//! enabled; enabling it builds and selects the winit backend
//! (`winit-embedder`) instead. On other platforms the winit backend is the
//! only option and the feature is a no-op.

/// Install the default windowed backend for the current platform and build
/// config. Call once at startup, before running the headed app.
pub fn install_default_windowed_backend() {
    #[cfg(all(target_os = "macos", not(feature = "winit_embedder")))]
    {
        embedder_core::install_windowed_backend(mac_embedder::run_windowed_app);
    }
    #[cfg(any(not(target_os = "macos"), feature = "winit_embedder"))]
    {
        embedder_core::install_windowed_backend(winit_embedder::run_windowed_app);
    }
}
