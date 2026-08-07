# Boa backend (`js_engine/src/boa`)

Opt-in engine: a git dependency on `boa-dev/boa`. Boa has no native
WebAssembly; the `wasm` feature wires the Wasmtime-based WebAssembly
implementation for this backend. (V8 and JSC implement WebAssembly
natively — see `../v8/README.md` and `../jsc/README.md`.)

## Build

```bash
# Build js_engine crate
rustup run 1.94.0 cargo build --release --no-default-features --features boa -p js_engine

# Build content binary with Boa
rustup run 1.94.0 cargo build --release --no-default-features --features boa,media -p content --bin formal-web-content

# Run a single WPT test via Boa
rustup run 1.94.0 cargo run --release --no-default-features --features boa,media -- wpt dom/nodes/Element-hasAttribute.html
```

## WPT results

Last recorded: `executed=79 unexpected=2` — the same two BYOB failures as
V8 (see `../README.md`, "Known cross-engine failures").

Wasm tests are excluded from the default WPT run (opt-in `--features wasm`).
