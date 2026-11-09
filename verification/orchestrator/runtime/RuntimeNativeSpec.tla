--------------------------- MODULE RuntimeNativeSpec ---------------------------
(***************************************************************************)
(* Zeron Orchestrator v2: harness-native subagents (Claude Task, Codex     *)
(* agents) inside one chat's runs.                                         *)
(*                                                                         *)
(* The harness (provider) emits events in order on one stream: spawn,      *)
(* progress, end per subagent, and rootEnd per run. The engine applies     *)
(* them to the subagent record (today: doc {chatId}--sub--{toolUseId}) and *)
(* to the unified task row (F10). A subagent is keyed by its toolUseId and *)
(* an activation epoch (Codex resume_agent / Claude SendMessage reopen).   *)
(*                                                                         *)
(* Foreground subagents belong to the root turn. Background subagents     *)
(* (Claude run_in_background, T3 pendingBackgroundTasks) may outlive the   *)
(* root turn; they close when they end or when the provider process ends.  *)
(*                                                                         *)
(* Environment: engine crash kills the provider process and loses events  *)
(* in flight (T3 ProviderRuntimeRecoveryService: "no provider process      *)
(* survives the server"). Replayed events (at-least-once journal replay)   *)
(* may arrive again, late. Interrupt may kill foreground subagents with no *)
(* end event (worst case).                                                 *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
  Subs, NoSub,          \* toolUseIds
  MaxRuns, MaxEp,       \* runs per chat, activations per subagent
  MaxCrash, MaxDup, MaxInterrupt, MaxSessEnd,
  MaxInbox,             \* events in flight (harness blocks when the pipe is full)
  Background,           \* harness may start background subagents
  SilentKill,           \* interrupt may kill foreground subagents without end events
  \* Mutants (AC-M6). All FALSE in the real model.
  M_AnyEndEndsRun,      \* handler ends the run on any end event (no root-node check)
  M_NoRunGuard,         \* rootEnd handler does not check the run ordinal
  M_NoOrphanClose,      \* run end does not close foreground subagents
  M_NoSessClose,        \* provider-process end does not close background subagents
  M_NoEpochGuard,       \* spawn/end applied without activation-epoch check
  M_ReopenTerminal,     \* a late progress event reopens a closed subagent
  M_NoTaskMirror,       \* orphan close writes the subagent doc but not the task row
  M_NoRecoveryClose,    \* crash recovery does not close open subagents
  M_OrphanCloseBg       \* run end also closes background subagents (DESIGN.md N3 as written)

ASSUME NoSub \notin Subs

Active   == "running"
Terminal == {"completed", "failed", "interrupted", "cancelled"}
SubSt    == {"none", Active} \cup Terminal
Outs     == {"completed", "failed"}

Ev(k, s, ep, r, bg, out) == [k |-> k, s |-> s, ep |-> ep, r |-> r, bg |-> bg, out |-> out]
Events == {Ev(k, s, ep, r, bg, out) :
             k \in {"spawn", "progress", "end", "rootEnd", "sessEnd"},
             s \in Subs \cup {NoSub}, ep \in 0..MaxEp, r \in 0..MaxRuns,
             bg \in BOOLEAN, out \in Outs \cup {"interrupted", "cancelled", "none"}}

VARIABLES
  \* engine
  rst, rord, endBy, recovering,
  est, eep, erun, ebg, how, closedEp, task,
  \* provider (harness) truth
  sess, proot, pst, pep, prun, pbg, pprog,
  \* event stream
  inbox, replay,
  \* budgets
  crashLeft, dupLeft, intLeft, sessLeft

engv  == <<rst, rord, endBy, recovering>>
subv  == <<est, eep, erun, ebg, how, closedEp, task>>
provv == <<sess, proot, pst, pep, prun, pbg, pprog>>
budv  == <<crashLeft, dupLeft, intLeft, sessLeft>>
vars  == <<engv, subv, provv, inbox, replay, budv>>

-----------------------------------------------------------------------------
TypeOK ==
  /\ rst \in {"idle", "live"} /\ rord \in 0..MaxRuns
  /\ endBy \in {"none", "root", "recovery", "subagent", "stale_root"}
  /\ recovering \in BOOLEAN
  /\ est \in [Subs -> SubSt] /\ task \in [Subs -> SubSt]
  /\ eep \in [Subs -> 0..MaxEp] /\ erun \in [Subs -> 0..MaxRuns] /\ ebg \in [Subs -> BOOLEAN]
  /\ how \in [Subs -> {"none", "event", "late", "orphan", "session", "recovery"}]
  /\ closedEp \in [Subs -> 0..MaxEp]
  /\ sess \in BOOLEAN /\ proot \in BOOLEAN
  /\ pst \in [Subs -> {"none", "running", "done", "dead"}]
  /\ pep \in [Subs -> 0..MaxEp] /\ prun \in [Subs -> 0..MaxRuns] /\ pbg \in [Subs -> BOOLEAN]
  /\ pprog \in [Subs -> BOOLEAN]
  /\ inbox \in Seq(Events) /\ replay \subseteq Events

Init ==
  /\ rst = "idle" /\ rord = 0 /\ endBy = "none" /\ recovering = FALSE
  /\ est = [s \in Subs |-> "none"] /\ task = [s \in Subs |-> "none"]
  /\ eep = [s \in Subs |-> 0] /\ erun = [s \in Subs |-> 0] /\ ebg = [s \in Subs |-> FALSE]
  /\ how = [s \in Subs |-> "none"] /\ closedEp = [s \in Subs |-> 0]
  /\ sess = FALSE /\ proot = FALSE
  /\ pst = [s \in Subs |-> "none"] /\ pep = [s \in Subs |-> 0]
  /\ prun = [s \in Subs |-> 0] /\ pbg = [s \in Subs |-> FALSE] /\ pprog = [s \in Subs |-> FALSE]
  /\ inbox = <<>> /\ replay = {}
  /\ crashLeft = MaxCrash /\ dupLeft = MaxDup /\ intLeft = MaxInterrupt /\ sessLeft = MaxSessEnd

Room == Len(inbox) < MaxInbox
Emit(e) == inbox' = Append(inbox, e)
\* One end event per killed subagent, in a fixed order.
RECURSIVE KillEnds(_)
KillEnds(S) == IF S = {} THEN <<>>
               ELSE LET x == CHOOSE x \in S : TRUE
                    IN <<Ev("end", x, pep[x], prun[x], FALSE, "interrupted")>> \o KillEnds(S \ {x})
ProvRunning(s) == pst[s] = "running"
FgRunning == {s \in Subs : ProvRunning(s) /\ ~pbg[s]}

-----------------------------------------------------------------------------
(* User / engine starts a run: the harness starts the root turn.           *)
StartRun ==
  /\ ~recovering /\ rst = "idle" /\ rord < MaxRuns /\ ~proot
  /\ rst' = "live" /\ rord' = rord + 1 /\ endBy' = "none"
  /\ sess' = TRUE /\ proot' = TRUE
  /\ UNCHANGED <<recovering, subv, pst, pep, prun, pbg, pprog, inbox, replay, budv>>

(* Harness: the root turn spawns a fresh subagent.                         *)
PSpawn(s, bg) ==
  /\ Room /\ proot /\ pst[s] = "none" /\ (bg => Background)
  /\ pst' = [pst EXCEPT ![s] = "running"] /\ pep' = [pep EXCEPT ![s] = 1]
  /\ prun' = [prun EXCEPT ![s] = rord] /\ pbg' = [pbg EXCEPT ![s] = bg]
  /\ Emit(Ev("spawn", s, 1, rord, bg, "none"))
  /\ UNCHANGED <<engv, subv, sess, proot, pprog, replay, budv>>

(* Harness: a later root turn resumes an ended subagent (new activation). *)
PResume(s) ==
  /\ Room /\ proot /\ pst[s] = "done" /\ pep[s] < MaxEp
  /\ pst' = [pst EXCEPT ![s] = "running"] /\ pep' = [pep EXCEPT ![s] = @ + 1]
  /\ prun' = [prun EXCEPT ![s] = rord] /\ pbg' = [pbg EXCEPT ![s] = FALSE]
  /\ pprog' = [pprog EXCEPT ![s] = FALSE]
  /\ Emit(Ev("spawn", s, pep[s] + 1, rord, FALSE, "none"))
  /\ UNCHANGED <<engv, subv, sess, proot, replay, budv>>

PProgress(s) ==
  /\ Room /\ ProvRunning(s) /\ ~pprog[s]
  /\ pprog' = [pprog EXCEPT ![s] = TRUE]
  /\ Emit(Ev("progress", s, pep[s], prun[s], pbg[s], "none"))
  /\ UNCHANGED <<engv, subv, sess, proot, pst, pep, prun, pbg, replay, budv>>

(* Harness: a subagent ends. Foreground ones end inside their root turn;   *)
(* background ones may end after it, while the process lives.             *)
PSubEnd(s, out) ==
  /\ Room /\ ProvRunning(s) /\ (pbg[s] \/ proot)
  /\ pst' = [pst EXCEPT ![s] = "done"]
  /\ Emit(Ev("end", s, pep[s], prun[s], pbg[s], out))
  /\ UNCHANGED <<engv, subv, sess, proot, pep, prun, pbg, pprog, replay, budv>>

(* Harness: the root turn ends. A root turn waits for its foreground tools. *)
PRootEnd ==
  /\ Room /\ proot /\ FgRunning = {}
  /\ proot' = FALSE
  /\ Emit(Ev("rootEnd", NoSub, 0, rord, FALSE, "completed"))
  /\ UNCHANGED <<engv, subv, sess, pst, pep, prun, pbg, pprog, replay, budv>>

(* Interrupt request confirmed by the harness: it kills foreground         *)
(* subagents (with or without end events) and ends the root turn.          *)
PInterrupt(silent) ==
  /\ proot /\ intLeft > 0 /\ (silent => SilentKill)
  /\ LET killed == FgRunning
     IN /\ pst' = [s \in Subs |-> IF s \in killed THEN "dead" ELSE pst[s]]
        /\ inbox' = inbox \o (IF silent THEN <<>> ELSE KillEnds(killed))
                          \o <<Ev("rootEnd", NoSub, 0, rord, FALSE, "interrupted")>>
  /\ proot' = FALSE /\ intLeft' = intLeft - 1
  /\ UNCHANGED <<engv, subv, sess, pep, prun, pbg, pprog, replay, crashLeft, dupLeft, sessLeft>>

(* The provider process exits between runs (chat closed, idle reaper).     *)
(* It cannot report; the engine observes the exit as sessEnd.              *)
PSessionEnd ==
  /\ Room /\ sess /\ ~proot /\ sessLeft > 0
  /\ sess' = FALSE /\ sessLeft' = sessLeft - 1
  /\ pst' = [s \in Subs |-> IF pst[s] = "running" THEN "dead" ELSE pst[s]]
  /\ Emit(Ev("sessEnd", NoSub, 0, rord, FALSE, "none"))
  /\ UNCHANGED <<engv, subv, proot, pep, prun, pbg, pprog, replay, crashLeft, dupLeft, intLeft>>

(* Engine crash: the provider process dies with it; events in flight are  *)
(* lost. Recovery runs before anything else.                               *)
Crash ==
  /\ crashLeft > 0 /\ ~recovering
  /\ crashLeft' = crashLeft - 1 /\ recovering' = TRUE
  /\ sess' = FALSE /\ proot' = FALSE
  /\ pst' = [s \in Subs |-> IF pst[s] = "running" THEN "dead" ELSE pst[s]]
  /\ inbox' = <<>>
  /\ UNCHANGED <<rst, rord, endBy, subv, pep, prun, pbg, pprog, replay, dupLeft, intLeft, sessLeft>>

(* At-least-once replay: an event already applied arrives again, late.     *)
Replay(e) ==
  /\ Room /\ dupLeft > 0 /\ e \in replay /\ ~recovering
  /\ Emit(e) /\ dupLeft' = dupLeft - 1
  /\ replay' = IF dupLeft = 1 THEN {} ELSE replay
  /\ UNCHANGED <<engv, subv, provv, crashLeft, intLeft, sessLeft>>

-----------------------------------------------------------------------------
(* Engine reducer: apply the head event.                                   *)
CloseSet(S, st, h) ==
  /\ est' = [s \in Subs |-> IF s \in S THEN st ELSE est[s]]
  /\ how' = [s \in Subs |-> IF s \in S THEN h ELSE how[s]]
  /\ closedEp' = [s \in Subs |-> IF s \in S THEN eep[s] ELSE closedEp[s]]
  /\ task' = [s \in Subs |-> IF s \in S /\ ~(M_NoTaskMirror /\ h = "orphan") THEN st ELSE task[s]]

Handle ==
  /\ ~recovering /\ inbox # <<>>
  /\ LET e == Head(inbox) IN
     /\ inbox' = Tail(inbox)
     /\ replay' = IF dupLeft > 0 /\ e.k # "sessEnd" THEN replay \cup {e} ELSE {}
     /\ CASE e.k = "spawn" ->
              IF e.ep > eep[e.s] \/ (M_NoEpochGuard /\ e.ep = eep[e.s])
              THEN /\ est' = [est EXCEPT ![e.s] = Active] /\ task' = [task EXCEPT ![e.s] = Active]
                   /\ eep' = [eep EXCEPT ![e.s] = e.ep] /\ erun' = [erun EXCEPT ![e.s] = e.r]
                   /\ ebg' = [ebg EXCEPT ![e.s] = e.bg]
                   /\ how' = [how EXCEPT ![e.s] = "none"]
                   /\ UNCHANGED <<closedEp, rst, rord, endBy>>
              ELSE UNCHANGED <<subv, rst, rord, endBy>>
          [] e.k = "progress" ->
              IF M_ReopenTerminal /\ est[e.s] \in Terminal
              THEN /\ est' = [est EXCEPT ![e.s] = Active] /\ task' = [task EXCEPT ![e.s] = Active]
                   /\ UNCHANGED <<eep, erun, ebg, how, closedEp, rst, rord, endBy>>
              ELSE UNCHANGED <<subv, rst, rord, endBy>>
          [] e.k = "end" ->
              /\ IF est[e.s] = Active /\ (e.ep = eep[e.s] \/ M_NoEpochGuard)
                 THEN /\ est' = [est EXCEPT ![e.s] = e.out] /\ task' = [task EXCEPT ![e.s] = e.out]
                      /\ how' = [how EXCEPT ![e.s] = IF rst = "live" /\ erun[e.s] = rord THEN "event" ELSE "late"]
                      /\ closedEp' = [closedEp EXCEPT ![e.s] = e.ep]
                      /\ UNCHANGED <<eep, erun, ebg>>
                 ELSE UNCHANGED subv
              /\ IF M_AnyEndEndsRun /\ rst = "live"
                 THEN rst' = "idle" /\ endBy' = "subagent" /\ UNCHANGED rord
                 ELSE UNCHANGED <<rst, rord, endBy>>
          [] e.k = "rootEnd" ->
              IF rst = "live" /\ (e.r = rord \/ M_NoRunGuard)
              THEN /\ rst' = "idle"
                   /\ endBy' = IF e.r = rord THEN "root" ELSE "stale_root"
                   /\ UNCHANGED rord
                   /\ LET orphans == IF M_NoOrphanClose THEN {}
                                     ELSE {s \in Subs : est[s] = Active /\ (~ebg[s] \/ M_OrphanCloseBg) /\ erun[s] = rord}
                      IN /\ CloseSet(orphans, "interrupted", "orphan")
                         /\ UNCHANGED <<eep, erun, ebg>>
              ELSE UNCHANGED <<subv, rst, rord, endBy>>
          [] e.k = "sessEnd" ->
              /\ LET open == IF M_NoSessClose THEN {} ELSE {s \in Subs : est[s] = Active}
                 IN /\ CloseSet(open, "cancelled", "session")
                    /\ UNCHANGED <<eep, erun, ebg>>
              /\ UNCHANGED <<rst, rord, endBy>>
  /\ UNCHANGED <<recovering, provv, budv>>

(* Recovery after a crash: the run ends (interrupted) and every open       *)
(* subagent is cancelled, since no provider process survived.              *)
Recover ==
  /\ recovering
  /\ recovering' = FALSE
  /\ rst' = "idle"
  /\ endBy' = IF rst = "live" THEN "recovery" ELSE endBy
  /\ LET open == IF M_NoRecoveryClose THEN {} ELSE {s \in Subs : est[s] = Active}
     IN CloseSet(open, "cancelled", "recovery")
  /\ replay' = {}
  /\ UNCHANGED <<rord, eep, erun, ebg, provv, inbox, budv>>

(* Documented terminal state (AC-M7): every run used, nothing in flight,   *)
(* no provider work left.                                                  *)
Quiescent ==
  /\ ~recovering /\ inbox = <<>> /\ rst = "idle" /\ ~proot
  /\ rord = MaxRuns
  /\ \A s \in Subs : pst[s] # "running"
Terminated == Quiescent /\ UNCHANGED vars

Env ==
  \/ StartRun
  \/ \E s \in Subs, bg \in BOOLEAN : PSpawn(s, bg)
  \/ \E s \in Subs : PResume(s) \/ PProgress(s)
  \/ \E s \in Subs, out \in Outs : PSubEnd(s, out)
  \/ PRootEnd
  \/ \E b \in BOOLEAN : PInterrupt(b)
  \/ PSessionEnd
  \/ Crash
  \/ \E e \in replay : Replay(e)

Next == Env \/ Handle \/ Recover \/ Terminated

Spec     == Init /\ [][Next]_vars
FairSpec == Spec /\ WF_vars(Handle) /\ WF_vars(Recover)

-----------------------------------------------------------------------------
(* S2: only the run's own root node ends the run (or crash recovery).      *)
OnlyRootEndsRun == endBy \in {"none", "root", "recovery"}

(* S13a: a native subagent ending never ends the parent run: covered by S2 *)
(* (endBy = "subagent" is the violation).                                  *)
(* S13b: orphans close. Outside recovery, an open subagent belongs to the  *)
(* live run, or is a background subagent whose provider process lives    *)
(* (or whose process end is still in flight).                              *)
SessEndPending == \E i \in 1..Len(inbox) : inbox[i].k = "sessEnd"
OrphansClosed ==
  ~recovering =>
    \A s \in Subs : est[s] = Active =>
      \/ rst = "live" /\ erun[s] = rord
      \/ ebg[s] /\ (sess \/ SessEndPending)

(* Closed by its own event => closed by this activation's event.           *)
NoStaleClose == \A s \in Subs : (est[s] \in Terminal /\ how[s] \in {"event", "late"}) => closedEp[s] = eep[s]

(* Orphan or session close never closes work the harness is still doing   *)
(* (it would hide a result that is still coming).                          *)
NoFalseOrphan == \A s \in Subs : (how[s] \in {"orphan", "session"} /\ pep[s] = eep[s]) => ~ProvRunning(s)

(* F10: the native subagent is a task in the unified task model; the task  *)
(* row shows the same status as the subagent record.                       *)
TaskMirror == \A s \in Subs : task[s] = est[s]

(* Engine never shows a subagent running when the provider has it stopped *)
(* and nothing in flight will tell the engine (no stuck task).             *)
Mentions(s) == \E i \in 1..Len(inbox) : inbox[i].s = s \/ inbox[i].k \in {"rootEnd", "sessEnd"}
NoStuckTask ==
  ~recovering =>
    \A s \in Subs : ~(est[s] = Active /\ ~(ProvRunning(s) /\ pep[s] = eep[s]) /\ ~Mentions(s))

(* Liveness: an open subagent whose provider work stopped is closed.       *)
OrphanEventuallyClosed ==
  \A s \in Subs : (est[s] = Active /\ ~ProvRunning(s)) ~> (est[s] # Active \/ ProvRunning(s))

-----------------------------------------------------------------------------
(* Witnesses (must be VIOLATED).                                           *)
W_F10_TaskCompleted == ~(\E s \in Subs : task[s] = "completed" /\ how[s] = "event")
(* Finding R10: a background result that arrives with no live parent run  *)
(* has no inline delivery target (DESIGN.md section 6 "Delivery").         *)
W_BgResultNoRun     == ~(\E s \in Subs : ebg[s] /\ task[s] = "completed" /\ how[s] = "late")
W_BgOutlivesRun     == ~(\E s \in Subs : ebg[s] /\ task[s] = Active /\ rst = "idle" /\ ~recovering)
W_OrphanClosed      == ~(\E s \in Subs : how[s] = "orphan")
W_ResumedCompleted  == ~(\E s \in Subs : eep[s] = 2 /\ task[s] = "completed")
W_SessionClosed     == ~(\E s \in Subs : how[s] = "session")
W_RecoveryClosed    == ~(\E s \in Subs : how[s] = "recovery")

(* Coverage (AC-M5): each must be VIOLATED (action enabled somewhere).     *)
Cov_StartRun    == ~ENABLED StartRun
Cov_PSpawn      == ~(\E s \in Subs, bg \in BOOLEAN : ENABLED PSpawn(s, bg))
Cov_PSpawnBg    == ~(\E s \in Subs : ENABLED PSpawn(s, TRUE))
Cov_PResume     == ~(\E s \in Subs : ENABLED PResume(s))
Cov_PProgress   == ~(\E s \in Subs : ENABLED PProgress(s))
Cov_PSubEnd     == ~(\E s \in Subs, o \in Outs : ENABLED PSubEnd(s, o))
Cov_PRootEnd    == ~ENABLED PRootEnd
Cov_PInterrupt  == ~(\E b \in BOOLEAN : ENABLED PInterrupt(b))
Cov_PSessionEnd == ~ENABLED PSessionEnd
Cov_Crash       == ~ENABLED Crash
Cov_Replay      == ~(\E e \in replay : ENABLED Replay(e))
Cov_Handle      == ~ENABLED Handle
Cov_Recover     == ~ENABLED Recover
Cov_Terminated  == ~ENABLED Terminated

(* DESIGN.md names: N1 NativeStart = Handle(spawn); N2 NativeEnd =        *)
(* Handle(end); N3 NativeOrphan = Handle(rootEnd) orphan close.            *)
NativeNeverEndsRun == OnlyRootEndsRun
NativeInTaskList   == TaskMirror

Sym == Permutations(Subs)

(* R41: an attempt boundary keeps the root run ordinal and background work.
   It closes foreground records of the superseded attempt. Provider events
   for that old root cannot close the next attempt. *)
PAttemptFail ==
  /\ Room /\ proot
  /\ pst'=[s \in Subs |-> IF s \in FgRunning THEN "dead" ELSE pst[s]]
  /\ proot'=FALSE
  /\ Emit(Ev("rootEnd",NoSub,0,rord,FALSE,"interrupted"))
  /\ UNCHANGED <<engv,subv,sess,pep,prun,pbg,pprog,replay,budv>>

PObservedEnd(out) ==
 /\ Room /\ proot /\ out \in {"completed","failed","interrupted"}
 /\ (out="completed" => FgRunning={})
 /\ (out="interrupted" => intLeft>0)
 /\ pst'=[s \in Subs |-> IF out#"completed" /\ s \in FgRunning THEN "dead" ELSE pst[s]]
 /\ proot'=FALSE
 /\ intLeft'=IF out="interrupted" THEN intLeft-1 ELSE intLeft
 /\ Emit(Ev("rootEnd",NoSub,0,rord,FALSE,out))
 /\ UNCHANGED <<engv,subv,sess,pep,prun,pbg,pprog,replay,crashLeft,dupLeft,sessLeft>>

LogicalStart ==
  /\ ~recovering /\ rst="idle" /\ ~proot
  /\ rst'="live" /\ rord'=rord+1 /\ endBy'="none"
  /\ UNCHANGED <<recovering,subv,provv,inbox,replay,budv>>

ObserveStart ==
  /\ ~recovering /\ rst="live" /\ ~proot
  /\ sess'=TRUE /\ proot'=TRUE
  /\ UNCHANGED <<engv,subv,pst,pep,prun,pbg,pprog,inbox,replay,budv>>

AttemptBoundary ==
  /\ ~recovering /\ rst="live"
  /\ LET fg == {s \in Subs : est[s]=Active /\ ~ebg[s]} IN
       /\ CloseSet(fg,"interrupted","orphan")
  /\ proot'=FALSE
  /\ UNCHANGED <<sess,pst>>
  /\ inbox'=SelectSeq(inbox,LAMBDA e: ~(e.k="rootEnd" /\ e.r=rord))
  /\ UNCHANGED <<rst,rord,endBy,recovering,eep,erun,ebg,pep,prun,pbg,pprog,replay,budv>>

=============================================================================
