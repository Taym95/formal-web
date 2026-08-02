---------------------------- MODULE GPURendering ----------------------------
EXTENDS Naturals, FiniteSets, TLC

(*
  GPU surface buffer workflow: a per-webview two-buffer alternating ring in
  the graphics process. The embedder's FrameNeeded pacing allows only one
  render per cycle, so the buffer chosen for a render — the one not used by
  the last render, hence holding the frame from two cycles ago — is always
  free. No ack is needed: the alternation guarantees the chosen buffer is
  no longer referenced by the embedder.

  Graphics side (producer), per webview:
    generation[w]    -- generation of the most recently SUBMITTED frame (0 before any)
    buffer_size[w]   -- size the two regions were allocated at (NONE before the first frame)
    last_buffer[w]   -- buffer the last render used (the next render uses 1 - last)
    submitted_size[w][g]   -- size submitted with generation g (NONE if g never submitted)
    sent[w]          -- generations whose pixels were delivered (PixelFrameReady sent)

  Each composed frame at size s has generation g = generation[w] + 1:
    - SurfaceFrameSubmitted picks the buffer the last render did not use
      (region 0 after a fresh allocation) and renders into it.
    - SurfaceFrameSent delivers the completed render. Sent generations are a
      subset of submitted generations (completions may be dropped on error),
      each sent at most once, and the sent region/size must match the
      submitted ones.

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
  /\ Region = {0, 1}
  /\ NONE \notin Size
  /\ NONE \notin Generation
  /\ \A g \in Generation: g >= 1

VARIABLES
  generation,
  buffer_size,
  last_buffer,
  submitted_size,
  sent,
  received,
  texture_size

vars == <<generation, buffer_size, last_buffer, submitted_size,
          sent, received, texture_size>>

\* ---- Type correctness ----

TypeOK ==
  /\ generation \in [Webview -> Generation \cup {0}]
  /\ buffer_size \in [Webview -> Size \cup {NONE}]
  /\ last_buffer \in [Webview -> Region]
  /\ submitted_size \in [Webview -> [Generation -> Size \cup {NONE}]]
  /\ sent \in [Webview -> SUBSET Generation]
  /\ received \in [Webview -> SUBSET Generation]
  /\ texture_size \in [Webview -> Size \cup {NONE}]

\* ---- Init ----

Init ==
  /\ generation = [w \in Webview |-> 0]
  /\ buffer_size = [w \in Webview |-> NONE]
  /\ last_buffer = [w \in Webview |-> 0]
  /\ submitted_size = [w \in Webview |-> [g \in Generation |-> NONE]]
  /\ sent = [w \in Webview |-> {}]
  /\ received = [w \in Webview |-> {}]
  /\ texture_size = [w \in Webview |-> NONE]

\* ---- Actions ----

\* Graphics submits frame g at size s, rendering into the buffer the last
\* render did not use: region 0 after a fresh allocation (first frame or
\* resize), otherwise 1 - last_buffer[w]. Enabled when g is the next
\* generation.
SurfaceFrameSubmitted(w, g, s, r) ==
  /\ w \in Webview
  /\ g \in Generation
  /\ s \in Size
  /\ r \in Region
  /\ g = generation[w] + 1
  /\ IF buffer_size[w] = NONE \/ s # buffer_size[w]
     THEN /\ r = 0
          /\ last_buffer' = [last_buffer EXCEPT ![w] = 0]
     ELSE /\ r = 1 - last_buffer[w]
          /\ last_buffer' = [last_buffer EXCEPT ![w] = r]
  /\ generation' = [generation EXCEPT ![w] = g]
  /\ buffer_size' = [buffer_size EXCEPT ![w] = s]
  /\ submitted_size' = [submitted_size EXCEPT ![w][g] = s]
  /\ UNCHANGED <<sent, received, texture_size>>

\* The GPU render for g completed; the frame is sent from the region it was
\* submitted into. Enabled when g was submitted, was not sent before, and the
\* sent size must match the submitted one.
SurfaceFrameSent(w, g, s, r) ==
  /\ w \in Webview
  /\ g \in Generation
  /\ s \in Size
  /\ r \in Region
  /\ submitted_size[w][g] # NONE
  /\ g \notin sent[w]
  /\ submitted_size[w][g] = s
  /\ sent' = [sent EXCEPT ![w] = sent[w] \cup {g}]
  /\ UNCHANGED <<generation, buffer_size, last_buffer, submitted_size,
                 received, texture_size>>

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
  /\ UNCHANGED <<generation, buffer_size, last_buffer, submitted_size,
                 sent>>

\* ---- Next-state relation ----

Stutter == UNCHANGED vars

Next ==
  \/ \E w \in Webview, g \in Generation, s \in Size, r \in Region:
       SurfaceFrameSubmitted(w, g, s, r)
  \/ \E w \in Webview, g \in Generation, s \in Size, r \in Region:
       SurfaceFrameSent(w, g, s, r)
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

THEOREM
    Spec => (TypeOK /\ SentFramesWereSubmitted /\ ConsumedFramesWereSent)
=============================================================================
