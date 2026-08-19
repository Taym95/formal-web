use ipc_messages::content::{Event as ContentEvent, MessageId, PortId};
use ipc_messages::safe_passing_of_structured_data::PortMessagePayload;
use js_engine::gc_struct;
use js_engine::{Completion, ExecutionContext, JsTypes};

use crate::dom::dispatch_with_path;
use crate::dom::event::{EventTarget, EventTargetAccess};
use crate::dom::simple_path;
use crate::dom::Event;
use crate::html::safe_passing_of_structured_data::{
    SerializeWithTransferResult, structured_deserialize_with_transfer, structured_serialize_with_transfer,
};
use crate::js::platform_objects::with_global_scope;
use crate::js::Types;
use crate::webidl::bindings::create_interface_instance;

use super::{GlobalScope, MessageEvent, MessageEventInit};

use crate::html::channel_messaging::ChannelMessaging;

type JsValue = <Types as JsTypes>::JsValue;
type JsObject = <Types as JsTypes>::JsObject;

/// <https://html.spec.whatwg.org/#messageport>
#[gc_struct]
pub(crate) struct MessagePort {
    /// <https://dom.spec.whatwg.org/#interface-eventtarget>
    pub(crate) event_target: EventTarget,

    /// <https://html.spec.whatwg.org/#global-object>
    pub(crate) global_scope: GlobalScope,

    /// <https://html.spec.whatwg.org/#message-ports>
    #[ignore_trace]
    pub(crate) port_id: PortId,
}

impl EventTargetAccess for MessagePort {
    fn get_event_target(&self, _ec: &mut dyn ExecutionContext<Types>) -> EventTarget {
        self.event_target.clone()
    }
}

impl MessagePort {
    /// The platform object wrapping this port (the [[Detached]]-bearing
    /// object of the current realm).  The record stores the object because
    /// the domain-side clone is created before the Web IDL layer sets the
    /// reflector.
    pub(crate) fn object(&self, ec: &mut dyn ExecutionContext<Types>) -> Option<JsObject> {
        self.messaging(ec)
            .and_then(|messaging| messaging.port_object(self.port_id, ec))
    }

