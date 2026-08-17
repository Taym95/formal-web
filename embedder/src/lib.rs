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
}

pub fn run_default(verify: bool, headless: bool) -> Result<(), String> {
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
        run_headed_app(trace_sender, options.startup_url, options.window_title)
    }
}

#[cfg(all(target_os = "macos", not(feature = "winit_embedder")))]
fn run_headed_app(
    trace_sender: Option<TraceSender>,
    startup_url: Option<String>,
    window_title: Option<String>,
) -> Result<(), String> {
    mac_embedder::run_windowed_app(trace_sender, startup_url, window_title)
}

#[cfg(any(not(target_os = "macos"), feature = "winit_embedder"))]
fn run_headed_app(
    trace_sender: Option<TraceSender>,
    startup_url: Option<String>,
    window_title: Option<String>,
) -> Result<(), String> {
    winit_embedder::run_windowed_app(trace_sender, startup_url, window_title)
}

pub fn run_webdriver(args: WebDriverArgs, verify: bool, headless: bool) -> Result<(), String> {
    if args.headless || headless {
        winit_embedder::run_webdriver(args, verify, true)
    } else {
        run_headed_webdriver(args, verify)
    }
}

#[cfg(all(target_os = "macos", not(feature = "winit_embedder")))]
fn run_headed_webdriver(args: WebDriverArgs, verify: bool) -> Result<(), String> {
    mac_embedder::run_webdriver(args, verify)
}

#[cfg(any(not(target_os = "macos"), feature = "winit_embedder"))]
fn run_headed_webdriver(args: WebDriverArgs, verify: bool) -> Result<(), String> {
    winit_embedder::run_webdriver(args, verify, false)
}

pub fn run_cdp(args: CdpArgs, verify: bool, headless: bool) -> Result<(), String> {
    if args.headless || headless {
        winit_embedder::run_cdp(args, verify, true)
    } else {
        run_headed_cdp(args, verify)
    }
}

#[cfg(all(target_os = "macos", not(feature = "winit_embedder")))]
fn run_headed_cdp(args: CdpArgs, verify: bool) -> Result<(), String> {
    mac_embedder::run_cdp(args, verify)
}

#[cfg(any(not(target_os = "macos"), feature = "winit_embedder"))]
fn run_headed_cdp(args: CdpArgs, verify: bool) -> Result<(), String> {
    winit_embedder::run_cdp(args, verify, false)
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
