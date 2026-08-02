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

The graphics process has two independent backends — scene delivery
(zero-copy IOSurface, macOS only, or CPU readback, all platforms) and
video frames (AVFoundation keeps decoded frames on the GPU, GStreamer
delivers CPU bytes):

| Build | Scene delivery | Video | Result |
|---|---|---|---|
| macOS default | zero-copy (IOSurface) | AVFoundation (GPU) | fully zero-copy |
| macOS + `--no-default-features --features backend-gstreamer,boa,media` | zero-copy (IOSurface) | GStreamer (CPU bytes) | video via CPU, scene zero-copy |
| macOS + `-p graphics --features cpu_readback` | CPU readback | either | scene via shared memory |
| Linux | CPU readback | GStreamer (CPU bytes) | no zero-copy |

Trade-offs: zero-copy avoids a GPU→CPU readback, IPC pixel bytes, and a
CPU→GPU upload per frame but needs macOS (IOSurface); CPU readback works
everywhere. AVFoundation keeps video on the GPU (macOS only); GStreamer
runs everywhere but copies each decoded video frame through the CPU.


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