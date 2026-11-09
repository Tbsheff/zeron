----------------------------- MODULE Delegation -----------------------------
(***************************************************************************)
(* Zeron Orchestrator v2: delegation, wake delivery, stop cascade and      *)
(* child questions. Starts from model 1 (../Orchestration.tla) with every  *)
(* model-1 fix built in (R1-R8) and each fix guarded by a mutant switch.   *)
(*                                                                         *)
(* Runs here are collapsed (live -> completed/failed/interrupted); attempts,*)
(* steering and usage-limit resume live in RunLifecycle. Both EXTEND       *)
(* RunBase, which owns queue, held, liveSet, rord and intReq.              *)
(*                                                                         *)
(* One task per child thread (task id = child id). A cohort is keyed by    *)
(* (parent, spawning run ordinal) as in T3 (Orchestrator.ts:12543).        *)
(***************************************************************************)
EXTENDS RunBase

CONSTANTS
  MaxDepth, MaxFanout, MaxGen,
  MaxPerson, MaxFail, MaxCrash, MaxAbandon, MaxStop, MaxNarrow, MaxInterrupt,
  DupDelivery,   \* settle notices duplicated / reordered (multi-device relay)
  Crash,         \* engine crash: in-memory waits die, kept effects replay
  LateAck,       \* a wait handler may outlive its run (zWait)
  Questions,     \* child questions
  FixWakeSkipsFailHold, \* R12: a failure hold does not hold result/question wakes
  StopCheckAnc,  \* R13 (rejected by D7; keep FALSE): Delegate also refuses on an ancestor's pending stop
  PartialCrash,  \* FALSE: a crash clears every thread's wait. TRUE: one host crashes, so only the
                 \* waits of some thread set S (any S: covers every host partition) die (ZeronImpl G1)
  QSplit         \* FALSE: a question is routed in one step. TRUE: DESIGN 3.3 Q1-Q10 as host-local
                 \* steps: publish (QuestionUp effect), decide at each hop's ingest, (qid, epoch)
                 \* answers sent down and applied on host(c), QuestionClosed up, Q9 release by epoch
  , MaxFollowUp  \* 0: one task per child. n > 0: FollowUp gives a settled, handled child a new
                 \* task; task id = (child, child run) (DESIGN 2, 7 send_messages), so settle
                 \* receipts are keyed on (child, child run). n bounds the child's run ordinal
                 \* at follow-up time (a model bound, not a design guard).

