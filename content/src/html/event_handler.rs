use crate::dom::EventTarget;
use crate::js::Types;
use crate::webidl::Callback;
use js_engine::ExecutionContext;

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
    // (Event handler content attributes are not yet compiled into handlers;
    // the map only holds IDL-assigned callbacks.)
    // Step 4: Return eventHandler's value.
    event_target.event_handler_value(name, ec)
}
