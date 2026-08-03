# `js_engine` — generic JS engine trait

<https://tc39.es/ecma262/>

Bridges between ECMAScript engines (Boa, JavaScriptCore, and V8) and formal-web's
HTML/DOM/WebIDL layers.  Migration to a fully generic `JsEngine<T>` /
`ExecutionContext<T>` trait architecture is complete — content code
never depends on backend-specific APIs.

## Architecture

Two categories of abstraction:

1. **Standard** — `JsEngine<T>` and `ExecutionContext<T>` mirror ECMA-262
   abstract operations (§7–§27). `ExecutionContext<T>` is threaded through
   every binding function and domain method as the HTML specification's realm
   execution context.
2. **Engine-specific** — `gc.rs` abstracts GC (`Trace`, `Finalize`,
   `GcRootHandle`, `GcCell`) which has no ECMA-262 equivalent.

### Key traits

| Trait | Role |
|---|---|
| `JsTypes` | Associated types for a backend's value/object/string/realm/etc. |
| `JsEngine<T>` | Factory operations: realm creation, script evaluation, builtin functions |
| `ExecutionContext<T>` | Interface for ECMA-262 operations that reference the surrounding agent's running execution context |
| `JsTypesGcExt` | Cycle-safe reflector link between Rust domain objects and their JS wrappers |

### Module layout

| Module | Contents |
|---|---|
| `types` | `JsTypes`, `JsTypesWithRealm` |
| `engine` | `JsEngine`, `ExecutionContext`, `Completion`, `HostHooks` |
| `enums` | `Numeric`, `PreferredType`, `IntegrityLevel`, `PromiseState`, etc. |
| `records` | `IteratorRecord`, `PromiseCapability`, `PromiseResolvers`, `PropertyDescriptor`, `RealmIntrinsics` |
| `gc` | `Trace`, `Finalize`, `GcRootHandle`, `GcCell` (backend-abstracted) |
| `boa/` | Boa backend implementation |
| `jsc/` | JSC backend implementation (macOS only) |
| `v8/` | V8 backend implementation through `rusty_v8` (macOS arm64 only) |

## Feature flags

| Flag | Engine | Default |
|---|---|---|
| `v8` | V8 150.1.0 through `rusty_v8` (macOS arm64) | **default** |
| `boa` | Boa (git dep) | opt-in |
| `jsc` | JavaScriptCore (macOS, experimental) | opt-in |

Exactly one engine feature must be active. V8 and WebAssembly cannot be
enabled together.

## Build commands

### V8 (default, macOS arm64)

```bash
# Build everything
rustup run 1.94.0 cargo build --release

# Run WPT suite
rustup run 1.94.0 cargo run --release -- wpt

# Run the generic engine tests
rustup run 1.94.0 cargo test -p content generic_js_test
```

The first build downloads the pinned V8 150.1.0 archive. Set
`RUSTY_V8_ARCHIVE=/absolute/path/to/librusty_v8_release_aarch64-apple-darwin.a.gz`
to use a local archive, or set `RUSTY_V8_MIRROR` to an alternate releases base
URL. Cargo also caches downloaded archives under `.cargo/.rusty_v8` in the
Cargo home directory.

### Boa (opt-in)

```bash
# Build js_engine crate
rustup run 1.94.0 cargo build --release --no-default-features --features boa -p js_engine

# Build content binary with Boa
rustup run 1.94.0 cargo build --release --no-default-features --features boa,media -p content --bin formal-web-content

# Run a single WPT test via Boa
rustup run 1.94.0 cargo run --release --no-default-features --features boa,media -- wpt dom/nodes/Element-hasAttribute.html
```

### JSC (macOS only, experimental)

```bash
# Build js_engine crate
rustup run 1.94.0 cargo build --release --no-default-features --features jsc -p js_engine

# Build content binary with JSC
rustup run 1.94.0 cargo build --release --no-default-features --features jsc -p content --bin formal-web-content

# Run a single WPT test via JSC
target/release/formal-web wpt dom/nodes/Element-hasAttribute.html
```

WebAssembly support is deferred for V8; use Boa with the `wasm` feature.

## WPT test results

### V8 backend (default — run full suite)

Latest: `executed=79 unexpected=2`

The two unexpected results are both BYOB byte-stream tests:
`streams/readable-byte-streams/patched-global.any.js` and
`streams/readable-byte-streams/respond-after-enqueue.any.js` (bytes read back
as 0 instead of the written values).

### Boa backend (opt-in)

Latest: `executed=79 unexpected=2` — the same two BYOB failures as V8.

Wasm tests are excluded from the default WPT run (opt-in `--features wasm`).

### JSC backend (experimental)

**PASS:** CSS.supports, DOM Element tests, Node-constants, document.title,
document-dir, iframe, anchor, basic streams (constructor, default-reader,
strategies, transform, writable), formal gc-protection.

**TIMEOUT:**  Most piping tests, cancel, read-task-handling.

**FAIL:** structured-clone (Blob not implemented), wasm compile (timeout).

## Remaining work

### V8 platform-object edge tracing through cppgc

`GcCell<T>` is now cppgc-backed on V8: `V8GcCell` allocates a `HeapCell<T>`
on the isolate's `cppgc::Heap` and keeps it alive with a strong `Persistent`
root, and access requires isolate-scoped proof (the execution context). The
`HeapCell` trace is currently a no-op because the values stored inside the
cells hold `v8::Global` handles, which are strong roots.

Remaining: migrate the JavaScript references inside platform objects from
`v8::Global` handles to cppgc `Member`/`WeakMember`/`TracedReference` edges.
Objects allocated on the heap must then trace every edge during marking, and
off-heap owners use `Persistent` handles only when they are genuine roots.
Add forced-collection tests covering reflector cycles, platform-object cycles,
weak edges, finalization, and isolate destruction.

### BYOB byte-stream WPT failures (both backends)

`streams/readable-byte-streams/patched-global.any.js` and
`streams/readable-byte-streams/respond-after-enqueue.any.js` fail on both Boa
and V8: the tests read back zeroed bytes instead of the values written via
`byobRequest.respond()`. Not yet investigated.

### JSC microtask drain during nested C API calls

`promise_state()` uses `eval_script_raw("void 0")` to drain microtasks, but
JSC only drains its microtask queue when control returns from the outermost
C API call.  Inside nested calls (common — stream algorithm code runs inside
a JS call), the eval does not trigger drainage and `.then()` handlers never
fire.

**Dead end:** No public C API forces JSC microtask drainage.  Tracked
promise states fail because stream algorithms poll CHAINED promises (via
`.then()`), not the original tracked promise.

### Other unfixed issues

- **`setTimeout` not pumped during piping tests** — `delay()` timeouts.
- **`instanceof Window` returns false (JSC)** — Global object's `[[Prototype]]`
  is immutable through the public C API.
- **`WindowTimer.arguments`** — `Vec<JsValue>` elements unprotected from GC.
  Needs `GcRootHandle` wrapping.
- **`detach_array_buffer` (JSC)** — No-op (`Ok(())`).
- **`species_constructor`** — Always returns `default_constructor`.
- **Cross-realm `new.target`** — `get_function_realm` always returns current realm.
- **WASM compile/instantiate timeout (JSC)** — Background compilation requires
  the creating thread's run loop to be pumped.
