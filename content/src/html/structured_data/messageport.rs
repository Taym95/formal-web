//! The MessagePort-specific parts of the safe passing of structured data:
//! recognizing a transferable MessagePort (the [[Detached]]-slot check of
//! StructuredSerializeWithTransfer step 2.1), running its transfer steps and
//! building its data holder (step 5.2), and rebuilding the port on the
//! receiving side (StructuredDeserializeWithTransfer step 3.2).  The generic
//! algorithms live in [`super::safe_passing_of_structured_data`]; the
//! wire-format data holders (`PortTransferData`, `PortMessagePayload`) live
//! in `ipc_messages::safe_passing_of_structured_data` so a transfer can
//! cross processes.

use ipc_messages::safe_passing_of_structured_data::{PortTransferData, TransferDataHolder};

use crate::html::MessagePort;
use crate::html::structured_data::safe_passing_of_structured_data::Transferable;

use js_engine::{Completion, ExecutionContext, JsTypes};

type Types = crate::js::Types;
type JsObject = <Types as JsTypes>::JsObject;
type JsValue = <Types as JsTypes>::JsValue;

/// <https://html.spec.whatwg.org/#message-ports:transfer-steps>
/// (StructuredSerializeWithTransfer step 5.2's MessagePort branch).
pub(crate) fn transfer_steps(
    object: &JsObject,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<TransferDataHolder, Types> {
    // StructuredSerializeWithTransfer step 5.2: "Otherwise, perform the
    // transfer steps for the interface identified by interfaceName."
    // Note: Only MessagePort is transferable here.  The port is read out of
    // its platform object, its data holder is built, and the transfer steps
    // run on the port (`Transferable::transfer_steps`), which drain the
    // port's record and queue into the holder — the source object's
    // [[Detached]] state is implicit in the record having left this realm.
    let port: MessagePort = ec
        .with_object_any(object)
        .and_then(|data| data.downcast_ref::<MessagePort>().cloned())
        .ok_or_else(|| crate::webidl::data_clone_error_value(ec))?;
    let mut data_holder = PortTransferData {
        port_id: port.port_id,
        queue: Vec::new(),
        remote_port: None,
        in_flight: 0,
    };
    port.transfer_steps(&mut data_holder, ec)?;
    Ok(TransferDataHolder::MessagePort(data_holder))
}

/// <https://html.spec.whatwg.org/#message-ports:transfer-receiving-steps>
/// (StructuredDeserializeWithTransfer step 3.2's MessagePort branch).
pub(crate) fn transfer_receiving_steps(
    data_holder: &PortTransferData,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsValue, Types> {
    // StructuredDeserializeWithTransfer step 3.2: "run the transfer-receiving
    // steps for MessagePort given dataHolder and a new MessagePort in
    // targetRealm."
    // Note: The new port is created in the current realm (the target realm)
    // first, then the transfer-receiving steps run on it
    // (`Transferable::transfer_receiving_steps`), which re-entangle it with
    // the remote port and register it with the user agent; the port's
    // platform object is the deserialized value.
    let port = MessagePort::new_port_with_id(data_holder.port_id, ec)?;
    port.transfer_receiving_steps(data_holder, ec)?;
    let object = port
        .object()
        .ok_or_else(|| crate::webidl::data_clone_error_value(ec))?;
    Ok(Types::value_from_object(object))
}

/// Whether a transferable is a MessagePort platform object, which has a
/// [[Detached]] internal slot and therefore satisfies the check of
/// StructuredSerializeWithTransfer step 2.1.
pub(crate) fn is_transferable_platform_object(
    object: &JsObject,
    ec: &mut dyn ExecutionContext<Types>,
) -> bool {
    ec.with_object_any(object)
        .and_then(|data| data.downcast_ref::<MessagePort>().cloned())
        .is_some()
}
