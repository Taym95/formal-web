use ipc_messages::content::{Event as ContentEvent, MessageId, PortId};
use ipc_messages::safe_passing_of_structured_data::PortMessagePayload;
use js_engine::gc_struct;
use js_engine::{Completion, ExecutionContext, JsTypes};

use crate::dom::Event;
use crate::dom::dispatch_with_path;
use crate::dom::event::{EventTarget, EventTargetAccess};
use crate::dom::simple_path;
use crate::html::safe_passing_of_structured_data::{
    SerializeWithTransferResult, structured_deserialize_with_transfer,
    structured_serialize_with_transfer,
};
use crate::js::Types;
use crate::js::platform_objects::with_global_scope;
use crate::webidl::bindings::create_interface_instance;

use super::{GlobalScope, MessageEvent, MessageEventInit};

use crate::html::channel_messaging::ChannelMessaging;

type JsValue = <Types as JsTypes>::JsValue;
type JsObject = <Types as JsTypes>::JsObject;

/// <https://html.spec.whatwg.org/#messageport>
#[gc_struct]
pub(crate) struct MessagePort {
    /// The port's EventTarget base; as the port's message event target
    /// defaults to the port itself, message and messageerror events are
    /// dispatched through this target.
    pub(crate) event_target: EventTarget,

    /// The global scope of the realm the port was created in: the port's
    /// ChannelMessaging and its IPC event sender are per-global.
    pub(crate) global_scope: GlobalScope,

    /// The id under which the user agent's channel messaging state and
    /// this realm's ChannelMessaging know the port.
    #[ignore_trace]
    pub(crate) port_id: PortId,
}

impl EventTargetAccess for MessagePort {
    fn get_event_target(&self, _ec: &mut dyn ExecutionContext<Types>) -> EventTarget {
        self.event_target.clone()
    }
}

impl MessagePort {
    /// The port's platform object, resolved through the reflector stored
    /// on the port's event target.
    pub(crate) fn object(&self) -> Option<JsObject> {
        self.event_target.reflector.clone()
    }

    /// Create a new MessagePort platform object in the current realm (the
    /// "a new MessagePort in this's relevant realm" of the MessageChannel
    /// constructor steps and of the transfer-receiving steps), with a
    /// fresh id not yet registered with the user agent.
    pub(crate) fn new_port(ec: &mut dyn ExecutionContext<Types>) -> Completion<Self, Types> {
        let global_scope = with_global_scope(ec, |global_scope, _ec| Ok(global_scope.clone()))?;
        let port = Self {
            event_target: EventTarget::new(ec),
            global_scope,
            port_id: PortId::new(),
        };
        let object = create_interface_instance::<Types, MessagePort>(port, ec)?;
        // The port returned to the caller is re-read from its wrapper,
        // whose reflector was set by the interface instance creation.
        ec.with_object_any(&object)
            .and_then(|data| data.downcast_ref::<MessagePort>().cloned())
            .ok_or_else(|| ec.new_type_error("MessagePort instance is not a MessagePort"))
    }

    /// This realm's ChannelMessaging, created on first use; `None` when
    /// the realm has no event loop yet.
    fn messaging(&self, ec: &mut dyn ExecutionContext<Types>) -> Option<ChannelMessaging> {
        self.global_scope.channel_messaging(ec)
    }

