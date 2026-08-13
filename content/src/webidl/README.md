# content/src/webidl

`content/src/webidl` implements the algorithms defined in Web IDL §3
(JavaScript binding).  It has two distinct roles:

1. **Domain-facing capabilities** — wrappers around JS operations used by
   other web standards (Streams, HTML, DOM): promise creation, promise
   reaction, type conversion, callback invocation.  These live at the
   `content/src/webidl/` top level (`promise.rs`, `callback.rs`, `buffer_source.rs`).

2. **JS binding infrastructure** — implements the Web IDL §3 algorithms
   for exposing platform objects to JavaScript: interface object creation,
   attribute/operation/constant definition, namespace registration.
   These live in `content/src/webidl/bindings/` and are the generic infra
   that `content/src/js/bindings/` calls into.

**Architecture:**

```
Domain code  →  content/src/webidl/  →  js_engine trait
(Streams,     (promise helpers,       (new_promise_pending,
 HTML, DOM)    callback, buf source)   perform_promise_then,
                                        create_builtin_fn, …)

Bindings      →  content/src/webidl/bindings/  →  js_engine trait
(Window,       (register_interface_spec,         (create_builtin_fn,
 Event,         AttributeDef, OperationDef)        define_property_or_throw,
 ReadableStream)                                   create_object_with_any, …)
```

Every call through this layer ends up at abstract `js_engine` trait methods
(`ExecutionContext<T>`, `JsEngine<T’>`) — no engine-specific APIs leak above.

## Domain-facing capabilities

### Promise manipulation

`promise.rs` implements the Web IDL promise algorithms:

- `https://webidl.spec.whatwg.org/#a-promise-resolved-with` — `resolved_promise()`
- `https://webidl.spec.whatwg.org/#a-promise-rejected-with` — `rejected_promise()`
- `https://webidl.spec.whatwg.org/#js-to-promise` — `promise_from_value()`
- `https://webidl.spec.whatwg.org/#dfn-perform-steps-once-promise-is-settled` — `transform_promise_to_undefined()`
- `https://webidl.spec.whatwg.org/#mark-a-promise-as-handled` — `mark_promise_as_handled()`
- `https://webidl.spec.whatwg.org/#react` — `upon_settlement()`

These are called by domain code in Streams, HTML, and DOM.  Each follows
its spec algorithm with `// Step N:` comments and uses only the
`ExecutionContext<T>` trait — no engine-specific APIs.

`wait_for_all()` and `wait_for_all_get_promise()` are spec-complete but
not yet wired to any domain call site (kept with `#[allow(dead_code)]`).

### Callback invocation

`callback.rs` implements:
- `https://webidl.spec.whatwg.org/#call-a-user-objects-operation` — `call_user_objects_operation()`
- `https://webidl.spec.whatwg.org/#invoke-a-callback-function` — `invoke_callback_function()`
- `https://webidl.spec.whatwg.org/#dfn-callback-interface` — `callback_interface_type_value()`
- `https://webidl.spec.whatwg.org/#dfn-callback-type` — `callback_function_value()`

These are used by DOM event dispatch and other algorithm callbacks.

### Realm access (HTML's direct JS calls)

