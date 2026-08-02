------------------------- MODULE RenderingOpportunityTrace -------------------------
EXTENDS Naturals, Sequences, TLC, RenderingOpportunityTraceData

VARIABLES
    pending,
    rendering_updated,
    composed,
    op_count,
    animating,
    frame_needed,
    parent,
    trace_index

vars == <<pending, rendering_updated, composed, op_count, animating,
          frame_needed, parent, trace_index>>

Base == INSTANCE RenderingOpportunity WITH
    Frame <- TraceFrame,
    NONE <- TraceNone,
    MaxCounter <- TraceMaxCounter

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
    /\ LET f == EventArg(1)
       IN
       /\ Base!NoteRenderingOpportunity(f)
       /\ Advance

FrameNeededTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "FrameNeeded"
    /\ Len(CurrentArgs) = 1
    /\ LET f == EventArg(1)
       IN
       /\ Base!FrameNeeded(f)
       /\ Advance

UpdateTheRenderingTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "UpdateTheRendering"
    /\ Len(CurrentArgs) = 1
    /\ LET f == EventArg(1)
       IN
       /\ Base!UpdateTheRendering(f)
       /\ Advance

GraphicsComputedTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "GraphicsComputed"
    /\ Len(CurrentArgs) = 1
    /\ LET f == EventArg(1)
       IN
       /\ Base!GraphicsComputed(f)
       /\ Advance

Done ==
    /\ trace_index > TraceLength
    /\ UNCHANGED vars

Next ==
    \/ NoteRenderingOpportunityTrace
    \/ FrameNeededTrace
    \/ UpdateTheRenderingTrace
    \/ GraphicsComputedTrace
    \/ Done

TypeOK == Base!TypeOK

TraceAccepted == trace_index = TraceLength + 1
=============================================================================
