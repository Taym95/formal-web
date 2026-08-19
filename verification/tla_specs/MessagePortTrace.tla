------------------------- MODULE MessagePortTrace -------------------------
EXTENDS Naturals, Sequences, TLC, MessagePortTraceData

\* The trace consumer for the MessagePort spec: replays the events recorded
\* by the content processes and the user agent against the coarse
\* `MessagePort.tla` model, which maps one-to-one to the HTML spec's channel
\* messaging steps (NewMessageChannel = entangle, PostMessage = step 7 of the
\* message port post message steps, ReceiveMessage = the message task firing,
\* Transfer/TransferReceive = the transfer steps).  The fine-grained routing
\* and transfer-completion machinery of `MessagePortExtraFG.tla` is an
\* implementation detail and its events (RouteMessage, RunTask) are skipped
\* here; the coarse model's per-port abstract queue preserves the ordering
\* property the message tasks must respect.

NONE == "<<NONE>>"

VARIABLES
    ports,
    trace_index

vars == <<ports, trace_index>>

Base == INSTANCE MessagePort WITH
    PortId        <- TracePortIDs,
    EventLoopId   <- TraceEventLoopIDs,
    MessageId     <- TraceMessageIDs,
    NoPortId      <- NONE,
    NoEventLoopId <- NONE,
    ports         <- ports

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

\* The port record of a port in the abstract model.
PortRecordOf(port) ==
    CHOOSE p \in ports : p.id = port

NewChannelTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "NewChannel"
    /\ Len(CurrentArgs) = 3
    /\ LET p1 == EventArg(1)
           p2 == EventArg(2)
           el == EventArg(3)
       IN
       /\ Base!NewMessageChannel(p1, p2, el)
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

\* The message task fired: the abstract queue head must be the popped
\* message (the port message queue is FIFO).
ReceiveMessageTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "ReceiveMessage"
    /\ Len(CurrentArgs) = 3
    /\ LET port == EventArg(1)
           mid  == EventArg(3)
       IN
       /\ port \in {p.id : p \in ports}
       /\ LET p == PortRecordOf(port) IN
          /\ p.queue /= <<>>
          /\ Head(p.queue) = mid
       /\ Base!ReceiveMessage(port)
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

\* The user agent's routing queue processing and the content processes'
\* task handling are implementation details of the fine-grained model; the
\* abstract queue already received the message at PostMessage time.
RouteMessageTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "RouteMessage"
    /\ UNCHANGED ports
    /\ Advance

RunTaskTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "RunTask"
    /\ UNCHANGED ports
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

================================================================================
