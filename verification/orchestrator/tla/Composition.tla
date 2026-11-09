----------------------------- MODULE Composition -----------------------------
(***************************************************************************)
(* Zeron Orchestrator v2, module Composition (AC-M1, DESIGN 9.1).          *)
(*                                                                         *)
(* All modules over ONE copy of the shared state. RunLifecycle (RL) and    *)
(* Delegation (D) are INSTANCEd over one RunBase copy (queue, held,        *)
(* liveSet, rord, intReq). NativeSubagents (NS) runs inside the runs of    *)
(* one thread NsT: its run slot (rst, rord) IS that thread's shared slot.  *)
(* ContextTransfer (CT) keeps its journal and transfer records; its run    *)
(* start is the shared run start. Placement is abstract (host per chat,    *)
(* device online flag, receiver-online gate on D's channel effects); the   *)
(* concrete Placement module is not INSTANCEd (its two fixed chats do not  *)
(* map onto the thread set).                                               *)
(*                                                                         *)
(* Layers (constants): UseNS, UseCT, UsePL. Composition mutants use the    *)
(* shared Mut switch: names "C_*" (seam guards), "NS_*" and "CT_*" (the   *)
(* NS / CT module mutants, passed through).                                *)
(*                                                                         *)
(* SEAM (shared vs per module)                                             *)
(*   shared: queue, held, liveSet, rord, intReq (RunBase). NS sees thread  *)
(*     NsT's slot as rst = (Live(NsT) ? live : idle), rord = rord[NsT].    *)
(*   RL only: attempts, provider events, retries, steers, usage limit.     *)
(*   D only: lineage, tasks, cohorts, questions, outbox, stop receipts.    *)
(*   NS only: native subagent records, provider stream (inbox).            *)
(*   CT only: journal, provider threads, transfers, delivery progress.     *)
(*   Run start = RL start /\ D start (task, wake accept, question run)     *)
(*               /\ NS!StartRun (on NsT) /\ CT!UserSend (if CT tracks t).  *)
(*   Provider root terminal = RL!ProviderTerminal /\ NS!PRootEnd or        *)
(*               NS!PInterrupt (on NsT) /\ CT!Complete (completed turn).   *)
(*   Run end   = RL!IngestEnd /\ D run-end hook (settle notice, R9 sweep,  *)
(*               question catch-up) /\ NS!Handle(rootEnd) orphan close     *)
(*               /\ CT turn end.                                           *)
(*   Stop      = RL!RequestInterrupt /\ D!Stop; cascade hop D!DoStop also  *)
(*               disarms the child's usage-limit resume (R14 per hop).     *)
(*   Narrow    = D!NarrowAccess /\ restart of every live descendant whose  *)
(*               harness cannot reconfigure in place (D3).                 *)
(*   Delegate  = D!Delegate /\ CT!SpawnChild when CT can record lineage.   *)
(*   See FINDINGS and DIFFS at the end of the module.                      *)
(***************************************************************************)
EXTENDS RunBase

CONSTANTS
  \* RunLifecycle
  RLMaxPerson, RLMaxServer, RLMaxRetry, RLMaxStop, RLMaxUL, RLMaxCrash, RLMaxAtt, FixLimitRelease,
  \* Delegation
  MaxDepth, MaxFanout, MaxGen, DMaxPerson, MaxFail, DMaxCrash, MaxAbandon, DMaxStop, MaxNarrow, MaxInterrupt,
  DupDelivery, Crash, LateAck, Questions, FixWakeSkipsFailHold, StopCheckAnc, PartialCrash, QSplit, MaxFollowUp,
  \* NativeSubagents (on thread NsT)
  UseNS, NsT, Subs, NoSub, MaxEp, NSMaxCrash, MaxDup, NSMaxInterrupt, MaxSessEnd, MaxInbox,
  Background, SilentKill,
  \* ContextTransfer
  UseCT, Providers, NoProv, CanHandoff, CanInject, NativeFork, MainProviders, MaxMsg, MaxX,
  CTMaxCrash, MaxStale, Forks, Spawns, Cuts, MaxCutK, BudgetFail,
  \* Composition
  InPlace,      \* threads whose harness can reconfigure access in place (D3)
  ToolNodes,    \* RL non-root nodes (ChildStart / ChildEnd) on: a state-space switch
  UsePL, Devices, RootDev, MaxOffline

ASSUME RootDev \in Devices /\ InPlace \subseteq Threads /\ NsT \in Threads

VARIABLES
  \* RunLifecycle
  att, why, emitted, kidOpen, pev, retries, steers, lastOut, rkind, ulArmed, ulDue,
  rPersonLeft, serverLeft, rStopLeft, ulLeft, rCrashLeft,
  \* Delegation
  created, parent, depth, access, rtask, waitOn, zWait,
  tSt, tWake, tDel, tCoh, pushes, kidIn,
  qPend, qAt, qVis, qWake, qUsed, answers, qEp,
  cDisp, cSt, cDel, cGen, outbox, replayable, rcpt, replayBad, receiptBad, doomed,
  dPersonLeft, failLeft, dCrashLeft, abandonLeft, dStopLeft, narrowLeft, interruptLeft,
  \* NativeSubagents
  nRst, nRord, nEndBy, nRec, nEst, nEep, nErun, nEbg, nHow, nClosedEp, nTask,
  nSess, nProot, nPst, nPep, nPrun, nPbg, nPprog, nInbox, nReplay,
  nCrashLeft, nDupLeft, nIntLeft, nSessLeft,
  \* ContextTransfer
  xMsgs, xCreated, xForkOf, xKindOf, xRun, xPt, xXf, xNx, xVol, xDup, xAbort, xCrashLeft, xStaleLeft,
  \* Composition ghosts
  ctTrk,      \* CT tracks the current run of t (its CT run started at this run's start)
  consumedBy, \* kind of the run that consumed transfer x ("none" until consumed)
  attAcc,     \* access in force when the current attempt started (D3)
  waitAtt,    \* attempt that issued the current blocking wait
  f9w,        \* F9 witness: 1 = resume run started with a wake queued, 2 = that wake delivered after it
  owed,       \* threads that owe a D3 clamp restart
  \* Placement (abstract)
  host, online, offLeft

-----------------------------------------------------------------------------
(* INSTANCES                                                               *)
(*                                                                         *)
(* Extra instances neutralize ONE shared variable each, by substituting a  *)
(* constant for it: that module's write becomes vacuous (the prime of a    *)
(* constant is the constant), and the other module, conjoined in the same  *)
(* step, pins the real value.                                              *)
(*   RLh: held <- NoHolds. Run start: D's queue gate decides (R12 lets a   *)
(*        wake pass a "fail" hold; RL's own gate would block it).          *)
(*   RLq: queue <- NoQueues. Stop: D drops queued wakes; RL's              *)
(*        RequestInterrupt says UNCHANGED queue.                           *)
(*   Dx:  held <- NoHolds. Run end on a non-failed outcome: RL owns the    *)
(*        hold ("limit" on a usage limit, which D does not know).          *)
(***************************************************************************)
NoHolds  == [t \in Threads |-> "none"]
NoQueues == [t \in Threads |-> <<>>]
NoWaits  == [t \in Threads |-> NoThread]

RL == INSTANCE RunLifecycle WITH
        MaxPerson <- RLMaxPerson, MaxServer <- RLMaxServer, MaxRetry <- RLMaxRetry,
        MaxStop <- RLMaxStop, MaxUL <- RLMaxUL, MaxCrash <- RLMaxCrash, MaxAtt <- RLMaxAtt,
        personLeft <- rPersonLeft, stopLeft <- rStopLeft, crashLeft <- rCrashLeft
RLh == INSTANCE RunLifecycle WITH
        MaxPerson <- RLMaxPerson, MaxServer <- RLMaxServer, MaxRetry <- RLMaxRetry,
        MaxStop <- RLMaxStop, MaxUL <- RLMaxUL, MaxCrash <- RLMaxCrash, MaxAtt <- RLMaxAtt,
        personLeft <- rPersonLeft, stopLeft <- rStopLeft, crashLeft <- rCrashLeft,
        held <- NoHolds
RLq == INSTANCE RunLifecycle WITH
        MaxPerson <- RLMaxPerson, MaxServer <- RLMaxServer, MaxRetry <- RLMaxRetry,
        MaxStop <- RLMaxStop, MaxUL <- RLMaxUL, MaxCrash <- RLMaxCrash, MaxAtt <- RLMaxAtt,
        personLeft <- rPersonLeft, stopLeft <- rStopLeft, crashLeft <- rCrashLeft,
        queue <- NoQueues

D == INSTANCE Delegation WITH
        MaxPerson <- DMaxPerson, MaxCrash <- DMaxCrash, MaxStop <- DMaxStop,
        personLeft <- dPersonLeft, crashLeft <- dCrashLeft, stopLeft <- dStopLeft
Dx == INSTANCE Delegation WITH
        MaxPerson <- DMaxPerson, MaxCrash <- DMaxCrash, MaxStop <- DMaxStop,
        personLeft <- dPersonLeft, crashLeft <- dCrashLeft, stopLeft <- dStopLeft,
        held <- NoHolds

NS == INSTANCE NativeSubagents WITH
        rst <- nRst, rord <- nRord,
        endBy <- nEndBy, recovering <- nRec, est <- nEst, eep <- nEep, erun <- nErun, ebg <- nEbg,
        how <- nHow, closedEp <- nClosedEp, task <- nTask,
        sess <- nSess, proot <- nProot, pst <- nPst, pep <- nPep, prun <- nPrun, pbg <- nPbg, pprog <- nPprog,
        inbox <- nInbox, replay <- nReplay,
        crashLeft <- nCrashLeft, dupLeft <- nDupLeft, intLeft <- nIntLeft, sessLeft <- nSessLeft,
        MaxCrash <- NSMaxCrash, MaxInterrupt <- NSMaxInterrupt,
        M_AnyEndEndsRun <- M("NS_AnyEndEndsRun"), M_NoRunGuard <- M("NS_NoRunGuard"),
        M_NoOrphanClose <- M("NS_NoOrphanClose"), M_NoSessClose <- M("NS_NoSessClose"),
        M_NoEpochGuard <- M("NS_NoEpochGuard"), M_ReopenTerminal <- M("NS_ReopenTerminal"),
        M_NoTaskMirror <- M("NS_NoTaskMirror"), M_NoRecoveryClose <- M("NS_NoRecoveryClose"),
        M_OrphanCloseBg <- M("NS_OrphanCloseBg")

CT == INSTANCE ContextTransfer WITH
        Main <- Root, MaxCrash <- CTMaxCrash, FixRecoverDelivery <- TRUE,
        msgs <- xMsgs, created <- xCreated, forkOf <- xForkOf, kindOf <- xKindOf, run <- xRun,
        pt <- xPt, xf <- xXf, nx <- xNx, vol <- xVol, dup <- xDup, abort <- xAbort,
        crashLeft <- xCrashLeft, staleLeft <- xStaleLeft,
        M_ReadHead <- M("CT_ReadHead"), M_InputInHandoff <- M("CT_InputInHandoff"),
        M_NoDedup <- M("CT_NoDedup"), M_NoUncertainGuard <- M("CT_NoUncertainGuard"),
        M_DeltaFromLeaving <- M("CT_DeltaFromLeaving"), M_NoAnchors <- M("CT_NoAnchors"),
        M_SilentCut <- M("CT_SilentCut"), M_ForkNoRollback <- M("CT_ForkNoRollback"),
        M_SkipFailed <- M("CT_SkipFailed"), M_ReplaceNoSummary <- M("CT_ReplaceNoSummary")

-----------------------------------------------------------------------------
rlOnly == <<att, why, emitted, kidOpen, pev, retries, steers, lastOut, rkind, ulArmed, ulDue,
            rPersonLeft, serverLeft, rStopLeft, ulLeft, rCrashLeft>>
dOnly  == <<created, parent, depth, access, rtask, waitOn, zWait, tSt, tWake, tDel, tCoh, pushes, kidIn,
            qPend, qAt, qVis, qWake, qUsed, answers, qEp, cDisp, cSt, cDel, cGen,
            outbox, replayable, rcpt, replayBad, receiptBad, doomed,
            dPersonLeft, failLeft, dCrashLeft, abandonLeft, dStopLeft, narrowLeft, interruptLeft>>
dBud   == <<dPersonLeft, failLeft, dCrashLeft, abandonLeft, dStopLeft, narrowLeft, interruptLeft>>
nsv    == <<nRst, nRord, nEndBy, nRec, nEst, nEep, nErun, nEbg, nHow, nClosedEp, nTask,
            nSess, nProot, nPst, nPep, nPrun, nPbg, nPprog, nInbox, nReplay,
            nCrashLeft, nDupLeft, nIntLeft, nSessLeft>>
ctv    == <<xMsgs, xCreated, xForkOf, xKindOf, xRun, xPt, xXf, xNx, xVol, xDup, xAbort, xCrashLeft, xStaleLeft>>
ghost  == <<waitAtt, f9w, owed>>   \* attAcc and consumedBy are set only by GhostStep
ghostNoOwed == <<waitAtt, f9w>>
plv    == <<host, online, offLeft>>
ext    == <<nsv, ctv, ctTrk>>
coreVars == <<base, rlOnly, dOnly>>
vars   == <<base, rlOnly, dOnly, nsv, ctv, ctTrk, consumedBy, attAcc, ghost, plv>>

Children == D!Children
NoDev    == "nodev"
IsNs(t)  == UseNS /\ t = NsT
InPlaceSet == InPlace \cup (IF UseNS THEN {NsT} ELSE {})

-----------------------------------------------------------------------------
(* Placement (abstract). Each chat has one host device, fixed when the     *)
(* chat is created (S9). A device may go offline (laptop asleep, Cloud     *)
(* stopped). The host runs every engine and provider step of its chats,    *)
(* so those need the host online. D's outbox is the channel: an effect is  *)
(* taken only when its receiver's host is online. Person sends and answers *)
(* land in the synced doc from any device.                                 *)
On(t) == ~UsePL \/ (host[t] \in Devices /\ online[host[t]])
Receiver(e) == IF e.k = "notice" THEN parent[e.t] ELSE e.t

PLInit ==
  /\ host = [t \in Threads |-> IF t = Root THEN RootDev ELSE NoDev]
  /\ online = [d \in Devices |-> TRUE]
  /\ offLeft = IF UsePL THEN MaxOffline ELSE 0

-----------------------------------------------------------------------------
(* Seam helpers *)

(* CT tracks the current run of t: a provider step of t waits for CT's    *)
(* turn start (lazy resolution and delivery happen first).                 *)
TurnOn(t) == ~UseCT \/ ~ctTrk[t] \/ xRun[t].st = "live"
AgentOK(t) == Live(t) /\ ~emitted[t] /\ On(t) /\ TurnOn(t) /\ t \notin owed

(* New attempt in thread set T: the old attempt's wait call died with it.  *)
(* Restated from D!EndRun's wait part. Mutant "C_attemptKeepsWait".         *)
AbandonWaits(T) ==
  IF M("C_attemptKeepsWait") THEN UNCHANGED <<waitOn, zWait>>
  ELSE /\ waitOn' = [u \in Threads |-> IF u \in T THEN NoThread ELSE waitOn[u]]
       /\ zWait'  = [u \in Threads |-> IF u \in T /\ LateAck /\ waitOn[u] # NoThread
                                       THEN waitOn[u] ELSE zWait[u]]

dNoWait == <<created, parent, depth, access, rtask, tSt, tWake, tDel, tCoh, pushes, kidIn,
             qPend, qAt, qVis, qWake, qUsed, answers, qEp, cDisp, cSt, cDel, cGen,
             outbox, replayable, rcpt, replayBad, receiptBad, doomed, dBud>>

(* RL's StartFromQueue body without the queue pop (question run).          *)
(* Restated; see DIFFS.                                                    *)
RLStartBody(t, k) ==
  /\ RL!NewAttempt(t, 1, "initial")
  /\ retries' = [retries EXCEPT ![t] = 0]
  /\ steers'  = [steers EXCEPT ![t] = 0]
  /\ rkind'   = [rkind EXCEPT ![t] = k]
  /\ ulArmed' = [ulArmed EXCEPT ![t] = 0]
  /\ ulDue'   = [ulDue EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<pev, lastOut, rPersonLeft, serverLeft, rStopLeft, ulLeft, rCrashLeft>>

(* D's start for a run that is not a queue head (usage-limit resume).      *)
DStartBody(t) ==
  /\ rtask'  = [rtask EXCEPT ![t] = FALSE]
  /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
  /\ UNCHANGED <<created, parent, depth, access, zWait, tSt, tWake, tDel, tCoh, pushes, kidIn,
                 qPend, qAt, qVis, qWake, qUsed, answers, qEp, cDisp, cSt, cDel, cGen,
                 outbox, replayable, rcpt, replayBad, receiptBad, doomed, dBud>>

(* D's run-end hook: D!EndRun with D!RootFail's UNCHANGED list minus its   *)
(* failure budget (RL's provider events bound failures here). "failed":    *)
(* D and RL agree on the hold, so the real D. Other outcomes: Dx.          *)
DEndUnch == UNCHANGED <<created, parent, depth, access, rtask, tSt, tWake, tCoh, pushes, kidIn,
                        qUsed, answers, qEp, replayable, rcpt, replayBad, receiptBad, doomed, dBud>>
DEndHook(t, out) ==
  CASE out = "completed"   -> Dx!RootComplete(t)
    [] out = "interrupted" -> D!ConfirmInterrupt(t)
    [] out = "failed"      -> D!EndRun(t, "failed") /\ DEndUnch
    [] OTHER               -> Dx!EndRun(t, out) /\ DEndUnch

OutOf(o) == CASE o = "completed"   -> "completed"
              [] o = "interrupted" -> "interrupted"
              [] o = "usage_limit" -> "usage_limit"
              [] OTHER             -> "failed"

(* NS hooks *)
NSStart(t) == IF IsNs(t) THEN NS!StartRun ELSE UNCHANGED nsv
NSProvEnd(t, o) ==
  IF IsNs(t)
  THEN \/ o \in {"completed", "failed", "usage_limit"} /\ NS!PRootEnd
       \/ o = "interrupted" /\ \E b \in BOOLEAN : NS!PInterrupt(b)
  ELSE UNCHANGED nsv
(* The run end handles NS's rootEnd: orphan close of foreground natives    *)
(* (N3). Mutant "C_nsNoRootSync": the run ends without it.                 *)
CurRootEnd == nInbox # <<>> /\ Head(nInbox).k = "rootEnd" /\ Live(NsT) /\ Head(nInbox).r = rord[NsT]
NSIngestEnd(t) ==
  IF IsNs(t)
  THEN IF M("C_nsNoRootSync")
       THEN nRst' = "idle" /\ UNCHANGED <<nRord, nEndBy, nRec, nEst, nEep, nErun, nEbg, nHow, nClosedEp, nTask,
                                         nSess, nProot, nPst, nPep, nPrun, nPbg, nPprog, nInbox, nReplay,
                                         nCrashLeft, nDupLeft, nIntLeft, nSessLeft>>
       ELSE CurRootEnd /\ NS!Handle
  ELSE UNCHANGED nsv

(* CT hooks. A run start on a CT thread is CT's UserSend (the run's input *)
(* joins the journal; transfers resolve lazily before the turn starts).   *)
(* If CT cannot record it (journal bound), the run is untracked.           *)
CTCan(t) == UseCT /\ xCreated[t] /\ ENABLED (\E p \in Providers : CT!UserSend(t, p))
CTStart(t) ==
  IF CTCan(t)
  THEN (\E p \in Providers : CT!UserSend(t, p)) /\ ctTrk' = [ctTrk EXCEPT ![t] = TRUE]
  ELSE UNCHANGED ctv /\ ctTrk' = [ctTrk EXCEPT ![t] = FALSE]
(* Provider terminal on a tracked run: a completed turn appends the       *)
(* assistant message (CT!Complete). A turn that never started (CT failed  *)
(* the start, or CT's crash killed it) can only fail.                     *)
CTProvEnd(t, o) ==
  IF UseCT /\ ctTrk[t]
  THEN IF xRun[t].st = "live"
       THEN xVol[t] = "none" /\ (IF o = "completed" THEN CT!Complete(t) ELSE UNCHANGED ctv)
       ELSE xRun[t].st = "idle" /\ o \in {"failed", "crash"} /\ UNCHANGED ctv
  ELSE UNCHANGED ctv
(* CT has no turn end except Complete; a failed / interrupted / usage-   *)
(* limit end clears its run (glue, see DIFFS).                             *)
CTIngestEnd(t) ==
  /\ IF UseCT /\ ctTrk[t]
     THEN /\ xRun' = [xRun EXCEPT ![t] = CT!NoRun]
          /\ UNCHANGED <<xMsgs, xCreated, xForkOf, xKindOf, xPt, xXf, xNx, xVol, xDup, xAbort, xCrashLeft, xStaleLeft>>
     ELSE UNCHANGED ctv
  /\ ctTrk' = [ctTrk EXCEPT ![t] = FALSE]
CTSpawn(t, c) ==
  IF UseCT /\ ENABLED CT!SpawnChild(t, c) THEN CT!SpawnChild(t, c) ELSE UNCHANGED ctv

-----------------------------------------------------------------------------
(* Person *)

(* A chat exists once D created it (root, delegated child) or CT created *)
(* it (fork). Messages and stops go only to existing chats.               *)
Exists(t) == created[t] \/ (UseCT /\ xCreated[t])

(* RL!PersonSend /\ D!PersonSend. *)
A_PersonSend(t, m) ==
  /\ RL!PersonSend(t, m) /\ Exists(t)
  /\ IF created[t] THEN D!PersonSend(t) ELSE UNCHANGED dOnly
  /\ UNCHANGED <<ext, ghost, plv>>

(* RL!Steer. *)
A_Steer(t) == RL!Steer(t) /\ On(t) /\ UNCHANGED <<dOnly, ext, ghost, plv>>

(* RL!RestartActive. NS has no attempts, so not on NsT (see FINDINGS). *)
A_Restart(t, m) ==
  /\ RL!RestartActive(t, m) /\ On(t) /\ ~IsNs(t)
  /\ AbandonWaits({t})
  /\ UNCHANGED <<dNoWait, ext, ghost, plv>>

(* RL!RequestInterrupt /\ D!Stop. *)
A_Stop(t) ==
  /\ Exists(t)
  /\ IF created[t] THEN RLq!RequestInterrupt(t) /\ D!Stop(t)
                   ELSE RL!RequestInterrupt(t) /\ UNCHANGED dOnly
  /\ UNCHANGED <<ext, ghost, plv>>

(* D!InterruptChat (interrupt_chat: request only, no hold, no disposal). *)
A_InterruptChat(t) == D!InterruptChat(t) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

(* RL!ServerEnqueue (scheduled follow-up). *)
A_ServerEnqueue(t) == RL!ServerEnqueue(t) /\ Exists(t) /\ UNCHANGED <<dOnly, ext, ghost, plv>>

(* D!NarrowAccess, then a restart (RestartActive body) of each live       *)
(* affected thread that cannot reconfigure in place (D3). The restart is  *)
(* owed from the narrowing step on: the owing attempt takes no agent step  *)
(* until C_ClampRestart runs, so no tool call uses the old access. (One    *)
(* step would need D!NarrowAccess to drop the restarted attempt's wait;   *)
(* see DIFFS.) Mutant "C_narrowNoRestart" owes nothing.                    *)
Affected(t) == {t} \cup D!Desc(t)
MustRestart(t) == {d \in Affected(t) : Live(d) /\ ~emitted[d] /\ ~intReq[d] /\ d \notin InPlaceSet
                                       /\ ~M("C_narrowNoRestart")}
A_Narrow(t) ==
  /\ D!NarrowAccess(t) /\ On(t)
  /\ owed' = owed \cup MustRestart(t)
  /\ UNCHANGED <<rlOnly, ext, ghostNoOwed, plv>>

(* The owed clamp restart (RL!RestartActive body, no person budget). If   *)
(* the attempt already ended or is stopping, nothing is left to restart.  *)
A_ClampRestart(d) ==
  /\ d \in owed /\ On(d)
  /\ owed' = owed \ {d}
  /\ IF Live(d) /\ ~emitted[d] /\ ~intReq[d]
     THEN /\ pev'     = [pev EXCEPT ![d] = @ \cup {RL!Ev("root", rord[d], att[d], "interrupted")}]
          /\ RL!NewAttempt(d, RL!Sat(att[d] + 1), "steering_restart")
          /\ steers'  = [steers EXCEPT ![d] = 0]
          /\ AbandonWaits({d})
     ELSE UNCHANGED <<pev, att, why, emitted, kidOpen, steers, waitOn, zWait>>
  /\ UNCHANGED <<base, retries, lastOut, rkind, ulArmed, ulDue, rPersonLeft, serverLeft, rStopLeft, ulLeft, rCrashLeft,
                 dNoWait, ext, ghostNoOwed, plv>>

(* D!PersonAnswer. *)
A_PersonAnswer(q) == D!PersonAnswer(q) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

-----------------------------------------------------------------------------
(* Provider *)

(* RL!ProviderTerminal [/\ NS!PRootEnd | NS!PInterrupt] [/\ CT!Complete].  *)
(* A completed root terminal needs D!RootComplete's guard (no open wait,   *)
(* no own question), moved here so the event never sticks in pev. Mutant   *)
(* "C_completeWhileWaiting". On NsT no retryable outcome (NS has no        *)
(* attempts).                                                              *)
A_ProviderTerminal(t, o) ==
  /\ RL!ProviderTerminal(t, o) /\ On(t)
  /\ o \in {"transient", "crash"} => RLMaxRetry > 0   \* without a retry budget it is a failure
  /\ o = "completed" /\ ~M("C_completeWhileWaiting") => waitOn[t] = NoThread /\ ~D!Blocked(t)
  /\ NSProvEnd(t, o)
  /\ CTProvEnd(t, o)
  /\ UNCHANGED <<dOnly, ctTrk, ghost, plv>>

(* RL!ChildStart / RL!ChildEnd (a tool call node). *)
A_ChildStart(t) == ToolNodes /\ RL!ChildStart(t) /\ On(t) /\ TurnOn(t) /\ UNCHANGED <<dOnly, ext, ghost, plv>>
A_ChildEnd(t)   == RL!ChildEnd(t) /\ On(t) /\ UNCHANGED <<dOnly, ext, ghost, plv>>

(* RL!ResetPasses (the timer). *)
A_ResetPasses(t) == RL!ResetPasses(t) /\ UNCHANGED <<dOnly, ext, ghost, plv>>

-----------------------------------------------------------------------------
(* Engine: run start *)

(* RL!StartFromQueue (via RLh) /\ D!StartFromQueue [/\ NS, CT starts]. *)
A_StartQueue(t) ==
  /\ RLh!StartFromQueue(t) /\ D!StartFromQueue(t) /\ On(t)
  /\ NSStart(t) /\ CTStart(t)
  /\ UNCHANGED <<ghost, plv>>

(* RL!StartFromQueue (via RLh) /\ D!StartFromQueueWake: the wake run       *)
(* accepts its delivery at start (T6) [/\ NS, CT starts].                  *)
A_StartWake(t) ==
  /\ RLh!StartFromQueue(t) /\ D!StartFromQueueWake(t) /\ On(t)
  /\ NSStart(t) /\ CTStart(t)
  /\ f9w' = [f9w EXCEPT ![t] = IF @ = 1 THEN 2 ELSE @]
  /\ UNCHANGED <<waitAtt, owed, plv>>

(* D!StartQuestion /\ RL start body [/\ NS, CT starts]. *)
A_StartQuestion(t, q) ==
  /\ D!StartQuestion(t, q) /\ RLStartBody(t, "wake") /\ On(t)
  /\ NSStart(t) /\ CTStart(t)
  /\ UNCHANGED <<ghost, plv>>

(* RL!UsageLimitRecovery /\ D start body [/\ NS, CT starts]. *)
HasQueuedWake(t) == \E i \in 1..Len(queue[t]) : queue[t][i].k = "wake"
A_ULRecovery(t) ==
  /\ RL!UsageLimitRecovery(t) /\ DStartBody(t) /\ On(t)
  /\ NSStart(t) /\ CTStart(t)
  /\ f9w' = [f9w EXCEPT ![t] = IF HasQueuedWake(t) THEN 1 ELSE @]
  /\ UNCHANGED <<waitAtt, owed, plv>>

-----------------------------------------------------------------------------
(* Engine: run end and attempts *)

(* RL!IngestEnd /\ D run-end hook (D!RootComplete | D!RootFail |           *)
(* D!ConfirmInterrupt) [/\ NS!Handle(rootEnd) /\ CT turn end]. Mutant      *)
(* "C_endNoHook": no D hook.                                               *)
A_IngestEnd(t, e) ==
  /\ RL!IngestEnd(t, e) /\ On(t)
  /\ IF M("C_endNoHook") THEN UNCHANGED dOnly ELSE DEndHook(t, OutOf(e.o))
  /\ NSIngestEnd(t) /\ CTIngestEnd(t)
  /\ UNCHANGED <<ghost, plv>>

(* RL!RetryAttempt: a new attempt abandons the old attempt's wait. *)
A_RetryAttempt(t, e) ==
  /\ RL!RetryAttempt(t, e) /\ On(t)
  /\ AbandonWaits({t})
  /\ UNCHANGED <<dNoWait, ext, ghost, plv>>

(* RL!IngestChild, RL!IngestStale. *)
A_IngestChild(t, e) == RL!IngestChild(t, e) /\ On(t) /\ UNCHANGED <<dOnly, ext, ghost, plv>>
A_IngestStale(t, e) == RL!IngestStale(t, e) /\ On(t) /\ UNCHANGED <<dOnly, ext, ghost, plv>>

-----------------------------------------------------------------------------
(* Agent tool steps inside the current attempt *)

(* D!Delegate [/\ CT!SpawnChild]. A CT fork is never a D child. The child's *)
(* host is chosen at creation (abstract PL).                               *)
A_Delegate(t, c, m, a) ==
  /\ D!Delegate(t, c, m, a) /\ AgentOK(t)
  /\ ~xCreated[c]
  /\ CTSpawn(t, c)
  /\ \E d \in (IF UsePL THEN Devices ELSE {NoDev}) : host' = [host EXCEPT ![c] = d]
  /\ waitAtt' = [waitAtt EXCEPT ![t] = IF m = "wait" THEN att[t] ELSE @]
  /\ UNCHANGED <<rlOnly, nsv, ctTrk, f9w, owed, online, offLeft>>

(* D!FollowUp (send_messages to an own settled child: a new task). The    *)
(* child's RL sees the queued task message like Delegate's first one.     *)
A_FollowUp(t, c, m) ==
  /\ D!FollowUp(t, c, m) /\ AgentOK(t)
  /\ waitAtt' = [waitAtt EXCEPT ![t] = IF m = "wait" THEN att[t] ELSE @]
  /\ UNCHANGED <<rlOnly, ext, f9w, owed, plv>>

(* D!ReadResult (task_status read). *)
A_ReadResult(p, c) == D!ReadResult(p, c) /\ AgentOK(p) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

(* D!RaiseQuestion. *)
A_RaiseQuestion(c) == D!RaiseQuestion(c) /\ AgentOK(c) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

(* D!AgentAnswer. *)
A_AgentAnswer(q, x) == D!AgentAnswer(q, x) /\ AgentOK(x) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

(* QSplit questions (DESIGN 3.3, host-local hops): D!RaiseQuestionS,      *)
(* D!AgentAnswerS, D!PersonAnswerS and the hop effects D!DoQUp, D!DoAns,   *)
(* D!DoRel, D!DoQClosed.                                                   *)
A_RaiseQuestionS(c)   == D!RaiseQuestionS(c) /\ AgentOK(c) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>
A_AgentAnswerS(q, x)  == D!AgentAnswerS(q, x) /\ AgentOK(x) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>
A_PersonAnswerS(q)    == D!PersonAnswerS(q) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>
A_DoQHop(e) == (D!DoQUp(e) \/ D!DoAns(e) \/ D!DoRel(e) \/ D!DoQClosed(e)) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

(* D!WaitReturn: the reply counts as an ack only while the attempt that    *)
(* called the wait still runs (S7 via the harness stream). Mutant          *)
(* "C_waitNoEmitGuard" drops ~emitted.                                     *)
A_WaitReturn(t) ==
  /\ D!WaitReturn(t) /\ On(t)
  /\ ~M("C_waitNoEmitGuard") => ~emitted[t]
  /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

(* D!WaitReturnQuestion, D!WaitTimeout, D!WaitAbandon, D!LateWaitAck. *)
A_WaitReturnQuestion(t) == D!WaitReturnQuestion(t) /\ On(t) /\ ~emitted[t] /\ UNCHANGED <<rlOnly, ext, ghost, plv>>
A_WaitTimeout(t)  == D!WaitTimeout(t) /\ On(t) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>
A_WaitAbandon(t)  == D!WaitAbandon(t) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>
A_LateWaitAck(t)  == D!LateWaitAck(t) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

-----------------------------------------------------------------------------
(* Engine: durable effects (the channel) *)

EffOK(e) == ~UsePL \/ (Receiver(e) \in Threads /\ On(Receiver(e)))

(* D!DoNotice. *)
A_DoNotice(e, how) == D!DoNotice(e, how) /\ EffOK(e) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

(* D!DoOffer. *)
A_DoOffer(e, how) == D!DoOffer(e, how) /\ EffOK(e) /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

(* D!DoStop /\ R14 on the hop: the first stop on the child disarms its     *)
(* usage-limit resume, as RL!RequestInterrupt does on the stopped thread.  *)
(* Restated from RL!RequestInterrupt. Mutant "C_cascadeNoDisarm".           *)
A_DoStop(e, how) ==
  LET first == <<e.t, e.r>> \notin rcpt /\ ~M("C_cascadeNoDisarm") IN
  /\ D!DoStop(e, how) /\ EffOK(e)
  /\ ulArmed' = [ulArmed EXCEPT ![e.t] = IF first THEN 0 ELSE @]
  /\ ulDue'   = [ulDue EXCEPT ![e.t] = IF first THEN FALSE ELSE @]
  /\ UNCHANGED <<att, why, emitted, kidOpen, pev, retries, steers, lastOut, rkind,
                 rPersonLeft, serverLeft, rStopLeft, ulLeft, rCrashLeft, ext, ghost, plv>>

(* D!CrashEngine (waits die; the outbox is durable). *)
A_CrashEngine == D!CrashEngine /\ UNCHANGED <<rlOnly, ext, ghost, plv>>

(* Placement (abstract) *)
A_GoOffline(d) ==
  /\ UsePL /\ online[d] /\ offLeft > 0
  /\ online' = [online EXCEPT ![d] = FALSE] /\ offLeft' = offLeft - 1
  /\ UNCHANGED <<coreVars, ext, ghost, host>>
A_GoOnline(d) ==
  /\ UsePL /\ ~online[d]
  /\ online' = [online EXCEPT ![d] = TRUE]
  /\ UNCHANGED <<coreVars, ext, ghost, host, offLeft>>

-----------------------------------------------------------------------------
(* NativeSubagents steps that touch only NS state (provider stream on      *)
(* NsT, and the engine's handling of non-root events).                     *)
NsFrame == UNCHANGED <<coreVars, ctv, ctTrk, ghost, plv>>

(* NS!PSpawn, NS!PResume: only inside a started turn of NsT. *)
A_NsSpawn(s, bg) == UseNS /\ NS!PSpawn(s, bg) /\ On(NsT) /\ TurnOn(NsT) /\ NsFrame
A_NsResume(s)    == UseNS /\ NS!PResume(s) /\ On(NsT) /\ TurnOn(NsT) /\ NsFrame
(* NS!PProgress, NS!PSubEnd, NS!PSessionEnd, NS!Replay. *)
A_NsProgress(s)    == UseNS /\ NS!PProgress(s) /\ On(NsT) /\ NsFrame
A_NsSubEnd(s, out) == UseNS /\ NS!PSubEnd(s, out) /\ On(NsT) /\ NsFrame
A_NsSessionEnd     == UseNS /\ NS!PSessionEnd /\ On(NsT) /\ NsFrame
A_NsReplay(e)      == UseNS /\ NS!Replay(e) /\ NsFrame
(* NS!Handle on any head but the current run's rootEnd (that one is the   *)
(* run end, C_IngestEnd). Mutant "NS_AnyEndEndsRun" lets a native end end *)
(* the run, which then shows on the shared slot (FinishRun).               *)
A_NsHandle ==
  /\ UseNS /\ ~CurRootEnd /\ On(NsT) /\ NS!Handle
  /\ \/ UNCHANGED <<liveSet, intReq>>
     \/ M("NS_AnyEndEndsRun") /\ FinishRun(NsT)
  /\ UNCHANGED <<queue, held, rord, rlOnly, dOnly, ctv, ctTrk, ghost, plv>>
(* NS!Crash (engine crash on NsT's host): kills the provider process. Only *)
(* between runs: during a run NS ends it by recovery, while RL treats a    *)
(* provider crash as a new provider_recovery attempt (see FINDINGS).       *)
A_NsCrash   == UseNS /\ NS!Crash /\ ~Live(NsT) /\ NsFrame
A_NsRecover == UseNS /\ NS!Recover /\ NsFrame

-----------------------------------------------------------------------------
(* ContextTransfer steps that touch only CT state. *)
CtFrame == UNCHANGED <<coreVars, nsv, ctTrk, ghost, plv>>

(* CT!Fork, CT!MergeBack: a fork is a new chat, never a D child. *)
A_CtFork(s, f)     == UseCT /\ CT!Fork(s, f) /\ ~created[f] /\ On(s) /\ CtFrame
A_CtMergeBack(f)   == UseCT /\ CT!MergeBack(f) /\ On(f) /\ CtFrame
(* CT!SubagentResult: the parent's host records the child's result as a   *)
(* transfer once D settled the task (the notice arrived).                  *)
A_CtSubagentResult(c) ==
  /\ UseCT /\ CT!SubagentResult(c) /\ c \in Children /\ tSt[c] = "settled" /\ On(parent[c]) /\ CtFrame
(* CT engine: lazy resolution and delivery at the target's run start. *)
A_CtResolve(t, x) == UseCT /\ CT!ResolveAny(t, x) /\ On(t) /\ CtFrame
A_CtDeliver(t)    == UseCT /\ CT!DeliverAny(t) /\ On(t) /\ CtFrame
A_CtRecoverDelivery(t, p) == UseCT /\ CT!RecoverDelivery(t, p) /\ On(t) /\ CtFrame
(* CT!Crash (CT-scoped engine crash: a live turn is lost, a starting run  *)
(* is re-driven). The RL run of a lost turn can only fail (CTProvEnd).     *)
A_CtCrash == UseCT /\ CT!Crash /\ CtFrame
(* Seam mutant "C_ctFreeStart": CT starts a run on its own (no real run). *)
A_CtFreeSend(t, p) == UseCT /\ M("C_ctFreeStart") /\ CT!UserSend(t, p) /\ CtFrame

-----------------------------------------------------------------------------
(* Ghosts: access in force at each attempt start (an in-place reconfigure  *)
(* applies a narrowing at once), and the kind of the run that consumed a   *)
(* transfer.                                                               *)
GhostStep ==
  /\ attAcc' = [u \in Threads |->
                 IF att'[u] # att[u] \/ rord'[u] # rord[u] \/ (u \in InPlaceSet /\ access'[u] # access[u])
                 THEN access'[u] ELSE attAcc[u]]
  /\ consumedBy' = [x \in CT!XIds |->
                     IF xXf[x].st # "consumed" /\ xXf'[x].st = "consumed"
                     THEN (IF xXf[x].tgt \in Threads THEN rkind'[xXf[x].tgt] ELSE "none")
                     ELSE consumedBy[x]]

(* Documented terminal states: nothing owed in any layer. *)
NsQuiet == ~UseNS \/ (~nRec /\ nInbox = <<>> /\ \A s \in Subs : nPst[s] # "running")
CtQuiet == ~UseCT \/ \A t \in Threads : xRun[t].st = "idle" /\ xVol[t] = "none"
C_Quiescent == RL!RL_Quiescent /\ D!D_Quiescent /\ NsQuiet /\ CtQuiet
(* At composition bounds the run-ordinal bound can bite (MaxRuns); a thread *)
(* that used every run ordinal is a documented terminal too.               *)
RunBoundHit == \E t \in Threads : rord[t] = MaxRuns
C_Done == (C_Quiescent \/ RunBoundHit) /\ UNCHANGED vars

Init ==
  /\ RL!RL_Init /\ D!D_Init /\ NS!Init /\ CT!Init
  /\ ctTrk = [t \in Threads |-> FALSE]
  /\ consumedBy = [x \in CT!XIds |-> "none"]
  /\ owed = {} /\ attAcc = [t \in Threads |-> 0] /\ waitAtt = [t \in Threads |-> 0] /\ f9w = [t \in Threads |-> 0]
  /\ PLInit

(* NS's run slot mirrors NsT's shared slot (rst, rord). It is a copy, not *)
(* an INSTANCE substitution: TLC 2.19 / tla-ts lost the outer context when *)
(* evaluating "rst <- (IF Live(NsT) ...)" (identifier liveSet undefined).  *)
(* Every step keeps the copy equal to the shared slot, so it is the same   *)
(* constraint.                                                             *)
NsSync ==
  UseNS => /\ nRst' = (IF liveSet'[NsT] # {} THEN "live" ELSE "idle")
           /\ nRord' = rord'[NsT]
NsMirror == UseNS => (nRst = (IF Live(NsT) THEN "live" ELSE "idle") /\ nRord = rord[NsT])

(* Each composed action as TLC names it (coverage): the body A_* and the  *)
(* per-step ghost and NS-mirror updates (Post).                           *)
Post == GhostStep /\ NsSync
C_PersonSend(t, m) == A_PersonSend(t, m) /\ Post
C_Steer(t) == A_Steer(t) /\ Post
C_Restart(t, m) == A_Restart(t, m) /\ Post
C_Stop(t) == A_Stop(t) /\ Post
C_InterruptChat(t) == A_InterruptChat(t) /\ Post
C_ServerEnqueue(t) == A_ServerEnqueue(t) /\ Post
C_Narrow(t) == A_Narrow(t) /\ Post
C_ClampRestart(d) == A_ClampRestart(d) /\ Post
C_PersonAnswer(q) == A_PersonAnswer(q) /\ Post
C_ProviderTerminal(t, o) == A_ProviderTerminal(t, o) /\ Post
C_ChildStart(t) == A_ChildStart(t) /\ Post
C_ChildEnd(t) == A_ChildEnd(t) /\ Post
C_ResetPasses(t) == A_ResetPasses(t) /\ Post
C_StartQueue(t) == A_StartQueue(t) /\ Post
C_StartWake(t) == A_StartWake(t) /\ Post
C_StartQuestion(t, q) == A_StartQuestion(t, q) /\ Post
C_ULRecovery(t) == A_ULRecovery(t) /\ Post
C_IngestEnd(t, e) == A_IngestEnd(t, e) /\ Post
C_RetryAttempt(t, e) == A_RetryAttempt(t, e) /\ Post
C_IngestChild(t, e) == A_IngestChild(t, e) /\ Post
C_IngestStale(t, e) == A_IngestStale(t, e) /\ Post
C_Delegate(t, c, m, a) == A_Delegate(t, c, m, a) /\ Post
C_FollowUp(t, c, m) == A_FollowUp(t, c, m) /\ Post
C_ReadResult(p, c) == A_ReadResult(p, c) /\ Post
C_RaiseQuestion(c) == A_RaiseQuestion(c) /\ Post
C_AgentAnswer(q, x) == A_AgentAnswer(q, x) /\ Post
C_RaiseQuestionS(c) == A_RaiseQuestionS(c) /\ Post
C_AgentAnswerS(q, x) == A_AgentAnswerS(q, x) /\ Post
C_PersonAnswerS(q) == A_PersonAnswerS(q) /\ Post
C_DoQHop(e) == A_DoQHop(e) /\ Post
C_WaitReturn(t) == A_WaitReturn(t) /\ Post
C_WaitReturnQuestion(t) == A_WaitReturnQuestion(t) /\ Post
C_WaitTimeout(t) == A_WaitTimeout(t) /\ Post
C_WaitAbandon(t) == A_WaitAbandon(t) /\ Post
C_LateWaitAck(t) == A_LateWaitAck(t) /\ Post
C_DoNotice(e, how) == A_DoNotice(e, how) /\ Post
C_DoOffer(e, how) == A_DoOffer(e, how) /\ Post
C_DoStop(e, how) == A_DoStop(e, how) /\ Post
C_CrashEngine == A_CrashEngine /\ Post
C_GoOffline(d) == A_GoOffline(d) /\ Post
C_GoOnline(d) == A_GoOnline(d) /\ Post
C_NsSpawn(s, bg) == A_NsSpawn(s, bg) /\ Post
C_NsResume(s) == A_NsResume(s) /\ Post
C_NsProgress(s) == A_NsProgress(s) /\ Post
C_NsSubEnd(s, out) == A_NsSubEnd(s, out) /\ Post
C_NsSessionEnd == A_NsSessionEnd /\ Post
C_NsReplay(e) == A_NsReplay(e) /\ Post
C_NsHandle == A_NsHandle /\ Post
C_NsCrash == A_NsCrash /\ Post
C_NsRecover == A_NsRecover /\ Post
C_CtFork(s, f) == A_CtFork(s, f) /\ Post
C_CtMergeBack(f) == A_CtMergeBack(f) /\ Post
C_CtSubagentResult(c) == A_CtSubagentResult(c) /\ Post
C_CtResolve(t, x) == A_CtResolve(t, x) /\ Post
C_CtDeliver(t) == A_CtDeliver(t) /\ Post
C_CtRecoverDelivery(t, p) == A_CtRecoverDelivery(t, p) /\ Post
C_CtCrash == A_CtCrash /\ Post
C_CtFreeSend(t, p) == A_CtFreeSend(t, p) /\ Post

Step ==
  \/ \E t \in Threads :
       \/ \E m \in RL!Modes : C_PersonSend(t, m) \/ C_Restart(t, m)
       \/ C_Steer(t) \/ C_Stop(t) \/ C_InterruptChat(t) \/ C_ServerEnqueue(t) \/ C_Narrow(t) \/ C_ClampRestart(t)
       \/ \E o \in RL!Outcomes : C_ProviderTerminal(t, o)
       \/ C_ChildStart(t) \/ C_ChildEnd(t) \/ C_ResetPasses(t)
       \/ C_StartQueue(t) \/ C_StartWake(t) \/ C_ULRecovery(t)
       \/ \E q \in Children : C_StartQuestion(t, q)
       \/ \E e \in pev[t] : C_IngestEnd(t, e) \/ C_RetryAttempt(t, e) \/ C_IngestChild(t, e) \/ C_IngestStale(t, e)
       \/ C_WaitReturn(t) \/ C_WaitReturnQuestion(t) \/ C_WaitTimeout(t) \/ C_WaitAbandon(t) \/ C_LateWaitAck(t)
  \/ \E t \in Threads, c \in Children, m \in {"wait", "async"}, a \in D!Levels : C_Delegate(t, c, m, a)
  \/ \E p \in Threads, c \in Children : C_ReadResult(p, c)
  \/ \E t \in Threads, c \in Children, m \in {"wait", "async"} : C_FollowUp(t, c, m)
  \/ \E q \in Children : C_RaiseQuestion(q) \/ C_PersonAnswer(q) \/ \E x \in Threads : C_AgentAnswer(q, x)
  \/ \E q \in Children : C_RaiseQuestionS(q) \/ C_PersonAnswerS(q) \/ \E x \in Threads : C_AgentAnswerS(q, x)
  \/ \E e \in outbox : C_DoQHop(e)
  \/ \E e \in outbox, how \in D!Hows : C_DoNotice(e, how) \/ C_DoOffer(e, how) \/ C_DoStop(e, how)
  \/ C_CrashEngine
  \/ \E d \in Devices : C_GoOffline(d) \/ C_GoOnline(d)
  \/ \E s \in Subs : \/ \E bg \in BOOLEAN : C_NsSpawn(s, bg)
                     \/ C_NsResume(s) \/ C_NsProgress(s)
                     \/ \E out \in NS!Outs : C_NsSubEnd(s, out)
  \/ C_NsSessionEnd \/ C_NsHandle \/ C_NsCrash \/ C_NsRecover
  \/ \E e \in nReplay : C_NsReplay(e)
  \/ \E s, f \in Threads : C_CtFork(s, f)
  \/ \E f \in Threads : C_CtMergeBack(f) \/ C_CtSubagentResult(f)
  \/ \E t \in Threads : \/ \E x \in CT!XIds : C_CtResolve(t, x)
                        \/ C_CtDeliver(t)
                        \/ \E p \in Providers : C_CtRecoverDelivery(t, p) \/ C_CtFreeSend(t, p)
  \/ C_CtCrash


(* Coverage only (AC-M5): the same steps, quantified over constant sets,  *)
(* so TLC splits and names every action (it does not split quantifiers   *)
(* over state sets such as pev[t] or outbox). Each guard re-checks        *)
(* membership, so StepCov = Step.                                          *)
StepCov ==
  \/ \E t \in Threads :
       \/ \E m \in RL!Modes : C_PersonSend(t, m) \/ C_Restart(t, m)
       \/ C_Steer(t) \/ C_Stop(t) \/ C_InterruptChat(t) \/ C_ServerEnqueue(t) \/ C_Narrow(t) \/ C_ClampRestart(t)
       \/ \E o \in RL!Outcomes : C_ProviderTerminal(t, o)
       \/ C_ChildStart(t) \/ C_ChildEnd(t) \/ C_ResetPasses(t)
       \/ C_StartQueue(t) \/ C_StartWake(t) \/ C_ULRecovery(t)
       \/ \E q \in Children : C_StartQuestion(t, q)
       \/ \E e \in RL!Events : C_IngestEnd(t, e) \/ C_RetryAttempt(t, e) \/ C_IngestChild(t, e) \/ C_IngestStale(t, e)
       \/ C_WaitReturn(t) \/ C_WaitReturnQuestion(t) \/ C_WaitTimeout(t) \/ C_WaitAbandon(t) \/ C_LateWaitAck(t)
  \/ \E t \in Threads, c \in Children, m \in {"wait", "async"}, a \in D!Levels : C_Delegate(t, c, m, a)
  \/ \E p \in Threads, c \in Children : C_ReadResult(p, c)
  \/ \E t \in Threads, c \in Children, m \in {"wait", "async"} : C_FollowUp(t, c, m)
  \/ \E q \in Children : C_RaiseQuestion(q) \/ C_PersonAnswer(q) \/ \E x \in Threads : C_AgentAnswer(q, x)
  \/ \E q \in Children : C_RaiseQuestionS(q) \/ C_PersonAnswerS(q) \/ \E x \in Threads : C_AgentAnswerS(q, x)
  \/ \E e \in D!Effects : C_DoQHop(e)
  \/ \E e \in D!NosEffects, how \in D!Hows : C_DoNotice(e, how) \/ C_DoOffer(e, how) \/ C_DoStop(e, how)
  \/ C_CrashEngine
  \/ \E d \in Devices : C_GoOffline(d) \/ C_GoOnline(d)
  \/ \E s \in Subs : \/ \E bg \in BOOLEAN : C_NsSpawn(s, bg)
                     \/ C_NsResume(s) \/ C_NsProgress(s)
                     \/ \E out \in NS!Outs : C_NsSubEnd(s, out)
  \/ C_NsSessionEnd \/ C_NsHandle \/ C_NsCrash \/ C_NsRecover
  \/ \E e \in NS!Events : C_NsReplay(e)
  \/ \E s, f \in Threads : C_CtFork(s, f)
  \/ \E f \in Threads : C_CtMergeBack(f) \/ C_CtSubagentResult(f)
  \/ \E t \in Threads : \/ \E x \in CT!XIds : C_CtResolve(t, x)
                        \/ C_CtDeliver(t)
                        \/ \E p \in Providers : C_CtRecoverDelivery(t, p) \/ C_CtFreeSend(t, p)
  \/ C_CtCrash
SpecCov == Init /\ [][StepCov \/ C_Done]_vars


Next == Step \/ C_Done

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Properties: module invariants that still apply *)

C_TypeOK ==
  /\ RL!RL_TypeOK /\ D!D_TypeOK /\ NS!TypeOK /\ CT!TypeOK
  /\ ctTrk \in [Threads -> BOOLEAN] /\ consumedBy \in [CT!XIds -> Kinds \cup {"none"}]
  /\ owed \subseteq Threads /\ attAcc \in [Threads -> D!Levels] /\ waitAtt \in [Threads -> 0..RLMaxAtt] /\ f9w \in [Threads -> 0..2]
  /\ host \in [Threads -> Devices \cup {NoDev}] /\ online \in [Devices -> BOOLEAN]

C_BoundsNotHit == RL!RL_BoundsNotHit /\ D!D_BoundsNotHit

(* RL's retry budget counts person restarts only; D3 clamp restarts add    *)
(* attempts, so the composed bound adds MaxNarrow (see FINDINGS).          *)
C_RetryBudget == \A t \in Threads : att[t] <= 1 + RLMaxPerson + RLMaxRetry + MaxNarrow

(* D safety, RL safety, NS safety, CT safety (ContextOrder) in one place. *)
D_Safety ==
  /\ D!S3_ExactlyOnce /\ D!S4_NoWakeIntoLiveRun /\ D!S5_DepthFanout /\ D!S6_AccessNarrow
  /\ D!S7_AckFromLiveRun /\ D!S10_ReplayIdempotent /\ D!S10a_ReceiptNoop /\ D!S11_NoOrphanRunning
  /\ D!D_NoStrandedResult /\ D!D_NoStaleWakeQueued /\ D!D_NoStaleQuestionWake
RL_Safety == RL!R14_StopDisarmsResume
NS_Safety == NS!OnlyRootEndsRun /\ NS!OrphansClosed /\ NS!NoStaleClose /\ NS!TaskMirror
             /\ NS!NoStuckTask /\ NS!NoFalseOrphan
CT_Safety == CT!ContextOrder
Q_Safety  == D!S12a_OneAgentAnswerer /\ D!S12a_FirstAnswerWins

(* Cross-module invariants *)

(* D3: a running attempt never uses wider access than now in force, unless *)
(* it is already ending (terminal emitted or interrupt requested) or its   *)
(* restart is owed (it takes no agent step then).                          *)
C_EnforcedAccess ==
  \A t \in Threads : (Live(t) /\ ~emitted[t] /\ ~intReq[t] /\ t \notin owed) => attAcc[t] <= access[t]

(* A wait is open only in a live run. *)
C_WaitInLiveRun == \A t \in Threads : waitOn[t] # NoThread => Live(t)

(* NS natives live inside NsT's runs: an open foreground native belongs to *)
(* the live run of NsT.                                                     *)
C_NativeInRun ==
  \A s \in Subs : (UseNS /\ ~nRec /\ nEst[s] = "running" /\ ~nEbg[s]) => Live(NsT) /\ nErun[s] = rord[NsT]

(* Seam not stuck: a current-attempt root terminal in flight can always be *)
(* ingested (ended or retried) once its host is up and NS has drained the  *)
(* events before it.                                                        *)
C_EndNotStuck ==
  \A t \in Threads : \A e \in pev[t] :
     (e.n = "root" /\ RL!CurEv(t, e) /\ On(t) /\ (IsNs(t) => (~nRec /\ CurRootEnd)))
       => ENABLED (C_IngestEnd(t, e) \/ C_RetryAttempt(t, e))
(* Seam not stuck: a startable queue head can start. *)
C_StartNotStuck ==
  \A t \in Threads :
     (D!CanStartD(t) /\ On(t) /\ (IsNs(t) => (~nRec /\ ~nProot)))
       => ENABLED (C_StartQueue(t) \/ C_StartWake(t))

(* Action properties *)

(* S2 over the shared run slot (RL!S2_OnlyRootEndsRun, all composed steps, *)
(* native ends included).                                                  *)
C_S2 ==
  [][\A t \in Threads :
       (rord'[t] = rord[t] /\ rord[t] \in liveSet[t] /\ rord[t] \notin liveSet'[t])
         => \E e \in pev[t] : e.n = "root" /\ e.r = rord[t] /\ e.a = att[t] /\ e \notin pev'[t]
    ]_vars

(* D6 never clears a stop hold: only a person send releases "stop". *)
C_StopHoldOnlyPerson ==
  [][\A t \in Threads : (held[t] = "stop" /\ held'[t] # "stop") => rPersonLeft' < rPersonLeft]_vars

(* S7 across attempts: a result reaches an agent only at a wake run start, *)
(* or inside a still-running attempt of the parent; a wait reply only to   *)
(* the attempt that issued the wait.                                        *)
C_NoPhantomAck ==
  [][\A c \in Children : pushes'[c] > pushes[c] =>
       LET p == parent[c] IN
       \/ ~Live(p) /\ Live(p)'
       \/ /\ Live(p) /\ ~emitted[p]
          /\ (waitOn[p] = c /\ waitOn'[p] = NoThread) => att[p] = waitAtt[p]
    ]_vars

(* A transfer is consumed only inside a real run start of its target. *)
C_ConsumeAtRunStart ==
  [][\A x \in CT!XIds : (xXf[x].st # "consumed" /\ xXf'[x].st = "consumed")
                          => (Live(xXf[x].tgt)' /\ ctTrk'[xXf[x].tgt])]_vars

(* Module action properties that still apply. *)
C_R7       == RL!R7_NoNewAttemptAfterStop
C_NoLostSteer == RL!RL_NoLostSteer
C_D6       == RL!D6_ReleaseAtReset
C_S11      == D!S11_NoDelegateAfterStop

(* S9 (abstract): a chat's host never changes once set. *)
C_HostFixed == [][\A t \in Threads : host[t] # NoDev => host'[t] = host[t]]_vars

-----------------------------------------------------------------------------
(* Witnesses (must be violated) *)

W_C_F1 == D!W_F1_GrandchildToRoot
(* F9 + wake: a usage-limit resume run started while a result wake was     *)
(* queued, and that wake was delivered after it.                            *)
W_C_F9Wake == ~\E t \in Threads : f9w[t] = 2
(* Stop during a usage limit: the cascade reached a child whose queue a   *)
(* usage limit held; the stop hold stays and nothing is armed.             *)
W_C_StopInLimit ==
  ~\E c \in Children : /\ ~Live(c) /\ lastOut[c] = "usage_limit" /\ held[c] = "stop"
                       /\ \E s \in D!StopIds : <<c, s>> \in rcpt
(* A context transfer consumed by a wake run. *)
W_C_CTWake == ~\E x \in CT!XIds : consumedBy[x] = "wake"
(* A native subagent running inside a delegated child's task run. *)
W_C_NativeInChild ==
  ~(UseNS /\ NsT \in Children /\ created[NsT] /\ Live(NsT) /\ rtask[NsT]
    /\ \E s \in Subs : nEst[s] = "running")
(* F11 (abstract): a child on another device delivered its result. *)
W_C_F11 ==
  ~\E c \in Children : created[c] /\ host[c] # host[parent[c]] /\ tDel[c] \in {"delivered", "acknowledged"}
(* D3: a clamp restarted a live attempt (with RLMaxPerson = 1 the one     *)
(* person send started the root run, so no person restart is left).        *)
W_C_ClampRestart ==
  ~\E t \in Threads : why[t] = "steering_restart" /\ Live(t) /\ narrowLeft < MaxNarrow /\ rPersonLeft = 0


-----------------------------------------------------------------------------
(* FINDINGS from composing (each forced a seam rule above; "mutant" names *)
(* the Mut switch whose run shows the rule is needed).                     *)
(*  C1 RL's start gate (~held) would block D's R12 wake through a "fail"   *)
(*     hold: the queue gate must be D's (RLh).                             *)
(*  C2 D has no usage-limit end and no "limit" hold: RL owns the hold at   *)
(*     run end; D contributes sweep / notice / catch-up only (Dx).         *)
(*  C3 Stop: D drops queued wakes, RL's RequestInterrupt pins the queue   *)
(*     (RLq).                                                              *)
(*  C4 D!RootComplete's guard (no open wait, no own question) must hold   *)
(*     when the provider EMITS completed, else the event sticks           *)
(*     (mutant C_completeWhileWaiting).                                    *)
(*  C5 Agent tool steps need the current attempt to be running            *)
(*     (~emitted); a wait reply after the terminal is a phantom ack        *)
(*     (mutant C_waitNoEmitGuard).                                         *)
(*  C6 A new attempt (retry, provider recovery, restart, clamp restart)    *)
(*     must abandon the old attempt's blocking wait (mutant                *)
(*     C_attemptKeepsWait).                                                *)
(*  C7 The stop cascade hop must disarm the child's usage-limit resume     *)
(*     (R14 per hop; mutant C_cascadeNoDisarm).                            *)
(*  C8 D3 clamp restarts add attempts: RL_RetryBudget must count them.     *)
(*  C9 RL sends need an existing chat (RL!PersonSend has no created       *)
(*     check; without it a never-created child ran and delegated to       *)
(*     itself).                                                            *)
(*  C10 NS ends a run on engine-crash recovery; RL turns a provider crash *)
(*     into a provider_recovery attempt of the same run. DESIGN must pick *)
(*     one. Here NS crashes only between runs.                             *)
(*  C11 NS has no attempts: what a retry / steering restart / clamp        *)
(*     restart does to running natives is unspecified. Here NsT never     *)
(*     retries or restarts.                                                *)
(*  C12 CT has no turn end except Complete (failed / interrupted /        *)
(*     usage-limit turns leave CT's run live); glue clears it.             *)
(*  C13 CT records lineage one level deep and only after a completed      *)
(*     assistant message: a first-run delegation and every grandchild get *)
(*     no subagent_spawn transfer.                                         *)
(*  C14 D's question run (StartQuestion) has no RL start, RL's resume run *)
(*     has no D start: both restated as glue.                              *)
(*                                                                         *)
(* DIFFS that would remove the glue (not applied; modules are not owned  *)
(* here):                                                                  *)
(*  RunLifecycle: StartBody(t, k) == the body of StartFromQueue minus     *)
(*    CanStartFromQueue and PopHead (use in StartFromQueue,               *)
(*    UsageLimitRecovery); RestartBody(t) == RestartActive minus the      *)
(*    person budget; EndHold(t, out) == the held' CASE of IngestEnd.      *)
(*  Delegation: EndRunCore(t, out) == EndRun minus its held' conjunct,    *)
(*    EndRun(t, out) == EndRunCore(t, out) /\ held' = ...; NarrowAccess   *)
(*    with UNCHANGED <<rtask>> instead of UNCHANGED runD (lets the        *)
(*    composition drop a restarted attempt's wait in the same step).      *)
(*  ContextTransfer: TurnEnd(t) == run[t].st = "live" /\ vol[t] = "none"  *)
(*    /\ run' = [run EXCEPT ![t] = NoRun] /\ UNCHANGED <<all else>>.       *)
(*  NativeSubagents: an attempt boundary event (AttemptEnd) that closes    *)
(*    foreground natives of the superseded attempt.                        *)

=============================================================================
