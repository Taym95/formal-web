//! <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
//!
//! The WindowProxy is a business-logic shim tied to a navigable rather than
//! to a document: it carries the target navigable's id and outlives document
//! swaps.  A window created by `window.open`, an iframe's `contentWindow`,
//! and a message event's `source` are all WindowProxy shims for their
//! navigable.
//!
//! The shim is a platform object created in the realm that needs it (so its
//! methods run in the caller's realm — the incumbent settings object of any
//! `postMessage` it forwards).  `postMessage` runs the window post message
//! steps steps 1–7 locally and hands the serialized message to the user
//! agent, which routes it to the target navigable's event loop (even when
//! the target lives in the same event loop).  Property access that requires
//! the target window's realm (e.g. `document`) resolves the local window
//! when the target navigable lives in this content process and is otherwise
//! a known gap.
//!
//! The same shim object is reused per (realm, navigable) through the
//! GlobalScope cache, so `event.source === iframe.contentWindow` holds.

use crate::html::Window;
use crate::js::platform_objects::with_global_scope;
use crate::webidl::bindings::create_interface_instance;
use ipc_messages::content::NavigableId;
use js_engine::gc::{GcCell, gc_cell_new};
use js_engine::gc_struct;
use js_engine::{Completion, ExecutionContext, JsTypes};

use crate::js::Types;

type JsValue = <Types as JsTypes>::JsValue;
type JsObject = <Types as JsTypes>::JsObject;

/// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
#[gc_struct]
pub struct WindowProxy {
    /// <https://html.spec.whatwg.org/#navigable-id>
    /// The navigable whose active window this proxy exposes.
    #[ignore_trace]
    pub target_navigable_id: NavigableId,

    /// The target navigable's active Window object when it lives in this
    /// content process; `None` when the window is remote or not yet resolved.
    pub local_window: GcCell<Option<JsObject>>,
}

impl WindowProxy {
    pub(crate) fn new(
        target_navigable_id: NavigableId,
        local_window: Option<JsObject>,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Self {
        Self {
            target_navigable_id,
            local_window: gc_cell_new(local_window, ec),
        }
    }

    /// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
    pub(crate) fn target_navigable_id(&self) -> NavigableId {
        self.target_navigable_id
    }

    /// Resolve the local Window backing this proxy, when the target navigable
    /// lives in this content process.
    pub(crate) fn local_window(&self, ec: &mut dyn ExecutionContext<Types>) -> Option<JsObject> {
        self.local_window.borrow(ec).clone()
    }
}

/// Create (or fetch from the realm's cache) the WindowProxy shim for a
/// navigable.  `local_window` seeds the same-process backing when known.
///
/// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
pub(crate) fn create_window_proxy(
    target_navigable_id: NavigableId,
    local_window: Option<JsObject>,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    let cached = with_global_scope(ec, |global_scope, ec| {
        Ok(global_scope.cached_window_proxy(target_navigable_id, ec))
    })?;
    if let Some(cached) = cached {
        return Ok(<Types as JsTypes>::value_from_object(cached));
    }

    let object = create_interface_instance::<Types, WindowProxy>(
        WindowProxy::new(target_navigable_id, local_window, ec),
        ec,
    )?;
    with_global_scope(ec, |global_scope, ec| {
        global_scope.cache_window_proxy(target_navigable_id, object.clone(), ec);
        Ok(())
    })?;
    Ok(<Types as JsTypes>::value_from_object(object))
}

/// Create the WindowProxy shim for a navigable and return the platform
/// object (used when the shim is embedded in another platform object, e.g.
/// MessageEvent's source).
pub(crate) fn window_proxy_object(
    target_navigable_id: NavigableId,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsObject, Types> {
    let cached = with_global_scope(ec, |global_scope, ec| {
        Ok(global_scope.cached_window_proxy(target_navigable_id, ec))
    })?;
    if let Some(cached) = cached {
        return Ok(cached);
    }

    let object = create_interface_instance::<Types, WindowProxy>(
        WindowProxy::new(target_navigable_id, None, ec),
        ec,
    )?;
    with_global_scope(ec, |global_scope, ec| {
        global_scope.cache_window_proxy(target_navigable_id, object.clone(), ec);
        Ok(())
    })?;
    Ok(object)
}

/// Resolve the Window from a value that may be a Window or a WindowProxy
/// shim.  For a shim, the local Window is returned when the target navigable
/// lives in this content process; otherwise the caller's global is the only
/// fallback available.
pub(crate) fn resolve_window(
    value: &JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> JsObject {
    if let Some(object) = value.as_object() {
        if let Some(_) = ec
            .with_object_any(&object)
            .and_then(|a| a.downcast_ref::<Window>())
        {
            return object;
        }
        if let Some(window) = ec
            .with_object_any(&object)
            .and_then(|a| a.downcast_ref::<WindowProxy>().cloned())
            .and_then(|shim| shim.local_window(ec))
        {
            return window;
        }
        // For non-Window values, return the global.
        return ec.global_object();
    }

    // For non-object values, fall back to the global object.
    ec.global_object()
}
