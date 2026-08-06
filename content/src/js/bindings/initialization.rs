use js_engine::{Completion, ExecutionContext, JsTypes};

use crate::js::Types;

type JsValue = <Types as JsTypes>::JsValue;

pub(crate) fn init_flag(
    init: &JsValue,
    key: &str,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<bool, Types> {
    let Some(object) = Types::value_as_object(init) else {
        return Ok(false);
    };
    let property_key = ec.property_key_from_str(key);
    let value = ExecutionContext::get(ec, object, property_key)?;
    Ok(ec.to_boolean(&value))
}

pub(crate) fn init_number(
    init: &JsValue,
    ec: &mut dyn ExecutionContext<Types>,
    key: &str,
    default: f64,
) -> Completion<f64, Types> {
    if let Some(object) = Types::value_as_object(init) {
        let property_key = ec.property_key_from_str(key);
        let value = ExecutionContext::get(ec, object, property_key)?;
        if !Types::value_is_undefined(&value) {
            return ec.to_number(value);
        }
    }
    Ok(default)
}

pub(crate) fn init_flag_bool(
    init: &JsValue,
    ec: &mut dyn ExecutionContext<Types>,
    key: &str,
) -> Completion<bool, Types> {
    if let Some(object) = Types::value_as_object(init) {
        let property_key = ec.property_key_from_str(key);
        let value = ExecutionContext::get(ec, object, property_key)?;
        if !Types::value_is_undefined(&value) {
            return Ok(ec.to_boolean(&value));
        }
    }
    Ok(false)
}
