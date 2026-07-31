---------------------------- MODULE GPURendering ----------------------------
EXTENDS Naturals, FiniteSets, TLC

(*
  GPU surface buffer workflow: a per-webview three-slot shared-memory ring in
  the graphics process, rendered asynchronously and consumed by the embedder
  with explicit acks.

  Graphics side (producer), per webview:
    generation[w]    -- generation of the most recently SUBMITTED frame (0 before any)
    buffer_size[w]   -- size the three regions were allocated at (NONE before the first frame)
    reserved[w][r]   -- generation of the frame submitted into region r whose GPU
                        readback is still in flight (0 = free)
    pending[w][r]    -- generation of the frame delivered into region r that still
                        awaits the embedder's ack (0 = free)
    submitted_size[w][g]   -- size submitted with generation g (NONE if g never submitted)
    sent[w]          -- generations whose pixels were delivered (PixelFrameReady sent)

  Each composed frame at size s has generation g = generation[w] + 1:
    - SurfaceFrameSubmitted picks a FREE region at submit time (region 0 after a
      fresh allocation) and marks it reserved; the pixels are written into it
      only when the GPU completes the readback.
    - SurfaceFrameSent delivers the completed readback: the region reserved for
      g becomes pending and the frame is sent.  Sent generations are a subset
      of submitted generations (completions may be dropped on error), each sent
      at most once, and the sent region/size must match the submitted ones.
    - TextureConsumed(w, g) frees the region holding g (the embedder's ack).
      A region is only picked for a new frame once it is fully free, so the
      embedder is guaranteed to have consumed the previous frame's pixels.

  Consumer side (embedder), per webview:
    received[w]     -- generations the embedder has consumed
    texture_size[w] -- texture size after the last consumed frame

  A consumed frame must have been sent, and its size must match the size
  submitted with that generation.
*)

CONSTANTS
  Webview,
  Generation,
  Size,
  Region,
  NONE

ASSUME
  /\ Webview # {}
  /\ Generation # {}
  /\ Size # {}
  /\ Region = {0, 1, 2}
  /\ NONE \notin Size
  /\ NONE \notin Generation
  /\ \A g \in Generation: g >= 1

VARIABLES
  generation,
  buffer_size,
  reserved,
  pending,
  submitted_size,
  sent,
  received,
  texture_size

vars == <<generation, buffer_size, reserved, pending, submitted_size,
          sent, received, texture_size>>

\* ---- Type correctness ----

TypeOK ==
  /\ generation \in [Webview -> Generation \cup {0}]
  /\ buffer_size \in [Webview -> Size \cup {NONE}]
  /\ reserved \in [Webview -> [Region -> Generation \cup {0}]]
  /\ pending \in [Webview -> [Region -> Generation \cup {0}]]
  /\ submitted_size \in [Webview -> [Generation -> Size \cup {NONE}]]
  /\ sent \in [Webview -> SUBSET Generation]
  /\ received \in [Webview -> SUBSET Generation]
  /\ texture_size \in [Webview -> Size \cup {NONE}]

\* ---- Init ----

Init ==
  /\ generation = [w \in Webview |-> 0]
  /\ buffer_size = [w \in Webview |-> NONE]
  /\ reserved = [w \in Webview |-> [r \in Region |-> 0]]
  /\ pending = [w \in Webview |-> [r \in Region |-> 0]]
  /\ submitted_size = [w \in Webview |-> [g \in Generation |-> NONE]]
  /\ sent = [w \in Webview |-> {}]
  /\ received = [w \in Webview |-> {}]
  /\ texture_size = [w \in Webview |-> NONE]

\* ---- Actions ----

\* Graphics submits frame g at size s, picking region r (a free buffer) and
\* reserving it.  Enabled when g is the next generation and region r is free:
\* fresh allocation (first frame or resize, region must be 0) or a region
\* whose previous frame has been acked by the embedder and delivered.
SurfaceFrameSubmitted(w, g, s, r) ==
  /\ w \in Webview
  /\ g \in Generation
  /\ s \in Size
  /\ r \in Region
  /\ g = generation[w] + 1
  /\ IF buffer_size[w] = NONE \/ s # buffer_size[w]
     THEN /\ r = 0
          /\ reserved' = [reserved EXCEPT ![w] =
              [r2 \in Region |-> IF r2 = 0 THEN g ELSE 0]]
          /\ pending' = [pending EXCEPT ![w] = [r2 \in Region |-> 0]]
     ELSE /\ reserved[w][r] = 0
          /\ pending[w][r] = 0
          /\ reserved' = [reserved EXCEPT ![w][r] = g]
          /\ pending' = pending
  /\ generation' = [generation EXCEPT ![w] = g]
  /\ buffer_size' = [buffer_size EXCEPT ![w] = s]
  /\ submitted_size' = [submitted_size EXCEPT ![w][g] = s]
  /\ UNCHANGED <<sent, received, texture_size>>

