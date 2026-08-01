# Graphics Process — Surface Delivery Pipeline

## Status

Composed scenes are delivered to the embedder via **CPU readback + shared
memory**: the graphics process renders each frame with Vello, reads the pixels
back to CPU, and ships them through IPC shared memory; the embedder uploads
those bytes into a persistent per-webview GPU texture and blits it. This path
works on every platform.

On macOS a **zero-copy** backend is also implemented: the graphics process
renders directly into a shared IOSurface texture and the embedder imports the
same surface and blits it — no CPU readback, no IPC pixel bytes. The
cross-process transport problem (shipping the surface's Mach port) is solved
by the forked `ipc-channel` (a git dependency on
<https://github.com/gterzian/ipc-channel>), which adds an `OsMachPort`
serde-transportable type. The backend is selected with
`FORMAL_WEB_SURFACE_BACKEND=iosurface|cpu` (default `cpu`).

## Current pipeline (CPU readback + shared memory)

Each frame travels through the processes as follows:

| Step | Where | What happens |
|---|---|---|
| Compose | graphics | `submit_composed_scene` picks the next free buffer of a per-webview **3-slot ring** and reserves it |
| Render + submit | graphics | `GpuRenderer::render_scene` renders the composed scene with Vello into an intermediate texture, then **submits** a GPU → CPU readback (`map_buffer_on_submit`) without blocking; a `PollRequest` goes to a dedicated **poll thread** |
| Wait | poll thread | blocks on `device.poll(PollType::Wait)` until the submission completes; the map callback fires there and delivers `ReadbackReady` to the main loop |
| Deliver | graphics | `handle_readback_ready` copies the completed pixels into the pre-selected shared-memory buffer, marks it pending, and sends `GraphicsEvent::PixelFrameReady` |
| Upload | embedder | `NewWebContentSurface` uploads the shared-memory bytes with `queue.write_texture` into the webview's persistent texture |
| Blit | embedder | `paint_frame` draws the texture at its natural size via a stable Vello resource (`PaintRef::Resource`) |
| Ack | embedder → graphics | after the upload, the embedder sends `TextureConsumed` (via `WebviewProvider::texture_consumed` → UA → graphics), freeing the buffer for reuse |

Key properties:

- The ring is **acked**: a buffer is `Reserved` at submit, `Pending` after
  delivery, and only rewritten after the embedder's `TextureConsumed` ack — the
  embedder is guaranteed to have consumed the previous frame's pixels.
- When every buffer is reserved or pending, the composed scene is **deferred**
  (`deferred_scene`) and submitted as soon as an ack frees a buffer.
- The transport ships pixel bytes as Mach **out-of-line descriptors with
  `MACH_MSG_VIRTUAL_COPY`** — the receiver gets a kernel (copy-on-write)
  snapshot, not shared pages.
- The `GpuRenderer` keeps a per-slot staging-buffer pool (one per in-flight
  frame) and a per-webview device; the renderer always produces a frame (sizes
  are clamped to ≥ 1) so the UA's rendering-opportunity cycle never stalls.

Per-frame copies today: GPU → CPU readback, kernel copy in the IPC transport,
CPU → GPU upload, and Vello's internal atlas copy.

## Failed approaches: cross-process IOSurface zero-copy

| Approach | Problem |
|---|---|
| `IOSurfaceLookup(id)` — look up by global IOSurfaceID | Returns NULL cross-process on modern macOS; deprecated since 10.11 |
| Mach port via ipc-channel serde — extract port from `OpaqueIpcSender` UB read + hand-rolled `mach_msg` | UB + sender port arrives as 0; custom Mach structs had layout bugs |
| Bootstrap register/lookup — `bootstrap_register` receive port, graphics sends IOSurface Mach port via `mach_msg` | `mach_msg` fails with `MACH_SEND_INVALID_DEST`; `bootstrap_register` is unreliable on Sequoia |

## Design: zero-copy GPU texture sharing (IOSurface route)

### Data flow

```
[ PRODUCER: graphics process ]          [ CONSUMER: embedder ]
  IOSurfaceRef::create(...)             receive the surface's Mach port
  Metal texture from the IOSurface      IOSurfaceRef::lookup_from_mach_port(port)
  (objc2-metal newTextureWithDescriptor_iosurface_plane)
  import into wgpu via wgpu-hal         import into wgpu via wgpu-hal
  (texture_from_raw + create_texture_from_hal)
  Vello render_to_texture INTO it       try_register_custom_resource (unchanged)
  create_mach_port + send the port      blit via PaintRef::Resource (unchanged)
  ... await TextureConsumed ack ...     ... send TextureConsumed ack ...
```

The producer renders directly into a ring of shared textures instead of the
intermediate + readback path; the consumer imports the shared texture once and
blits it. No readback, no IPC pixel bytes, no upload.

### Verified platform APIs (macOS, as used by this workspace)

- `objc2-io-surface` 0.3.2 (already a dependency of both processes) provides
  `IOSurfaceRef::create(&CFDictionary)`, `.create_mach_port()`, and
  `.lookup_from_mach_port(port)`.
- `objc2-metal` 0.3.2 (already a dependency) provides
  `MTLDevice::newTextureWithDescriptor_iosurface_plane(...)`.
- wgpu 29 exposes `Device::create_texture_from_hal::<Metal>` and
  `Device::as_hal::<Metal>()` (both `#[cfg(wgpu_core)]`, always enabled
  natively), and `wgpu_hal::metal::Device::texture_from_raw(raw: Retained<...MTLTexture>, format, raw_type, array_layers, mip_levels, copy_size)` plus `raw_device()` (objc2 types, not the `metal` crate).

### Required corrections vs. a naive sketch

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

### The transport blocker (Mach port over ipc-channel) — solved

The `mach_msg` machinery in ipc-channel 0.22 already sends Mach ports in every
message — but only its own channel endpoints (`OsIpcChannel` ports collected by
the serializer from `OpaqueIpcSender`/`OpaqueIpcReceiver` values). There was no
public API to attach an **arbitrary** Mach port (such as an IOSurface's) to a
message: port names are per-task so the right must physically travel in the
message.

**Solution (implemented in the `ipc-channel` fork, a git dependency on
<https://github.com/gterzian/ipc-channel>):** a serializable
`OsMachPort` type that pushes the port into a serialization thread-local
(mirroring `OS_IPC_CHANNELS_FOR_SERIALIZATION`) and pops it on deserialize. The
ports travel as a single `MACH_MSG_OOL_PORTS_DESCRIPTOR` (out-of-line ports,
`MOVE_SEND`) appended after the shared-memory descriptors; the receive path
collects them into a third descriptor phase. `IpcSender::send` gathers them
alongside channel ports. The fork adds `OsIpcSender::send_with_mach_ports`;
non-macOS platforms are untouched.

Rejected alternatives, for the record:

1. Plain per-port descriptors (`MACH_MSG_PORT_DESCRIPTOR`) for the arbitrary
   ports — indistinguishable from channel ports on the receive side (the
   receiver cannot tell how many leading descriptors are channels).
2. **Implement the raw `mach_msg` in-tree** (in the `ipc` crate), using
   ipc-channel's own macOS `Message` layout as the reference — the previous
   failure was a buggy hand-rolled struct, not an impossible one.
3. **XPC**: `xpc_dictionary_set_iosurface` carries the surface transparently —
   but the XPC receiver is not implemented (`_xpc_unimplemented`), so this
   needs the XPC backend finished first.

## Generic surface backend abstraction

The ring, the ack protocol, the deferral, the messages, and the embedder's draw
path are all transport-agnostic. A per-webview `SurfaceBuffers` enum holds the
frame buffers for one of two implementations: the cross-platform CPU path and
the macOS-only zero-copy path. The ring lifecycle (`SurfaceRingState`: pick /
reserve / mark-pending / ack) is shared verbatim.

### Producer side (graphics)

```rust
enum SurfaceBuffers {
    Cpu(SurfaceShmemBuffers),                    // current
    #[cfg(target_os = "macos")]
    Iosurface(IosurfaceBuffers),                 // zero-copy
}

struct SurfaceRingState {
    state: [BufferState; 3], // Free / Reserved(g) / Pending(g)
    write_index: usize,
    width: u32,
    height: u32,
}
```

The renderer's target differs per backend: the CPU path renders to an
intermediate texture and submits a readback; the zero-copy path renders Vello
directly into the shared texture (`render_scene_shared`) and the poll thread
delivers `RenderDone` once the submission completes.

### Wire payload

```rust
/// What PixelFrameReady carries beyond width/height/generation.
enum SurfacePayload {
    CpuShmem { shmem_key: usize },                    // current
    #[cfg(target_os = "macos")]
    SharedTexture { texture_id: u64, port: OsMachPort }, // zero-copy
}
```

### Consumer side (embedder)

`NewWebContentSurface` matches on the payload: `CpuShmem` → `write_texture`
from the bytes into the persistent texture (current); `SharedTexture` → the
`WebviewSurfaceTexture` is created from the imported shared texture (via the
hal-import machinery), registered once, then blit. The draw path
(`PaintRef::Resource`) is identical for both.

### Backend selection

Chosen at startup (e.g. `FORMAL_WEB_SURFACE_BACKEND=iosurface|cpu`, defaulting
to `cpu`); both processes agree because the payload enum identifies the backend
in use.

### What stays identical

- The ack protocol (`TextureConsumed`), the deferral, and the ring logic.
- The embedder's registration + draw path.
- The GPURendering TLA model — it validates the shared ring/ack semantics and
  would check both backends' traces unchanged.

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

Caveats: the `CVPixelBuffer` must stay alive while the texture referencing it
is in use (kept until the next frame replaces it); a BGRA→RGBA blit is needed
because Vello's `register_texture` requires `Rgba8Unorm`.

## Open risks and questions

- **Resize**: on resize the whole 3-slot IOSurface ring is recreated and the
  embedder re-imports + re-registers the new surfaces. The update happens in
  one large step once the resize settles — there is no incremental (live)
  resize animation. Known and accepted for now; a smoother resize would
  re-render at intermediate sizes during the drag and/or reuse ring slots
  whose size is unchanged.
- **GPU sync across processes**: the producer waits for its render submission
  before signaling (coarse fence); a true zero-copy path may need a shared
  `MTLSharedEvent` to bound the consumer's blit against the producer's render
  without the CPU-side wait.
- **Device topology**: the zero-copy path works best with one shared wgpu
  device across webviews (and with the media backend); today the graphics
  process creates a device per webview. Video textures live on the webview's
  device, so a pipeline plays only on the webview it was created for.
- **Format/lifecycle**: RGBA-only for Vello, pixel-buffer/texture lifetime
  management, the 64-multiple width padding (see above).
