# React Todo App (artifacts/todo-app)

A small todo-list app built with the real React library — react-dom
`createRoot`, hooks, reconciliation, and synthetic events — bundled with
esbuild and served locally. It exercises formal-web's DOM/event platform
surface through a production React 19 bundle.

`artifacts/StartupExample.html` links to it: the "React Todo App" section
opens `http://localhost:8080/` in a new tab.

## Run it

```bash
cd artifacts/todo-app
npm install      # installs react, react-dom, esbuild (one-time)
npm start        # builds dist/app.js and serves on http://localhost:8080
```

- `npm run build` — esbuild bundle (IIFE, minified) into `dist/app.js`.
- `npm run serve` — static file server only (needs a prior build).
- The server logs every request to `requests.log` (git-ignored); a
  `/health` endpoint returns JSON for quick checks.

`node_modules/`, `dist/`, and `requests.log` are git-ignored.

## What the app uses

- Function components, `useState`, `useEffect`, `useRef`-free props/state flow
- A class-component error boundary (`componentDidCatch`) that records errors
  on `window.__appErrors` for out-of-process inspection (CDP/WebDriver)
- Controlled `<input>` with `onChange`, list rendering with `key`s,
  conditional rendering, and a filter bar
- `document.title` update from an effect
- Standard `HTMLElement.click()` on buttons and native `MouseEvent`
  construction for event simulation

## Engine API gaps this app surfaced (all fixed in content)

Running React 19 in formal-web required the following content-process fixes
(the app failed at each one before the fix; see the session log in
`content/src/dom/README.md` for the full investigation):

1. **`Node`-binding downcast whitelists** — `HTMLInputElement`,
   `HTMLMediaElement`, `HTMLVideoElement` were missing from
   `try_with_node_ref`/`appendable_node` (content/src/js/bindings/dom/node.rs),
   so `appendChild(input)` threw "appendChild requires a Node" and React's
   commit aborted mid-tree.
2. **Event path building from inputs** — `event_target_from_js_object` and
   `build_path_from_target_js_object` (content/src/js/downcast.rs,
   content/src/js/platform_objects.rs) missed the same element types, so
   events dispatched on an `<input>` never reached ancestor listeners
   (React's delegation on the root container).
3. **GlobalEventHandlers `on*` attributes** — React feature-detects
   `"oninput" in document` to choose its input-event path. Without `on*`
   IDL attributes the detection failed and React fell back to a
   keydown/keyup-only polyfill, so `onChange` never fired. Added the full
   `on*` attribute set on `Element`, `Document`, and `Window`
   (content/src/js/bindings/dom/global_event_handlers.rs), backed by
   per-EventTarget handler storage (content/src/dom/event.rs).
4. **`HTMLInputElement.type`** — React's `isTextInputElement` reads
   `input.type`; it was missing, so the input was treated as non-text and
   `onChange` extraction was skipped.
5. **`HTMLElement.click()`** — missing entirely; added per
   <https://html.spec.whatwg.org/#dom-click>.
6. **`Element.prototype.closest`** — missing; added via stylo's
   `element_closest` (<https://dom.spec.whatwg.org/#dom-element-closest>).
7. **`MouseEvent` constructor + dispatch** — `MouseEvent` was not
   registered; added the interface, its attributes, and the dispatch/
   reflector downcast arms so `new MouseEvent(...)` dispatches with a
   visible `target` and bubbles correctly.

## Not covered here

- `fetch()` is not implemented in formal-web; the app does not use it (the
  startup page deliberately avoids a fetch-based status probe).
- `queueMicrotask`, `MessageChannel`, and `Element.prototype.contains`/
  `compareDocumentPosition` are absent; React tolerates all of them (its
  scheduler falls back to Promise/setTimeout, and its `containsNode` helper
  degrades gracefully).
