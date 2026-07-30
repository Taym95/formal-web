# formal-web

formal-web is a Rust web-engine prototype in alpha status, with an embedding API and an optional TLA+ verification layer.

## Getting Started

The project has only been run on macOS; all build commands assume macOS.

**Boa** + **AVFoundation** (default):
```bash
cargo build --release
cargo run --release
```

Add `--features wasm` for wasmtime-based WebAssembly module support (Boa only).

**JSC** (experimental):
```bash
cargo build --release --no-default-features --features jsc,media
cargo run --release --no-default-features --features jsc,media
```

**V8** (experimental, macOS arm64 only):
```bash
cargo build --release --no-default-features --features v8,media
cargo run --release --no-default-features --features v8,media
```

V8 cannot be combined with `wasm`. The first build downloads the pinned V8
archive. Set `RUSTY_V8_ARCHIVE` or `RUSTY_V8_MIRROR` for offline or mirrored
builds.

**GStreamer** media backend instead of AVFoundation:
```bash
cargo build --release --no-default-features --features backend-gstreamer,boa,media
```

** WPT Testing:**
```bash
cargo run --release -- wpt
```

## Spec-algorithm annotations

Every function, struct, or constant that implements a spec algorithm carries
**only** the spec anchor URL in its doc comment — zero prose.  Inside the
function body, every spec step has a `// Step N:` comment quoting the
exact spec text verbatim, with blank lines separating code blocks (not
comments from code).  See `AGENTS.md` §Algorithm Implementation for the
complete rules.

## Project architecture

A multiprocess approach is chosen by default, because the goal is to match [Apple's guidelines for an independent browser engine](https://developer.apple.com/documentation/BrowserEngineKit/designing-your-browser-architecture). 

The following procesess are used:

- **Main** (`src/main.rs`): runs the `embedder`, `webview`, and `user_agent` crates. Owns windows, chrome, and the redraw loop.
- **Content** (`user_agent/src/event_loop.rs`): runs the `content` crate. One per [similar origin window agent](https://html.spec.whatwg.org/#similar-origin-window-agent). Each is a dedicated event loop that handles parsing, JS execution, and `paint_scene` for its documents.
- **Graphics** (`graphics/src/bin/graphics_process.rs`): runs the `graphics` and `media` crates. Receives `PaintFrame` payloads directly from content processes via IPC shared memory, composes them (including iframe and video embed sites), renders the final scene via Vello GPU rasterisation, and ships the RGBA result back to the embedder. The media backend (AVFoundation or GStreamer) runs on the graphics process's main thread, driven by a timer arm in the select loop — no separate media process.
- **Net** (`user_agent/src/fetch.rs`): runs the `net` crate. Handles HTTP and file fetch requests.

## Project structure

| Directory | Description |
|-----------|-------------|
| [`embedder/`](./embedder/README.md) | Application lifecycle, window management, browser chrome, redraw loop |
| [`user_agent/`](./user_agent/README.md) | Navigables, session history, event loops, timers, fetch workers |
| [`content/`](./content/README.md) | DOM, HTML algorithms, Boa JS integration, Web IDL bridges |
| [`graphics/`](./graphics/README.md) | Scene composition, Vello GPU rasterisation, hit-testing |
| [`media/`](./media/README.md) | Media pipeline: GStreamer or AVFoundation backend, frame extraction |
| [`net/`](./net/README.md) | HTTP and file fetch |
| [`webview/`](./webview/README.md) | Embedder-facing compositor and redraw API |
| [`automation/`](./automation/README.md) | WebDriver and CDP wire-protocol servers |
| [`verification/`](./verification/README.md) | Trace recording, TLA+ validation |
| `ipc_messages/` | Shared IPC message types |
| [`tests/`](./tests/formal/README.md) | Formal tests and WPT runner |
| `artifacts/` | Default startup pages for testing |

## Extensions

- [**`browser`**](.pi/extensions/browser/README.md) — Wraps CDP server into agent-callable dev tools
- [**`web_standards`**](.pi/extensions/web_standards/README.md) — Lazily loaded web spec content for interactive reading