    /// <https://html.spec.whatwg.org/#message-port-post-message-steps>
    pub(crate) fn post_message(
        &self,
        message: JsValue,
        transfer: Vec<JsValue>,
        source_object: JsObject,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Completion<(), Types> {
        let Some(messaging) = self.messaging(ec) else {
            return Ok(());
        };
        let Some(event_sender) = self.global_scope.event_sender() else {
            return Ok(());
        };

        // Step 1: Let targetPort be the port with which this is entangled,
        //         if any; otherwise let it be null.
        let target_port: Option<PortId> = messaging
            .port_record(self.port_id, ec)
            .and_then(|record| record.entangled);

        // Step 2: If transfer contains sourcePort, then throw a
        //         "DataCloneError" DOMException.
        let transfer_contains_source = transfer
            .iter()
            .filter_map(Types::value_as_object)
            .any(|object| object == source_object);
        if transfer_contains_source {
            return Err(crate::webidl::data_clone_error_value(ec));
        }

        // Step 3: Let doomed be false.
        // Step 4: If targetPort is not null and transfer contains targetPort,
        //         then set doomed to true and optionally report to a developer
        //         console that the target port was posted to itself, causing
        //         the communication channel to be lost.
        // Note: The target port's object is resolved by id through the
        // records; comparing the transferred objects against the target's
        // platform object decides doom.
        let target_object: Option<JsObject> =
            target_port.and_then(|port| messaging.port_object(port, ec));
        let doomed = match target_object {
            Some(target_object) => transfer
                .iter()
                .filter_map(Types::value_as_object)
                .any(|object| object == target_object),
            None => false,
        };

        // Step 5: Let serializeWithTransferResult be
        //         StructuredSerializeWithTransfer(message, transfer). Rethrow
        //         any exceptions.
        let serialize_result = structured_serialize_with_transfer(&message, transfer, ec)?;

        // Step 6: If targetPort is null, or if doomed is true, then return.
        if target_port.is_none() || doomed {
            return Ok(());
        }

        // Step 7: Add a task that runs the following steps to the port
        //         message queue of targetPort.
        // Note: The delivery is `MessagePort.tla`'s `PostMessage` (step 7): the message goes
        // straight into the target's queue when the target is managed by
        // this same event loop, and is appended to the user agent's routing
        // queue otherwise.  The task's substeps (7.1-7.7) run when the
        // message event fires (`run_message_task`): for a routed message the
        // delivering task itself runs them, and for a directly queued
        // message the queued message task does.
        let payload = PortMessagePayload {
            message_id: MessageId::new(),
            serialized: serialize_result.serialized,
            transfer_data_holders: serialize_result.transfer_data_holders,
        };
        let Some(target_port) = target_port else {
            return Ok(());
        };
        messaging
            .post_message(self.port_id, Some(target_port), payload, &event_sender, ec)
            .map_err(|error| ec.new_type_error(&format!("postMessage: {error}")))
    }

    /// <https://html.spec.whatwg.org/#dom-messageport-start>
    pub(crate) fn start(&self, ec: &mut dyn ExecutionContext<Types>) {
        // Step 1: The start() method steps are to enable this's port
        //         message queue, if it is not already enabled.
        // Note: The enabling runs in the per-global ChannelMessaging
        // (`messaging.start`), which also requests message tasks for any
        // pending messages.
        let Some(messaging) = self.messaging(ec) else {
            return;
        };
        if let Some(event_sender) = self.global_scope.event_sender() {
            messaging.start(self.port_id, &event_sender, ec);
        }
    }

    /// <https://html.spec.whatwg.org/#dom-messageport-close>
    pub(crate) fn close(&self, ec: &mut dyn ExecutionContext<Types>) -> Completion<(), Types> {
        // Step 1: Set this's [[Detached]] internal slot value to true.
        // Step 2: If this is entangled, disentangle it.
        // Note: Steps 1-3 run in the per-global ChannelMessaging
        // (`messaging.close`), which detaches the record and returns the
        // entangled twin; step 4 of the disentangle steps (fire an event
        // named close at otherPort) runs below.
        let Some(messaging) = self.messaging(ec) else {
            return Ok(());
        };
        let other = messaging.close(self.port_id, ec);
        if let Some(other) = other {
            // Step 4: Fire an event named close at otherPort.
            if let Some(other_object) = messaging.port_object(other, ec) {
                let other_port: Option<MessagePort> = ec
                    .with_object_any(&other_object)
                    .and_then(|data| data.downcast_ref::<MessagePort>().cloned());
                if let Some(other_port) = other_port {
                    fire_close_event(&other_port, ec)?;
                }
            }
        }
        Ok(())
    }

    /// Enable the port's message queue, as when start() is called or the
    /// first onmessage handler is set.
    pub(crate) fn enable_queue(&self, ec: &mut dyn ExecutionContext<Types>) {
        let Some(messaging) = self.messaging(ec) else {
            return;
        };
        if let Some(event_sender) = self.global_scope.event_sender() {
            messaging.enable_queue(self.port_id, &event_sender, ec);
        }
    }

    /// <https://html.spec.whatwg.org/#message-port-post-message-steps>
    pub(crate) fn run_message_task(
        &self,
        time_millis: f64,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Completion<(), Types> {
        let Some(messaging) = self.messaging(ec) else {
            return Ok(());
        };
        let Some(event_sender) = self.global_scope.event_sender() else {
            return Ok(());
        };
        let payload = match messaging.pop_queued_message(self.port_id, &event_sender, ec) {
            Ok(Some(payload)) => payload,
            Ok(None) => return Ok(()),
            Err(error) => return Err(ec.new_type_error(&format!("port message task: {error}"))),
        };

        // Step 7.1: Let finalTargetPort be the MessagePort in whose port
        //           message queue the task now finds itself.
        // Note: The task runs on the port's own event loop, so
        // finalTargetPort is this port.
        // Step 7.2: Let messageEventTarget be finalTargetPort's message
        //           event target.
        // Note: The message event target defaults to the port itself.
        // Step 7.3: Let targetRealm be finalTargetPort's relevant realm.
        // Note: The current realm is the port's realm.
        let serialize_result = SerializeWithTransferResult {
            serialized: payload.serialized,
            transfer_data_holders: payload.transfer_data_holders,
        };
        // Step 7.4: Let deserializeRecord be
        //           StructuredDeserializeWithTransfer(serializeWithTransferResult,
        //           targetRealm).
        let deserialize_outcome =
            structured_deserialize_with_transfer(&serialize_result, &ec.value_undefined(), ec);
        let deserialize_result = match deserialize_outcome {
            Ok(result) => result,
            Err(_) => {
                // If this throws an exception, catch it, fire an event named
                // messageerror at messageEventTarget, using MessageEvent, and
                // then return.
                let message_event = MessageEvent::new(
                    String::from("messageerror"),
                    MessageEventInit {
                        bubbles: false,
                        cancelable: false,
                        composed: false,
                        data: ec.value_null(),
                        origin: String::new(),
                        last_event_id: String::new(),
                        source: None,
                        ports: Vec::new(),
                    },
                    ec,
                );
                fire_message_event(&self.event_target, message_event, time_millis, ec)?;
                return Ok(());
            }
        };

        // Step 7.5: Let messageClone be deserializeRecord.[[Deserialized]].
        let message_clone = deserialize_result.deserialized;

        // Step 7.6: Let newPorts be a new frozen array consisting of all
        //           MessagePort objects in deserializeRecord.[[TransferredValues]],
        //           if any, maintaining their relative order.
        let new_ports: Vec<JsObject> = deserialize_result
            .transferred_values
            .iter()
            .filter_map(Types::value_as_object)
            .collect();

        // Step 7.7: Fire an event named message at messageEventTarget, using
        //           MessageEvent, with the data attribute initialized to
        //           messageClone and the ports attribute initialized to
        //           newPorts.
        let message_event = MessageEvent::new(
            String::from("message"),
            MessageEventInit {
                bubbles: false,
                cancelable: false,
                composed: false,
                data: message_clone,
                origin: String::new(),
                last_event_id: String::new(),
                source: None,
                ports: new_ports,
            },
            ec,
        );
        fire_message_event(&self.event_target, message_event, time_millis, ec)
    }
}

/// <https://html.spec.whatwg.org/#messagechannel>
#[gc_struct]
pub(crate) struct MessageChannel {
    /// <https://html.spec.whatwg.org/#dom-messagechannel-port1>
    pub(crate) port1: MessagePort,

    /// <https://html.spec.whatwg.org/#dom-messagechannel-port2>
    pub(crate) port2: MessagePort,
}

impl MessageChannel {
    /// <https://html.spec.whatwg.org/#dom-messagechannel>
    pub(crate) fn new_channel(ec: &mut dyn ExecutionContext<Types>) -> Completion<Self, Types> {
        // Step 1: Set this's port 1 to a new MessagePort in this's relevant
        //         realm.
        let port1 = MessagePort::new_port(ec)?;
        // Step 2: Set this's port 2 to a new MessagePort in this's relevant
        //         realm.
        let port2 = MessagePort::new_port(ec)?;
        // Step 3: Entangle this's port 1 and this's port 2.
        let Some(messaging) = port1.messaging(ec) else {
            return Err(ec.new_type_error("MessageChannel: no event loop"));
        };
        messaging.entangle_pair(port1.clone(), port2.clone(), ec);
        // The user agent must know both ports to route messages to either
        // one's owning event loop (`MessagePortExtraFG.tla`'s `NewChannel`).
        if let Some(event_sender) = port1.global_scope.event_sender()
            && let Err(error) = event_sender.send(ContentEvent::PortChannelCreated {
                port1: port1.port_id,
                port2: port2.port_id,
            })
        {
            return Err(ec.new_type_error(&format!("MessageChannel: {error}")));
        }
        Ok(Self { port1, port2 })
    }
}

/// <https://html.spec.whatwg.org/#message-ports:transfer-receiving-steps>
pub(crate) fn create_transferred_port_object(
    port_id: PortId,
    remote_port: Option<PortId>,
    queue: Vec<PortMessagePayload>,
    in_flight: u32,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsObject, Types> {
    // Step 1: Set value's has been shipped flag to true.
    // Note: The user agent is told the port was received
    // (`PortTransferReceived`) so it stops buffering messages for the port;
    // the shipped flag itself is not modelled, and the record registered by
    // `receive_transferred_port` tracks the hand-over
    // (`CompletionInProgress`).
    // Step 2: Move all the tasks that are to fire message events in
    //         dataHolder.[[PortMessageQueue]] to the port message queue
    //         of value, if any, leaving value's port message queue in
    //         its initial disabled state, and, if value's relevant
    //         global object is a Window, associating the moved tasks
    //         with value's relevant global object's associated Document.
    // Step 3: If dataHolder.[[RemotePort]] is not null, then entangle
    //         dataHolder.[[RemotePort]] and value. (This will disentangle
    //         dataHolder.[[RemotePort]] from the original port that was
    //         transferred.)
    // Note: Steps 2-3 run in `receive_transferred_port`, which moves the
    // transferred queue into the new port's record (left disabled) and
    // entangles the record with the remote port.  The new port's wrapper is
    // created here (in the receiving realm) before the record is
    // registered.
    let global_scope = with_global_scope(ec, |global_scope, _ec| Ok(global_scope.clone()))?;
    let Some(messaging) = global_scope.channel_messaging(ec) else {
        return Err(ec.new_type_error("transfer receive: no event loop"));
    };
    let port = MessagePort {
        event_target: EventTarget::new(ec),
        global_scope,
        port_id,
    };
    let event_sender = port.global_scope.event_sender();
    let object = create_interface_instance::<Types, MessagePort>(port.clone(), ec)?;
    // The record stores the port re-read from its wrapper, whose reflector
    // was set by the interface instance creation.
    let port = ec
        .with_object_any(&object)
        .and_then(|data| data.downcast_ref::<MessagePort>().cloned())
        .ok_or_else(|| ec.new_type_error("transfer receive: wrapper is not a MessagePort"))?;
    messaging.receive_transferred_port(port, remote_port, queue, in_flight, ec);
    if let Some(event_sender) = event_sender
        && let Err(error) = event_sender.send(ContentEvent::PortTransferReceived { port: port_id })
    {
        log::error!("failed to notify the user agent of port receive: {error}");
    }
    Ok(object)
}

/// The data produced by the MessagePort transfer steps (the dataHolder of
/// <https://html.spec.whatwg.org/#message-ports:transfer-steps>).
pub(crate) struct PortTransferData {
    /// The id of the transferred port.
    pub(crate) port_id: PortId,
    /// The port's pending message queue, moved with the transfer
    /// (dataHolder.[[PortMessageQueue]]).
    pub(crate) queue: Vec<PortMessagePayload>,
    /// The port the transferred port was entangled with, if any
    /// (dataHolder.[[RemotePort]]).
    pub(crate) remote_port: Option<PortId>,
    /// Routed messages still in flight toward the port (not yet delivered
    /// to its queue), moved with the transfer so the receiving process's
    /// direct-delivery guard stays correct.
    pub(crate) in_flight: u32,
}

/// <https://html.spec.whatwg.org/#message-ports:transfer-steps>
pub(crate) fn message_port_transfer_steps(
    object: &JsObject,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<Option<PortTransferData>, Types> {
    // Note: The transfer steps are invoked with a MessagePort value; the
    // caller only runs them for MessagePort platform objects, so `None`
    // here is a defensive fallback.
    let port: Option<MessagePort> = ec
        .with_object_any(object)
        .and_then(|data| data.downcast_ref::<MessagePort>().cloned());
    let Some(port) = port else {
        return Ok(None);
    };
    let Some(messaging) = port.messaging(ec) else {
        return Ok(None);
    };
    let Some(event_sender) = port.global_scope.event_sender() else {
        return Ok(None);
    };
    // Step 3: If value is entangled with another port remotePort:
    // Step 3.1: Set remotePort's has been shipped flag to true.
    // Note: The user agent, informed of the transfer, routes messages for
    // the port away while it is in transit, which covers the remote port
    // as well.
    // Step 3.2: Set dataHolder.[[RemotePort]] to remotePort.
    // Step 4: Otherwise, set dataHolder.[[RemotePort]] to null.
    // Note: The entanglement is read before `transfer_port` removes the
    // record (steps 1-2 below), since the record no longer exists after.
    let remote_port = messaging
        .port_record(port.port_id, ec)
        .and_then(|record| record.entangled);
    // Step 1: Set value's has been shipped flag to true.
    // Step 2: Set dataHolder.[[PortMessageQueue]] to value's port message
    //         queue.
    // Note: These run in `transfer_port` (`MessagePortExtraFG.tla`'s
    // `Transfer`): the record leaves this realm and its queue is drained
    // into the transfer data holder.  The user agent is informed there so
    // it buffers or re-routes messages while the port is in transit (the
    // shipped flag's cross-process effect; step 3.1's remote port is
    // covered by the same notification).
    let (queue, in_flight) = messaging
        .transfer_port(port.port_id, &event_sender, ec)
        .map_err(|_error| crate::webidl::data_clone_error_value(ec))?;
    Ok(Some(PortTransferData {
        port_id: port.port_id,
        queue,
        remote_port,
        in_flight,
    }))
}

/// <https://dom.spec.whatwg.org/#concept-event-fire>
fn fire_message_event(
    target: &EventTarget,
    message_event: MessageEvent,
    time_millis: f64,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<(), Types> {
    // Step 2: Let event be the result of creating an event given
    //         eventConstructor, in the relevant realm of target.
    // Note: eventConstructor (MessageEvent) is given, so step 1 does not
    // apply; the event's type, data, and ports attributes were initialized
    // by the caller (`run_message_task`'s step 7.7, the fire algorithm's
    // steps 3-4).  Creating the event also initializes its isTrusted
    // attribute to true and its timeStamp attribute to the time of the
    // occurrence.
    let event_object = create_interface_instance::<Types, MessageEvent>(message_event, ec)?;
    let message_event: MessageEvent = ec
        .with_object_any(&event_object)
        .and_then(|data| data.downcast_ref::<MessageEvent>().cloned())
        .ok_or_else(|| ec.new_type_error("event_object is not a MessageEvent"))?;
    *message_event.event.is_trusted.borrow_mut(ec) = true;
    *message_event.event.time_stamp.borrow_mut(ec) = time_millis;
    // Step 5: Return the result of dispatching event at target, with
    //         legacy target override flag set if set.
    let path = simple_path(target, ec);
    dispatch_with_path(ec, &path, &message_event.event)
        .map(|_| ())
        .map_err(|error| ec.new_type_error(&format!("failed to dispatch event: {error:?}")))
}

/// <https://html.spec.whatwg.org/#disentangle>
fn fire_close_event(
    other_port: &MessagePort,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<(), Types> {
    // Step 4: Fire an event named close at otherPort.
    // Note: Steps 1-3 of the disentangle steps (otherPort, the assertion,
    // and the disentangling of the pair) run in the caller
    // (`MessagePort::close` via `ChannelMessaging::close`).
    let event = Event::new(String::from("close"), false, false, false, true, 0.0, ec);
    let path = simple_path(&other_port.event_target, ec);
    dispatch_with_path(ec, &path, &event)
        .map(|_| ())
        .map_err(|error| ec.new_type_error(&format!("failed to dispatch close event: {error:?}")))
}
