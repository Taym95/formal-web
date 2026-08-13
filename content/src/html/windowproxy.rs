//! <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
//!
//! The identity handed to JavaScript is an ECMAScript Proxy whose target is
//! a [`WindowProxy`] platform object tied to the navigable (one per (realm,
//! navigable), cached on the realm's GlobalScope).  The proxy's `backing`
//! field carries the navigable's active Window when it lives in this content
//! process (same agent cluster): the proxy traps then delegate property
//! access to that Window — the local behavior.  When the navigable's
//! document was created in another content process, the backing is
//! `CrossContentProcess` and the traps resolve the proxy's cross-origin
//! member set instead, while keeping the proxy's identity.  `event.source
//! === iframe.contentWindow` holds because the same proxy is reused per
//! (realm, navigable).

use crate::html::Window;
use crate::js::create_builtin_fn_with_traced_captures;
use crate::js::platform_objects::with_global_scope;
use crate::webidl::bindings::create_interface_instance;
use crate::webidl::is_array_index_key;
use ipc_messages::content::NavigableId;
use js_engine::gc::{GcCell, gc_cell_new};
use js_engine::gc_struct;
use js_engine::{Completion, ExecutionContext, JsTypes};

use crate::js::Types;

type JsValue = <Types as JsTypes>::JsValue;
type JsObject = <Types as JsTypes>::JsObject;

// ────────────────────────────────────────────────────────────────────────────
// The WindowProxy domain object
// ────────────────────────────────────────────────────────────────────────────

/// <https://html.spec.whatwg.org/#concept-windowproxy-window>
/// The Window the proxy exposes, when its document lives in this content
/// process; a cross-process window is not backed by a local Window.
#[gc_struct]
pub(crate) enum WindowProxyBacking {
    /// The navigable's active Window lives in this content process; the
    /// proxy traps delegate property access to it.  `js_object` is the
    /// Window's JS object handle, kept rooted so the backing stays usable
    /// across the navigation-commit garbage collection that runs when the
    /// old document is destroyed (a cppgc-traced edge read back from the
    /// cell after that collection is not reliably usable on the V8 backend).
    SameContentProcess {
        /// <https://html.spec.whatwg.org/#concept-windowproxy-window>
        window: Window,

        /// <https://html.spec.whatwg.org/#concept-windowproxy-window>
        /// The Window's JS object handle, rooted for the proxy's lifetime.
        #[ignore_trace]
        js_object: JsObject,
    },

    /// The navigable's active Window was created in another content process;
    /// the proxy traps resolve the proxy's cross-origin member set.
    CrossContentProcess,
}

/// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
#[gc_struct]
pub struct WindowProxy {
    /// <https://html.spec.whatwg.org/#navigable>
    /// The navigable whose active window this proxy exposes.
    #[ignore_trace]
    pub target_navigable_id: NavigableId,

    /// <https://html.spec.whatwg.org/#concept-windowproxy-window>
    /// The proxy's backing, held in a shared cell so every clone of this
    /// struct (the realm's cached copy and the platform object's copy) sees
    /// the same window: navigation commit re-points the cell in place, and
    /// the traps read it.
    pub backing: GcCell<WindowProxyBacking>,
}

