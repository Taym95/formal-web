pub mod chrome;
mod headless;

use self::headless::HeadlessEmbedderApp;
use anyrender::{PaintScene, render_to_buffer};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use automation::AutomationCommand;
use blitz_traits::events::BlitzKeyEvent;
use blitz_traits::shell::ColorScheme;
use ipc_messages::content::WebviewId;
use kurbo::{Affine, Rect};
use log::error;
use peniko::{Color, Fill};
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::time::Duration;
use verification::TraceSender;
use webview::{Embedder, WebviewProvider};
use winit::application::ApplicationHandler;
use winit::event_loop::{EventLoop, EventLoopProxy};

const STARTUP_ARTIFACT_RELATIVE_PATH: &str = "artifacts/StartupExample.html";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavigationCompletion {
    Committed { url: String },
    Aborted { message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationCompleted {
    pub webview_id: WebviewId,
    pub status: NavigationCompletion,
}

/// The windowed embedder backend entry point: receives the trace sender and
/// runs the app's own event loop until the app exits. Installed by the
/// `embedder-backend` crate before the headed app runs.
pub type WindowedRunner = fn(Option<TraceSender>) -> Result<(), String>;

static WINDOWED_RUNNER: LazyLock<Mutex<Option<WindowedRunner>>> =
    LazyLock::new(|| Mutex::new(None));

pub fn install_windowed_backend(runner: WindowedRunner) {
    *WINDOWED_RUNNER
        .lock()
        .expect("windowed runner mutex poisoned") = Some(runner);
}

/// The user-event bus: how the user agent (any thread) hands events to the
/// app's event loop (main thread).
pub trait UserEventSink: Send + Sync {
    fn send(&self, event: FormalWebUserEvent) -> Result<(), String>;
}

/// Winit-backed sink: forwards events through the winit event loop proxy.
#[derive(Clone)]
pub struct WinitEventSink {
    proxy: EventLoopProxy<FormalWebUserEvent>,
}

impl WinitEventSink {
    pub fn new(proxy: EventLoopProxy<FormalWebUserEvent>) -> Self {
        Self { proxy }
    }
}

impl UserEventSink for WinitEventSink {
    fn send(&self, event: FormalWebUserEvent) -> Result<(), String> {
        self.proxy
            .send_event(event)
            .map_err(|error| format!("failed to send user event: {error}"))
    }
}

static USER_EVENT_SINK: LazyLock<Mutex<Option<Arc<dyn UserEventSink>>>> =
    LazyLock::new(|| Mutex::new(None));

pub fn install_user_event_sink(sink: Arc<dyn UserEventSink>) {
    *USER_EVENT_SINK
        .lock()
        .expect("user event sink mutex poisoned") = Some(sink);
}

pub fn clear_user_event_sink() {
    *USER_EVENT_SINK
        .lock()
        .expect("user event sink mutex poisoned") = None;
}

pub fn send_user_event(event: FormalWebUserEvent) -> Result<(), String> {
    let guard = USER_EVENT_SINK
        .lock()
        .expect("user event sink mutex poisoned");
    match guard.as_ref() {
        Some(sink) => sink.send(event),
        None => Err(String::from("user event sink is not installed")),
    }
}

pub fn event_loop_is_ready() -> bool {
    USER_EVENT_SINK
        .lock()
        .expect("user event sink mutex poisoned")
        .is_some()
}

#[derive(Clone, Default)]
pub struct EventLoopOptions {
    pub startup_url: Option<String>,
    pub window_title: Option<String>,
}

static EVENT_LOOP_OPTIONS: LazyLock<Mutex<EventLoopOptions>> =
    LazyLock::new(|| Mutex::new(EventLoopOptions::default()));

pub fn set_event_loop_options(options: EventLoopOptions) {
    *EVENT_LOOP_OPTIONS
        .lock()
        .expect("event loop options mutex poisoned") = options;
}

pub fn clear_event_loop_options() {
    *EVENT_LOOP_OPTIONS
        .lock()
        .expect("event loop options mutex poisoned") = EventLoopOptions::default();
}

pub fn event_loop_options() -> EventLoopOptions {
    EVENT_LOOP_OPTIONS
        .lock()
        .expect("event loop options mutex poisoned")
        .clone()
}

pub struct EventLoopEmbedder {
    sink: Arc<dyn UserEventSink>,
}

impl EventLoopEmbedder {
    pub fn new(sink: Arc<dyn UserEventSink>) -> Self {
        Self { sink }
    }
}

impl Embedder for EventLoopEmbedder {
    fn navigation_requested(
        &self,
        webview_id: WebviewId,
        destination_url: String,
    ) -> Result<(), String> {
        self.sink.send(FormalWebUserEvent::NavigationRequested {
            webview_id,
            destination_url,
        })
    }

    fn navigation_completed(&self, completed: webview::NavigationCompleted) -> Result<(), String> {
        let status = match completed.status {
            webview::NavigationCompletion::Committed { url } => {
                NavigationCompletion::Committed { url }
            }
            webview::NavigationCompletion::Aborted { message } => {
                NavigationCompletion::Aborted { message }
            }
        };
        self.sink.send(FormalWebUserEvent::NavigationCompleted(
            NavigationCompleted {
                webview_id: completed.webview_id,
                status,
            },
        ))
    }

    fn new_webview(&self, webview_id: WebviewId, target_name: String) -> Result<(), String> {
        log::debug!(
            "[embedder] Embedder::new_webview webview={:?} target={}",
            webview_id,
            target_name
        );
        self.sink
            .send(FormalWebUserEvent::NewWebview(webview_id, target_name))
    }

    fn request_redraw(&self, webview_id: WebviewId) {
        if let Err(error) = self
            .sink
            .send(FormalWebUserEvent::RequestRedraw(webview_id))
        {
            error!("failed to request redraw for webview {webview_id:?}: {error}");
        }
    }

    fn viewport_scale_factor(&self) -> f32 {
        window_viewport_snapshot()
            .map(|(_, _, scale, _)| scale)
            .unwrap_or(1.0)
    }

    fn window_viewport_snapshot(&self) -> Option<(u32, u32, f32, ColorScheme)> {
        window_viewport_snapshot()
    }

    fn clipboard_get_text(&self, timeout: Duration) -> Result<String, String> {
        clipboard_get_text(timeout)
    }

    fn clipboard_set_text(&self, text: String, timeout: Duration) -> Result<(), String> {
        clipboard_set_text(text, timeout)
    }

    fn title_changed(&self, webview_id: WebviewId, title: String) -> Result<(), String> {
        send_user_event(FormalWebUserEvent::TitleChanged { webview_id, title })
    }

    fn new_web_content_scene(
        &self,
        webview_id: WebviewId,
        scene_bytes: Vec<u8>,
        font_registrations: Vec<ipc_messages::content::RegisteredFont>,
        font_data: std::collections::HashMap<usize, Vec<u8>>,
    ) -> Result<(), String> {
        self.sink.send(FormalWebUserEvent::NewWebContentScene {
            webview_id,
            scene_bytes,
            font_registrations,
            font_data,
        })
    }

    fn new_web_content_surface(
        &self,
        webview_id: WebviewId,
        frame: ipc_messages::graphics::SurfaceFrame,
        width: u32,
        height: u32,
        generation: u64,
        animating: bool,
    ) -> Result<(), String> {
        self.sink
            .send(FormalWebUserEvent::NewWebContentSurface {
                webview_id,
                frame,
                width,
                height,
                generation,
                animating,
            })
            .map_err(|error| format!("failed to send surface event: {error}"))
    }
}

pub enum FormalWebUserEvent {
    RequestRedraw(WebviewId),
    NewWebContentScene {
        webview_id: WebviewId,
        scene_bytes: Vec<u8>,
        font_registrations: Vec<ipc_messages::content::RegisteredFont>,
        font_data: std::collections::HashMap<usize, Vec<u8>>,
    },
    NewWebContentSurface {
        webview_id: WebviewId,
        /// The rendered surface frame: how the pixels are delivered (CPU
        /// shared memory vs. shared IOSurface on macOS) and the payload.
        frame: ipc_messages::graphics::SurfaceFrame,
        width: u32,
        height: u32,
        generation: u64,
        /// Whether the composed scene contains animated content (video, CSS
        /// animations) that needs the next frame at display cadence.
        animating: bool,
    },
    NavigationRequested {
        webview_id: WebviewId,
        destination_url: String,
    },
    NavigationCompleted(NavigationCompleted),
    #[allow(dead_code)]
    NewWebview(WebviewId, String),
    CreateWindow,
    Automation(AutomationCommand),
    ClipboardRead {
        reply: mpsc::Sender<Result<String, String>>,
    },
    ClipboardWrite {
        text: String,
        reply: mpsc::Sender<Result<(), String>>,
    },
    /// The parsed title of a top-level document, for tab and window labels.
    TitleChanged {
        webview_id: WebviewId,
        title: String,
    },
    Exit,
}

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

/// Run a winit-based app (headless, or the winit windowed backend) on a
/// winit event loop: installs the event sink, starts the user agent, and
/// runs the app until it exits.
pub fn run_winit_event_loop<A, MakeApp>(
    trace_sender: Option<TraceSender>,
    make_app: MakeApp,
) -> Result<(), String>
where
    A: ApplicationHandler<FormalWebUserEvent>,
    MakeApp: FnOnce(WebviewProvider, Option<TraceSender>) -> A,
{
    let event_loop = EventLoop::<FormalWebUserEvent>::with_user_event()
        .build()
        .map_err(|error| format!("failed to create event loop: {error}"))?;
    let sink: Arc<dyn UserEventSink> = Arc::new(WinitEventSink::new(event_loop.create_proxy()));
    install_user_event_sink(sink.clone());

    let event_loop_embedder = Arc::new(EventLoopEmbedder::new(sink));
    let provider = match WebviewProvider::new(event_loop_embedder, trace_sender.clone()) {
        Ok(provider) => provider,
        Err(error) => {
            clear_user_event_sink();
            update_window_viewport_snapshot(None);
            return Err(error);
        }
    };

    let mut app = make_app(provider, trace_sender);
    let run_result = event_loop
        .run_app(&mut app)
        .map_err(|error| format!("winit event loop failed: {error}"));

    clear_user_event_sink();
    update_window_viewport_snapshot(None);

    run_result
}

pub fn run_headed_event_loop(trace_sender: Option<TraceSender>) -> Result<(), String> {
    let runner = *WINDOWED_RUNNER
        .lock()
        .expect("windowed runner mutex poisoned");
    match runner {
        Some(runner) => runner(trace_sender),
        None => Err(String::from(
            "no windowed embedder backend installed (enable the `winit_embedder` or `mac_embedder` build config)",
        )),
    }
}

pub fn run_headless_event_loop(trace_sender: Option<TraceSender>) -> Result<(), String> {
    run_winit_event_loop(trace_sender.clone(), |provider, _trace_sender| {
        HeadlessEmbedderApp {
            provider: Some(provider),
            ..HeadlessEmbedderApp::default()
        }
    })
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

pub fn automation_screenshot_png(
    _provider: &mut Option<WebviewProvider>,
    _current_webview_id: Option<WebviewId>,
) -> Result<Vec<u8>, String> {
    let Some((width, height, _, _)) = window_viewport_snapshot() else {
        return Err(String::from("content viewport is not initialized"));
    };
    if width == 0 || height == 0 {
        return Err(String::from("content viewport is zero-sized"));
    }

    let rgba = render_to_buffer::<VelloCpuImageRenderer, _>(
        |painter| {
            painter.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                Color::WHITE,
                None,
                &Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
            );
        },
        width,
        height,
    );

    encode_png_rgba(&rgba, width, height)
}

pub fn encode_png_rgba(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut png_data = Vec::new();
    let mut encoder = png::Encoder::new(&mut png_data, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| format!("failed to encode screenshot header: {error}"))?;
    writer
        .write_image_data(rgba)
        .map_err(|error| format!("failed to encode screenshot pixels: {error}"))?;
    drop(writer);
    Ok(png_data)
}

/// Render a Vello scene to an RGBA8 pixel buffer on the CPU, at the given
/// physical pixel size. Used by the AppKit backend to rasterize the chrome
/// document into a layer bitmap.
pub fn render_scene_to_rgba(scene: anyrender::Scene, width: u32, height: u32) -> Vec<u8> {
    render_to_buffer::<VelloCpuImageRenderer, _>(
        |painter| {
            painter.append_scene(scene, Affine::IDENTITY);
        },
        width,
        height,
    )
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

/// Apple-standard text-editing keybindings (⌘←, ⌥⌫, ^A, …), shared by the
/// winit and AppKit backends for the chrome's text input.
pub fn apple_standard_keybinding_for_key_down(event: &BlitzKeyEvent) -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        use keyboard_types::{Key, Modifiers as KeyboardModifiers};

        if !event.state.is_pressed() {
            return None;
        }

        let command_mod = event.modifiers.contains(KeyboardModifiers::SUPER);
        let control_mod = event.modifiers.contains(KeyboardModifiers::CONTROL);
        let option_mod = event.modifiers.contains(KeyboardModifiers::ALT);
        let shift_mod = event.modifiers.contains(KeyboardModifiers::SHIFT);

        if command_mod {
            match &event.key {
                Key::Backspace => return Some("deleteToBeginningOfLine:"),
                Key::Delete => return Some("deleteToEndOfLine:"),
                Key::ArrowLeft if shift_mod => {
                    return Some("moveToBeginningOfLineAndModifySelection:");
                }
                Key::ArrowLeft => return Some("moveToBeginningOfLine:"),
                Key::ArrowRight if shift_mod => return Some("moveToEndOfLineAndModifySelection:"),
                Key::ArrowRight => return Some("moveToEndOfLine:"),
                Key::ArrowUp if shift_mod => {
                    return Some("moveToBeginningOfDocumentAndModifySelection:");
                }
                Key::ArrowUp => return Some("moveToBeginningOfDocument:"),
                Key::ArrowDown if shift_mod => {
                    return Some("moveToEndOfDocumentAndModifySelection:");
                }
                Key::ArrowDown => return Some("moveToEndOfDocument:"),
                _ => {}
            }
        }

        if option_mod {
            match &event.key {
                Key::Backspace => return Some("deleteWordBackward:"),
                Key::Delete => return Some("deleteWordForward:"),
                Key::ArrowLeft if shift_mod => return Some("moveWordLeftAndModifySelection:"),
                Key::ArrowLeft => return Some("moveWordLeft:"),
                Key::ArrowRight if shift_mod => return Some("moveWordRightAndModifySelection:"),
                Key::ArrowRight => return Some("moveWordRight:"),
                _ => {}
            }
        }

        if control_mod && let Key::Character(value) = &event.key {
            return match value.to_lowercase().as_str() {
                "a" if shift_mod => Some("moveToBeginningOfParagraphAndModifySelection:"),
                "a" => Some("moveToBeginningOfParagraph:"),
                "b" if shift_mod => Some("moveBackwardAndModifySelection:"),
                "b" => Some("moveBackward:"),
                "d" => Some("deleteForward:"),
                "e" if shift_mod => Some("moveToEndOfParagraphAndModifySelection:"),
                "e" => Some("moveToEndOfParagraph:"),
                "f" if shift_mod => Some("moveForwardAndModifySelection:"),
                "f" => Some("moveForward:"),
                "h" => Some("deleteBackward:"),
                "k" => Some("deleteToEndOfParagraph:"),
                "n" if shift_mod => Some("moveDownAndModifySelection:"),
                "n" => Some("moveDown:"),
                "o" => Some("insertNewlineIgnoringFieldEditor:"),
                "p" if shift_mod => Some("moveUpAndModifySelection:"),
                "p" => Some("moveUp:"),
                _ => None,
            };
        }

        match &event.key {
            Key::Backspace => Some("deleteBackward:"),
            _ => None,
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = event;
        None
    }
}
