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

### GcCell borrow discipline

Domain code must **never call an engine method (any `ec` operation) while a
`GcCell` borrow guard (`borrow`/`borrow_mut`) is live** — shared or mutable.
The rule is engine-independent: an engine call may allocate, and on the V8
backend an allocation can trigger a cppgc trace that reads the cell while the
borrow is live. A *mutable* borrow being traced is an aliasing violation
(undefined behavior); a *shared* borrow being traced is legal aliasing, but
the rule still forbids it so content code never has to know which engine
operations allocate (and a shared-borrow site can silently become a
mutable-borrow site later). The approved patterns are:

- **Clone out, write back** — `let mut value = cell.borrow(ec).clone();` …
  use the owned value (mutably, across `ec` calls) …
  `cell.set(value, ec);`.
- **Scope the borrow** — hold the guard only for the section that touches
the cell, and drop it before any `ec` call (an explicit `drop(guard)` where
the control flow is not obvious).

The V8 backend enforces the rule as a backstop: `HeapCell::trace` aborts if
marking visits a mutably-borrowed cell (a Rust panic there would unwind
across the C++ marking visitor, so the failure is a hard abort with a log
line), and there is deliberately no `Trace` impl for bare
`std::cell::RefCell` — a `#[gc_struct]` field that needs interior mutability
must use `GcCell`, or be marked `#[ignore_trace]` when it holds no cppgc
edges. The one remaining exception class is the `with_object_any_mut_with`
platform-object closure pattern (it hands `&mut dyn Any` and `ec` to the
operation; see "Remaining work").

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

Latest: `executed=79 unexpected=1`.

The two `garbage-collection.any.js` files (`streams/readable-streams/` and
its `crashtests/` sibling) now PASS — the window-cell sweep hang is fixed
(associated platforms are traced from the realm host data) and
`TestUtils.gc()` defers the collection past the current microtask
checkpoint. `streams/piping/close-propagation-backward.any.js` and the two
BYOB byte-stream failures documented previously (`patched-global.any.js`,
`respond-after-enqueue.any.js`) also PASS in this run.

Remaining unexpected: `streams/readable-byte-streams/enqueue-with-
detached-buffer.any.js` FAIL (`controller.enqueue` after detaching
`byobRequest.view.buffer` does not throw). The BYOB byte-stream failures
have historically appeared and disappeared between runs.

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

All existing tests pass (30 js_engine, 93 generic JS). Forced-collection
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
3. **Wrapper data for Boolean/String/BigInt created by scripts.** The
   `wrapper_primitive` profile slot is extracted at wrap time only for
   Number wrappers (the native `NumberValue` fast path unboxes
   `[[NumberData]]`). Wrappers created by evaluating `new Boolean(false)`,
   `new String(\"x\")`, or `Object(5n)` therefore report `None` from
   `boolean_wrapper_data`/`string_wrapper_data`/`bigint_wrapper_data`;
   the `construct` path coerces Boolean/String arguments (BigInt has no
   [[Construct]]). Extracting the other wrapper slots needs a per-realm
   captured `%Boolean.prototype.valueOf%`-style intrinsic.
4. **Scope-macro `&mut PinScope` reborrow.** The `v8_engine_scope_with_*`
   macros reborrow the callback scope as `&mut` on each nested engine call
   during a native callback. The references never overlap in use (each is
   created, used for a bounded sequence of C calls, and dropped) and the
   underlying memory is C++-owned, but the pattern is not Stacked-Borrows
   clean; a Miri run would flag it.
5. **`with_object_any_mut_with` vs. cppgc tracing.** The operation receives
   `&mut dyn Any` into the platform data AND an execution context; if it
   allocates, a trace pass can read the platform data while the mutable
   borrow is live — the same aliasing hazard the `HeapCell` writer check
   closes for `GcCell`. The `with_object_any_mut` variant is
   compiler-protected (the `&mut` is tied to `&mut ec`, so `ec` cannot be
   used while the borrow is outstanding); the `_with` variant exists for
   operations that need both. Not reproduced as a crash; structurally open.

