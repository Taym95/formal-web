type JsValue = <crate::js::Types as JsTypes>::JsValue;
type Types = crate::js::Types;

use crate::js::downcast::event_target_from_js_object;
use crate::webidl::bindings::{AttributeDef, InterfaceDefinition};
use crate::webidl::{callback_function_value, nullable_value};
use js_engine::{Completion, ExecutionContext, JsTypes};

/// <https://html.spec.whatwg.org/#getting-the-current-value-of-the-event-handler>
fn get_event_handler(
    this: &JsValue,
    ec: &mut dyn ExecutionContext<Types>,
    event_type: &str,
) -> Completion<JsValue, Types> {
    let object = Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("event handler receiver is not an object"))?;
    let callback = event_target_from_js_object(ec, &object)
        .and_then(|target| target.event_handler_value(event_type, ec));
    Ok(callback
        .map(|callback| callback.to_js_value())
        .unwrap_or_else(|| ec.value_null()))
}

/// <https://html.spec.whatwg.org/#event-handler-idl-attributes>
// Note: Setting an event handler deactivates the previous handler's listener
// and activates the new one. The listener is registered as a regular bubbling
// listener so the handler fires through the normal dispatch path; the spec's
// activation bookkeeping (onerror argument shape, once-only deactivation) is
// not yet modeled.
fn set_event_handler(
    this: &JsValue,
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
    event_type: &str,
) -> Completion<JsValue, Types> {
    let object = Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("event handler receiver is not an object"))?;
    let callback = nullable_value(
        args.get(0).unwrap_or(&ec.value_undefined()),
        ec,
        callback_function_value,
    )?;

    let Some(target) = event_target_from_js_object(ec, &object) else {
        return Err(ec.new_type_error("receiver is not an EventTarget"));
    };

    // Deactivate the previous handler, if any.
    let previous = target.event_handler_value(event_type, ec);
    if let Some(previous) = previous {
        target.remove_event_listener_entry(event_type, &previous, false, ec);
    }

    // Activate the new handler as a regular event listener.
    if let Some(callback) = callback.clone() {
        target.add_event_listener(
            target.clone(),
            event_type.to_owned(),
            Some(callback),
            false,
            false,
            Some(false),
            None,
            ec,
        );
    }

    target.set_event_handler_value(event_type, callback, ec);
    Ok(ec.value_undefined())
}

macro_rules! define_event_handler_attrs {
    ($def:ident, $(($attr:ident, $event:ident)),+ $(,)?) => {
        $(
            {
                fn get(
                    this: &JsValue,
                    _args: &[JsValue],
                    ec: &mut dyn ExecutionContext<Types>,
                ) -> Completion<JsValue, Types> {
                    get_event_handler(this, ec, stringify!($event))
                }

                fn set(
                    this: &JsValue,
                    args: &[JsValue],
                    ec: &mut dyn ExecutionContext<Types>,
                ) -> Completion<JsValue, Types> {
                    set_event_handler(this, args, ec, stringify!($event))
                }

                $def.add_attribute(AttributeDef {
                    id: stringify!($attr),
                    getter: get,
                    setter: Some(set),
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
        )+
    };
}

/// Defines the `on*` event handler IDL attributes on an interface prototype.
/// <https://html.spec.whatwg.org/#globaleventhandlers>
pub(crate) fn define_global_event_handlers(def: &mut InterfaceDefinition<Types>) {
    define_event_handler_attrs!(
        def,
        (onabort, abort),
        (onauxclick, auxclick),
        (onbeforeinput, beforeinput),
        (onbeforetoggle, beforetoggle),
        (onblur, blur),
        (oncancel, cancel),
        (oncanplay, canplay),
        (oncanplaythrough, canplaythrough),
        (onchange, change),
        (onclick, click),
        (onclose, close),
        (oncontextlost, contextlost),
        (oncontextmenu, contextmenu),
        (oncontextrestored, contextrestored),
        (oncopy, copy),
        (oncuechange, cuechange),
        (oncut, cut),
        (ondblclick, dblclick),
        (ondrag, drag),
        (ondragend, dragend),
        (ondragenter, dragenter),
        (ondragleave, dragleave),
        (ondragover, dragover),
        (ondragstart, dragstart),
        (ondrop, drop),
        (ondurationchange, durationchange),
        (onemptied, emptied),
        (onended, ended),
        (onerror, error),
        (onfocus, focus),
        (onformdata, formdata),
        (oninput, input),
        (oninvalid, invalid),
        (onkeydown, keydown),
        (onkeypress, keypress),
        (onkeyup, keyup),
        (onload, load),
        (onloadeddata, loadeddata),
        (onloadedmetadata, loadedmetadata),
        (onloadstart, loadstart),
        (onmousedown, mousedown),
        (onmouseenter, mouseenter),
        (onmouseleave, mouseleave),
        (onmousemove, mousemove),
        (onmouseout, mouseout),
        (onmouseover, mouseover),
        (onmouseup, mouseup),
        (onpaste, paste),
        (onpause, pause),
        (onplay, play),
        (onplaying, playing),
        (onprogress, progress),
        (onratechange, ratechange),
        (onreset, reset),
        (onresize, resize),
        (onscroll, scroll),
        (onscrollend, scrollend),
        (onseeked, seeked),
        (onseeking, seeking),
        (onselect, select),
        (onselectionchange, selectionchange),
        (onselectstart, selectstart),
        (onstalled, stalled),
        (onsubmit, submit),
        (onsuspend, suspend),
        (ontimeupdate, timeupdate),
        (ontoggle, toggle),
        (onvolumechange, volumechange),
        (onwaiting, waiting),
        (onwheel, wheel),
    );
}
