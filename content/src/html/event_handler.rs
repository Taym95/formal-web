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
    // the value removes the previous handler's listener and registers the
    // new one; setting null just removes it.
    let previous = event_target.event_handler_value(name, ec);
    if let Some(previous) = previous {
        event_target.remove_event_listener_entry(name, &previous, false, ec);
    }
    if let Some(callback) = callback.clone() {
        event_target.add_event_listener(
            event_target.clone(),
            name.to_owned(),
            Some(callback),
            false,
            false,
            Some(false),
            None,
            ec,
        );
    }
    event_target.set_event_handler_value(name, callback, ec);
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

/// <https://html.spec.whatwg.org/#event-handler-attributes>
pub(crate) fn sync_event_handler_content_attribute(
    event_target: &EventTarget,
    event_type: &str,
    value: Option<&str>,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<(), Types> {
    // Step 1: "If namespace is not null, or localName is not the name of an
    //          event handler content attribute on element, then return."
    // Note: Ran in the caller (sync_event_handler_content_attributes), which
    // filters the element's attributes to its `on*` content attributes.
    // Step 2: "Let eventTarget be the result of determining the target of an
    //          event handler given element and localName."
    // Note: Ran in the caller, which resolves the element's EventTarget.
    // Step 3: "If eventTarget is null, then return."
    // Step 4: "If value is null, then deactivate an event handler given
    //          eventTarget and localName."
    // Step 5: "Otherwise:"
    // Step 5.1: "If the Should element's inline behavior be blocked by
    //            Content Security Policy? algorithm returns "Blocked" when
    //            executed upon element, "script attribute", and value, then
    //            return."
    // TODO: Not yet implemented.
    // Step 5.2: "Let handlerMap be eventTarget's event handler map."
    // Step 5.3: "Let eventHandler be handlerMap[localName]."
    // Step 5.4: "Let location be the script location that triggered the
    //            execution of these steps."
    // Step 5.5: "Set eventHandler's value to the internal raw uncompiled
    //            handler value/location."
    // Note: Instead of storing an internal raw uncompiled handler, the
    // value is compiled here into a callback (see
    // compile_event_handler_content_attribute) and the event handler IDL
    // setter path below sets it as the handler's value, so the compilation
    // steps 3.7-3.12 of "getting the current value of the event handler"
    // run at attribute-change time instead of on first get.  When
    // compilation fails, the error is reported and the handler is left as
    // it was, mirroring substeps 3.7.1-3.7.4 (set the value to null without
    // deactivating, report the exception, return null) except that a
    // previously-set value is not cleared.
    // Step 5.6: "Activate an event handler given eventTarget and localName."
    // Note: Steps 5.2-5.6 run through event_handler_idl_attribute_setter,
    // which sets the handler's value and activates the event handler (or
    // deactivates it on the step 4 path).
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
