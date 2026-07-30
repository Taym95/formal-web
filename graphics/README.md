# Graphics Process — IOSurface Zero-Copy Pipeline

## Status: Cross-process IOSurface sharing is not working

The graphics process renders composed scenes to an IOSurface-backed wgpu::Texture
via Vello (intermediate texture → GPU blit → export texture).  The GPU-side
pipeline works correctly.  Sharing the IOSurface with the embedder for zero-copy
compositing does not work on macOS Sequoia 15.x — all approaches tried so far
have failed.

## Failed approaches

| Approach | Problem |
|---|---|
| `IOSurfaceLookup(id)` — look up by global IOSurfaceID | Returns NULL cross-process on modern macOS; deprecated since 10.11 |
| Mach port via ipc-channel serde — extract port from `OpaqueIpcSender` UB read + hand-rolled `mach_msg` | UB + sender port arrives as 0; custom Mach structs had layout bugs |
| Bootstrap register/lookup — `bootstrap_register` receive port, graphics sends IOSurface Mach port via `mach_msg` | `mach_msg` fails with `MACH_SEND_INVALID_DEST`; `bootstrap_register` is unreliable on Sequoia |

## Remaining options (unexplored)

- **Unix socket + SCM_RIGHTS:** Both processes are children of the same parent.
  Embedder creates a socket pair, sends one fd via existing IPC (`sendmsg` with
  `SCM_RIGHTS`).  Graphics sends the IOSurface Mach port through the socket.
- **Inline the renderer:** Run compositor + Vello renderer in the embedder
  process, eliminating cross-process GPU sharing entirely.
- **XPC IOSurface objects:** `xpc_dictionary_set_iosurface` /
  `xpc_dictionary_copy_iosurface` handle Mach port transfer transparently.
  Requires XPC IPC backend or a standalone XPC channel.
- **CPU readback + shared memory:** GPU → CPU readback, ship pixels via
  `IpcSharedRegion`, embedder creates Vello `ImageData`.  CPU round-trip but
  will work immediately.  Partially implemented in an earlier iteration.

## Current implementation

The `ipc::mach_transport` module (gated on `#[cfg(target_os = "macos")]`)
contains working `mach2`-based primitives for sending/receiving IOSurface port
rights via raw Mach messages.  The `bootstrap_register` / `bootstrap_look_up`
pair is wired through existing IPC as
`GraphicsCommand::SetSurfaceTransport { bootstrap_name: String }`.
These primitives could be reused with a different channel-establishment mechanism
(e.g. Unix socket SCM_RIGHTS).