(* D2 (Tyler): the design caps. MaxDepth / MaxFanout are the model's caps.  *)
(* Depth counts from the root at 0, so MaxDepth = 2 allows a grandchild.   *)
(* Fan-out 8 cannot bind at 3-4 threads, so configs scale it down to make  *)
(* the guard bite; D2_DesignCaps checks the design values hold too.        *)
DesignMaxDepth  == 2
DesignMaxFanout == 8

Children == Threads \ {Root}
Levels   == 0..1
DelSt    == {"pending", "claimed", "acknowledged", "delivered", "disposed"}
Handled  == {"acknowledged", "delivered", "disposed"}
StopIds  == 1..Max(MaxStop, 1)
(* Settle receipts keyed on (child, child run), stored in rcpt next to the *)
(* stop receipts (offset so the two id ranges never meet). Written only    *)
(* when MaxFollowUp > 0, so the flag-off state space is unchanged.        *)
NoticeRcpt(c, r) == <<c, Max(MaxStop, 1) + r>>
FollowUps == MaxFollowUp > 0
Hows     == {"ack", "crash", "dup"}

OfferE(p, r, g) == [k |-> "offer",  t |-> p, r |-> r, g |-> g]
StopE(t, s)     == [k |-> "stop",   t |-> t, r |-> s, g |-> 0]
NoticeE(c, r)   == [k |-> "notice", t |-> c, r |-> r, g |-> 0]
(* QSplit effects. Same record shape; g is a hop DEPTH on q's ancestor chain. *)
QUpE(q, e, d)     == [k |-> "qup",     t |-> q, r |-> e, g |-> d]  \* QuestionUp travelling to the hop at depth d
AnsE(q, e)        == [k |-> "ans",     t |-> q, r |-> e, g |-> 0]  \* AnswerDown to host(q) (relay hops collapsed)
RelE(q, e, d)     == [k |-> "rel",     t |-> q, r |-> e, g |-> d]  \* Q9: the holder at depth d released epoch e
ClosedE(q, e, d)  == [k |-> "qclosed", t |-> q, r |-> e, g |-> d]  \* QuestionClosed travelling to the hop at depth d
Epochs == 0..MaxGen
Depths == 0..Cardinality(Threads)
(* NOS effects (offer, stop, notice): the constant bound of D_Next's        *)
(* DoNotice / DoOffer / DoStop quantifier (question effects range over the  *)
(* outbox instead, so the flag-off search does not pay for them).           *)
NosEffects == {OfferE(p, r, g) : p \in Threads, r \in Ords, g \in 1..MaxGen}
              \cup {StopE(t, s) : t \in Threads, s \in StopIds}
              \cup {NoticeE(c, r) : c \in Children, r \in Ords}
Effects == NosEffects
           \cup {QUpE(q, e, d) : q \in Children, e \in Epochs, d \in Depths}
           \cup {AnsE(q, e) : q \in Children, e \in Epochs}
           \cup {RelE(q, e, d) : q \in Children, e \in Epochs, d \in Depths}
           \cup {ClosedE(q, e, d) : q \in Children, e \in Epochs, d \in Depths}

VARIABLES
  created, parent, depth, access,           \* lineage
  rtask, waitOn, zWait,                     \* run kind, blocking wait, late wait handler
  tSt, tWake, tDel, tCoh, pushes, kidIn,    \* task of child c (kidIn: F1 ghost)
  qPend, qAt, qVis, qWake, qUsed, answers,  \* questions
  qEp,                                      \* QSplit: question epoch on host(q) (0 when QSplit is off)
  cDisp, cSt, cDel, cGen,                   \* delivery cohort per (thread, run ordinal)
  outbox, replayable, rcpt, replayBad, receiptBad,\* durable effects, replay tracking, stop receipts
  doomed,                                   \* ghost: threads a Stop must reach
  personLeft, failLeft, crashLeft, abandonLeft, stopLeft, narrowLeft, interruptLeft

lin   == <<created, parent, depth, access>>
runD  == <<rtask, waitOn, zWait>>
task  == <<tSt, tWake, tDel, tCoh, pushes>>
qv    == <<qPend, qAt, qVis, qWake, qUsed, answers, qEp>>
coh   == <<cDisp, cSt, cDel, cGen>>
bud   == <<personLeft, failLeft, crashLeft, abandonLeft, stopLeft, narrowLeft, interruptLeft>>
dVars == <<base, lin, runD, task, kidIn, qv, coh, outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>
\* Domain state compared by the S10 replay check (no outbox, ghosts, budgets).
dom   == <<base, lin, runD, task, qv, coh, rcpt>>

-----------------------------------------------------------------------------
RECURSIVE AncN(_, _)
AncN(x, n) == IF n = 0 \/ parent[x] = NoThread THEN {}
              ELSE {parent[x]} \cup AncN(parent[x], n - 1)
Anc(x)  == AncN(x, Cardinality(Threads))
Desc(t) == {d \in Threads : created[d] /\ t \in Anc(d)}
Kids(t) == {c \in Children : created[c] /\ parent[c] = t}
(* R10 (DESIGN T1): fan-out counts unsettled tasks, not every child ever. *)
UnsettledKids(t) == {c \in Kids(t) : tSt[c] # "settled"}
TaskKids(t, r) == {c \in Kids(t) : tCoh[c] = r}
Blocked(t) == t \in Children /\ qPend[t]
CohortLive(p, r) == Live(p) /\ rord[p] = r
NotWakeOf(r, m) == ~(m.k = "wake" /\ m.r = r)

(* Nearest thread at or above x that is not blocked on its own question    *)
(* (R8). Root never blocks. Mutant "L4_escalate" stops at x.               *)
Chain(x) == {x} \cup Anc(x)
Up(x) == IF M("L4_escalate") THEN x
         ELSE CHOOSE y \in Chain(x) : ~Blocked(y) /\
                \A z \in Chain(x) : ~Blocked(z) => depth[z] <= depth[y]

(* Queue gate. R12 fix: a failure hold does not hold wakes. *)
WakeMayPass(t) == ~Held(t) \/ (FixWakeSkipsFailHold /\ held[t] = "fail")
HeadIsWake(t)  == queue[t] # <<>> /\ Head(queue[t]).k = "wake"
QueueGate(t)   == ~Held(t) \/ (HeadIsWake(t) /\ WakeMayPass(t))
CanStartD(t)   == queue[t] # <<>> /\ rord[t] < MaxRuns /\ NoActiveRun(t) /\ QueueGate(t)

(* Plan delivery for settled, pending tasks S of cohort (p, r).            *)
(* Orchestrator.ts:12518. A settled_only task stays pending while its      *)
(* spawning run is live (the wait will show it): S4. Mutant "S4_liveSkip". *)
Plan(p, r, S, liveNow, ob) ==
  LET open  == cDisp[p][r] = "open"
      E     == IF open THEN {c \in S : M("S4_liveSkip") \/ ~(tWake[c] = "settled_only" /\ liveNow)} ELSE {}
      D     == IF open THEN {} ELSE S
      fresh == E # {} /\ cSt[p][r] = "none"
      g     == IF fresh THEN Min(cGen[p][r] + 1, MaxGen) ELSE cGen[p][r]
  IN /\ tDel' = [c \in Children |-> IF c \in E THEN "claimed"
                                    ELSE IF c \in D THEN "disposed" ELSE tDel[c]]
     /\ cDel' = [cDel EXCEPT ![p][r] = @ \cup E]
     /\ cSt'  = [cSt EXCEPT ![p][r] = IF fresh THEN "offered" ELSE @]
     /\ cGen' = [cGen EXCEPT ![p][r] = g]
     /\ outbox' = IF E # {} /\ (fresh \/ cSt[p][r] = "offered") THEN ob \cup {OfferE(p, r, g)} ELSE ob

(* Acknowledge (task_status read or wait return), :2605. Removes the task  *)
(* from its wake; an emptied offered/queued wake is cancelled.             *)
AckBy(c, seen) ==
  LET p == parent[c]  r == tCoh[c]
      rem   == cDel[p][r] \ {c}
      clear == c \in cDel[p][r] /\ rem = {} /\ cSt[p][r] \in {"offered", "queued"}
  IN /\ tDel'   = [tDel EXCEPT ![c] = "acknowledged"]
     /\ pushes' = [pushes EXCEPT ![c] = IF seen THEN Min(@ + 1, 2) ELSE @]
     /\ cDel'   = [cDel EXCEPT ![p][r] = rem]
     /\ cSt'    = [cSt EXCEPT ![p][r] = IF clear THEN "none" ELSE @]
     /\ queue'  = IF clear THEN [queue EXCEPT ![p] = SelectSeq(@, LAMBDA m : NotWakeOf(r, m))] ELSE queue

-----------------------------------------------------------------------------
D_Init ==
  /\ BaseInit
  /\ created  = [t \in Threads |-> t = Root]
  /\ parent   = [t \in Threads |-> NoThread]
  /\ depth    = [t \in Threads |-> 0]
  /\ access   = [t \in Threads |-> IF t = Root THEN 1 ELSE 0]
  /\ rtask    = [t \in Threads |-> FALSE]
  /\ waitOn   = [t \in Threads |-> NoThread]
  /\ zWait    = [t \in Threads |-> NoThread]
  /\ tSt      = [c \in Children |-> "none"]
  /\ tWake    = [c \in Children |-> "always"]
  /\ tDel     = [c \in Children |-> "pending"]
  /\ tCoh     = [c \in Children |-> 0]
  /\ pushes   = [c \in Children |-> 0]
  /\ kidIn    = [c \in Children |-> FALSE]
  /\ qPend    = [c \in Children |-> FALSE]
  /\ qAt      = [c \in Children |-> NoThread]
  /\ qVis     = [c \in Children |-> {}]
  /\ qWake    = {}
  /\ qUsed    = [c \in Children |-> FALSE]
  /\ answers  = [c \in Children |-> 0]
  /\ qEp      = [c \in Children |-> 0]
  /\ cDisp    = [t \in Threads |-> [r \in Ords |-> "open"]]
  /\ cSt      = [t \in Threads |-> [r \in Ords |-> "none"]]
  /\ cDel     = [t \in Threads |-> [r \in Ords |-> {}]]
  /\ cGen     = [t \in Threads |-> [r \in Ords |-> 0]]
  /\ outbox = {} /\ replayable = {} /\ rcpt = {} /\ replayBad = FALSE /\ receiptBad = FALSE
  /\ doomed   = {}
  /\ personLeft = MaxPerson /\ failLeft = MaxFail /\ crashLeft = MaxCrash
  /\ abandonLeft = MaxAbandon /\ stopLeft = MaxStop /\ narrowLeft = MaxNarrow
  /\ interruptLeft = MaxInterrupt

-----------------------------------------------------------------------------
(* Person (environment, no fairness) *)

PersonSend(t) ==
  /\ created[t] /\ personLeft > 0 /\ Len(queue[t]) < MaxQueue
  /\ queue' = [queue EXCEPT ![t] = Append(@, Msg("person", 0, 0))]
  /\ held'  = [held EXCEPT ![t] = "none"]           \* a client send releases the hold
  /\ doomed' = doomed \ {t}
  /\ personLeft' = personLeft - 1
  /\ UNCHANGED <<liveSet, rord, intReq, lin, runD, task, kidIn, qv, coh,
                 outbox, replayable, rcpt, replayBad, receiptBad,
                 failLeft, crashLeft, abandonLeft, stopLeft, narrowLeft, interruptLeft>>

(* thread.stop on t (Orchestrator.ts:12177): interrupt request, hold,      *)
(* dispose undelivered results of t's cohorts, drop queued wakes and       *)
(* question wakes aimed at t, and a durable stop effect per child (R5).    *)
(* Mutant "L2_cascade" omits the child stop effects.                       *)
ThreadStop(t, s, ob) ==
  LET Tk == {c \in Kids(t) : tDel[c] \notin Handled}
      old(r) == r <= rord[t]
  IN /\ MarkInterrupt(t)
     /\ tDel'  = [c \in Children |-> IF c \in Tk THEN "disposed" ELSE tDel[c]]
     /\ cDisp' = [cDisp EXCEPT ![t] = [r \in Ords |-> IF old(r) THEN "stopped" ELSE @[r]]]
     /\ cSt'   = [cSt EXCEPT ![t] = [r \in Ords |-> IF old(r) THEN "none" ELSE @[r]]]
     /\ cDel'  = [cDel EXCEPT ![t] = [r \in Ords |-> IF old(r) THEN {} ELSE @[r]]]
     /\ queue' = [queue EXCEPT ![t] = SelectSeq(@, LAMBDA m : m.k # "wake")]
     /\ qWake' = {q \in qWake : qAt[q] # t}
     /\ outbox' = IF M("L2_cascade") THEN ob ELSE ob \cup {StopE(c, s) : c \in Kids(t)}

Stop(t) ==
  /\ created[t] /\ stopLeft > 0
  /\ ThreadStop(t, MaxStop - stopLeft + 1, outbox)
  /\ doomed' = doomed \cup {t} \cup Desc(t)
  /\ stopLeft' = stopLeft - 1
  /\ UNCHANGED <<liveSet, rord, lin, runD, tSt, tWake, tCoh, pushes, kidIn,
                 qPend, qAt, qVis, qUsed, answers, qEp, cGen, replayable, rcpt, replayBad, receiptBad,
                 personLeft, failLeft, crashLeft, abandonLeft, narrowLeft, interruptLeft>>

(* R4: narrowing clamps every descendant. Mutant "S6_clamp". *)
NarrowAccess(t) ==
  /\ created[t] /\ narrowLeft > 0 /\ access[t] > 0
  /\ access' = [d \in Threads |->
                  IF d = t THEN access[t] - 1
                  ELSE IF ~M("S6_clamp") /\ d \in Desc(t) THEN Min(access[d], access[t] - 1)
                  ELSE access[d]]
  /\ narrowLeft' = narrowLeft - 1
  /\ UNCHANGED <<base, created, parent, depth, runD, task, kidIn, qv, coh,
                 outbox, replayable, rcpt, replayBad, receiptBad, doomed,
                 personLeft, failLeft, crashLeft, abandonLeft, stopLeft, interruptLeft>>

(* interrupt_chat (DESIGN RL6): an interrupt request with no hold and no   *)
(* disposal. The run's cohort stays open, so its results need the RL10     *)
(* sweep on the interrupted end.                                            *)
InterruptChat(t) ==
  /\ created[t] /\ Live(t) /\ ~intReq[t] /\ interruptLeft > 0
  /\ intReq' = [intReq EXCEPT ![t] = TRUE]
  /\ interruptLeft' = interruptLeft - 1
  /\ UNCHANGED <<queue, held, liveSet, rord, lin, runD, task, kidIn, qv, coh,
                 outbox, replayable, rcpt, replayBad, receiptBad, doomed,
                 personLeft, failLeft, crashLeft, abandonLeft, stopLeft, narrowLeft>>

(* Answer applied once: only to a pending request. Mutant "S12_answerOnce". *)
AnswerGuard(q) == M("S12_answerOnce") \/ qPend[q]
(* An answer cancels the queued question wake. Mutant "QW_answerCancel". *)
AnswerCancelsWake(q) == IF M("QW_answerCancel") THEN qWake ELSE qWake \ {q}

(* The person answers in the UI; agents that saw it keep it in context. *)
PersonAnswer(q) ==
  /\ Questions /\ ~QSplit /\ AnswerGuard(q) /\ answers[q] < 2 /\ qUsed[q]
  /\ qPend'   = [qPend EXCEPT ![q] = FALSE]
  /\ answers' = [answers EXCEPT ![q] = @ + 1]
  /\ qWake'   = AnswerCancelsWake(q)
  /\ UNCHANGED <<base, lin, runD, task, kidIn, qAt, qVis, qUsed, qEp, coh,
                 outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

-----------------------------------------------------------------------------
(* Agent (environment, no fairness) *)

(* Stop reached t. D7 (Tyler): Delegate checks only local state, so       *)
(* StopCheckAnc stays FALSE; a grandchild started while an ancestor's Stop *)
(* is in flight is stopped by the arriving cascade (S11a below). The R13   *)
(* branch (a non-local read of a pending stop on an ancestor) is kept only *)
(* to reproduce the rejected alternative.                                  *)
StopBlocksDelegate(t) ==
  \/ ~M("S11_stopGuard") /\ intReq[t]
  \/ StopCheckAnc /\ \E a \in {t} \cup Anc(t) : (\E s \in StopIds : StopE(a, s) \in outbox) \/ intReq[a]

(* delegate_task (Orchestrator.ts:7955). R3 caps in the engine: mutants    *)
(* "S5_depth", "S5_fanout". wait => settled_only, async => always.         *)
Delegate(t, c, mode, a) ==
  /\ Live(t) /\ ~StopBlocksDelegate(t) /\ waitOn[t] = NoThread /\ ~Blocked(t)
  /\ c \in Children /\ ~created[c]
  /\ M("S5_depth")  \/ depth[t] + 1 <= MaxDepth
  /\ M("S5_fanout") \/ Cardinality(UnsettledKids(t)) < MaxFanout
  /\ a <= access[t]
  /\ created' = [created EXCEPT ![c] = TRUE]
  /\ parent'  = [parent EXCEPT ![c] = t]
  /\ depth'   = [depth EXCEPT ![c] = depth[t] + 1]
  /\ access'  = [access EXCEPT ![c] = a]
  /\ queue'   = [queue EXCEPT ![c] = <<Msg("task", 0, 0)>>]
  /\ tSt'     = [tSt EXCEPT ![c] = "running"]
  /\ tWake'   = [tWake EXCEPT ![c] = IF mode = "wait" THEN "settled_only" ELSE "always"]
  /\ tCoh'    = [tCoh EXCEPT ![c] = rord[t]]
  /\ waitOn'  = [waitOn EXCEPT ![t] = IF mode = "wait" THEN c ELSE NoThread]
  /\ doomed'  = IF t \in doomed THEN doomed \cup {c} ELSE doomed
  /\ UNCHANGED <<held, liveSet, rord, intReq, rtask, zWait, tDel, pushes, kidIn,
                 qv, coh, outbox, replayable, rcpt, replayBad, receiptBad, bud>>

(* Follow-up task (send_messages to an own child, DESIGN 7; T3            *)
(* recordSubagentFollowup, Orchestrator.ts:8202). Same guards as Delegate, *)
(* host(parent)-local; the child must have a settled, handled task (one    *)
(* open task per child). The new task is (c, next child run): it reuses   *)
(* the child's task row, and its settle needs a NEW receipt (c, r).       *)
FollowUp(t, c, mode) ==
  /\ FollowUps /\ Live(t) /\ ~StopBlocksDelegate(t) /\ waitOn[t] = NoThread /\ ~Blocked(t)
  /\ c \in Kids(t) /\ tSt[c] = "settled" /\ tDel[c] \in Handled
  /\ rord[c] <= MaxFollowUp
  /\ M("S5_fanout") \/ Cardinality(UnsettledKids(t)) < MaxFanout
  /\ queue'  = [queue EXCEPT ![c] = Append(@, Msg("task", 0, 0))]
  /\ tSt'    = [tSt EXCEPT ![c] = "running"]
  /\ tWake'  = [tWake EXCEPT ![c] = IF mode = "wait" THEN "settled_only" ELSE "always"]
  /\ tDel'   = [tDel EXCEPT ![c] = "pending"]
  /\ tCoh'   = [tCoh EXCEPT ![c] = rord[t]]
  /\ pushes' = [pushes EXCEPT ![c] = 0]
  /\ kidIn'  = [kidIn EXCEPT ![c] = FALSE]
  /\ waitOn' = [waitOn EXCEPT ![t] = IF mode = "wait" THEN c ELSE NoThread]
  /\ UNCHANGED <<held, liveSet, rord, intReq, lin, rtask, zWait, qv, coh,
                 outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* task_status read by a live, unblocked parent run (R2). S3: only an       *)
(* unhandled result is acknowledged. Mutant "S3_readHandled".               *)
ReadResult(p, c) ==
  /\ Live(p) /\ ~Blocked(p) /\ c \in Kids(p) /\ tSt[c] = "settled"
  /\ M("S3_readHandled") \/ tDel[c] \in {"pending", "claimed"}
  /\ AckBy(c, TRUE)
  /\ UNCHANGED <<held, liveSet, rord, intReq, lin, runD, tSt, tWake, tCoh, kidIn, qv,
                 cDisp, cGen, outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* Child asks one blocking question. Target: nearest unblocked ancestor    *)
(* (R8). The wake is skipped only while the parent's wait on this task is  *)
(* active (it will show the question). The child now blocks, so questions  *)
(* aimed at it move up; their old visibility is cleared (S12, mutant       *)
(* "S12_reroute" keeps it).                                                 *)
RaiseQuestion(c) ==
  LET p == parent[c]
      tgt == Up(p)
      R == {q \in Children : qPend[q] /\ qAt[q] = c}
      skip == tWake[c] = "settled_only" /\ waitOn[p] = c /\ tgt = p
  IN
  /\ Questions /\ ~QSplit /\ c \in Children /\ Live(c) /\ rtask[c] /\ ~qUsed[c] /\ ~intReq[c]
  /\ waitOn[c] = NoThread
  /\ qPend' = [qPend EXCEPT ![c] = TRUE]
  /\ qUsed' = [qUsed EXCEPT ![c] = TRUE]
  /\ qAt'   = [q \in Children |-> IF q = c \/ q \in R THEN tgt ELSE qAt[q]]
  /\ qVis'  = [q \in Children |-> IF q = c THEN {}
                                  ELSE IF q \in R /\ ~M("S12_reroute") THEN {}
                                  ELSE qVis[q]]
  /\ qWake' = (IF skip THEN qWake ELSE qWake \cup {c}) \cup R
  /\ UNCHANGED <<base, lin, runD, task, kidIn, answers, qEp, coh,
                 outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* The one visible answerer answers. *)
AgentAnswer(q, x) ==
  /\ Questions /\ ~QSplit /\ AnswerGuard(q) /\ answers[q] < 2
  /\ x \in qVis[q] /\ Live(x) /\ ~Blocked(x)
  /\ qPend'   = [qPend EXCEPT ![q] = FALSE]
  /\ answers' = [answers EXCEPT ![q] = @ + 1]
  /\ qVis'    = [qVis EXCEPT ![q] = {}]
  /\ qWake'   = AnswerCancelsWake(q)
  /\ UNCHANGED <<base, lin, runD, task, kidIn, qAt, qUsed, qEp, coh,
                 outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

-----------------------------------------------------------------------------
(* QSplit questions (DESIGN 3.3). Each step reads and writes one host only.  *)
(* Epoch owner: host(q) (qEp). A hop that holds q holds the epoch carried by *)
(* the QuestionUp it ingested; host(q) moves the epoch only on a Release     *)
(* from that holder, so a holder's epoch always equals qEp[q].               *)

AncAt(q, d) == CHOOSE a \in Chain(q) : depth[a] = d

(* Q1 on host(c), plus Q9 trigger 1: c now blocks, so it releases every     *)
(* question it holds (they go up with a new epoch decided on host(q)).      *)
RaiseQuestionS(c) ==
  LET R1 == {q \in Children : qAt[q] = c} IN
  /\ Questions /\ QSplit /\ c \in Children /\ Live(c) /\ rtask[c] /\ ~qUsed[c] /\ ~intReq[c]
  /\ waitOn[c] = NoThread
  /\ qPend' = [qPend EXCEPT ![c] = TRUE]
  /\ qUsed' = [qUsed EXCEPT ![c] = TRUE]
  /\ qEp'   = [qEp EXCEPT ![c] = 1]
  /\ qAt'   = [q \in Children |-> IF q \in R1 THEN NoThread ELSE qAt[q]]
  /\ qVis'  = [q \in Children |-> IF q \in R1 THEN {} ELSE qVis[q]]
  /\ qWake' = qWake \ R1
  /\ outbox' = outbox \cup {QUpE(c, 1, depth[c] - 1)} \cup {RelE(q, qEp[q], depth[c]) : q \in R1}
  /\ UNCHANGED <<base, lin, runD, task, kidIn, answers, coh,
                 replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* Q2 at hop h = the ancestor of q at depth e.g: blocked => escalate one hop *)
(* up (same epoch); else hold it; the wake is skipped while h's wait on q   *)
(* is active (the wait shows it, R8a).                                      *)
DoQUp(e) ==
  LET q == e.t  h == AncAt(q, e.g) IN
  /\ QSplit /\ e \in outbox /\ e.k = "qup"
  /\ IF ~M("L4_escalate") /\ Blocked(h)
     THEN /\ outbox' = (outbox \ {e}) \cup {QUpE(q, e.r, e.g - 1)}
          /\ UNCHANGED <<qAt, qVis, qWake>>
     ELSE /\ outbox' = outbox \ {e}
          /\ qAt'   = [qAt EXCEPT ![q] = h]
          /\ qVis'  = [qVis EXCEPT ![q] = {}]
          /\ qWake' = IF tWake[q] = "settled_only" /\ waitOn[h] = q THEN qWake ELSE qWake \cup {q}
  /\ UNCHANGED <<base, lin, runD, task, kidIn, qPend, qUsed, answers, qEp, coh,
                 replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* Q5 on host(x): only the holder answers; it sends (qid, epoch) down. *)
AgentAnswerS(q, x) ==
  /\ Questions /\ QSplit /\ x \in qVis[q] /\ qAt[q] = x /\ Live(x) /\ ~Blocked(x)
  /\ qVis'  = [qVis EXCEPT ![q] = {}]
  /\ qAt'   = [qAt EXCEPT ![q] = NoThread]
  /\ qWake' = qWake \ {q}
  /\ outbox' = outbox \cup {AnsE(q, qEp[q])}
  /\ UNCHANGED <<base, lin, runD, task, kidIn, qPend, qUsed, answers, qEp, coh,
                 replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* Q6 / Q6' on host(q): applies only to the open qid at the current epoch. *)
(* Mutant "S12_answerOnce" applies any answer.                            *)
DoAns(e) ==
  LET q == e.t IN
  /\ QSplit /\ e \in outbox /\ e.k = "ans"
  /\ IF M("S12_answerOnce") \/ (qPend[q] /\ qEp[q] = e.r)
     THEN /\ qPend'   = [qPend EXCEPT ![q] = FALSE]
          /\ answers' = [answers EXCEPT ![q] = Min(@ + 1, 2)]
          /\ outbox'  = (outbox \ {e}) \cup {ClosedE(q, e.r, depth[q] - 1)}
     ELSE /\ outbox'  = outbox \ {e}
          /\ UNCHANGED <<qPend, answers>>
  /\ UNCHANGED <<base, lin, runD, task, kidIn, qAt, qVis, qWake, qUsed, qEp, coh,
                 replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* Q7 on host(q): the person answers in the child's UI. *)
PersonAnswerS(q) ==
  /\ Questions /\ QSplit /\ AnswerGuard(q) /\ answers[q] < 2 /\ qUsed[q]
  /\ qPend'   = [qPend EXCEPT ![q] = FALSE]
  /\ answers' = [answers EXCEPT ![q] = @ + 1]
  /\ outbox'  = outbox \cup {ClosedE(q, qEp[q], depth[q] - 1)}
  /\ UNCHANGED <<base, lin, runD, task, kidIn, qAt, qVis, qWake, qUsed, qEp, coh,
                 replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* Q9 on host(q): a holder at depth d released epoch e. If q is still open *)
(* at e, move to e+1 and send it up from the releaser's parent; a release  *)
(* by the root leaves it to the person. Stale releases are no-ops.         *)
DoRel(e) ==
  LET q == e.t IN
  /\ QSplit /\ e \in outbox /\ e.k = "rel"
  /\ IF qPend[q] /\ qEp[q] = e.r /\ e.r < MaxGen
     THEN /\ qEp'    = [qEp EXCEPT ![q] = e.r + 1]
          /\ outbox' = (outbox \ {e}) \cup (IF e.g > 0 THEN {QUpE(q, e.r + 1, e.g - 1)} ELSE {})
     ELSE /\ outbox' = outbox \ {e}
          /\ UNCHANGED qEp
  /\ UNCHANGED <<base, lin, runD, task, kidIn, qPend, qAt, qVis, qWake, qUsed, answers, coh,
                 replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* Q10 at hop h: the holder drops its wake and view; any other hop passes  *)
(* it one hop up; past the root it is dropped.                             *)
DoQClosed(e) ==
  LET q == e.t  h == AncAt(q, e.g) IN
  /\ QSplit /\ e \in outbox /\ e.k = "qclosed"
  /\ IF qAt[q] = h
     THEN /\ outbox' = outbox \ {e}
          /\ qAt'   = [qAt EXCEPT ![q] = NoThread]
          /\ qVis'  = [qVis EXCEPT ![q] = {}]
          /\ qWake' = qWake \ {q}
     ELSE /\ outbox' = (outbox \ {e}) \cup (IF e.g > 0 THEN {ClosedE(q, e.r, e.g - 1)} ELSE {})
          /\ UNCHANGED <<qAt, qVis, qWake>>
  /\ UNCHANGED <<base, lin, runD, task, kidIn, qPend, qUsed, answers, qEp, coh,
                 replayable, rcpt, replayBad, receiptBad, doomed, bud>>

-----------------------------------------------------------------------------
(* Blocking wait (delegate_task mode=wait) *)

(* Wait returns the result inside the same run (F4). *)
WaitReturn(t) ==
  LET c == waitOn[t] IN
  /\ c # NoThread /\ Live(t) /\ tSt[c] = "settled"
  /\ IF tDel[c] \in {"pending", "claimed"} THEN AckBy(c, TRUE)
     ELSE UNCHANGED <<tDel, pushes, cDel, cSt, queue>>
  /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
  /\ UNCHANGED <<held, liveSet, rord, intReq, lin, rtask, zWait, tSt, tWake, tCoh, kidIn, qv,
                 cDisp, cGen, outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* Wait returns the child's question; completionWake upgrades to always    *)
(* (OrchestratorMcpService.ts:2556).                                        *)
WaitReturnQuestion(t) ==
  LET c == waitOn[t] IN
  /\ c # NoThread /\ Live(t) /\ tSt[c] # "settled" /\ Questions /\ (QSplit \/ qPend[c]) /\ qAt[c] = t
  /\ qVis'   = [qVis EXCEPT ![c] = @ \cup {t}]
  /\ qWake'  = qWake \ {c}
  /\ tWake'  = [tWake EXCEPT ![c] = "always"]
  /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
  /\ UNCHANGED <<base, lin, rtask, zWait, tSt, tDel, tCoh, pushes, kidIn,
                 qPend, qAt, qUsed, answers, qEp, coh, outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

WaitTimeout(t) ==
  LET c == waitOn[t] IN
  /\ c # NoThread /\ Live(t) /\ tSt[c] # "settled"
  /\ ~(Questions /\ IF QSplit THEN qAt[c] = t ELSE qPend[c])
  /\ tWake'  = [tWake EXCEPT ![c] = "always"]
  /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
  /\ UNCHANGED <<base, lin, rtask, zWait, tSt, tDel, tCoh, pushes, kidIn, qv, coh,
                 outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* The MCP handler fiber is interrupted; the wake upgrade never runs. *)
WaitAbandon(t) ==
  /\ waitOn[t] # NoThread /\ abandonLeft > 0
  /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
  /\ abandonLeft' = abandonLeft - 1
  /\ UNCHANGED <<base, lin, rtask, zWait, task, kidIn, qv, coh,
                 outbox, replayable, rcpt, replayBad, receiptBad, doomed,
                 personLeft, failLeft, crashLeft, stopLeft, narrowLeft, interruptLeft>>

(* A wait handler that outlived its run times out and reads the task with  *)
(* acknowledgeTerminal (:2591). R2/S7: the ack counts only from the live   *)
(* run that read it, so here it is a no-op. Mutant "S7_ackLive".           *)
LateWaitAck(t) ==
  LET c == zWait[t] IN
  /\ c # NoThread
  /\ zWait' = [zWait EXCEPT ![t] = NoThread]
  /\ IF M("S7_ackLive") /\ tSt[c] = "settled" /\ tDel[c] \in {"pending", "claimed"}
     THEN AckBy(c, FALSE)
     ELSE UNCHANGED <<tDel, pushes, cDel, cSt, queue>>
  /\ UNCHANGED <<held, liveSet, rord, intReq, lin, rtask, waitOn, tSt, tWake, tCoh, kidIn, qv,
                 cDisp, cGen, outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

-----------------------------------------------------------------------------
(* Engine (weak fairness) *)

StaleWake(t) ==
  LET m == Head(queue[t]) IN m.k = "wake" /\ ~(cSt[t][m.r] = "queued" /\ cGen[t][m.r] = m.g)

(* Person or task message starts a run. *)
StartFromQueue(t) ==
  /\ CanStartD(t) /\ Head(queue[t]).k # "wake"
  /\ BeginRun(t) /\ PopHead(t)
  /\ rtask'  = [rtask EXCEPT ![t] = Head(queue[t]).k = "task"]
  /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
  /\ UNCHANGED <<held, lin, zWait, task, kidIn, qv, coh, outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* Wake run accepts its delivery at start (:13245): claimed -> delivered,  *)
