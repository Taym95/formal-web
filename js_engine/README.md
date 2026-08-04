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

Latest: `executed=79 unexpected=3..4` — unstable across runs.

Deterministic failures: `streams/readable-byte-streams/enqueue-with-
detached-buffer.any.js` FAIL, and the two `garbage-collection.any.js` files
(`streams/readable-streams/` and its `crashtests/` sibling) TIMEOUT. A
crash/ERROR flaps to a different streams test each run. The two BYOB
byte-stream failures documented previously (`patched-global.any.js`,
`respond-after-enqueue.any.js`) appear and disappear between runs. See the
session log under "Remaining work" for the crash/hang investigation.

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

### V8 GC tracing through cppgc — implemented, forced-collection tests pending

The V8 backend now integrates with cppgc (the unified heap). The design:

- `#[gc_struct]` types implement the generic `Trace` (field-walking `trace`
  visiting every cppgc edge, plus `store` converting rooted handles into
  edges) and `GarbageCollected`, generated by the new `gc_struct_v8` proc
  macro (`js_engine_macros`), which skips `#[ignore_trace]` and cfg-gated
  fields.
- `GcCell<T>` is a cppgc `Member<HeapCell<T>>` edge; cloning creates a second
  edge via `Member::new(&existing)` (`GetRustObj`). The `HeapCell` is a heap
  object whose `trace` delegates to `T`'s edges. Cells are traced from their
  owning platform object — no `Persistent` roots.
- JS references are two-mode `V8Handle`s: `Root(Global)` for ephemeral Rust
  values, `Edge(Rc<TracedReference>)` once stored. Conversion happens at
  `gc_cell_new`/`GcCell::set` (via `Trace::store`) and at reflector writes
  (`ExecutionContext::store_js_object`, `with_object_any_mut_with`).
- Platform objects are allocated on the cppgc heap (`V8PlatformData`, a
  type-erased cppgc object tracing through the concrete type) and linked to
  their JS wrapper with `v8::Object::wrap`, so the unified heap traces
  wrapper → platform → cells → JS edges and can collect cycles.
- The isolate's `CppHeap` is created with atomic marking and sweeping
  (`SharedIsolate::new`), so traces run stop-the-world on the isolate thread
  and traced-value destructors (which drop V8 handles) run on the isolate
  thread.

All existing tests pass (18 js_engine, 93 generic JS). Forced-collection
coverage lives in the js_engine test module: `engine.gc()` (a V8 full
collection plus a cppgc sweep with `NoHeapPointers`) reclaims a
wrapper↔platform reflector cycle, a two-platform mutual cycle, and a JS
object referenced only through a cell edge in a single pass; platform data
is finalized at isolate destruction. Remaining:

1. **`borrow_mut` writes of fresh values stay rooted.** `Trace::store`
   conversion runs only at the cell-construction/`set` boundaries and the
   reflector writes. Values written into cell contents through
   `GcCell::borrow_mut` (e.g. streams queue entries, `WriteRequest`
   resolvers) keep their `Global` roots (over-retention, safe — no UAF).
   Converting those would need the store operation threaded through the
   borrow-mut write sites.
2. **Edge equality approximation.** `V8Object`/`V8Symbol` `PartialEq` uses
   `V8Handle::same_identity`, which is exact for clones of one edge but
   under-approximates two independently created edges to the same object
   (use `ec.same_value` for those — `Callback::equals` already does).

### BYOB byte-stream WPT failures (both backends)

`streams/readable-byte-streams/patched-global.any.js` and
`streams/readable-byte-streams/respond-after-enqueue.any.js` fail on both Boa
and V8: the tests read back zeroed bytes instead of the values written via
`byobRequest.respond()`. Not yet investigated.

### 2026-08-04 — WPT stream GC tests crash, then hang, on the V8 backend

**Files changed (permanent fix):**

- `content/src/webidl/bindings/interface.rs` — the V8 `register_interface_spec`
  constructor path now wraps the platform object in
  `js_engine::v8::V8PlatformData::new` instead of passing the raw `Box`
  (which made `create_object_with_any` fall back to a noop cppgc trace).
- `content/src/webidl/async_iterable.rs` — `create_default_async_iterator_object`
  likewise wraps in `V8PlatformData` on `v8_backend`.

**What was confirmed:**

