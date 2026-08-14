use crate::dom::DOMException;
use crate::html::windowproxy::{WindowProxy, WindowProxyBacking};
use crate::html::{Window, window_post_message_steps};
use crate::js::bindings::html::window::parse_post_message_options;
use crate::webidl::bindings::create_interface_instance;
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

/// Resolve the domain [`Window`] backing the proxy, when the target
/// navigable lives in this content process.
fn local_window_domain(
    this: &JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Option<Window> {
    let obj = <crate::js::Types as JsTypes>::value_as_object(this)?;
    let proxy = ec
        .with_object_any(&obj)
        .and_then(|data| data.downcast_ref::<WindowProxy>().cloned())?;
    let backing = proxy.backing(ec);
    match &backing {
        WindowProxyBacking::SameContentProcess { window, .. } => Some(window.clone()),
        WindowProxyBacking::CrossContentProcess => None,
    }
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
    // postMessage targets the proxy's navigable: for a same-content-process
    // window the steps run on the backing window's navigable; for a
    // cross-content-process window the message routes through the user agent.
    match local_window_domain(this, ec) {
        Some(window) => {
            window.post_message(message, options, ec)?;
        }
        None => {
            let target_navigable_id =
                with_window_proxy_ref(this, ec, |proxy| proxy.target_navigable_id())?;
            window_post_message_steps(target_navigable_id, message, options, ec)?;
        }
    }
    Ok(ec.value_undefined())
}

fn close_method(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-window-close>
    match local_window_domain(this, ec) {
        Some(window) => {
            window.close();
            Ok(ec.value_undefined())
        }
        None => {
            // The close member is available cross-origin (it is part of the
            // WindowProxy's cross-origin member set), but the target
            // navigable's window lives in another content process and
            // closing it from here is not yet implemented.
            let exception = DOMException::new(
                String::from("window.close() across content processes is not implemented"),
                String::from("NotSupportedError"),
            );
            Err(
                create_interface_instance::<crate::js::Types, DOMException>(exception, ec)
                    .map(crate::js::Types::value_from_object)
                    .unwrap_or_else(|error| error),
            )
        }
    }
}

fn focus_method(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-window-focus>
    match local_window_domain(this, ec) {
        Some(window) => {
            window.focus();
            Ok(ec.value_undefined())
        }
        None => {
            // The focus member is available cross-origin (it is part of the
            // WindowProxy's cross-origin member set), but focusing a window
            // in another content process is not yet implemented.
            let exception = DOMException::new(
                String::from("window.focus() across content processes is not implemented"),
                String::from("NotSupportedError"),
            );
            Err(
                create_interface_instance::<crate::js::Types, DOMException>(exception, ec)
                    .map(crate::js::Types::value_from_object)
                    .unwrap_or_else(|error| error),
            )
        }
    }
}

fn blur_method(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    // <https://html.spec.whatwg.org/#dom-window-blur>
    match local_window_domain(this, ec) {
        Some(window) => {
            window.blur();
            Ok(ec.value_undefined())
        }
        None => {
            // The blur member is available cross-origin (it is part of the
            // WindowProxy's cross-origin member set), but blurring a window
            // in another content process is not yet implemented.
            let exception = DOMException::new(
                String::from("window.blur() across content processes is not implemented"),
                String::from("NotSupportedError"),
            );
            Err(
                create_interface_instance::<crate::js::Types, DOMException>(exception, ec)
                    .map(crate::js::Types::value_from_object)
                    .unwrap_or_else(|error| error),
            )
        }
    }
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
    // steps.  A cross-content-process window resolves these keys through the
    // proxy's [[Get]] trap, which returns the WindowProxy itself per
    // CrossOriginGet; the member is only reachable on a proxy with a
    // same-content-process backing.
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
    // Note: The navigable target name is tracked by the user agent
    // (`traversable_target_names` in `user_agent/src/user_agent.rs`) and is
    // not sent to the content process, so a cross-content-process window
    // returns the empty string.
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
        // Note: The navigable target name is tracked by the user agent
        // (`traversable_target_names` in `user_agent/src/user_agent.rs`);
        // setting it from the content process is not yet wired.
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
    // A cross-content-process window resolves `top` through the proxy's
    // [[Get]] trap, which returns the WindowProxy itself per CrossOriginGet.
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
    // A cross-content-process window resolves `parent` through the proxy's
    // [[Get]] trap, which returns the WindowProxy itself per CrossOriginGet.
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
    // Note: The opener browsing context id is tracked by the user agent
    // (`BrowsingContext.opener_browsing_context` in
    // `user_agent/src/user_agent.rs`); the content process does not receive
    // it, so a cross-content-process window has no opener to resolve and the
    // getter returns null.
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
    // Note: A cross-content-process window's document lives in another
    // content process; there is no local Document to return.
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
    // Note: A cross-content-process window's Location object lives in
    // another content process; there is no local Location to return.
    if let Some(window) = local_window_domain(this, ec) {
        // The domain method creates the Location on first access and caches
        // its JS object on the global scope; the binding returns that cached
        // object.
        window.location_value(ec)?;
        let location_object = window
            .global_scope
            .location_object(ec)
            .ok_or_else(|| ec.new_type_error("window has no Location object"))?;
        return Ok(<crate::js::Types as JsTypes>::value_from_object(
            location_object,
        ));
    }
    Ok(ec.value_null())
}
