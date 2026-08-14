# content crate

The content crate owns the content process: DOM and HTML algorithms, document
parsing and lifecycle, generic JavaScript engine integration via the
`js_engine` trait, Streams and Web IDL bridges, and the typed IPC boundary
back to the embedder and user agent.

## Design philosophy

Content code follows the same call chains the web standards define.  When a
spec algorithm calls Web IDL (e.g. type conversion, promise manipulation),
content code routes through `content/src/webidl/`.  When a spec algorithm
calls ECMA-262 directly (e.g. realm creation, script evaluation), content
code calls the `js_engine` trait directly.  The exception that routes
through `content/src/webidl/` anyway: HTML algorithms that read JS state in
place of a Web IDL step (e.g. the `self` getter's "relevant
realm.[[GlobalEnv]].[[GlobalThisValue]]" read, implemented in
`content/src/webidl/realm.rs`) — webidl hosts those direct-JS-call quirks
so the domain getter implements the spec steps and the bindings stay thin.
See `content/src/js/bindings/README.md`.  No Boa-specific APIs appear
above `js_engine/src/boa/`.  See `js_engine/README.md` for the full
design philosophy and `content/src/generic_js_test.rs` for validated
patterns.

## GcCell borrow discipline

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

Do not write code that hands `ec` to a closure while a cell borrow is live
(e.g. `with_..._mut(|data, ec| ...)` patterns). The V8 backend enforces the
rule as a backstop: `HeapCell::trace` aborts if marking visits a
mutably-borrowed cell (a Rust panic there would unwind across the C++
marking visitor, so the failure is a hard abort with a log line), and there
is deliberately no `Trace` impl for bare `std::cell::RefCell` — a
`#[gc_struct]` field that needs interior mutability must use `GcCell`, or be
marked `#[ignore_trace]` when it holds no cppgc edges. The one remaining
exception class is the `with_object_any_mut_with` platform-object closure
pattern (it hands `&mut dyn Any` and `ec` to the operation; see
`js_engine/src/v8/README.md`, "Remaining work").

## Known issues

- **Clippy warning backlog.** The content crate has a backlog of pre-existing
  clippy warnings (e.g. "useless conversion to the same type: V8Object").

## Layout

- `content/src/main.rs` and the root modules resume embedder-driven HTML algorithms and content IPC entry points.
- `content/src/dom` holds native DOM [platform objects](https://webidl.spec.whatwg.org/#dfn-platform-object) and DOM Standard algorithm implementations.
- `content/src/ui_events` holds the UI Events Standard types (UIEvent, MouseEvent) and their constructors.  Note: `MouseEvent` and `MouseEventInit` are defined by the Pointer Events spec (`https://w3c.github.io/pointerevents/`), not UI Events; only `UIEvent` and its members live in `https://w3c.github.io/uievents/`.
- `content/src/html` holds parser, document lifecycle, navigation helpers, and HTML global-object [platform objects](https://webidl.spec.whatwg.org/#dfn-platform-object).
- `content/src/js` holds the content crate's JS integration layer: type aliases pointing to the concrete `js_engine` backend, generic platform-object resolution and downcast helpers, and JavaScript dispatch glue. The `js_engine` trait itself lives in the top-level `js_engine/` crate (see its `README.md`).
- `content/src/webidl` holds shared Web IDL callback and promise algorithms (implements Web IDL §3 JavaScript binding).
- `content/src/streams` holds native Streams [platform objects](https://webidl.spec.whatwg.org/#dfn-platform-object) and Streams Standard algorithms.
- `content/src/infra` holds shared Infra Standard helpers.

## Three-layer architecture

Every Web-exposed feature follows a three-layer split (domain → Web IDL infra →
JS bindings glue).  See `content/src/js/bindings/README.md` for the definitive
description with examples and common mistakes.

## Spec Documentation

### Anchor-only doc comments

Every function, struct, associated constant, and constant definition has
**only** the spec anchor URL in its doc comment. Zero prose — not a single
explanatory sentence.

```rust
/// <https://dom.spec.whatwg.org/#concept-event-dispatch>
pub(crate) fn dispatch_event(ec, path, event) { … }
```

- Any prose following the anchor is a violation.  The spec IS the documentation.
- The only exception is a `// Note:` on a separate line below the anchor,
  and only for genuine spec discrepancies (split-process, browser-engine
  refactoring).  Such notes must be fewer than ten across the codebase.

### Step comments inside function bodies

Every spec algorithm step has a `// Step N:` comment quoting the **exact spec
step text verbatim** — not an abbreviation or summary.  Step numbering must
match the spec exactly.

```rust
// Step 1: If event's dispatch flag is set, or if its initialized flag is not set,
//         then throw an "InvalidStateError" DOMException.
if *event.dispatch_flag.borrow() || !*event.initialized_flag.borrow() {
    return Err(ec.new_type_error("…"));
}

// Step 3: Return the result of dispatching event to this.
crate::dom::dispatch_event(ec, path, event)
```

- Blank lines separate code BLOCKS, not comments from code. The pattern:
  ```
  // Step N: comment
  code;
                          ← blank line
  // Step N+1: comment
  code;
  ```
- NO blank line between the function/block opening `{` and the first step comment.
- NO blank line between a step comment and its immediately following code.
- Blank line AFTER the code, before the next step's comment.
- Use `// TODO: Not yet implemented.` for spec steps that are not yet
  implemented — every step must be accounted for.
- For sub-algorithms called by the spec, cross-reference with the anchor URL
  in a comment (e.g. `// <https://dom.spec.whatwg.org/#concept-event-dispatch>`).

### Function naming and algorithm structure

- Name functions after the spec algorithm they implement (e.g. `flatten_more`
  for "flatten more options", `convert_js_to_dictionary` for "convert a JavaScript
  value to dictionary").
- If you must split a spec algorithm into multiple internal helpers, provide
  a single public function with the spec's name and explain the split with a
  `// Note:`.
- When a function partially implements a spec algorithm, annotate with `// Step N:`
  for ALL steps of the algorithm. Mark missing steps with `// TODO: Not yet
  implemented.` See `html/dispatch.rs::steps_to_fire_beforeunload` for the correct pattern.

### `// Note:` for discrepancies only

`// Note:` is for discrepancies between the code and the spec text (e.g. steps
merged across processes, browser-engine refactoring). Design notes, architecture
rationales, and implementation plans belong in the README chain, not in Notes.

### Full reference

See `content/src/js/bindings/README.md` for the complete Common Mistakes table
covering all annotation patterns, three-layer architecture rules, and the
correct treatment of infrastructure code vs spec algorithms.