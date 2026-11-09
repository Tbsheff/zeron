-------------------------------- MODULE ZI_CT --------------------------------
(***************************************************************************)
(* Zeron Orchestrator v2: implementation-level model of context transfers  *)
(* (DESIGN 9.1 ZeronImpl, slice: ContextTransfer, sections 3.4 and 5).     *)
(* Refined to ContextTransfer in ZI_CTRef.tla.                             *)
(*                                                                         *)
(* Reuses ContextTransfer's actions where Zeron does the same thing in one *)
(* durable step, and adds two Zeron mechanisms:                            *)
(*  1. Host-local replicas. Main is hosted on device A; with CrossHost,    *)
(*     every other thread is hosted on device B. Transfer rows are read    *)
(*     and resolved on host(target) (DESIGN 3.4). A thread's messages are  *)
(*     known at once on its own host and on the other host only after the  *)
(*     thread doc syncs (syncW = last synced message id). The resolver     *)
(*     waits until every message of the pinned range is known on          *)
(*     host(target) (sync gate).                                           *)
(*  2. Per-native-thread delivery row (R14). Settling a delivery writes,   *)
(*     in ONE step, the delivery row (delivered ids, stated-omitted ids,   *)
(*     last-seen point, pending flag) and a settle receipt naming the      *)
(*     transfer rows it consumed (dset). The transfer rows are marked      *)
(*     consumed in a later step (receipt before mark). Until the mark,     *)
(*     the thread's engine does nothing else with transfers.               *)
(* Native fork needs the source native thread on the same host (added     *)
(* guard). With CrossHost, refinement configs set NativeFork = {} (see     *)
(* ZI_CTRef gap note and ZI_CT_Gap_NativeForkCrossHost.cfg).               *)
(***************************************************************************)
EXTENDS ContextTransfer

CONSTANTS
  CrossHost,   \* TRUE: Main on A, other threads on B. FALSE: all on A.
  CMut         \* "none", "NoSyncGate", "VolatileSettle"

ASSUME CMut \in {"none", "NoSyncGate", "VolatileSettle"}

Devs == {"A", "B"}
Host(t) == IF t = Main \/ ~CrossHost THEN "A" ELSE "B"
NoSettle == [ids |-> {}, how |-> "none"]

VARIABLES
  syncW,   \* [Devs -> [Threads -> 0..MaxId]] last message id of thread t replicated on d (remote pairs only)
  dset     \* [Threads -> [ids, how]] settle receipt in t's delivery row, transfer rows not yet marked

zvars == <<vars, syncW, dset>>

ZInit ==
  /\ Init
  /\ syncW = [d \in Devs |-> [t \in Threads |-> 0]]
  /\ dset = [t \in Threads |-> NoSettle]

-----------------------------------------------------------------------------
Known(d, i) == Host(msgs[i].th) = d \/ i <= syncW[d][msgs[i].th]
\* What the resolver on host(t) can read of range R.
Readable(t, R) == IF CMut = "NoSyncGate" THEN {i \in R : Known(Host(t), i)} ELSE R
SyncGate(t, R) == CMut = "NoSyncGate" \/ \A i \in R : Known(Host(t), i)
Clear(t) == dset[t] = NoSettle

(* Thread t's doc syncs to device d: d learns t's messages so far.         *)
Sync(d, t) ==
  /\ Host(t) # d /\ created[t]
  /\ LET last == Max(Own(t, Len0)) IN
     /\ last > syncW[d][t]
     /\ syncW' = [syncW EXCEPT ![d][t] = last]
  /\ UNCHANGED <<vars, dset>>

-----------------------------------------------------------------------------
(* Resolution on host(target): ContextTransfer's resolvers, reading only   *)
(* what host(target) knows, behind the sync gate.                          *)
ZHandoffOrFail(t, x, strat, bp, newPT) ==
  LET R == ReadRange(x, strat, bp)
      ER == Exp(Readable(t, R)) IN
  /\ SyncGate(t, R)
  /\ IF run[t].p \notin CanHandoff THEN Fail(t, x, "unsupported")
     ELSE \E oc \in Outcomes(ER) :
            IF oc.o = "fail" THEN Fail(t, x, "budget") ELSE Resolved(t, x, strat, bp, oc, newPT)

ZResolveSwitch(t, x) ==
  /\ Clear(t) /\ ResolveCommon(t, x) /\ xf[x].ty = "provider_switch"
  /\ LET p == run[t].p
         P == pt[t][p]
         others == {pt[t][q].seen : q \in Providers \ {p}}
         bp == IF M_DeltaFromLeaving THEN Max(others) ELSE P.seen
     IN IF P.alive /\ P.seen >= xf[x].sp
        THEN /\ xf' = [xf EXCEPT ![x].st = "consumed", ![x].strat = "native_resume", ![x].tp = p, ![x].hd = Len0]
             /\ UNCHANGED <<msgs, pt, run, abort, staleLeft>>
        ELSE \E stale \in (IF P.alive /\ staleLeft > 0 THEN BOOLEAN ELSE {FALSE}) :
               /\ staleLeft' = IF stale THEN staleLeft - 1 ELSE staleLeft
               /\ IF P.alive /\ ~stale
                  THEN ZHandoffOrFail(t, x, "delta", bp, FALSE)
                  ELSE ZHandoffOrFail(t, x, "full_summary", 0, TRUE)
  /\ UNCHANGED <<created, forkOf, kindOf, nx, vol, dup, crashLeft, syncW, dset>>

ZResolveLineage(t, x, ty) ==
  /\ Clear(t) /\ ResolveCommon(t, x) /\ xf[x].ty = ty
  /\ LET p == run[t].p
         s == xf[x].src
         S == pt[s][p]
         sp == xf[x].sp
         canNative == p \in NativeFork /\ S.alive /\ Exp(Hist(s, sp)) \subseteq S.ecov
                      /\ Host(s) = Host(t)   \* the provider forks only a native thread it holds
         cut(X) == IF M_ForkNoRollback THEN X ELSE {i \in X : i <= sp}
     IN IF canNative
        THEN /\ pt' = [pt EXCEPT ![t][p] = [alive |-> TRUE, pcov |-> cut(S.pcov), pst |-> cut(S.pst),
                                            ecov |-> cut(S.ecov), est |-> cut(S.est), seen |-> sp, dpend |-> FALSE]]
             /\ xf' = [xf EXCEPT ![x].st = "consumed", ![x].strat = "native_fork", ![x].tp = p, ![x].hd = Len0,
                                 ![x].sel = Exp(Hist(s, sp))]
             /\ UNCHANGED <<msgs, run, abort>>
        ELSE ZHandoffOrFail(t, x, "portable_replay", 0, TRUE)
  /\ UNCHANGED <<created, forkOf, kindOf, nx, vol, dup, crashLeft, staleLeft, syncW, dset>>

ZResolveCtx(t, x, ty, strat) ==
  /\ Clear(t) /\ ResolveCommon(t, x) /\ xf[x].ty = ty
  /\ ZHandoffOrFail(t, x, strat, xf[x].bp, ~pt[t][run[t].p].alive)
  /\ UNCHANGED <<created, forkOf, kindOf, nx, vol, dup, crashLeft, staleLeft, syncW, dset>>

-----------------------------------------------------------------------------
(* Settle: delivery row + settle receipt in one step; rows marked later.   *)
ZSettle(t, p, how, extra) ==
  /\ pt' = [pt EXCEPT ![t][p].ecov = @ \cup Payload(t, p) \cup extra,
                      ![t][p].est = @ \cup StatedOm(t, p),
                      ![t][p].seen = SeenAfter(t, p),
                      ![t][p].dpend = FALSE]
  /\ dset' = [dset EXCEPT ![t] = [ids |-> ResolvedFor(t, p), how |-> how]]

