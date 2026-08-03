use crate::dom::{Event, UIEvent};
type JsValue = <crate::js::Types as JsTypes>::JsValue;

fn with_ui_event_ref<R>(
    this: &JsValue,
    ec: &mut dyn ExecutionContext<crate::js::Types>,
    f: impl FnOnce(&UIEvent, &mut dyn ExecutionContext<crate::js::Types>) -> R,
) -> Completion<R, crate::js::Types> {
    let obj = crate::js::Types::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("UIEvent receiver is not an object"))?;
    // Clone the handle out of the object registry so `f` can borrow `ec`
    // mutably while the platform object is accessed. The clone shares all
    // GC-managed state with the registered platform object.
    let ui_event = ec
        .with_object_any(&obj)
        .and_then(|data| data.downcast_ref::<UIEvent>().cloned());
    let Some(ui_event) = ui_event else {
        return Err(ec.new_type_error("receiver is not a UIEvent"));
    };
    Ok(f(&ui_event, ec))
}

use crate::webidl::bindings::{AttributeDef, InterfaceDefinition, WebIdlInterface};

use super::event::init_flag;

use js_engine::{Completion, ExecutionContext, JsTypes};

impl WebIdlInterface<crate::js::Types> for UIEvent {
    const NAME: &'static str = "UIEvent";

    fn parent_name() -> Option<&'static str> {
        Some("Event")
    }

    fn create_platform_object(
        _new_target: &JsValue,
        args: &[JsValue],
        ec: &mut dyn ExecutionContext<crate::js::Types>,
    ) -> Completion<Self, crate::js::Types> {
        let undefined = ec.value_undefined();
        let type_ = ec.to_rust_string(args.first().cloned().unwrap_or(undefined))?;
        let init = args.get(1).cloned().unwrap_or(ec.value_undefined());
        let detail = if let Some(object) = crate::js::Types::value_as_object(&init) {
            let property_key = ec.property_key_from_str("detail");
            let detail_value = ExecutionContext::get(ec, object, property_key)?;
            ec.to_number(detail_value)? as i32
        } else {
            0
        };
        Ok(UIEvent {
            event: Event::new(
                type_,
                init_flag(&init, "bubbles", ec)?,
                init_flag(&init, "cancelable", ec)?,
                init_flag(&init, "composed", ec)?,
                false,
                0.0,
                ec,
            ),
            view: None,
            detail,
        })
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
    }
}

fn get_view(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let view = with_ui_event_ref(this, ec, |ui_event, _ec| ui_event.view_value())?;
    Ok(view
        .map(crate::js::Types::value_from_object)
        .unwrap_or_else(|| ec.value_null()))
}

fn get_detail(
    this: &JsValue,
    _: &[JsValue],
    ec: &mut dyn ExecutionContext<crate::js::Types>,
) -> Completion<JsValue, crate::js::Types> {
    let val = with_ui_event_ref(this, ec, |ui_event, _ec| ui_event.detail_value())?;
    Ok(ec.value_from_number(val as f64))
}
