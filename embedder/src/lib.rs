//! CLI entry points for the `formal-web` and `formal-web-embedder`
//! binaries, and the windowed-backend selection: AppKit on macOS by
//! default, the winit windowed backend elsewhere or whenever the
//! `winit_embedder` feature is enabled. The embedders themselves share
//! nothing but the `webview` crate API.

use automation::{CdpArgs, WebDriverArgs};
use verification::{TraceSender, VerificationRun};

#[derive(Clone, Default)]
pub struct AppRunOptions {
    pub headless: bool,
    pub startup_url: Option<String>,
    pub window_title: Option<String>,
    pub trace_sender: Option<TraceSender>,
    /// When set, start a CDP server on this port. The AppKit embedder uses
    /// it to expose real-pixel screenshots (and navigation/evaluation) to
    /// the pi browser tooling; the winit automation entry points use the
    /// `cdp` subcommand instead.
    pub cdp_port: Option<u16>,
}

pub fn run_default(verify: bool, headless: bool, cdp_port: Option<u16>) -> Result<(), String> {
    let verification_run = if verify {
        Some(
            VerificationRun::start()
                .map_err(|error| format!("failed to start verification: {error}"))?,
        )
    } else {
        None
    };
    let trace_sender = verification_run.as_ref().map(VerificationRun::sender_clone);

    let result = run_app_with_options(AppRunOptions {
        headless,
        cdp_port,
        trace_sender,
        ..AppRunOptions::default()
    });

    let verification_result = verification_run
        .map(VerificationRun::finish)
        .unwrap_or(Ok(()));
    combine_results(result, verification_result)
}

pub fn run_app_with_options(options: AppRunOptions) -> Result<(), String> {
    let trace_sender = options.trace_sender;
    if options.headless {
        winit_embedder::run_headless_app(trace_sender, options.startup_url, options.window_title)
    } else {
        run_headed_app(
            trace_sender,
            options.startup_url,
            options.window_title,
            options.cdp_port,
        )
    }
}

#[cfg(all(target_os = "macos", not(feature = "winit_embedder")))]
fn run_headed_app(
    trace_sender: Option<TraceSender>,
    startup_url: Option<String>,
    window_title: Option<String>,
    cdp_port: Option<u16>,
) -> Result<(), String> {
    mac_embedder::run_windowed_app(trace_sender, startup_url, window_title, cdp_port)
}

#[cfg(any(not(target_os = "macos"), feature = "winit_embedder"))]
fn run_headed_app(
    trace_sender: Option<TraceSender>,
    startup_url: Option<String>,
    window_title: Option<String>,
    cdp_port: Option<u16>,
) -> Result<(), String> {
    let _ = cdp_port;
    winit_embedder::run_windowed_app(trace_sender, startup_url, window_title)
}

/// Automation (WebDriver, CDP) always runs on the winit embedder, never
/// the AppKit embedder: the winit backend is the single automation port.
/// Headed automation on macOS requires the `winit_embedder` feature (the
/// winit windowed app is otherwise not compiled); headless automation
/// works on any build.
pub fn run_webdriver(args: WebDriverArgs, verify: bool, headless: bool) -> Result<(), String> {
    winit_embedder::run_webdriver(args, verify, headless)
}

/// Automation (WebDriver, CDP) always runs on the winit embedder, never
/// the AppKit embedder: the winit backend is the single automation port.
/// Headed automation on macOS requires the `winit_embedder` feature (the
/// winit windowed app is otherwise not compiled); headless automation
/// works on any build.
pub fn run_cdp(args: CdpArgs, verify: bool, headless: bool) -> Result<(), String> {
    winit_embedder::run_cdp(args, verify, headless)
}

fn combine_results(
    primary: Result<(), String>,
    final_step: Result<(), String>,
) -> Result<(), String> {
    match (primary, final_step) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(final_error)) => Err(format!("{error}; {final_error}")),
    }
}
