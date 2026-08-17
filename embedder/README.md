# embedder crate

The embedder crate owns the top-level application lifecycle, window management,
browser chrome, and the redraw loop. It delegates to content and net
processes through the `webview` and `user_agent` crates.

## Architecture

### Two app implementations

- **`WindowedApp`** (`event_loop/windowed/mod.rs`): headed (GUI) application with
  native windows, a Blitz-rendered browser chrome, and multi-window/multi-tab
  support. Runs via winit's event loop.

- **`HeadlessEmbedderApp`** (`event_loop/headless.rs`): headless application for
  automation-only hosting (WebDriver, CDP). No window, no chrome, just a fixed
  viewport and event-loop plumbing.

### Windowed backend split (`embedder-backend`)

The headed app is provided by one of two backends selected at startup via
`install_default_windowed_backend()`. On macOS the AppKit backend is the
default and the winit backend is **not compiled** unless the
`winit_embedder` feature is enabled (`--features winit_embedder`); on other
platforms the winit backend is the only option and the feature is a no-op.

- **`mac-embedder`** (AppKit): the default on macOS. Runs an `NSApplication`
  with `NSWindow`/`NSView`/`CALayer` display; the web content is presented
  zero-copy by setting the content layer's `contents` to the shared IOSurface
  from the graphics process. The chrome is native AppKit controls: a main
  menu bar (App/File/Edit/View/History/Window/Help), a real `NSToolbar` in a
  unified (transparent-titlebar, full-size content) window, and a tab strip
  as its own row below the toolbar. The toolbar hosts a joined
  back/forward control, a reload item, the editable address field (which
  shows the active tab's URL) centered between two flexible spaces, and a
  new-tab button at the trailing edge; focusing the address field draws a
  tight accent-colored border on the field (instead of the system focus
  ring) and selects the whole URL on first focus. The tab strip row (a
  header-view material) hosts pill-styled tabs (rounded, the active tab
  filled in light grey, the close × always visible, and a hover fill
  darker than the active pill so the hover stays visible on the active
  tab) that show the page title, falling back to the truncated URL, and
  shrink as more tabs open. The window title mirrors the active tab's
  label. A `CVDisplayLink` paces
  animated content via `WebviewProvider::frame_needed`.

  Menu key equivalents are executed from the local event monitor via
  `NSMenu::performKeyEquivalent`, which lets the menu own ⌘T/⌘W/⌘L/⌘R and
  friends while unbound ⌘-combinations still reach the web content (pages
  keep their own ⌘-shortcuts). The toolbar allows user customization
  (drag-to-rearrange the navigation items, Customize Toolbar…) but the
  address field is immovable. The web viewport is the window's
  `contentLayoutRect` minus the tab strip row, and mouse events in the
  titlebar/toolbar/tab-strip region pass through to AppKit. The content
  process reports a top-level document's parsed `<title>` after parsing
  (content → user agent → embedder), so tab labels and the window title
  reflect page titles on load; titles changed later via JS
  (`document.title = …`) are not yet propagated.

- **`winit-embedder`**: winit windows with a Blitz-rendered chrome. The
  only option on non-macOS platforms; on macOS it is built and used only
  when the `winit_embedder` feature is enabled.

Known gaps in the AppKit backend relative to winit:

- **IME is not implemented.** The AppKit backend sends `KeyDown`/`KeyUp` events
  only; text composition (CJK and other marked-text input) requires the
  `NSTextInputClient` protocol on the web content view, which is not yet wired.
  Basic ASCII text input into page fields works through `KeyDown` text.
- **Touch events are not handled** (desktop macOS has no touch input; winit's
  touch path is for trackpads/tablets).
- **JS-driven titles are not propagated.** The content process reports a
  top-level document's parsed `<title>` after parsing, but titles changed
  later via JS (`document.title = …` or DOM manipulation of the title
  element) are not sent, so a page that sets its title after load keeps the
  parsed title.
- **Session history is not implemented.** The History menu's Back/Forward items
  are disabled; Reload re-navigates to the tab's committed URL because the
  user agent has no reload command.
- **Closing a tab does not tear down its traversable.** The user agent has no
  webview-teardown path, so a closed tab's webview keeps living there (the
  same situation as closing a window).

### Multi-window and multi-tab

`WindowedApp` owns a `HashMap<WindowId, WindowState>` where each `WindowState`
represents one native window (one winit `Window` + one `VelloWindowRenderer`).

Each window has:

- A `ChromeUi` instance — a Blitz-based HTML/CSS chrome with an address bar
  and a tab strip.
- A `HashMap<WebviewId, TabState>` of open tabs, ordered by a `Vec<WebviewId>`
  (`tab_order`).
- One `active_tab` (`Option<WebviewId>`) — the currently displayed tab.
- An `AutomationController` for WebDriver/CDP integration.
- Per-window input state (pointer position, keyboard modifiers, mouse buttons).

A `webview_to_window` mapping routes `WebviewId`-scoped events
(`NavigationRequested`, `NavigationCompleted`, `NewWebview`, `RequestRedraw`)
to the correct window.

### Tab lifecycle

1. A tab is created when the user agent dispatches a `NewWebview` event
   (triggered by `provider.navigate(None, url)` or by the user clicking the
   `+` button in the chrome).
