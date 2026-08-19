//! User-agent-side channel messaging: the routing queue and per-port
//! transfer state of the cross-process MessagePort workflow modelled by
//! `verification/tla_specs/MessagePortExtraFG.tla`.
//!
//! The content processes own the port records of the ports their event
//! loops manage (`content/src/html/channel_messaging.rs`); the user agent
//! owns the `routing_queue` and the per-port `{ ts, owner, buf }` needed by
//! `RouteMessage` to deliver messages between event loops and to buffer them
//! while a port is in transit.

use std::collections::{HashMap, VecDeque};

use ipc_messages::content::{Command as ContentCommand, EventLoopId, PortId, PortTaskKind, TransferState};
use ipc_messages::safe_passing_of_structured_data::PortMessagePayload;
use log::warn;

use verification::{TLATracer, TraceSender};

/// One item of the routing queue (the `routing_queue` of MessagePortExtraFG).
enum RoutingItem {
    /// A message posted to a port not managed by the sender's event loop.
    Single {
        tgt: PortId,
        msg: PortMessagePayload,
    },
    /// A transfer-completion task that ran on an event loop whose port was
    /// transferred away; the buffer is routed back to the new owner.
    ReturnedBuffer {
        tgt: PortId,
        buf: Vec<PortMessagePayload>,
    },
    /// A transfer-completion task ran on the port's owning event loop; the
    /// port is now managed again.
    Success { tgt: PortId },
}

/// The user agent's per-port state (the `port_state[id]` of
/// MessagePortExtraFG, restricted to the fields `RouteMessage` needs).
struct UaPortState {
    /// The port's transfer state (`MessagePortExtraFG.tla`'s `ts`).
    ts: TransferState,
    /// The event loop owning the port; `None` while the port is in transit
    /// (`MessagePortExtraFG.tla`'s `owner` with `NoEventLoopId`).
    owner: Option<EventLoopId>,
    /// Messages buffered while the port is in transit (`MessagePortExtraFG.tla`'s `buf`).
    buf: VecDeque<PortMessagePayload>,
}

/// The user agent's channel messaging state.
pub(crate) struct ChannelMessaging {
    ports: HashMap<PortId, UaPortState>,
    routing_queue: VecDeque<RoutingItem>,
    /// TLA trace sender for the MessagePort spec (`MessagePortExtraFG.tla`'s `RouteMessage`
    /// action traced from the user agent).
    trace_sender: Option<TraceSender>,
}

impl ChannelMessaging {
    pub(crate) fn new(trace_sender: Option<TraceSender>) -> Self {
        Self {
            ports: HashMap::new(),
            routing_queue: VecDeque::new(),
            trace_sender,
        }
    }

    /// Emit a MessagePort trace event (the actions of MessagePortExtraFG.tla).
    fn trace(&self, event: &str, args: Vec<String>) {
        let Some(sender) = &self.trace_sender else {
            return;
        };
        let mut tracer = TLATracer::new("MessagePort", "formal-web:user-agent", Some(sender.clone()));
        tracer.log_with_location(Some("MessagePort"), event, args, file!(), line!());
    }

    /// `MessagePortExtraFG.tla`'s `NewChannel`: register the two ports of a new channel
    /// with their owning event loop.
    fn new_channel(&mut self, port1: PortId, port2: PortId, event_loop: EventLoopId) {
        self.ports.insert(
            port1,
            UaPortState {
                ts: TransferState::Managed,
                owner: Some(event_loop),
                buf: VecDeque::new(),
            },
        );
        self.ports.insert(
            port2,
            UaPortState {
                ts: TransferState::Managed,
                owner: Some(event_loop),
                buf: VecDeque::new(),
            },
        );
    }

    /// `MessagePortExtraFG.tla`'s `Transfer`: a port left its owning event loop during
    /// structured serialization.
    fn transfer_started(&mut self, port: PortId) {
        let state = self.ports.entry(port).or_insert(UaPortState {
            ts: TransferState::TransferInProgress,
            owner: None,
            buf: VecDeque::new(),
        });
        state.buf.clear();
        match state.ts {
            TransferState::Managed => {
                state.ts = TransferState::TransferInProgress;
                state.owner = None;
            }
            TransferState::CompletionInProgress => {
                state.ts = TransferState::CompletionFailed;
                state.owner = None;
            }
            _ => {
                // A port already in transit is transferred again only after
                // it was received somewhere; keep the state as is.
            }
        }
    }

