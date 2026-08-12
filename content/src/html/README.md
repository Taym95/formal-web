# content/src/html

`content/src/html` owns HTML parser integration, document lifecycle work, navigation helpers, and HTML global-object [platform objects](https://webidl.spec.whatwg.org/#dfn-platform-object) such as `Window` and `GlobalScope`.

- Keep DOM-tree entry points under `content/src/html/html_dom_tree.rs`, and route per-element hooks from there into element modules.
- Keep iframe bindings and iframe processing algorithms together in `content/src/html/html_iframe_element.rs` as free functions over content-process state (`ContentProcess`).
- Keep helper names aligned with the corresponding HTML algorithm anchors, and prefer explicit error returns or `debug_assert!` plus safe early returns over sentinel ids.
- Trigger parser-discovered iframe work from document-load parsing completion.
- Use the `web_standards` extension (`spec_lookup`) with `https://html.spec.whatwg.org/` to read the HTML spec.

## Structured clone (`safe_passing_of_structured_data.rs`)

### String round-tripping — use UTF-16 units, never a display-escaped string

Strings are serialized as raw UTF-16 code units. Any display/escaping
conversion (one that replaces unpaired surrogates with literal `\uXXXX`
escape sequences) corrupts strings like lone surrogates (`\uD800`, `\uDC00`).

**Correct serialization:**
```rust
let utf16_units: Vec<u16> = ec.js_string_to_rust_string(&s).encode_utf16().collect();
```

**Correct deserialization:**
```rust
let js_string = ec.js_string_from_str(&String::from_utf16_lossy(&utf16_units[..]));
```

### RegExp source — `[[OriginalSource]]` vs the escaped getter

The `source` accessor on RegExp applies `EscapeRegExpPattern` (spec 22.2.3.2.5),
which escapes `/`, `\n`, `\r`, `\u2028`, and `\u2029`. Passing the escaped form
back to the RegExp constructor produces a different pattern. Always store the
raw `[[OriginalSource]]`: read `ec.get_regexp_source` and reverse the escaping
with `unescape_regexp_source()`.

### Error "message" — `[[GetOwnProperty]]`, not `[[Get]]`

The spec step for Error serialization (step 17.4) uses `[[GetOwnProperty]]` for
the "message" property — this checks only own data descriptors, ignores the
prototype chain, and does not invoke accessors. Using `EcmascriptHost::get`
(which is `[[Get]]`) is wrong. Use `ec.get_own_property` and read the value
from the data descriptor:
```rust
let msg_key = ec.property_key_from_str("message");
let msg_desc = ec.get_own_property(object.clone(), msg_key)?;
let message: Option<String> = match msg_desc {
    Some(ref desc) if desc.value.is_some() => desc
        .value
        .clone()
        .map(|v| ec.to_rust_string(v))
        .transpose()?,
    _ => None,
};
```

### EnumerableOwnProperties — filter by enumerability

The spec uses `EnumerableOwnProperties(value, "key")`, which returns only
enumerable own property keys. `ec.own_property_keys` returns ALL own keys
(including non-enumerable ones like `length` on arrays). Always check
enumerability through `ec.get_own_property`:
```rust
let keys = ec.own_property_keys(object.clone())?;
// ...for each key:
let desc = ec.get_own_property(object.clone(), key.clone())?;
let enumerable = desc.as_ref().and_then(|d| d.enumerable).unwrap_or(false);
```

### Wrapper objects — Boolean/Number/String/BigInt

When serializing, check for `[[BooleanData]]` / `[[NumberData]]` / etc.
internal slots (steps 7–10). When deserializing, create wrapper *objects*
with the correct prototype (steps 6–9), not primitive values — construct
through the realm's intrinsic constructor:
```rust
let num_val = ec.value_from_number(*n);
let obj = ec.construct(intrinsics.number.clone(), &[num_val], None)?;
value = Types::value_from_object(obj);
```

### Error cause — serialize custom data

The spec says "User agents should attach a serialized representation of any
interesting accompanying data." The `cause` property (ES2022) was added as
an optional `Box<SerializedRecord>` to the `Error` variant.

## Algorithm split: content process vs user agent

Many HTML algorithms (navigation, window.open, iframe creation) span both the
content process (which runs JS and owns DOM state) and the user agent (which
owns the navigable tree, browsing contexts, and event-loop dispatch). The
split is:

| Side | Owns | Runs |
|------|------|------|
| **Content** | Document, Window, JS `Context`, `GlobalScope` | Document-owning algorithm steps: URL parsing, feature tokenization, noopener computation, rules-for-choosing-a-navigable (local subset), document creation |
| **User agent** | Navigable tree, browsing contexts, browsing context groups, agents, event loops, session history | Navigable-owning algorithm steps: find-by-target-name (cross-process), new-traversable creation (non-window.open), opener tracking, beforeunload, navigation fetching |

When an algorithm crosses this boundary, the side that hits its limit sends an
IPC message and the other side continues. The IPC ordering guarantee (per
content process, messages arrive in order) makes this safe.

### Document creation: two directions

Documents can be created either by the user agent (for startup, iframes, UA-originated
`_blank` navigations) or by content (for `window.open`). These are inverses:

**UA→Content** (`create_new_top_level_traversable` in `user_agent/src/user_agent.rs`):
1. UA allocates IDs (traversable, document, browsing context, agent)
2. UA sends `CreateEmptyDocument` IPC to content's event loop
3. Content creates the about:blank document, Window, and JS Context
4. UA registers the navigable in its state

**Content→UA** (`window_open_steps` in `window.rs`):
1. Content creates the about:blank document, Window, and JS Context locally
2. Content sends `NavigateRequest` with `new_traversable_info`
3. UA calls `create_new_top_level_traversable_from_content` (UA-side inverse of step 1)
4. UA registers the navigable, browsing context, agent, event loop WITHOUT
   sending `CreateEmptyDocument` back (content already did it)

Both paths converge to the same final state.

## Posting messages (`window_post_message_steps`)

Implements <https://html.spec.whatwg.org/#window-post-message-steps>, split
across all three sides:

| Side | Runs |
|------|------|
| **Source content** | Steps 1–7: resolve the incumbent origin and the target navigable, process `targetOrigin`, run `StructuredSerializeWithTransfer` (`window.rs:window_post_message_steps`) |
| **User agent** | Step 8: queue a global task on the posted message task source given targetWindow by routing `ContentCommand::PostMessage` to the target navigable's event loop (`user_agent.rs:handle_post_message`), even when the target window lives in the same event loop |
| **Target content** | Substeps 8.1–8.7: origin check, deserialize with the target realm, fire `message`/`messageerror` via `MessageEvent` (`main.rs:dispatch_post_message`) |

The wire payload is `PostMessageRequest`
(`ipc_messages::safe_passing_of_structured_data`): the serialized record, the
transfer data holders, the processed target origin, and the source navigable +
origin.  The serialized record and holders are pure data so the same payload
crosses content→UA and UA→content.

Transfer identity: `StructuredSerializeWithTransfer` places
`SerializedRecord::TransferredValue(index)` records in the serialized graph in
place of each transferable; the holders list at `index` carries the data, and
`StructuredDeserializeWithTransfer` rebuilds the values and resolves the
records by index.  Record identity cannot cross an IPC boundary, which is why
the implementation does not follow the spec's shared-record identity model.

The source Window (step 8.3) is resolved in the target process from the
source navigable id; a cross-process source currently leaves `source` null.

## The rules for choosing a navigable (`the_rules_for_choosing_a_navigable`)

Implements <https://html.spec.whatwg.org/#the-rules-for-choosing-a-navigable>.
Split between content and user agent:

### Content side (`html.rs:the_rules_for_choosing_a_navigable`)
| Step | What content does |
|------|-------------------|
| 1 | Let chosen = null |
| 3 | `_self` / empty → currentNavigable (Resolved) |
| 4 | `_parent` → parent (or current) (Resolved) |
| 5 | `_top` → traversable (Resolved) |
| 6 | Named target, not `_blank`, not noopener → cross-process lookup needed (NeedsUserAgentAction) |
| 7 | Otherwise → new top-level traversable (NeedsUserAgentAction) |

### User agent side (`user_agent.rs:the_rules_for_choosing_a_navigable`)
Continues when the content process returned `NeedsUserAgentAction`:
| Step | What UA does |
|------|-------------|
| 7 cont. | `find_navigable_by_target_name` across the global navigable registry |
| 8 | If still null: `create_new_top_level_traversable` (UA→Content path) |

## Window.open (`window_open_steps`)

Implements <https://html.spec.whatwg.org/#window-open-steps>.

### Steps 1–12 (content only)
URL parsing, target normalization, feature tokenization, noopener/referrerPolicy
computation. All local to the source document.

### Step 13 — apply the rules for choosing a navigable
Content runs `the_rules_for_choosing_a_navigable` (local subset) to resolve `_self`, `_parent`,
`_top`. For `_blank`, named targets, and noopener, it returns `NeedsUserAgentAction`.

### Step 14 — handle the chosen navigable
- **Resolved(id) where id == source:** Same-navigable. Return current window proxy.
- **Resolved(id) where id != source:** `_parent`/`_top`. Send `chosen_navigable_id`
  in the `NavigateRequest`. The UA navigates the correct navigable. The returned
  WindowProxy is the current global (wrong if parent/top is a different navigable —
  needs IPC resolution, tracked as a gap).
- **NeedsUserAgentAction:** Create an about:blank document locally via
  `GlobalScope::create_auxiliary_context_document`. This gives us a Window to back the
  WindowProxy immediately. Send `NavigateRequest` with `new_traversable_info`.

### Steps 15–17 (UA side)
- UA calls `create_new_top_level_traversable_from_content` to sync navigable state
- UA calls `setup_opener_for_window_open` for new-auxiliary tracking
- UA creates webview for the new top-level traversable
- UA starts navigation (fetch the destination URL)
- noopener → return null

### Step 18 — return WindowProxy
Return the target navigable's active Window's JsObject. For same-origin the
WindowProxy is transparent.

### Document creation for new traversables (the inverted split)

```
Content (window_open_steps):             UA (handle_navigate):
  |                                        |
  |-- create about:blank document          |
  |   (GlobalScope::create_auxiliary_context_document)        |
  |-- NavigateRequest {                    |
  |     new_traversable_info: Some(...),   |
  |     chosen_navigable_id: Some(id)      |
  |   }                                    |
  |                                        |
  |========================= IPC =========>|
  |                                        |
  |                                        |-- create_new_top_level_traversable_from_content
  |                                        |     (navigable, BCG, agent, 
  |                                        |      doc state, event-loop reg)
  |                                        |-- setup_opener_for_window_open
  |                                        |-- create_webview_for_new_top_level
  |                                        |-- handle navigation (fetch URL)
```

`GlobalScope::create_auxiliary_context_document` creates the about:blank document,
JS Context, and Window directly on the GlobalScope (no callback indirection). The
method returns the Window's global object which backs the WindowProxy.

The UA's `create_new_top_level_traversable_from_content` is the inverse of
`create_new_top_level_traversable`: it sets up only UA-side state (navigable,
browsing context group, agent, event-loop registration) and does NOT send
`CreateEmptyDocument` back to content.

### Opener tracking for auxiliary browsing contexts

<https://html.spec.whatwg.org/#creating-a-new-auxiliary-browsing-context>

When `window.open` creates a new navigable and noopener is false, the UA sets
up the opener relationship via `setup_opener_for_window_open`. This corresponds
to the spec's "create a new auxiliary browsing context" which:
1. Creates a new top-level traversable with the source navigable's browsing
   context as opener
2. Sets the opener browsing context on the new browsing context

The content process does not track opener relationships — those are purely
UA-side state. The opener is only used for:
- Navigation policy (e.g., `target=_blank` with `rel=opener`)
- `window.opener` JS property (not yet implemented)
- Popup blocking

## WindowProxy (`windowproxy.rs`)

<https://html.spec.whatwg.org/#the-windowproxy-exotic-object>

### Current implementation: one WindowProxy mechanism

There is a single WindowProxy mechanism.  The identity handed to JavaScript
is an ECMAScript Proxy whose target is a [`WindowProxy`](windowproxy.rs)
shim platform object tied to the navigable (one per (realm, navigable),
cached on the realm's GlobalScope), so `event.source === iframe.contentWindow`
holds.  The shim's `local_window` field carries the navigable's active Window
when it lives in this content process (same agent cluster); the proxy traps
then delegate property access to that Window — the local behavior
(`window.open` results, `iframe.contentWindow`, and the message event's
`source` all resolve property gets/sets against the local Window, e.g.
`w.location = url` reaches the target's Location binding).  When the
navigable's document was created in another content process, `local_window`
is `None` and the traps resolve the shim's cross-origin member set instead
— `postMessage` runs in the caller's realm (steps 1–7 locally, user-agent
routing for step 8), and the remaining members are the shim's fixed set.

Cross-realm property access in V8 is gated by the context security token;
the engine installs a shared token on every context so same-origin windows
can reach each other's globals, and native callbacks run in their creation
realm (the callback machinery switches the engine's realm state), so
invoking the target window's methods through the proxy — `w.location = url`
via the `[PutForwards=href]` Location attribute, `w.open(...)`, timers, and
gets/sets on the target's globals — runs in the target realm and works.

The `local_window` handle is a rooted (non-traced) `JsObject`, not a
`GcCell` edge: the WindowProxy's backing must stay usable across the
navigation-commit garbage collection that runs when the old document is
destroyed, and a cppgc-traced edge held in a cell is not reliably usable
after a full collection on the V8 backend (the materialized handle can
point at a swept object — reproducible in `js_engine`'s own
`associated_platform_cells_survive_forced_gc` when the cell value is read
back and used after the forced gc).  The root keeps the window alive for
exactly as long as the proxy references it, and the navigation-commit
severing clears it (releasing the root) once the navigable's document is
created in another content process.

### Lifecycle: navigation commit is the proxy transition

Per the spec, a browsing context (navigable) has **one** WindowProxy
identity, and *"when the browsing context is navigated, the Window object
wrapped by the browsing context's associated WindowProxy object is
changed"* (§7.2.3).  The shim's `local_window` field is exactly that
wrapped Window, and navigation commit updates it:

1. **Local backing** — while the navigable's active document is in this
   content process (same agent cluster), the proxy is backed by that
   document's Window: `local_window` is set and the traps delegate property
   access to it.
2. **Navigation commit (the old document unloads)** — when the old
   document is destroyed in this process (`destroy_document`), every cached
   WindowProxy for that navigable is updated:
   - if a new document for the navigable is already active in this process
     (same-process navigation), the backing is re-pointed at the new
     document's Window (cross-realm proxy — §7.5.1 step 6 reuses the
     initial about:blank Window for same-origin navigations);
   - if the navigable's document was created in another content process
     (cross-origin navigation), `local_window` is cleared and the proxy
     becomes a remote shim (cross-process forwarding via the user agent)
     while keeping its identity.

The user agent routes `DestroyDocument` to the event loop that owns the
document (`command_sender_for_document`), not the traversable's current
event loop: after a cross-process navigation the traversable has moved to a
new event loop, and routing by traversable would send the destroy to the
wrong content process — leaving the old document (and the WindowProxy's
local backing) alive in the old process forever.

Note: the user agent keeps a traversable whose active document is initial
about:blank on the opener's event loop for its first URL navigation
(`initialise_the_document_object`), so the first navigation of a
`window.open`'d popup stays in the same process and re-points the backing;
the window is created in another content process (and the proxy becomes a
remote shim) on later cross-origin navigations and for child navigables
whose parent document is cross-origin.

### Agents, processes, and realms

The taxonomy comes from the spec's agent model, not from our process
layout.  Windows are placed into agents by <https://html.spec.whatwg.org/#obtain-a-similar-origin-window-agent>
(defined in §8.1.2.2, used at §7.3.2.1 "creating browsing contexts" step 9
and §7.5.1 "shared document creation infrastructure" step 7.4):

- **Agent cluster** — the spec's idealized "process boundary" (§8.1.2.2:
  *"the agent cluster concept is an architecture-independent, idealized
  process boundary"*).  An agent cluster holds one similar-origin window
  agent.  Windows in the same cluster: a Window and a same-origin-domain
  iframe it created, and a Window and a same-origin-domain Window that
  opened it (opener/opened).  Windows with no opener or ancestor
  relationship are in **different** clusters *even when same-origin*.
- **Similar-origin window agent** — the spec unit our content process is
  the concrete realization of: one content process hosts one agent cluster
  with one similar-origin window agent (`AgentCluster.similar_origin_window_agent`
  and `AgentClusterKey` in `user_agent/src/user_agent.rs`).  Same-cluster
  windows are same-process; different-cluster windows are cross-process.
  In particular, cross-origin windows are always cross-process, and an
  auxiliary browsing context (`window.open`) is same-origin-domain-related
  to its opener, so it shares the opener's cluster and process.
- **Realm** (V8 context) — an engine detail, orthogonal to the agent
  model: every Window is its own realm even within the same agent, and V8
  gates property access on another context's global object on
  security-token equality.  The engine installs a shared security token on
  every context, so same-cluster (same-process) property gets/sets through
  the WindowProxy resolve against the local Window.  Native callbacks run
  in their creation realm (the callback machinery switches the engine's
  realm state), so method calls through the proxy (e.g. `w.location = url`
  reaching the Location binding's navigation) run in the target realm and
  work.

What a WindowProxy access involves therefore splits as:

- **Same cluster (same process)**: the proxy resolves the target window's
  realm locally (property gets/sets work with the shared security token;
  method invocation runs the target realm's native bindings).
- **Cross cluster (cross process)**: the proxy's `local_window` is `None`
  and the traps resolve the shim's fixed member set; `postMessage` routes
  through the user agent (the remaining members need selective-access
  forwarding, gap 2 below).

### Remaining gaps

**1. Cross-cluster selective access is not wired.**  When the target
navigable lives in another agent cluster (another content process), the
shim must give selective access to the remote window: `postMessage` already
routes through the user agent (see "Posting messages" above), and the
remaining members (`document`, `location`, `name`, …) must be forwarded to
the target process the same way.  The shim's business-logic design is what
makes this possible — the proxy is just a navigable id plus a forwarding
policy, so it can hand any member off to the user agent.

**2. The shim exposes a fixed member set.**  The shim (cross-cluster
WindowProxy) exposes the Window members the current features need rather
than delegating every property access; members not in its set (e.g.
`setTimeout`, `onmessage`, or script-defined globals on the target window)
are absent until the selective-access forwarding is wired.

**3. Child navigable properties (array-index and named).**  The spec requires
WindowProxy to expose child browsing contexts by numeric index (`window[0]`,
`window[1]`) and by name.  This requires tracking the document-tree child
navigables on the Document, which is not yet implemented.

**4. `top`/`parent` return the proxy itself.**  Resolving the top/parent
navigable's WindowProxy requires the navigable hierarchy, which the shim does
not yet consult.

**5. `name`, `opener`, `closed`, `focus` are stubs.**  The navigable target
name, opener relationship, and closed state are user-agent state that the
shim does not yet track or forward.

## Related documentation

- `content/src/webidl/README.md` — Boa platform object integration, exotic object pattern
- `content/src/js/README.md` — Boa integration specifics (Context ownership, bindings)
- `content/README.md` — Content-crate overview
- `user_agent/src/user_agent.rs` — `create_new_top_level_traversable_from_content`, `create_new_top_level_traversable`, `the_rules_for_choosing_a_navigable` (UA side), `setup_opener_for_window_open`, and the agent model (`AgentCluster`, `AgentClusterKey`, `similar_origin_window_agent`)
- `ipc_messages/src/content.rs` — `NewTraversableInfo`, `CreateEmptyDocument`, `NavigateRequest`
- `content/src/html.rs` — `the_rules_for_choosing_a_navigable` (content side), `navigate`, `ChosenNavigable`
- `content/src/html/window.rs` — `Window::open`, `window_open_steps`
- `content/src/html/global_scope.rs` — `create_auxiliary_context_document`, `set_navigable_hierarchy`
