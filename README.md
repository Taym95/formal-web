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

**Zero-copy surface backend** (macOS, optional): by default rendered frames are
shipped as CPU readback + IPC shared memory. With
`FORMAL_WEB_SURFACE_BACKEND=iosurface` the graphics process renders directly
into a shared IOSurface and the embedder blits it — no CPU readback, no IPC
pixel bytes. This requires the forked `ipc-channel` (see below):
```bash
cargo build --release
FORMAL_WEB_SURFACE_BACKEND=iosurface cargo run --release
```
Resize updates in one step once the resize settles (no live-resize animation);
see `graphics/README.md` for details.


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

### External dependency: forked ipc-channel

The zero-copy IOSurface surface path needs to transport arbitrary Mach ports
(an IOSurface's) inside IPC messages, which upstream ipc-channel cannot do. The
workspace depends on a fork at
<https://github.com/gterzian/ipc-channel> that adds an `OsMachPort`
serde-transportable type (each crate declares it as a git dependency; the
`Cargo.lock` pins the fork revision). The CPU-only surface path also builds
from the fork.

## Formal verification

A set of core algorithms will be formalized using TLA+, and their Rust implementation model-checked against those formal specification using the tracing approach described in [Validating Traces of Distributed Programs Against TLA+ Specifications](https://arxiv.org/abs/2404.16075). For further details, see [the verification folder](verification/README.md).

## Pi coding agent extensions

- [**`browser`**](.pi/extensions/browser/README.md) — browser automation for testing
- [**`web_standards`**](.pi/extensions/web_standards/README.md) — interactive spec content