    /// `MessagePortExtraFG.tla`'s `TransferReceive`: a transferred port was received by an
    /// event loop during structured deserialization.
    fn transfer_received(
        &mut self,
        port: PortId,
        event_loop: EventLoopId,
        send_task: &mut dyn FnMut(EventLoopId, ContentCommand),
    ) {
        let Some(state) = self.ports.get_mut(&port) else {
            warn!("transfer receive: unknown port {port}");
            return;
        };
        match state.ts {
            TransferState::TransferInProgress => {
                state.ts = TransferState::CompletionInProgress;
                state.owner = Some(event_loop);
                // The messages buffered while the port was in transit move to
                // the receiving event loop as a completion task.
                let buf: Vec<PortMessagePayload> = std::mem::take(&mut state.buf).into();
                send_task(
                    event_loop,
                    ContentCommand::PortTask {
                        port,
                        task: PortTaskKind::Buffer { buf },
                    },
                );
            }
            TransferState::CompletionFailed | TransferState::CompletionRequested => {
                state.ts = TransferState::CompletionRequested;
                state.owner = Some(event_loop);
            }
            _ => {
                warn!("transfer receive: port {port} is not in transit ({:?})", state.ts);
            }
        }
    }

    /// `MessagePortExtraFG.tla`'s `PostMessage` routed branch: append a "Single" item to
    /// the routing queue and process it.
    pub(crate) fn route_single(
        &mut self,
        tgt: PortId,
        msg: PortMessagePayload,
        send_task: &mut dyn FnMut(EventLoopId, ContentCommand),
    ) {
        self.routing_queue
            .push_back(RoutingItem::Single { tgt, msg });
        self.process_routing_queue(send_task);
    }

    /// `MessagePortExtraFG.tla`'s `RunTask` with a port no longer owned by the event loop:
    /// a "ReturnedBuffer" item returns the completion buffer to the routing
    /// queue.
    pub(crate) fn route_returned_buffer(
        &mut self,
        tgt: PortId,
        buf: Vec<PortMessagePayload>,
        send_task: &mut dyn FnMut(EventLoopId, ContentCommand),
    ) {
        self.routing_queue
            .push_back(RoutingItem::ReturnedBuffer { tgt, buf });
        self.process_routing_queue(send_task);
    }

    /// `MessagePortExtraFG.tla`'s `RunTask` with a port owned by the event loop: a
    /// "Success" item completes the transfer at the user agent.
    pub(crate) fn route_success(
        &mut self,
        tgt: PortId,
        send_task: &mut dyn FnMut(EventLoopId, ContentCommand),
    ) {
        self.routing_queue.push_back(RoutingItem::Success { tgt });
        self.process_routing_queue(send_task);
    }

