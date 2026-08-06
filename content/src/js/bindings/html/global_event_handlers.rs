type JsValue = <crate::js::Types as JsTypes>::JsValue;
type Types = crate::js::Types;

use crate::html::event_handler::{
    event_handler_idl_attribute_getter, event_handler_idl_attribute_setter,
};
use crate::js::downcast::event_target_from_js_object;
use crate::webidl::bindings::{AttributeDef, InterfaceDefinition};
use crate::webidl::{callback_function_value, nullable_value};
use js_engine::{Completion, ExecutionContext, JsTypes};

fn get_event_handler(
    this: &JsValue,
    ec: &mut dyn ExecutionContext<Types>,
    event_type: &str,
) -> Completion<JsValue, Types> {
    let object = Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("event handler receiver is not an object"))?;
    let Some(event_target) = event_target_from_js_object(ec, &object) else {
        return Ok(ec.value_null());
    };
    let callback = event_handler_idl_attribute_getter(&event_target, event_type, ec);
    Ok(callback
        .map(|callback| callback.to_js_value())
        .unwrap_or_else(|| ec.value_null()))
}

fn set_event_handler(
    this: &JsValue,
    args: &[JsValue],
    ec: &mut dyn ExecutionContext<Types>,
    event_type: &str,
) -> Completion<JsValue, Types> {
    let object = Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("event handler receiver is not an object"))?;
    let callback = nullable_value(
        args.first().unwrap_or(&ec.value_undefined()),
        ec,
        callback_function_value,
    )?;

    let Some(event_target) = event_target_from_js_object(ec, &object) else {
        return Err(ec.new_type_error("receiver is not an EventTarget"));
    };
    event_handler_idl_attribute_setter(&event_target, event_type, callback, ec);
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