impl WindowProxy {
    pub(crate) fn new(
        target_navigable_id: NavigableId,
        backing: WindowProxyBacking,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Self {
        Self {
            target_navigable_id,
            backing: gc_cell_new(backing, ec),
        }
    }

    /// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
    pub(crate) fn target_navigable_id(&self) -> NavigableId {
        self.target_navigable_id
    }

    /// <https://html.spec.whatwg.org/#concept-windowproxy-window>
    /// Clone the backing out of the shared cell (no engine call is made
    /// while the cell borrow is live).
    pub(crate) fn backing(&self, ec: &mut dyn ExecutionContext<Types>) -> WindowProxyBacking {
        self.backing.borrow(ec).clone()
    }

    /// <https://html.spec.whatwg.org/#concept-windowproxy-window>
    pub(crate) fn set_backing(
        &self,
        backing: WindowProxyBacking,
        ec: &mut dyn ExecutionContext<Types>,
    ) {
        self.backing.set(backing, ec);
    }
}

/// The JS object handle of the Window backing the proxy, when it lives in
/// this content process.  The handle is the rooted one stored in the
/// backing (not the EventTarget reflector edge), so it stays usable across
/// the navigation-commit garbage collection.
fn window_object_handle(backing: &WindowProxyBacking) -> Option<JsObject> {
    match backing {
        WindowProxyBacking::SameContentProcess { js_object, .. } => Some(js_object.clone()),
        WindowProxyBacking::CrossContentProcess => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The WindowProxy traps (same-content-process and cross-content-process)
// ────────────────────────────────────────────────────────────────────────────

//
// Each trap is called by the Proxy internal methods and receives:
//     fn(args: &[JsValue], _this: JsValue, ec: &mut dyn ExecutionContext<crate::js::Types>)
//         -> Completion<JsValue, crate::js::Types>
//
// Per the ECMAScript Proxy internal methods (10.5), the **target** is always
// the first argument (`args[0]`).  Since the WindowProxy is created with the
// [`WindowProxy`] platform object as the proxy target, `args[0]` is that
// platform object in every trap call.  The traps resolve the proxy's backing
// from the target's `backing` cell; when it is `CrossContentProcess` (the
// navigable's document lives in another content process) the traps resolve
// the proxy's cross-origin member set instead.
//
// These functions are used as built-in function behaviours: each is wrapped
// with `ec.create_builtin_fn()` and set as a property on the handler
// object passed to `ec.create_proxy()`.

/// Resolve the proxy target (the platform object) and its backing.
fn proxy_target_and_backing(
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Result<(JsObject, WindowProxyBacking), JsValue> {
    let proxy_target = args
        .first()
        .and_then(<Types as JsTypes>::value_as_object)
        .ok_or_else(|| ec.value_undefined())?;
    let proxy = ec
        .with_object_any(&proxy_target)
        .and_then(|data| data.downcast_ref::<WindowProxy>().cloned());
    let backing = match proxy {
        Some(proxy) => proxy.backing(ec),
        None => WindowProxyBacking::CrossContentProcess,
    };
    Ok((proxy_target, backing))
}

/// <https://html.spec.whatwg.org/#windowproxy-getprototypeof>
fn trap_get_prototype_of(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (_proxy_target, backing) = proxy_target_and_backing(args, ec)?;

    // Step 1: "Let W be the value of the [[Window]] internal slot of this."
    // Step 2: "If IsPlatformObjectSameOrigin(W) is true, then return !
    //           OrdinaryGetPrototypeOf(W)."
    // Note: A cross-content-process window has no local Window, so the
    // same-origin branch is skipped and the proxy returns null per step 3.
    let proto = match &backing {
        WindowProxyBacking::SameContentProcess { .. } => {
            let Some(window_object) = window_object_handle(&backing) else {
                return Ok(ec.value_null());
            };
            ec.get_prototype_of(window_object)?
        }
        WindowProxyBacking::CrossContentProcess => None,
    };
    match proto {
        Some(p) => Ok(<crate::js::Types as JsTypes>::value_from_object(p)),

        // Step 3: "Return null."
        None => Ok(ec.value_null()),
    }
}

/// <https://html.spec.whatwg.org/#windowproxy-setprototypeof>
fn trap_set_prototype_of(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (_proxy_target, backing) = proxy_target_and_backing(args, ec)?;
    let val = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 1: "Return ! SetImmutablePrototype(this, V)."
    // Note: A cross-content-process window has no local Window, so its
    // prototype is null.
    let current = match &backing {
        WindowProxyBacking::SameContentProcess { .. } => {
            let Some(window_object) = window_object_handle(&backing) else {
                return Ok(ec.value_from_bool(false));
            };
            ec.get_prototype_of(window_object)?
        }
        WindowProxyBacking::CrossContentProcess => None,
    };
    let same = match (&current, val.as_object()) {
        (Some(current_proto), Some(v)) => *current_proto == v,
        (None, None) => val.is_null(),
        _ => false,
    };
    Ok(ec.value_from_bool(same))
}

/// <https://html.spec.whatwg.org/#windowproxy-preventextensions>
fn trap_prevent_extensions(
    _args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // Step 1: "Return false."
    Ok(ec.value_from_bool(false))
}

/// <https://html.spec.whatwg.org/#windowproxy-isextensible>
fn trap_is_extensible(
    _args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // Step 1: "Return true."
    Ok(ec.value_from_bool(true))
}

/// <https://html.spec.whatwg.org/#windowproxy-getownproperty>
fn trap_get_own_property_descriptor(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (_proxy_target, backing) = proxy_target_and_backing(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "If P is an array index property name: ..."
    // Note: Child navigable support not yet implemented.
    // Step 3: "If IsPlatformObjectSameOrigin(W) is true, then return !
    //           OrdinaryGetOwnProperty(W, P)."
    let Some(window_object) = (match &backing {
        WindowProxyBacking::SameContentProcess { .. } => window_object_handle(&backing),
        WindowProxyBacking::CrossContentProcess => None,
    }) else {
        // Step 4: "Let property be CrossOriginGetOwnPropertyHelper(W, P)."
        // Step 5: "If property is not undefined, then return property."
        // Step 6: Named child navigable target name properties.
        // Step 7: "Return ? CrossOriginPropertyFallback(P)."
        // Note: A cross-content-process window exposes no own properties
        // beyond the fixed member set, which is resolved by the [[Get]] trap.
        return Ok(ec.value_undefined());
    };

    let prop_key = ec.to_property_key(key)?;
    let Some(desc) = ec.get_own_property(window_object, prop_key)? else {
        return Ok(ec.value_undefined());
    };

    let desc_obj = ec.create_plain_object(None);
    if let Some(value) = desc.value {
        let key = ec.property_key_from_str("value");
        ec.set(desc_obj.clone(), key, value, false)?;
    }
    if let Some(writable) = desc.writable {
        let key = ec.property_key_from_str("writable");
        let value = ec.value_from_bool(writable);
        ec.set(desc_obj.clone(), key, value, false)?;
    }
    if let Some(get) = desc.get {
        let key = ec.property_key_from_str("get");
        let value = <crate::js::Types as JsTypes>::value_from_object(
            <crate::js::Types as JsTypes>::object_from_function(get),
        );
        ec.set(desc_obj.clone(), key, value, false)?;
    }
    if let Some(set) = desc.set {
        let key = ec.property_key_from_str("set");
        let value = <crate::js::Types as JsTypes>::value_from_object(
            <crate::js::Types as JsTypes>::object_from_function(set),
        );
        ec.set(desc_obj.clone(), key, value, false)?;
    }
    if let Some(enumerable) = desc.enumerable {
        let key = ec.property_key_from_str("enumerable");
        let value = ec.value_from_bool(enumerable);
        ec.set(desc_obj.clone(), key, value, false)?;
    }
    if let Some(configurable) = desc.configurable {
        let key = ec.property_key_from_str("configurable");
        let value = ec.value_from_bool(configurable);
        ec.set(desc_obj.clone(), key, value, false)?;
    }
    Ok(<crate::js::Types as JsTypes>::value_from_object(desc_obj))
}

/// <https://html.spec.whatwg.org/#windowproxy-defineownproperty>
fn trap_define_property(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (_proxy_target, backing) = proxy_target_and_backing(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());
    let desc_obj_val = args.get(2).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "If IsPlatformObjectSameOrigin(W) is true:"
    let Some(window_object) = (match &backing {
        WindowProxyBacking::SameContentProcess { .. } => window_object_handle(&backing),
        WindowProxyBacking::CrossContentProcess => None,
    }) else {
        // Step 3: "Throw a 'SecurityError' DOMException."
        // Note: The trap contract returns a boolean; the security error is
        // approximated by refusing the define.
        return Ok(ec.value_from_bool(false));
    };

    // Step 2.1: "If P is an array index property name, return false."
    if is_array_index_key(&key, ec) {
        return Ok(ec.value_from_bool(false));
    }

    // Step 2.2: "Return ? OrdinaryDefineOwnProperty(W, P, Desc)."
    let desc_obj = ec.to_object(desc_obj_val)?;
    let desc = ec.to_property_descriptor(desc_obj)?;
    let prop_key = ec.to_property_key(key)?;
    match ec.define_property_or_throw(window_object, prop_key, desc) {
        Ok(_) => Ok(ec.value_from_bool(true)),
        Err(_) => Ok(ec.value_from_bool(false)),
    }
}

/// <https://html.spec.whatwg.org/#windowproxy-get>
fn trap_get(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (proxy_target, backing) = proxy_target_and_backing(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "Check if an access between two browsing contexts should be
    //           reported, given the current global object's browsing context,
    //           W's browsing context, P, and the current settings object."
    // Note: Access reporting is not yet implemented.
    // Step 3: "If IsPlatformObjectSameOrigin(W) is true, then return ?
    //           OrdinaryGet(this, P, Receiver)."
    // Step 4: "Return ? CrossOriginGet(this, P, Receiver)."
    let receiver = match &backing {
        WindowProxyBacking::SameContentProcess { .. } => {
            let Some(window_object) = window_object_handle(&backing) else {
                return Ok(ec.value_undefined());
            };
            window_object
        }
        WindowProxyBacking::CrossContentProcess => {
            // The self-referencing members return the WindowProxy itself
            // (the trap's Receiver argument), per CrossOriginGet.
            if let Some(s) = key.as_string()
                && (s == "self" || s == "window" || s == "frames" || s == "top" || s == "parent")
            {
                if let Some(receiver) = args.get(2).and_then(<Types as JsTypes>::value_as_object) {
                    return Ok(<Types as JsTypes>::value_from_object(receiver));
                }
                return Ok(<Types as JsTypes>::value_from_object(proxy_target));
            }
            proxy_target
        }
    };

    let prop_key = ec.to_property_key(key)?;
    let result = {
        let receiver_value = <crate::js::Types as JsTypes>::value_from_object(receiver.clone());
        ec.get_v(receiver_value, prop_key)?
    };

    // Note: Wrap callable results so they are invoked with `this` set to the
    // resolved receiver — the Window for a same-content-process window, the
    // proxy's platform object for a cross-content-process window.  The Proxy
    // [[Get]] returns the trap result, but the subsequent Call expression
    // uses the Proxy itself as `this`, and the member functions downcast
    // their receiver to a platform object.
    if let Some(func_obj) = <Types as JsTypes>::value_as_object(&result)
        && ec.is_callable(&result)
    {
        let name_key = ec.property_key_from_str("wrapped");
        let wrapper_fn = create_builtin_fn_with_traced_captures(
            ec,
            WindowProxyGetCapture {
                receiver,
                original_fn: func_obj,
            },
            window_proxy_get_wrapper_behaviour,
            0,
            name_key,
            false,
        );
        let wrapper_obj = <Types as JsTypes>::object_from_function(wrapper_fn);
        return Ok(<Types as JsTypes>::value_from_object(wrapper_obj));
    }

    Ok(result)
}

/// <https://html.spec.whatwg.org/#windowproxy-set>
fn trap_set(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (_proxy_target, backing) = proxy_target_and_backing(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "Check if an access between two browsing contexts should be
    //           reported, ..."
    // Note: Access reporting is not yet implemented.
    // Step 3: "If IsPlatformObjectSameOrigin(W) is true:"
    let Some(window_object) = (match &backing {
        WindowProxyBacking::SameContentProcess { .. } => window_object_handle(&backing),
        WindowProxyBacking::CrossContentProcess => None,
    }) else {
        // Step 4: "Return ? CrossOriginSet(this, P, V, Receiver)."
        // Note: The cross-origin member set has no settable members yet.
        return Ok(ec.value_from_bool(false));
    };

    // Step 3.1: "If P is an array index property name, return false."
    if is_array_index_key(&key, ec) {
        return Ok(ec.value_from_bool(false));
    }

    // Step 3.2: "Return ? OrdinarySet(W, P, V, Receiver)."
    let value = args.get(2).cloned().unwrap_or_else(|| ec.value_undefined());
    let prop_key = ec.to_property_key(key)?;
    ec.set(window_object, prop_key, value, false)?;
    Ok(ec.value_from_bool(true))
}

/// <https://html.spec.whatwg.org/#windowproxy-delete>
fn trap_delete_property(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (_proxy_target, backing) = proxy_target_and_backing(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "If IsPlatformObjectSameOrigin(W) is true:"
    let Some(window_object) = (match &backing {
        WindowProxyBacking::SameContentProcess { .. } => window_object_handle(&backing),
        WindowProxyBacking::CrossContentProcess => None,
    }) else {
        // Step 3: "Throw a 'SecurityError' DOMException."
        // Note: The trap contract returns a boolean; the security error is
        // approximated by refusing the delete.
        return Ok(ec.value_from_bool(false));
    };

    // Step 2.1: "If P is an array index property name:"
    if is_array_index_key(&key, ec) {
        let prop_key = ec.to_property_key(key)?;

        // Step 2.1.1: "Let desc be ! this.[[GetOwnProperty]](P)."
        // Uses has_own_property as proxy for "desc is undefined".
        // Step 2.1.2: "If desc is undefined, then return true."
        // Step 2.1.3: "Return false."
        let has = ec.has_own_property(window_object, prop_key)?;
        return Ok(ec.value_from_bool(!has));
    }

    // Step 2.2: "Return ? OrdinaryDelete(W, P)."
    let prop_key = ec.to_property_key(key)?;
    ec.delete_property_or_throw(window_object, prop_key)?;
    Ok(ec.value_from_bool(true))
}

/// <https://html.spec.whatwg.org/#windowproxy-has>
fn trap_has(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (proxy_target, backing) = proxy_target_and_backing(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Note: The WindowProxy spec does not override [[HasProperty]].  This
    // trap is provided for completeness.  "length" returns true (child
    // frame count); all other keys delegate to the target's [[HasProperty]].
    if let Some(s) = key.as_string()
        && s == "length"
    {
        return Ok(ec.value_from_bool(true));
    }

    let prop_key = ec.to_property_key(key)?;
    let backing_object = match &backing {
        WindowProxyBacking::SameContentProcess { .. } => {
            window_object_handle(&backing).unwrap_or(proxy_target)
        }
        WindowProxyBacking::CrossContentProcess => proxy_target,
    };
    let result = ec.has_property(backing_object, prop_key)?;
    Ok(ec.value_from_bool(result))
}

/// <https://html.spec.whatwg.org/#windowproxy-ownpropertykeys>
fn trap_own_keys(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (proxy_target, backing) = proxy_target_and_backing(args, ec)?;

    // Step 2: "Let maxProperties be W's associated Document's document-tree
    //          child navigables's size."
    // Note: Child navigable support not yet implemented — keys is empty.
    // Step 3: "Let keys be the range 0 to maxProperties, exclusive."
    // Step 4: "If IsPlatformObjectSameOrigin(W) is true, then return the
    //           concatenation of keys and OrdinaryOwnPropertyKeys(W)."
    // Step 5: "Return the concatenation of keys and !
    //           CrossOriginOwnPropertyKeys(W)."
    // Note: A cross-content-process window resolves the proxy's own keys
    // (empty; the cross-origin member set lives on the platform object's
    // prototype).
    let backing_object = match &backing {
        WindowProxyBacking::SameContentProcess { .. } => {
            window_object_handle(&backing).unwrap_or(proxy_target)
        }
        WindowProxyBacking::CrossContentProcess => proxy_target,
    };
    let window_keys = ec.own_property_keys(backing_object)?;
    let key_array = ec.create_empty_array();
    for val in window_keys.into_iter() {
        let js_val = ec.value_from_property_key(val);
        ec.array_push(&key_array, js_val)?;
    }
    Ok(<crate::js::Types as JsTypes>::value_from_object(key_array))
}

/// Captures for the wrapper function created by `trap_get`.
///
/// Stores the receiver to use as `this` in the wrapped call and the original
/// callable value to invoke.
#[gc_struct]
struct WindowProxyGetCapture {
    /// The object to use as `this` when calling the wrapped function.
    receiver: JsObject,
    /// The original callable function object to invoke.
    original_fn: JsObject,
}

/// Behaviour function for the wrapper created by `trap_get`.
///
/// Ignores `this` (which is the WindowProxy) and calls the original function
/// with `this` set to the captured receiver.
fn window_proxy_get_wrapper_behaviour(
    args: &[JsValue],
    _this: JsValue,
    captures: &WindowProxyGetCapture,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let this_value = <Types as JsTypes>::value_from_object(captures.receiver.clone());
    ec.call(&captures.original_fn, &this_value, args)
}

// ────────────────────────────────────────────────────────────────────────────
// The WindowProxy platform object and the ECMAScript Proxy wrapping it
// ────────────────────────────────────────────────────────────────────────────

/// Create (or fetch from the realm's cache) the WindowProxy for a navigable:
/// an ECMAScript Proxy wrapping the cached [`WindowProxy`] platform object
/// for the navigable.  `local_window` seeds the same-content-process backing
/// when the proxy does not exist yet; navigation commit later re-points it.
///
/// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
pub(crate) fn create_window_proxy(
    navigable_id: NavigableId,
    local_window: Option<(Window, JsObject)>,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    let (cached_proxy, cached_object) = with_global_scope(ec, |global_scope, ec| {
        Ok(global_scope.cached_window_proxy_state(navigable_id, ec))
    })?;

    // Seed the backing when a same-process Window is known and the proxy has
    // no backing yet (e.g. the navigable's document was created in this
    // process after the proxy existed without one).
    if let Some(local_window) = local_window.clone()
        && let Some(cached) = cached_proxy.as_ref()
        && matches!(cached.backing(ec), WindowProxyBacking::CrossContentProcess)
    {
        let (window, js_object) = local_window;
        with_global_scope(ec, |global_scope, ec| {
            global_scope.set_window_proxy_backing(
                navigable_id,
                WindowProxyBacking::SameContentProcess { window, js_object },
                ec,
            );
            Ok(())
        })?;
    }

    if let Some(object) = cached_object {
        return Ok(<Types as JsTypes>::value_from_object(object));
    }

    let window_proxy = match cached_proxy {
        Some(window_proxy) => window_proxy,
        None => {
            let backing = match local_window {
                Some((window, js_object)) => {
                    WindowProxyBacking::SameContentProcess { window, js_object }
                }
                None => WindowProxyBacking::CrossContentProcess,
            };
            let window_proxy = WindowProxy::new(navigable_id, backing, ec);
            with_global_scope(ec, |global_scope, ec| {
                global_scope.cache_window_proxy(navigable_id, window_proxy.clone(), ec);
                Ok(())
            })?;
            window_proxy
        }
    };

    // The platform object (the proxy's target) holds a clone of the domain
    // WindowProxy; the clone shares the backing cell, so navigation commit
    // re-points the backing the traps read.
    let proxy_target = create_interface_instance::<Types, WindowProxy>(window_proxy, ec)?;
    let proxy = create_ecmascript_proxy(proxy_target, ec)?;
    with_global_scope(ec, |global_scope, ec| {
        global_scope.cache_window_proxy_object(navigable_id, proxy.clone(), ec);
        Ok(())
    })?;
    Ok(<Types as JsTypes>::value_from_object(proxy))
}

/// Wrap the platform object in the ECMAScript Proxy that implements the
/// WindowProxy exotic object's internal methods.
///
/// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
fn create_ecmascript_proxy(
    proxy_target: JsObject,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsObject, crate::js::Types> {
    let handler = ec.create_plain_object(None::<&JsObject>);

    let traps: &[(
        fn(
            &[JsValue],
            JsValue,
            &mut dyn ExecutionContext<crate::js::Types>,
        ) -> Completion<JsValue, crate::js::Types>,
        u32,
        &str,
    )] = &[
        (trap_get_prototype_of, 1, "getPrototypeOf"),
        (trap_set_prototype_of, 2, "setPrototypeOf"),
        (trap_is_extensible, 1, "isExtensible"),
        (trap_prevent_extensions, 1, "preventExtensions"),
        (
            trap_get_own_property_descriptor,
            2,
            "getOwnPropertyDescriptor",
        ),
        (trap_define_property, 3, "defineProperty"),
        (trap_get, 3, "get"),
        (trap_set, 4, "set"),
        (trap_delete_property, 2, "deleteProperty"),
        (trap_has, 2, "has"),
        (trap_own_keys, 1, "ownKeys"),
    ];
    #[gc_struct]
    struct TrapCapture {
        #[ignore_trace]
        func: fn(
            &[JsValue],
            JsValue,
            &mut dyn ExecutionContext<crate::js::Types>,
        ) -> Completion<JsValue, crate::js::Types>,
    }

    fn trap_behaviour(
        args: &[JsValue],
        this: JsValue,
        captures: &TrapCapture,
        ec: &mut dyn ExecutionContext<crate::js::Types>,
    ) -> Completion<JsValue, crate::js::Types> {
        (captures.func)(args, this, ec)
    }

    for &(trap_fn, length, name) in traps.iter() {
        let name_key = ec.property_key_from_str(name);
        let builtin_fn = create_builtin_fn_with_traced_captures(
            ec,
            TrapCapture { func: trap_fn },
            trap_behaviour,
            length,
            name_key,
            false,
        );
        let builtin_fn_jsobj = <crate::js::Types as JsTypes>::object_from_function(builtin_fn);
        ec.set(
            handler.clone(),
            ec.property_key_from_str(name),
            <crate::js::Types as JsTypes>::value_from_object(builtin_fn_jsobj),
            false,
        )?;
    }

    let proxy = ec.create_proxy(proxy_target, handler)?;
    Ok(proxy)
}

/// Create the WindowProxy for a navigable and return the JS object handle
/// (used when the WindowProxy is embedded in another platform object, e.g.
/// MessageEvent's source).
pub(crate) fn window_proxy_object(
    navigable_id: NavigableId,
    local_window: Option<(Window, JsObject)>,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsObject, Types> {
    let value = create_window_proxy(navigable_id, local_window, ec)?;
    <Types as JsTypes>::value_as_object(&value)
        .ok_or_else(|| ec.new_type_error("WindowProxy is not an object"))
}

/// Resolve the Window from a value that may be a Window or a WindowProxy
/// platform object.  For a WindowProxy, the local Window is returned when
/// the target navigable lives in this content process; otherwise the
/// caller's global is the only fallback available.
pub(crate) fn resolve_window(
    value: &JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> JsObject {
    if let Some(object) = value.as_object() {
        if ec
            .with_object_any(&object)
            .and_then(|a| a.downcast_ref::<Window>())
            .is_some()
        {
            return object;
        }
        if let Some(window) = ec
            .with_object_any(&object)
            .and_then(|a| a.downcast_ref::<WindowProxy>().cloned())
            .and_then(|proxy| {
                let backing = proxy.backing(ec);
                window_object_handle(&backing)
            })
        {
            return window;
        }
        // For non-Window values, return the global.
        return ec.global_object();
    }

    // For non-object values, fall back to the global object.
    ec.global_object()
}
