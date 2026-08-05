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
| `RenderingOpportunity` | UA `NoteRenderingOpportunity`/`FrameNeeded`, content `UpdateTheRendering`, graphics `GraphicsComputed` (traced when the pixels are actually sent) | FrameNeeded-gated render cycle with double buffering: a render starts only when the embedder needs a frame (paced by vsync) AND a rendering opportunity was noted; the paint consumes the composed frames, and the pipeline never holds more than `BufferCount` (2) renders in flight (one displayed, one being rendered) |
| `MessagePort*` | (not written yet) | — |

### RenderingOpportunity scope

The model tracks the frame-production cycle at the **top-level traversable** only.
`FrameNeeded` and `GraphicsComputed` only ever fire for top-levels (the embedder
sends `frame_needed` for webviews; graphics only computes when the root frame
arrives), and child navigables' rendering updates
(`NoteRenderingOpportunity`/`UpdateTheRendering` for child ids) are content that
is composed into the parent's frame — they change no counter the model's
invariants constrain.

The verification session (`verify-specs.sh`) navigates only to `about:blank`, so
no child-navigable events ever enter the trace and the child path is untested.
Observed when a local page with an `<iframe>` was loaded instead: the trace
contained child `NoteRenderingOpportunity`/`UpdateTheRendering` entries, which
the model as written cannot replay — a child's `pending` counter can never rise
because `FrameNeeded`/`GraphicsComputed` are top-level-only and no model action
drains a child's batched opportunity (in the code this is the UA's
`queue_update_the_rendering_for_navigables`, called when the top-level's
`FrameNeeded` arrives). Child rendering updates are therefore deliberately out
of the model's scope.

Not investigated: because the UA clears a composed child's pending state at
`PixelFrameReady`, a child whose `PaintFrame` reaches graphics after the
parent's composition is stored but not composed until the parent's next frame,
and its next update depends on a new rendering opportunity being noted. Whether
child content can consequently lag one frame behind (staleness) has not been
investigated.

### Adding a spec

1. Write `{Name}.tla` (the model) and `{Name}Trace.tla` (the trace consumer) plus `{Name}.cfg` / `{Name}Trace.cfg` in `verification/tla_specs/`. Discovery is automatic from the flat `.tla` files.
2. Emit events from code with `verification::tla_log!(tracer, -> "{Name}", "Event", args...)`. Producers holding a `TraceSender`/`TLATracer` are the UA, the graphics process, and the embedder apps.
3. For specs whose event args need TLA+ numeric types (e.g. generation counters), extend `collect_trace_ids_for_spec` / `render_trace_data_module_for_spec` in `verification/src/validate.rs` with a custom event renderer (see `render_gpu_rendering_trace_event`, which renders generation/region args as integer literals).
4. `./verification/verify-specs.sh` picks the new spec up automatically; a `CHECK {Name} ... OK` line means the recorded trace is consistent with the model.

### Spec file style

- The `.tla` model files carry **no prose**: at most a one-line comment stating the
  reason the spec exists. The model's definitions, action names, and invariants
  are the documentation — explanatory paragraphs about the modeled system live in
  this README and the owning feature's README, not in the spec.