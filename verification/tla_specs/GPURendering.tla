---------------------------- MODULE GPURendering ----------------------------
EXTENDS Naturals, FiniteSets, TLC

(*
  GPU surface buffer workflow: a per-webview three-slot shared-memory ring in
  the graphics process, consumed by the embedder with explicit acks.

  Graphics side (producer), per webview:
    generation[w]    -- generation of the most recently sent frame (0 before any)
    buffer_size[w]   -- size the three regions were allocated at (NONE before the first frame)
    buffer_state[w][r] -- FREE, or the generation of the frame written into
                         region r that still awaits the embedder's ack
    sent_size[w][g]  -- size of the frame sent with generation g (NONE if g was never sent)

  Each composed frame at size s has generation g = generation[w] + 1:
    - first frame, or size change: the three regions are freshly allocated
      (all FREE), so the frame is written into region 0
    - otherwise (same size): the frame is written into a FREE region -- a
      buffer whose previous frame has been acked (TextureConsumed).  The
      graphics process never rewrites a PENDING buffer, so the embedder is
      guaranteed to have consumed the previous frame's pixels.

  Consumer side (embedder), per webview:
    received[w]     -- generations the embedder has consumed
    texture_size[w] -- texture size after the last consumed frame

  TextureConsumed(w, g) frees the region holding generation g (the embedder's
  ack).  A consumed frame must match a sent frame: its generation must have
  been sent and its size must equal the size sent with that generation.
*)

CONSTANTS
  Webview,
  Generation,
  Size,
  Region,
  FREE,
  NONE

ASSUME
  /\ Webview # {}
  /\ Generation # {}
  /\ Size # {}
  /\ Region = {0, 1, 2}
  /\ FREE \notin Generation
  /\ NONE \notin Size
  /\ NONE \notin Generation
  /\ \A g \in Generation: g >= 1

VARIABLES
  generation,
  buffer_size,
  buffer_state,
  sent_size,
  received,
  texture_size

vars == <<generation, buffer_size, buffer_state, sent_size, received, texture_size>>

\* ---- Type correctness ----

TypeOK ==
  /\ generation \in [Webview -> Generation \cup {0}]
  /\ buffer_size \in [Webview -> Size \cup {NONE}]
  /\ buffer_state \in [Webview -> [Region -> Generation \cup {FREE}]]
  /\ sent_size \in [Webview -> [Generation -> Size \cup {NONE}]]
  /\ received \in [Webview -> SUBSET Generation]
  /\ texture_size \in [Webview -> Size \cup {NONE}]

\* ---- Init ----

Init ==
  /\ generation = [w \in Webview |-> 0]
  /\ buffer_size = [w \in Webview |-> NONE]
  /\ buffer_state = [w \in Webview |-> [r \in Region |-> FREE]]
  /\ sent_size = [w \in Webview |-> [g \in Generation |-> NONE]]
  /\ received = [w \in Webview |-> {}]
  /\ texture_size = [w \in Webview |-> NONE]

\* ---- Actions ----

\* Graphics sends frame g at size s written into region r.
\* Enabled when g is the next generation and region r is free to be written:
\* fresh allocation (first frame or resize, region must be 0) or a region
\* whose previous frame has been acked by the embedder.  Region r becomes
\* PENDING(g) until the ack arrives.
SurfaceFrameSent(w, g, s, r) ==
  /\ w \in Webview
  /\ g \in Generation
  /\ s \in Size
  /\ r \in Region
  /\ g = generation[w] + 1
  /\ IF buffer_size[w] = NONE \/ s # buffer_size[w]
     THEN /\ r = 0
          /\ buffer_state' = [buffer_state EXCEPT ![w] =
              [r2 \in Region |-> IF r2 = 0 THEN g ELSE FREE]]
     ELSE /\ buffer_state[w][r] = FREE
          /\ buffer_state' = [buffer_state EXCEPT ![w][r] = g]
  /\ generation' = [generation EXCEPT ![w] = g]
  /\ buffer_size' = [buffer_size EXCEPT ![w] = s]
  /\ sent_size' = [sent_size EXCEPT ![w][g] = s]
  /\ UNCHANGED <<received, texture_size>>

\* Embedder acks frame g; the region holding it becomes free again.
\* Enabled when generation g is pending in some region.
TextureConsumed(w, g) ==
  /\ w \in Webview
  /\ g \in Generation
  /\ \E r \in Region: buffer_state[w][r] = g
  /\ buffer_state' = [buffer_state EXCEPT ![w] =
      [r2 \in Region |-> IF buffer_state[w][r2] = g THEN FREE ELSE buffer_state[w][r2]]]
  /\ UNCHANGED <<generation, buffer_size, sent_size, received, texture_size>>

\* Embedder consumes frame g at size s.  Enabled when g was sent and the
\* size matches the size sent with that generation.
SurfaceFrameReceived(w, g, s) ==
  /\ w \in Webview
  /\ g \in Generation
  /\ s \in Size
  /\ sent_size[w][g] # NONE
  /\ sent_size[w][g] = s
  /\ g \notin received[w]
  /\ received' = [received EXCEPT ![w] = received[w] \cup {g}]
  /\ texture_size' = [texture_size EXCEPT ![w] = s]
  /\ UNCHANGED <<generation, buffer_size, buffer_state, sent_size>>

\* ---- Next-state relation ----

Stutter == UNCHANGED vars

Next ==
  \/ \E w \in Webview, g \in Generation, s \in Size, r \in Region:
       SurfaceFrameSent(w, g, s, r)
  \/ \E w \in Webview, g \in Generation:
       TextureConsumed(w, g)
  \/ \E w \in Webview, g \in Generation, s \in Size:
       SurfaceFrameReceived(w, g, s)
  \/ Stutter

Spec == Init /\ [][Next]_vars

\* ---- Invariants ----

\* Every consumed generation was actually sent.
ConsumedFramesWereSent ==
  \A w \in Webview:
    \A g \in received[w]:
      sent_size[w][g] # NONE

\* A PENDING region holds a generation that was actually sent.
PendingBuffersWereSent ==
  \A w \in Webview:
    \A r \in Region:
      buffer_state[w][r] # FREE => sent_size[w][buffer_state[w][r]] # NONE

THEOREM
    Spec => (TypeOK /\ ConsumedFramesWereSent /\ PendingBuffersWereSent)
=============================================================================
