------------------------- MODULE RenderingOpportunityTrace -------------------------
EXTENDS Naturals, Sequences, TLC, RenderingOpportunityTraceData

VARIABLES
    webviews,
    trace_index

vars == <<webviews, trace_index>>

Base == INSTANCE RenderingOpportunity WITH
    WebViews <- TraceWebViews,
    MaxRenderingOps <- TraceMaxRenderingOps

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

NoteRenderingOpportunityTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "NoteRenderingOpportunity"
    /\ Len(CurrentArgs) = 1
    /\ LET w == EventArg(1)
       IN
       /\ Base!NoteRenderingOpportunity(w)
       /\ Advance

FrameRenderedTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "FrameRendered"
    /\ Len(CurrentArgs) = 1
    /\ LET w == EventArg(1)
       IN
       /\ Base!FrameRendered(w)
       /\ Advance

Done ==
    /\ trace_index > TraceLength
    /\ UNCHANGED vars

Next ==
    \/ NoteRenderingOpportunityTrace
    \/ FrameRenderedTrace
    \/ Done

TypeOK == Base!TypeOK

NoMissedOpportunity == Base!NoMissedOpportunity

PendingIsAtMostOne == Base!PendingIsAtMostOne

TraceAccepted == trace_index = TraceLength + 1
=============================================================================
