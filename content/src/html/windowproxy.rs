//! <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
//!
//! A single WindowProxy mechanism: the identity handed to JavaScript is an
//! ECMAScript Proxy whose target is a [`WindowProxy`] shim platform object
//! tied to the navigable (one per (realm, navigable), cached on the realm's
//! GlobalScope).  The shim's `local_window` field carries the navigable's
//! active Window when it lives in this content process (same agent cluster):
//! the proxy traps then delegate property access to that Window — the local
//! "proxy" behavior.  When the navigable is navigated across origin and its
//! Window is created in another content process, navigation commit severs
//! `local_window` (the old document's destruction in this process clears or
//! re-points the backing), and the traps resolve the shim's cross-origin
//! member set instead — the WindowProxy has become a remote shim while
//! keeping its identity.  `event.source === iframe.contentWindow` holds
//! because the same shim (and proxy) is reused per (realm, navigable).

use crate::html::Window;
use crate::js::create_builtin_fn_with_traced_captures;
use crate::js::platform_objects::with_global_scope;
use crate::webidl::bindings::create_interface_instance;
use crate::webidl::is_array_index_key;
use ipc_messages::content::NavigableId;
use js_engine::gc_struct;
use js_engine::{Completion, ExecutionContext, JsTypes};

use crate::js::Types;

type JsValue = <Types as JsTypes>::JsValue;
type JsObject = <Types as JsTypes>::JsObject;

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
// shim platform object as the proxy target, `args[0]` IS the shim in every
// trap call.  The traps resolve the local backing Window from the shim's
// `local_window` field; when it is `None` (the navigable's document lives in
// another content process) the traps resolve the shim's cross-origin member
// set instead.
//
// These functions are used as built-in function behaviours: each is wrapped
// with `ec.create_builtin_fn()` and set as a property on the handler
// object passed to `ec.create_proxy()`.

