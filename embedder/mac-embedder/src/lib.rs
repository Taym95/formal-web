#![cfg(target_os = "macos")]
//! The AppKit windowed embedder: the default macOS embedder. Runs an
//! NSApplication with NSWindow/NSView/CALayer display, presents composited
//! web content zero-copy by setting the content layer's `contents` to the
//! shared IOSurface from the graphics process, and paces animated content
//! with a CVDisplayLink that requests the next frame via
//! `WebviewProvider::frame_needed`.
//!
//! The crate is self-contained: it shares nothing with the winit embedder
//! except the `webview` crate API. It has no winit, Blitz, or GPU
//! dependencies.

mod app;
mod events;
mod input;
mod platform;
mod window;

use automation::{CdpArgs, WebDriverArgs};
use events::{FormalWebUserEvent, event_loop_is_ready, send_user_event};
use verification::VerificationRun;

pub use app::run_windowed_app;

pub fn run_webdriver(args: WebDriverArgs, verify: bool) -> Result<(), String> {
    let verification_run = if verify {
        Some(
            VerificationRun::start()
                .map_err(|error| format!("failed to start verification: {error}"))?,
        )
    } else {
        None
    };
    let trace_sender = verification_run.as_ref().map(VerificationRun::sender_clone);

    let runtime = automation::automation_bridge(
        |command| send_user_event(FormalWebUserEvent::Automation(command)),
        || send_user_event(FormalWebUserEvent::Exit),
        event_loop_is_ready,
    );
    let webdriver_server = automation::WebDriverServer::start(
        args.port,
        args.exit_on_session_delete,
        runtime.clone(),
    )?;
    let cdp_server = args
        .cdp_port
        .map(|port| automation::CdpServerHandle::start(port, runtime))
        .transpose()?;
    let result = run_windowed_app(
        trace_sender,
        args.startup_url
            .or_else(|| Some(String::from("about:blank"))),
        Some(format!("formal-web WebDriver :{}", args.port)),
    );
    drop(cdp_server);
    drop(webdriver_server);

    let verification_result = verification_run
        .map(VerificationRun::finish)
        .unwrap_or(Ok(()));
    combine_results(result, verification_result)
}

pub fn run_cdp(args: CdpArgs, verify: bool) -> Result<(), String> {
    let verification_run = if verify {
        Some(
            VerificationRun::start()
                .map_err(|error| format!("failed to start verification: {error}"))?,
        )
    } else {
        None
    };
    let trace_sender = verification_run.as_ref().map(VerificationRun::sender_clone);

    let runtime = automation::automation_bridge(
        |command| send_user_event(FormalWebUserEvent::Automation(command)),
        || send_user_event(FormalWebUserEvent::Exit),
        event_loop_is_ready,
    );
    let server = automation::CdpServerHandle::start(args.port, runtime)?;
    let result = run_windowed_app(
        trace_sender,
        args.startup_url
            .or_else(|| Some(String::from("about:blank"))),
        Some(format!("formal-web CDP :{}", args.port)),
    );
    drop(server);

    let verification_result = verification_run
        .map(VerificationRun::finish)
        .unwrap_or(Ok(()));
    combine_results(result, verification_result)
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
