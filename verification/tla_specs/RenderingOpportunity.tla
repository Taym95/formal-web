---------------------------- MODULE RenderingOpportunity ----------------------------
EXTENDS Naturals

CONSTANTS WebViews, MaxRenderingOps
ASSUME WebViews # {}

VARIABLES webviews
  \* webviews[w] == [opp |-> noted opportunities, pending |-> pending renderings]

vars == <<webviews>>

TypeOK ==
  webviews \in [WebViews -> [opp: 0..MaxRenderingOps, pending: 0..1]]

Init ==
  webviews = [w \in WebViews |-> [opp |-> 0, pending |-> 0]]

NoteRenderingOpportunity(w) ==
  IF webviews[w].pending = 0
  THEN webviews' = [webviews EXCEPT ![w].pending = 1]
  ELSE /\ webviews[w].opp < MaxRenderingOps
       /\ webviews' = [webviews EXCEPT ![w].opp = @ + 1]

FrameRendered(w) ==
  /\ webviews[w].pending > 0
  /\ webviews' = [webviews EXCEPT ![w] =
       IF webviews[w].opp > 0
       THEN [opp |-> 0, pending |-> 1]
       ELSE [opp |-> 0, pending |-> 0]]

Next ==
  \E w \in WebViews :
    \/ NoteRenderingOpportunity(w)
    \/ FrameRendered(w)

Fairness == \A w \in WebViews : WF_vars(FrameRendered(w))

Spec == Init /\ [][Next]_vars /\ Fairness

NoMissedOpportunity ==
  \A w \in WebViews :
    (webviews[w].opp > 0 /\ webviews[w].pending = 0)
      => (\E v \in WebViews : webviews[v].pending > 0)

PendingIsAtMostOne ==
  \A w \in WebViews : webviews[w].pending <= 1

EventuallyServiced ==
  \A w \in WebViews : webviews[w].opp > 0 ~> webviews[w].opp = 0

THEOREM
    Spec => (TypeOK /\ NoMissedOpportunity /\ PendingIsAtMostOne /\ EventuallyServiced)
===================================================================================