\* The GPU readback for g completed; the pixels are delivered into the region
\* reserved for g, the frame is sent, and the region becomes pending.
\* Enabled when g was submitted, was not sent before, and its region is still
\* reserved for it; the sent size and region must match the submitted ones.
SurfaceFrameSent(w, g, s, r) ==
  /\ w \in Webview
  /\ g \in Generation
  /\ s \in Size
  /\ r \in Region
  /\ submitted_size[w][g] # NONE
  /\ g \notin sent[w]
  /\ reserved[w][r] = g
  /\ submitted_size[w][g] = s
  /\ reserved' = [reserved EXCEPT ![w][r] = 0]
  /\ pending' = [pending EXCEPT ![w][r] = g]
  /\ sent' = [sent EXCEPT ![w] = sent[w] \cup {g}]
  /\ UNCHANGED <<generation, buffer_size, submitted_size,
                 received, texture_size>>

\* Embedder acks frame g; the region holding it becomes free again.
\* Enabled when generation g is pending in some region.
TextureConsumed(w, g) ==
  /\ w \in Webview
  /\ g \in Generation
  /\ \E r \in Region: pending[w][r] = g
  /\ pending' = [pending EXCEPT ![w] =
      [r2 \in Region |-> IF pending[w][r2] = g THEN 0 ELSE pending[w][r2]]]
  /\ UNCHANGED <<generation, buffer_size, reserved, submitted_size,
                 sent, received, texture_size>>

\* Embedder consumes frame g at size s.  Enabled when g was sent and the
\* size matches the size submitted with that generation.
SurfaceFrameReceived(w, g, s) ==
  /\ w \in Webview
  /\ g \in Generation
  /\ s \in Size
  /\ g \in sent[w]
  /\ submitted_size[w][g] = s
  /\ g \notin received[w]
  /\ received' = [received EXCEPT ![w] = received[w] \cup {g}]
  /\ texture_size' = [texture_size EXCEPT ![w] = s]
  /\ UNCHANGED <<generation, buffer_size, reserved, pending, submitted_size,
                 sent>>

\* ---- Next-state relation ----

Stutter == UNCHANGED vars

Next ==
  \/ \E w \in Webview, g \in Generation, s \in Size, r \in Region:
       SurfaceFrameSubmitted(w, g, s, r)
  \/ \E w \in Webview, g \in Generation, s \in Size, r \in Region:
       SurfaceFrameSent(w, g, s, r)
  \/ \E w \in Webview, g \in Generation:
       TextureConsumed(w, g)
  \/ \E w \in Webview, g \in Generation, s \in Size:
       SurfaceFrameReceived(w, g, s)
  \/ Stutter

Spec == Init /\ [][Next]_vars

\* ---- Invariants ----

\* Every sent frame was submitted.
SentFramesWereSubmitted ==
  \A w \in Webview:
    \A g \in sent[w]:
      submitted_size[w][g] # NONE

\* Every consumed generation was actually sent.
ConsumedFramesWereSent ==
  \A w \in Webview:
    \A g \in received[w]:
      g \in sent[w]

\* A pending region holds a generation that was actually submitted.
PendingBuffersWereSubmitted ==
  \A w \in Webview:
    \A r \in Region:
      pending[w][r] # 0 => submitted_size[w][pending[w][r]] # NONE

\* A reserved region holds a generation that was actually submitted.
ReservedBuffersWereSubmitted ==
  \A w \in Webview:
    \A r \in Region:
      reserved[w][r] # 0 => submitted_size[w][reserved[w][r]] # NONE

\* A region is never reserved and pending at the same time.
ReservedAndPendingDisjoint ==
  \A w \in Webview:
    \A r \in Region:
      \/ reserved[w][r] = 0
      \/ pending[w][r] = 0

THEOREM
    Spec => (TypeOK /\ SentFramesWereSubmitted /\ ConsumedFramesWereSent
             /\ PendingBuffersWereSubmitted /\ ReservedBuffersWereSubmitted
             /\ ReservedAndPendingDisjoint)
=============================================================================
