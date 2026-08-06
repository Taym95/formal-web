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

## Known failures

### readable-byte-streams pull-into data content

`streams/readable-byte-streams/patched-global.any.js` and
`respond-after-enqueue.any.js` fail deterministically (identical failure across
three runs) with `assert_array_equals: result1.value contents expected property
0 to be 66 but got 0` — a BYOB pull-into descriptor content bug. All other
default-suite tests pass. Ruled out: renderer/compositor separation (the
failing assertion is pure byte-stream JS data semantics in `content/src/streams`;
the graphics process does not produce byte-stream data). The pull-into
descriptor bug itself is not yet fixed.

### enqueue-with-detached-buffer does not throw

`streams/readable-byte-streams/enqueue-with-detached-buffer.any.js` fails
deterministically (identical failure across two runs, full suite and
single-test) with `assert_throws_js: function "() =>
controller.enqueue(new Uint8Array([42]))" did not throw` — enqueueing after
detaching `byobRequest.view.buffer` should throw. Baseline verified: the same
single test fails identically with unrelated working-tree changes stashed.
Whether enqueue-after-detach enforcement is missing in `content/src/streams`
is not yet investigated.