ZPersistInjected(t) ==
  /\ Clear(t) /\ vol[t] = "injected"
  /\ ZSettle(t, run[t].p, "injected", {})
  /\ vol' = [vol EXCEPT ![t] = "none"]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, run, xf, nx, dup, abort, crashLeft, staleLeft, syncW>>

ZPersistInline(t) ==
  /\ Clear(t) /\ vol[t] = "sent"
  /\ ZSettle(t, run[t].p, "inline", {run[t].u})
  /\ vol' = [vol EXCEPT ![t] = "none"]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, run, xf, nx, dup, abort, crashLeft, staleLeft, syncW>>

(* Host(t) marks the receipted transfer rows consumed, then clears it.     *)
MarkRows(t) ==
  /\ ~Clear(t)
  /\ xf' = [x \in XIds |-> IF x \in dset[t].ids
                           THEN [xf[x] EXCEPT !.dl = dset[t].how, !.st = "consumed"] ELSE xf[x]]
  /\ dset' = [dset EXCEPT ![t] = NoSettle]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, run, pt, nx, vol, dup, abort, crashLeft, staleLeft, syncW>>

-----------------------------------------------------------------------------
(* Status of a transfer row as the engine sees it (receipt applied).       *)
MSt(x) == IF xf[x].tgt \in Threads /\ x \in dset[xf[x].tgt].ids THEN "consumed" ELSE xf[x].st