### ArrayBuffer / IsConstructor gaps (V8)

- `allocate_array_buffer`/`allocate_shared_array_buffer` honor the supplied
  constructor via a full `[[Construct]]` call, but `AllocateArrayBuffer`
  (§25.1.2.1) only reads the constructor's `.prototype`
  (`OrdinaryCreateFromConstructor`) and never runs a subclass constructor
  body. Untriggered today — every caller passes the realm's intrinsic
  `ArrayBuffer` — but a future `SpeciesConstructor`-aware
  `ArrayBuffer.prototype.slice` would run subclass bodies the spec forbids.
- `is_constructor` (the `ObjectProfile` bit cached at wrap time) is a
  heuristic — own `prototype` property with generator/async functions
  excluded — not ECMA-262 `IsConstructor` (§7.2.4). It can go stale after
  `delete Foo.prototype` (false negative) or a `prototype` assignment on an
  arrow function (false positive). `rusty_v8` 150.1.0 exposes no native
  predicate; a JS-side probe would be needed.

### BYOB byte-stream WPT failures (both backends)

`streams/readable-byte-streams/patched-global.any.js` and
`streams/readable-byte-streams/respond-after-enqueue.any.js` fail on both Boa
and V8: the tests read back zeroed bytes instead of the values written via
`byobRequest.respond()`. Not yet investigated.

### 2026-08-04 — WPT stream GC crash/hang on the V8 backend — fixed

**Root cause (two independent bugs):**

1. Platform objects whose data was stored as a raw `Box` (the Web IDL
   constructor path before the previous fix, and `associate_existing_object`
   for the Window) were wrapped in `V8PlatformData::noop`, so their cppgc
   cells and JS edges were never traced. A full GC swept them (crash: the
   AbortSignal state cell; hang: the Window's event-listener/timer cells,
   leaving `dispatch load: listeners=0`).
2. `TestUtils.gc()` ran the collection synchronously, before the queued
   microtask reactions that still referenced realm objects (e.g. a stream's
   start reaction, which sets `started` and performs the first pull); the
   collection reaped the stream machinery while a read was still pending.

**Fixes (in the current tree):**

- `content/src/webidl/bindings/interface.rs` and
  `content/src/webidl/async_iterable.rs` wrap constructor-created platforms
  in `V8PlatformData::new` (previous session).
- `js_engine/src/gc.rs` + `js_engine/src/v8/engine.rs`: the generic
  `associate_existing_object` wraps the platform in `V8PlatformData::new`
  and stores it on the cppgc heap; `RealmHostData` holds a `Member` edge to
  each associated platform and its `Trace` visits them, so the Window's
  cells stay alive for the realm's lifetime.
- `content/src/testutils/mod.rs`: `TestUtils.gc()` enqueues the collection
  as a realm job, so it runs at the next microtask checkpoint (browsers
  defer the spec's "in parallel" steps past the current checkpoint).

**Outcome:** `streams/readable-streams/garbage-collection.any.js` and its
`crashtests/` sibling PASS; default run `executed=79 unexpected=1`.

**Dead end (from the investigation):** an explicit
`perform_a_microtask_checkpoint` inside `TestUtils::gc` does not drain
queued microtasks when gc is called from within a microtask (V8 skips
re-entrant checkpoints) — the collection must be deferred to the next
top-level checkpoint instead.

**Still open:**

- `streams/readable-byte-streams/enqueue-with-detached-buffer.any.js` FAIL
  (`controller.enqueue` after detaching `byobRequest.view.buffer` does not
  throw). The BYOB byte-stream failures have historically appeared and
  disappeared between runs.
- The pre-existing clippy warning backlog in `content/` from the type
  unification (e.g. "useless conversion to the same type: V8Object").

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
- **`species_constructor` (JSC)** — Always returns `default_constructor`.
- **Cross-realm `new.target` (JSC)** — `get_function_realm` always returns current realm.
- **WASM compile/instantiate timeout (JSC)** — Background compilation requires
  the creating thread's run loop to be pumped.
