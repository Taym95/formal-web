# Graphics Process — Surface Delivery Pipeline

## Status

Composed scenes are delivered to the embedder via **CPU readback + shared
memory**: the graphics process renders each frame with Vello, reads the pixels
back to CPU, and ships them through IPC shared memory; the embedder uploads
those bytes into a persistent per-webview GPU texture and blits it. This path
works on every platform.

On macOS the **zero-copy** backend is the default (matching the default
AVFoundation media backend): the graphics process renders directly into a
shared IOSurface texture and the embedder imports the same surface and
blits it — no CPU readback, no IPC pixel bytes. The cross-process transport
problem (shipping the surface's Mach port) is solved by the forked
`ipc-channel` (a git dependency on <https://github.com/gterzian/ipc-channel>),
which adds an `OsMachPort` serde-transportable type. The backend is chosen
at compile time by feature: the zero-copy IOSurface backend is the macOS
default, the `cpu_readback` feature replaces it with the CPU readback path
on macOS, and elsewhere (GStreamer media backend) the CPU readback path is
the only one. The embedder and user agent handle both wire payloads
regardless of which backend the graphics process was built with.

## CPU readback + shared memory pipeline (all platforms)

Each frame travels through the processes as follows:

| Step | Where | What happens |
|---|---|---|
| Compose | graphics | `submit_scene` renders into the buffer of a per-webview **2-slot alternating double buffer** that the last render did not use |
| Render + submit | graphics | the renderer renders the composed scene with Vello (CPU path: into an intermediate texture, then **submits** a GPU → CPU readback (`map_buffer_on_submit`) without blocking); a `PollRequest` goes to a dedicated **poll thread** |
| Wait | poll thread | blocks on `device.poll(PollType::Wait)` until the submission completes; the map callback fires there and delivers `ReadbackReady` to the main loop |
| Deliver | graphics | `handle_readback_ready` copies the completed pixels into the pre-selected shared-memory buffer and sends `GraphicsEvent::PixelFrameReady` |
| Upload | embedder | `NewWebContentSurface` uploads the shared-memory bytes with `queue.write_texture` into the webview's persistent texture |
| Blit | embedder | `paint_frame` draws the texture at its natural size via a stable Vello resource (`PaintRef::Resource`) |
| Pace | embedder → UA | just before rendering, the embedder sends `FrameNeeded` (via `WebviewProvider::frame_needed` → UA), paced by vsync (the paint blocks on the drawable); the UA starts the next render cycle only when a frame is needed AND a rendering opportunity was noted |

Key properties:

- The buffers **alternate**: each render cycle renders into the buffer the last
  render did not use. FrameNeeded pacing allows only one render per cycle, so
  the chosen buffer holds the frame from two cycles ago — long since consumed
  by the embedder. **No ack is sent**; the alternation guarantees the chosen
  buffer is free.
- The transport ships pixel bytes as Mach **out-of-line descriptors with
  `MACH_MSG_VIRTUAL_COPY`** — the receiver gets a kernel (copy-on-write)
  snapshot, not shared pages.
- The renderer keeps a per-slot staging-buffer pool (one per in-flight
  frame) and a per-webview device; the renderer always produces a frame (sizes
  are clamped to ≥ 1) so the UA's rendering-opportunity cycle never stalls.
- The FrameNeeded-gated render cycle is modeled and validated by the
  `RenderingOpportunity` TLA+ spec (`verification/tla_specs/RenderingOpportunity.tla`):
  the UA traces `NoteRenderingOpportunity`/`FrameNeeded`, content traces
  `UpdateTheRendering`, and the graphics process traces `GraphicsComputed` when
  `PixelFrameReady` is actually sent — i.e. after the poll thread's
  `device.poll(Wait)` confirmed the render completed on the GPU. The model
  checks that a render starts only when the embedder needs a frame AND a
  rendering opportunity was noted, and that the pipeline never holds more than
  `BufferCount` (2) renders in flight (one displayed, one being rendered).

Per-frame copies today: GPU → CPU readback, kernel copy in the IPC transport,
CPU → GPU upload, and Vello's internal atlas copy.

## Design: zero-copy GPU texture sharing (IOSurface route)

### Data flow

```
[ PRODUCER: graphics process ]          [ CONSUMER: embedder ]
  IOSurfaceRef::create(...)             receive the surface's ID + Mach port
  Metal texture from the IOSurface      IOSurfaceLookup(id) / fallback to
  (objc2-metal newTextureWithDescriptor_iosurface_plane)  lookup_from_mach_port
  import into wgpu via wgpu-hal         import into wgpu via wgpu-hal
  (texture_from_raw + create_texture_from_hal)
  Vello render_to_texture INTO it       try_register_custom_resource (unchanged)
  send the global ID + Mach port        blit via PaintRef::Resource (unchanged)
  ... alternate to the other buffer ... ... send FrameNeeded (next cycle) ...
```

The producer renders directly into one of the two shared textures,
alternating per render cycle; the consumer imports the shared texture once and
blits it. No readback, no IPC pixel bytes, no upload, no ack.

### Verified platform APIs (macOS, as used by this workspace)

- `objc2-io-surface` 0.3.2 (already a dependency of both processes) provides
  `IOSurfaceRef::create(&CFDictionary)`, `.create_mach_port()`, `.id()`,
  `.lookup(id)`, and `.lookup_from_mach_port(port)`.
- `objc2-metal` 0.3.2 (already a dependency) provides
  `MTLDevice::newTextureWithDescriptor_iosurface_plane(...)`.
- wgpu 29 exposes `Device::create_texture_from_hal::<Metal>` and
  `Device::as_hal::<Metal>()` (both `#[cfg(wgpu_core)]`, always enabled
  natively), and `wgpu_hal::metal::Device::texture_from_raw(raw: Retained<...MTLTexture>, format, raw_type, array_layers, mip_levels, copy_size)` plus `raw_device()` (objc2 types, not the `metal` crate).

### Shared-texture constraints

- **Format**: the shared texture must be `Rgba8Unorm` (IOSurface `'RGBA'`,
  `MTLPixelFormat::RGBA8Unorm`) — Vello's `register_texture` requires it.
- **Usage**: producer needs `STORAGE_BINDING | TEXTURE_BINDING | COPY_SRC`
  (Vello renders via compute into the target); consumer needs
  `TEXTURE_BINDING | COPY_SRC` (the `register_texture` requirement).
- **Width padding**: Metal rejects an IOSurface-backed texture whose **width is
  not a multiple of 64** (verified empirically: 1516 fails, 1600 works; height
  is unconstrained). Both sides create/import the surface at
  `round_up_64(width)` and the producer renders only the logical width's
  top-left region; the consumer clips the draw to the logical rect. See
  `graphics/tests/iosurface_sizes.rs`.
- **GPU sync**: the producer waits for its render submission to complete (the
  dedicated poll thread, `PollRequest.done`) before sending `PixelFrameReady`,
  so the embedder's blit never starts before the producer's render. A true
  fence would use a shared `MTLSharedEvent`.
- **Lifecycle**: resize recreates the IOSurface + re-shares + re-registers; the
  texture handle's lifetime must outlive in-flight blits.

### Transporting the IOSurface ID and Mach port

The producer creates each shared surface with `kIOSurfaceIsGlobal` (deprecated
by Apple, but the only mechanism for cross-process IOSurfaceID visibility on
macOS 13+; the surfaces carry page pixels, not secrets) and ships both the
surface's global ID and a Mach port in the `PixelFrameReady` message. The
embedder looks the surface up by ID first (`IOSurfaceLookup`) and falls back
to the Mach port.

The two handles exist because of how CoreAnimation composites layer contents:
a surface object imported only from its Mach port (`IOSurfaceLookupFromMachPort`)
renders empty in a `CALayer`, while a by-ID lookup of the same surface
composites correctly — and, without `kIOSurfaceIsGlobal`, the by-ID lookup
fails for a surface created in another process (macOS 13+ keeps IOSurfaces
process-local by default). The port remains as a fallback for producers that
do not mark the surface global.

The forked `ipc-channel` (a git dependency on
<https://github.com/gterzian/ipc-channel>) provides a serializable `OsMachPort`
type: the port is pushed into a serialization thread-local (mirroring
`OS_IPC_CHANNELS_FOR_SERIALIZATION`) and popped on deserialize, traveling as a
single `MACH_MSG_OOL_PORTS_DESCRIPTOR` (out-of-line ports, `MOVE_SEND`)
appended after the shared-memory descriptors; the receive path collects them
into a third descriptor phase. The fork adds
`OsIpcSender::send_with_mach_ports`; non-macOS platforms are untouched.

## Generic surface backend abstraction

The double buffer, the alternation, the messages, and the embedder's draw
path are all transport-agnostic. The per-webview state is a `WebviewState<R>`
struct holding the compositor (scene assembly, fonts, video frames) and the
renderer (Vello + surface delivery). The double buffer is hidden entirely
inside the renderer —
the graphics event loop only sees the `SurfaceRenderer` trait (`submit_scene`,
`handle_render_done`). Each renderer owns its `SurfaceBuffers` (the generic
alternating lifecycle `SurfaceRingState` plus its backend's payloads:
shared-memory regions or IOSurface textures) and its texture id counter.

`run_graphics_process` is generic over the renderer exactly like the media
backend: `run_graphics_process<B: MediaBackend, R: SurfaceRenderer>`. The
graphics process binary selects the concrete renderer at compile time by
feature (CPU readback off macOS and with `cpu_readback`, zero-copy IOSurface
on macOS by default) and the loop operates on it only through the trait.

The renderers are two implementations of the `SurfaceRenderer` trait
(`renderer/cpu.rs` and `renderer/iosurface.rs`): each defines its own
`RenderData` associated type — the per-frame payload produced at submit time
and consumed by its `handle_render_done`, which sends `PixelFrameReady`.
Completed frames arrive on a single channel whose
message type is the backend's `RenderData` (chosen at compile time), delivered
by the readback map callbacks (CPU) or the poll thread (zero-copy). The shared
`GpuContext` holds what every backend needs (the wgpu device, the Vello
renderer, the video texture machinery, the generation counter).

The renderer's target differs per backend: the CPU path renders to an
intermediate texture and submits a readback; the zero-copy path renders Vello
directly into the shared texture and the poll thread delivers the done notice
once the submission completes.

The video texture import (macOS AVFoundation `PixelBufferFrame` → Metal
texture → Vello `override_image`) lives in its own module, `renderer/video.rs`,
behind the renderer trait's macOS-only `store_video_frame`.

### Consumer side (embedder)

`NewWebContentSurface` matches on the payload: `CpuShmem` → `write_texture`
from the bytes into the persistent texture; `SharedTexture` → the
`WebviewSurfaceTexture` is created from the imported shared texture (via the
hal-import machinery), registered once, then blit. The draw path
(`PaintRef::Resource`) is identical for both.

### Backend selection

Chosen at compile time by the graphics crate's features: the zero-copy
IOSurface backend is the default on macOS (matching the default AVFoundation
media backend); building with `--features cpu_readback` selects the CPU
readback backend on macOS instead. Off macOS the CPU readback backend is the
only one (`zero_copy` is a compile error there — IOSurface sharing is
macOS-only; the GStreamer media backend delivers CPU bytes). The two are
separate implementations of the `SurfaceRenderer` trait (`renderer/cpu.rs`
and `renderer/iosurface.rs`), so only one is compiled per configuration; the
payload enum identifies the backend in use on the wire.

### What stays identical

- The embedder's registration + draw path.

## AVFoundation → shared texture

Video frames are composited as GPU textures (macOS): the AVFoundation pipeline
delivers the decoded `CVPixelBuffer` itself (`MediaBackendEvent::PixelBufferFrame`)
instead of CPU bytes; the graphics process wraps it as a Metal texture via
`CVMetalTextureCacheCreateTextureFromImage` (zero-copy when the pixel buffer is
GPU-backed), does a one-pass BGRA→RGBA compute blit into a per-pipeline RGBA
texture, and registers that texture with its Vello renderer via
`Renderer::override_image` (a fake `ImageData` with an empty blob; the scene
draws it as a plain image brush, so no anyrender changes are needed). The video
composites into the composed scene — including the shared IOSurface on the
zero-copy surface backend — without a CPU round-trip. GStreamer keeps the CPU
byte path (`MediaBackendEvent::Frame`).

**The import is deferred from frame arrival to compose time.** The media
callback (`store_video_frame`) only stores the latest raw frame — the pixel
buffer, its size, and a generation counter — without touching the GPU. When
`submit_scene` runs, `VideoTextures::record_imports` blits exactly the frames
whose generation is newer than the last imported one in their own submission,
right before Vello's render submits (two back-to-back submissions; GPU
execution order guarantees the blit completes before the render reads it).
Re-blitted images are marked dirty so Vello recopies them into its atlas;
unchanged frames reuse their RGBA texture. This makes the import a compose-time
step on the render cycle's own thread instead of an extra `queue.submit` from
the media event path — every `queue.submit` on the main thread blocks on the
gpu poll thread's fence lock until its current `device.poll(Wait)` finishes, so
the media handler no longer stalls the loop, and frames that are never
composited are never blitted.

Caveats: the `CVPixelBuffer` must stay alive while the texture referencing it
is in use (the stored raw frame keeps it until the next frame replaces it); a
BGRA→RGBA blit is needed because Vello's `register_texture` requires
`Rgba8Unorm`. A blit that fails to wrap its source (first import of a paint)
records a texture clear instead so the frame shows black — like a browser
shows for a video that fails to decode — and is retried on the next compose;
a failed re-blit of a previously imported paint leaves the last good frame in
place.

(Alternative tried and parked: merging the blit and Vello's render into a
single command encoder + one submit, which needs a record-only vello API that
0.9 does not expose. The modified vello source with `render_to_texture_into` /
`run_recording_into` is parked at `../../Projects/vello` for now; the build
uses stock vello 0.9.0 with the two-submit layout.)

## Open risks and questions

- **Video still drives the render cycle via the coarse `animating` flag
  (follow-up).** Content still sets `PaintFrame.animating = has_video ||
  document.is_animating()`, so a page with any non-ended video keeps the
  render cycle alive even when the video is not producing frames (failed/
  blocked load, ended-but-not-marked, muted autoplay blocked). The content
  process now *skips blitz* on a clean cycle — a document that was not
  mutated reuses its last recorded scene instead of re-running resolve + paint
  (see `content/src/main.rs` `update_the_rendering`), and the graphics process
  keeps the content layer clean (the reused scene compares byte-identical).
  The remaining waste is the render cycle itself (compose + forward) running
  at vsync. Remaining follow-up:
  - Set `animating` only when the document has a **pending rAF callback**
    (script-driven animation) or blitz is genuinely advancing CSS
    animations, and handle video-driven flow separately: a
    video-frame arrival should re-note a rendering opportunity through the
    UA, and the composition still happens on the top-level `PaintFrame`
    within the cycle (the "never compose independently from the video
    handler" rule, to keep the RenderingOpportunity TLA pipeline model
    valid).
- **Composition waits for the latest embedded frames, and a changed child
  re-notes the parent.** A top-level
  `PaintFrame` arriving at the graphics process before a child (cross-origin
  iframe) `PaintFrame` — the two are produced in parallel content processes —
  marks the composition pending and defers it until every embedded frame it
  references has arrived: child frames (their `PaintFrame` is in flight,
  so the wait is bounded) and video frames whose pipeline is live and has
  not ended/failed (`expected_videos`). A late child or video frame
  completes the pending composition; the composed scene therefore always
  includes the latest embedded frames. The RenderingOpportunity TLA model
  is unchanged: there is still exactly one composition per top-level render
  cycle.
- **The composed scene aggregates the animating flag across the composed
  frames.** `PaintFrame.animating` is recorded per stored frame; a
  composition reports `animating = true` when any composed frame animates
  (the top-level document, or a cross-origin iframe — same-origin iframes
  are subdocuments and already fold into the parent's `is_animating()`),
  and carries the animating frame ids. The UA notes rendering
  opportunities for those navigables on `PixelFrameReady`, so a CSS
  animation or video inside a cross-origin iframe keeps both its own
  process and the top-level rendering until it ends. The RenderingOpportunity TLA model
  abstracts the hierarchy: `frame_needed`, `pending`, and the per-frame
  counters are traced only for the top-level navigable, and the
  model-checking configuration uses independent top-level frames.
- **Resize**: on resize the whole 2-slot IOSurface double buffer is recreated
  and the embedder re-imports + re-registers the new surfaces. The update
  happens in one large step once the resize settles — there is no incremental
  (live) resize animation. Known and accepted for now; a smoother resize would
  re-render at intermediate sizes during the drag and/or reuse buffer slots
  whose size is unchanged.
- **GPU sync across processes**: the producer waits for its render submission
  before signaling (coarse fence); a true zero-copy path may need a shared
  `MTLSharedEvent` to bound the consumer's blit against the producer's render
  without the CPU-side wait.
- **Consumer-blit vs. producer-overwrite ordering**: the consumer's blit of a
  shared buffer and the producer's next render into the same buffer are not
  synchronized at the GPU level (no shared `MTLSharedEvent`). The submission
  ordering is what makes this safe in practice: the producer is strictly
  request-paced — content renders and the producer submits only in response to
  the embedder's `frame_needed`, which `paint_frame` sends before the blit, and
  the UA gates update-the-rendering on `frame_needed` — so the producer can
  never submit a render whose frame the consumer has not requested at a redraw
  (it never runs more than one frame ahead). The alternation means a given
  buffer is written every other cycle; the write into buffer B at cycle N+2 is
  submitted only after the consumer has processed `PixelFrameReady` for B(N) and
  A(N+1), so the B(N) blit was enqueued a full redraw earlier and the overwrite
  submission is downstream of a further full redraw plus a content render. The
  residual (the blit's GPU execution overlapping the overwrite's execution)
  therefore requires the consumer's single draw call to remain unexecuted for
  more than a full render cycle — a consumer-side GPU queue stall. The
  submission ordering is structural; only the GPU execution timing of an
  already-enqueued blit vs. a much-later-submitted overwrite is unenforced.
- **Device topology**: the zero-copy path works best with one shared wgpu
  device across webviews (and with the media backend); today the graphics
  process creates a device per webview. Video textures live on the webview's
  device, so a pipeline plays only on the webview it was created for.
- **Format/lifecycle**: RGBA-only for Vello, pixel-buffer/texture lifetime
  management, the 64-multiple width padding (see above).
- **Layer granularity is one layer per navigable (and per video).** Each
  document is rendered into a single `RecordedScene` and presented to
  CoreAnimation as one `IOSurface` per web layer; there is no sub-division
  within a document. A change that genuinely alters the document's visible
  output (a `:hover` effect, a caret blink, a scroll) therefore re-rasterizes
  and re-presents the whole layer's surface — `store_frame`'s scene comparison
  correctly flags the whole layer dirty, and `setContents:` swaps the entire
  `IOSurface` with no old-vs-new diff, so Quartz Debug flashes the full layer
  bounds rather than the changed pixels. This is expected for the current
  granularity, not a bug in the dirty tracking; eliminating it needs
  damage-rect tracking inside the raster step, or splitting a document into
  multiple GPU-composited layers per containing block / `will-change`
  promotion.
- **`is_animating()` is too coarse: `has_canvas` is unconditional.** Blitz's
  `BaseDocument::is_animating()` includes `has_canvas`, and `compute_has_canvas`
  returns true for any node that is a `<canvas>` element with a `src`
  attribute — regardless of whether that canvas is being redrawn. Content
  therefore treats a page containing a `src`-bearing canvas as animating and
  runs `should_render` (blitz resolve + paint) every cycle for the whole
  document. Not fixable in-repo: `is_animating`/`compute_has_canvas` live in the
  pinned blitz git dependency.
