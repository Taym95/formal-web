# verification crate

The verification crate owns trace recording, TLA+ validation, and the shutdown workflow that ties them together.

- The main process starts the trace monitor, shares senders with local workers, and sends the same sender to the content and net processes after IPC bootstrap.
- Trace specs live under `verification/tla_specs/`, and recorded NDJSON logs plus TLC working files live in temporary directories that are removed after validation.
- `VerificationRun::finish()` requires all top-level `TraceSender` clones to be dropped so shutdown can observe channel closure and complete.
- Verification uses the local TLA+ Toolbox jar at `/Applications/TLA+ Toolbox.app/Contents/Eclipse/tla2tools.jar` by default.
- `./verification/verify-navigation.sh` is the canonical end-to-end navigation verification command. It drives one link click and treats successful `Navigation` TLA+ validation as the acceptance criterion. On failure, the human-readable report includes the failing NDJSON entry, preceding context entries, and the full TLC counterexample trace.

## Specs

| Spec | Events traced (producer) | Validates |
|---|---|---|
| `Navigation` | UA navigation lifecycle | navigable/navigation state machine |
| `RenderingOpportunity` | UA `NoteRenderingOpportunity`/`NoteComposedScene`, content `UpdateTheRendering`, graphics `ComposeScene` | batched rendering-opportunity counters |
| `GPURendering` | graphics `SurfaceFrameSent`/`TextureConsumed`, embedder `SurfaceFrameReceived` | per-webview surface buffer ring: strict +1 generation sequencing, three-slot ring, a buffer is never rewritten while pending the embedder's ack, fresh free ring after resize, consumed frames match sent frames |
| `MessagePort*` | (not written yet) | — |

### Adding a spec

1. Write `{Name}.tla` (the model) and `{Name}Trace.tla` (the trace consumer) plus `{Name}.cfg` / `{Name}Trace.cfg` in `verification/tla_specs/`. Discovery is automatic from the flat `.tla` files.
2. Emit events from code with `verification::tla_log!(tracer, -> "{Name}", "Event", args...)`. Producers holding a `TraceSender`/`TLATracer` include the UA, the graphics process, and (since the GPURendering work) the embedder apps.
3. For specs whose event args need TLA+ numeric types (e.g. generation counters), extend `collect_trace_ids_for_spec` / `render_trace_data_module_for_spec` in `verification/src/validate.rs` with a custom event renderer (see `render_gpu_rendering_trace_event`, which renders generation/region args as integer literals).
4. `./verification/verify-specs.sh` picks the new spec up automatically; a `CHECK {Name} ... OK` line means the recorded trace is consistent with the model.

## Session investigation log

### 2026-08-01 — GPURendering model confirms the surface buffer workflow

**Files changed:** `verification/tla_specs/GPURendering*.tla`/`.cfg` (new),
`verification/src/validate.rs` (spec wiring + typed trace-event renderer),
`graphics/src/lib.rs` (`SurfaceFrameSent` trace in `send_composed_scene`),
`embedder/src/{event_loop,windowed,headless}.rs` (embedder `TLATracer`
plumbing + `SurfaceFrameReceived`).

**What was confirmed:** The recorded trace (headless verify run) shows
`SurfaceFrameSent [webview, gen=1, "800x600", region=0]`,
`SurfaceFrameReceived [webview, gen=1, "800x600"]`, then
`gen=2, region=1` — strict +1 generation sequencing, double-buffer region
alternation, and embedder consumption matching the sent generation and size.
The model checks clean (model checking + trace validation).

**Scope limits:** The headless verify flow has a fixed 800x600 viewport, so
the resize rule (fresh region 0 after a size change) is exercised by model
checking and by windowed `--verify` sessions with real resizes, not by the
automated headless run. The model validates the buffer bookkeeping
(generation/region/size); it does not observe pixel contents, so it cannot
directly catch corruption whose cause is in the render output or transport
bytes.

### 2026-08-01 — ack protocol: GPURendering updated to the 3-slot ring

**Files changed:** `verification/tla_specs/GPURendering.tla` (model now tracks
per-region `buffer_state` ∈ {FREE} ∪ generations, `TextureConsumed` action
freeing the pending region, ack-gating in `SurfaceFrameSent`),
`GPURenderingTrace.tla` (`TextureConsumedTrace`), `validate.rs` (typed args
for `TextureConsumed`; `TraceRegion == {0, 1, 2}`; `TraceFree == 0` — an
integer sentinel, since TLC errors on mixed string/int comparisons).

**What was confirmed:** The recorded trace shows the full ack cycle per frame
(`SurfaceFrameSent gen=N region=r` → `SurfaceFrameReceived` →
`TextureConsumed gen=N`) with the ring advancing 0→1→2; the model's
ack-gating invariant (never rewrite a PENDING buffer) holds on the trace.
Model checking (1 webview, 3 generations, 2 sizes — 647 states) passes.

**What was ruled out:** the previous 2-buffer design gave no hard guarantee
that the embedder consumed a frame before its buffer was rewritten; the
ack protocol closes the gap.  A larger model-checking instance (2 webviews)
exploded the state space and was dropped in favor of the 1-webview instance.