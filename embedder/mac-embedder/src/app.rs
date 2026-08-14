//! The AppKit application: NSApplication lifecycle, windows, a native
//! AppKit chrome (tab strip and address field), event routing, display-link
//! pacing, and the automation host.
//!
//! The app runs an `NSApplication` with the web content in a layer-hosting
//! view whose layer `contents` is set to the shared IOSurface from the
//! graphics process (the zero-copy blit). The chrome is native AppKit
//! controls: a tab strip of `NSButton`s and an editable `NSTextField`
//! address bar. A `CVDisplayLink` paces animated content: each tick
//! requests the next frame via `WebviewProvider::frame_needed` at the
//! display refresh rate, and the link runs only while the composed scene is
//! animating.

use crate::input;
use crate::window::{new_layer_hosted_view, present_shared_surface, surface_to_rgba};
use automation::{
    AutomationController, AutomationHost, AutomationSnapshot, AutomationVisibleFrameViewport,
};
use blitz_traits::events::{
    BlitzPointerEvent, BlitzPointerId, BlitzWheelEvent, MouseEventButton, MouseEventButtons,
    UiEvent,
};
use blitz_traits::shell::ColorScheme;
use block2::RcBlock;
use embedder_core::{
    EventLoopEmbedder, FormalWebUserEvent, NavigationCompleted, NavigationCompletion,
    UserEventSink, automation_screenshot_png, encode_png_rgba, event_loop_options,
    normalize_browser_destination, read_clipboard_text, startup_destination_url,
    update_window_viewport_snapshot, write_clipboard_text,
};
use ipc_channel::platform::deallocate_mach_port;
use ipc_messages::content::WebviewId;
use ipc_messages::graphics::SurfaceFrame;
use keyboard_types::Modifiers as KeyboardModifiers;
use log::{debug, error, info};
use objc2::define_class;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSButton, NSButtonType, NSFont, NSTextField,
    NSTextFieldBezelStyle, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_app_kit::{NSBezelStyle, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{CVDisplayLink, CVOptionFlags, CVReturn, CVTimeStamp, kCVReturnSuccess};
use objc2_foundation::NSInteger;
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, ns_string,
};
use objc2_io_surface::IOSurfaceRef;
use serde_json::Value;
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;
use verification::TraceSender;
use webview::WebviewProvider;

const INITIAL_WINDOW_WIDTH: f64 = 1200.0;
const INITIAL_WINDOW_HEIGHT: f64 = 800.0;

/// The native chrome bar height in points: the tab strip on top, the
/// address field below.
const CHROME_BAR_HEIGHT: f64 = 76.0;
const ADDRESS_FIELD_HEIGHT: f64 = 34.0;
const TAB_STRIP_HEIGHT: f64 = 30.0;
const TAB_BUTTON_WIDTH: f64 = 140.0;
const TAB_BUTTON_HEIGHT: f64 = 26.0;
const NEW_TAB_BUTTON_WIDTH: f64 = 28.0;

// ── App delegate ───────────────────────────────────────────────────────────

/// The window delegate ivars: a pointer to the app state, confined to the
/// main thread.
#[repr(C)]
struct DelegateIvars {
    app: UnsafeCell<*mut MacApp>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the delegate is
    // retained by the app and released before the app state is dropped.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    #[name = "FormalWebAppDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: `NSWindowDelegate` has no safety requirements; the methods
    // only touch main-thread-confined app state.
    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, notification: &NSNotification) {
            let app = unsafe { &mut *(*self.ivars().app.get()) };
            app.window_will_close(notification);
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, notification: &NSNotification) {
            let app = unsafe { &mut *(*self.ivars().app.get()) };
            app.window_did_resize(notification);
        }

        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing_properties(&self, notification: &NSNotification) {
            let app = unsafe { &mut *(*self.ivars().app.get()) };
            app.window_did_change_backing_properties(notification);
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, notification: &NSNotification) {
            let app = unsafe { &mut *(*self.ivars().app.get()) };
            app.window_did_become_key(notification);
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn window_did_miniaturize(&self, _notification: &NSNotification) {
            let app = unsafe { &mut *(*self.ivars().app.get()) };
            app.stop_display_link();
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn window_did_deminiaturize(&self, _notification: &NSNotification) {
            let app = unsafe { &mut *(*self.ivars().app.get()) };
            app.start_display_link_if_animating();
        }
    }

    impl Delegate {
        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let app = unsafe { &mut *(*self.ivars().app.get()) };
            app.post_exit();
        }

        #[unsafe(method(switchTab:))]
        fn switch_tab(&self, sender: Option<&AnyObject>) {
            let app = unsafe { &mut *(*self.ivars().app.get()) };
            let Some(sender) = sender else { return };
            // SAFETY: the sender is one of the tab buttons, which respond
            // to `tag`.
            let tag: NSInteger = unsafe { msg_send![sender, tag] };
            app.action_switch_tab(tag.max(0) as usize);
        }

        #[unsafe(method(newTab:))]
        fn new_tab(&self, _sender: Option<&AnyObject>) {
            let app = unsafe { &mut *(*self.ivars().app.get()) };
            app.action_new_tab();
        }

        #[unsafe(method(navigate:))]
        fn navigate(&self, sender: Option<&AnyObject>) {
            let app = unsafe { &mut *(*self.ivars().app.get()) };
            let Some(sender) = sender else { return };
            // SAFETY: the sender is the address field, which responds to
            // `stringValue`.
            let value: Retained<NSString> = unsafe { msg_send![sender, stringValue] };
            app.action_navigate(value.to_string());
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            app: UnsafeCell::new(std::ptr::null_mut()),
        });
        // SAFETY: the superclass initializer has the correct signature.
        unsafe { msg_send![super(this), init] }
    }
}

// ── Main-thread handle ─────────────────────────────────────────────────────

/// A raw pointer to the app state that is only ever dereferenced on the
/// main thread. The explicit `Send`/`Sync` impls let the handle travel
/// through dispatch blocks that run on the main queue.
#[derive(Clone, Copy)]
struct MainThreadHandle {
    app: NonNull<MacApp>,
}

// SAFETY: the pointer is only ever dereferenced on the main thread.
unsafe impl Send for MainThreadHandle {}
unsafe impl Sync for MainThreadHandle {}

impl MainThreadHandle {
    fn new(app: *mut MacApp) -> Self {
        Self {
            app: NonNull::new(app).expect("app pointer must be non-null"),
        }
    }

    /// The app pointer. A method call (rather than field access) so that
    /// closures capture the whole `Send` handle, not the raw field.
    fn app_ptr(self) -> *mut MacApp {
        self.app.as_ptr()
    }
}

/// The user-event sink for the AppKit app: posts each event to the main
/// dispatch queue, where the app's run loop picks it up.
struct MacEventSink {
    handle: MainThreadHandle,
}

impl UserEventSink for MacEventSink {
    fn send(&self, event: FormalWebUserEvent) -> Result<(), String> {
        let handle = self.handle;
        dispatch::Queue::main().exec_async(move || {
            let app = unsafe { &mut *handle.app_ptr() };
            app.process_user_event(event);
        });
        Ok(())
    }
}

