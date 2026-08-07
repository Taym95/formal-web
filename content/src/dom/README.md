# content/src/dom

`content/src/dom` stores the native [platform objects](https://webidl.spec.whatwg.org/#dfn-platform-object) for the JavaScript-visible DOM interfaces and the DOM Standard algorithms that operate on them.

- `BaseDocument` remains the authoritative DOM tree and document state.
- `Document` and `Element` compose `Node`, so shared tree algorithms live on `Node` while type-specific Web IDL behavior stays on the owning [platform object](https://webidl.spec.whatwg.org/#dfn-platform-object).
- HTML-owned global-object [platform objects](https://webidl.spec.whatwg.org/#dfn-platform-object) such as `GlobalScope` (implementing the [global object](https://html.spec.whatwg.org/#global-object) concept) and [Window](https://html.spec.whatwg.org/#window) live in `content/src/html`, and DOM dispatch code here depends on them when the DOM Standard talks about window-backed targets.
- `content/src/js/bindings` should delegate DOM algorithms here instead of embedding DOM logic in the binding layer.
- Native UI-event to DOM-dispatch bridging lives in `content/src/html/ui_events.rs`; activation-target selection stays in `dispatch.rs`.
- Use the `web_standards` extension (`spec_lookup`) with `https://dom.spec.whatwg.org/` to read the DOM spec, and for single-sentence spec definitions quote the defining sentence instead of inventing `Step N:` comments.

## Event dispatch architecture

The dispatch algorithm (implemented in `dispatch.rs`) operates on two domain types: `Event` and `EventTarget` — both `#[gc_struct]` platform objects. No JsObject appears in the dispatch algorithm itself. The JsObject GC handle is only accessed at the Web IDL boundary (`call_user_objects_operation` in `inner_invoke`), via the `reflector` field on `EventTarget` and `Event`.

### EventTargetAccess trait

Types that embed an `EventTarget` (Node, Window, AbortSignal, Element, etc.)
implement `EventTargetAccess`:

```rust
pub(crate) trait EventTargetAccess {
    fn get_event_target(&self) -> EventTarget;                   // clone of embedded EventTarget
    fn get_the_parent(&self) -> Option<EventTarget>;             // parent EventTarget for path building
}
```

Dispatch functions (`fire_event`, `dispatch_with_path`) take `EventTarget`
values and pre-built `EventPathItem` paths — no JsObject is involved in path
building or the dispatch loop.

### Interior mutability via GcCell

All mutable fields on `Event` and `EventTarget` use `GcCell` instead of `&mut self` methods:

```rust
pub struct EventTarget {
    pub(crate) event_listener_list: GcCell<Vec<EventListener>>,
    next_listener_id: Cell<u64>,  // Cell (not GcCell) since u64 has no GC pointers
}
```

Methods take `&self` and use `borrow()`/`borrow_mut()`.  Since `GcCell` shares its underlying data through a `Gc` pointer, cloning an EventTarget and mutating the clone affects the original — no sync-back needed.

### Event stores domain EventTarget, not JsObject

`Event.target` and `Event.currentTarget` store `GcCell<Option<EventTarget>>` — domain types, not JsObject handles. The JsObject is only resolved at the binding layer via the EventTarget's `reflector` field.

### Path entries use domain types only

`EventPathItem` stores:
- `invocation_target: EventTarget` — the current target in the propagation path
- `shadow_adjusted_target: Option<EventTarget>` — the shadow-adjusted target for `event.target`
- `has_activation_behavior: bool` — whether the item is an anchor with an href
  (activation-target selection per dispatch step 6.9.6.1)

No JsObject appears in path items.

### Entry points

| Function | Takes | Creates path? | Used by |
|---|---|---|---|
| `fire_event` | `ec`, `&dyn EventTargetAccess`, event_type, time, flags | Yes (via simple_path) | Domain callers (main.rs, html_iframe_element, abort) |
| `dispatch_event` | `ec`, `&[EventPathItem]`, `&Event` | No (pre-built path) | EventTarget.dispatchEvent, activation behavior |
| `dispatch_with_path` | `ec`, `&[EventPathItem]`, event_object | No (pre-built path) | UI-event pipeline (html/ui_events.rs) |
| `simple_path` | `&dyn EventTargetAccess` | Yes (single entry) | html/dispatch.rs (fire_global_event) |

### Path building for UI events (content/src/html/ui_events.rs)

`build_event_path()` resolves a blitz node chain into `Vec<EventPathItem>` by
extracting the `EventTarget` from each node's platform object. The chain is the
event target followed by its ancestors (DOM dispatch steps 6.3-6.9); the
document and Window event targets close the path. The trusted-click builder
`build_path_from_target_js_object` (content/src/js/platform_objects.rs) walks
the same parent chain from a JS target object. Both builders mark
`has_activation_behavior` on anchor-with-href items so dispatch step 12 can
run the anchor's activation behavior.

### EnvironmentSettingsObject owns a Document platform object

`EnvironmentSettingsObject.document` is `crate::dom::Document` (the platform
object). The blitz BaseDocument is accessible via `document.node.document`.
This provides direct access to the Document's `EventTarget` for dispatch
path building without going through a JsObject.

### What NOT to do

- Do not add JsObject parameters to dispatch functions — domain dispatch operates on EventTarget only.
- Do not use `&mut self` on EventTarget methods — use `GcCell` fields with `&self`.
- Do not clone Event or EventTarget and sync back — `GcCell` shares data across clones.
- Do not put JsObject-only helpers (like `event_target_from_object`) in dispatch.rs — keep domain dispatch pure. Such helpers belong in `platform_objects.rs` or `html/ui_events.rs`.

## GcCell interior mutability pattern elsewhere

Types that currently use `&mut self` for mutation but could use `GcCell` + `&self`:

- **Node** — `child_node_ids` is read-only; other mutating methods could use GcCell.
- **Window** — `setTimeout` callbacks stored behind GcCell.
- **Streams** — writable/readable stream state machines use `Cell<bool>`/`RefCell`; some could use GcCell for GC-traced callback fields.
- **AbortSignal** — already uses `GcCell<AbortSignalState>` at the top level; internal fields like `onabort` could move to GcCell if needed.

