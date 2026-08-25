//! Window display for the AppKit backend: a layer-hosting view whose layer
//! `contents` is set directly to the shared IOSurface from the graphics
//! process. CoreAnimation composites the surface with no texture import and
//! no pixel copy — the zero-copy blit. The surface is padded to a
//! 64-multiple width (Metal constraint), so `contentsRect` clips the
//! display to the logical width.
//!
//! This backend supports only the zero-copy shared-surface path; the CPU
//! readback path belongs to the non-macOS embedders.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::MainThreadMarker;
use objc2_io_surface::IOSurfaceRef;
use objc2_quartz_core::CALayer;

/// Create a layer-hosting view for web content: the view owns an explicit
/// `CALayer` whose `contents` the app sets directly (the zero-copy
/// IOSurface blit). Returns the view and its layer; the app keeps the layer
/// alive, and the redraw policy is `Never` so AppKit never touches the
/// layer's contents on display cycles.
///
/// Order matters: the custom layer is attached with `setLayer` **before**
/// `setWantsLayer(true)`, so AppKit hosts the provided layer instead of
/// creating its own backing layer for a layer-backed view.
pub(super) fn new_layer_hosted_view(
    mtm: MainThreadMarker,
    initial_scale_factor: f64,
) -> (Retained<NSView>, Retained<CALayer>) {
    let view = NSView::new(mtm);
    let layer = CALayer::layer();

    // STEP 1: attach the custom layer first; this also implies
    // `wantsLayer` for the view.
    view.setLayer(Some(&layer));

    // STEP 2: explicitly mark the view layer-backed; AppKit must not
    // create its own layer or clear the provided one.
    view.setWantsLayer(true);
    view.setLayerContentsRedrawPolicy(objc2_app_kit::NSViewLayerContentsRedrawPolicy::Never);

    // The layer follows the view's bounds on resize.
    layer.setAutoresizingMask(
        objc2_quartz_core::CAAutoresizingMask::LayerWidthSizable
            | objc2_quartz_core::CAAutoresizingMask::LayerHeightSizable,
    );
    // Compositor transparency glitches: the web content is always opaque.
    layer.setOpaque(true);
    // Clip the per-navigable/per-video sublayers to the web content area so
    // they cannot overflow onto the chrome (tab strip, address bar) when the
    // page scrolls them outside the webview's bounds.
    layer.setMasksToBounds(true);
    // Match the window's backing scale so the contents map 1:1 onto
    // physical pixels on Retina displays.
    layer.setContentsScale(initial_scale_factor);

    (view, layer)
}

/// Present a shared IOSurface on the web content layer. `surface_width` is
/// the padded (64-multiple) surface width; `logical_width` is the content's
/// actual width in pixels.
///
/// The presentation runs inside a `CATransaction` with implicit actions
/// disabled so the contents swap commits immediately, without fade or
/// animation glitches; the explicit `flush` pushes the transaction to the
/// Window Server right away instead of waiting for the next run-loop pass.
pub(super) fn present_shared_surface(
    layer: &CALayer,
    surface: &IOSurfaceRef,
    logical_width: u32,
    surface_width: u32,
    scale_factor: f64,
) {
    // Keep the layer's contents scale in sync with the display the window
    // currently lives on.
    if layer.contentsScale() != scale_factor {
        layer.setContentsScale(scale_factor);
    }

    objc2_quartz_core::CATransaction::begin();
    objc2_quartz_core::CATransaction::setDisableActions(true);

    // SAFETY: the layer retains the surface; the surface is a valid
    // Objective-C object.
    let _: () = unsafe { msg_send![layer, setContents: surface] };
    layer.setContentsRect(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(
            f64::from(logical_width) / f64::from(surface_width.max(1)),
            1.0,
        ),
    ));

    objc2_quartz_core::CATransaction::commit();
    // Force the transaction to the render server immediately.
    objc2_quartz_core::CATransaction::flush();
}

use objc2_foundation::{NSPoint, NSRect, NSSize};