(* pending settled siblings -> next generation.                             *)
StartFromQueueWake(t) ==
  LET m == Head(queue[t])  r == m.r
      C == {c \in cDel[t][r] : tDel[c] = "claimed"}
      P == {c \in TaskKids(t, r) : tSt[c] = "settled" /\ tDel[c] = "pending"}
      next == P # {} /\ cDisp[t][r] = "open"
      g == Min(cGen[t][r] + 1, MaxGen)
  IN
  /\ CanStartD(t) /\ HeadIsWake(t) /\ ~StaleWake(t)
  /\ BeginRun(t) /\ PopHead(t)
  /\ rtask'  = [rtask EXCEPT ![t] = FALSE]
  /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
  /\ tDel' = [c \in Children |-> IF c \in C THEN "delivered"
                                 ELSE IF c \in P /\ next THEN "claimed" ELSE tDel[c]]
  /\ pushes' = [c \in Children |-> IF c \in C THEN Min(pushes[c] + 1, 2) ELSE pushes[c]]
  /\ cDel' = [cDel EXCEPT ![t][r] = IF next THEN P ELSE {}]
  /\ cSt'  = [cSt EXCEPT ![t][r] = IF next THEN "offered" ELSE "none"]
  /\ cGen' = [cGen EXCEPT ![t][r] = IF next THEN g ELSE @]
  /\ outbox' = IF next THEN outbox \cup {OfferE(t, r, g)} ELSE outbox
  /\ UNCHANGED <<held, lin, zWait, tSt, tWake, tCoh, kidIn, qv, cDisp, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* DESIGN RL2 "stale wake: drop" was an action here; coverage (AC-M5)      *)
