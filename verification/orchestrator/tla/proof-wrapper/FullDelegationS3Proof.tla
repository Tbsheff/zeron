----------------------- MODULE FullDelegationS3Proof -----------------------
(* Generated source differs from Delegation only in AncN parameterization. *)
(* S3 holds for EVERY ProofAncestry operator, so it holds for the original *)
(* recursive ancestry operator as an instance. No ancestry axiom is used. *)
(* The generator checks the inverse transform byte for byte.              *)
EXTENDS DelegationS3Source, TLAPS
R == INSTANCE ResultS3Proof WITH Children <- Children
ASSUME NoMutant == Mut = "none"
USE NoMutant

THEOREM InitLink == D_Init => R!Init
 BY DEF D_Init, R!Init

THEOREM PlanLink ==
 ASSUME NEW p, NEW r, NEW S, NEW liveNow, NEW ob,
        (S \cap Children) \subseteq {c \in Children : tDel[c] = "pending"},
        Plan(p, r, S, liveNow, ob), UNCHANGED pushes
 PROVE [R!Next]_R!vars
<1> DEFINE E == (IF cDisp[p][r] = "open" THEN
                  {c \in S : M("S4_liveSkip") \/ ~(tWake[c] = "settled_only" /\ liveNow)} ELSE {}) \cap Children
<1> DEFINE D == (IF cDisp[p][r] = "open" THEN {} ELSE S) \cap Children
<1>1. R!Plan(E, D) BY SMT DEF R!Plan, Plan, E, D
<1>2. E \in SUBSET Children /\ D \in SUBSET Children BY DEF E, D
<1>. QED BY <1>1, <1>2 DEF R!Next

THEOREM AckLink ==
 ASSUME NEW c, R!Inv, tDel[c] \in {"pending", "claimed"}, AckBy(c, TRUE)
 PROVE [R!Next]_R!vars
<1>1. CASE c \in Children
 <2>1. R!Ack(c) BY <1>1 DEF AckBy, R!Ack, Min
 <2>. QED BY <1>1, <2>1 DEF R!Next
<1>2. CASE c \notin Children
 <2>1. UNCHANGED <<tDel, pushes>> BY <1>2, SMT DEF AckBy, R!Inv, R!TypeOK
 <2>. QED BY <2>1 DEF R!vars
<1>. QED BY <1>1, <1>2

THEOREM StopLink ==
 ASSUME NEW t, NEW s, NEW ob, ThreadStop(t, s, ob), UNCHANGED pushes
 PROVE [R!Next]_R!vars
<1> DEFINE K == {c \in Kids(t) : tDel[c] \notin Handled}
<1>1. R!Stop(K) BY DEF R!Stop, ThreadStop, K, Kids, R!Handled, Handled
<1>2. K \in SUBSET Children BY DEF K, Kids
<1>. QED BY <1>1, <1>2 DEF R!Next

THEOREM WakeLink ==
 ASSUME NEW t, StartFromQueueWake(t)
 PROVE [R!Next]_R!vars
<1> DEFINE C == {c \in Children : c \in cDel[t][Head(queue[t]).r] /\ tDel[c] = "claimed"}
<1> DEFINE P == {c \in TaskKids(t, Head(queue[t]).r) : tSt[c] = "settled" /\ tDel[c] = "pending"}
<1> DEFINE N == IF P # {} /\ cDisp[t][Head(queue[t]).r] = "open" THEN P ELSE {}
<1>1. R!Wake(C, N) BY SMT DEF R!Wake, StartFromQueueWake, Min, C, P, N, TaskKids, Kids
<1>2. C \in SUBSET Children /\ N \in SUBSET Children BY SMT DEF C, P, N, TaskKids, Kids
<1>. QED BY <1>1, <1>2 DEF R!Next

THEOREM FrameLink == UNCHANGED <<tDel, pushes>> => [R!Next]_R!vars
 BY DEF R!vars

THEOREM EndLink ==
 ASSUME NEW t, NEW out, EndRun(t, out), UNCHANGED pushes
 PROVE [R!Next]_R!vars
<1> DEFINE S == {c \in TaskKids(t, rord[t]) : tSt[c] = "settled" /\ tDel[c] = "pending"}
<1>1. S \subseteq {c \in Children : tDel[c] = "pending"}
 BY DEF S, TaskKids, Kids
<1>2. (\E ob : Plan(t, rord[t], S, FALSE, ob)) \/ UNCHANGED <<tDel, pushes>>
 BY DEF EndRun, S, coh
<1>. QED BY <1>1, <1>2, PlanLink, FrameLink

THEOREM FollowUpLink ==
 ASSUME NEW t, NEW c, NEW mode, FollowUp(t, c, mode)
 PROVE [R!Next]_R!vars
<1>1. c \in Children /\ R!FollowUp(c)
 BY DEF FollowUp, Kids, R!FollowUp, Handled, R!Handled
<1>. QED BY <1>1 DEF R!Next

THEOREM WaitLink ==
 ASSUME NEW t, R!Inv, WaitReturn(t)
 PROVE [R!Next]_R!vars
<1>1. (tDel[waitOn[t]] \in {"pending", "claimed"} /\ AckBy(waitOn[t], TRUE))
       \/ UNCHANGED <<tDel, pushes>>
 BY DEF WaitReturn
<1>. QED BY <1>1, AckLink, FrameLink

THEOREM NoticeLink ==
 ASSUME NEW e, NEW how, e \in NosEffects, DoNotice(e, how)
 PROVE [R!Next]_R!vars
