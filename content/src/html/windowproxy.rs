//! <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
//!
//! Two WindowProxy mechanisms coexist:
//!
//! - **The V8 Proxy** ([`create_window_proxy`]) wraps the target Window in a
//!   real ECMAScript Proxy whose traps delegate property access to it.  It is
//!   the right mechanism for windows in the same content process (same agent
//!   cluster, cross realm): property gets/sets resolve against the local
//!   Window.  Cross-realm property access in V8 is gated by the context
//!   security token (see the README); invoking the target realm's native
//!   bindings from the caller's realm is not yet safe.
//!
//! - **The shim** ([`WindowProxy`], [`create_window_proxy_shim`]) is a
//!   business-logic platform object tied to the navigable rather than to a
//!   document.  It is the mechanism for windows in another content process
//!   (different agent cluster): it carries the target navigable's id and
//!   forwards operations (postMessage) through the user agent, running them
//!   in the caller's realm.  It is reused per (realm, navigable) so
//!   `event.source === iframe.contentWindow` holds.

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
// The V8 Proxy WindowProxy (same-content-process targets)
// ────────────────────────────────────────────────────────────────────────────

//
// Each trap is called by the Proxy internal methods and receives:
//     fn(args: &[JsValue], _this: JsValue, ec: &mut dyn ExecutionContext<crate::js::Types>)
//         -> Completion<JsValue, crate::js::Types>
//
// Per the ECMAScript Proxy internal methods (10.5), the **target** is always
// the first argument (`args[0]`).  Since the WindowProxy is created with the
// Window as the proxy target, `args[0]` IS the Window in every trap call.
//
// These functions are used as built-in function behaviours: each is wrapped
// with `ec.create_builtin_fn()` and set as a property on the handler
// object passed to `ec.create_proxy()`.

