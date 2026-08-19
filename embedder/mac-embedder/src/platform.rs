//! Platform helpers for the AppKit embedder: clipboard access, screenshot
//! encoding, startup URL resolution, URL normalization, and the current
//! window viewport snapshot.

use crate::events::{FormalWebUserEvent, send_user_event};
use std::sync::{LazyLock, Mutex, mpsc};
use std::time::Duration;
use webview::ColorScheme;

const STARTUP_ARTIFACT_RELATIVE_PATH: &str = "artifacts/StartupExample.html";

pub fn read_clipboard_text() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| format!("failed to access clipboard: {error}"))?;
    clipboard
        .get_text()
        .map_err(|error| format!("failed to read clipboard text: {error}"))
}

pub fn write_clipboard_text(text: String) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| format!("failed to access clipboard: {error}"))?;
    clipboard
        .set_text(text)
        .map_err(|error| format!("failed to write clipboard text: {error}"))
}

pub fn clipboard_get_text(timeout: Duration) -> Result<String, String> {
    let (reply, receiver) = mpsc::channel();
    send_user_event(FormalWebUserEvent::ClipboardRead { reply })?;
    receiver.recv_timeout(timeout).map_err(|error| {
        format!(
            "timed out after {} ms waiting for clipboard text: {error}",
            timeout.as_millis()
        )
    })?
}

pub fn clipboard_set_text(text: String, timeout: Duration) -> Result<(), String> {
    let (reply, receiver) = mpsc::channel();
    send_user_event(FormalWebUserEvent::ClipboardWrite { text, reply })?;
    receiver.recv_timeout(timeout).map_err(|error| {
        format!(
            "timed out after {} ms waiting to write clipboard text: {error}",
            timeout.as_millis()
        )
    })?
}

type ViewportSnapshot = Option<(u32, u32, f32, ColorScheme)>;
static WINDOW_VIEWPORT_SNAPSHOT: LazyLock<Mutex<ViewportSnapshot>> =
    LazyLock::new(|| Mutex::new(None));

pub fn update_window_viewport_snapshot(snapshot: Option<(u32, u32, f32, ColorScheme)>) {
    *WINDOW_VIEWPORT_SNAPSHOT.lock().expect("poisoned") = snapshot;
}

pub fn window_viewport_snapshot() -> Option<(u32, u32, f32, ColorScheme)> {
    *WINDOW_VIEWPORT_SNAPSHOT.lock().expect("poisoned")
}

pub fn startup_destination_url(startup_url: Option<&str>) -> Result<String, String> {
    match startup_url {
        Some(url) => Ok(url.to_owned()),
        None => startup_artifact_url(),
    }
}

fn startup_artifact_url() -> Result<String, String> {
    let current_dir = std::env::current_dir()
        .map_err(|error| format!("failed to determine current directory: {error}"))?;
    // Try CWD-relative path first, then parent directory (for running from embedder/).
    for base in [current_dir.clone(), current_dir.join("..")] {
        let artifact_path = base.join(STARTUP_ARTIFACT_RELATIVE_PATH);
        if let Ok(canonical) = artifact_path.canonicalize() {
            return Ok(format!("file://{}", canonical.display()));
        }
    }
    Err(format!(
        "startup artifact not found at {} or ../{}",
        STARTUP_ARTIFACT_RELATIVE_PATH, STARTUP_ARTIFACT_RELATIVE_PATH
    ))
}

pub fn normalize_browser_destination(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://") || trimmed.starts_with("about:") {
        return Some(trimmed.to_owned());
    }
    Some(format!("https://{trimmed}"))
}
