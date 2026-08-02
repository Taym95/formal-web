------------------------- MODULE GPURenderingTrace -------------------------
EXTENDS Naturals, Sequences, TLC, GPURenderingTraceData

VARIABLES
    generation,
    buffer_size,
    last_buffer,
    submitted_size,
    sent,
    received,
    texture_size,
    trace_index

vars == <<generation, buffer_size, last_buffer, submitted_size,
          sent, received, texture_size, trace_index>>

Base == INSTANCE GPURendering WITH
    Webview <- TraceWebview,
    Generation <- TraceGeneration,
    Size <- TraceSize,
    Region <- TraceRegion,
    NONE <- TraceNone

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

\* Graphics submits frame g at size s into region r (buffer picked at submit).
SurfaceFrameSubmittedTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "SurfaceFrameSubmitted"
    /\ Len(CurrentArgs) = 4
    /\ LET w == EventArg(1)
           g == EventArg(2)
           s == EventArg(3)
           r == EventArg(4)
       IN
       /\ Base!SurfaceFrameSubmitted(w, g, s, r)
       /\ Advance

\* The GPU render for g completed; the pixels were delivered and the frame
\* was sent from region r.
SurfaceFrameSentTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "SurfaceFrameSent"
    /\ Len(CurrentArgs) = 4
    /\ LET w == EventArg(1)
           g == EventArg(2)
           s == EventArg(3)
           r == EventArg(4)
       IN
       /\ Base!SurfaceFrameSent(w, g, s, r)
       /\ Advance

\* Embedder consumes frame g at size s.
SurfaceFrameReceivedTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "SurfaceFrameReceived"
    /\ Len(CurrentArgs) = 3
    /\ LET w == EventArg(1)
           g == EventArg(2)
           s == EventArg(3)
       IN
       /\ Base!SurfaceFrameReceived(w, g, s)
       /\ Advance

Done ==
    /\ trace_index > TraceLength
    /\ UNCHANGED vars

Next ==
    \/ SurfaceFrameSubmittedTrace
    \/ SurfaceFrameSentTrace
    \/ SurfaceFrameReceivedTrace
    \/ Done

TypeOK == Base!TypeOK

TraceAccepted == trace_index = TraceLength + 1
=============================================================================