// ── Display link ───────────────────────────────────────────────────────────

/// The CVDisplayLink context, kept alive for the app's lifetime and passed
/// to the C callback as user data.
struct DisplayLinkContext {
    handle: MainThreadHandle,
}

unsafe extern "C-unwind" fn display_link_callback(
    _link: NonNull<CVDisplayLink>,
    _now: NonNull<CVTimeStamp>,
    _output: NonNull<CVTimeStamp>,
    _flags: CVOptionFlags,
    _flags_out: NonNull<CVOptionFlags>,
    context: *mut c_void,
) -> CVReturn {
    let context = unsafe { &*context.cast::<DisplayLinkContext>() };
    let handle = context.handle;
    dispatch::Queue::main().exec_async(move || {
        let app = unsafe { &mut *handle.app_ptr() };
        app.display_link_tick();
    });
    kCVReturnSuccess
}

// ── State ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct WindowId(Uuid);

impl WindowId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Per-tab state.
struct TabState {
    pending_url: Option<String>,
    committed_url: Option<String>,
}

impl TabState {
    fn new() -> Self {
        Self {
            pending_url: None,
            committed_url: None,
        }
    }

    fn display_url(&self) -> String {
        self.pending_url
            .clone()
            .or_else(|| self.committed_url.clone())
            .unwrap_or_default()
    }
}

/// The latest composited surface for one webview: the zero-copy shared
/// IOSurface (the only delivery path on macOS) plus the metadata needed to
/// present it and to pace the display link.
struct SurfaceState {
    surface: CFRetained<IOSurfaceRef>,
    width: u32,
    height: u32,
    /// The IOSurface's padded (64-multiple) width.
    padded_width: u32,
    animating: bool,
}

/// Per-window state: the NSWindow, the native chrome views, tabs, and
/// per-webview surfaces.
struct MacWindow {
    window: Retained<NSWindow>,
    chrome_bar: Retained<NSView>,
    tab_strip: Retained<NSView>,
    address_field: Retained<NSTextField>,
    tab_buttons: Vec<Retained<NSButton>>,
    new_tab_button: Retained<NSButton>,
    web_view: Retained<NSView>,
    /// The layer-hosting layer that presents the shared IOSurface; kept
    /// alive by the app.
    web_layer: Retained<objc2_quartz_core::CALayer>,
    tabs: HashMap<WebviewId, TabState>,
    tab_order: Vec<WebviewId>,
    active_tab: Option<WebviewId>,
    surfaces: HashMap<WebviewId, SurfaceState>,
    keyboard_modifiers: KeyboardModifiers,
    buttons: MouseEventButtons,
    /// Content view size in points (the window's content area).
    content_size: (f64, f64),
    scale: f64,
    minimized: bool,
}

struct MacApp {
    mtm: MainThreadMarker,
    ns_app: Retained<NSApplication>,
    delegate: Retained<Delegate>,
    /// Installed right after the app struct is built (self-referential).
    handle: Option<MainThreadHandle>,
    windows: HashMap<WindowId, MacWindow>,
    active_window_id: Option<WindowId>,
    provider: Option<WebviewProvider>,
    automation: AutomationController,
    display_link: Option<CFRetained<CVDisplayLink>>,
    display_link_context: Option<Box<DisplayLinkContext>>,
    display_link_running: bool,
    event_monitor: Option<Retained<AnyObject>>,
    exiting: bool,
}

impl MacApp {
    // ── Entry point ────────────────────────────────────────────────────────

    fn run(mtm: MainThreadMarker, trace_sender: Option<TraceSender>) -> Result<(), String> {
        let ns_app = NSApplication::sharedApplication(mtm);
        ns_app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

        let mut app = Self {
            mtm,
            ns_app: ns_app.clone(),
            delegate: Delegate::new(mtm),
            handle: None,
            windows: HashMap::new(),
            active_window_id: None,
            provider: None,
            automation: AutomationController::default(),
            display_link: None,
            display_link_context: None,
            display_link_running: false,
            event_monitor: None,
            exiting: false,
        };
        let app_ptr = &mut app as *mut MacApp;
        app.handle = Some(MainThreadHandle::new(app_ptr));
        unsafe {
            *app.delegate.ivars().app.get() = app_ptr;
        }

        // The sink must be installed before the user agent starts, so the
        // UA's initial events reach the app.
        let sink: Arc<dyn UserEventSink> = Arc::new(MacEventSink {
            handle: app.handle.expect("app handle must be installed"),
        });
        embedder_core::install_user_event_sink(sink.clone());
        let embedder = Arc::new(EventLoopEmbedder::new(sink));
        let provider = WebviewProvider::new(embedder, trace_sender)?;
        app.provider = Some(provider);

        app.install_event_monitor()?;
        app.install_main_menu();
        app.create_display_link()?;

        let title = event_loop_options()
            .window_title
            .unwrap_or_else(|| String::from("formal-web"));
        let window_id = app.create_window(&title)?;
        app.active_window_id = Some(window_id);

        // Activate the app (required when launching unbundled) and start
        // the run loop; the display link and the event sink drive the rest.
        #[allow(deprecated)]
        ns_app.activateIgnoringOtherApps(true);

        info!("[mac-embedder] starting NSApplication run loop");
        ns_app.run();
        info!("[mac-embedder] NSApplication run loop ended");

        embedder_core::clear_user_event_sink();
        update_window_viewport_snapshot(None);
        Ok(())
    }

