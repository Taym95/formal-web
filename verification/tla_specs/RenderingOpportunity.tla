---------------------------- MODULE RenderingOpportunity ----------------------------
EXTENDS Naturals, FiniteSets, TLC

(* Validates the FrameNeeded-gated render cycle and its double-buffer bound. *)

CONSTANTS
  Frame,
  NONE,
  BufferCount

ASSUME
  /\ Frame # {}
  /\ NONE \notin Frame
  /\ BufferCount > 0

VARIABLES
  pending,           \* queued update the rendering for f (UA pending_update_the_rendering)
  rendering_updated, \* content completed the update (UpdateTheRendering)
  composed,          \* graphics sent the pixels (PixelFrameReady)
  op_count,          \* a batched rendering opportunity (UA queued_rendering_opportunities, 0 or 1)
  animating,         \* the last composed frame had animated content
  frame_needed,      \* the embedder needs a frame (FrameNeeded, sent at each paint)
  parent             \* navigable hierarchy: parent navigable of f (NONE at the top-level traversable)

vars == <<pending, rendering_updated, composed, op_count, animating,
          frame_needed, parent>>

\* ---- Helper operators ----

RECURSIVE Ancestors(_)
Ancestors(f) == {f} \cup IF parent[f] = NONE THEN {} ELSE Ancestors(parent[f])

HierarchyRoot(f) == CHOOSE r \in Ancestors(f): parent[r] = NONE

HierarchyFrames(root) == {f \in Frame: HierarchyRoot(f) = root}

\* ---- Type correctness ----

\* Counters are relative to the last paint; the pipeline depth is bounded
\* by BufferCount.
TypeOK ==
  /\ pending \in [Frame -> 0..BufferCount]
  /\ rendering_updated \in [Frame -> 0..BufferCount]
  /\ composed \in [Frame -> 0..BufferCount]
  /\ op_count \in [Frame -> 0..1]
  /\ animating \in [Frame -> BOOLEAN]
  /\ frame_needed \in [Frame -> BOOLEAN]
  /\ parent \in [Frame -> (Frame \cup {NONE})]
  /\ \A f \in Frame: parent[f] # NONE => parent[f] \in Frame
  /\ \A f \in Frame: parent[f] # f
  /\ \A f \in Frame: parent[f] # NONE => f \notin Ancestors(parent[f])

\* ---- Init ----

Init ==
  /\ pending = [f \in Frame |-> 0]
  /\ rendering_updated = [f \in Frame |-> 0]
  /\ composed = [f \in Frame |-> 0]
  /\ op_count = [f \in Frame |-> 0]
  /\ animating = [f \in Frame |-> FALSE]
  /\ frame_needed = [f \in Frame |-> FALSE]
  /\ parent = [f \in Frame |-> NONE]

\* ---- Actions ----

\* An update is in flight for f when graphics has not yet computed it.
InFlight(f) == pending[f] > composed[f]

\* The UA noted a rendering opportunity for f (input, navigation, viewport
\* change, or content request). When an update is in flight or the embedder
\* does not need a frame, the opportunity is batched (set semantics:
\* repeated notes stay one batched opportunity); otherwise update the
\* rendering is queued and the batch drains.
NoteRenderingOpportunity(f) ==
  IF InFlight(f) \/ ~frame_needed[f]
  THEN /\ op_count' = [op_count EXCEPT ![f] = 1]
       /\ UNCHANGED <<pending, rendering_updated, composed, animating,
                      frame_needed, parent>>
  ELSE /\ pending[f] < BufferCount
       /\ pending' = [pending EXCEPT ![f] = pending[f] + 1]
       /\ frame_needed' = [frame_needed EXCEPT ![f] = FALSE]
       /\ op_count' = [op_count EXCEPT ![f] = 0]
       /\ UNCHANGED <<rendering_updated, composed, animating, parent>>

