---------------------------- MODULE RenderingOpportunity ----------------------------
EXTENDS Naturals, FiniteSets, TLC

(*
  Generation-counter model of the FrameNeeded-gated render cycle with
  double-buffered surface delivery.

  All pipeline counters are relative to the last consumption (the
  embedder's paint): the pipeline depth is bounded by BufferCount, which is
  the only bound the model needs. Each frame tracks:
    pending[f]           — renders started but not yet consumed (painted)
    rendering_updated[f] — renders completed by content, not yet consumed
    composed[f]          — renders whose pixels were computed by graphics, not yet consumed
    op_count[f]          — a batched rendering opportunity not yet applied (set semantics, 0 or 1)
    frame_needed[f]      — the embedder needs a frame (sent at each paint, paced by vsync)
    animating[f]         — the last composed frame had animated content
    parent[f]            — parent navigable, for the frame hierarchy

  A render cycle starts only when the embedder needs a frame (frame_needed)
  AND there is something to render (a batched opportunity or animating
  content). NoteRenderingOpportunity and FrameNeeded each start a cycle when
  the other condition is already met; otherwise they batch (op_count) or set
  the flag. A render in flight (pending > composed) batches new opportunities
  and holds a frame_needed flag until the in-flight render completes.
  UpdateTheRendering renders the document; GraphicsComputed completes the
  cycle and, if content is animating or opportunities were batched, keeps
  the next cycle ready.

  Double buffering: the embedder consumes (paints) the composed frames at
  each FrameNeeded — the pipeline counters drop by composed — and the next
  render can start while the previous frame is still being displayed (the
  graphics process renders into the other buffer). The pipeline is bounded:
  pending never exceeds BufferCount (DoubleBufferBound).
*)

CONSTANTS
  Frame,
  NONE,
  BufferCount

ASSUME
  /\ Frame # {}
  /\ NONE \notin Frame
  /\ BufferCount > 0

VARIABLES
  pending,           \* [Frame -> 0..BufferCount]
  rendering_updated, \* [Frame -> 0..BufferCount]
  composed,          \* [Frame -> 0..BufferCount]
  op_count,          \* [Frame -> 0..1]
  animating,         \* [Frame -> BOOLEAN]
  frame_needed,      \* [Frame -> BOOLEAN]
  parent             \* [Frame -> Frame \cup {NONE}]

vars == <<pending, rendering_updated, composed, op_count, animating,
          frame_needed, parent>>

\* ---- Helper operators ----

RECURSIVE Ancestors(_)
Ancestors(f) == {f} \cup IF parent[f] = NONE THEN {} ELSE Ancestors(parent[f])

HierarchyRoot(f) == CHOOSE r \in Ancestors(f): parent[r] = NONE

HierarchyFrames(root) == {f \in Frame: HierarchyRoot(f) = root}

\* ---- Type correctness ----

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

\* A render is in flight for f when a started cycle has not yet been
\* completed by graphics.
InFlight(f) == pending[f] > composed[f]

\* Always enabled.  When a render is in flight, or the embedder does not
\* need a frame, batches the opportunity (set semantics: repeated notes
\* while already batched stay one batched opportunity).  Otherwise, when
\* the embedder needs a frame, starts a render and drains the batch.
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

\* The embedder needs a frame: the paint consumes the composed frames (the
\* embedder blits the most recent texture), so the pipeline counters drop by
\* composed and a render can start while the previous frame is still being
\* displayed — the double buffering.  When the pipeline is drained and there
\* is something to render (a batched opportunity or animating content),
\* starts a render and drains the batch; otherwise the flag stays set until
\* a future opportunity.
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

\* Content renders frame f.  Enabled when content is behind (rendering_updated <
\* pending).  Advances rendering_updated to match pending.
UpdateTheRendering(f) ==
  /\ rendering_updated[f] < pending[f]
  /\ rendering_updated' = [rendering_updated EXCEPT ![f] = pending[f]]
  /\ UNCHANGED <<pending, composed, op_count, animating, frame_needed, parent>>

\* Graphics finishes a frame's output: the composed scene was rendered and the
\* pixels were sent (PixelFrameReady).  Enabled when content has rendered
\* something not yet output (rendering_updated > composed).  Completes the
\* cycle; animated content queues the next frame's opportunity, and a held
\* frame_needed flag with something to render starts the next cycle
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

PendingLeadsRendering ==
  \A f \in Frame: pending[f] >= rendering_updated[f]

RenderingLeadsComposed ==
  \A f \in Frame: rendering_updated[f] >= composed[f]

\* Double buffering: the pipeline never holds more than BufferCount renders
\* started but not yet consumed by the embedder's paint (one being displayed,
\* the rest being rendered into the other buffers).
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