2. The `NewWebview` handler calls `add_tab()` which inserts a `TabState` into
   the window's tab map and pushes the webview ID onto `tab_order`.
3. Navigation state is tracked per-tab via `pending_url` and `committed_url`.
4. The chrome tab strip is rebuilt whenever tab count changes (the
   `ChromeUi` re-generates its HTML template with ordered tab buttons).

### Viewport management

Each window computes its content viewport as
`(window_width, window_height - chrome_height, scale, color_scheme)` and
propagates it to the provider via `set_default_viewport` (for new traversables)
and `set_traversable_viewport` (for the active tab's traversable).

Viewport updates happen on:
- Window creation (`resumed`)
- Tab creation (`NewWebview`)
- Tab switch (`SwitchTab`)
- Navigation progression (`NavigationRequested`, `NavigationCompleted`)
- Window resize (`Resized`)

### Chrome

The chrome is rendered as a Blitz HTML document with CSS styling. It contains:
- An address bar (`<input id="address">`) — shows the active tab's current URL.
- A tab strip with tab buttons (`<button id="tab-N">`) — one per open tab.
- A `+` button (`<div id="new-tab-btn">`) — opens a new tab; shift+click
  opens a new window.

When tab state changes, the entire chrome HTML is regenerated with the correct
number of tab buttons (each with a unique DOM id like `tab-0`, `tab-1`, etc.).
Hit-testing uses the `id` attribute from the DOM (not node IDs) to avoid stale
references after HTML rebuilds.

## Current implementation status

- [x] Multi-window support (one winit event loop, many windows)
- [x] Multi-tab support per window (webview-backed tabs)
- [x] Chrome: address bar with URL display
- [x] Chrome: tab strip with click-to-switch
- [x] Chrome: `+` button for new tab / shift+click for new window
- [x] Tab labels show page URL (truncated) or "New Tab" for blank pages
- [x] Viewport tracking and propagation to provider
- [x] Automation (WebDriver/CDP) targets the active tab in the active window
- [x] Navigating an existing tab to `about:blank` logs a content-process
  "unknown document id" error (pre-existing); new top-level traversables to
  `about:blank` (new tabs, new windows, startup) work
- [ ] Address-bar Enter opens new tab instead of navigating (under investigation)
- [ ] Tab close button
- [ ] Tab reordering

## Possible future work

- **Tab close button**: Add an `×` button to each tab for closing. Requires
  a `ChromeAction::CloseTab(usize)` action and cleanup of the tab state,
  compositor, and webview-to-window mapping.
- **Tab reordering**: Make tabs draggable to reorder. Requires drag-and-drop
  in the chrome HTML and updating `tab_order` accordingly.
- **Tab drag-out to new window**: Dragging a tab out of its window creates a
  new window with that tab. Requires moving a `TabState` between windows.
- **URL bar spellcheck/suggestions**: Autocomplete or search-engine integration
  in the address bar.
- **Window title update**: Sync the winit window title with the active tab's
  page title. The content→UA title plumbing exists (parse-time titles); the
  winit window title is not yet set from it.
- **CDP multi-target support**: Expose each tab/window as a separate CDP target
  (`Target.getTargets`, `Target.attachToTarget`) so automation tools can
  interact with specific pages.
- **About:blank fix**: Navigating an *existing* tab to `about:blank` (e.g.
  the address bar) logs a content-process "unknown document id" error during
  navigation finalization, although the URL still ends up as `about:blank`.
  New top-level traversables to `about:blank` (new tabs, new windows, CDP
  startup) work; only the existing-tab path is affected.
- **Browser history integration**: Remove the per-tab `committed_url` /
  `pending_url` tracking in favour of the user agent's session history once
  that's implemented.
- **Performance**: The chrome HTML is fully rebuilt whenever tab count changes.
  For many tabs this could be slow. A virtual-scrolling tab strip or
  incremental DOM updates would scale better.
- **Headless/headed sharing**: Some input-event dispatch helpers are duplicated
  between `WindowedApp` and `HeadlessEmbedderApp`. These could be extracted
  into shared utility functions.

## Key files

| File | Purpose |
|------|---------|
| `src/main.rs` | CLI entry point |
| `src/event_loop.rs` | Event loop orchestration, shared types |
| `src/event_loop/winit.rs` | Winit integration (shell provider, key/mouse mapping) |
| `src/event_loop/windowed/mod.rs` | `WindowedApp` + `WindowState` |
| `src/event_loop/windowed/chrome.rs` | `ChromeUi` — Blitz-based browser chrome |
| `src/event_loop/headless.rs` | `HeadlessEmbedderApp` |
| `src/ui_event.rs` | UI event serialization |
| `backend/src/lib.rs` | Backend selection (`install_default_windowed_backend`) |
| `mac-embedder/src/app.rs` | AppKit application, window/chrome/event routing |
| `mac-embedder/src/window.rs` | Layer-hosting view, IOSurface presentation |
| `mac-embedder/src/input.rs` | NSEvent → Blitz input mapping |
| `winit-embedder/src/windowed.rs` | `WindowedApp` — winit window/chrome/events |