- At branch HEAD,
  `streams/readable-streams/garbage-collection.any.js` and
  `crashtests/garbage-collection.any.js` SIGSEGV the content process
  (release) / SIGBUS (debug), deterministically. Crash report
  (`~/Library/Logs/DiagnosticReports/formal-web-content-*.ips`):
  `Rc<TracedReference>::drop` on a null/freed Rc pointer inside
  `AbortSignal::begin_abort`, called from testharness's per-test
  `AbortController.abort("Test cleanup")`.
- Root cause of the crash: the Web IDL **constructor** path stored the raw
  platform object, so constructor-created platforms (AbortController,
  DOMException, Event, streams, …) were wrapped in `V8PlatformData::noop` —
  their cells and JS edges were **never traced on the cppgc heap**. A full GC
  swept the AbortSignal state cell while the controller was still alive
  (cell-address instrumentation showed the aborting signal's cell `0x…a70`
  RustObj = `0x…a98` HeapCell was the first swept `AbortSignalState`); the
  controller's Rust-side `Member` then pointed at freed memory → UAF.
- After the fix the two tests no longer crash; they now TIME OUT.
  `streams/piping/close-propagation-backward.any.js` went from TIMEOUT to
  PASS. Full-suite results are unstable across runs
  (`executed=79 unexpected=3` … `4`):
  `enqueue-with-detached-buffer.any.js` FAIL and the two
  `garbage-collection.any.js` files TIMEOUT, with a crash/ERROR flapping to
  a different streams test each run.
- The hang was reduced to a minimal reproduction: a testharness page whose
  only test is `promise_test(async () => { await TestUtils.gc(); await
  Promise.resolve(42); })` never completes. Inspected live via CDP
  (`browser_evaluate` against a debug build): the async test body completes
  in ~2 ms (timestamp probes t0–t3); the harness `result` callback fires but
  the `completion` callback never does; no `#summary` is produced; manual
  `done()` does not help.
- Content logging showed the window `load` event fires after deferred script
  evaluation, but `dispatch load: listeners=0` — the Window's event-listener
  list is empty at dispatch time when a `TestUtils.gc()` ran during script
  evaluation. A control page without a GC dispatches `load` with the
  listener present.
- Likely root cause of the hang (strongly supported, not yet fixed): the
  Window platform is attached to the realm global via
  `associate_existing_object` (raw `Box` in `RealmHostData.associated_objects`),
  and the `RealmHostData` holder itself is created with a raw `Box` →
  `V8PlatformData::noop`. The Window's cells (event-listener list, timer
  registry, …) are therefore never traced; the first full GC sweeps them and
  subsequent operations see missing state — the same bug class as the
  crash, in the `associate_existing_object` path. See
  `scratchpad/v8-safety-review.md` for a broader static-safety review of the
  V8 integration (aliasing in `with_object_any_mut_with`, re-entrant
  `&mut PinScope` in the scope macros, transmute assumptions in
  `create_builtin_fn_with_captures`; not addressed this session).

**What was ruled out:**

- Missing microtask checkpoints: disproved. `evaluate_script` and
  `evaluate_script_to_json` both run a checkpoint after evaluation; a
  4-microtask `Promise.resolve().then…` chain scheduled from one eval is
  fully drained by the next; adding an explicit
  `perform_a_microtask_checkpoint` inside `TestUtils::gc` immediately after
  resolving did not fix the hang; the TestUtils promise resolves to
  `Fulfilled` (probed via `promise_state`).
- testharness watchdog: the `test_state` callback's `{status: 2, message:
  "Test timed out"}` is the test's *initial* STARTED status (set in
  `Test.prototype.step`), not an actual timeout.
- `v8::Weak::is_empty` collection detection: not re-attempted (previous dead
  end documented above).

**Not investigated:**

- Wrapping the Window / `RealmHostData` in `V8PlatformData` (with a
  `RealmHostData::trace` that walks `associated_objects`) — the natural next
  fix for the hang. Whether `Object::wrap` works on the realm global (which
  has no internal fields) is unknown.
- `streams/readable-byte-streams/enqueue-with-detached-buffer.any.js` FAIL
  and the two BYOB byte-stream failures.
- The pre-existing clippy warning backlog in `content/` from the type
  unification (e.g. 17× "useless conversion to the same type: V8Object").

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
