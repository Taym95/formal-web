------------------------- MODULE RenderingOpportunityTrace -------------------------
EXTENDS Naturals, Sequences, TLC, RenderingOpportunityTraceData

VARIABLES
    live,
    parent,
    pending,
    rendering_updated,
    composed,
    op_count,
    animating,
    frame_needed,
    trace_index

vars == <<live, parent, pending, rendering_updated, composed, op_count,
          animating, frame_needed, trace_index>>

Base == INSTANCE RenderingOpportunity WITH
    Frame <- TraceFrame,
    NONE <- TraceNone,
    BufferCount <- TraceBufferCount,
    live <- live,
    parent <- parent,
    pending <- pending,
    rendering_updated <- rendering_updated,
    composed <- composed,
    op_count <- op_count,
    animating <- animating,
    frame_needed <- frame_needed

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

\* A navigable was born (CreateNavigable / CreateChildNavigable in the
\* navigation path, traced here too since the frame is the graphics-side
\* effect of navigation): a top-level frame carries one argument, a child
\* frame carries (child, parent).
CreateFrameTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "CreateFrame"
    /\ LET f == EventArg(1)
           parentArg == IF Len(CurrentArgs) = 2 THEN EventArg(2) ELSE TraceNone
       IN
       /\ Base!CreateFrame(f, parentArg)
       /\ Advance

NoteRenderingOpportunityTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "NoteRenderingOpportunity"
    /\ Len(CurrentArgs) = 1
    /\ LET f == EventArg(1)
       IN
       /\ f \in live
       /\ Base!NoteRenderingOpportunity(f)
       /\ Advance

FrameNeededTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "FrameNeeded"
    /\ Len(CurrentArgs) = 1
    /\ LET f == EventArg(1)
       IN
       /\ f \in live
       /\ Base!FrameNeeded(f)
       /\ Advance

UpdateTheRenderingTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "UpdateTheRendering"
    /\ Len(CurrentArgs) = 1
    /\ LET f == EventArg(1)
       IN
       /\ f \in live
       /\ Base!UpdateTheRendering(f)
       /\ Advance

GraphicsComputedTrace ==
    /\ trace_index \in 1..TraceLength
    /\ CurrentEvent = "GraphicsComputed"
    /\ Len(CurrentArgs) = 1
    /\ LET f == EventArg(1)
       IN
       /\ f \in live
       /\ Base!GraphicsComputed(f)
       /\ Advance

Done ==
    /\ trace_index > TraceLength
    /\ UNCHANGED vars

Next ==
    \/ CreateFrameTrace
    \/ NoteRenderingOpportunityTrace
    \/ FrameNeededTrace
    \/ UpdateTheRenderingTrace
    \/ GraphicsComputedTrace
    \/ Done

TypeOK == Base!TypeOK

TraceAccepted == trace_index = TraceLength + 1
=============================================================================