(* showed it is never enabled: every step that invalidates a queued        *)
(* delivery also removes its wake message (AckBy, ThreadStop), and a       *)
(* generation advances only at accept. It is now the invariant             *)
(* D_NoStaleWakeQueued; the code keeps the check as an assertion.          *)

(* Question wake (queue_after_active, fixed message id) to its target. *)
StartQuestion(t, q) ==
  /\ q \in qWake /\ (QSplit \/ qPend[q]) /\ qAt[q] = t
  /\ created[t] /\ NoActiveRun(t) /\ WakeMayPass(t) /\ rord[t] < MaxRuns
  /\ BeginRun(t)
  /\ qVis'  = [qVis EXCEPT ![q] = @ \cup {t}]
  /\ qWake' = qWake \ {q}
  /\ rtask'  = [rtask EXCEPT ![t] = FALSE]
  /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
  /\ UNCHANGED <<queue, held, lin, zWait, task, kidIn, qPend, qAt, qUsed, answers, qEp, coh,
                 outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

(* A stale question wake (DropQuestionWake) is likewise unreachable: an   *)
(* answer or the child's run end removes it. Invariant D_NoStaleQuestionWake.*)

(* Run end (root terminal only). A child's task run emits a settle notice. *)
(* R1 wait sweep on EVERY end, interrupted too (DESIGN RL10 delta): plan  *)
(* the cohort's settled, pending tasks (mutants "L1_waitSweep",           *)
(* "R9_sweepInterrupted" = model-1 rule). Questions: the child's  *)
(* own question is cancelled; settled_only tasks with an unseen question   *)
(* are re-woken on EVERY end, a plain interrupt too (R15; T3 only on      *)
(* completed/failed, DelegatedTaskQuestionReactor.ts:48; mutant           *)
(* "R15_catchUpInterrupted"). A failure holds the  *)
(* queue (Orchestrator.ts:1780), but never downgrades a Stop hold: R12    *)
(* lets wakes pass a "fail" hold only (mutant "R12_stopPrecedence").       *)
(* QSplit: every write is host-local. t's own question is closed by a     *)
(* QuestionClosed effect up (not by editing the hop's rows); questions t  *)
(* holds and has seen are released (Q9: epoch+1 decided on host(q)); the  *)
(* Q8 catch-up reads t's own hold (qAt = t) instead of qPend[c].          *)
EndRun(t, out) ==
  LET r   == rord[t]
      own == QSplit /\ t \in Children /\ qPend[t]
      R2  == IF QSplit THEN {q \in Children : qAt[q] = t /\ t \in qVis[q]} ELSE {}
      ob0 == (IF t \in Children /\ rtask[t] THEN outbox \cup {NoticeE(t, r)} ELSE outbox)
             \cup (IF own THEN {ClosedE(t, qEp[t], depth[t] - 1)} ELSE {})
             \cup {RelE(q, qEp[q], depth[t]) : q \in R2}
      Pnd(c) == IF QSplit THEN qAt[c] = t ELSE qPend[c]
      CU  == IF Questions /\ (M("R15_catchUpInterrupted") => out \in {"completed", "failed"})
             THEN {c \in TaskKids(t, r) : tWake[c] = "settled_only" /\ tDel[c] # "disposed"
                                          /\ tSt[c] = "running" /\ Pnd(c) /\ qVis[c] = {}}
             ELSE {}
      S   == {c \in TaskKids(t, r) : tSt[c] = "settled" /\ tDel[c] = "pending"}
  IN /\ FinishRun(t)
     /\ held'   = [held EXCEPT ![t] = IF out = "failed" /\ (M("R12_stopPrecedence") \/ @ # "stop") THEN "fail" ELSE @]
     /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
     /\ zWait'  = IF LateAck /\ waitOn[t] # NoThread
                  THEN [zWait EXCEPT ![t] = waitOn[t]] ELSE zWait
     /\ qPend'  = IF t \in Children THEN [qPend EXCEPT ![t] = FALSE] ELSE qPend
     /\ qVis'   = IF QSplit THEN [q \in Children |-> IF q \in R2 THEN {} ELSE qVis[q]]
                  ELSE IF t \in Children THEN [qVis EXCEPT ![t] = {}] ELSE qVis
     /\ qAt'    = [q \in Children |-> IF q \in R2 THEN NoThread ELSE qAt[q]]
     /\ qWake'  = IF QSplit THEN (qWake \ R2) \cup CU ELSE (qWake \ {t}) \cup CU
     /\ IF ~M("L1_waitSweep") /\ (M("R9_sweepInterrupted") => out # "interrupted") /\ S # {}
        THEN /\ Plan(t, r, S, FALSE, ob0)
             /\ UNCHANGED cDisp
        ELSE /\ outbox' = ob0
             /\ UNCHANGED <<tDel, coh>>

RootComplete(t) ==
  /\ Live(t) /\ waitOn[t] = NoThread /\ ~Blocked(t)
  /\ EndRun(t, "completed")
  /\ UNCHANGED <<queue, rord, lin, rtask, tSt, tWake, tCoh, pushes, kidIn, qUsed, answers, qEp,
                 replayable, rcpt, replayBad, receiptBad, doomed, bud>>

RootFail(t) ==
  /\ Live(t) /\ failLeft > 0
  /\ EndRun(t, "failed")
  /\ failLeft' = failLeft - 1
  /\ UNCHANGED <<queue, rord, lin, rtask, tSt, tWake, tCoh, pushes, kidIn, qUsed, answers, qEp,
                 replayable, rcpt, replayBad, receiptBad, doomed,
                 personLeft, crashLeft, abandonLeft, stopLeft, narrowLeft, interruptLeft>>

(* Provider confirms the interrupt (R7). *)
ConfirmInterrupt(t) ==
  /\ Live(t) /\ intReq[t]
  /\ EndRun(t, "interrupted")
  /\ UNCHANGED <<queue, rord, lin, rtask, tSt, tWake, tCoh, pushes, kidIn, qUsed, answers, qEp,
                 replayable, rcpt, replayBad, receiptBad, doomed, bud>>

-----------------------------------------------------------------------------
(* Effects. how = "ack": applied and removed. "crash": applied, then the   *)
(* engine crashed before the ack, so it stays and will replay. "dup": a    *)
(* relay duplicate of a notice, applied and kept (reorder).                *)

HowOK(e, how) ==
  \/ how = "ack"
  \/ how = "crash" /\ Crash /\ crashLeft > 0
  \/ how = "dup" /\ DupDelivery /\ e.k = "notice"
ObAfter(e, how) == IF how = "ack" THEN outbox \ {e} ELSE outbox
EffBook(e, how) ==
  /\ replayable' = IF how = "ack" THEN replayable \ {e} ELSE replayable \cup {e}
  /\ crashLeft'  = IF how = "crash" THEN crashLeft - 1 ELSE crashLeft
(* S10 check: a replay must leave the domain state unchanged. Must be the  *)
(* last conjunct (reads primed vars).                                      *)
(* S10a: an effect whose receipt already exists (stop receipt, settled   *)
(* notice) must change nothing, however it got applied again.            *)
HasReceipt(e) == \/ e.k = "stop" /\ <<e.t, e.r>> \in rcpt
                 \/ e.k = "notice" /\ ~FollowUps /\ tSt[e.t] = "settled"
                 \/ e.k = "notice" /\ FollowUps /\ NoticeRcpt(e.t, e.r) \in rcpt
ReplayCheck(e) ==
  /\ replayBad' = (replayBad \/ (e \in replayable /\ dom' # dom))
  /\ receiptBad' = (receiptBad \/ (HasReceipt(e) /\ dom' # dom))

(* Settle: idempotent on (child, child run) (R6). Mutants "S3_settleDedupe" *)
(* and "S3_planSkip" each remove one of the two layers. With follow-ups,   *)
(* the task row is reused, so "already settled" is not enough: a late      *)
(* duplicate of the previous task's notice must be dropped by its (c, r)   *)
(* receipt. Mutant "FU_runKey" keys the receipt on the child only.         *)
NoticeDeduped(e) ==
  \/ ~M("S3_settleDedupe") /\ tSt[e.t] = "settled"
  \/ FollowUps /\ ~M("FU_runKey") /\ NoticeRcpt(e.t, e.r) \in rcpt
DoNotice(e, how) ==
  LET c == e.t  p == parent[c]  r == tCoh[c] IN
  /\ e \in outbox /\ e.k = "notice" /\ HowOK(e, how)
  /\ IF NoticeDeduped(e)
     THEN /\ outbox' = ObAfter(e, how)
          /\ UNCHANGED <<tSt, tDel, coh, kidIn, rcpt>>
     ELSE /\ tSt' = [tSt EXCEPT ![c] = "settled"]
          /\ kidIn' = [kidIn EXCEPT ![c] = @ \/ \E k \in Kids(c) : pushes[k] >= 1]
          /\ Plan(p, r, IF M("S3_planSkip") \/ tDel[c] = "pending" THEN {c} ELSE {},
                  CohortLive(p, r), ObAfter(e, how))
          /\ rcpt' = IF FollowUps THEN rcpt \cup {NoticeRcpt(c, e.r)} ELSE rcpt
          /\ UNCHANGED cDisp
  /\ EffBook(e, how)
  /\ UNCHANGED <<base, lin, runD, tWake, tCoh, pushes, qv, doomed,
                 personLeft, failLeft, abandonLeft, stopLeft, narrowLeft, interruptLeft>>
  /\ ReplayCheck(e)

(* Wake dispatch only if the delivery is current (ProviderContinuation-    *)
(* Service.ts:118). Queued behind any active run (queue_after_active).     *)
DoOffer(e, how) ==
  LET p == e.t  r == e.r IN
  /\ e \in outbox /\ e.k = "offer" /\ HowOK(e, how)
  /\ IF cSt[p][r] = "offered" /\ (M("S4_genGate") \/ cGen[p][r] = e.g)
     THEN /\ queue' = [queue EXCEPT ![p] = Append(@, Msg("wake", r, e.g))]
          /\ cSt'   = [cSt EXCEPT ![p][r] = "queued"]
     ELSE UNCHANGED <<queue, cSt>>
  /\ outbox' = ObAfter(e, how)
  /\ EffBook(e, how)
  /\ UNCHANGED <<held, liveSet, rord, intReq, lin, runD, task, kidIn, qv, cDisp, cDel, cGen,
                 rcpt, doomed, personLeft, failLeft, abandonLeft, stopLeft, narrowLeft, interruptLeft>>
  /\ ReplayCheck(e)

(* Stop cascade step, keyed by a receipt (child, stop id) so a replay is a *)
(* no-op even after the person resumed the child. Mutant "S10_stopReceipt".*)
DoStop(e, how) ==
  /\ e \in outbox /\ e.k = "stop" /\ HowOK(e, how)
  /\ IF ~M("S10_stopReceipt") /\ <<e.t, e.r>> \in rcpt
     THEN /\ outbox' = ObAfter(e, how)
          /\ UNCHANGED <<queue, held, intReq, tDel, coh, qWake, rcpt>>
     ELSE /\ ThreadStop(e.t, e.r, ObAfter(e, how))
          /\ rcpt' = rcpt \cup {<<e.t, e.r>>}
          /\ UNCHANGED cGen
  /\ EffBook(e, how)
  /\ UNCHANGED <<liveSet, rord, lin, runD, tSt, tWake, tCoh, pushes, kidIn,
                 qPend, qAt, qVis, qUsed, answers, qEp, doomed,
                 personLeft, failLeft, abandonLeft, stopLeft, narrowLeft, interruptLeft>>
  /\ ReplayCheck(e)

(* Engine crash: in-memory waits die. The outbox is durable (R5). Mutant   *)
(* "S11_outbox" keeps effects in memory: only what startup recovery can    *)
(* re-derive survives (Orchestrator.ts:14255), not stop cascades.          *)
Recoverable ==
  UNION {{OfferE(p, r, cGen[p][r]) : r \in {r3 \in Ords : cSt[p][r3] = "offered"}} : p \in Threads}
  \cup {NoticeE(c, rord[c]) : c \in {x \in Children : created[x] /\ rtask[x] /\ ~Live(x)
                                     /\ rord[x] >= 1 /\ tSt[x] = "running"}}

(* PartialCrash: the crashed host's threads S lose their in-memory waits;   *)
(* other hosts keep theirs. Off: S = Threads (one engine hosts everything).  *)
CrashSets == IF PartialCrash THEN SUBSET Threads ELSE {Threads}
CrashEngine ==
  /\ Crash /\ crashLeft > 0
  /\ \E S \in CrashSets :
       /\ waitOn' = [t \in Threads |-> IF t \in S THEN NoThread ELSE waitOn[t]]
       /\ zWait'  = [t \in Threads |-> IF t \in S THEN NoThread ELSE zWait[t]]
  /\ outbox' = IF M("S11_outbox") THEN Recoverable ELSE outbox
  /\ replayable' = replayable \cap outbox'
  /\ crashLeft' = crashLeft - 1
  /\ UNCHANGED <<base, lin, rtask, task, kidIn, qv, coh, rcpt, replayBad, receiptBad, doomed,
                 personLeft, failLeft, abandonLeft, stopLeft, narrowLeft, interruptLeft>>

-----------------------------------------------------------------------------
(* Documented terminal states (AC-M7): nothing owed. Done stutters only    *)
(* there; checks run WITHOUT -deadlock.                                     *)
QueueIdle(t) == queue[t] = <<>> \/ ~QueueGate(t)
D_Quiescent ==
  /\ outbox = {}
  /\ \A t \in Threads : ~Live(t) /\ QueueIdle(t) /\ zWait[t] = NoThread
  /\ \A q \in qWake : (QSplit \/ qPend[q]) /\ ~WakeMayPass(qAt[q])
D_Done == D_Quiescent /\ UNCHANGED dVars

D_Next ==
  \/ \E t \in Threads :
       \/ PersonSend(t) \/ Stop(t) \/ NarrowAccess(t) \/ InterruptChat(t)
       \/ WaitReturn(t) \/ WaitReturnQuestion(t) \/ WaitTimeout(t) \/ WaitAbandon(t) \/ LateWaitAck(t)
       \/ StartFromQueue(t) \/ StartFromQueueWake(t)
       \/ RootComplete(t) \/ RootFail(t) \/ ConfirmInterrupt(t)
       \/ \E q \in Children : StartQuestion(t, q)
  \/ \E t \in Threads, c \in Children, m \in {"wait", "async"}, a \in Levels : Delegate(t, c, m, a)
  \/ \E p \in Threads, c \in Children : ReadResult(p, c)
  \/ \E t \in Threads, c \in Children, m \in {"wait", "async"} : FollowUp(t, c, m)
  \/ \E q \in Children : RaiseQuestion(q) \/ PersonAnswer(q)
                         \/ \E x \in Threads : AgentAnswer(q, x)
  \/ \E q \in Children : RaiseQuestionS(q) \/ PersonAnswerS(q)
                         \/ \E x \in Threads : AgentAnswerS(q, x)
  \/ \E e \in outbox : DoQUp(e) \/ DoAns(e) \/ DoRel(e) \/ DoQClosed(e)
  \/ \E e \in NosEffects, how \in Hows : DoNotice(e, how) \/ DoOffer(e, how) \/ DoStop(e, how)
  \/ CrashEngine
  \/ D_Done

D_Fairness ==
  /\ \A t \in Threads : WF_dVars(StartFromQueue(t) \/ StartFromQueueWake(t))
  /\ \A t \in Threads : WF_dVars(\E q \in Children : StartQuestion(t, q))
  /\ \A t \in Threads : WF_dVars(RootComplete(t))
  /\ \A t \in Threads : WF_dVars(ConfirmInterrupt(t))
  /\ \A t \in Threads : WF_dVars(WaitReturn(t) \/ WaitReturnQuestion(t) \/ WaitTimeout(t))
  /\ \A t \in Threads : WF_dVars(\E e \in outbox : e.t = t /\
                                   (DoNotice(e, "ack") \/ DoOffer(e, "ack") \/ DoStop(e, "ack")))
  \* QSplit question effects are engine steps too (vacuous when QSplit = FALSE).
  /\ \A t \in Threads : WF_dVars(\E e \in outbox : e.t = t /\
                                   (DoQUp(e) \/ DoAns(e) \/ DoRel(e) \/ DoQClosed(e)))

DSpec == D_Init /\ [][D_Next]_dVars /\ D_Fairness

D_Sym == Permutations(Children)

(* Trimming VIEW (Fast_trimming.md): kidIn is read only by its own update  *)
(* and W_F1_GrandchildToRoot, so safety configs without W_F1 may merge     *)
(* states that differ only in kidIn. Not for liveness configs.             *)
D_View == <<base, lin, runD, task, qv, coh, outbox, replayable, rcpt, replayBad, receiptBad, doomed, bud>>

-----------------------------------------------------------------------------
(* Safety *)

D_TypeOK ==
  /\ BaseTypeOK
  /\ created \in [Threads -> BOOLEAN]
  /\ parent \in [Threads -> Threads \cup {NoThread}]
  /\ depth \in [Threads -> 0..Cardinality(Threads)]
  /\ access \in [Threads -> Levels]
  /\ rtask \in [Threads -> BOOLEAN]
  /\ waitOn \in [Threads -> Children \cup {NoThread}]
  /\ zWait \in [Threads -> Children \cup {NoThread}]
  /\ tSt \in [Children -> {"none", "running", "settled"}]
  /\ tWake \in [Children -> {"always", "settled_only"}]
  /\ tDel \in [Children -> DelSt]
  /\ tCoh \in [Children -> 0..MaxRuns]
  /\ pushes \in [Children -> 0..2]
  /\ kidIn \in [Children -> BOOLEAN]
  /\ qPend \in [Children -> BOOLEAN] /\ qUsed \in [Children -> BOOLEAN]
  /\ qAt \in [Children -> Threads \cup {NoThread}]
  /\ qVis \in [Children -> SUBSET Threads]
  /\ qWake \subseteq Children /\ answers \in [Children -> 0..2]
  /\ qEp \in [Children -> Epochs]
  /\ cDisp \in [Threads -> [Ords -> {"open", "stopped"}]]
  /\ cSt \in [Threads -> [Ords -> {"none", "offered", "queued"}]]
  /\ cDel \in [Threads -> [Ords -> SUBSET Children]]
  /\ cGen \in [Threads -> [Ords -> 0..MaxGen]]
  /\ outbox \subseteq Effects /\ replayable \subseteq outbox
  /\ rcpt \subseteq (Threads \X (StopIds \cup {Max(MaxStop, 1) + r : r \in Ords})) /\ replayBad \in BOOLEAN /\ receiptBad \in BOOLEAN
  /\ doomed \subseteq Threads

D_BoundsNotHit ==
  /\ \A t \in Threads : rord[t] < MaxRuns
  /\ \A t \in Threads, r \in Ords : cGen[t][r] < MaxGen

(* S3: a result reaches the parent agent at most once, and a result that   *)
(* reached it is never marked disposed; conversely a handled-as-delivered  *)
(* result really reached an agent (with S7).                                *)
S3_ExactlyOnce ==
  \A c \in Children : /\ pushes[c] <= 1
                      /\ pushes[c] = 1 => tDel[c] \in {"delivered", "acknowledged"}

(* S4: a settled_only result is never claimed for a wake while its spawning *)
(* run is live (wakes are queue_after_active only, so a wake never enters  *)
(* a live run by construction).                                             *)
S4_NoWakeIntoLiveRun ==
  \A c \in Children : (created[c] /\ tWake[c] = "settled_only" /\ tDel[c] = "claimed")
                        => ~CohortLive(parent[c], tCoh[c])

S5_DepthFanout ==
  \A t \in Threads : created[t] => depth[t] <= MaxDepth /\ Cardinality(UnsettledKids(t)) <= MaxFanout

(* D2: the design caps (depth 2 from the root, 8 unsettled tasks) hold.   *)
D2_DesignCaps ==
  \A t \in Threads : created[t] => depth[t] <= DesignMaxDepth /\ Cardinality(UnsettledKids(t)) <= DesignMaxFanout

(* Follow-ups: a task is settled only after a run started by ITS task      *)
(* message ended; a late notice of the previous task must not settle the  *)
(* follow-up early (task id = (child, child run)).                        *)
FU_SettledAfterTaskRun ==
  \A c \in Children : tSt[c] = "settled" => ~(Live(c) /\ rtask[c])

(* S5 as originally worded: every child ever created counts. R10 drops it. *)
S5_OriginalTotalFanout ==
  \A t \in Threads : created[t] => Cardinality(Kids(t)) <= MaxFanout

S6_AccessNarrow == \A c \in Children : created[c] => access[c] <= access[parent[c]]

(* S7: delivered or acknowledged => an agent run really got it. *)
S7_AckFromLiveRun ==
  \A c \in Children : tDel[c] \in {"delivered", "acknowledged"} => pushes[c] = 1

(* S10: replaying a kept effect never changes the domain state. *)
S10_ReplayIdempotent == ~replayBad
S10a_ReceiptNoop == ~receiptBad

(* S11a: no delegation from a thread the stop reached (action property). *)
S11_NoDelegateAfterStop ==
  [][\A c \in Children : (~created[c] /\ created'[c]) => ~intReq[parent'[c]]]_dVars

(* S11 literal: no thread in a stopped subtree delegates (action property).*)
S11_NoDelegateInStoppedTree ==
  [][\A c \in Children : (~created[c] /\ created'[c]) => parent'[c] \notin doomed]_dVars

(* S11b: every live thread a Stop must reach has its interrupt request or  *)
(* is covered by a pending stop effect on itself or an ancestor.           *)
S11_NoOrphanRunning ==
  \A d \in doomed : Live(d) => intReq[d] \/ \E a \in {d} \cup Anc(d), s \in StopIds : StopE(a, s) \in outbox

(* S12: a pending question has at most one visible agent answerer, and an  *)
(* answer is applied once.                                                  *)
S12a_OneAgentAnswerer == \A q \in Children : qPend[q] => Cardinality(qVis[q]) <= 1
S12a_FirstAnswerWins  == \A q \in Children : answers[q] <= 1

(* Dead branches turned into invariants (see StartFromQueueWake). *)
D_NoStaleWakeQueued ==
  \A t \in Threads : \A i \in 1..Len(queue[t]) :
    queue[t][i].k = "wake" => (cSt[t][queue[t][i].r] = "queued" /\ cGen[t][queue[t][i].r] = queue[t][i].g)
D_NoStaleQuestionWake == \A q \in qWake : qPend[q]
(* QSplit: a hop may hold a question whose QuestionClosed has not reached it *)
(* (or passed it before the QuestionUp did); its question run is harmless   *)
(* because the stale answer is a Q6' no-op. What must hold instead:        *)
(* S12b: a holder's epoch is host(q)'s epoch while q is open (one agent     *)
(* answerer per epoch), and an AnswerDown in flight for an open q carries   *)
(* an epoch host(q) issued.                                                 *)
S12b_HolderHasEpoch ==
  \A q \in Children : (QSplit /\ qPend[q] /\ qAt[q] # NoThread) =>
      ~\E e \in outbox : e.k = "rel" /\ e.t = q /\ e.r = qEp[q]
(* S12c (R8 under QSplit): a question is never held by a hop that is      *)
(* itself blocked on its own question; DoQUp escalates past it, and a hop *)
(* that blocks later releases what it holds (Q9). Mutant "L4_escalate".   *)
S12c_HolderNotBlocked == \A q \in Children : (QSplit /\ qAt[q] # NoThread) => ~Blocked(qAt[q])

(* Stuck-state form of L1 (safety): a settled result no path will deliver. *)
D_NoStrandedResult ==
  \A c \in Children :
    ~( /\ created[c] /\ tSt[c] = "settled" /\ tDel[c] = "pending"
       /\ ~CohortLive(parent[c], tCoh[c]) /\ cSt[parent[c]][tCoh[c]] = "none"
       /\ cDisp[parent[c]][tCoh[c]] = "open" )

(* S2 (action property): a run leaves live only by a root terminal action;  *)
(* a child settling never ends the parent run.                              *)
D_EndStep(t) == RootComplete(t) \/ RootFail(t) \/ ConfirmInterrupt(t)
D_S2_OnlyRootEndsRun ==
  [][\A t \in Threads : (Live(t) /\ ~Live(t)') => D_EndStep(t)]_dVars

-----------------------------------------------------------------------------
(* Liveness *)

L1_ResultHandled ==
  \A c \in Children : (created[c] /\ tSt[c] = "settled") ~> (tDel[c] \in Handled)

L2_StopQuiesces ==
  \A t \in Threads : (t \in doomed) ~> (t \notin doomed \/ (~Live(t) /\ Held(t)))

(* S11a (D7, eventual form): after a Stop is applied on a host, that host   *)
(* starts no new delegation (S11_NoDelegateAfterStop), and every thread    *)
(* the Stop must reach, including a grandchild created while the stop was  *)
(* in flight, is eventually stopped (not live, held) or resumed by a       *)
(* person send.                                                            *)
S11a_StopReachesDescendants ==
  \A t \in Threads : (t \in doomed) ~> (t \notin doomed \/ (~Live(t) /\ held[t] = "stop"))

(* QSplit: a question the root's run saw and left unanswered is released  *)
(* past the root (Q9 at depth 0), so it goes to the person (desktop        *)
(* notification): no hop holds it and no QuestionUp/release is in flight.  *)
AtPerson(q) == QSplit /\ qPend[q] /\ qAt[q] = NoThread
               /\ ~\E e \in outbox : e.t = q /\ e.k \in {"qup", "rel"}

L4_QuestionReachesAnswerer ==
  \A q \in Children : qPend[q] ~> (~qPend[q] \/ qVis[q] # {} \/ AtPerson(q))

L6_ParentWoken ==
  \A t \in Threads, r \in Ords : (cSt[t][r] # "none") ~> (cSt[t][r] = "none")

-----------------------------------------------------------------------------
(* F witnesses: must be violated. *)

W_F1_GrandchildToRoot ==
  ~\E a, b \in Children : /\ created[b] /\ parent[b] = a /\ parent[a] = Root
                          /\ kidIn[a] /\ tDel[a] \in {"delivered", "acknowledged"} /\ pushes[a] = 1
W_F2_PushedNoPoll ==
  ~\E c \in Children : tDel[c] = "delivered"
W_F3_Coalesced ==
  ~\E p \in Threads, r \in Ords :
     /\ Cardinality({c \in cDel[p][r] : tDel[c] = "claimed"}) >= 2
     /\ cSt[p][r] = "queued" /\ HeadIsWake(p) /\ Head(queue[p]).r = r /\ ~StaleWake(p)
     /\ CanStartD(p)
W_F4_WaitInRun ==
  ~\E t \in Threads : /\ waitOn[t] # NoThread /\ Live(t)
                      /\ tSt[waitOn[t]] = "settled" /\ tDel[waitOn[t]] \in {"pending", "claimed"}
W_F7_QuestionAnswerable ==
  ~\E q \in Children, x \in Threads : qPend[q] /\ x \in qVis[q] /\ Live(x) /\ ~Blocked(x)
W_R8_NestedPastBlocked ==
  ~\E q \in Children, x \in Threads : /\ qPend[q] /\ x \in qVis[q] /\ x # parent[q]
                                      /\ Live(x) /\ ~Blocked(x) /\ Blocked(parent[q])
W_FU_FollowUpHandled ==   \* a follow-up task (second task run of a child) settled and reached the parent
  ~\E c \in Children : /\ tSt[c] = "settled" /\ tDel[c] \in {"delivered", "acknowledged"}
                        /\ \E r \in Ords : r >= 2 /\ NoticeRcpt(c, r) \in rcpt
W_Q9_Rerouted ==   \* QSplit: Q9 moved q to epoch 2 and a hop holds q again at the new epoch
  ~\E q \in Children : QSplit /\ qPend[q] /\ qEp[q] >= 2 /\ qAt[q] # NoThread
W_Q9_AnsweredAfterMove ==   \* QSplit: Q9 moved q to epoch 2 (root released it) and q was then answered
  ~\E q \in Children : QSplit /\ qEp[q] >= 2 /\ ~qPend[q] /\ answers[q] >= 1
W_F8_StopCascades ==
  ~\E g \in Children : created[g] /\ depth[g] = 2 /\ intReq[g] /\ Root \in doomed

=============================================================================