    /// Create a new MessagePort in the current realm (a "new MessagePort"
    /// of the spec).
    /// <https://html.spec.whatwg.org/#dom-messagechannel>
    pub(crate) fn new_port(
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Completion<(Self, JsObject), Types> {
        let global_scope = with_global_scope(ec, |global_scope, _ec| Ok(global_scope.clone()))?;
        let port = Self {
            event_target: EventTarget::new(ec),
            global_scope,
            port_id: PortId::new(),
        };
        let object = create_interface_instance::<Types, MessagePort>(port.clone(), ec)?;
        Ok((port, object))
    }

    /// The channel messaging state of the port's realm.
    fn messaging(
        &self,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Option<ChannelMessaging> {
        self.global_scope.channel_messaging(ec)
    }

    /// <https://html.spec.whatwg.org/#message-port-post-message-steps>
    /// The message port post message steps, given message and options.
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
        // Note: The delivery is `MessagePortExtraFG.tla`'s `PostMessage`: the message goes
        // straight into the target's queue when the target is managed by
        // this same event loop, and is appended to the user agent's routing
        // queue otherwise.  The task's substeps (7.1-7.7) run when the
        // message event fires (`run_message_task`).
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
    /// The start() method steps are to enable this's port message queue, if
    /// it is not already enabled.
    pub(crate) fn start(&self, ec: &mut dyn ExecutionContext<Types>) {
        let Some(messaging) = self.messaging(ec) else {
            return;
        };
        if let Some(event_sender) = self.global_scope.event_sender() {
            messaging.start(self.port_id, &event_sender, ec);
        }
    }

    /// <https://html.spec.whatwg.org/#dom-messageport-close>
    /// The close() method steps: set [[Detached]] to true and, if this is
    /// entangled, disentangle it (which fires a close event at the other
    /// port).
    pub(crate) fn close(&self, ec: &mut dyn ExecutionContext<Types>) -> Completion<(), Types> {
        let Some(messaging) = self.messaging(ec) else {
            return Ok(());
        };
        let other = messaging.close(self.port_id, ec);
        if let Some(other) = other {
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

    /// The first time the onmessage IDL attribute is set, enable the port
    /// message queue (as if start() had been called).
    /// <https://html.spec.whatwg.org/#message-ports>
    pub(crate) fn enable_queue(&self, ec: &mut dyn ExecutionContext<Types>) {
        let Some(messaging) = self.messaging(ec) else {
            return;
        };
        if let Some(event_sender) = self.global_scope.event_sender() {
            messaging.enable_queue(self.port_id, &event_sender, ec);
        }
    }

    /// Run the message task of the message port post message steps: pop one
    /// queued message and fire the message (or messageerror) event at this
    /// port.  `time_millis` is the current time on the port's event loop
    /// clock, stamped on the fired event.
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
    /// The new MessageChannel() constructor steps.
    pub(crate) fn new_channel(ec: &mut dyn ExecutionContext<Types>) -> Completion<Self, Types> {
        // Step 1: Set this's port 1 to a new MessagePort in this's relevant
        //         realm.
        let (port1, object1) = MessagePort::new_port(ec)?;
        // Step 2: Set this's port 2 to a new MessagePort in this's relevant
        //         realm.
        let (port2, object2) = MessagePort::new_port(ec)?;
        // Step 3: Entangle this's port 1 and this's port 2.
        let Some(messaging) = port1.messaging(ec) else {
            return Err(ec.new_type_error("MessageChannel: no event loop"));
        };
        messaging.entangle_pair(
            port1.port_id,
            port2.port_id,
            object1,
            object2,
            ec,
        );
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
/// Create the MessagePort platform object of a transferred port in the
/// current realm: the record enters `CompletionInProgress`, re-entangles with
/// the remote port, and takes over the transferred message queue.  The user
/// agent is informed so its routing can deliver messages to the new owner.
pub(crate) fn create_transferred_port_object(
    port_id: PortId,
    remote_port: Option<PortId>,
    queue: Vec<PortMessagePayload>,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<JsObject, Types> {
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
    let object = create_interface_instance::<Types, MessagePort>(port, ec)?;
    messaging.receive_transferred_port(port_id, object.clone(), remote_port, queue, ec);
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
}

/// <https://html.spec.whatwg.org/#message-ports:transfer-steps>
/// The transfer steps of a MessagePort, run during structured serialization:
/// the port leaves its realm (`MessagePortExtraFG.tla`'s `Transfer`).  Returns `None` when
/// the object is not a MessagePort.
pub(crate) fn message_port_transfer_steps(
    object: &JsObject,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<Option<PortTransferData>, Types> {
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
    let remote_port = messaging
        .port_record(port.port_id, ec)
        .and_then(|record| record.entangled);
    let queue = messaging
        .transfer_port(port.port_id, &event_sender, ec)
        .map_err(|_error| crate::webidl::data_clone_error_value(ec))?;
    Ok(Some(PortTransferData {
        port_id: port.port_id,
        queue,
        remote_port,
    }))
}

/// <https://dom.spec.whatwg.org/#concept-event-fire>
/// Fire a pre-built MessageEvent at a port's event target, with the trusted
/// flag and current timestamp of a user-agent-fired event.
fn fire_message_event(
    target: &EventTarget,
    message_event: MessageEvent,
    time_millis: f64,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<(), Types> {
    let event_object = create_interface_instance::<Types, MessageEvent>(message_event, ec)?;
    let message_event: MessageEvent = ec
        .with_object_any(&event_object)
        .and_then(|data| data.downcast_ref::<MessageEvent>().cloned())
        .ok_or_else(|| ec.new_type_error("event_object is not a MessageEvent"))?;
    *message_event.event.is_trusted.borrow_mut(ec) = true;
    *message_event.event.time_stamp.borrow_mut(ec) = time_millis;
    let path = simple_path(target, ec);
    dispatch_with_path(ec, &path, &message_event.event)
        .map(|_| ())
        .map_err(|error| ec.new_type_error(&format!("failed to dispatch event: {error:?}")))
}

/// <https://html.spec.whatwg.org/#disentangle>
/// Fire an event named close at otherPort (the last step of the disentangle
/// steps).
fn fire_close_event(
    other_port: &MessagePort,
    ec: &mut dyn ExecutionContext<Types>,
) -> Completion<(), Types> {
    let event = Event::new(
        String::from("close"),
        false,
        false,
        false,
        true,
        0.0,
        ec,
    );
    let path = simple_path(&other_port.event_target, ec);
    dispatch_with_path(ec, &path, &event)
        .map(|_| ())
        .map_err(|error| ec.new_type_error(&format!("failed to dispatch close event: {error:?}")))
}
