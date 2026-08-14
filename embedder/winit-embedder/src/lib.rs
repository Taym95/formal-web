//! The winit-based windowed embedder backend: native windows and a
//! Blitz-rendered browser chrome driven by winit's event loop. On macOS this
//! backend is opt-in (the `winit_embedder` build config); the default macOS
//! backend is the AppKit `mac-embedder` crate.

mod windowed;
mod winit_integration;

use embedder_core::{TraceSender, run_winit_event_loop};
pub use windowed::WindowedApp;

/// Run the winit windowed app until it exits.
pub fn run_windowed_app(trace_sender: Option<TraceSender>) -> Result<(), String> {
    run_winit_event_loop(trace_sender.clone(), |provider, _trace_sender| {
        WindowedApp {
            provider: Some(provider),
            ..WindowedApp::default()
        }
    })
}
