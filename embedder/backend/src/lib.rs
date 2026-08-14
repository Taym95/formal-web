//! Windowed embedder backend selection.
//!
//! The AppKit backend (`mac-embedder`) is the default on macOS; the winit
//! backend (`winit-embedder`) is the default everywhere else and can be
//! forced on macOS with the `winit_embedder` build config.

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
