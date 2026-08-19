//! Per-global channel messaging state: the content-process half of the
//! cross-process MessagePort workflow.  The message task scheduling follows
//! the HTML spec's message port post message steps (step 7 adds a task to
//! the target's port message queue; the substeps run when the target content
//! process handles the message), validated by the coarse `MessagePort.tla`
//! trace consumer; the transfer state machine of
//! `verification/tla_specs/MessagePortExtraFG.tla` models the routing
//! between event loops.
//!
//! Each realm's [`GlobalScope`] lazily creates one [`ChannelMessaging`] on
//! first port use.  It owns the [`PortRecord`]s of the ports managed by the
//! realm's event loop.  The user-agent side
//! (`user_agent/src/channel_messaging.rs`) owns the routing queue and the
//! per-port transfer state needed to route messages between event loops.

use std::collections::VecDeque;

use ipc::IpcSender;
use ipc_messages::content::{
    Event as ContentEvent, EventLoopId, PortId, PortTaskKind, TransferState,
};
use ipc_messages::safe_passing_of_structured_data::PortMessagePayload;
use js_engine::gc::{GcCell, gc_cell_new};
use js_engine::gc_struct;
use js_engine::{ExecutionContext, JsTypes};
use log::warn;

use crate::js::Types;

use verification::{TLATracer, TraceSender};

type JsObject = <Types as JsTypes>::JsObject;

/// <https://html.spec.whatwg.org/#message-ports>
#[gc_struct]
pub(crate) struct PortRecord {
    /// <https://html.spec.whatwg.org/#message-ports>
    #[ignore_trace]
    pub(crate) port_id: PortId,

    /// <https://html.spec.whatwg.org/#message-ports>
    pub(crate) object: Option<JsObject>,

    /// <https://html.spec.whatwg.org/#message-ports:transfer-steps>
    #[ignore_trace]
    pub(crate) ts: TransferState,

    /// <https://html.spec.whatwg.org/#entangle>
    #[ignore_trace]
    pub(crate) entangled: Option<PortId>,

    /// <https://html.spec.whatwg.org/#port-message-queue>
    #[ignore_trace]
    pub(crate) queue: VecDeque<PortMessagePayload>,

    /// <https://html.spec.whatwg.org/#port-message-queue>
    #[ignore_trace]
    pub(crate) enabled: bool,

    /// <https://html.spec.whatwg.org/#dom-messageport-close>
    #[ignore_trace]
    pub(crate) detached: bool,

    /// Routed messages still in flight toward this port.
    #[ignore_trace]
    in_flight: u32,
}

impl PortRecord {
    /// <https://html.spec.whatwg.org/#message-ports:transfer-steps>
    fn is_local(&self) -> bool {
        matches!(
            self.ts,
            TransferState::Managed | TransferState::CompletionInProgress
        )
    }
}

/// <https://html.spec.whatwg.org/#channel-messaging>
#[gc_struct]
pub(crate) struct ChannelMessaging {
    /// <https://html.spec.whatwg.org/#channel-messaging>
    #[ignore_trace]
    event_loop_id: EventLoopId,

    /// <https://html.spec.whatwg.org/#channel-messaging>
    #[ignore_trace]
    trace_sender: Option<TraceSender>,

    /// <https://html.spec.whatwg.org/#channel-messaging>
    ports: GcCell<Vec<PortRecord>>,
}

impl ChannelMessaging {
    /// <https://html.spec.whatwg.org/#channel-messaging>
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

    /// <https://html.spec.whatwg.org/#channel-messaging>
    fn trace(&self, event: &str, args: Vec<String>) {
        let Some(sender) = &self.trace_sender else {
            return;
        };
        let mut tracer = TLATracer::new("MessagePort", "formal-web:content", Some(sender.clone()));
        tracer.log_with_location(Some("MessagePort"), event, args, file!(), line!());
    }

