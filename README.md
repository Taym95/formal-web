# formal-web

formal-web is a Rust web-engine prototype with a modular architecture and support for formal verfication.

## Getting Started

The project has only been run on macOS; all build commands assume macOS.

The default: **Boa** as the js engine and **AVFoundation** as the media backend:
```bash
cargo build --release
cargo run --release
```

For Boa, wasm support can be added via Wasmtime by way of `--features wasm`.

**JSC** (experimental):
```bash
cargo build --release --no-default-features --features jsc,media
cargo run --release --no-default-features --features jsc,media
```

**V8** (experimental):
```bash
cargo build --release --no-default-features --features v8,media
cargo run --release --no-default-features --features v8,media
```

**GStreamer** media backend instead of AVFoundation:
```bash
cargo build --release --no-default-features --features backend-gstreamer,boa,media
```


## Project architecture

A multiprocess approach is chosen by default, with the goal of having the possibility to meet [Apple's guidelines for an independent browser engine](https://developer.apple.com/documentation/BrowserEngineKit/designing-your-browser-architecture). 

Besides this, a modular approach is followed by making the following components generic with swappable implementations:

- The JS engine: Boa, V8, or JSC. 
- The media engine: Gstreamer or AvFoundation.
- The IPC layer: ipc-channel or Xpc/BrowserKit.
- The networking layer (planned, for now tokio only).

The following procesess are used:

- **Main** (`src/main.rs`): runs the `embedder`, `webview`, and `user_agent` crates.
- **Content** (`user_agent/src/event_loop.rs`): runs the `content` crate. Multiple processes: one per [similar origin window agent](https://html.spec.whatwg.org/#similar-origin-window-agent).
- **Graphics** (`graphics/src/bin/graphics_process.rs`): runs the `graphics` and `media` crates.
- **Net** (`user_agent/src/fetch.rs`): runs the `net` crate.

## Formal verification

A set of core algorithms will be formalized using TLA+, and their Rust implementation model-checked against those formal specification using the tracing approach described in [Validating Traces of Distributed Programs Against TLA+ Specifications](https://arxiv.org/abs/2404.16075). For further details, see [the verification folder](verification/README.md).

## Pi coding agent extensions

Pi automatically discovers extensions in `.pi/extensions/` (one level deep, each
directory containing an `index.ts` or a `package.json` with a `pi.extensions`
field). The extensions are plain TypeScript with their own npm dependencies.

### Setup

`node_modules/` is git-ignored, so a fresh checkout must install the npm
dependencies for each extension before pi can load it. From the repository root:

```bash
cd .pi/extensions/browser && npm ci && cd ../../..
cd .pi/extensions/web_standards && npm ci && cd ../../..
```

After this, restart pi in the repository directory (or reload extensions if you
are already in a session) and the tools and commands below become available.
If an extension fails to load with `Cannot find module 'ws'` or
`Cannot find module 'cheerio'`, the npm install step above was skipped.

### Extensions

- [**`browser`**](.pi/extensions/browser/README.md) — browser automation for
  testing. Depends on [`ws`](https://www.npmjs.com/package/ws) for its WebSocket
  CDP client. Connect it to formal-web's CDP server (`/browser-connect <port>`)
  to drive live debugging sessions; the extension also works with standard
  Chrome/Chromium instances.
- [**`web_standards`**](.pi/extensions/web_standards/README.md) — interactive
  spec reading (`spec_lookup`, `spec_ref_links`, `spec_search_id`). Depends on
  [`cheerio`](https://www.npmjs.com/package/cheerio) for server-side HTML
  parsing and traversal of WHATWG/W3C spec documents.
- [**`readme-chain`**](.pi/extensions/readme-chain/README.md) — walks the
  AGENTS.md/README.md documentation chain for a path; no npm dependencies.