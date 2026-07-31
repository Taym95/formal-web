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
| `RenderingOpportunity` | UA `NoteRenderingOpportunity`/`NoteComposedScene`, content `UpdateTheRendering`, graphics `GraphicsComputed` (traced when the pixels are actually sent) | batched rendering-opportunity counters |
| `GPURendering` | graphics `SurfaceFrameSubmitted`/`SurfaceFrameSent`/`TextureConsumed`, embedder `SurfaceFrameReceived` | per-webview surface buffer ring: strict +1 submit generations, sent frames are a subset of submitted frames delivered into the pre-selected region, a region is never rewritten while reserved or pending the embedder's ack, fresh free ring after resize, consumed frames match sent frames |
| `MessagePort*` | (not written yet) | — |

### Adding a spec

1. Write `{Name}.tla` (the model) and `{Name}Trace.tla` (the trace consumer) plus `{Name}.cfg` / `{Name}Trace.cfg` in `verification/tla_specs/`. Discovery is automatic from the flat `.tla` files.
2. Emit events from code with `verification::tla_log!(tracer, -> "{Name}", "Event", args...)`. Producers holding a `TraceSender`/`TLATracer` are the UA, the graphics process, and the embedder apps.
3. For specs whose event args need TLA+ numeric types (e.g. generation counters), extend `collect_trace_ids_for_spec` / `render_trace_data_module_for_spec` in `verification/src/validate.rs` with a custom event renderer (see `render_gpu_rendering_trace_event`, which renders generation/region args as integer literals).
4. `./verification/verify-specs.sh` picks the new spec up automatically; a `CHECK {Name} ... OK` line means the recorded trace is consistent with the model.