(* Engine crash: as ContextTransfer. The settle receipt is durable         *)
(* (CMut VolatileSettle: it was only in memory).                           *)
ZCrash ==
  /\ crashLeft > 0
  /\ crashLeft' = crashLeft - 1
  /\ vol' = [t \in Threads |-> "none"]
  /\ run' = [t \in Threads |-> IF run[t].st = "live" THEN NoRun ELSE run[t]]
  /\ xf' = [x \in XIds |-> IF MSt(x) \in {"pending", "resolved"} /\ run[xf[x].tgt].st = "starting"
                           THEN [xf[x] EXCEPT !.cr = TRUE] ELSE xf[x]]
  /\ dset' = IF CMut = "VolatileSettle" THEN [t \in Threads |-> NoSettle] ELSE dset
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, pt, nx, dup, abort, staleLeft, syncW>>

-----------------------------------------------------------------------------
Same == UNCHANGED <<syncW, dset>>

ZEngine ==
  \/ \E t \in Threads, x \in XIds :
       \/ ZResolveSwitch(t, x)
       \/ ZResolveLineage(t, x, "fork") \/ ZResolveLineage(t, x, "subagent_spawn")
       \/ ZResolveCtx(t, x, "merge_back", "merge_delta") \/ ZResolveCtx(t, x, "subagent_result", "result")
  \/ \E t \in Threads :
       \/ Clear(t) /\ (DeliverBatch(t) \/ Uncertain(t) \/ Inject(t) \/ TurnStart(t)) /\ Same
       \/ ZPersistInjected(t) \/ ZPersistInline(t) \/ MarkRows(t)
  \/ \E t \in Threads, p \in Providers : Clear(t) /\ RecoverDelivery(t, p) /\ Same
  \/ \E d \in Devs, t \in Threads : Sync(d, t)

ZEnv ==
  \/ \E t \in Threads, p \in Providers : Clear(t) /\ UserSend(t, p) /\ Same
  \/ \E s, t \in Threads : (Fork(s, t) \/ SpawnChild(s, t)) /\ Same
  \/ \E f \in Threads : (MergeBack(f) \/ SubagentResult(f)) /\ Clear(forkOf[f].src) /\ Same
  \/ \E t \in Threads : Complete(t) /\ Same
  \/ ZCrash

ZNext == ZEngine \/ ZEnv
ZSpec == ZInit /\ [][ZNext]_zvars

-----------------------------------------------------------------------------
(* Implementation-level invariants.                                        *)
(* A settle receipt names only rows of its own target thread that are     *)
(* still resolved, and the delivery row is already settled.                *)
ZC_ReceiptLocal == \A t \in Threads : \A x \in dset[t].ids :
                     xf[x].tgt = t /\ xf[x].st = "resolved" /\ ~pt[t][xf[x].tp].dpend
(* Every consumed or resolved portable handoff read only messages that     *)
(* were known on host(target) (sync gate): sel and om lie in the range.    *)
ZC_SelInRange == \A x \in Used : xf[x].sel \cup xf[x].om \subseteq Exp(RangeOf(x))

(* Witnesses (must be VIOLATED): a cross-host read waits on the sync gate, *)
(* and a crash lands between the settle receipt and the row mark.          *)
W_CrossHostResolve == ~(\E x \in XIds : xf[x].st \in {"resolved", "consumed"} /\ xf[x].strat \notin {"none", "native_resume"}
                          /\ \E i \in RangeOf(x) : Host(msgs[i].th) # Host(xf[x].tgt))
W_CrashBeforeMark == ~(crashLeft = 0 /\ \E t \in Threads : ~Clear(t))
=============================================================================