    fn post_exit(&mut self) {
        if self.exiting {
            return;
        }
        self.exiting = true;
        info!("[mac-embedder] exiting");
        self.stop_display_link();
        if let Some(monitor) = self.event_monitor.take() {
            // SAFETY: the monitor object is the one returned by the
            // addLocalMonitor call.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        self.provider = None;
        update_window_viewport_snapshot(None);
        self.ns_app.terminate(None);
    }

    // ── Display link ───────────────────────────────────────────────────────

    fn create_display_link(&mut self) -> Result<(), String> {
        let mut link_ptr: *mut CVDisplayLink = std::ptr::null_mut();
        #[allow(deprecated)]
        let ret =
            unsafe { CVDisplayLink::create_with_active_cg_displays(NonNull::from(&mut link_ptr)) };
        if ret != kCVReturnSuccess || link_ptr.is_null() {
            return Err(format!("failed to create CVDisplayLink (CVReturn {ret})"));
        }
        // SAFETY: the display link was just created with a +1 retain.
        let link = unsafe { CFRetained::from_raw(NonNull::new(link_ptr).unwrap()) };
        let context = Box::new(DisplayLinkContext {
            handle: self.handle.expect("app handle must be installed"),
        });
        let context_ptr: *mut c_void = &*context as *const DisplayLinkContext as *mut c_void;
        #[allow(deprecated)]
        let ret = unsafe { link.set_output_callback(Some(display_link_callback), context_ptr) };
        if ret != kCVReturnSuccess {
            return Err(format!(
                "failed to set CVDisplayLink output callback (CVReturn {ret})"
            ));
        }
        self.display_link_context = Some(context);
        self.display_link = Some(link);
        Ok(())
    }

    fn start_display_link(&mut self) {
        if self.display_link_running {
            return;
        }
        let visible = self
            .windows
            .values()
            .any(|window_state| !window_state.minimized);
        if !visible {
            return;
        }
        if let Some(link) = &self.display_link {
            #[allow(deprecated)]
            let started = link.start() == kCVReturnSuccess;
            if started {
                self.display_link_running = true;
                info!("[mac-embedder] display link started");
            }
        }
    }

    fn start_display_link_if_animating(&mut self) {
        let animating = self.windows.values().any(|window_state| {
            window_state
                .active_tab
                .and_then(|webview_id| window_state.surfaces.get(&webview_id))
                .is_some_and(|surface| surface.animating)
        });
        if animating {
            self.start_display_link();
        }
    }

    fn stop_display_link(&mut self) {
        if !self.display_link_running {
            return;
        }
        if let Some(link) = &self.display_link {
            #[allow(deprecated)]
            link.stop();
        }
        self.display_link_running = false;
        info!("[mac-embedder] display link stopped");
    }

    fn display_link_tick(&mut self) {
        if self.exiting {
            return;
        }
        // Request the next frame at display cadence. The UA gates actual
        // rendering on its queued rendering opportunities, so an idle scene
        // is not re-rendered.
        if let Some(window_id) = self.active_window_id
            && let Some(webview_id) = self.windows.get(&window_id).and_then(|w| w.active_tab)
            && let Some(provider) = &self.provider
        {
            if let Err(error) = provider.frame_needed(webview_id) {
                error!("[mac-embedder] frame needed: {error}");
            }
        }
    }

    // ── Menu ──────────────────────────────────────────────────────────────

    fn install_main_menu(&mut self) {
        let mtm = self.mtm;
        let main_menu = objc2_app_kit::NSMenu::new(mtm);
        let app_menu_item = objc2_app_kit::NSMenuItem::new(mtm);
        main_menu.addItem(&app_menu_item);

        let app_menu = objc2_app_kit::NSMenu::new(mtm);
        let quit_item = objc2_app_kit::NSMenuItem::new(mtm);
        quit_item.setTitle(&NSString::from_str("Quit"));
        quit_item.setKeyEquivalent(ns_string!("q"));
        quit_item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
        // SAFETY: the selector is a valid action for the delegate.
        unsafe { quit_item.setAction(Some(sel!(quit:))) };
        // SAFETY: the delegate is a valid target object.
        let _: () = unsafe { msg_send![&quit_item, setTarget: &*self.delegate] };
        app_menu.addItem(&quit_item);
        app_menu_item.setSubmenu(Some(&app_menu));

        self.ns_app.setMainMenu(Some(&main_menu));
    }

    // ── Event monitor ──────────────────────────────────────────────────────

    fn install_event_monitor(&mut self) -> Result<(), String> {
        let handle = self.handle.expect("app handle must be installed");
        let block: RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> =
            RcBlock::new(move |event: NonNull<NSEvent>| {
                let app = unsafe { &mut *handle.app_ptr() };
                app.handle_ns_event(event)
            });
        // SAFETY: the block is valid and lives for as long as the monitor.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::Any, &*block)
        };
        let Some(monitor) = monitor else {
            return Err(String::from("failed to install the local event monitor"));
        };
        self.event_monitor = Some(monitor);
        Ok(())
    }

    /// Route a raw NSEvent. Keyboard events go to the app unless the native
    /// address field is editing; mouse and scroll events inside the web
    /// content area are routed to the content. Everything else is passed
    /// through to AppKit (the native chrome controls, the titlebar, other
    /// windows).
    fn handle_ns_event(&mut self, event: NonNull<NSEvent>) -> *mut NSEvent {
        let event_ref = unsafe { event.as_ref() };
        let event_type = event_ref.r#type();
        match event_type {
            NSEventType::KeyDown | NSEventType::KeyUp => {
                if self.address_field_is_first_responder() {
                    // The address field is editing: let AppKit deliver the
                    // event to the field (IME, shortcuts, etc.).
                    event.as_ptr()
                } else {
                    self.handle_content_keyboard_event(event_ref);
                    std::ptr::null_mut()
                }
            }
            NSEventType::FlagsChanged => event.as_ptr(),
            NSEventType::LeftMouseDown
            | NSEventType::LeftMouseUp
            | NSEventType::LeftMouseDragged
            | NSEventType::RightMouseDown
            | NSEventType::RightMouseUp
            | NSEventType::RightMouseDragged
            | NSEventType::OtherMouseDown
            | NSEventType::OtherMouseUp
            | NSEventType::OtherMouseDragged
            | NSEventType::MouseMoved
            | NSEventType::ScrollWheel => {
                let Some(window_id) = self.window_id_for_ns_event(event_ref) else {
                    return event.as_ptr();
                };
                let Some((x, y_from_top)) = self.content_point(window_id, event_ref) else {
                    return event.as_ptr();
                };
                if y_from_top < CHROME_BAR_HEIGHT {
                    // Inside the native chrome: let the controls handle it.
                    return event.as_ptr();
                }
                self.handle_content_mouse_event(window_id, event_ref, event_type, x, y_from_top);
                std::ptr::null_mut()
            }
            _ => event.as_ptr(),
        }
    }

    fn address_field_is_first_responder(&self) -> bool {
        self.active_window_id.is_some_and(|window_id| {
            self.windows.get(&window_id).is_some_and(|window_state| {
                window_state
                    .window
                    .firstResponder()
                    .is_some_and(|responder| {
                        // SAFETY: `isEqual` is identity comparison for
                        // Objective-C objects.
                        let same: bool =
                            unsafe { msg_send![&responder, isEqual: &*window_state.address_field] };
                        same
                    })
            })
        })
    }

    fn window_id_for_ns_event(&self, event: &NSEvent) -> Option<WindowId> {
        let event_window = event.window(self.mtm)?;
        self.windows.iter().find_map(|(window_id, window_state)| {
            if std::ptr::eq(&*event_window, &*window_state.window) {
                Some(*window_id)
            } else {
                None
            }
        })
    }

    /// The event location in the content view's top-left coordinate space
    /// (points). Returns `None` when the event is not inside the content
    /// view (e.g. the titlebar), which is left to AppKit.
    fn content_point(&self, window_id: WindowId, event: &NSEvent) -> Option<(f64, f64)> {
        let window_state = self.windows.get(&window_id)?;
        let (width, height) = window_state.content_size;
        let location = event.locationInWindow();
        let x = location.x;
        let y_from_top = height - location.y;
        if x < 0.0 || y_from_top < 0.0 || x > width || y_from_top > height {
            None
        } else {
            Some((x, y_from_top))
        }
    }

