# net crate

The `net` crate owns the `formal-web-net` entrypoint and executes fetch requests on behalf of the user-agent fetch worker.

- Launches the dedicated net process and performs typed IPC bootstrap.
- Executes file and HTTP fetches and returns typed responses.
- Keeps network work behind a separate process boundary.
- Will host HTTP cache logic when the Fetch spec reaches that layer.

## Modular network backends

Network work is hidden behind the `NetworkBackend` trait
(`net/src/backend/`), whose single method, `http_network_or_cache_fetch`,
maps coarsely to the fetch spec's
[HTTP-network-or-cache fetch](https://fetch.spec.whatwg.org/#http-network-or-cache-fetch).
Each build feature provides one concrete backend:

| Feature | Backend | Notes |
|---|---|---|
| `url_session` | `UrlSessionBackend` (`backend/url_session.rs`) | Apple URLSession, macOS/iOS only; wins over `tokio` when both are enabled |
| `tokio` | `TokioBackend` (`backend/tokio.rs`) | reqwest blocking client |

On Apple platforms the URLSession backend is the default: `build.rs` emits
the `url_session_default` cfg when no backend feature is enabled, so a
plain `cargo build` compiles no reqwest/tokio stack there. On non-Apple
platforms the tokio backend is the only option and is always compiled. The
tokio backend on Apple is opt-in via `--features tokio` (the URLSession
feature flag itself is a marker — the `url_session` crate is always
compiled on Apple).

The root build prebuilds `formal-web-net` with the platform default backend
(see `build.rs`).

### Session partitioning by event loop

Each backend keeps one session per
[network partition key](https://fetch.spec.whatwg.org/#network-partition-key),
which is modelled as the event loop id that travels on every fetch message:
the tokio backend stores one reqwest client per key, the URLSession backend
stores one session (with no shared cache) per key. Sessions are created on
first use and reused.

### Response routing lives in the backends

`route_response` (`backend/mod.rs`) delivers the fetch outcome to the
`reply_to` recipient, which carries the response sender in both cases — the
content process's command sender, or the user agent's response channel. The
UA reply channel is modelled on the fetch spec's
[parallel queue](https://fetch.spec.whatwg.org/#fetch-useparallelqueue):
responses are delivered onto it. Because routing lives in the backend, the
response may be sent at any time: the tokio backend sends it before its
fetch method returns, the URLSession backend sends it from the data task's
completion handler (a background queue). The UA's sender end of the
net→UA response channel is retained at extension launch by the ipc crate
(`IpcConnection::incoming_sender`) and embedded into
`ResponseRecipient::UserAgent` by `user_agent/src/fetch.rs`.

## url_session and url_session_sys

- `net/url_session` — safe Apple URLSession client: one session with no
  shared cache, fetch completions delivered on a background queue.
- `net/url_session_sys` — raw Objective-C bindings: a C/ObjC wrapper
  (`src/url_session_wrapper.m`, compiled via `cc`) exposing a small C API
  (`fw_url_session_create/fetch/release`), following the `ipc/xpc-sys`
  pattern. Apple targets only.