<1>2. ((IF tDel[e.t] = "pending" THEN {e.t} ELSE {}) \cap Children) \subseteq
        {c \in Children : tDel[c] = "pending"}
 BY SMT
<1>3. UNCHANGED pushes BY DEF DoNotice, task, runD, lin, base
<1>4. (\E p, r, liveNow, ob :
        Plan(p, r, IF tDel[e.t] = "pending" THEN {e.t} ELSE {}, liveNow, ob))
       \/ UNCHANGED <<tDel, pushes>>
 BY NoMutant DEF DoNotice, M
<1>. QED BY <1>2, <1>3, <1>4, PlanLink, FrameLink

THEOREM NextLink == R!Inv /\ [D_Next]_dVars => [R!Next]_R!vars
<1> SUFFICES ASSUME R!Inv, [D_Next]_dVars PROVE [R!Next]_R!vars OBVIOUS
<1>1. CASE UNCHANGED dVars
 BY <1>1, FrameLink DEF dVars, task
<1>2. ASSUME NEW t \in Threads, Stop(t) PROVE [R!Next]_R!vars
 BY <1>2, StopLink DEF Stop
<1>3. ASSUME NEW t \in Threads, WaitReturn(t) PROVE [R!Next]_R!vars
 BY <1>3, WaitLink
<1>4. ASSUME NEW t \in Threads, StartFromQueueWake(t) PROVE [R!Next]_R!vars
 BY <1>4, WakeLink
<1>5. ASSUME NEW t \in Threads, RootComplete(t) \/ RootFail(t) \/ ConfirmInterrupt(t)
 PROVE [R!Next]_R!vars
 BY <1>5, EndLink DEF RootComplete, RootFail, ConfirmInterrupt
<1>6. ASSUME NEW p \in Threads, NEW c \in Children, ReadResult(p,c) PROVE [R!Next]_R!vars
 BY <1>6, AckLink, NoMutant DEF ReadResult, M
<1>7. ASSUME NEW t \in Threads, NEW c \in Children, NEW mode, FollowUp(t,c,mode)
 PROVE [R!Next]_R!vars
 BY <1>7, FollowUpLink
<1>8. ASSUME NEW e \in NosEffects, NEW how, DoNotice(e,how) PROVE [R!Next]_R!vars
 BY <1>8, NoticeLink
<1>9. ASSUME NEW e \in NosEffects, NEW how, DoStop(e,how) PROVE [R!Next]_R!vars
 BY <1>9, StopLink, FrameLink DEF DoStop
<1>10. ASSUME NEW t \in Threads,
 PersonSend(t) \/ NarrowAccess(t) \/ InterruptChat(t) \/ WaitReturnQuestion(t)
 \/ WaitTimeout(t) \/ WaitAbandon(t) \/ LateWaitAck(t) \/ StartFromQueue(t)
 PROVE [R!Next]_R!vars
 BY <1>10, FrameLink, NoMutant DEF PersonSend, NarrowAccess, InterruptChat,
 WaitReturnQuestion, WaitTimeout, WaitAbandon, LateWaitAck, StartFromQueue,
 base, lin, runD, task, coh, M
<1>11. ASSUME NEW t \in Threads, NEW q \in Children, StartQuestion(t,q)
 PROVE [R!Next]_R!vars
 BY <1>11, FrameLink DEF StartQuestion, task
<1>12. ASSUME NEW t \in Threads, NEW c \in Children, NEW mode, NEW a, Delegate(t,c,mode,a)
 PROVE [R!Next]_R!vars
 BY <1>12, FrameLink DEF Delegate
<1>13. ASSUME NEW q \in Children, NEW x \in Threads,
 RaiseQuestion(q) \/ PersonAnswer(q) \/ AgentAnswer(q,x) \/ RaiseQuestionS(q)
 \/ PersonAnswerS(q) \/ AgentAnswerS(q,x)
 PROVE [R!Next]_R!vars
 BY <1>13, FrameLink DEF RaiseQuestion, PersonAnswer, AgentAnswer, RaiseQuestionS,
 PersonAnswerS, AgentAnswerS, task
<1>14. ASSUME NEW e \in outbox, DoQUp(e) \/ DoAns(e) \/ DoRel(e) \/ DoQClosed(e)
 PROVE [R!Next]_R!vars
 BY <1>14, FrameLink DEF DoQUp, DoAns, DoRel, DoQClosed, task
<1>15. ASSUME NEW e \in NosEffects, NEW how, DoOffer(e,how)
 PROVE [R!Next]_R!vars
 BY <1>15, FrameLink DEF DoOffer, task
<1>16. CASE CrashEngine \/ D_Done
 BY <1>16, FrameLink DEF CrashEngine, D_Done, dVars, task
<1>. QED BY <1>1, <1>2, <1>3, <1>4, <1>5, <1>6, <1>7, <1>8, <1>9,
 <1>10, <1>11, <1>12, <1>13, <1>14, <1>15, <1>16, SMT DEF D_Next, Children

THEOREM StepInv == R!Inv /\ [D_Next]_dVars => R!Inv'
 BY NextLink, R!StepInv

THEOREM FullS3 == DSpec => []S3_ExactlyOnce
<1>1. D_Init => R!Inv BY InitLink, R!InitInv
<1>2. R!Inv /\ [D_Next]_dVars => R!Inv' BY StepInv
<1>3. R!Inv => S3_ExactlyOnce BY DEF R!Inv, R!Safe, S3_ExactlyOnce
<1>. QED BY <1>1, <1>2, <1>3, PTL DEF DSpec
=======================================================================
