---------------------------- MODULE RenderingOpportunity ----------------------------
EXTENDS Naturals, FiniteSets, TLC

(*
  Generation-counter model with batched rendering opportunities.

  Each frame tracks:
    pending[f]           — render requests that started a render cycle
    rendering_updated[f] — renders completed by content
    composed[f]          — renders whose graphical output was computed and sent by graphics
    op_count[f]          — batched opportunities noted while a render was in flight

  NoteRenderingOpportunity is always enabled.  When no render is in flight
  (pending <= composed) it starts a new render.  Otherwise it increments
  the batch counter (op_count).  NoteComposedScene drains the batch:
  it resets op_count and, if any opportunities were batched, starts a
  new render.
*)

CONSTANTS
  Frame,
  NONE,
  MaxCounter

ASSUME
  /\ Frame # {}
  /\ NONE \notin Frame
  /\ MaxCounter > 0

VARIABLES
  pending,           \* [Frame -> 0..MaxCounter]
  rendering_updated, \* [Frame -> 0..MaxCounter]
  composed,          \* [Frame -> 0..MaxCounter]
  op_count,          \* [Frame -> 0..MaxCounter]
  animating,         \* [Frame -> BOOLEAN]
  parent             \* [Frame -> Frame \cup {NONE}]

vars == <<pending, rendering_updated, composed, op_count, animating, parent>>

\* ---- Helper operators ----

RECURSIVE Ancestors(_)
Ancestors(f) == {f} \cup IF parent[f] = NONE THEN {} ELSE Ancestors(parent[f])

HierarchyRoot(f) == CHOOSE r \in Ancestors(f): parent[r] = NONE

HierarchyFrames(root) == {f \in Frame: HierarchyRoot(f) = root}

\* ---- Type correctness ----

TypeOK ==
  /\ pending \in [Frame -> 0..MaxCounter]
  /\ rendering_updated \in [Frame -> 0..MaxCounter]
  /\ composed \in [Frame -> 0..MaxCounter]
  /\ op_count \in [Frame -> 0..MaxCounter]
  /\ animating \in [Frame -> BOOLEAN]
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
  /\ parent = [f \in Frame |-> NONE]

\* ---- Actions ----

\* Always enabled.  When no render is in flight (pending <= composed) starts
\* a new render by incrementing pending.  Otherwise batches the opportunity
\* by incrementing op_count.
NoteRenderingOpportunity(f) ==
  IF pending[f] <= composed[f] /\ pending[f] < MaxCounter
  THEN /\ pending' = [pending EXCEPT ![f] = pending[f] + 1]
       /\ UNCHANGED <<rendering_updated, composed, op_count, animating, parent>>
  ELSE /\ pending[f] > composed[f]
       /\ op_count[f] < MaxCounter
       /\ op_count' = [op_count EXCEPT ![f] = op_count[f] + 1]
       /\ UNCHANGED <<pending, rendering_updated, composed, animating, parent>>

\* Content renders frame f.  Enabled when content is behind (rendering_updated <
\* pending).  Advances rendering_updated to match pending.
UpdateTheRendering(f) ==
  /\ rendering_updated[f] < pending[f]
  /\ rendering_updated' = [rendering_updated EXCEPT ![f] = pending[f]]
  /\ UNCHANGED <<pending, composed, op_count, animating, parent>>

\* Graphics finishes a frame's output: the composed scene was rendered and the
\* pixels were sent (PixelFrameReady).  Enabled when content has rendered
\* something not yet output (rendering_updated > composed).
GraphicsComputed(f) ==
  /\ rendering_updated[f] > composed[f]
  /\ composed' = [composed EXCEPT ![f] = rendering_updated[f]]
  /\ UNCHANGED <<pending, rendering_updated, op_count, animating, parent>>

\* UA processes composition completion.  Drains batched opportunities: resets
\* op_count to zero, and if any were batched, starts a new render by
\* incrementing pending.
NoteComposedScene(f) ==
  /\ composed[f] = pending[f]    \* composition must have completed
  /\ op_count' = [op_count EXCEPT ![f] = 0]
  /\ IF (op_count[f] > 0 \/ animating[f]) /\ pending[f] < MaxCounter
     THEN pending' = [pending EXCEPT ![f] = pending[f] + 1]
     ELSE pending' = pending
  /\ UNCHANGED <<rendering_updated, composed, animating, parent>>

\* ---- Next-state relation ----

Next ==
  \/ \E f \in Frame: NoteRenderingOpportunity(f)
  \/ \E f \in Frame: UpdateTheRendering(f)
  \/ \E f \in Frame: GraphicsComputed(f)
  \/ \E f \in Frame: NoteComposedScene(f)

\* ---- Fairness ----

Fairness ==
  /\ \A f \in Frame: WF_vars(UpdateTheRendering(f))
  /\ \A f \in Frame: WF_vars(GraphicsComputed(f))
  /\ \A f \in Frame: WF_vars(NoteComposedScene(f))

Spec == Init /\ [][Next]_vars /\ Fairness

\* ---- Invariants ----

PendingLeadsRendering ==
  \A f \in Frame: pending[f] >= rendering_updated[f]

RenderingLeadsComposed ==
  \A f \in Frame: rendering_updated[f] >= composed[f]

\* ---- Liveness ----

\* Batched opportunities are eventually serviced: op_count drops to zero.
OpportunitiesServiced ==
  \A f \in Frame: (op_count[f] > 0) ~> (op_count[f] = 0)

THEOREM
    Spec => (TypeOK /\ PendingLeadsRendering /\ RenderingLeadsComposed)
=============================================================================
