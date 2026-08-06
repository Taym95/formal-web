use crate::js::bindings::dom::event::init_flag;
use crate::ui_events::{MouseEvent, MouseEventInit};
type JsValue = <crate::js::Types as JsTypes>::JsValue;

use crate::webidl::bindings::{AttributeDef, InterfaceDefinition, WebIdlInterface};

use js_engine::{Completion, ExecutionContext, JsTypes};

fn with_mouse_event_ref<R>(
    this: &JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
    f: impl FnOnce(&MouseEvent) -> R,
) -> Completion<R, crate::js::Types> {
    let obj = crate::js::Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("MouseEvent receiver is not an object"))?;
    let mouse_event = ec
        .with_object_any(&obj)
        .and_then(|data| data.downcast_ref::<MouseEvent>().cloned());
    let Some(mouse_event) = mouse_event else {
        return Err(ec.new_type_error("receiver is not a MouseEvent"));
    };
    Ok(f(&mouse_event))
}

fn init_number(
    init: &JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
    key: &str,
    default: f64,
) -> Completion<f64, crate::js::Types> {
    if let Some(object) = crate::js::Types::value_as_object(init) {
        let property_key = ec.property_key_from_str(key);
        let value = ExecutionContext::get(ec, object, property_key)?;
        if !crate::js::Types::value_is_undefined(&value) {
            return ec.to_number(value);
        }
    }
    Ok(default)
}

fn init_flag_bool(
    init: &JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
    key: &str,
) -> Completion<bool, crate::js::Types> {
    if let Some(object) = crate::js::Types::value_as_object(init) {
        let property_key = ec.property_key_from_str(key);
        let value = ExecutionContext::get(ec, object, property_key)?;
        if !crate::js::Types::value_is_undefined(&value) {
            return Ok(ec.to_boolean(&value));
        }
    }
    Ok(false)
}

impl WebIdlInterface<crate::js::Types> for MouseEvent {
    const NAME: &'static str = "MouseEvent";

    fn parent_name() -> Option<&'static str> {
        Some("UIEvent")
    }

    fn create_platform_object(
        _new_target: &JsValue,
        args: &[JsValue],
        ec: &mut dyn ExecutionContext<crate::js::Types>,
    ) -> Completion<Self, crate::js::Types> {
        let undefined = ec.value_undefined();
        let type_ = ec.to_rust_string(args.first().cloned().unwrap_or(undefined))?;
        let init = args.get(1).cloned().unwrap_or(ec.value_undefined());
        Ok(MouseEvent::new(
            type_,
            MouseEventInit {
                bubbles: init_flag(&init, "bubbles", ec)?,
                cancelable: init_flag(&init, "cancelable", ec)?,
                composed: init_flag(&init, "composed", ec)?,
                detail: init_number(&init, ec, "detail", 0.0)? as i32,
                screen_x: init_number(&init, ec, "screenX", 0.0)?,
                screen_y: init_number(&init, ec, "screenY", 0.0)?,
                client_x: init_number(&init, ec, "clientX", 0.0)?,
                client_y: init_number(&init, ec, "clientY", 0.0)?,
                button: init_number(&init, ec, "button", 0.0)? as i16,
                buttons: init_number(&init, ec, "buttons", 0.0)? as u16,
                ctrl_key: init_flag_bool(&init, ec, "ctrlKey")?,
                shift_key: init_flag_bool(&init, ec, "shiftKey")?,
                alt_key: init_flag_bool(&init, ec, "altKey")?,
                meta_key: init_flag_bool(&init, ec, "metaKey")?,
            },
            ec,
        ))
    }

    fn define_members(def: &mut InterfaceDefinition<crate::js::Types>) {
        def.add_attribute(AttributeDef {
            id: "view",
            getter: get_view,
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
            id: "detail",
            getter: get_detail,
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
            id: "screenX",
            getter: get_screen_x,
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
            id: "screenY",
            getter: get_screen_y,
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
            id: "clientX",
            getter: get_client_x,
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
            id: "clientY",
            getter: get_client_y,
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
            id: "button",
            getter: get_button,
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
            id: "buttons",
            getter: get_buttons,
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
            id: "ctrlKey",
            getter: get_ctrl_key,
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
            id: "shiftKey",
            getter: get_shift_key,
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
            id: "altKey",
            getter: get_alt_key,
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
            id: "metaKey",
            getter: get_meta_key,
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

fn get_view(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let view = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.ui_event.view.clone())?;
    Ok(view
        .map(crate::js::Types::value_from_object)
        .unwrap_or_else(|| ec.value_null()))
}

fn get_detail(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let detail = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.ui_event.detail)?;
    Ok(ec.value_from_number(detail as f64))
}

fn get_screen_x(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let value = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.screen_x_value())?;
    Ok(ec.value_from_number(value))
}

fn get_screen_y(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let value = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.screen_y_value())?;
    Ok(ec.value_from_number(value))
}

fn get_client_x(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let value = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.client_x_value())?;
    Ok(ec.value_from_number(value))
}

fn get_client_y(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let value = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.client_y_value())?;
    Ok(ec.value_from_number(value))
}

fn get_button(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let value = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.button_value())?;
    Ok(ec.value_from_number(value as f64))
}

fn get_buttons(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let value = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.buttons_value())?;
    Ok(ec.value_from_number(value as f64))
}

fn get_ctrl_key(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let value = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.ctrl_key_value())?;
    Ok(ec.value_from_bool(value))
}

fn get_shift_key(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let value = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.shift_key_value())?;
    Ok(ec.value_from_bool(value))
}

fn get_alt_key(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let value = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.alt_key_value())?;
    Ok(ec.value_from_bool(value))
}

fn get_meta_key(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let value = with_mouse_event_ref(this, ec, |mouse_event| mouse_event.meta_key_value())?;
    Ok(ec.value_from_bool(value))
}
