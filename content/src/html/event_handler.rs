use crate::dom::EventTarget;
use crate::js::Types;
use crate::webidl::Callback;
use js_engine::{Completion, ExecutionContext, JsTypes};

/// <https://html.spec.whatwg.org/#event-handler-idl-attributes>
/// The getter of an event handler IDL attribute. The receiver's EventTarget
/// is resolved by the binding layer.
pub(crate) fn event_handler_idl_attribute_getter(
    event_target: &EventTarget,
    name: &str,
    ec: &mut dyn ExecutionContext<Types>,
) -> Option<Callback> {
    // Step 1: Let eventTarget be the result of determining the target of an
    //         event handler given this object and name.
    // Note: body/frameset Window reflection is not modeled; the receiver's
    // EventTarget is always the target.
    // Step 2: If eventTarget is null, then return null.
    // (The binding resolves the receiver to an EventTarget before calling.)
    // Step 3: Return the result of getting the current value of the event
    //         handler given eventTarget and name.
    get_current_value_of_event_handler(event_target, name, ec)
}

/// <https://html.spec.whatwg.org/#event-handler-idl-attributes>
/// The setter of an event handler IDL attribute. The receiver's EventTarget
/// is resolved by the binding layer.
pub(crate) fn event_handler_idl_attribute_setter(
    event_target: &EventTarget,
    name: &str,
    callback: Option<Callback>,
    ec: &mut dyn ExecutionContext<Types>,
) {
    // Step 1: Let eventTarget be the result of determining the target of an
    //         event handler given this object and name.
    // Note: body/frameset Window reflection is not modeled; the receiver's
    // EventTarget is always the target.
    // Step 2: If eventTarget is null, then return.
    // (The binding resolves the receiver to an EventTarget before calling.)
    // Step 3: If the given value is null, then deactivate an event handler
    //         given eventTarget and name.
    // Step 4: Otherwise:
    // Step 4.1: Let handlerMap be eventTarget's event handler map.
    // Step 4.2: Let eventHandler be handlerMap[name].
    // Step 4.3: Set eventHandler's value to the given value.
    // Step 4.4: Activate an event handler given eventTarget and name.
    // Note: The spec registers a single wrapper listener that runs the event
    // handler processing algorithm against the handler's current value, so
    // re-setting the value never touches the listener. Here the handler
    // callback is registered directly as the event listener, so replacing
    // the value removes the previous handler's listener (identified by the
    // previous value) before the new one is registered.
    match callback {
        None => deactivate_event_handler(event_target, name, ec),
        Some(callback) => {
            if let Some(previous) = event_target.event_handler_value(name, ec) {
                event_target.remove_event_listener_entry(name, &previous, false, ec);
            }
            event_target.set_event_handler_value(name, Some(callback.clone()), ec);
            activate_event_handler(event_target, name, &callback, ec);
        }
    }
}

/// <https://html.spec.whatwg.org/#deactivate-an-event-handler>
fn deactivate_event_handler(
    event_target: &EventTarget,
    name: &str,
    ec: &mut dyn ExecutionContext<Types>,
) {
    // Step 1: Let handlerMap be eventTarget's event handler map.
    // Step 2: Let eventHandler be handlerMap[name].
    // Step 3: Set eventHandler's value to null.
    // Step 4: Let listener be eventHandler's listener.
    // Step 5: If listener is not null, then remove an event listener with
    //         eventTarget and listener.
    // Step 6: Set eventHandler's listener to null.
    // Note: The event handler's listener is not stored separately from its
    // value (see the note in event_handler_idl_attribute_setter), so the
    // listener of step 4 is identified by the current value.
    if let Some(previous) = event_target.event_handler_value(name, ec) {
        event_target.remove_event_listener_entry(name, &previous, false, ec);
    }
    event_target.set_event_handler_value(name, None, ec);
}

/// <https://html.spec.whatwg.org/#activate-an-event-handler>
fn activate_event_handler(
    event_target: &EventTarget,
    name: &str,
    callback: &Callback,
    ec: &mut dyn ExecutionContext<Types>,
) {
    // Step 1: Let handlerMap be eventTarget's event handler map.
    // Step 2: Let eventHandler be handlerMap[name].
    // Step 3: If eventHandler's listener is not null, then return.
    // Note: The wrapper listener running the event handler processing
    // algorithm is not created; the handler callback is registered directly
    // as the event listener, so the listener identity is the callback itself
    // and re-activation re-registers it (event_handler_idl_attribute_setter
    // removes the previous listener first).  Step 3's early return therefore
    // never applies, and the listener of step 7 is never stored separately
    // from the value.
    // Step 4: Let callback be the result of creating a Web IDL EventListener
    //         instance representing a reference to a function of one argument
    //         that executes the steps of the event handler processing
    //         algorithm, given eventTarget, name, and its argument.
    // Step 5: Let listener be a new event listener whose type is the event
    //         handler event type corresponding to eventHandler and callback
    //         is callback.
    // Step 6: Add an event listener with eventTarget and listener.
    // Step 7: Set eventHandler's listener to listener.
    event_target.add_event_listener(
        event_target.clone(),
        name.to_owned(),
        Some(callback.clone()),
        false,
        false,
        Some(false),
        None,
        ec,
    );
}

