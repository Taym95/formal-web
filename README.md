# formal-web

formal-web is a Rust web-engine prototype in alpha status.

## Getting Started

The project has only been run on macOS; all build commands assume macOS.

The default: (Boa as the js engine and AVFoundation as the media backend):
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

- **Main** (`src/main.rs`): runs the `embedder`, `webview`, and `user_agent` crates. Owns windows, chrome, and the redraw loop.
- **Content** (`user_agent/src/event_loop.rs`): runs the `content` crate. Multiple processes: one per [similar origin window agent](https://html.spec.whatwg.org/#similar-origin-window-agent).
- **Graphics** (`graphics/src/bin/graphics_process.rs`): runs the `graphics` and `media` crates.
- **Net** (`user_agent/src/fetch.rs`): runs the `net` crate.

## Pi coding agent extensions

- [**`browser`**](.pi/extensions/browser/README.md) — browser automation for testing
- [**`web_standards`**](.pi/extensions/web_standards/README.md) — interactive spec content