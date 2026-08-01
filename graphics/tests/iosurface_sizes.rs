//! Verify the 64-multiple width padding produces usable Metal textures.
#![cfg(all(target_os = "macos", not(feature = "cpu_readback")))]
use graphics::iosurface::padded_width;
use graphics::renderer::{GpuRenderer, ReadbackChannels};

#[test]
fn iosurface_dimension_constraints() {
    let (poll_tx, _poll_rx) = crossbeam_channel::unbounded();
    let (render_done_tx, _render_done_rx) = crossbeam_channel::unbounded();
    let channels = ReadbackChannels {
        poll_tx,
        render_done_tx,
    };
    let renderer = GpuRenderer::new(channels).expect("renderer");
    let mut texture_id = 0u64;
    let sizes = [
        (1600u32, 1030u32),
        (1516u32, 1000u32),
        (100u32, 100u32),
        (63u32, 63u32),
        (1u32, 1u32),
    ];
    for (w, h) in sizes {
        texture_id += 1;
        assert_eq!(padded_width(w) % 64, 0, "padded width must be mult of 64");
        let result = graphics::iosurface::create_shared_texture(&renderer, w, h, texture_id);
        println!(
            "{w}x{h} (padded {}x{h}): {}",
            padded_width(w),
            if result.is_some() { "OK" } else { "FAIL" }
        );
        assert!(result.is_some(), "create_shared_texture({w}x{h}) failed");
    }
}
