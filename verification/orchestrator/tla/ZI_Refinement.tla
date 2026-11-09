---------------------------- MODULE ZI_Refinement ----------------------------
(***************************************************************************)
(* Safety refinement ZI_ZeronImpl => Delegation (DESIGN 9.2, AC-P2 slice). *)
(* Checked as PROPERTY AbsSafe == Abs!D_Init /\ [][Abs!D_Next]_Abs!dVars   *)
(* (no fairness: DESIGN 12, AC-P2 row).                                    *)
(*                                                                         *)
(* Mapping, variable by variable (Delegation name <- ZeronImpl expression):*)
(*   queue[t]      <- <<task>> \o hq[t] while t's first Run row has no     *)
(*                    receipt at host(t) (CreateChild pending), else hq[t] *)
(*   held, liveSet, rord, intReq, rtask, waitOn <- host(t) run row (id)    *)
(*   created, parent, depth <- host(parent) task row (id)                  *)
(*   access[d]     <- min of the configs on d's chain (DESIGN 9.2): the    *)
(*                    Clamp lag stays invisible; ZI checks it itself       *)
(*   tSt[c]        <- "settled" iff host(parent) has receipt notice:c:r    *)
(*   tWake, tDel, tCoh <- host(parent) task row (id); pushes, kidIn ghosts *)
(*   cDisp, cSt, cDel, cGen <- host(t) cohort rows (id)                    *)
(*   qPend, qUsed, answers, qEp <- host(q) rows (id); qAt, qVis, qWake <-  *)
(*                    the hop rows (id); qHe = qEp is ZI_HeldEpoch         *)
(*   outbox        <- offers in any host outbox                            *)
(*                  \cup notice(c, r) in host(c) outbox or c's orch_out,   *)
(*                      minus ids host(parent) has a receipt for           *)
(*                  \cup stop(c, s) in host(parent) outbox or c's ledger,  *)
(*                      minus ids host(c) has a receipt for                *)
(*                  \cup qup / qclosed rows in a chat's orch_out the next  *)
(*                      hop has no receipt for, ans / rel rows in a ledger *)
(*                      its host has no receipt for (QSplit effects)       *)
(*   rcpt          <- {<<c, s>> : receipt stop:c:s at host(c)}             *)
(*   zWait, replayable, replayBad, receiptBad <- constants (no late ack;   *)
(*                    receipts make replays inert)                         *)
(*   doomed        <- ghost (id)                                           *)
(*   personLeft, failLeft, stopLeft <- ZI counters (id)                    *)
(*   narrowLeft    <- MaxNarrow minus narrowings that lowered an abstract  *)
(*                    access (others are stutters)                         *)
(*   crashLeft     <- 2 * MaxCrash - crashes summed over both devices; a   *)
(*                    DeviceCrash(d) is CrashEngine with S = OnDev(d)      *)
(*                    (PartialCrash; closes gap G1 at any depth)           *)
(*   abandonLeft, interruptLeft <- 0 (actions not in this slice)           *)
(* Stutters: PersonWrite, MarkLedger, DrainTask, *Write/*Mark effect steps,*)
(* DispatchRun/Mark, WaitReply, ReadReply, GoOffline/Online, Dup* and the  *)
(* dropped second copies, question-row relays, Clamp delivery.             *)
(***************************************************************************)
EXTENDS ZI_ZeronImpl

TaskPending(t) == t \in Children /\ created[t] /\ TaskId(t) \notin rc[t]
AbsQueue == [t \in Threads |-> IF TaskPending(t) THEN <<Msg("task", 0, 0)>> \o hq[t] ELSE hq[t]]
AbsTSt   == [c \in Children |-> IF ~created[c] THEN "none"
                                ELSE IF Settled(c) THEN "settled" ELSE "running"]
AllObx   == UNION {obx[d] : d \in Devs}
(* Questions: an upward row is in flight until the next hop (the parent of  *)
(* the chat whose orch_out holds it) has its receipt; a downward row until  *)
(* the chat whose ledger holds it has. Relay hops copy the row unchanged,   *)
(* so relaying is a stutter.                                                *)
QE(row) == [k |-> row.k, t |-> row.q, r |-> row.e, g |-> row.d]
QUpRows   == UNION {{x \in qOut[k] : QId(x) \notin rc[parent[k]]} : k \in {k2 \in Children : created[k2]}}
QDownRows == UNION {{x \in qCmd[k] : QId(x) \notin rc[k]} : k \in Children}
AbsQOutbox == {QE(x) : x \in QUpRows \cup QDownRows}
AbsOutbox ==
  AbsQOutbox
  \cup {[k |-> "offer", t |-> e.t, r |-> e.r, g |-> e.g] : e \in {x \in AllObx : x.k = "offer"}}
  \cup {[k |-> "notice", t |-> c, r |-> r, g |-> 0] :
          <<c, r>> \in {cr \in Children \X Ords :
                          /\ created[cr[1]]
                          /\ (Eff("publish", cr[1], cr[2], 0) \in obx[Host(cr[1])] \/ cr[2] \in orchOut[cr[1]])
                          /\ NoticeId(cr[1], cr[2]) \notin rc[parent[cr[1]]]}}
  \cup {[k |-> "stop", t |-> c, r |-> s, g |-> 0] :
          <<c, s>> \in {cs \in Children \X StopIds :
                          /\ created[cs[1]]
                          /\ (Eff("stop", cs[1], cs[2], 0) \in obx[Host(parent[cs[1]])] \/ StopId(cs[1], cs[2]) \in cmds[cs[1]])
                          /\ StopId(cs[1], cs[2]) \notin rc[cs[1]]}}
AbsRcpt == {cs \in Threads \X StopIds : StopId(cs[1], cs[2]) \in rc[cs[1]]}

Abs == INSTANCE Delegation WITH
  queue <- AbsQueue,
  Mut <- "none", MaxAbandon <- 0, MaxInterrupt <- 0,
  DupDelivery <- FALSE, Crash <- TRUE, LateAck <- FALSE, Questions <- (MaxQ > 0),
  FixWakeSkipsFailHold <- TRUE, StopCheckAnc <- FALSE, PartialCrash <- TRUE, QSplit <- TRUE, MaxFollowUp <- 0,
  MaxCrash <- 2 * MaxCrash,
  zWait <- [t \in Threads |-> NoThread],
  tSt <- AbsTSt,
  access <- [d \in Threads |-> AbsAcc(d)],
  outbox <- AbsOutbox, replayable <- {}, rcpt <- AbsRcpt,
  replayBad <- FALSE, receiptBad <- FALSE,
  crashLeft <- 2 * MaxCrash - crashes[RootDev] - crashes[ChildDev],
  abandonLeft <- 0, narrowLeft <- MaxNarrow - narrowsAbs, interruptLeft <- 0

AbsSafe == Abs!D_Init /\ [][Abs!D_Next]_(Abs!dVars)

\* Abstract invariants read through the mapping (implied by AbsSafe; listed for quick failures).
AbsS3 == Abs!S3_ExactlyOnce
AbsS4 == Abs!S4_NoWakeIntoLiveRun
AbsS11 == Abs!S11_NoOrphanRunning
AbsStranded == Abs!D_NoStrandedResult

\* Former gap G1 guard (abstract crash cleared every wait). Closed by PartialCrash; now a
\* must-violate witness in the cross-device configs: a thread off RootDev really waits.
G1_OnlyRootDevWaits == \A t \in Threads : waitOn[t] # NoThread => Host(t) = RootDev
\* Witness: a ChildDev thread waits while the other device crashes (the partial crash keeps it).
W_WaitSurvivesOtherCrash == ~\E t \in Threads :
  waitOn[t] # NoThread /\ Host(t) = ChildDev /\ crashes[RootDev] = 1

\* Coverage witnesses (must be violated): the slice reaches a wake delivery, a wait return,
\* a stop cascade to the child, and a dispatch after a crash.
W_WakeDelivered == ~\E c \in Children : tDel[c] = "delivered"
W_WaitAcked     == ~\E c \in Children : tDel[c] = "acknowledged" /\ tWake[c] = "settled_only"
W_StopReachedChild == ~\E c \in Children : StopId(c, 1) \in rc[c]
W_CrashThenDeliver == ~(crashes[RootDev] = 1 /\ \E c \in Children : tDel[c] = "delivered")
\* Item 2: a second copy of a ledger row was delivered and dropped; a second orch_out copy of a
\* notice reached a host that already holds its receipt.
W_DupRowDropped  == ~\E t \in Threads : dmark[t] # {}
W_DupNoticeAtHost == ~\E c \in Children : created[c] /\ \E r \in dupO[c] : NoticeId(c, r) \in rc[parent[c]]
\* Item 3: an async result acknowledged through an observed task_status read.
W_ReadAcked == ~\E c \in Children : tDel[c] = "acknowledged" /\ tWake[c] = "always" /\ ackSeen[c]
\* Item 3: a reply was written, then lost (crash or run end), and the result still arrived by wake.
W_ReplyLostThenWake == ~\E c \in Children : tDel[c] = "delivered" /\ crashes[RootDev] = 1
\* Item 4 witnesses: an agent answer applied; the epoch moved (Q9 release); a hold of a closed question.
W_AgentAnswered == ~\E q \in Children : ~qPend[q] /\ answers[q] = 1 /\ qEp[q] = 1 /\ \E k \in Threads : QRow("ans", q, 1, 0) \in qCmd[k]
W_EpochMoved    == ~\E q \in Children : qEp[q] = 2
\* Item 5 witnesses: a clamp reached a child and lowered a live run.
W_ClampApplied == ~\E c \in Children : aDone[c] # {} /\ Live(c) /\ runAcc[c] = 0 /\ access[c] = 1
\* State constraint for the tiny depth-2 check: a RootDev child is never the root's own child, so
\* every RootDev child is a grandchild under a ChildDev child (the cross-device chain of gap G1).
CrossChainOnly == \A c \in RootDevKids : created[c] => parent[c] # Root
\* item 1 tiny bound: the cross-device chain and one crash in total (either device).
D2Tiny == CrossChainOnly /\ crashes[RootDev] + crashes[ChildDev] <= 1
AbsS6  == Abs!S6_AccessNarrow
AbsS12 == Abs!S12a_OneAgentAnswerer /\ Abs!S12a_FirstAnswerWins
=============================================================================
