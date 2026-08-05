# content/src/streams

`content/src/streams` owns the native Streams [platform objects](https://webidl.spec.whatwg.org/#dfn-platform-object) and Streams Standard algorithms used by the content process.

- All stream code operates exclusively on the generic `js_engine` trait API.
  Zero `boa_engine::*` or `boa_gc::*` imports in the entire `streams/` directory.
- Use the local type alias pattern:
  ```rust
  use crate::js::Types;
  type JsValue = <Types as JsTypes>::JsValue;
  type JsObject = <Types as JsTypes>::JsObject;
  type ArrayBuffer = <Types as JsTypes>::ArrayBuffer;
  ```
- Keep Web IDL-visible stream methods on the [platform object](https://webidl.spec.whatwg.org/#dfn-platform-object) types here, and keep `content/src/js/bindings/streams` limited to argument conversion, [inherited interfaces](https://webidl.spec.whatwg.org/#dfn-inherited-interfaces) checks, and delegation.
- Match each [platform object](https://webidl.spec.whatwg.org/#dfn-platform-object) method's return channel to the Web IDL contract: throwing operations use `JsResult`, while promise-returning operations create and settle their promise on the platform object side.
- Prefer typed Rust state for internal slots and related DOM integration, converting back to `JsObject` only at Web IDL boundaries.
- Keep long-lived pipe state, abort handling, and finalization on typed Rust state instead of routing them through JavaScript callbacks.
- Model shared mixins and abstract operations with Rust traits or receiver-owned methods when the spec describes reusable behavior.
- Use the `web_standards` extension (`spec_lookup`) with `https://streams.spec.whatwg.org/` to read the Streams spec, and `vendor/wpt/streams` as the test reference.

## Session investigation log

### 2026-08-02 — readable-byte-streams pull-into data content

**Files changed:** none (observation during a graphics-only refactor's WPT verification; `git diff HEAD` touched only `graphics/`).
**What was confirmed:** `streams/readable-byte-streams/patched-global.any.js` and `respond-after-enqueue.any.js` fail deterministically (identical failure across three runs) with `assert_array_equals: result1.value contents expected property 0 to be 66 but got 0` — a BYOB pull-into descriptor content bug in the content process. All other default-suite tests (including HTML/DOM tests that exercise the full render pipeline) pass.
**What was ruled out:** the graphics refactor (renderer/compositor separation). The failing assertion is deterministic JS data semantics implemented in `content/src/streams` (untouched); the graphics process does not produce byte-stream data.
**Not investigated:** the pull-into descriptor ordering/content bug itself; whether these tests ever passed on this branch (no baseline re-run was performed).
### 2026-08-05 — enqueue-with-detached-buffer does not throw

**Files changed:** none (observation during the graphics-workflow review fixes; `git diff HEAD` touched embedder/graphics/user_agent/verification only).
**What was confirmed:** `streams/readable-byte-streams/enqueue-with-detached-buffer.any.js` fails deterministically (identical failure across two runs, full suite and single-test) with `assert_throws_js: function "() => controller.enqueue(new Uint8Array([42]))" did not throw` — enqueueing after detaching `byobRequest.view.buffer` should throw. Baseline verified: the same single test fails identically with the session's changes stashed (`git stash` + rebuild + single-test run).
**What was ruled out:** the session's changes (embedder Mach-port deallocation, graphics viewport clamp, UA iframe-traversable unregister). The failing assertion is pure byte-stream JS semantics in `content/src/streams` (untouched); none of the changed crates participate in stream data flow.
**Not investigated:** the bug itself — whether enqueue-after-detach enforcement is missing in `content/src/streams`; whether this test ever passed on this branch (baseline was verified against the stashed state of the current working tree, not against a known-good commit).