    fn handle_content_keyboard_event(&mut self, event: &NSEvent) {
        let Some(window_id) = self.active_window_id else {
            return;
        };

        // App-level shortcuts, handled before the content.
        if event.r#type() == NSEventType::KeyDown
            && event
                .modifierFlags()
                .contains(NSEventModifierFlags::Command)
        {
            match event.keyCode() {
                0x0C => {
                    // ⌘Q
                    self.post_exit();
                    return;
                }
                0x0D => {
                    // ⌘W: close the active window.
                    self.close_window(window_id);
                    return;
                }
                0x2D => {
                    // ⌘N: new window.
                    let _ = self.create_window("formal-web");
                    return;
                }
                _ => {}
            }
        }

        let key = input::ns_event_to_blitz_key(event);
        let ui_event = if event.r#type() == NSEventType::KeyDown {
            UiEvent::KeyDown(key)
        } else {
            UiEvent::KeyUp(key)
        };
        self.dispatch_to_content(window_id, ui_event);
    }

    fn handle_content_mouse_event(
        &mut self,
        window_id: WindowId,
        event: &NSEvent,
        event_type: NSEventType,
        x: f64,
        y_from_top: f64,
    ) {
        let Some(window_state) = self.windows.get_mut(&window_id) else {
            return;
        };
        window_state.keyboard_modifiers = input::modifiers_from_flags(event.modifierFlags());
        let modifiers = window_state.keyboard_modifiers;

        let is_down = matches!(
            event_type,
            NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown
        );
        let is_up = matches!(
            event_type,
            NSEventType::LeftMouseUp | NSEventType::RightMouseUp | NSEventType::OtherMouseUp
        );
        if is_down || is_up {
            let button = input::button_from_number(event.buttonNumber());
            if is_down {
                window_state.buttons |= button.into();
            } else {
                window_state.buttons.remove(button.into());
            }
        }
        let buttons = window_state.buttons;

        if is_down {
            // Clicking the content hands text-input focus away from the
            // native address field, so the next keystrokes go to the page.
            window_state.window.makeFirstResponder(None);
        }
        let coords = input::content_coords(x, y_from_top, CHROME_BAR_HEIGHT);

        let event_kind = if is_down {
            UiEventKind::PointerDown
        } else if is_up {
            UiEventKind::PointerUp
        } else if event_type == NSEventType::ScrollWheel {
            UiEventKind::Wheel
        } else {
            UiEventKind::PointerMove
        };

        let ui_event = |kind: UiEventKind, pointer: BlitzPointerEvent| match kind {
            UiEventKind::PointerDown => UiEvent::PointerDown(pointer),
            UiEventKind::PointerUp => UiEvent::PointerUp(pointer),
            UiEventKind::PointerMove => UiEvent::PointerMove(pointer),
            UiEventKind::Wheel => UiEvent::Wheel(BlitzWheelEvent {
                delta: input::wheel_delta_from_event(event),
                coords: pointer.coords,
                buttons: pointer.buttons,
                mods: pointer.mods,
            }),
        };

        let pointer = input::pointer_event(
            BlitzPointerId::Mouse,
            true,
            coords,
            input::button_from_number(event.buttonNumber()),
            buttons,
            modifiers,
        );
        self.dispatch_to_content(window_id, ui_event(event_kind, pointer));
    }

    // ── Native chrome ──────────────────────────────────────────────────────

    fn make_tab_button(
        mtm: MainThreadMarker,
        delegate: &Delegate,
        label: &str,
        index: usize,
        active: bool,
    ) -> Retained<NSButton> {
        let button = NSButton::new(mtm);
        button.setTitle(&NSString::from_str(label));
        button.setButtonType(NSButtonType::MomentaryPushIn);
        button.setBezelStyle(NSBezelStyle::Toolbar);
        button.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        button.setTag(index as NSInteger);
        if active {
            button.setState(1);
        }
        // SAFETY: the delegate is a valid target and the selector matches
        // its `switchTab:` action.
        let _: () = unsafe { msg_send![&button, setTarget: delegate] };
        unsafe { button.setAction(Some(sel!(switchTab:))) };
        button
    }

    fn make_new_tab_button(mtm: MainThreadMarker, delegate: &Delegate) -> Retained<NSButton> {
        let button = NSButton::new(mtm);
        button.setTitle(ns_string!("+"));
        button.setButtonType(NSButtonType::MomentaryPushIn);
        button.setBezelStyle(NSBezelStyle::Toolbar);
        button.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
        // SAFETY: the delegate is a valid target and the selector matches
        // its `newTab:` action.
        let _: () = unsafe { msg_send![&button, setTarget: delegate] };
        unsafe { button.setAction(Some(sel!(newTab:))) };
        button
    }

    fn make_address_field(mtm: MainThreadMarker, delegate: &Delegate) -> Retained<NSTextField> {
        let field = NSTextField::new(mtm);
        field.setEditable(true);
        field.setSelectable(true);
        field.setUsesSingleLineMode(true);
        field.setDrawsBackground(true);
        field.setBezelStyle(NSTextFieldBezelStyle::RoundedBezel);
        field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        field.setPlaceholderString(Some(&NSString::from_str("Search or enter address")));
        // No focus ring: the field's rounded bezel already indicates focus,
        // and the animated blue ring is distracting in a browser chrome.
        field.setFocusRingType(objc2_app_kit::NSFocusRingType::None);
        // SAFETY: the delegate is a valid target and the selector matches
        // its `navigate:` action; the action fires on Return.
        let _: () = unsafe { msg_send![&field, setTarget: delegate] };
        unsafe { field.setAction(Some(sel!(navigate:))) };
        field
    }

    fn refresh_tab_strip(&mut self, window_id: WindowId) {
        let delegate = self.delegate.clone();
        let mtm = self.mtm;
        let Some(window_state) = self.windows.get_mut(&window_id) else {
            return;
        };
        for button in window_state.tab_buttons.drain(..) {
            button.removeFromSuperview();
        }
        for (index, webview_id) in window_state.tab_order.iter().enumerate() {
            let label = Self::tab_label(window_state, webview_id);
            let active = window_state.active_tab == Some(*webview_id);
            let button = Self::make_tab_button(mtm, &delegate, &label, index, active);
            window_state.tab_buttons.push(button.clone());
            window_state.tab_strip.addSubview(&button);
        }
        Self::layout_tab_strip(window_state);
    }

    fn refresh_address_field(&mut self, window_id: WindowId) {
        let Some(window_state) = self.windows.get_mut(&window_id) else {
            return;
        };
        let address = window_state
            .active_tab
            .and_then(|webview_id| window_state.tabs.get(&webview_id))
            .map(TabState::display_url)
            .unwrap_or_default();
        // Don't clobber the field while the user is typing into it.
        let is_editing = window_state
            .window
            .firstResponder()
            .is_some_and(|responder| {
                // SAFETY: `isEqual` is identity comparison for Objective-C
                // objects.
                let same: bool =
                    unsafe { msg_send![&responder, isEqual: &*window_state.address_field] };
                same
            });
        if !is_editing {
            window_state
                .address_field
                .setStringValue(&NSString::from_str(&address));
        }
    }

    fn refresh_chrome(&mut self, window_id: WindowId) {
        self.refresh_tab_strip(window_id);
        self.refresh_address_field(window_id);
    }

    fn tab_label(window_state: &MacWindow, webview_id: &WebviewId) -> String {
        if let Some(tab) = window_state.tabs.get(webview_id) {
            if let Some(url) = &tab.committed_url
                && !url.is_empty()
            {
                return Self::truncate_url(url);
            }
            if let Some(url) = &tab.pending_url
                && !url.is_empty()
            {
                return Self::truncate_url(url);
            }
        }
        String::from("New Tab")
    }

    fn truncate_url(url: &str) -> String {
        let display = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .or_else(|| url.strip_prefix("file://"))
            .unwrap_or(url);
        if display.len() > 24 {
            format!("{}…", &display[..21])
        } else {
            display.to_owned()
        }
    }

    /// Lay out the native chrome: the tab strip on top, the address field
    /// below, the web content filling the rest.
    fn layout_window_views(window_state: &mut MacWindow) {
        let (width, height) = window_state.content_size;
        let web_height = (height - CHROME_BAR_HEIGHT).max(0.0);
        // The content view is not flipped: frames use a bottom-left origin,
        // so the web content sits at the bottom and the chrome on top.
        window_state.web_view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width, web_height),
        ));
        // Keep the layer-hosting layer's frame in sync with the view.
        window_state.web_layer.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width, web_height),
        ));
        window_state.chrome_bar.setFrame(NSRect::new(
            NSPoint::new(0.0, web_height),
            NSSize::new(width, CHROME_BAR_HEIGHT),
        ));
        window_state.address_field.setFrame(NSRect::new(
            NSPoint::new(8.0, 8.0),
            NSSize::new((width - 16.0).max(0.0), ADDRESS_FIELD_HEIGHT),
        ));
        window_state.tab_strip.setFrame(NSRect::new(
            NSPoint::new(0.0, 8.0 + ADDRESS_FIELD_HEIGHT + 6.0),
            NSSize::new(width, TAB_STRIP_HEIGHT),
        ));
        Self::layout_tab_strip(window_state);
    }

    fn layout_tab_strip(window_state: &mut MacWindow) {
        let strip_width = window_state.tab_strip.frame().size.width;
        for (index, button) in window_state.tab_buttons.iter().enumerate() {
            let x = 8.0 + (index as f64) * (TAB_BUTTON_WIDTH + 6.0);
            button.setFrame(NSRect::new(
                NSPoint::new(x, 2.0),
                NSSize::new(TAB_BUTTON_WIDTH, TAB_BUTTON_HEIGHT),
            ));
        }
        let new_tab_x =
            8.0 + (window_state.tab_buttons.len() as f64) * (TAB_BUTTON_WIDTH + 6.0) + 2.0;
        let new_tab_x = if new_tab_x + NEW_TAB_BUTTON_WIDTH > strip_width {
            (strip_width - NEW_TAB_BUTTON_WIDTH - 8.0).max(8.0)
        } else {
            new_tab_x
        };
        window_state.new_tab_button.setFrame(NSRect::new(
            NSPoint::new(new_tab_x, 2.0),
            NSSize::new(NEW_TAB_BUTTON_WIDTH, TAB_BUTTON_HEIGHT),
        ));
    }

    // ── Chrome actions ─────────────────────────────────────────────────────

    fn action_switch_tab(&mut self, index: usize) {
        let Some(window_id) = self.active_window_id else {
            return;
        };
        let webview_id = self
            .windows
            .get(&window_id)
            .and_then(|window_state| window_state.tab_order.get(index).copied());
        let Some(webview_id) = webview_id else {
            return;
        };
        if let Some(window_state) = self.windows.get_mut(&window_id) {
            window_state.active_tab = Some(webview_id);
            Self::present_active_surface(window_state);
        }
        self.refresh_chrome(window_id);
        self.update_provider_viewport(window_id);
        if let Some(provider) = self.provider.as_ref() {
            let _ = provider.frame_needed(webview_id);
        }
    }

    fn action_new_tab(&mut self) {
        if let Some(provider) = self.provider.as_ref() {
            let _ = provider.navigate(None, "about:blank");
        }
    }

    fn action_navigate(&mut self, input: String) {
        let Some(url) = normalize_browser_destination(&input) else {
            return;
        };
        let Some(window_id) = self.active_window_id else {
            return;
        };
        let Some(webview_id) = self
            .windows
            .get(&window_id)
            .and_then(|window_state| window_state.active_tab)
        else {
            return;
        };
        if let Some(provider) = self.provider.as_ref() {
            let _ = provider.navigate(Some(webview_id), &url);
        }
        if let Some(window_state) = self.windows.get_mut(&window_id)
            && let Some(tab) = window_state.tabs.get_mut(&webview_id)
        {
            tab.pending_url = Some(url.clone());
        }
        self.refresh_address_field(window_id);
    }

    fn add_tab(&mut self, window_id: WindowId, webview_id: WebviewId) {
        let Some(window_state) = self.windows.get_mut(&window_id) else {
            return;
        };
        if window_state.tabs.contains_key(&webview_id) {
            window_state.active_tab = Some(webview_id);
            return;
        }
        window_state.tabs.insert(webview_id, TabState::new());
        window_state.tab_order.push(webview_id);
        window_state.active_tab = Some(webview_id);
    }

    /// Present the active tab's stored surface (the latest composited
    /// frame) on the web content layer.
    fn present_active_surface(window_state: &mut MacWindow) {
        let Some(webview_id) = window_state.active_tab else {
            return;
        };
        let Some(surface) = window_state.surfaces.get(&webview_id) else {
            return;
        };
        present_shared_surface(
            &window_state.web_layer,
            &surface.surface,
            surface.width,
            surface.padded_width,
            window_state.scale,
        );
    }

    // ── Windows ────────────────────────────────────────────────────────────

    fn create_window(&mut self, title: &str) -> Result<WindowId, String> {
        let mtm = self.mtm;
        // SAFETY: the window is created with the standard init; it is
        // retained by the app and released when closed (see below).
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(INITIAL_WINDOW_WIDTH, INITIAL_WINDOW_HEIGHT),
                ),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                objc2_app_kit::NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: the window is retained by the app; AppKit must not
        // release it when the window closes.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(title));
        window.setBackgroundColor(Some(&objc2_app_kit::NSColor::windowBackgroundColor()));
        window.setDelegate(Some(ProtocolObject::from_ref(&*self.delegate)));
        window.setAcceptsMouseMovedEvents(true);

        let content_view = NSView::new(mtm);
        content_view.setWantsLayer(true);

        let chrome_bar = NSView::new(mtm);
        chrome_bar.setWantsLayer(true);
        let tab_strip = NSView::new(mtm);
        tab_strip.setWantsLayer(true);
        let new_tab_button = Self::make_new_tab_button(mtm, &self.delegate);
        tab_strip.addSubview(&new_tab_button);
        let address_field = Self::make_address_field(mtm, &self.delegate);
        chrome_bar.addSubview(&tab_strip);
        chrome_bar.addSubview(&address_field);

        let window_id = WindowId::new();
        let scale = window.backingScaleFactor();
        let (web_view, web_layer) = new_layer_hosted_view(mtm, scale);
        let content_size = (
            window
                .contentView()
                .map(|view| view.frame().size.width)
                .unwrap_or(INITIAL_WINDOW_WIDTH),
            window
                .contentView()
                .map(|view| view.frame().size.height)
                .unwrap_or(INITIAL_WINDOW_HEIGHT),
        );

        let mut window_state = MacWindow {
            window: window.clone(),
            chrome_bar,
            tab_strip,
            address_field,
            tab_buttons: Vec::new(),
            new_tab_button,
            web_view,
            web_layer,
            tabs: HashMap::new(),
            tab_order: Vec::new(),
            active_tab: None,
            surfaces: HashMap::new(),
            keyboard_modifiers: KeyboardModifiers::default(),
            buttons: MouseEventButtons::None,
            content_size,
            scale,
            minimized: false,
        };
        Self::layout_window_views(&mut window_state);

        content_view.addSubview(&window_state.chrome_bar);
        content_view.addSubview(&window_state.web_view);
        window.setContentView(Some(&content_view));

        update_window_viewport_snapshot(Some(Self::viewport_tuple(content_size, scale)));
        if let Some(provider) = self.provider.as_mut() {
            let _ = provider.set_default_viewport(Some(Self::viewport_tuple(content_size, scale)));
        }

        window.center();
        window.makeKeyAndOrderFront(None);

        let destination = startup_destination_url(event_loop_options().startup_url.as_deref())
            .unwrap_or_else(|_| String::from("about:blank"));
        if let Some(provider) = self.provider.as_ref() {
            let _ = provider.navigate(None, &destination);
        }

        self.windows.insert(window_id, window_state);
        Ok(window_id)
    }

    fn viewport_tuple(content_size: (f64, f64), scale: f64) -> (u32, u32, f32, ColorScheme) {
        let (width, height) = content_size;
        (
            (width * scale) as u32,
            (height * scale) as u32,
            scale as f32,
            ColorScheme::Light,
        )
    }

    fn window_for_webview(&self, webview_id: WebviewId) -> Option<WindowId> {
        self.windows.iter().find_map(|(window_id, window_state)| {
            if window_state.tabs.contains_key(&webview_id) {
                Some(*window_id)
            } else {
                None
            }
        })
    }

    fn close_window(&mut self, window_id: WindowId) {
        if let Some(window_state) = self.windows.get_mut(&window_id) {
            window_state.window.close();
        }
    }

    fn window_will_close(&mut self, notification: &NSNotification) {
        let Some(window_id) = self.window_id_for_notification(notification) else {
            return;
        };
        info!("[mac-embedder] window closed window={window_id:?}");
        if let Some(window_state) = self.windows.get_mut(&window_id) {
            window_state.tabs.clear();
            window_state.tab_order.clear();
            window_state.active_tab = None;
            window_state.surfaces.clear();
            window_state.tab_buttons.clear();
        }
        self.windows.remove(&window_id);
        if self.active_window_id == Some(window_id) {
            self.active_window_id = self.windows.keys().next().copied();
        }
        if self.windows.is_empty() {
            self.post_exit();
        }
    }

    fn window_did_resize(&mut self, notification: &NSNotification) {
        let Some(window_id) = self.window_id_for_notification(notification) else {
            return;
        };
        let Some(window_state) = self.windows.get_mut(&window_id) else {
            return;
        };
        let size = window_state
            .window
            .contentView()
            .map(|view| view.frame().size)
            .unwrap_or_default();
        window_state.content_size = (size.width, size.height);
        window_state.scale = window_state.window.backingScaleFactor();
        Self::layout_window_views(window_state);
        let viewport = Self::viewport_tuple(window_state.content_size, window_state.scale);
        let webview_id = window_state.active_tab;

        update_window_viewport_snapshot(Some(viewport));
        if let Some(provider) = self.provider.as_mut() {
            let _ = provider.set_default_viewport(Some(viewport));
            if let Some(webview_id) = webview_id {
                let _ = provider.set_traversable_viewport(webview_id, viewport, 0.0, 0.0);
                let _ = provider.frame_needed(webview_id);
            }
        }
    }

    fn window_did_change_backing_properties(&mut self, notification: &NSNotification) {
        let Some(window_id) = self.window_id_for_notification(notification) else {
            return;
        };
        let Some(window_state) = self.windows.get_mut(&window_id) else {
            return;
        };
        window_state.scale = window_state.window.backingScaleFactor();
        let viewport = Self::viewport_tuple(window_state.content_size, window_state.scale);
        let webview_id = window_state.active_tab;

        update_window_viewport_snapshot(Some(viewport));
        if let Some(provider) = self.provider.as_mut()
            && let Some(webview_id) = webview_id
        {
            let _ = provider.set_traversable_viewport(webview_id, viewport, 0.0, 0.0);
            let _ = provider.frame_needed(webview_id);
        }
    }

    fn window_did_become_key(&mut self, notification: &NSNotification) {
        if let Some(window_id) = self.window_id_for_notification(notification) {
            self.active_window_id = Some(window_id);
        }
    }

    fn window_id_for_notification(&self, notification: &NSNotification) -> Option<WindowId> {
        let window = notification.object()?.downcast::<NSWindow>().ok()?;
        self.windows.iter().find_map(|(window_id, window_state)| {
            if std::ptr::eq(&*window, &*window_state.window) {
                Some(*window_id)
            } else {
                None
            }
        })
    }

    // ── Provider plumbing ──────────────────────────────────────────────────

    fn update_provider_viewport(&mut self, window_id: WindowId) {
        let Some(window_state) = self.windows.get(&window_id) else {
            return;
        };
        let viewport = Self::viewport_tuple(window_state.content_size, window_state.scale);
        let tab_webview_ids: Vec<WebviewId> = window_state.tab_order.clone();

        update_window_viewport_snapshot(Some(viewport));
        if let Some(provider) = self.provider.as_mut() {
            let _ = provider.set_default_viewport(Some(viewport));
            for tab_webview_id in tab_webview_ids {
                if let Err(error) =
                    provider.set_traversable_viewport(tab_webview_id, viewport, 0.0, 0.0)
                {
                    error!("[mac-embedder] set traversable viewport: {error}");
                }
            }
        }
    }

    fn dispatch_to_content(&mut self, window_id: WindowId, event: UiEvent) {
        let webview_id = self
            .windows
            .get(&window_id)
            .and_then(|window_state| window_state.active_tab);
        let Some(webview_id) = webview_id else {
            return;
        };
        if let Some(provider) = &self.provider {
            if let Err(error) = provider.send_ui_event(webview_id, event) {
                error!("content event error: {error}");
            }
        }
    }

    fn frame_needed(&self, webview_id: WebviewId) {
        if let Some(provider) = &self.provider
            && let Err(error) = provider.frame_needed(webview_id)
        {
            error!("[mac-embedder] frame needed: {error}");
        }
    }

    // ── User events ────────────────────────────────────────────────────────

    fn process_user_event(&mut self, event: FormalWebUserEvent) {
        if self.exiting {
            return;
        }
        match event {
            FormalWebUserEvent::RequestRedraw(webview_id) => {
                self.frame_needed(webview_id);
            }
            FormalWebUserEvent::NavigationRequested {
                webview_id,
                destination_url,
            } => {
                if let Some(window_id) = self.window_for_webview(webview_id) {
                    if let Some(window_state) = self.windows.get_mut(&window_id)
                        && let Some(tab) = window_state.tabs.get_mut(&webview_id)
                    {
                        tab.pending_url = Some(destination_url.clone());
                    }
                    if self
                        .windows
                        .get(&window_id)
                        .is_some_and(|w| w.active_tab == Some(webview_id))
                    {
                        self.refresh_chrome(window_id);
                        self.update_provider_viewport(window_id);
                    }
                } else if let Some(active_window) = self.active_window_id {
                    self.add_tab(active_window, webview_id);
                    self.refresh_chrome(active_window);
                    self.update_provider_viewport(active_window);
                }
            }
            FormalWebUserEvent::NavigationCompleted(completion) => {
                self.handle_navigation_completed(completion);
            }
            FormalWebUserEvent::NewWebview(webview_id, _) => {
                debug!("[mac-embedder] NewWebview webview={webview_id:?}");
                if let Some(active_window) = self.active_window_id {
                    self.add_tab(active_window, webview_id);
                    self.refresh_chrome(active_window);
                    self.update_provider_viewport(active_window);
                }
            }
            FormalWebUserEvent::CreateWindow => {
                let _ = self.create_window("formal-web");
            }
            FormalWebUserEvent::Automation(command) => {
                let mut automation = std::mem::take(&mut self.automation);
                automation.handle_command(self, command);
                self.automation = automation;
            }
            FormalWebUserEvent::ClipboardRead { reply } => {
                let _ = reply.send(read_clipboard_text());
            }
            FormalWebUserEvent::ClipboardWrite { text, reply } => {
                let _ = reply.send(write_clipboard_text(text));
            }
            FormalWebUserEvent::NewWebContentScene { webview_id, .. } => {
                // The IOSurface surface path replaces the scene-bytes path.
                debug!(
                    "[mac-embedder] NewWebContentScene ignored (surface path) webview={webview_id:?}"
                );
            }
            FormalWebUserEvent::NewWebContentSurface {
                webview_id,
                frame,
                width,
                height,
                animating,
                ..
            } => {
                self.handle_new_surface(webview_id, frame, width, height, animating);
            }
            FormalWebUserEvent::Exit => self.post_exit(),
        }
    }

    fn handle_navigation_completed(&mut self, completion: NavigationCompleted) {
        let Some(window_id) = self.window_for_webview(completion.webview_id) else {
            // Child traversables (iframes) fire their own completions and
            // don't create tabs.
            return;
        };
        let is_current = self
            .windows
            .get(&window_id)
            .is_some_and(|w| w.active_tab == Some(completion.webview_id));
        match &completion.status {
            NavigationCompletion::Committed { url } => {
                if let Some(window_state) = self.windows.get_mut(&window_id)
                    && let Some(tab) = window_state.tabs.get_mut(&completion.webview_id)
                {
                    tab.pending_url = None;
                    tab.committed_url = Some(url.clone());
                }
                if let Some(provider) = self.provider.as_mut() {
                    provider.on_navigation_committed(completion.webview_id);
                }
                if is_current {
                    self.refresh_chrome(window_id);
                    self.update_provider_viewport(window_id);
                    self.frame_needed(completion.webview_id);
                }
            }
            NavigationCompletion::Aborted { message } => {
                if is_current {
                    let mut automation = std::mem::take(&mut self.automation);
                    automation.abort_pending_navigation(message.clone());
                    self.automation = automation;
                    if let Some(window_state) = self.windows.get_mut(&window_id)
                        && let Some(tab) = window_state.tabs.get_mut(&completion.webview_id)
                    {
                        tab.pending_url = None;
                    }
                    self.refresh_chrome(window_id);
                }
            }
        }
    }

    fn handle_new_surface(
        &mut self,
        webview_id: WebviewId,
        frame: SurfaceFrame,
        width: u32,
        height: u32,
        animating: bool,
    ) {
        let Some(window_id) = self.window_for_webview(webview_id) else {
            info!("[mac-embedder] no window for webview={webview_id:?}");
            return;
        };
        let Some(window_state) = self.windows.get_mut(&window_id) else {
            return;
        };
        let is_active = window_state.active_tab == Some(webview_id);
        // The zero-copy delivery path: the frame was rendered directly into
        // a shared IOSurface by the graphics process; the embedder imports
        // the surface and hands it to CoreAnimation.
        let SurfaceFrame::SharedTexture {
            surface_id, port, ..
        } = frame
        else {
            error!(
                "[mac-embedder] received a CPU surface for webview={webview_id:?}; the AppKit embedder only supports the zero-copy shared-surface path"
            );
            return;
        };
        // Look the surface up by its global ID first: a surface object
        // imported from its Mach port (`IOSurfaceLookupFromMachPort`)
        // cannot be composited by CoreAnimation, while a by-ID lookup
        // (`IOSurfaceLookup`) of the same surface composites correctly.
        // Fall back to the port when the ID is not resolvable (e.g. a
        // producer that did not mark the surface global).
        let surface = IOSurfaceRef::lookup(surface_id).or_else(|| {
            let port_name = port.into_name();
            let surface = IOSurfaceRef::lookup_from_mach_port(port_name);
            deallocate_mach_port(port_name);
            surface
        });
        let Some(surface) = surface else {
            error!(
                "[mac-embedder] IOSurfaceLookup failed for webview={webview_id:?} id={surface_id}"
            );
            return;
        };
        let surface_state = SurfaceState {
            surface,
            width,
            height,
            padded_width: padded_surface_width(width),
            animating,
        };
        if is_active {
            present_shared_surface(
                &window_state.web_layer,
                &surface_state.surface,
                surface_state.width,
                surface_state.padded_width,
                window_state.scale,
            );
        }
        window_state.surfaces.insert(webview_id, surface_state);
        info!(
            "[mac-embedder] surface webview={webview_id:?} {}x{} animating={animating} active={is_active}",
            width, height
        );

        // Pacing: animated content runs the display link; a static scene
        // just needs the next frame on demand.
        if animating {
            self.start_display_link();
        } else {
            self.stop_display_link();
        }
        self.frame_needed(webview_id);
    }
}

