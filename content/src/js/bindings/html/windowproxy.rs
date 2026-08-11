use crate::html::Window;
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
    let obj = crate::js::Types::value_as_object(this)
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
    let obj = crate::js::Types::value_as_object(this)?;
    let proxy = ec
        .with_object_any(&obj)
        .and_then(|data| data.downcast_ref::<WindowProxy>().cloned())?;
    proxy.local_window(ec)
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
            put_forwards: None,
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
    crate::html::window_post_message_steps(target_navigable_id, message, options, ec)?;
    Ok(ec.value_undefined())
}

fn close_method(
    _this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    Ok(ec.value_undefined())
}

fn focus_method(
    _this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    Ok(ec.value_undefined())
}

fn blur_method(
    _this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    Ok(ec.value_undefined())
}

fn get_closed(
    _this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    Ok(ec.value_from_bool(false))
}

fn get_self(
    this: &JsValue,
    _args: &[JsValue],
    _ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    Ok(this.clone())
}

fn get_name(
    _this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#the-window-object>
    // Note: The navigable target name lives in the user agent; the local
    // window's GlobalScope does not yet track it, so the getter returns the
    // empty string.
    Ok(ec.value_from_string(ec.js_string_from_str("")))
}

fn set_name(
    _this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // Note: The navigable target name is user-agent state; setting it is not
    // yet wired.
    Ok(ec.value_undefined())
}

fn get_length(
    _this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#the-window-object>
    // Note: Document-tree child navigable tracking is not yet implemented.
    Ok(ec.value_from_number(0.0))
}

fn get_top(
    this: &JsValue,
    _args: &[JsValue],
    _ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#the-window-object>
    // Note: Resolving the top traversable's WindowProxy requires the
    // navigable hierarchy; return the proxy itself for now.
    Ok(this.clone())
}

fn get_parent(
    this: &JsValue,
    _args: &[JsValue],
    _ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#the-window-object>
    // Note: Resolving the parent navigable's WindowProxy requires the
    // navigable hierarchy; return the proxy itself for now.
    Ok(this.clone())
}

fn get_opener(
    _this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#the-window-object>
    // Note: Opener tracking is user-agent state and is not yet wired.
    Ok(ec.value_null())
}

fn get_document(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#the-window-object>
    // Note: Resolves the target window's document when the target navigable
    // lives in this content process; cross-realm property reads on the
    // returned object are subject to V8's context isolation.
    if let Some(local_window) = local_window_for(this, ec) {
        let document = ec
            .with_object_any(&local_window)
            .and_then(|data| data.downcast_ref::<Window>().cloned())
            .and_then(|window| window.global_scope.document_object(ec));
        if let Some(document) = document {
            return Ok(crate::js::Types::value_from_object(document));
        }
    }
    Ok(ec.value_null())
}

fn get_location(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#the-window-object>
    // Note: Resolves the target window's location when the target navigable
    // lives in this content process.
    if let Some(local_window) = local_window_for(this, ec) {
        let location = ec
            .with_object_any(&local_window)
            .and_then(|data| data.downcast_ref::<Window>().cloned())
            .and_then(|window| window.global_scope.location_object(ec));
        if let Some(location) = location {
            return Ok(crate::js::Types::value_from_object(location));
        }
    }
    Ok(ec.value_null())
}
