use crate::html::{MessageChannel, MessagePort};
use crate::js::Types;
use crate::webidl::bindings::{AttributeDef, InterfaceDefinition, OperationDef, WebIdlInterface};
use crate::webidl::{callback_function_value, nullable_value};
use js_engine::{Completion, ExecutionContext, JsTypes};

type JsValue = <Types as JsTypes>::JsValue;

fn with_message_port_ref(
    this: &JsValue,
    ec: &mut dyn ExecutionContext<Types>,
    f: impl FnOnce(&MessagePort, &mut dyn ExecutionContext<Types>) -> Completion<JsValue, Types>,
) -> Completion<JsValue, Types> {
    let object = Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("MessagePort receiver is not an object"))?;
    let port = ec
        .with_object_any(&object)
        .and_then(|data| data.downcast_ref::<MessagePort>().cloned());
    let Some(port) = port else {
        return Err(ec.new_type_error("receiver is not a MessagePort"));
    };
    f(&port, ec)
}

impl WebIdlInterface<Types> for MessagePort {
    const NAME: &'static str = "MessagePort";

    fn parent_name() -> Option<&'static str> {
        Some("EventTarget")
    }

    fn define_members(def: &mut InterfaceDefinition<Types>) {
        def.add_operation(OperationDef {
            id: "postMessage",
            length: 1,
            method: post_message,
            static_: false,
            unforgeable: false,
            promise_type: false,
            exposed: None,
        });
        def.add_operation(OperationDef {
            id: "start",
            length: 0,
            method: start,
            static_: false,
            unforgeable: false,
            promise_type: false,
            exposed: None,
        });
        def.add_operation(OperationDef {
            id: "close",
            length: 0,
            method: close,
            static_: false,
            unforgeable: false,
            promise_type: false,
            exposed: None,
        });
        def.add_attribute(AttributeDef {
            id: "onmessage",
            getter: get_onmessage,
            setter: Some(set_onmessage),
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
            id: "onmessageerror",
            getter: get_onmessageerror,
            setter: Some(set_onmessageerror),
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
            id: "onclose",
            getter: get_onclose,
            setter: Some(set_onclose),
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

impl WebIdlInterface<Types> for MessageChannel {
    const NAME: &'static str = "MessageChannel";

    fn create_platform_object(
        _new_target: &JsValue,
        _args: &[JsValue],
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Completion<Self, Types> {
        MessageChannel::new_channel(ec)
    }

    fn define_members(def: &mut InterfaceDefinition<Types>) {
        def.add_attribute(AttributeDef {
            id: "port1",
            getter: get_port1,
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
            id: "port2",
            getter: get_port2,
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

/// <https://html.spec.whatwg.org/#dom-messageport-postmessage>
/// The two overloads: `postMessage(message, transfer)` (a sequence of
/// transferable objects) and `postMessage(message, options)` (a
/// StructuredSerializeOptions dictionary).  Web IDL overload resolution
/// picks the sequence form when the second argument is an array.
fn post_message(
    this: &JsValue,
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    let undefined = ec.value_undefined();
    let message = args.first().cloned().unwrap_or(undefined);
    // <https://webidl.spec.whatwg.org/#dfn-overload-resolution>
    // The `postMessage(message, transfer)` overload takes a sequence; the
    // `postMessage(message, options)` overload a dictionary.  An array
    // argument matches the sequence form, any other object the dictionary
    // form.
    let is_sequence_form = match args.get(1).and_then(Types::value_as_object) {
        Some(second_object) => ec.is_array(&Types::value_from_object(second_object))?,
        None => false,
    };
    let transfer = if is_sequence_form {
        parse_transfer_sequence(args.get(1), ec)?
    } else {
        options_dict_transfer(args.get(1), ec)?
    };
    let source_object = Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("MessagePort receiver is not an object"))?;
    let object = source_object.clone();
    with_message_port_ref(this, ec, move |port, ec| {
        port.post_message(message, transfer, object, ec)?;
        Ok(ec.value_undefined())
    })
}

/// Read the `transfer` member (a `sequence<object>`) from the
/// StructuredSerializeOptions dictionary.
fn options_dict_transfer(
    dict: Option<&JsValue>,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<Vec<JsValue>, Types> {
    let Some(dict) = dict else {
        return Ok(Vec::new());
    };
    let Some(object) = Types::value_as_object(dict) else {
        return Ok(Vec::new());
    };
    let key_pk = ec.property_key_from_str("transfer");
    let value = ExecutionContext::get(ec, object, key_pk)?;
    parse_transfer_sequence(Some(&value), ec)
}

/// Convert the `transfer` argument (a `sequence<object>`) to a list of
/// values per Web IDL: an absent or `undefined` value converts to the
/// default empty sequence, a non-object or non-iterable value throws a
/// TypeError, and each iterated element must be an object.
/// <https://webidl.spec.whatwg.org/#es-sequence>
fn parse_transfer_sequence(
    transfer_value: Option<&JsValue>,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<Vec<JsValue>, Types> {
    let Some(transfer_value) = transfer_value else {
        return Ok(Vec::new());
    };
    if Types::value_is_undefined(transfer_value) {
        return Ok(Vec::new());
    }
    if Types::value_as_object(transfer_value).is_none() {
        // Step 1: If V is not an Object, throw a TypeError.
        return Err(ec.new_type_error("transfer is not an object"));
    }
    // Step 2: Let method be ? GetMethod(V, %Symbol.iterator%).
    // Step 3: If method is undefined, throw a TypeError.
    // Step 4: Return the result of creating a sequence from V and method.
    let mut iterator =
        ec.get_iterator(transfer_value.clone(), js_engine::IteratorKind::Sync, None)?;
    let mut transfer = Vec::new();
    loop {
        let next = ec.iterator_step_value(&mut iterator)?;
        let Some(next) = next else {
            break;
        };
        // <https://webidl.spec.whatwg.org/#es-object>
        // Each element converts to the `object` IDL type.
        if Types::value_as_object(&next).is_none() {
            return Err(ec.new_type_error("transfer element is not an object"));
        }
        transfer.push(next);
    }
    Ok(transfer)
}

/// <https://html.spec.whatwg.org/#dom-messageport-start>
fn start(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    with_message_port_ref(this, ec, |port, ec| {
        port.start(ec);
        Ok(ec.value_undefined())
    })
}

/// <https://html.spec.whatwg.org/#dom-messageport-close>
fn close(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    with_message_port_ref(this, ec, |port, ec| {
        port.close(ec)?;
        Ok(ec.value_undefined())
    })
}

/// <https://html.spec.whatwg.org/#event-handler-idl-attributes>
fn event_handler_getter(
    this: &JsValue,
    ec: &mut dyn ExecutionContext<Types>,
    event_type: &str,
) -> Completion<JsValue, Types> {
    let object = Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("MessagePort receiver is not an object"))?;
    let port = ec
        .with_object_any(&object)
        .and_then(|data| data.downcast_ref::<MessagePort>().cloned());
    let Some(port) = port else {
        return Ok(ec.value_null());
    };
    let callback = port.event_target.event_handler_value(event_type, ec);
    Ok(callback
        .map(|callback| callback.to_js_value())
        .unwrap_or_else(|| ec.value_null()))
}

/// <https://html.spec.whatwg.org/#event-handler-idl-attributes>
/// The onmessage setter also enables the port message queue the first time
/// it is set (as if start() had been called).
fn event_handler_setter(
    this: &JsValue,
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
    event_type: &str,
    enable_queue: bool,
) -> Completion<JsValue, Types> {
    let object = Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("MessagePort receiver is not an object"))?;
    let callback = nullable_value(
        args.first().unwrap_or(&ec.value_undefined()),
        ec,
        callback_function_value,
    )?;
    let port = ec
        .with_object_any(&object)
        .and_then(|data| data.downcast_ref::<MessagePort>().cloned());
    let Some(port) = port else {
        return Ok(ec.value_undefined());
    };
    let previous = port.event_target.event_handler_value(event_type, ec);
    if let Some(previous) = previous {
        port.event_target
            .remove_event_listener_entry(event_type, &previous, false, ec);
    }
    if let Some(callback) = callback.clone() {
        port.event_target.add_event_listener(
            port.event_target.clone(),
            event_type.to_owned(),
            Some(callback),
            false,
            false,
            Some(false),
            None,
            ec,
        );
    }
    port.event_target
        .set_event_handler_value(event_type, callback, ec);
    if enable_queue {
        // <https://html.spec.whatwg.org/#message-ports>
        // "The first time a MessagePort object's onmessage IDL attribute is
        // set, the port's port message queue must be enabled, as if the
        // start() method had been called."
        port.enable_queue(ec);
    }
    Ok(ec.value_undefined())
}

fn get_onmessage(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    event_handler_getter(this, ec, "message")
}

fn set_onmessage(
    this: &JsValue,
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    event_handler_setter(this, args, ec, "message", true)
}

fn get_onmessageerror(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    event_handler_getter(this, ec, "messageerror")
}

fn set_onmessageerror(
    this: &JsValue,
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    event_handler_setter(this, args, ec, "messageerror", false)
}

fn get_onclose(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    event_handler_getter(this, ec, "close")
}

fn set_onclose(
    this: &JsValue,
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    event_handler_setter(this, args, ec, "close", false)
}

/// <https://html.spec.whatwg.org/#dom-messagechannel-port1>
fn get_port1(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    let object = Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("MessageChannel receiver is not an object"))?;
    let channel = ec
        .with_object_any(&object)
        .and_then(|data| data.downcast_ref::<MessageChannel>().cloned());
    let Some(channel) = channel else {
        return Err(ec.new_type_error("receiver is not a MessageChannel"));
    };
    let port_object = channel
        .port1
        .object()
        .ok_or_else(|| ec.new_type_error("MessageChannel port1 is missing its object"))?;
    Ok(Types::value_from_object(port_object))
}

/// <https://html.spec.whatwg.org/#dom-messagechannel-port2>
fn get_port2(
    this: &JsValue,
    _args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    let object = Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("MessageChannel receiver is not an object"))?;
    let channel = ec
        .with_object_any(&object)
        .and_then(|data| data.downcast_ref::<MessageChannel>().cloned());
    let Some(channel) = channel else {
        return Err(ec.new_type_error("receiver is not a MessageChannel"));
    };
    let port_object = channel
        .port2
        .object()
        .ok_or_else(|| ec.new_type_error("MessageChannel port2 is missing its object"))?;
    Ok(Types::value_from_object(port_object))
}