/// Round a surface width up to a multiple of 64, the Metal constraint for
/// IOSurface-backed textures. Must match the producer's padding.
fn padded_surface_width(width: u32) -> u32 {
    (width.max(1) + 63) & !63
}

/// The kind of pointer event being dispatched.
#[derive(Clone, Copy)]
enum UiEventKind {
    PointerDown,
    PointerUp,
    PointerMove,
    Wheel,
}

// ── AutomationHost ─────────────────────────────────────────────────────────

impl AutomationHost for MacApp {
    fn automation_snapshot(&mut self) -> AutomationSnapshot {
        let window_id = self.active_window_id;
        let (active_tab, committed_url, displayed_url) = if let Some(window_id) = window_id
            && let Some(window_state) = self.windows.get(&window_id)
        {
            (
                window_state.active_tab,
                window_state
                    .active_tab
                    .and_then(|webview_id| window_state.tabs.get(&webview_id))
                    .and_then(|tab| tab.committed_url.clone()),
                window_state
                    .active_tab
                    .and_then(|webview_id| window_state.tabs.get(&webview_id))
                    .map(TabState::display_url)
                    .unwrap_or_default(),
            )
        } else {
            (None, None, String::new())
        };
        AutomationSnapshot {
            webview_id: active_tab,
            current_url: committed_url,
            displayed_url,
            navigable_id: None,
            has_top_level_traversable: active_tab.is_some(),
        }
    }