    /// `MessagePortExtraFG.tla`'s `RouteMessage`: process the head of the routing queue,
    /// one item at a time, queueing tasks on the ports' event loops and
    /// buffering messages for ports in transit.
    fn process_routing_queue(&mut self, send_task: &mut dyn FnMut(EventLoopId, ContentCommand)) {
        while let Some(item) = self.routing_queue.pop_front() {
            let tgt = match &item {
                RoutingItem::Single { tgt, .. }
                | RoutingItem::ReturnedBuffer { tgt, .. }
                | RoutingItem::Success { tgt } => *tgt,
            };
            // Decide the transition and what to send while the state borrow
            // is live; the trace emission and the send happen after it drops.
            let decision = match self.ports.get_mut(&tgt) {
                Some(state) => {
                    let msg_id = match &item {
                        RoutingItem::Single { msg, .. } => Some(msg.message_id.to_string()),
                        _ => None,
                    };
                    match (state.ts, item) {
                        // If item is a "Success" item and the port is
                        // completing its transfer, the transfer completes.
                        (TransferState::CompletionInProgress, RoutingItem::Success { tgt }) => {
                            state.ts = TransferState::Managed;
                            // The completion task queued the transfer's
                            // buffered messages on the port's queue.  Any
                            // messages buffered at the user agent (routed
                            // while the port was completing or in transit,
                            // with no event loop to complete via a returned
                            // buffer — e.g. a same-realm re-transfer) are
                            // shipped now as a completion task.
                            let command = if !state.buf.is_empty() {
                                let buf: Vec<PortMessagePayload> =
                                    std::mem::take(&mut state.buf).into();
                                state.owner.map(|owner| {
                                    (
                                        owner,
                                        ContentCommand::PortTask {
                                            port: tgt,
                                            task: PortTaskKind::Buffer { buf },
                                        },
                                    )
                                })
                            } else {
                                state.owner.map(|owner| {
                                    (
                                        owner,
                                        ContentCommand::RunPortMessageTask { port: tgt },
                                    )
                                })
                            };
                            RouteDecision {
                                kind: "Success",
                                tgt,
                                msg_id: None,
                                command,
                            }
                        }
                        // A transfer completion notification for a port that
                        // is already managed is a no-op (a same-realm
                        // re-transfer can complete twice).
                        (TransferState::Managed, RoutingItem::Success { tgt }) => RouteDecision {
                            kind: "Success",
                            tgt,
                            msg_id: None,
                            command: None,
                        },
                        // A transfer completion for a port whose completion
                        // was requested after a failed re-transfer completes
                        // the port (the completion task ran on the receiving
                        // event loop) and ships any buffered messages.
                        (TransferState::CompletionRequested, RoutingItem::Success { tgt }) => {
                            state.ts = TransferState::Managed;
                            let command = if !state.buf.is_empty() {
                                let buf: Vec<PortMessagePayload> =
                                    std::mem::take(&mut state.buf).into();
                                state.owner.map(|owner| {
                                    (
                                        owner,
                                        ContentCommand::PortTask {
                                            port: tgt,
                                            task: PortTaskKind::Buffer { buf },
                                        },
                                    )
                                })
                            } else {
                                state.owner.map(|owner| {
                                    (
                                        owner,
                                        ContentCommand::RunPortMessageTask { port: tgt },
                                    )
                                })
                            };
                            RouteDecision {
                                kind: "Success",
                                tgt,
                                msg_id: None,
                                command,
                            }
                        }
                        // If the port is managed (or completing its transfer)
                        // and the item is a "Single" message, queue a message
                        // task on the port's event loop.
                        (
                            TransferState::Managed | TransferState::CompletionInProgress,
                            RoutingItem::Single { tgt, msg },
                        ) => {
                            let command = state.owner.map(|owner| {
                                (
                                    owner,
                                    ContentCommand::PortTask {
                                        port: tgt,
                                        task: PortTaskKind::NewTask { msg },
                                    },
                                )
                            });
                            RouteDecision {
                                kind: "Single",
                                tgt,
                                msg_id: msg_id.clone(),
                                command,
                            }
                        }
                        // If the port failed to complete its transfer and the
                        // item is a "ReturnedBuffer", the port goes back in
                        // transit and the returned buffer is prepended to its
                        // buffer.
                        (
                            TransferState::CompletionFailed,
                            RoutingItem::ReturnedBuffer { tgt, buf },
                        ) => {
                            state.ts = TransferState::TransferInProgress;
                            state.owner = None;
                            let mut merged: VecDeque<PortMessagePayload> = buf.into();
                            merged.append(&mut state.buf);
                            state.buf = merged;
                            RouteDecision {
                                kind: "ReturnedBuffer",
                                tgt,
                                msg_id: None,
                                command: None,
                            }
                        }
                        // If the port's completion was requested and the item
                        // is a "ReturnedBuffer", the completion proceeds: the
                        // port is completing again and the merged buffer is
                        // queued as a completion task on its event loop.
                        (
                            TransferState::CompletionRequested,
                            RoutingItem::ReturnedBuffer { tgt, buf },
                        ) => {
                            state.ts = TransferState::CompletionInProgress;
                            let mut merged: VecDeque<PortMessagePayload> = buf.into();
                            merged.append(&mut state.buf);
                            state.buf = VecDeque::new();
                            let command = state.owner.map(|owner| {
                                (
                                    owner,
                                    ContentCommand::PortTask {
                                        port: tgt,
                                        task: PortTaskKind::Buffer {
                                            buf: merged.into(),
                                        },
                                    },
                                )
                            });
                            RouteDecision {
                                kind: "ReturnedBuffer",
                                tgt,
                                msg_id: None,
                                command,
                            }
                        }
                        // Otherwise the port is in transit and the item is a
                        // "Single" message: buffer it until the port is
                        // received.
                        (
                            TransferState::TransferInProgress
                            | TransferState::CompletionFailed
                            | TransferState::CompletionRequested,
                            RoutingItem::Single { tgt, msg },
                        ) => {
                            state.buf.push_back(msg);
                            RouteDecision {
                                kind: "Single",
                                tgt,
                                msg_id: msg_id.clone(),
                                command: None,
                            }
                        }
                        (state_ts, item) => {
                            warn!(
                                "route message: unmatched item {:?} for port {tgt} in state {state_ts:?}; dropping",
                                item_kind_name(&item)
                            );
                            RouteDecision {
                                kind: "dropped",
                                tgt,
                                msg_id: None,
                                command: None,
                            }
                        }
                    }
                }
                None => {
                    warn!("route message: unknown port {tgt}; dropping item");
                    RouteDecision {
                        kind: "dropped",
                        tgt,
                        msg_id: None,
                        command: None,
                    }
                }
            };
            let mut args = vec![decision.kind.to_string(), decision.tgt.to_string()];
            if let Some(msg_id) = decision.msg_id {
                args.push(msg_id);
            }
            self.trace("RouteMessage", args);
            if let Some((owner, command)) = decision.command {
                send_task(owner, command);
            }
        }
    }
}

