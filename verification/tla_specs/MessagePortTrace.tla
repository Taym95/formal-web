------------------------- MODULE MessagePortTrace -------------------------
EXTENDS Naturals, Sequences, TLC, MessagePortTraceData

\* The trace consumer for the MessagePort spec: replays the events recorded
\* by the content processes and the user agent against the
\* MessagePortExtraFG model (the cross-process port workflow with the
\* routing queue, per-event-loop task queues, and transfer state machine).

NONE == "<<NONE>>"

VARIABLES
    port_state,
    routing_queue,
    el_tasks,
    trace_index

vars == <<port_state, routing_queue, el_tasks, trace_index>>

Base == INSTANCE MessagePortExtraFG WITH
    PortId      <- TracePortIDs,
    EventLoopId <- TraceEventLoopIDs,
    MessageId   <- TraceMessageIDs,
    NoPortId    <- NONE,
    NoEventLoopId <- NONE,
    port_state    <- port_state,
    routing_queue <- routing_queue,
    el_tasks      <- el_tasks

TraceLength == Len(Trace)

CurrentEntry ==
    IF trace_index \in 1..TraceLength
    THEN Trace[trace_index]
    ELSE [event |-> "", event_args |-> <<>>]

CurrentEvent == CurrentEntry.event
CurrentArgs == CurrentEntry.event_args

EventArg(position) == CurrentArgs[position]

Advance == trace_index' = trace_index + 1

Init ==
    /\ Base!Init
    /\ trace_index = 1

NewChannelTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "NewChannel"
    /\ Len(CurrentArgs) = 3
    /\ LET p1 == EventArg(1)
           p2 == EventArg(2)
           el == EventArg(3)
       IN
       /\ Base!NewChannel(p1, p2, el)
       /\ Advance

PostMessageTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "PostMessage"
    /\ Len(CurrentArgs) = 3
    /\ LET src == EventArg(1)
           el  == EventArg(2)
           mid == EventArg(3)
       IN
       /\ Base!PostMessage(src, el, mid)
       /\ Advance

ReceiveMessageTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "ReceiveMessage"
    /\ Len(CurrentArgs) = 2
    /\ LET port == EventArg(1)
           el   == EventArg(2)
       IN
       /\ Base!ReceiveMessage(port, el)
       /\ Advance

TransferTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "Transfer"
    /\ Len(CurrentArgs) = 2
    /\ LET id == EventArg(1)
           el == EventArg(2)
       IN
       /\ Base!Transfer(id, el)
       /\ Advance

TransferReceiveTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "TransferReceive"
    /\ Len(CurrentArgs) = 2
    /\ LET id == EventArg(1)
           el == EventArg(2)
       IN
       /\ Base!TransferReceive(id, el)
       /\ Advance

RouteMessageTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "RouteMessage"
    /\ routing_queue /= <<>>
    /\ LET item == Head(routing_queue) IN
       /\ item.tgt  = EventArg(2)
       /\ item.kind = EventArg(1)
       /\ Base!RouteMessage
    /\ Advance

RunTaskTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "RunTask"
    /\ Len(CurrentArgs) = 3
    /\ LET el    == EventArg(1)
           port  == EventArg(2)
           kind  == EventArg(3)
       IN
       /\ el_tasks[el] /= <<>>
       /\ LET task == Head(el_tasks[el]) IN
          /\ task.port = port
          /\ task.kind = kind
       /\ Base!RunTask(el)
       /\ Advance

Done ==
    /\ trace_index > TraceLength
    /\ UNCHANGED vars

Next ==
    \/ NewChannelTrace
    \/ PostMessageTrace
    \/ ReceiveMessageTrace
    \/ TransferTrace
    \/ TransferReceiveTrace
    \/ RouteMessageTrace
    \/ RunTaskTrace
    \/ Done

Spec == Init /\ [][Next]_vars

=============================================================================