    fn automation_visible_frame_viewports(
        &mut self,
    ) -> Result<Vec<AutomationVisibleFrameViewport>, String> {
        Ok(Vec::new())
    }

    fn automation_screenshot(&mut self) -> Result<Vec<u8>, String> {
        // Real screenshot: read the active tab's shared IOSurface back.
        let window_id = self.active_window_id;
        let (webview_id, surface) = if let Some(window_id) = window_id
            && let Some(window_state) = self.windows.get(&window_id)
            && let Some(webview_id) = window_state.active_tab
            && let Some(surface) = window_state.surfaces.get(&webview_id)
        {
            (Some(webview_id), Some(&surface.surface))
        } else {
            (None, None)
        };
        if let (Some(webview_id), Some(surface)) = (webview_id, surface) {
            let window_id = self
                .active_window_id
                .ok_or_else(|| String::from("no window state"))?;
            let Some(surface_state) = self
                .windows
                .get(&window_id)
                .and_then(|w| w.surfaces.get(&webview_id))
            else {
                return Err(String::from("no surface state"));
            };
            let rgba = surface_to_rgba(surface, surface_state.width, surface_state.height)?;
            return encode_png_rgba(&rgba, surface_state.width, surface_state.height);
        }
        automation_screenshot_png(&mut self.provider, webview_id)
    }