fn item_kind_name(item: &RoutingItem) -> &'static str {
    match item {
        RoutingItem::Single { .. } => "Single",
        RoutingItem::ReturnedBuffer { .. } => "ReturnedBuffer",
        RoutingItem::Success { .. } => "Success",
    }
}

/// The outcome of routing one item: what happened, for the trace, and an
/// optional task command to deliver to an event loop.
struct RouteDecision {
    kind: &'static str,
    tgt: PortId,
    msg_id: Option<String>,
    command: Option<(EventLoopId, ContentCommand)>,
}

/// Handle a content port event: register a channel, a transfer, or a
/// routing item.  `send_task` delivers a task command to an event loop.
pub(crate) fn handle_port_event(
    messaging: &mut ChannelMessaging,
    event: PortEvent,
    send_task: &mut dyn FnMut(EventLoopId, ContentCommand),
) {
    match event {
        PortEvent::ChannelCreated {
            port1,
            port2,
            event_loop,
        } => messaging.new_channel(port1, port2, event_loop),
        PortEvent::TransferStarted { port } => messaging.transfer_started(port),
        PortEvent::TransferReceived { port, event_loop } => {
            messaging.transfer_received(port, event_loop, send_task)
        }
        PortEvent::MessageRouted { tgt, msg } => messaging.route_single(tgt, msg, send_task),
        PortEvent::BufferReturned { tgt, buf } => {
            messaging.route_returned_buffer(tgt, buf, send_task)
        }
        PortEvent::TransferCompleted { tgt } => messaging.route_success(tgt, send_task),
    }
}

/// A port event forwarded from a content process (the events of the
/// MessagePortExtraFG model as observed at the user agent).
pub enum PortEvent {
    ChannelCreated {
        port1: PortId,
        port2: PortId,
        event_loop: EventLoopId,
    },
    TransferStarted {
        port: PortId,
    },
    TransferReceived {
        port: PortId,
        event_loop: EventLoopId,
    },
    MessageRouted {
        tgt: PortId,
        msg: PortMessagePayload,
    },
    BufferReturned {
        tgt: PortId,
        buf: Vec<PortMessagePayload>,
    },
    TransferCompleted {
        tgt: PortId,
    },
}