/// Resolve the shim (the proxy target) and the local Window backing it, when
/// the navigable's active document lives in this content process.
fn target_shim_and_window(
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Result<(JsObject, Option<JsObject>), JsValue> {
    let shim = args
        .first()
        .and_then(<Types as JsTypes>::value_as_object)
        .ok_or_else(|| ec.value_undefined())?;
    let window = ec
        .with_object_any(&shim)
        .and_then(|data| data.downcast_ref::<WindowProxy>().cloned())
        .and_then(|proxy| proxy.local_window(ec));
    Ok((shim, window))
}

/// <https://html.spec.whatwg.org/#windowproxy-getprototypeof>
fn trap_get_prototype_of(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (_shim, window) = target_shim_and_window(args, ec)?;

    // Step 1: "Let W be the value of the [[Window]] internal slot of this."
    // Note: The shim's local backing window is W; a remote navigable has no
    // local Window (the backing was severed at navigation commit).
    // Step 2: "If IsPlatformObjectSameOrigin(W) is true, then return !
    //           OrdinaryGetPrototypeOf(W)."
    let proto = match window {
        Some(win) => ec.get_prototype_of(win)?,
        None => None,
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
    let (_shim, window) = target_shim_and_window(args, ec)?;
    let val = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 1: "Return ! SetImmutablePrototype(this, V)."
    let current = match window {
        Some(win) => ec.get_prototype_of(win)?,
        None => None,
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
    let (_shim, window) = target_shim_and_window(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "If P is an array index property name: ..."
    // Note: Child navigable support not yet implemented.
    // Step 3: "If IsPlatformObjectSameOrigin(W) is true, then return !
    //           OrdinaryGetOwnProperty(W, P)."
    let Some(window) = window else {
        // Step 4: "Let property be CrossOriginGetOwnPropertyHelper(W, P)."
        // Step 5: "If property is not undefined, then return property."
        // Step 6: Named child navigable target name properties.
        // Step 7: "Return ? CrossOriginPropertyFallback(P)."
        // Note: Remote targets expose no own properties beyond the fixed
        // member set, which is resolved by the [[Get]] trap.
        return Ok(ec.value_undefined());
    };

    let prop_key = ec.to_property_key(key)?;
    let Some(desc) = ec.get_own_property(window, prop_key)? else {
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
    let (_shim, window) = target_shim_and_window(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());
    let desc_obj_val = args.get(2).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "If IsPlatformObjectSameOrigin(W) is true:"
    let Some(window) = window else {
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
    match ec.define_property_or_throw(window, prop_key, desc) {
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
    let (shim, window) = target_shim_and_window(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "Check if an access between two browsing contexts should be
    //           reported, given the current global object's browsing context,
    //           W's browsing context, P, and the current settings object."
    // Note: Access reporting is not yet implemented.
    // Step 3: "If IsPlatformObjectSameOrigin(W) is true, then return ?
    //           OrdinaryGet(this, P, Receiver)."
    // Note: postMessage runs in the caller's realm (the incumbent settings
    // object) with the target navigable's id, exactly like the shim's
    // member, so the message source is the caller's settings object.
    if let Some(s) = key.as_string()
        && s == "postMessage"
    {
        let navigable_id = ec
            .with_object_any(&shim)
            .and_then(|data| data.downcast_ref::<WindowProxy>())
            .map(|proxy| proxy.target_navigable_id())
            .ok_or_else(|| ec.value_undefined())?;
        return create_caller_realm_post_message(navigable_id, ec);
    }

    // Step 4: "Return ? CrossOriginGet(this, P, Receiver)."
    // Note: A remote target (no local backing) resolves the shim's
    // cross-origin member set.  The self-referencing members return the
    // WindowProxy itself (the trap's Receiver argument), per CrossOriginGet.
    let remote = window.is_none();
    if remote
        && let Some(s) = key.as_string()
        && (s == "self" || s == "window" || s == "frames" || s == "top" || s == "parent")
    {
        if let Some(receiver) = args.get(2).and_then(<Types as JsTypes>::value_as_object) {
            return Ok(<Types as JsTypes>::value_from_object(receiver));
        }
        return Ok(<Types as JsTypes>::value_from_object(shim));
    }

    let prop_key = ec.to_property_key(key)?;
    let result = match &window {
        Some(window) => {
            let win_val = <crate::js::Types as JsTypes>::value_from_object(window.clone());
            ec.get_v(win_val, prop_key)?
        }
        None => {
            let shim_val = <crate::js::Types as JsTypes>::value_from_object(shim);
            ec.get_v(shim_val, prop_key)?
        }
    };

    // Note: Wrap callable results so they are invoked with `this` = the
    // Window target, not the WindowProxy.  The Proxy [[Get]] returns
    // trapResult, but the subsequent Call expression uses the base object
    // (the Proxy) as `this`, and resolve_window cannot extract the Window
    // from a Proxy.  Only the local target needs the wrap: the shim's own
    // cross-origin members handle their `this` receiver.
    if let Some(window) = window
        && let Some(func_obj) = <Types as JsTypes>::value_as_object(&result)
        && ec.is_callable(&result)
    {
        let name_key = ec.property_key_from_str("wrapped");
        let wrapper_fn = create_builtin_fn_with_traced_captures(
            ec,
            WindowProxyGetCapture {
                window,
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
    let (_shim, window) = target_shim_and_window(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "Check if an access between two browsing contexts should be
    //           reported, ..."
    // Note: Access reporting is not yet implemented.
    // Step 3: "If IsPlatformObjectSameOrigin(W) is true:"
    let Some(window) = window else {
        // Step 4: "Return ? CrossOriginSet(this, P, V, Receiver)."
        // Note: The remote member set has no settable members yet.
        return Ok(ec.value_from_bool(false));
    };

    // Step 3.1: "If P is an array index property name, return false."
    if is_array_index_key(&key, ec) {
        return Ok(ec.value_from_bool(false));
    }

    // Step 3.2: "Return ? OrdinarySet(W, P, V, Receiver)."
    let value = args.get(2).cloned().unwrap_or_else(|| ec.value_undefined());
    let prop_key = ec.to_property_key(key)?;
    ec.set(window, prop_key, value, false)?;
    Ok(ec.value_from_bool(true))
}

/// <https://html.spec.whatwg.org/#windowproxy-delete>
fn trap_delete_property(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (_shim, window) = target_shim_and_window(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "If IsPlatformObjectSameOrigin(W) is true:"
    let Some(window) = window else {
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
        let has = ec.has_own_property(window, prop_key)?;
        return Ok(ec.value_from_bool(!has));
    }

    // Step 2.2: "Return ? OrdinaryDelete(W, P)."
    let prop_key = ec.to_property_key(key)?;
    ec.delete_property_or_throw(window, prop_key)?;
    Ok(ec.value_from_bool(true))
}

/// <https://html.spec.whatwg.org/#windowproxy-has>
fn trap_has(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (shim, window) = target_shim_and_window(args, ec)?;
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
    let backing = window.unwrap_or(shim);
    let result = ec.has_property(backing, prop_key)?;
    Ok(ec.value_from_bool(result))
}

/// <https://html.spec.whatwg.org/#windowproxy-ownpropertykeys>
fn trap_own_keys(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let (shim, window) = target_shim_and_window(args, ec)?;

    // Step 2: "Let maxProperties be W's associated Document's document-tree
    //          child navigables's size."
    // Note: Child navigable support not yet implemented — keys is empty.
    // Step 3: "Let keys be the range 0 to maxProperties, exclusive."
    // Step 4: "If IsPlatformObjectSameOrigin(W) is true, then return the
    //           concatenation of keys and OrdinaryOwnPropertyKeys(W)."
    // Step 5: "Return the concatenation of keys and !
    //           CrossOriginOwnPropertyKeys(W)."
    // Note: Remote targets resolve the shim's own keys (empty; the
    // cross-origin member set lives on the shim's prototype).
    let backing = window.unwrap_or(shim);
    let window_keys = ec.own_property_keys(backing)?;
    let key_array = ec.create_empty_array();
    for val in window_keys.into_iter() {
        let js_val = ec.value_from_property_key(val);
        ec.array_push(&key_array, js_val)?;
    }
    Ok(<crate::js::Types as JsTypes>::value_from_object(key_array))
}

/// Captures for the wrapper function created by `trap_get`.
///
/// Stores the Window target (to use as `this` in the wrapped call) and
/// the original callable value (to invoke with the corrected `this`).
#[gc_struct]
struct WindowProxyGetCapture {
    /// The Window to use as `this` when calling the wrapped function.
    window: JsObject,
    /// The original callable function object to invoke.
    original_fn: JsObject,
}

/// Behaviour function for the wrapper created by `trap_get`.
///
/// Ignores `this` (which is the WindowProxy) and calls the original
/// function with `this` set to the captured Window.
fn window_proxy_get_wrapper_behaviour(
    args: &[JsValue],
    _this: JsValue,
    captures: &WindowProxyGetCapture,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let this_value = <Types as JsTypes>::value_from_object(captures.window.clone());
    ec.call(&captures.original_fn, &this_value, args)
}

/// A caller-realm `postMessage` for the WindowProxy: runs the window post
/// message steps with the target navigable's id, so the message source is
/// the caller's settings object and the user agent routes the message.
///
/// <https://html.spec.whatwg.org/#window-post-message-steps>
fn create_caller_realm_post_message(
    navigable_id: NavigableId,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    #[gc_struct]
    struct PostMessageCapture {
        #[ignore_trace]
        navigable_id: NavigableId,
    }

    fn post_message_behaviour(
        args: &[JsValue],
        _this: JsValue,
        captures: &PostMessageCapture,
        ec: &mut dyn ExecutionContext<crate::js::Types>,
    ) -> Completion<JsValue, crate::js::Types> {
        let undefined = ec.value_undefined();
        let message = args.first().cloned().unwrap_or_else(|| undefined.clone());
        let options = crate::js::bindings::html::window::parse_post_message_options(args, ec)?;
        crate::html::window_post_message_steps(captures.navigable_id, message, options, ec)?;
        Ok(ec.value_undefined())
    }

    let name_key = ec.property_key_from_str("postMessage");
    let builtin = create_builtin_fn_with_traced_captures(
        ec,
        PostMessageCapture { navigable_id },
        post_message_behaviour,
        1,
        name_key,
        false,
    );
    Ok(<Types as JsTypes>::value_from_object(
        <Types as JsTypes>::object_from_function(builtin),
    ))
}

// ────────────────────────────────────────────────────────────────────────────
// The WindowProxy shim and the ECMAScript Proxy wrapping it
// ────────────────────────────────────────────────────────────────────────────

/// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
#[gc_struct]
pub struct WindowProxy {
    /// <https://html.spec.whatwg.org/#navigable>
    /// The navigable whose active window this proxy exposes.
    #[ignore_trace]
    pub target_navigable_id: NavigableId,

    /// The target navigable's active Window object when it lives in this
    /// content process; `None` when the window is remote (the navigable's
    /// document was created in another content process) or not yet resolved.
    ///
    /// The handle is deliberately a rooted (non-traced) `JsObject` rather
    /// than a `GcCell` edge: the WindowProxy's backing must stay usable
    /// across the navigation-commit garbage collection that runs when the
    /// old document is destroyed, and a cppgc-traced edge held in a cell is
    /// not reliably usable after a full collection on the V8 backend (the
    /// materialized handle can point at a swept object).  A root keeps the
    /// window alive for exactly as long as the proxy references it, and the
    /// navigation-commit severing clears it (releasing the root) once the
    /// navigable's document is created in another content process.
    #[ignore_trace]
    pub local_window: Option<JsObject>,
}

impl WindowProxy {
    pub(crate) fn new(
        target_navigable_id: NavigableId,
        local_window: Option<JsObject>,
        _ec: &mut dyn ExecutionContext<Types>,
    ) -> Self {
        Self {
            target_navigable_id,
            local_window,
        }
    }

    /// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
    pub(crate) fn target_navigable_id(&self) -> NavigableId {
        self.target_navigable_id
    }

    /// Resolve the local Window backing this proxy, when the target navigable
    /// lives in this content process.
    pub(crate) fn local_window(&self, _ec: &mut dyn ExecutionContext<Types>) -> Option<JsObject> {
        self.local_window.clone()
    }

    /// Sever or re-point the local Window backing this proxy.  Navigation
    /// commit calls this with `None` when the navigable's active document was
    /// created in another content process (the WindowProxy becomes a remote
    /// shim), or with the new Window when the navigation stays in this
    /// process.
    pub(crate) fn set_local_window(&mut self, local_window: Option<JsObject>) {
        self.local_window = local_window;
    }
}

/// Create (or fetch from the realm's cache) the WindowProxy for a navigable:
/// an ECMAScript Proxy wrapping the cached [`WindowProxy`] shim for the
/// navigable.  `local_window` seeds the same-process backing when the shim
/// does not exist yet; navigation commit later severs or re-points it.
///
/// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
pub(crate) fn create_window_proxy(
    navigable_id: NavigableId,
    local_window: Option<JsObject>,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    let (cached_shim, cached_identity) = with_global_scope(ec, |global_scope, ec| {
        Ok(global_scope.cached_window_proxy_state(navigable_id, ec))
    })?;

    // Seed the shim's local backing when a same-process Window is known and
    // the shim has no backing yet (e.g. the navigable's document was created
    // in this process after the shim existed without one).
    if cached_shim.is_some() && local_window.is_some() {
        with_global_scope(ec, |global_scope, ec| {
            global_scope.set_window_proxy_backing(navigable_id, local_window.clone(), ec);
            Ok(())
        })?;
    }

    if let Some(identity) = cached_identity {
        return Ok(<Types as JsTypes>::value_from_object(identity));
    }

    let shim = match cached_shim {
        Some(shim) => shim,
        None => {
            let shim = create_interface_instance::<Types, WindowProxy>(
                WindowProxy::new(navigable_id, local_window, ec),
                ec,
            )?;
            with_global_scope(ec, |global_scope, ec| {
                global_scope.cache_window_proxy_shim(navigable_id, shim.clone(), ec);
                Ok(())
            })?;
            shim
        }
    };

    let proxy = create_window_proxy_for_shim(shim, ec)?;
    with_global_scope(ec, |global_scope, ec| {
        global_scope.cache_window_proxy_identity(navigable_id, proxy.clone(), ec);
        Ok(())
    })?;
    Ok(<Types as JsTypes>::value_from_object(proxy))
}

/// Wrap the shim in the ECMAScript Proxy that implements the WindowProxy
/// exotic object's internal methods.
///
/// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
fn create_window_proxy_for_shim(
    shim: JsObject,
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

    let proxy = ec.create_proxy(shim, handler)?;
    Ok(proxy)
}

/// Create the WindowProxy for a navigable and return the JS object handle
/// (used when the WindowProxy is embedded in another platform object, e.g.
/// MessageEvent's source).
pub(crate) fn window_proxy_object(
    navigable_id: NavigableId,
    local_window: Option<JsObject>,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsObject, Types> {
    let value = create_window_proxy(navigable_id, local_window, ec)?;
    <Types as JsTypes>::value_as_object(&value)
        .ok_or_else(|| ec.new_type_error("WindowProxy is not an object"))
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
