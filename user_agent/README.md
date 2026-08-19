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
