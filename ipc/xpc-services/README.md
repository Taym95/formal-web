# XPC Service Configuration for formal-web

The `ipc/` crate provides an abstract IPC layer with two backends:

- **`ipc-channel-backend`** (default, works reliably on all platforms)
- **Native XPC** (macOS only, experimental, requires additional setup)

The native XPC backend is **disabled by default** — the `ipc-channel-backend`
feature (the ipc crate's default) is used for all extensions. Build the
workspace with `--no-default-features` to disable it and enable the mixed
backend: XPC for net and media, ipc-channel for content.

## Prerequisites for Native XPC

The native XPC backend requires each helper process to be registered as a launchd
XPC service. The content process always uses ipc-channel even in XPC mode because
macOS AMFI rejects ad-hoc-signed embedded XPC services.

## Setup (for native XPC development)

```bash
# 1. Build all binaries (V8 engine, media enabled)
cargo build --release --no-default-features --features v8,media

# 2. Install XPC service plists with correct binary paths
./xpc-services/install.sh $(pwd)/target/release

# 3. Load services into launchd (content plist is unused — content uses ipc-channel)
launchctl load ~/Library/LaunchAgents/formal-web.net.plist
launchctl load ~/Library/LaunchAgents/formal-web.media.plist

# 4. Run with native XPC backend
cargo run --release --no-default-features --features v8,media
```

## Why Content Can't Use XPC

macOS **AMFI (Apple Mobile File Integrity)** rejects ad-hoc-signed binaries in
embedded XPC services with error:

```
amfid: not valid: Error Code=-423
"The file is adhoc signed or signed by an unknown certificate chain"
```

This happens even with Developer Mode enabled (`developerMode: 1`). Embedded
XPC services (inside an `.app` bundle's `XPCServices/` directory) require a
**paid Apple Developer certificate** for code signing — ad-hoc signing is
insufficient, and no workaround has been found. The AMFI rejection is the
remaining reason content cannot use XPC.

## Known Issues

- Content process cannot use XPC (macOS AMFI rejects ad-hoc-signed embedded
  XPC services). Content always uses ipc-channel even in mixed mode.
- Service plists must be updated whenever binary paths change.

## Architecture (XPC mode)

| Service Name | Type | Binary | Backend |
|---|---|---|---|
| `formal-web.net` | Singleton (Application) | `formal-web-net` | XPC |
| `formal-web.media` | Singleton (Application) | `formal-web-media` | XPC |
| `formal-web.content` | MultipleInstances | `formal-web-content` | ipc-channel (always) |
