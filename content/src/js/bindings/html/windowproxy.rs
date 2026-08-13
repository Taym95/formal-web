use crate::html::Window;
use crate::html::window_post_message_steps;
use crate::html::windowproxy::WindowProxy;
use crate::js::bindings::html::window::parse_post_message_options;
use crate::webidl::bindings::{AttributeDef, InterfaceDefinition, OperationDef, WebIdlInterface};
use js_engine::{Completion, ExecutionContext, JsTypes};

type JsValue = <crate::js::Types as JsTypes>::JsValue;

fn with_window_proxy_ref<R>(
    this: &JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
    f: impl FnOnce(&WindowProxy) -> R,
) -> Completion<R, crate::js::Types> {
    let obj = <crate::js::Types as JsTypes>::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("WindowProxy receiver is not an object"))?;
    let proxy = ec
        .with_object_any(&obj)
        .and_then(|data| data.downcast_ref::<WindowProxy>().cloned());
    let Some(proxy) = proxy else {
        return Err(ec.new_type_error("receiver is not a WindowProxy"));
    };
    Ok(f(&proxy))
}

/// Resolve the local Window backing the shim, when the target navigable
/// lives in this content process.
fn local_window_for(
    this: &JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Option<<crate::js::Types as JsTypes>::JsObject> {
    let obj = <crate::js::Types as JsTypes>::value_as_object(this)?;
    let proxy = ec
        .with_object_any(&obj)
        .and_then(|data| data.downcast_ref::<WindowProxy>().cloned())?;
    proxy.local_window(ec)
}

/// Resolve the domain [`Window`] backing the shim, when the target
/// navigable lives in this content process.
fn local_window_domain(
    this: &JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Option<Window> {
    let local_window = local_window_for(this, ec)?;
    ec.with_object_any(&local_window)
        .and_then(|data| data.downcast_ref::<Window>().cloned())
}

impl WebIdlInterface<crate::js::Types> for WindowProxy {
    const NAME: &'static str = "WindowProxy";

    fn create_platform_object(
        _new_target: &JsValue,
        _args: &[JsValue],
        ec: &mut dyn ExecutionContext<crate::js::Types>,
    ) -> Completion<Self, crate::js::Types> {
        Err(ec.new_type_error("Illegal constructor"))
    }

    fn define_members(def: &mut InterfaceDefinition<crate::js::Types>) {
        def.add_operation(OperationDef {
            id: "postMessage",
            length: 1,
            method: post_message_method,
            static_: false,
            unforgeable: false,
            promise_type: false,
            exposed: None,
        });
        def.add_operation(OperationDef {
            id: "close",
            length: 0,
            method: close_method,
            static_: false,
            unforgeable: false,
            promise_type: false,
            exposed: None,
        });
        def.add_operation(OperationDef {
            id: "focus",
            length: 0,
            method: focus_method,
            static_: false,
            unforgeable: false,
            promise_type: false,
            exposed: None,
        });
        def.add_operation(OperationDef {
            id: "blur",
            length: 0,
            method: blur_method,
            static_: false,
            unforgeable: false,
            promise_type: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "closed",
            getter: get_closed,
            setter: None,
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: None,
            legacy_lenient_setter: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "self",
            getter: get_self,
            setter: None,
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: None,
            legacy_lenient_setter: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "window",
            getter: get_self,
            setter: None,
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: None,
            legacy_lenient_setter: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "frames",
            getter: get_self,
            setter: None,
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: None,
            legacy_lenient_setter: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "name",
            getter: get_name,
            setter: Some(set_name),
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: None,
            legacy_lenient_setter: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "length",
            getter: get_length,
            setter: None,
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: None,
            legacy_lenient_setter: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "top",
            getter: get_top,
            setter: None,
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: None,
            legacy_lenient_setter: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "parent",
            getter: get_parent,
            setter: None,
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: None,
            legacy_lenient_setter: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "opener",
            getter: get_opener,
            setter: None,
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: None,
            legacy_lenient_setter: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "document",
            getter: get_document,
            setter: None,
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: None,
            legacy_lenient_setter: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "location",
            getter: get_location,
            setter: None,
            static_: false,
            unforgeable: false,
            promise_type: false,
            legacy_lenient_this: false,
            replaceable: false,
            put_forwards: Some("href"),
            legacy_lenient_setter: false,
            exposed: None,
        });
    }
}

/// <https://html.spec.whatwg.org/#dom-window-postmessage-options>
fn post_message_method(
    this: &JsValue,
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let undefined = ec.value_undefined();
    let message = args.first().cloned().unwrap_or_else(|| undefined.clone());
    let options = parse_post_message_options(args, ec)?;

    // <https://html.spec.whatwg.org/#window-post-message-steps>
    // The shim runs steps 1–7 in the caller's realm (the incumbent settings
    // object), then hands the serialized message to the user agent for step 8.
    let target_navigable_id = with_window_proxy_ref(this, ec, |proxy| proxy.target_navigable_id())?;
    window_post_message_steps(target_navigable_id, message, options, ec)?;
    Ok(ec.value_undefined())
}

fn close_method(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-window-close>
    // A remote shim (no local Window) has no closeable window in this
    // content process.
    if let Some(window) = local_window_domain(this, ec) {
        window.close();
    }
    Ok(ec.value_undefined())
}

fn focus_method(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-window-focus>
    if let Some(window) = local_window_domain(this, ec) {
        window.focus();
    }
    Ok(ec.value_undefined())
}

fn blur_method(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-window-blur>
    if let Some(window) = local_window_domain(this, ec) {
        window.blur();
    }
    Ok(ec.value_undefined())
}

fn get_closed(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-window-closed>
    let closed = local_window_domain(this, ec)
        .map(|window| window.closed_value())
        .unwrap_or(false);
    Ok(ec.value_from_bool(closed))
}

fn get_self(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-self>
    // The `window`, `frames`, and `self` members share the same getter
    // steps.  A remote shim resolves these keys through the proxy's [[Get]]
    // trap, which returns the WindowProxy itself per CrossOriginGet; the
    // member is only reachable on a shim with a local backing.
    if let Some(window) = local_window_domain(this, ec) {
        return Ok(window.self_value(ec));
    }
    Ok(this.clone())
}

fn get_name(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-name>
    // A remote shim's navigable target name is user-agent state that is not
    // available in this content process.
    let name = local_window_domain(this, ec)
        .map(|window| window.name_value())
        .unwrap_or_default();
    Ok(ec.value_from_string(ec.js_string_from_str(&name)))
}

fn set_name(
    this: &JsValue,
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-name>
    let Some(window) = local_window_domain(this, ec) else {
        // Note: A remote shim's navigable target name is user-agent state;
        // setting it is not yet wired.
        return Ok(ec.value_undefined());
    };
    let undefined = ec.value_undefined();
    let value = ec.to_rust_string(args.first().cloned().unwrap_or(undefined))?;
    window.set_name_value(value);
    Ok(ec.value_undefined())
}

fn get_length(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-length>
    let length = local_window_domain(this, ec)
        .map(|window| window.length_value())
        .unwrap_or(0);
    Ok(ec.value_from_number(length as f64))
}

fn get_top(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-top>
    // A remote shim resolves `top` through the proxy's [[Get]] trap, which
    // returns the WindowProxy itself per CrossOriginGet.
    if let Some(window) = local_window_domain(this, ec) {
        return window.top_value(ec);
    }
    Ok(this.clone())
}

fn get_parent(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-parent>
    // A remote shim resolves `parent` through the proxy's [[Get]] trap,
    // which returns the WindowProxy itself per CrossOriginGet.
    if let Some(window) = local_window_domain(this, ec) {
        return window.parent_value(ec);
    }
    Ok(this.clone())
}

fn get_opener(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-opener>
    // Note: The opener browsing context is user-agent state; a remote shim
    // has no opener in this content process.
    if let Some(window) = local_window_domain(this, ec) {
        return Ok(window.opener_value(ec));
    }
    Ok(ec.value_null())
}

fn get_document(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-document>
    // A remote shim's document lives in another content process.
    if let Some(window) = local_window_domain(this, ec) {
        return window.document_value(ec);
    }
    Ok(ec.value_null())
}

fn get_location(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-location>
    // A remote shim's Location object lives in another content process.
    if let Some(window) = local_window_domain(this, ec) {
        return window.location_value(ec);
    }
    Ok(ec.value_null())
}
