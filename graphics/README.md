# Graphics Process — Surface Delivery Pipeline

## Status

Composed scenes are delivered to the embedder via **CPU readback + shared
memory**: the graphics process renders each frame with Vello, reads the pixels
back to CPU, and ships them through IPC shared memory; the embedder uploads
those bytes into a persistent per-webview GPU texture and blits it. This path
works on every platform.

The long-term goal is **zero-copy GPU texture rendering**: the graphics process
renders directly into a GPU texture and the embedder blits that same texture,
with no CPU readback and no IPC byte copies. On macOS the only mechanism for
sharing GPU memory across processes is **IOSurface**, and every attempt to
share an IOSurface cross-process has failed so far (see below). The design in
this document covers the current pipeline, the zero-copy target, the generic
backend abstraction that lets both coexist, and the transport problem that
currently blocks the zero-copy path.

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
- **GPU sync**: the embedder's blit must not start before the producer's GPU
  render into the shared texture completes. The coarse sync is the ack (the
  embedder acks after its blit is submitted — refine to ack after
  `on_submitted_work_done` if needed); a true fence uses a shared
  `MTLSharedEvent`.
- **Lifecycle**: resize recreates the IOSurface + re-shares + re-registers; the
  texture handle's lifetime must outlive in-flight blits.

### The transport blocker (Mach port over ipc-channel)

The `mach_msg` machinery in ipc-channel 0.22 already sends Mach ports in every
message — but only its own channel endpoints (`OsIpcChannel` ports collected by
the serializer from `OpaqueIpcSender`/`OpaqueIpcReceiver` values). There is no
public API to attach an **arbitrary** Mach port (such as an IOSurface's) to a
message: no `OsMachPort` type exists in ipc-channel 0.22 (it is a Servo-fork
addition), and port names are per-task so the right must physically travel in
the message. This is why the earlier attempts resorted to UB port extraction
and hand-rolled messages (both failed).

Options:

1. **Patch ipc-channel** (what Servo does): add a serializable `OsMachPort`
   that pushes a raw `mach_port_t` into a serialization thread-local (mirroring
   `OS_IPC_CHANNELS_FOR_SERIALIZATION`) and pops it on deserialize; extend the
   receive path to collect arbitrary ports alongside channel ports. The
   transport's `send(.., ports, ..)` already accepts a ports vec, so the change
   is bounded — but requires a Cargo `[patch]` to a fork or vendoring.
2. **Implement the raw `mach_msg` in-tree** (in the `ipc` crate), using
   ipc-channel's own macOS `Message` layout as the reference — the previous
   failure was a buggy hand-rolled struct, not an impossible one.
3. **XPC**: `xpc_dictionary_set_iosurface` carries the surface transparently —
   but the XPC receiver is not implemented (`_xpc_unimplemented`), so this
   needs the XPC backend finished first.

## Generic surface backend abstraction

The current code is already shaped for multiple delivery backends: the ring,
the ack protocol, the deferral, the messages, and the embedder's draw path are
all transport-agnostic. The abstraction introduces a per-webview "frame buffer"
concept with two implementations: the current cross-platform CPU path, and a
macOS-only zero-copy path.

### Producer side (graphics)

```rust
/// A per-webview shared frame buffer: the thing a rendered frame is delivered into.
trait SurfaceBuffer {
    /// CPU path: copy rendered pixels into the shared-memory region.
    fn deliver_pixels(&mut self, pixels: &[u8], width: u32, height: u32);
    /// Zero-copy path: the wgpu texture Vello renders *into* directly.
    #[cfg(target_os = "macos")]
    fn render_target(&mut self) -> Option<&wgpu::Texture>;
    /// The wire payload that travels in PixelFrameReady.
    fn wire_payload(&self) -> SurfacePayload;
}

/// The ring lifecycle is backend-agnostic.
struct SurfaceRing<T: SurfaceBuffer> {
    buffers: [T; 3],
    state: [BufferState; 3], // Free / Reserved(g) / Pending(g)
    write_index: usize,
    width: u32,
    height: u32,
}

type CpuSurfaceRing = SurfaceRing<CpuShmemBuffer>;        // current
#[cfg(target_os = "macos")]
type IosurfaceRing = SurfaceRing<IosurfaceTextureBuffer>; // zero-copy
```

The renderer's target becomes an enum, so the readback machinery (staging pool,
poll thread, `ReadbackReady`) is confined to the CPU variant:

```rust
enum RenderTarget<'a> {
    Readback,                          // render to intermediate, read back
    #[cfg(target_os = "macos")]
    SharedTexture(&'a wgpu::Texture),  // render Vello directly into the shared texture
}
```

### Wire payload

```rust
/// What PixelFrameReady carries beyond width/height/generation.
enum SurfacePayload {
    CpuShmem { shmem_key: usize },                    // current
    #[cfg(target_os = "macos")]
    SharedTexture { texture_id: u64 },                // handle to the shared texture
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

Video frames are currently decoded to CPU bytes (`AVPlayerItemVideoOutput::copy_pixel_buffer` → `pixel_buffer_to_frame`) and embedded into the composed scene as image bytes. The zero-copy path composes video as a GPU texture instead:

1. `copy_pixel_buffer` already yields a `CVPixelBuffer`; wrap it as a Metal
   texture via `CVMetalTextureCacheCreateTextureFromImage` (zero-copy when the
   pixel buffer is GPU-backed).
2. Import into the graphics device via the same `texture_from_raw` +
   `create_texture_from_hal` machinery, on the **same device** the `GpuRenderer`
   composites with (the media backend currently has no wgpu device — it needs
   the graphics device's Metal handle from `raw_device()`).
3. Register with the graphics Vello renderer and draw the video embed site as
   `Paint::Resource(video_resource_id)` in the composed scene (the `anyrender`
   recording already supports the `Resource` paint variant).
4. The graphics Vello render samples the video texture directly while rendering
   into the shared IOSurface texture; the embedder's blit includes video with
   zero extra copies.

Caveats: `AVPlayerItemVideoOutput` yields BGRA, so either request an RGBA
pixel-buffer format or do a one-pass BGRA→RGBA GPU blit per frame; the
`CVPixelBuffer` must be retained while the texture referencing it is in use (a
small pool cycling with the ring). This work is orthogonal to the transport
problem — the blit happens entirely inside the graphics process — and is a
prerequisite for the zero-copy route (video must composite into the shared
texture without a CPU round-trip).

## Open risks and questions

- **Transport**: the IOSurface Mach-port delivery is unsolved (the blocker
  above); every option touches either a vendored crate or the XPC backend.
- **GPU sync across processes**: the ack is a coarse fence; a true zero-copy
  path may need a shared `MTLSharedEvent` to bound the consumer's blit against
  the producer's render.
- **Device topology**: the zero-copy path works best with one shared wgpu
  device across webviews (and with the media backend); today the graphics
  process creates a device per webview.
- **Format/lifecycle**: RGBA-only for Vello, pixel-buffer/texture lifetime
  management, resize re-sharing.