/// <https://html.spec.whatwg.org/#windowproxy-getprototypeof>
fn trap_get_prototype_of(
    _args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let win = target_window(_args, ec)?;

    // Step 2: "If IsPlatformObjectSameOrigin(W) is true, then return !
    //           OrdinaryGetPrototypeOf(W)."
    let proto = ec.get_prototype_of(win)?;
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
    let win = target_window(args, ec)?;
    let val = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 1: "Return ! SetImmutablePrototype(this, V)."
    let current = ec.get_prototype_of(win)?;
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

/// <https://html.spec.whatwg.org/#windowproxy-defineownproperty>
fn trap_define_property(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let win = target_window(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());
    let desc_obj_val = args.get(2).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "If IsPlatformObjectSameOrigin(W) is true:"
    // Step 2.1: "If P is an array index property name, return false."
    if is_array_index_key(&key, ec) {
        return Ok(ec.value_from_bool(false));
    }

    // Step 2.2: "Return ? OrdinaryDefineOwnProperty(W, P, Desc)."
    let desc_obj = ec.to_object(desc_obj_val)?;
    let desc = ec.to_property_descriptor(desc_obj)?;
    let prop_key = ec.to_property_key(key)?;
    match ec.define_property_or_throw(win, prop_key, desc) {
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
    let win = target_window(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "Check if an access between two browsing contexts should be
    //           reported, given the current global object's browsing context,
    //           W's browsing context, P, and the current settings object."
    // Note: Access reporting is not yet implemented.
    // Step 3: "If IsPlatformObjectSameOrigin(W) is true, then return ?
    //           OrdinaryGet(this, P, Receiver)."
    // Note: postMessage runs in the caller's realm (the incumbent settings
    // object) with the target navigable's id, exactly like the shim's member;
    // invoking the target realm's native binding through `ec.call` is not
    // safe in V8 (see the module doc).
    let caller_realm_post_message = match key.as_string() {
        Some(s) if s == "postMessage" => ec
            .with_object_any(&win)
            .and_then(|data| data.downcast_ref::<Window>())
            .and_then(|window| window.global_scope.source_navigable_id())
            .map(|navigable_id| create_caller_realm_post_message(win.clone(), navigable_id, ec))
            .transpose()?,
        _ => None,
    };
    let prop_key = ec.to_property_key(key)?;
    let win_val = <crate::js::Types as JsTypes>::value_from_object(win.clone());
    let result = ec.get_v(win_val, prop_key)?;
    if let Some(post_message) = caller_realm_post_message {
        return Ok(post_message);
    }

    // Note: Wrap callable results so they are invoked with `this` = the
    // Window target, not the WindowProxy.  The Proxy [[Get]] returns
    // trapResult, but the subsequent Call expression uses the base object
    // (the Proxy) as `this`, and resolve_window cannot extract the Window
    // from a Proxy.
    if let Some(func_obj) = <Types as JsTypes>::value_as_object(&result) {
        if ec.is_callable(&result) {
            let name_key = ec.property_key_from_str("wrapped");
            let wrapper_fn = create_builtin_fn_with_traced_captures(
                ec,
                WindowProxyGetCapture {
                    window: win.clone(),
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
    }

    Ok(result)
}

/// <https://html.spec.whatwg.org/#windowproxy-set>
fn trap_set(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let win = target_window(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "Check if an access between two browsing contexts should be
    //           reported, given the current global object's browsing context,
    //           W's browsing context, P, and the current settings object."
    // Note: Access reporting is not yet implemented.
    // Step 3: "If IsPlatformObjectSameOrigin(W) is true:"
    // Step 3.1: "If P is an array index property name, return false."
    if is_array_index_key(&key, ec) {
        return Ok(ec.value_from_bool(false));
    }

    // Step 3.2: "Return ? OrdinarySet(W, P, V, Receiver)."
    let value = args.get(2).cloned().unwrap_or_else(|| ec.value_undefined());
    let prop_key = ec.to_property_key(key)?;
    ec.set(win, prop_key, value, false)?;
    Ok(ec.value_from_bool(true))
}

/// <https://html.spec.whatwg.org/#windowproxy-delete>
fn trap_delete_property(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let win = target_window(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Step 2: "If IsPlatformObjectSameOrigin(W) is true:"
    // Step 2.1: "If P is an array index property name:"
    if is_array_index_key(&key, ec) {
        let prop_key = ec.to_property_key(key)?;

        // Step 2.1.1: "Let desc be ! this.[[GetOwnProperty]](P)."
        // Uses has_own_property as proxy for "desc is undefined".
        // Step 2.1.2: "If desc is undefined, then return true."
        // Step 2.1.3: "Return false."
        let has = ec.has_own_property(win, prop_key)?;
        return Ok(ec.value_from_bool(!has));
    }

    // Step 2.2: "Return ? OrdinaryDelete(W, P)."
    let prop_key = ec.to_property_key(key)?;
    ec.delete_property_or_throw(win, prop_key)?;
    Ok(ec.value_from_bool(true))
}

/// <https://html.spec.whatwg.org/#windowproxy-has>
fn trap_has(
    args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let win = target_window(args, ec)?;
    let key = args.get(1).cloned().unwrap_or_else(|| ec.value_undefined());

    // Note: The WindowProxy spec does not override [[HasProperty]].  This
    // trap is provided for completeness.  "length" returns true (child
    // frame count); all other keys delegate to the target's [[HasProperty]].
    if let Some(s) = key.as_string() {
        if s == "length" {
            return Ok(ec.value_from_bool(true));
        }
    }

    let prop_key = ec.to_property_key(key)?;
    let result = ec.has_property(win, prop_key)?;
    Ok(ec.value_from_bool(result))
}

/// <https://html.spec.whatwg.org/#windowproxy-ownpropertykeys>
fn trap_own_keys(
    _args: &[JsValue],
    _this: JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let win = target_window(_args, ec)?;

    // Step 2: "Let maxProperties be W's associated Document's document-tree
    //          child navigables's size."
    // Note: Child navigable support not yet implemented — keys is empty.
    // Step 3: "Let keys be the range 0 to maxProperties, exclusive."
    // Step 4: "If IsPlatformObjectSameOrigin(W) is true, then return the
    //           concatenation of keys and OrdinaryOwnPropertyKeys(W)."
    let window_keys = ec.own_property_keys(win)?;
    let key_array = ec.create_empty_array();
    for val in window_keys.into_iter() {
        let js_val = ec.value_from_property_key(val);
        ec.array_push(&key_array, js_val)?;
    }
    Ok(<crate::js::Types as JsTypes>::value_from_object(key_array))
}

/// Extract the target Window from the proxy trap arguments.
///
/// The proxy target IS W (the Window object), passed as `args[0]` by the
/// ECMAScript Proxy internal methods (10.5).
fn target_window(
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Result<JsObject, JsValue> {
    args.first()
        .and_then(|value| <Types as JsTypes>::value_as_object(value))
        .ok_or_else(|| ec.value_undefined())
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

/// A caller-realm `postMessage` for the V8 Proxy: runs the window post
/// message steps with the target navigable's id, so the message source is
/// the caller's settings object and the user agent routes the message.
///
/// <https://html.spec.whatwg.org/#window-post-message-steps>
fn create_caller_realm_post_message(
    window: JsObject,
    navigable_id: NavigableId,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    #[gc_struct]
    struct PostMessageCapture {
        window: JsObject,
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
        PostMessageCapture {
            window,
            navigable_id,
        },
        post_message_behaviour,
        1,
        name_key,
        false,
    );
    Ok(<Types as JsTypes>::value_from_object(
        <Types as JsTypes>::object_from_function(builtin),
    ))
}

/// Create the V8 Proxy WindowProxy for a target Window that lives in this
/// content process.
///
/// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
pub(crate) fn create_window_proxy(
    window: &JsObject,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
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

    let proxy = ec.create_proxy(window.clone(), handler)?;
    Ok(<crate::js::Types as JsTypes>::value_from_object(proxy))
}

// ────────────────────────────────────────────────────────────────────────────
// The WindowProxy shim (cross-content-process targets)
// ────────────────────────────────────────────────────────────────────────────

/// <https://html.spec.whatwg.org/#the-windowproxy-exotic-object>
#[gc_struct]
pub struct WindowProxy {
    /// <https://html.spec.whatwg.org/#navigable>
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
pub(crate) fn create_window_proxy_shim(
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