/// <https://html.spec.whatwg.org/#getting-the-current-value-of-the-event-handler>
fn get_current_value_of_event_handler(
    event_target: &EventTarget,
    name: &str,
    ec: &mut dyn ExecutionContext<Types>,
) -> Option<Callback> {
    // Step 1: Let handlerMap be eventTarget's event handler map.
    // Step 2: Let eventHandler be handlerMap[name].
    // Step 3: If eventHandler's value is an internal raw uncompiled handler:
    // Note: The map never holds an internal raw uncompiled handler: event
    // handler content attributes are compiled eagerly at attribute-change
    // time (see sync_event_handler_content_attribute), so the substeps of
    // step 3 never run here.
    // Step 4: Return eventHandler's value.
    event_target.event_handler_value(name, ec)
}

/// <https://html.spec.whatwg.org/#getting-the-current-value-of-the-event-handler>
pub(crate) fn compile_event_handler_content_attribute(
    source: &str,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<Callback, Types> {
    // Step 3.7: "If body is not parsable as FunctionBody or if parsing
    //            detects an early error, then follow these substeps:"
    // Note: The substeps 3.7.1-3.7.4 (set eventHandler's value to null
    // without deactivating, report the exception, return null) run in the
    // caller sync_event_handler_content_attribute when this function fails.
    // Step 3.8: "Push settings object's realm execution context onto the
    //            JavaScript execution context stack; it is now the running
    //            JavaScript execution context."
    // Note: The engine evaluates the script in the current realm, so the
    // push/pop of steps 3.8 and 3.10 is implicit.
    // Step 3.9: "Let function be the result of calling OrdinaryFunctionCreate,
    //            with arguments: functionPrototype, %Function.prototype%,
    //            sourceText, the string formed by concatenating "function ",
    //            name, "(event) {", U+000A LF, body, U+000A LF, and "}",
    //            ParameterList, a single argument called event, body, and
    //            thisMode, non-lexical-this."
    // Note: The onerror five-argument special case of step 3.9 is not
    // modeled, so every handler is a function of one argument named `event`.
    // Step 3.10: "Remove settings object's realm execution context from the
    //             JavaScript execution context stack."  (See step 3.8.)
    // Step 3.11: "Set function.[[ScriptOrModule]] to null."
    // TODO: Not yet implemented.
    // Step 3.12: "Set eventHandler's value to the result of creating a Web
    //             IDL EventHandler callback function object whose object
    //             reference is function and whose callback context is
    //             settings object."
    // Note: The compiled function is returned as a Callback (the EventHandler
    // callback object); the caller sync_event_handler_content_attribute sets
    // it as the handler's value.
    let wrapper = format!("(function(event) {{\n{source}\n}})");
    let function = ec.evaluate_script(&wrapper)?;
    let object = <Types as JsTypes>::value_as_object(&function).ok_or_else(|| {
        ec.new_type_error("event handler content attribute did not compile to a function")
    })?;
    Ok(Callback::from_object(object, ec))
}

/// Sync an `on*` content attribute with the element's event handler: the
/// attribute change steps that synchronize between event handler content
/// attributes and event handlers (an unnamed algorithm in the spec's event
/// handler section, so there is no anchor to link).  The step-1
/// namespace/name filter and step-2 target resolution run in the caller
/// (sync_event_handler_content_attributes).
pub(crate) fn sync_event_handler_content_attribute(
    event_target: &EventTarget,
    event_type: &str,
    value: Option<&str>,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<(), Types> {
    // Attribute change steps, step 4: "If value is null, then deactivate an
    // event handler given eventTarget and localName."
    // Attribute change steps, step 5.5: "Set eventHandler's value to the
    // internal raw uncompiled handler value/location."
    // Note: Instead of storing an internal raw uncompiled handler, the value
    // is compiled here into a callback (see
    // compile_event_handler_content_attribute, which runs the compilation
    // steps 3.7-3.12 of "getting the current value of the event handler" at
    // attribute-change time).  When compilation fails, the error is
    // reported and the handler is left as it was, mirroring substeps
    // 3.7.1-3.7.4 (set the value to null without deactivating, report the
    // exception, return null) except that a previously-set value is not
    // cleared.
    // Attribute change steps, step 4 / step 5.6: deactivate the event
    // handler for a removed attribute, or set the handler's value and
    // activate it for a present one; both run through the event handler IDL
    // setter path.
    let callback = match value {
        Some(source) => match compile_event_handler_content_attribute(source, ec) {
            Ok(callback) => Some(callback),
            Err(error) => {
                ec.report_exception(error);
                return Ok(());
            }
        },
        None => None,
    };
    event_handler_idl_attribute_setter(event_target, event_type, callback, ec);
    Ok(())
}
