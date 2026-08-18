# user_agent crate

The `user_agent` crate owns all browser-global coordination: navigables and traversables, navigation and session history, event loops, timers, content-process lifecycle, and requests coming from the embedder and webview layers.

- `user_agent.rs` owns the top-level user-agent state and command loop (uses `select!` to also process net, graphics, and media responses directly).
- `event_loop.rs` owns content event loops and manages the content process.
- `timer.rs` owns the timer worker.
- `fetch.rs` provides `NetConnection` — owns the IPC connection to the net extension,
  tracks pending navigation fetches, and routes responses back to the user agent.
- `ui_event.rs` provides UI event serialization for routing across process boundaries.
- The UA and content processes send requests directly to the net, graphics, and media extensions;
  there are no intermediary fetch or media worker threads.
- Key cross-worker ownership with UUID newtypes such as `EventLoopId`, `NavigableId`, and related ids from `ipc_messages`.
- Keep spec-facing algorithms and continuations as named worker methods on the owning type instead of as transport-oriented helper functions.
- Route browser, embedder, automation, and webview requests through this crate instead of through synchronous cross-thread bridges.

## Graphics process routing

The user agent starts the `formal-web-graphics` process alongside net and media on startup.
Paint frames from content processes are forwarded to the graphics process via
`GraphicsCommand::PaintFrame`. The graphics process composes scenes (iframe embed
sites + video frames) and sends the final composed scene back via
`GraphicsEvent::ComposedSceneReady`. The UA stores the accompanying
`FrameHitInfo` for hit-testing and forwards the scene to the embedder host
via `Embedder::new_web_content_scene`.

Hit-testing info (`FrameHitInfo`) from each composed scene is stored in
`UserAgentState::frame_hit_info`, keyed by webview id. This data enables
UI event routing without the embedder needing access to the compositor tree.

During a cross-origin navigation the traversable's event loop (and content
process) switches before the UA-side active document does: the active
document only changes at finalization, so in the migration window
`traversable_handles` and `active_documents_by_traversable` disagree.
Commands pairing those two maps (e.g. `UpdateTheRendering`) must verify the
active document is owned by the traversable's current event loop and skip
otherwise — a stale send fails in the new content process and, because no
paint frame is produced, leaves the render loop's pending flag stuck.

## Window.open flow

`window.open()` goes through the shared `navigate` path. The content process
resolves the easy cases (`_self`) directly and sends a `NavigateRequest` IPC
with `features_json` set and an optional `chosen_navigable_id`. The user agent:

1. When `chosen_navigable_id` is `Some`, uses it directly.
2. When `chosen_navigable_id` is `None`, runs the remaining rules-for-choosing
   steps: find-by-target-name (cross-process), or create a new top-level
   traversable.
3. Notifies the embedder to open a new tab for new top-level traversables.
4. Sets up the opener relationship (`opener_browsing_context`) for
   `"new and unrestricted"` window types (step 15.3 of window-open-steps).
5. Navigates the target navigable.

WindowProxy return value is a null placeholder on the content side — the
user agent only performs the navigation and does not need to maintain
a reference for the caller.

## Iframe navigation flow

Iframe navigation is the content → UA → content round trip that exercises the
navigable-owning half of the navigate algorithm; the content-side trace lives
in `content/src/html/README.md` ("Iframe navigation flow").  The UA-side
steps are:

1. `handle_navigate` with `new_child_navigable` runs the UA-side of "create a
   new child navigable": browsing context group membership, document state,
   session history, and registration of the child on the parent's event loop
   (same process, same agent cluster) — the child starts life as an
   about:blank document created by the content process.
2. `navigate` runs the shared navigate algorithm for the child
   (`check_if_unloading_is_canceled` → `create_navigation_params_by_fetching`
   → net fetch).  A newer navigation supersedes an in-flight one: the older
   continuation's `navigation_is_current` check aborts it, which is how the
   child's initial about:blank navigation yields to the real `src`
   navigation.
3. `initialise_the_document_object` runs the UA-side of
   "create and initialize a Document object" — step 1 (obtain the browsing
   context) and step 7 (agent selection).  For a child navigable the
   cross-origin check between the parent document and the destination decides
   between the parent's event loop (same process) and a fresh agent (new
   content process, traversable moved before `CreateLoadedDocument` is
   dispatched).  The initial about:blank Window reuse of spec step 6 is
   implemented on the content side (`ContentProcess::initialise_the_document_object`):
   the child's initial about:blank inherits the parent's origin, and a
   same-origin destination reuses the child's realm/Window instead of
   creating a fresh one.
4. `finalize_cross_document_navigation` commits the new document: active
   document switch, session history push, destroy of the previous document
   (routed by its owning event loop), and graphics scene-root replacement.