\* The embedder's paint (FrameNeeded): the paint consumes the composed
\* frames — the counters drop by composed — and if the pipeline is drained
\* and something is to render, queues the next update (the double buffering:
\* the next frame renders while the previous one is still being displayed).
FrameNeeded(f) ==
  LET in_flight == pending[f] - composed[f]
      consume_rendering_updated == rendering_updated[f] - composed[f]
      can_start == in_flight = 0 /\ (op_count[f] > 0 \/ animating[f])
  IN
  /\ composed' = [composed EXCEPT ![f] = 0]
  /\ rendering_updated' = [rendering_updated EXCEPT ![f] = consume_rendering_updated]
  /\ IF can_start
     THEN /\ pending' = [pending EXCEPT ![f] = 1]
          /\ frame_needed' = [frame_needed EXCEPT ![f] = FALSE]
          /\ op_count' = [op_count EXCEPT ![f] = 0]
     ELSE /\ pending' = [pending EXCEPT ![f] = in_flight]
          /\ frame_needed' = [frame_needed EXCEPT ![f] = TRUE]
          /\ op_count' = op_count
  /\ UNCHANGED <<animating, parent>>

\* Content runs the queued update: rendering_updated catches up to pending.
UpdateTheRendering(f) ==
  /\ rendering_updated[f] < pending[f]
  /\ rendering_updated' = [rendering_updated EXCEPT ![f] = pending[f]]
  /\ UNCHANGED <<pending, composed, op_count, animating, frame_needed, parent>>

\* Graphics sent the frame (PixelFrameReady): composed catches up to
\* rendering_updated. Animated content batches the next opportunity, and a
\* held frame_needed flag with something to render queues the next update
\* immediately.
GraphicsComputed(f) ==
  /\ rendering_updated[f] > composed[f]
  /\ composed' = [composed EXCEPT ![f] = rendering_updated[f]]
  /\ IF frame_needed[f] /\ (op_count[f] > 0 \/ animating[f])
        /\ pending[f] < BufferCount
     THEN /\ pending' = [pending EXCEPT ![f] = pending[f] + 1]
          /\ frame_needed' = [frame_needed EXCEPT ![f] = FALSE]
          /\ op_count' = [op_count EXCEPT ![f] = 0]
     ELSE /\ IF animating[f]
           THEN op_count' = [op_count EXCEPT ![f] = 1]
           ELSE op_count' = op_count
          /\ pending' = pending
          /\ frame_needed' = frame_needed
  /\ UNCHANGED <<rendering_updated, animating, parent>>

\* ---- Next-state relation ----

Next ==
  \/ \E f \in Frame: NoteRenderingOpportunity(f)
  \/ \E f \in Frame: FrameNeeded(f)
  \/ \E f \in Frame: UpdateTheRendering(f)
  \/ \E f \in Frame: GraphicsComputed(f)

\* ---- Fairness ----

Fairness ==
  /\ \A f \in Frame: WF_vars(FrameNeeded(f))
  /\ \A f \in Frame: WF_vars(UpdateTheRendering(f))
  /\ \A f \in Frame: WF_vars(GraphicsComputed(f))

Spec == Init /\ [][Next]_vars /\ Fairness

\* ---- Invariants ----

\* A queued update is completed by content before graphics computes it.
PendingLeadsRendering ==
  \A f \in Frame: pending[f] >= rendering_updated[f]

RenderingLeadsComposed ==
  \A f \in Frame: rendering_updated[f] >= composed[f]

\* The pipeline never holds more than BufferCount updates queued but not yet
\* consumed by the embedder's paint (the double buffer).
DoubleBufferBound ==
  \A f \in Frame: pending[f] <= BufferCount

\* ---- Liveness ----

\* Batched opportunities are serviced when the embedder needs a frame: once
\* a frame is needed and a render can start, the batch drains to zero.
OpportunitiesServiced ==
  \A f \in Frame:
    ((op_count[f] > 0 /\ frame_needed[f] /\ pending[f] < BufferCount)
       ~> (op_count[f] = 0))

THEOREM
    Spec => (TypeOK /\ PendingLeadsRendering /\ RenderingLeadsComposed
             /\ DoubleBufferBound)
=============================================================================
