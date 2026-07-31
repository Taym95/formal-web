# Graphics Process — Surface Delivery Pipeline

## Status

Composed scenes are delivered to the embedder via **CPU readback + shared
memory**: the graphics process renders each frame with Vello, reads the pixels
back to CPU, and writes them into a persistent per-webview shared-memory buffer.
The embedder uploads those bytes in place into a persistent per-webview wgpu
texture registered once with its own Vello renderer, and draws the stable
resource each frame. Cross-process IOSurface sharing was attempted first and
failed on macOS Sequoia 15.x — see below.

## Failed approaches (IOSurface zero-copy)

| Approach | Problem |
|---|---|
| `IOSurfaceLookup(id)` — look up by global IOSurfaceID | Returns NULL cross-process on modern macOS; deprecated since 10.11 |
| Mach port via ipc-channel serde — extract port from `OpaqueIpcSender` UB read + hand-rolled `mach_msg` | UB + sender port arrives as 0; custom Mach structs had layout bugs |
| Bootstrap register/lookup — `bootstrap_register` receive port, graphics sends IOSurface Mach port via `mach_msg` | `mach_msg` fails with `MACH_SEND_INVALID_DEST`; `bootstrap_register` is unreliable on Sequoia |

## Current implementation

- **Graphics process** (`graphics/src/renderer.rs`, `graphics/src/lib.rs`):
  `GpuRenderer::render_scene` renders the composed scene (Vello compute →
  intermediate texture) and **submits** the GPU → CPU readback without
  blocking: the staging buffer is mapped via `map_buffer_on_submit`, and a
  `PollRequest` is sent to a dedicated **poll thread** (spawned at graphics
  startup) that blocks on `device.poll(PollType::Wait)` until the submission
  completes.  The map callback fires on the poll thread and delivers a
  `ReadbackReady` message (carrying the frame metadata and the pre-selected
  shared-memory buffer index) to the main `select!` loop — no interval
  polling or busy loop.  `submit_composed_scene` picks the next free buffer of
  the **three-slot ring** at submit time (`SurfaceFrameSubmitted`), and
  `handle_readback_ready` writes the completed pixels into that buffer, marks
  it pending, and sends `GraphicsEvent::PixelFrameReady`
  (`SurfaceFrameSent`).  A buffer is only rewritten after the embedder acks
  the frame that used it (`GraphicsCommand::TextureConsumed` carrying the
  generation); when every buffer is reserved or pending, the composed scene
  is deferred (`deferred_scene`) and submitted as soon as an ack frees a
  buffer.
- **Embedder** (`embedder/src/windowed.rs`): one persistent `wgpu::Texture` per
  webview (`Rgba8Unorm`, `COPY_DST | COPY_SRC | TEXTURE_BINDING`), registered
  once with Vello via `try_register_custom_resource`.  Each
  `NewWebContentSurface` uploads the shared-memory bytes with
  `queue.write_texture` (in place, no allocation, no re-registration) and
  `paint_frame` draws `PaintRef::Resource` with the stable `ResourceId`.  Vello
  re-copies the override texture into its image atlas every frame
  (`mark_override_image_dirty`), so the updated pixels are picked up without a
  CPU-side image import.  Textures are recreated and re-registered only on
  viewport resize.  After the upload the embedder sends the `TextureConsumed`
  ack (via `WebviewProvider::texture_consumed` → UA → graphics process), which
  frees the shared-memory buffer for reuse.

The `ipc::mach_transport` module (gated on `#[cfg(target_os = "macos")]`)
still contains the working `mach2`-based primitives for sending/receiving
IOSurface port rights via raw Mach messages, plus the
`bootstrap_register`/`bootstrap_look_up` wiring.  They are unused by the
shipped path but could be reused if a zero-copy transport is ever revisited.
