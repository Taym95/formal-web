use crate::js::Types;
use js_engine::{ExecutionContext, JsTypes};

type JsValue = <Types as JsTypes>::JsValue;

/// <https://html.spec.whatwg.org/#concept-relevant-realm>
// Note: The HTML spec defines the relevant realm of a platform object as the
// value of its [[Realm]] field and then reads that realm's
// [[GlobalEnv]].[[GlobalThisValue]] directly, calling into ECMA-262 instead
// of going through Web IDL.  This helper implements that JS-side read for
// the platform object's own realm: the getters that use it (e.g. the
// `self` getter) run in that realm, so the current realm's global object is
// the value of `[[GlobalEnv]].[[GlobalThisValue]]` (ECMA-262
// SetRealmGlobalObject installs the realm's global object as the global
// environment's this value).
pub(crate) fn relevant_realm_global_this_value(ec: &mut dyn ExecutionContext<Types>) -> JsValue {
    <Types as JsTypes>::value_from_object(ec.realm_global_object())
}