    fn begin_automation_navigation(&mut self, url: String) -> Result<(), String> {
        let window_id = self
            .active_window_id
            .ok_or_else(|| String::from("no window"))?;
        let webview_id = self
            .windows
            .get(&window_id)
            .and_then(|w| w.active_tab)
            .ok_or_else(|| String::from("no active tab"))?;
        let provider = self
            .provider
            .as_ref()
            .ok_or_else(|| String::from("no provider"))?;
        provider.navigate(Some(webview_id), &url)?;
        if let Some(window_state) = self.windows.get_mut(&window_id)
            && let Some(tab) = window_state.tabs.get_mut(&webview_id)
        {
            tab.pending_url = Some(url);
        }
        Ok(())
    }

    fn automation_click(&mut self, x: f32, y: f32) -> Result<(), String> {
        let window_id = self
            .active_window_id
            .ok_or_else(|| String::from("no window"))?;
        let (webview_id, buttons, modifiers) = {
            let window_state = self
                .windows
                .get_mut(&window_id)
                .ok_or_else(|| String::from("no window state"))?;
            let webview_id = window_state
                .active_tab
                .ok_or_else(|| String::from("no active tab"))?;
            let buttons = window_state.buttons;
            let modifiers = window_state.keyboard_modifiers;
            (webview_id, buttons, modifiers)
        };
        let provider = self
            .provider
            .as_mut()
            .ok_or_else(|| String::from("no provider"))?;
        let coords = input::content_coords(
            f64::from(x),
            f64::from(y) + CHROME_BAR_HEIGHT,
            CHROME_BAR_HEIGHT,
        );
        let send_event = |provider: &mut WebviewProvider, webview_id: WebviewId, ui_event| {
            provider.send_ui_event(webview_id, ui_event).ok();
        };
        let make_pointer = |button: MouseEventButton, buttons: MouseEventButtons| {
            input::pointer_event(
                BlitzPointerId::Mouse,
                true,
                coords,
                button,
                buttons,
                modifiers,
            )
        };
        send_event(
            provider,
            webview_id,
            UiEvent::PointerMove(make_pointer(Default::default(), buttons)),
        );
        send_event(
            provider,
            webview_id,
            UiEvent::PointerDown(make_pointer(MouseEventButton::Main, buttons)),
        );
        send_event(
            provider,
            webview_id,
            UiEvent::PointerUp(make_pointer(MouseEventButton::Main, buttons)),
        );
        Ok(())
    }

