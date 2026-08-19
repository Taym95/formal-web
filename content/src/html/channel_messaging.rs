//! Per-global channel messaging state: the content-process half of the
//! cross-process MessagePort workflow modelled by
//! `verification/tla_specs/MessagePortExtraFG.tla`.
//!
//! Each realm's [`GlobalScope`] lazily creates one [`ChannelMessaging`] on
//! first port use.  It owns the [`PortRecord`]s of the ports managed by the
//! realm's event loop (`MessagePortExtraFG.tla`'s `port_state`, restricted to the ports this
//! content process knows about).  The user-agent side
//! (`user_agent/src/channel_messaging.rs`) owns the routing queue and the
//! per-port transfer state needed to route messages between event loops.

use std::collections::VecDeque;

use ipc::IpcSender;
use ipc_messages::content::{Event as ContentEvent, EventLoopId, PortId, PortTaskKind, TransferState};
use ipc_messages::safe_passing_of_structured_data::PortMessagePayload;
use js_engine::gc::{GcCell, gc_cell_new};
use js_engine::gc_struct;
use js_engine::{ExecutionContext, JsTypes};
use log::warn;

use crate::js::Types;

use verification::{TLATracer, TraceSender};

type JsObject = <Types as JsTypes>::JsObject;

/// One port's record, owned by the content process of the port's event
/// loop (the `port_state[id]` of MessagePortExtraFG.tla).  A record lives in
/// the [`GlobalScope`]'s [`ChannelMessaging`] from port creation until the
/// port's document is torn down; transferring a port moves the record to the
/// receiving realm's `ChannelMessaging`.
///
/// <https://html.spec.whatwg.org/#message-ports>
#[gc_struct]
pub(crate) struct PortRecord {
    /// <https://html.spec.whatwg.org/#message-ports>
    #[ignore_trace]
    pub(crate) port_id: PortId,

    /// The MessagePort platform object of this realm, used to resolve the
    /// port's event target when firing message events.
    /// <https://html.spec.whatwg.org/#message-ports>
    pub(crate) object: Option<JsObject>,

    /// The port's transfer state (`MessagePortExtraFG.tla`'s `ts`).
    /// <https://html.spec.whatwg.org/#message-ports:transfer-steps>
    #[ignore_trace]
    pub(crate) ts: TransferState,

    /// The id of the port this port is entangled with; `None` when
    /// disentangled (`MessagePortExtraFG.tla`'s `entangled`, with `NoPortId`).
    /// <https://html.spec.whatwg.org/#entangle>
    #[ignore_trace]
    pub(crate) entangled: Option<PortId>,

    /// The port message queue: the tasks that are to fire message events,
    /// in order (`MessagePortExtraFG.tla`'s `queue`).  Initially disabled; message tasks
    /// fire only once the queue is enabled (`start()` or the first
    /// `onmessage` set).
    /// <https://html.spec.whatwg.org/#port-message-queue>
    #[ignore_trace]
    pub(crate) queue: VecDeque<PortMessagePayload>,

    /// Whether the port message queue is enabled (the spec's enabled flag;
    /// once enabled a port can never be disabled again).
    /// <https://html.spec.whatwg.org/#port-message-queue>
    #[ignore_trace]
    pub(crate) enabled: bool,

    /// The port's [[Detached]] internal slot.  `close()` sets it; a
    /// detached port cannot be transferred.
    /// <https://html.spec.whatwg.org/#dom-messageport-close>
    #[ignore_trace]
    pub(crate) detached: bool,
}

impl PortRecord {
    /// Whether the port is managed by this event loop and can receive
    /// messages directly (`MessagePortExtraFG.tla`'s `owner = el` for the direct
    /// delivery branch of `PostMessage`).
    fn is_local(&self) -> bool {
        matches!(
            self.ts,
            TransferState::Managed | TransferState::CompletionInProgress
        )
    }
}

/// The per-global channel messaging state, created lazily on first port
/// use.  Owns the port records of the ports this realm's event loop manages
/// or has in transit.
///
/// <https://html.spec.whatwg.org/#channel-messaging>
#[gc_struct]
pub(crate) struct ChannelMessaging {
    /// The event loop this content process hosts (`MessagePortExtraFG.tla`'s `el`), used
    /// for the MessagePort TLA trace event arguments.
    #[ignore_trace]
    event_loop_id: EventLoopId,

    /// The TLA trace sender for the MessagePort spec (`MessagePortExtraFG.tla`'s actions
    /// traced from the content side), set at creation from the realm's
    /// global scope.
    #[ignore_trace]
    trace_sender: Option<TraceSender>,