The HTML spec sometimes reads realm state directly instead of going through
Web IDL — e.g. the `window`/`frames`/`self` getters return "this's relevant
realm.[[GlobalEnv]].[[GlobalThisValue]]"
(<https://html.spec.whatwg.org/#dom-self>).  Such JS-side reads live here
(`realm.rs::relevant_realm_global_this_value`) even though they are not Web
IDL algorithms; webidl hosts "stuff that is used to call into js
indirectly", including HTML's direct-JS-call quirks.  The domain getter
(e.g. `Window::self_value` in `content/src/html/window.rs`) implements the
spec steps and calls this helper; the helper carries the concept anchor
(<https://html.spec.whatwg.org/#concept-relevant-realm>) and a `// Note:`
documenting that the read is the spec's direct-JS-call quirk rather than a
Web IDL conversion.

## JS binding infrastructure (`bindings/`)

`content/src/webidl/bindings/` implements the algorithms from Web IDL §3
JavaScript binding.  It provides generic traits — NOT domain-specific — that
the bindings layer (`content/src/js/bindings/`) calls into.

| Module | Spec section | Purpose |
|---|---|---|
| `interface.rs` | [#js-interfaces](https://webidl.spec.whatwg.org/#js-interfaces) | `WebIdlInterface`, `WebIdlNamespace` traits, `register_interface_spec`, `register_namespace_spec`, `create_interface_instance` |
| `attribute.rs` | [#js-attributes](https://webidl.spec.whatwg.org/#js-attributes) | `AttributeDef`, `define_regular_attributes`, `define_static_attributes` |
| `operation.rs` | [#js-operations](https://webidl.spec.whatwg.org/#js-operations) | `OperationDef`, `define_regular_operations`, `define_static_operations` |
| `constant.rs` | [#js-constants](https://webidl.spec.whatwg.org/#js-constants) | `ConstantDef`, `define_constants` |
| `registry.rs` | — (domain registry) | `InterfaceRegistry`, `register_in_host_defined`, `wire_prototype` |

### Spec compliance: `register_interface_spec`

`register_interface_spec` implements <https://webidl.spec.whatwg.org/#create-an-interface-object>.

**Followed:**
- Step 10: Creates a built-in function with `create_builtin_function(steps, length, id, constructor=true)`
- Step 11: Creates an interface prototype object and defines regular attributes/operations on it
- Step 12: Sets `F.prototype` to the prototype object with `[[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: false`
- Step 13-15: Defines constants, static attributes, and static operations on F
- Step 16: Installs F on the global object (or legacy namespace)

**Gaps:**
| Step | Status |
|---|---|
| Step 3: constructorProto inheritance from parent interface | Wired for registered interfaces via `wire_registry_constructor_prototype` in `build_context.rs`; prototype chain wiring via `wire_registry_prototype` there too. |
| Steps 4-7: `[[Unforgeables]]` slot | Not implemented. Unforgeable attributes/operations are handled by `configurable: false` on the descriptor but not stored on a shared `[[Unforgeables]]` object. |
| Step 1.1-1.7: Overloaded constructor resolution | Not implemented — only single-argument constructors. Overload resolution is deferred. |

### Spec compliance: Attributes

`define_regular_attributes` / `define_static_attributes` / `define_attributes_on_target`
implements the attribute getter/setter creation algorithm from
<https://webidl.spec.whatwg.org/#define-the-attributes>.

**Followed:**
- Property descriptor: `{get: getter, set: setter, enumerable: true, configurable: configurable}`
  where `configurable` is `false` for unforgeable attributes
- Getter/setter are created as built-in functions via `create_builtin_fn`

**Gaps:**
| Step | Status |
|---|---|
| Step 1.1: "If attr is not exposed in realm, then continue" | Not implemented — realm-based exposure checking is deferred. |
| Step 1.8: Observable array type | Not implemented — observable array types are not yet supported. |
| Attribute getter ([[LegacyLenientThis]] handling) | Delegated to the user-provided getter function rather than auto-generated by the binding infra. The `legacy_lenient_this` field exists on `AttributeDef` but is not used by the infra. |

### Spec compliance: Operations

`define_regular_operations` / `define_static_operations` / `define_operations_on_target`
implements the operation function creation algorithm from
<https://webidl.spec.whatwg.org/#define-the-operations>.

**Followed:**
- Property descriptor: `{value: method, writable: modifiable, enumerable: true, configurable: modifiable}`
  where `modifiable` is `false` for unforgeable operations
- Method is created as a built-in function via `create_builtin_fn` with the correct `length`

**Gaps:**
| Step | Status |
|---|---|
| Step 1.1: "If op is not exposed in realm, then continue" | Not implemented — realm-based exposure checking is deferred. |
| Steps 2.1.1-2.1.5: `this`-value normalization, security check, overload resolution | Delegated to the user-provided method function. The spec algorithm for "creating an operation function" that wraps `this`-checking and security checks is not auto-generated. |

### Spec compliance: Namespace objects

`register_namespace_spec` implements
<https://webidl.spec.whatwg.org/#create-a-namespace-object>.

**Followed:** Creates a plain object with `%Object.prototype%`, defines regular
attributes and operations on it, installs as a property on the global object.

**Gaps:** Simple creation only — no namespace prototype handling or extended
attribute support (e.g. `[Exposed]`).

## Design decisions

### `this`-value checking is manual

The Web IDL spec defines attribute getter/setter and operation function
creation algorithms that wrap `this`-value normalization and security
checks around the user-provided steps.  Our binding infra delegates this
to the user-provided function pointer (e.g., `try_with_html_iframe_element_ref`
in the binding functions).  This is a deliberate simplification: the
binding infra would need to know the interface type to generate the
`this`-checking code, which would require type-level dispatch or macros.

The check looks like:
```rust
let obj = T::value_as_object(this).ok_or_else(|| ec.new_type_error("..."))?;
if let Some(data) = ec.with_object_any(&obj) {
    if let Some(domain_obj) = data.downcast_ref::<MyInterface>() {
        return Ok(/* ... */);
    }
}
Err(ec.new_type_error("receiver is not a MyInterface"))
```

## [Platform objects](https://webidl.spec.whatwg.org/#dfn-platform-object) across backends

The content crate defines Rust types that correspond to Web IDL interface types (e.g.
`Window`, `Document`, `HTMLAnchorElement`). In comments and documentation, refer to these
as a [platform object](https://webidl.spec.whatwg.org/#dfn-platform-object) that implements
the *named interface* — for example:
- "a [platform object](https://webidl.spec.whatwg.org/#dfn-platform-object) that implements
  the [Document](https://dom.spec.whatwg.org/#interface-document) interface"
- "the [Window](https://html.spec.whatwg.org/#window) [platform object](https://webidl.spec.whatwg.org/#dfn-platform-object)"

The Rust `downcast_ref` operation checks which interface a `JsObject`'s backing data
implements — this maps to the Web IDL concept of
[inherited interfaces](https://webidl.spec.whatwg.org/#dfn-inherited-interfaces).
Prefer phrasing like "check the platform object's inherited interfaces" over
"downcast the platform object".

Platform object types are `#[gc_struct]` Rust structs; the macro derives the
active backend's GC traits (`Trace`/`Finalize`/`GarbageCollected` on the
default V8 backend, `boa_gc::Trace`/`Finalize`/`boa_engine::JsData` on Boa).
The engine stores the struct on its managed heap and links it to the JS
wrapper: on V8 through `create_object_with_any` (a `V8PlatformData` object on
the cppgc heap, traced from the wrapper) or `associate_existing_object` (for
the realm global), on Boa inside the `JsObject` via `from_proto_and_data()` or
`ObjectInitializer::with_native_data_and_proto()`. Domain code reaches the
struct through the generic `with_object_any` / `with_object_any_mut` /
`with_object_any_mut_with` accessors.

The typical pattern for a platform object:

```rust
#[gc_struct]
pub struct MyInterface {
    /// Rust backing state — not JS-visible properties.
    pub inner: GcCell<InnerState>,
}
```

The JS-visible properties and methods are registered separately via the Web
IDL bindings (`WebIdlInterface`); the Rust struct holds only the backing state.

### Where [platform object](https://webidl.spec.whatwg.org/#dfn-platform-object) types live

- **DOM interfaces** (`Document`, `EventTarget`, `Element`, …): `content/src/dom/`
- **HTML interfaces** (`Window`, `HTMLAnchorElement`, `HTMLIFrameElement`, `Location`, …): `content/src/html/`
- **Streams interfaces** (`ReadableStream`, `WritableStream`, …): `content/src/streams/`
- **WebAssembly domains** (`WasmModule`, compilation worker, …): `content/src/wasm/`

### Three-layer architecture

Every Web-exposed feature follows a three-layer split:

1. **Domain** (`content/src/<domain>/`) — Rust struct + spec-algorithm methods returning Rust types.
2. **Web IDL bindings infra** (`content/src/webidl/bindings/`) — generic traits
   (`WebIdlInterface`, `WebIdlNamespace`, `OperationDef`, etc.). Not domain-specific.
3. **JS bindings glue** (`content/src/js/bindings/<domain>/`) — `WebIdlInterface` impl,
   thin function pointers that downcast, call domain methods, wrap in `JsValue`.

See `content/src/js/bindings/README.md` for the definitive description.

**What belongs where:**

| What | Where |
|---|---|
| Rust struct definition (`WasmModule`), `#[gc_struct]` derive | `content/src/<domain>/types.rs` |
| Spec-algorithm methods returning Rust types (`export_descriptors() → Vec<…>`) | `content/src/<domain>/functions.rs` — `impl WasmModule` |
| `WebIdlInterface` impl (`define_members`, `create_platform_object`) | `content/src/js/bindings/<domain>/` |
| Thin JsValue-wrapping function pointers (`fn(this, args, ctx) → JsResult<JsValue>`) | `content/src/js/bindings/<domain>/` |
| `WebIdlInterface` trait, `register_interface_spec`, `OperationDef`, `AttributeDef` | `content/src/webidl/bindings/` (generic — no domain logic) |

**Never add domain-specific code to `content/src/webidl/bindings/`.**
Use the trait methods (`legacy_namespace()`, `constructor_length()`) to
customize behaviour.  Never add an `impl WebIdlInterface` outside of
`content/src/js/bindings/`.

### Exotic objects and custom internal methods

Some Web/HTML spec objects (e.g. `WindowProxy`, `Location`) require exotic internal
methods — they override `[[Get]]`, `[[Set]]`, `[[GetPrototypeOf]]`, etc. rather than
using the ordinary object behaviour.

The generic `ExecutionContext::create_proxy(target, handler)` builds these as
proxies on every backend — content only ever calls the generic trait method.
See `content/src/html/windowproxy.rs` for the WindowProxy pattern: each trap
is a built-in function set on the handler object.

On the **Boa backend** the proxy is created through the `%Proxy%` constructor
(`JsProxyBuilder`, which supplies each trap as a plain `NativeFunctionPointer`);
Boa also exposes exotic objects through `InternalObjectMethods` (a vtable
stored on every `JsObject`):

1. Define a Rust type implementing `JsData` by deriving `#[derive(Trace, Finalize)]`
   and implementing `JsData` manually.
2. Override `JsData::internal_methods()` to return a `static InternalObjectMethods`
   with the custom function pointers:

```rust
#[derive(Trace, Finalize)]
pub struct MyExotic { ... }

impl JsData for MyExotic {
    fn internal_methods(&self) -> &'static InternalObjectMethods {
        static METHODS: InternalObjectMethods = InternalObjectMethods {
            __get__: my_exotic_get,
            __set__: my_exotic_set,
            __delete__: my_exotic_delete,
            ..ORDINARY_INTERNAL_METHODS
        };
        &METHODS
    }
}
```

3. Inside each function, use `obj.downcast_ref::<MyExotic>()` to access the data.
4. Delegate to the inner object using the **public** `JsObject` methods
   (`get()`, `set()`, `prototype()`, `own_property_keys()`, etc.).
   See `content/src/js/README.md` for the full methodology.

**Rejected approach:** Modifying the external engine dependency to make internal
APIs public. All exotic-object implementations must use only public engine APIs.

**Note:** `#[derive(JsData)]` cannot be used when manually overriding
`internal_methods()` because the derive macro generates a conflicting
implementation. Use `#[derive(Trace, Finalize)]` and implement `JsData` by hand.

**Visibility note:** When implementing exotic objects, **do not modify**
the external engine dependency to make internal APIs public. Instead, use only
what the engine already exposes publicly. See `content/src/js/README.md`
("Working with the engine's public API: use spec links, not `pub(crate)`
internals") for the correct methodology.

### The ObjectInitializer pattern (Boa backend)

For Boa platform objects that don't need exotic behaviour and just need a prototype chain:

```rust
let object = ObjectInitializer::with_native_data_and_proto(
    MyInterface::new(...),
    prototype,  // e.g. context.intrinsics().constructors().my_interface().prototype()
    context,
)
.property("someProp", js_string!("value"), Attribute::all())
.build();
```

See `content/src/js/bindings/` for concrete examples per interface.

## Buffer source types

<https://webidl.spec.whatwg.org/#js-buffer-source-types>

The Web IDL buffer source types (`ArrayBuffer`, `ArrayBufferView`, `BufferSource`)
have specific conversion algorithms implemented in `buffer_source.rs`.

| Function | Spec algorithm | Purpose |
|---|---|---|
| `get_a_copy_of_the_buffer_source` | [#dfn-get-buffer-source-copy](https://webidl.spec.whatwg.org/#dfn-get-buffer-source-copy) | Extract bytes from an `ArrayBuffer` or typed array |
| `is_buffer_source` | [#dfn-buffer-source-type](https://webidl.spec.whatwg.org/#dfn-buffer-source-type) | Check whether a JS value is a buffer source type |

The `get_a_copy_of_the_buffer_source` function is called by the bindings layer (e.g.
`content/src/js/bindings/wasm/mod.rs`) to convert JS values into Rust `Vec<u8>`
before passing them to domain functions.  Domain functions receive clean Rust types,
never raw `JsValue`.

`SharedArrayBuffer` values do not match `object_as_array_buffer` on the generic
`JsTypes`, so buffer sources reject them (the `[AllowShared]` constraint).

## Related documentation

- `content/README.md` — Content-crate overview
- `content/src/js/README.md` — JS integration specifics (engine context ownership, bindings)
- `content/src/html/README.md` — HTML platform objects, WindowProxy, navigation split
