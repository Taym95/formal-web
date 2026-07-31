------------------------- MODULE GPURenderingTrace -------------------------
EXTENDS Naturals, Sequences, TLC, GPURenderingTraceData

VARIABLES
    generation,
    buffer_size,
    buffer_state,
    sent_size,
    received,
    texture_size,
    trace_index

vars == <<generation, buffer_size, buffer_state, sent_size, received,
          texture_size, trace_index>>

Base == INSTANCE GPURendering WITH
    Webview <- TraceWebview,
    Generation <- TraceGeneration,
    Size <- TraceSize,
    Region <- TraceRegion,
    FREE <- TraceFree,
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

\* Graphics sends frame g at size s into region r.
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

\* Embedder acks frame g; the region holding it becomes free.
TextureConsumedTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "TextureConsumed"
    /\ Len(CurrentArgs) = 2
    /\ LET w == EventArg(1)
           g == EventArg(2)
       IN
       /\ Base!TextureConsumed(w, g)
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
    \/ SurfaceFrameSentTrace
    \/ TextureConsumedTrace
    \/ SurfaceFrameReceivedTrace
    \/ Done

TypeOK == Base!TypeOK

TraceAccepted == trace_index = TraceLength + 1
=============================================================================