    /// The port records, keyed by port id (`MessagePortExtraFG.tla`'s `port_state`).
    ports: GcCell<Vec<PortRecord>>,
}

impl ChannelMessaging {
    /// Create the per-global channel messaging state on first use.
    pub(crate) fn new(
        event_loop_id: EventLoopId,
        trace_sender: Option<TraceSender>,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Self {
        Self {
            event_loop_id,
            trace_sender,
            ports: gc_cell_new(Vec::new(), ec),
        }
    }

    /// Emit a MessagePort trace event (the actions of MessagePortExtraFG.tla).
    fn trace(&self, event: &str, args: Vec<String>) {
        let Some(sender) = &self.trace_sender else {
            return;
        };
        let mut tracer = TLATracer::new("MessagePort", "formal-web:content", Some(sender.clone()));
        tracer.log_with_location(Some("MessagePort"), event, args, file!(), line!());
    }

    /// <https://html.spec.whatwg.org/#entangle>
    /// Create the entangled record pair of a new channel (`MessagePortExtraFG.tla`'s
    /// `NewChannel`).  The caller has already created the two MessagePort
    /// platform objects and registers the pair with the user agent.
    pub(crate) fn entangle_pair(
        &self,
        port1: PortId,
        port2: PortId,
        object1: JsObject,
        object2: JsObject,
        ec: &mut dyn ExecutionContext<Types>,
    ) {
        let mut ports = self.ports.borrow_mut(ec);
        ports.push(PortRecord {
            port_id: port1,
            object: Some(object1),
            ts: TransferState::Managed,
            entangled: Some(port2),
            queue: VecDeque::new(),
            enabled: false,
            detached: false,
        });
        ports.push(PortRecord {
            port_id: port2,
            object: Some(object2),
            ts: TransferState::Managed,
            entangled: Some(port1),
            queue: VecDeque::new(),
            enabled: false,
            detached: false,
        });
        drop(ports);
        self.trace(
            "NewChannel",
            vec![port1.to_string(), port2.to_string(), self.event_loop_id.to_string()],
        );
    }

    /// Create the record of a port received in this event loop during
    /// structured deserialization (`MessagePortExtraFG.tla`'s `TransferReceive`): the port
    /// enters `CompletionInProgress`, re-entangles with its remote port, and
    /// takes over the transferred message queue.
    /// <https://html.spec.whatwg.org/#message-ports:transfer-receiving-steps>
    pub(crate) fn receive_transferred_port(
        &self,
        port_id: PortId,
        object: JsObject,
        remote_port: Option<PortId>,
        queue: Vec<PortMessagePayload>,
        ec: &mut dyn ExecutionContext<Types>,
    ) {
        let mut ports = self.ports.borrow_mut(ec);
        ports.push(PortRecord {
            port_id,
            object: Some(object),
            ts: TransferState::CompletionInProgress,
            entangled: remote_port,
            queue: queue.into(),
            enabled: false,
            detached: false,
        });
        drop(ports);
        self.trace(
            "TransferReceive",
            vec![port_id.to_string(), self.event_loop_id.to_string()],
        );
    }

    /// The transfer steps of a port being serialized (`MessagePortExtraFG.tla`'s
    /// `Transfer`): the port leaves this event loop.  Its record is removed
    /// here — the port's data, including its pending message queue, travels
    /// in the transfer data holder and the record is recreated in the
    /// receiving realm.
    /// <https://html.spec.whatwg.org/#message-ports:transfer-steps>
    pub(crate) fn transfer_port(
        &self,
        port_id: PortId,
        event_sender: &IpcSender<ContentEvent>,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Result<Vec<PortMessagePayload>, String> {
        let (queue, removed) = {
            let mut ports = self.ports.borrow_mut(ec);
            let Some(index) = ports.iter().position(|record| record.port_id == port_id) else {
                return Err(format!("transfer: unknown port {port_id}"));
            };
            if ports[index].detached {
                return Err(String::from("transfer: port is detached"));
            }
            if !matches!(
                ports[index].ts,
                TransferState::Managed | TransferState::CompletionInProgress
            ) {
                return Err(format!("transfer: port {port_id} is already in transit"));
            }
            let queue: Vec<PortMessagePayload> = ports[index].queue.drain(..).collect();
            ports.remove(index);
            (queue, true)
        };
        // The record leaves this realm; the transfer data holder carries the
        // queue to the receiving realm.
        if !removed {
            return Err(format!("transfer: unknown port {port_id}"));
        }
        // Note: `MessagePortExtraFG.tla`'s `Transfer` keeps the record (with the `buf`
        // cleared and `owner` set to `NoEventLoopId`); here the record is
        // removed and the queue shipped in the transfer data holder, which
        // is the cross-process equivalent of the record moving to the new
        // owner.  The user agent is informed so it can buffer or re-route
        // messages for the port while it is in transit.
        self.trace(
            "Transfer",
            vec![port_id.to_string(), self.event_loop_id.to_string()],
        );
        event_sender
            .send(ContentEvent::PortTransferStarted { port: port_id })
            .map_err(|error| format!("failed to notify the user agent of port transfer: {error}"))?;
        Ok(queue)
    }

    /// The message port post message steps after serialization: deliver the
    /// message to the target port, either directly (`MessagePortExtraFG.tla`'s `PostMessage`
    /// direct branch: the target is managed by this same event loop and the
    /// loop is quiescent) or by appending a "Single" item to the user agent's
    /// routing queue.
    /// <https://html.spec.whatwg.org/#message-port-post-message-steps>
    pub(crate) fn post_message(
        &self,
        src_id: PortId,
        target_id: Option<PortId>,
        msg: PortMessagePayload,
        event_sender: &IpcSender<ContentEvent>,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Result<(), String> {
        // Step 6: If targetPort is null, or if doomed is true, then return.
        let Some(target_id) = target_id else {
            return Ok(());
        };
        let (src_managed, direct) = {
            let ports = self.ports.borrow_mut(ec);
            let Some(src_index) = ports.iter().position(|record| record.port_id == src_id) else {
                // The source port is detached (transferred or closed); its
                // entanglement was severed, so targetPort is null.
                return Ok(());
            };
            if ports[src_index].entangled != Some(target_id) {
                return Ok(());
            }
            // `MessagePortExtraFG.tla`'s `PostMessage` direct branch: the target is managed
            // by this same event loop, so the message goes straight into its
            // queue.
            let direct = ports[src_index].ts == TransferState::Managed
                && match ports.iter().position(|record| record.port_id == target_id) {
                    Some(target_index) => ports[target_index].ts == TransferState::Managed,
                    None => false,
                };
            (ports[src_index].ts == TransferState::Managed, direct)
        };
        // `MessagePortExtraFG.tla`'s `PostMessage` requires the source port to be managed by
        // the posting event loop; the trace records the action only when the
        // model accepts it.
        if src_managed {
            self.trace(
                "PostMessage",
                vec![
                    src_id.to_string(),
                    self.event_loop_id.to_string(),
                    msg.message_id.to_string(),
                ],
            );
        }
        if direct {
            let mut ports = self.ports.borrow_mut(ec);
            let Some(target_index) = ports.iter().position(|record| record.port_id == target_id)
            else {
                return Ok(());
            };
            ports[target_index].queue.push_back(msg);
            drop(ports);
            self.request_message_tasks(target_id, event_sender, ec)?;
        } else {
            // `MessagePortExtraFG.tla`'s `PostMessage` routed branch: append a "Single"
            // item to the user agent's routing queue.
            event_sender
                .send(ContentEvent::PortMessageRouted {
                    tgt: target_id,
                    msg,
                })
                .map_err(|error| format!("failed to route port message: {error}"))?;
        }
        Ok(())
    }

    /// The start() method steps: enable the port message queue and request
    /// task slots for the messages that were queued while disabled.
    /// <https://html.spec.whatwg.org/#dom-messageport-start>
    pub(crate) fn start(
        &self,
        port_id: PortId,
        event_sender: &IpcSender<ContentEvent>,
        ec: &mut dyn ExecutionContext<Types>,
    ) {
        let was_enabled = {
            let mut ports = self.ports.borrow_mut(ec);
            let Some(index) = ports.iter().position(|record| record.port_id == port_id) else {
                return;
            };
            let was_enabled = ports[index].enabled;
            ports[index].enabled = true;
            was_enabled
        };
        if !was_enabled {
            if let Err(error) = self.request_message_tasks(port_id, event_sender, ec) {
                warn!("failed to request port message tasks after start: {error}");
            }
        }
    }

    /// The close() method steps: set [[Detached]], disentangle, and return
    /// the port that was entangled with this one so the caller can fire a
    /// close event at it.  The record stays so already queued messages are
    /// still delivered.
    /// <https://html.spec.whatwg.org/#dom-messageport-close>
    pub(crate) fn close(
        &self,
        port_id: PortId,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Option<PortId> {
        let mut ports = self.ports.borrow_mut(ec);
        let Some(index) = ports.iter().position(|record| record.port_id == port_id) else {
            return None;
        };
        // Step 1: Set this's [[Detached]] internal slot value to true.
        ports[index].detached = true;
        // Step 2: If this is entangled, disentangle it.
        let other = ports[index].entangled.take();
        if let Some(other) = other
            && let Some(other_index) = ports.iter().position(|record| record.port_id == other)
        {
            ports[other_index].entangled = None;
        }
        other
    }

    /// Enable a port's message queue without the start() steps (the implied
    /// start of the first `onmessage` set).  Returns whether the queue was
    /// previously disabled.
    /// <https://html.spec.whatwg.org/#port-message-queue>
    pub(crate) fn enable_queue(
        &self,
        port_id: PortId,
        event_sender: &IpcSender<ContentEvent>,
        ec: &mut dyn ExecutionContext<Types>,
    ) {
        let was_enabled = {
            let mut ports = self.ports.borrow_mut(ec);
            let Some(index) = ports.iter().position(|record| record.port_id == port_id) else {
                return;
            };
            let was_enabled = ports[index].enabled;
            ports[index].enabled = true;
            was_enabled
        };
        if !was_enabled {
            if let Err(error) = self.request_message_tasks(port_id, event_sender, ec) {
                warn!("failed to request port message tasks after enabling: {error}");
            }
        }
    }

    /// Run a task queued by the user agent's routing (`MessagePortExtraFG.tla`'s
    /// `RunTask`): a `NewTask` message is appended to the port's queue, and
    /// a `Buffer` task completes a transfer.
    pub(crate) fn handle_port_task(
        &self,
        port_id: PortId,
        task: PortTaskKind,
        event_sender: &IpcSender<ContentEvent>,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Result<(), String> {
        match task {
            PortTaskKind::NewTask { msg } => {
                let local = {
                    let ports = self.ports.borrow_mut(ec);
                    let Some(index) = ports.iter().position(|record| record.port_id == port_id)
                    else {
                        // The port was transferred away (or closed) before the
                        // routed task ran; per `MessagePortExtraFG.tla`'s `RunTask`, the task
                        // is returned to the routing queue.
                        return Ok(());
                    };
                    ports[index].is_local()
                };
                if local {
                    let mut ports = self.ports.borrow_mut(ec);
                    let Some(index) = ports.iter().position(|record| record.port_id == port_id)
                    else {
                        return Ok(());
                    };
                    ports[index].queue.push_back(msg);
                    drop(ports);
                    self.trace(
                        "RunTask",
                        vec![
                            self.event_loop_id.to_string(),
                            port_id.to_string(),
                            String::from("NewTask"),
                        ],
                    );
                    self.request_message_tasks(port_id, event_sender, ec)?;
                } else {
                    self.trace(
                        "RunTask",
                        vec![
                            self.event_loop_id.to_string(),
                            port_id.to_string(),
                            String::from("NewTask"),
                        ],
                    );
                    event_sender
                        .send(ContentEvent::PortMessageRouted { tgt: port_id, msg })
                        .map_err(|error| format!("failed to return port message: {error}"))?;
                }
                Ok(())
            }
            PortTaskKind::Buffer { buf } => {
                let completed = {
                    let mut ports = self.ports.borrow_mut(ec);
                    let Some(index) = ports.iter().position(|record| record.port_id == port_id)
                    else {
                        // The port was transferred away before the completion
                        // task ran; the buffer is returned to the routing queue.
                        return Ok(());
                    };
                    // Mirror the user agent's `RouteMessage` transition for a
                    // "ReturnedBuffer" item against a `CompletionRequested`
                    // port: the transfer completion now proceeds.
                    if ports[index].ts == TransferState::CompletionRequested {
                        ports[index].ts = TransferState::CompletionInProgress;
                    }
                    ports[index].is_local()
                };
                if completed {
                    let mut ports = self.ports.borrow_mut(ec);
                    let Some(index) = ports.iter().position(|record| record.port_id == port_id)
                    else {
                        return Ok(());
                    };
                    for msg in buf {
                        ports[index].queue.push_back(msg);
                    }
                    ports[index].ts = TransferState::Managed;
                    drop(ports);
                    self.trace(
                        "RunTask",
                        vec![
                            self.event_loop_id.to_string(),
                            port_id.to_string(),
                            String::from("Buffer"),
                        ],
                    );
                    // `MessagePortExtraFG.tla`'s `RunTask` appends a "Success" routing item
                    // when the completion task runs on the port's owner.  The
                    // user agent completes the transfer (Managed) and fires
                    // the first queued message task, so no task slot is
                    // requested here.
                    event_sender
                        .send(ContentEvent::PortTransferCompleted { tgt: port_id })
                        .map_err(|error| {
                            format!("failed to notify port transfer completion: {error}")
                        })?;
                } else {
                    self.trace(
                        "RunTask",
                        vec![
                            self.event_loop_id.to_string(),
                            port_id.to_string(),
                            String::from("Buffer"),
                        ],
                    );
                    event_sender
                        .send(ContentEvent::PortBufferReturned { tgt: port_id, buf })
                        .map_err(|error| format!("failed to return port buffer: {error}"))?;
                }
                Ok(())
            }
        }
    }

    /// Return a routed task to the user agent's routing queue when the port
    /// is no longer managed by this event loop (`MessagePortExtraFG.tla`'s `RunTask` when
    /// `port_state[port_id].owner /= el`).
    pub(crate) fn return_task_to_ua(
        &self,
        port_id: PortId,
        task: PortTaskKind,
        event_sender: &IpcSender<ContentEvent>,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Result<(), String> {
        let _ = ec;
        let (kind, result) = match task {
            PortTaskKind::NewTask { msg } => (
                "NewTask",
                event_sender
                    .send(ContentEvent::PortMessageRouted { tgt: port_id, msg })
                    .map_err(|error| format!("failed to return port message: {error}")),
            ),
            PortTaskKind::Buffer { buf } => (
                "Buffer",
                event_sender
                    .send(ContentEvent::PortBufferReturned { tgt: port_id, buf })
                    .map_err(|error| format!("failed to return port buffer: {error}")),
            ),
        };
        self.trace(
            "RunTask",
            vec![self.event_loop_id.to_string(), port_id.to_string(), String::from(kind)],
        );
        result
    }

    /// Pop one queued message (`MessagePortExtraFG.tla`'s `ReceiveMessage`): the caller
    /// fires the message event.  If more messages remain, a new task slot
    /// is requested.
    pub(crate) fn pop_queued_message(
        &self,
        port_id: PortId,
        event_sender: &IpcSender<ContentEvent>,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Result<Option<PortMessagePayload>, String> {
        let popped = {
            let mut ports = self.ports.borrow_mut(ec);
            let Some(index) = ports.iter().position(|record| record.port_id == port_id) else {
                return Ok(None);
            };
            if !ports[index].enabled {
                return Ok(None);
            }
            ports[index].queue.pop_front()
        };
        if popped.is_some() {
            // `MessagePortExtraFG.tla`'s `ReceiveMessage` action: the message task ran and
            // the queue popped.
            self.trace(
                "ReceiveMessage",
                vec![port_id.to_string(), self.event_loop_id.to_string()],
            );
            // The queue may hold further messages; each fires in its own task.
            self.request_message_tasks(port_id, event_sender, ec)?;
        }
        Ok(popped)
    }

    /// The record of a port, if this realm manages it.
    pub(crate) fn port_record(
        &self,
        port_id: PortId,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Option<PortRecord> {
        self.ports
            .borrow(ec)
            .iter()
            .find(|record| record.port_id == port_id)
            .cloned()
    }

    /// The platform object of a port, if this realm manages it.
    pub(crate) fn port_object(
        &self,
        port_id: PortId,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Option<JsObject> {
        self.ports
            .borrow(ec)
            .iter()
            .find(|record| record.port_id == port_id)
            .and_then(|record| record.object.clone())
    }

    /// Whether a record for the port exists in this realm.
    pub(crate) fn has_port(
        &self,
        port_id: PortId,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> bool {
        self.ports
            .borrow(ec)
            .iter()
            .any(|record| record.port_id == port_id)
    }

    /// Request a task slot for each queued message of an enabled port (the
    /// event loop bridge replies with one `RunPortMessageTask` per request).
    fn request_message_tasks(
        &self,
        port_id: PortId,
        event_sender: &IpcSender<ContentEvent>,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Result<(), String> {
        let pending = {
            let ports = self.ports.borrow(ec);
            ports
                .iter()
                .find(|record| record.port_id == port_id)
                .map(|record| record.enabled && !record.queue.is_empty())
                .unwrap_or(false)
        };
        if !pending {
            return Ok(());
        }
        event_sender
            .send(ContentEvent::PortMessageTaskPending { port: port_id })
            .map_err(|error| format!("failed to request port message task: {error}"))
    }
}