    /// <https://html.spec.whatwg.org/#entangle>
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
            in_flight: 0,
        });
        ports.push(PortRecord {
            port_id: port2,
            object: Some(object2),
            ts: TransferState::Managed,
            entangled: Some(port1),
            queue: VecDeque::new(),
            enabled: false,
            detached: false,
            in_flight: 0,
        });
        drop(ports);
        self.trace(
            "NewChannel",
            vec![
                port1.to_string(),
                port2.to_string(),
                self.event_loop_id.to_string(),
            ],
        );
    }

    /// <https://html.spec.whatwg.org/#message-ports:transfer-receiving-steps>
    pub(crate) fn receive_transferred_port(
        &self,
        port_id: PortId,
        object: JsObject,
        remote_port: Option<PortId>,
        queue: Vec<PortMessagePayload>,
        in_flight: u32,
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
            in_flight,
        });
        drop(ports);
        self.trace(
            "TransferReceive",
            vec![port_id.to_string(), self.event_loop_id.to_string()],
        );
    }

    /// <https://html.spec.whatwg.org/#message-ports:transfer-steps>
    pub(crate) fn transfer_port(
        &self,
        port_id: PortId,
        event_sender: &IpcSender<ContentEvent>,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Result<(Vec<PortMessagePayload>, u32), String> {
        let (queue, in_flight, removed) = {
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
            let in_flight = ports[index].in_flight;
            ports.remove(index);
            (queue, in_flight, true)
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
            .map_err(|error| {
                format!("failed to notify the user agent of port transfer: {error}")
            })?;
        Ok((queue, in_flight))
    }

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
        let (direct, target_index) = {
            let ports = self.ports.borrow_mut(ec);
            let Some(src_index) = ports.iter().position(|record| record.port_id == src_id) else {
                // The source port is detached (transferred or closed); its
                // entanglement was severed, so targetPort is null.
                return Ok(());
            };
            if ports[src_index].entangled != Some(target_id) {
                return Ok(());
            }
            let target_index = ports.iter().position(|record| record.port_id == target_id);
            // The port message queue is FIFO, so the message may be delivered
            // directly only when nothing older is still in flight toward the
            // target (routed messages that have not landed in its queue yet);
            // otherwise it is appended after them in the routing queue.
            let direct = ports[src_index].ts == TransferState::Managed
                && target_index.is_some_and(|index| {
                    ports[index].ts == TransferState::Managed && ports[index].in_flight == 0
                });
            (direct, target_index)
        };
        // `MessagePort.tla`'s `PostMessage` (the message port post message steps' step 7:
        // add a task to the port message queue of targetPort).  The source is
        // managed by this event loop whenever its record is held here, so the
        // action is recorded for both direct and routed delivery.
        self.trace(
            "PostMessage",
            vec![
                src_id.to_string(),
                self.event_loop_id.to_string(),
                msg.message_id.to_string(),
            ],
        );
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
            if let Some(target_index) = target_index {
                let mut ports = self.ports.borrow_mut(ec);
                if let Some(record) = ports.get_mut(target_index) {
                    record.in_flight = record.in_flight.saturating_add(1);
                }
            }
            event_sender
                .send(ContentEvent::PortMessageRouted {
                    tgt: target_id,
                    msg,
                })
                .map_err(|error| format!("failed to route port message: {error}"))?;
        }
        Ok(())
    }

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

    /// <https://html.spec.whatwg.org/#message-port-post-message-steps>
    pub(crate) fn handle_port_task(
        &self,
        port_id: PortId,
        task: PortTaskKind,
        event_sender: &IpcSender<ContentEvent>,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Result<bool, String> {
        match task {
            PortTaskKind::NewTask { msg } => {
                let local = {
                    let ports = self.ports.borrow_mut(ec);
                    let Some(index) = ports.iter().position(|record| record.port_id == port_id)
                    else {
                        // The port was transferred away (or closed) before the
                        // routed task ran; the task is returned to the routing
                        // queue by the caller.
                        return Ok(false);
                    };
                    ports[index].is_local()
                };
                if local {
                    let mut ports = self.ports.borrow_mut(ec);
                    let Some(index) = ports.iter().position(|record| record.port_id == port_id)
                    else {
                        return Ok(false);
                    };
                    ports[index].queue.push_back(msg);
                    ports[index].in_flight = ports[index].in_flight.saturating_sub(1);
                    let fire = ports[index].enabled && !ports[index].queue.is_empty();
                    drop(ports);
                    self.trace(
                        "RunTask",
                        vec![
                            self.event_loop_id.to_string(),
                            port_id.to_string(),
                            String::from("NewTask"),
                        ],
                    );
                    Ok(fire)
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
                    Ok(false)
                }
            }
            PortTaskKind::Buffer { buf } => {
                let completed = {
                    let mut ports = self.ports.borrow_mut(ec);
                    let Some(index) = ports.iter().position(|record| record.port_id == port_id)
                    else {
                        // The port was transferred away before the completion
                        // task ran; the buffer is returned to the routing queue.
                        return Ok(false);
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
                        return Ok(false);
                    };
                    let landed = buf.len() as u32;
                    for msg in buf {
                        ports[index].queue.push_back(msg);
                    }
                    ports[index].in_flight = ports[index].in_flight.saturating_sub(landed);
                    ports[index].ts = TransferState::Managed;
                    let fire = ports[index].enabled && !ports[index].queue.is_empty();
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
                    // user agent completes the transfer; the message tasks for
                    // the moved messages are requested here (or run inline by
                    // the caller).
                    event_sender
                        .send(ContentEvent::PortTransferCompleted { tgt: port_id })
                        .map_err(|error| {
                            format!("failed to notify port transfer completion: {error}")
                        })?;
                    Ok(fire)
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
                    Ok(false)
                }
            }
        }
    }

    /// <https://html.spec.whatwg.org/#message-port-post-message-steps>
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
            vec![
                self.event_loop_id.to_string(),
                port_id.to_string(),
                String::from(kind),
            ],
        );
        result
    }

    /// <https://html.spec.whatwg.org/#message-port-post-message-steps>
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
        if let Some(payload) = &popped {
            // `MessagePort.tla`'s `ReceiveMessage` action: the message task ran and
            // the queue popped.  The message id is recorded so the trace
            // consumer can check the pop against the abstract queue head.
            self.trace(
                "ReceiveMessage",
                vec![
                    port_id.to_string(),
                    self.event_loop_id.to_string(),
                    payload.message_id.to_string(),
                ],
            );
            // The queue may hold further messages; each fires in its own task.
            self.request_message_tasks(port_id, event_sender, ec)?;
        }
        Ok(popped)
    }

    /// <https://html.spec.whatwg.org/#message-ports>
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

    /// <https://html.spec.whatwg.org/#message-ports>
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

    /// <https://html.spec.whatwg.org/#message-ports>
    pub(crate) fn has_port(&self, port_id: PortId, ec: &mut dyn ExecutionContext<Types>) -> bool {
        self.ports
            .borrow(ec)
            .iter()
            .any(|record| record.port_id == port_id)
    }

    /// <https://html.spec.whatwg.org/#port-message-queue>
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