    fn automation_click_element(&mut self, selector: String) -> Result<(), String> {
        let webview_id = self
            .active_window_id
            .and_then(|window_id| self.windows.get(&window_id))
            .and_then(|w| w.active_tab)
            .ok_or_else(|| String::from("no tab"))?;
        self.provider
            .as_ref()
            .ok_or_else(|| String::from("no provider"))?
            .click_element(webview_id, selector)
    }

    fn automation_scroll(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> Result<(), String> {
        let window_id = self
            .active_window_id
            .ok_or_else(|| String::from("no window"))?;
        let (webview_id, buttons, modifiers) = {
            let window_state = self
                .windows
                .get_mut(&window_id)
                .ok_or_else(|| String::from("no window state"))?;
            let webview_id = window_state
                .active_tab
                .ok_or_else(|| String::from("no active tab"))?;
            let buttons = window_state.buttons;
            let modifiers = window_state.keyboard_modifiers;
            (webview_id, buttons, modifiers)
        };
        let provider = self
            .provider
            .as_mut()
            .ok_or_else(|| String::from("no provider"))?;
        let coords = input::content_coords(
            f64::from(x),
            f64::from(y) + CHROME_BAR_HEIGHT,
            CHROME_BAR_HEIGHT,
        );
        let pointer = input::pointer_event(
            BlitzPointerId::Mouse,
            true,
            coords,
            Default::default(),
            buttons,
            modifiers,
        );
        provider
            .send_ui_event(webview_id, UiEvent::PointerMove(pointer))
            .ok();
        provider
            .send_ui_event(
                webview_id,
                UiEvent::Wheel(BlitzWheelEvent {
                    delta: blitz_traits::events::BlitzWheelDelta::Pixels(
                        f64::from(dx),
                        f64::from(dy),
                    ),
                    coords,
                    buttons,
                    mods: modifiers,
                }),
            )
            .map_err(|error| format!("wheel event error: {error}"))
    }

    fn automation_evaluate_script(
        &mut self,
        source: String,
        timeout: Duration,
    ) -> Result<Value, String> {
        let webview_id = self
            .active_window_id
            .and_then(|window_id| self.windows.get(&window_id))
            .and_then(|w| w.active_tab)
            .ok_or_else(|| String::from("no tab"))?;
        self.provider
            .as_ref()
            .ok_or_else(|| String::from("no provider"))?
            .evaluate_script(webview_id, source, timeout)
    }
}

/// Entry point: run the AppKit windowed app until it exits.
pub fn run_windowed_app(trace_sender: Option<TraceSender>) -> Result<(), String> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| String::from("the macOS embedder must run on the main thread"))?;
    MacApp::run(mtm, trace_sender)
}
