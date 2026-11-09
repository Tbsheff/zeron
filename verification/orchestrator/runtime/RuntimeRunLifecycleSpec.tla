---------------------------- MODULE RuntimeRunLifecycleSpec ----------------------------
(***************************************************************************)
(* Zeron Orchestrator v2: run lifecycle of one thread (no delegation).      *)
(*                                                                         *)
(* Per-thread queue (person, server wake/follow-up), held queue with a      *)
(* reason, dispatch modes (steer_active, restart_active,                    *)
(* queue_after_active, start_immediately), run attempts (initial,           *)
(* steering_restart, retry, provider_recovery), interrupt as a request      *)
(* plus provider confirmation (R7), usage-limit failure and auto-resume    *)
(* (F9), retries with a budget.                                             *)
(*                                                                         *)
(* Provider events are asynchronous: the provider emits an event for an    *)
(* attempt into pev; the engine ingests it later. Events carry (run,       *)
(* attempt), so an event of a superseded attempt or an ended run is stale. *)
(*                                                                         *)
(* T3 ground truth: Orchestrator.ts:5834-5932 (dispatch-mode resolution),  *)
(* :5449-5479 (steering_restart supersedes the attempt), :6115 (client     *)
(* sends release held runs; provider failure and usage limits re-hold),    *)
(* UsageLimitRecoveryWorker.ts:1-80 (arm per run, resume with a fixed id,  *)
(* start_immediately, created by server). T3 declares the "retry" and      *)
(* "provider_recovery" attempt reasons but never creates them; here they   *)
(* are the Zeron design (same run, new attempt, shared retry budget).      *)
(***************************************************************************)
EXTENDS RuntimeSpecRunBase

CONSTANTS
  MaxPerson,       \* person sends (key dimension N for AC-M3)
  MaxServer,       \* server-created queued messages (wake, follow-up)
  MaxRetry,        \* retry budget per run (transient failure, provider crash)
  MaxStop,         \* Stop presses
  MaxUL,           \* usage-limit failures
  MaxCrash,        \* provider process crashes
  MaxAtt,          \* attempt ordinal bound (saturates; BoundsNotHit)
  FixLimitRelease  \* R11/D6: the usage-limit reset releases a "limit" hold (FALSE = T3: never)

Modes    == {"steer", "restart", "queue", "now"}
Outcomes == {"completed", "failed", "interrupted", "usage_limit", "transient", "crash"}
Reasons  == {"initial", "steering_restart", "retry", "provider_recovery"}
Ends     == {"none", "completed", "failed", "interrupted", "usage_limit"}
Atts     == 0..MaxAtt
Ev(n, r, a, o) == [n |-> n, r |-> r, a |-> a, o |-> o]
Events == [n : {"root", "child"}, r : 0..MaxRuns, a : Atts, o : Outcomes]

VARIABLES
  att,       \* current attempt ordinal of the current run
  why,       \* reason of the current attempt
  emitted,   \* the current attempt's root turn has emitted its terminal event
  kidOpen,   \* a non-root node (tool call, native subagent) is open
  pev,       \* provider events in flight (set)
  retries,   \* retries used by the current run
  steers,    \* steer messages attached to the current attempt
  lastOut,   \* outcome of the latest ended run
  rkind,     \* message kind that started the latest run
  ulArmed,   \* run ordinal armed for usage-limit resume (0 = none)
  ulDue,     \* the reset time of the armed limit has passed
  personLeft, serverLeft, stopLeft, ulLeft, crashLeft

rl     == <<att, why, emitted, kidOpen, pev, retries, steers, lastOut, rkind, ulArmed, ulDue>>
rlBud  == <<personLeft, serverLeft, stopLeft, ulLeft, crashLeft>>
rlVars == <<base, rl, rlBud>>

CurEv(t, e) == Live(t) /\ e.r = rord[t] /\ e.a = att[t]
Sat(n)      == Min(n, MaxAtt)

-----------------------------------------------------------------------------
RL_Init ==
  /\ BaseInit
  /\ att = [t \in Threads |-> 0]
  /\ why = [t \in Threads |-> "initial"]
  /\ emitted = [t \in Threads |-> FALSE]
  /\ kidOpen = [t \in Threads |-> FALSE]
  /\ pev = [t \in Threads |-> {}]
  /\ retries = [t \in Threads |-> 0]
  /\ steers = [t \in Threads |-> 0]
  /\ lastOut = [t \in Threads |-> "none"]
  /\ rkind = [t \in Threads |-> "none"]
  /\ ulArmed = [t \in Threads |-> 0]
  /\ ulDue = [t \in Threads |-> FALSE]
  /\ personLeft = MaxPerson /\ serverLeft = MaxServer /\ stopLeft = MaxStop
  /\ ulLeft = MaxUL /\ crashLeft = MaxCrash

(* A fresh attempt of the current run. *)
NewAttempt(t, a, reason) ==
  /\ att'     = [att EXCEPT ![t] = a]
  /\ why'     = [why EXCEPT ![t] = reason]
  /\ emitted' = [emitted EXCEPT ![t] = FALSE]
  /\ kidOpen' = [kidOpen EXCEPT ![t] = FALSE]

-----------------------------------------------------------------------------
(* Person (environment, no fairness) *)

(* Too late to steer: the attempt already emitted its terminal (T3         *)
(* Orchestrator.ts:5842 turns steer into start_immediately). Mutant         *)
(* "Steer_tooLate" removes this check.                                      *)
TooLate(t) == ~M("Steer_tooLate") /\ emitted[t]

(* Message lands in the queue: no active run (any mode), queue or now       *)
(* (start_immediately behind an active run queues, :6110), or a steer or   *)
(* restart that is too late or comes after Stop reached the run (auto      *)
(* intent, :5857). A client send releases the hold (:6117).                 *)
PersonSend(t, mode) ==
  /\ personLeft > 0 /\ Len(queue[t]) < MaxQueue
  /\ \/ ~Live(t)
     \/ mode \in {"queue", "now"}
     \/ mode \in {"steer", "restart"} /\ (intReq[t] \/ TooLate(t))
  /\ queue' = [queue EXCEPT ![t] = Append(@, Msg("person", 0, 0))]
  /\ held'  = [held EXCEPT ![t] = IF M("L3_heldNeverCleared") THEN @ ELSE "none"]
  /\ personLeft' = personLeft - 1
  /\ UNCHANGED <<liveSet, rord, intReq, rl, serverLeft, stopLeft, ulLeft, crashLeft>>

(* Native steer: the message joins the active attempt. *)
Steer(t) ==
  /\ personLeft > 0 /\ Live(t) /\ ~intReq[t] /\ ~TooLate(t)
  /\ steers' = [steers EXCEPT ![t] = Min(@ + 1, 2)]
  /\ personLeft' = personLeft - 1
  /\ UNCHANGED <<base, att, why, emitted, kidOpen, pev, retries, lastOut, rkind, ulArmed, ulDue,
                 serverLeft, stopLeft, ulLeft, crashLeft>>

(* restart_active, or steer_active on a provider without native steering:   *)
(* supersede the attempt and start a steering_restart attempt in the same  *)
(* run (:5449). The superseded turn later reports "interrupted" (stale).    *)
RestartActive(t, mode) ==
  /\ mode \in {"steer", "restart"}
  /\ personLeft > 0 /\ Live(t) /\ ~intReq[t] /\ ~TooLate(t)
  /\ pev' = [pev EXCEPT ![t] = IF emitted[t] THEN @
                                ELSE @ \cup {Ev("root", rord[t], att[t], "interrupted")}]
  /\ NewAttempt(t, Sat(att[t] + 1), "steering_restart")
  /\ steers' = [steers EXCEPT ![t] = 0]
  /\ personLeft' = personLeft - 1
  /\ UNCHANGED <<base, retries, lastOut, rkind, ulArmed, ulDue,
                 serverLeft, stopLeft, ulLeft, crashLeft>>

(* R14 (new): Stop disarms a pending usage-limit resume. Mutant           *)
(* "R14_stopDisarm" keeps it armed (T3: limitRecoveryCommand ignores holds).*)
StopWinsOverLimit == ~M("R14_stopDisarm")
RequestInterrupt(t) ==
  /\ stopLeft > 0
  /\ MarkInterrupt(t)
  /\ ulArmed' = [ulArmed EXCEPT ![t] = IF StopWinsOverLimit THEN 0 ELSE @]
  /\ ulDue'   = [ulDue EXCEPT ![t] = IF StopWinsOverLimit THEN FALSE ELSE @]
  /\ stopLeft' = stopLeft - 1
  /\ UNCHANGED <<queue, liveSet, rord, att, why, emitted, kidOpen, pev, retries, steers, lastOut, rkind,
                 personLeft, serverLeft, ulLeft, crashLeft>>

(* Server-created message (delegation wake, scheduled follow-up):           *)
(* queue_after_active only, and it does not release a hold.                *)
ServerEnqueue(t) ==
  /\ serverLeft > 0 /\ Len(queue[t]) < MaxQueue
  /\ queue' = [queue EXCEPT ![t] = Append(@, Msg("server", 0, 0))]
  /\ serverLeft' = serverLeft - 1
  /\ UNCHANGED <<held, liveSet, rord, intReq, rl, personLeft, stopLeft, ulLeft, crashLeft>>

-----------------------------------------------------------------------------
(* Provider (environment; weak fairness on emitting some root terminal) *)

ProviderTerminal(t, o) ==
  /\ Live(t) /\ ~emitted[t]
  /\ o = "interrupted" => intReq[t]       \* provider interrupts only on request
  /\ o = "usage_limit" => ulLeft > 0
  /\ o = "crash" => crashLeft > 0
  /\ emitted' = [emitted EXCEPT ![t] = TRUE]
  /\ pev' = [pev EXCEPT ![t] = @ \cup {Ev("root", rord[t], att[t], o)}]
  /\ ulLeft' = IF o = "usage_limit" THEN ulLeft - 1 ELSE ulLeft
  /\ crashLeft' = IF o = "crash" THEN crashLeft - 1 ELSE crashLeft
  /\ UNCHANGED <<base, att, why, kidOpen, retries, steers, lastOut, rkind, ulArmed, ulDue,
                 personLeft, serverLeft, stopLeft>>

(* A non-root node (tool call, native subagent) opens and later ends. *)
ChildStart(t) ==
  /\ Live(t) /\ ~emitted[t] /\ ~kidOpen[t]
  /\ kidOpen' = [kidOpen EXCEPT ![t] = TRUE]
  /\ UNCHANGED <<base, att, why, emitted, pev, retries, steers, lastOut, rkind, ulArmed, ulDue, rlBud>>

ChildEnd(t) ==
  /\ kidOpen[t]
  /\ kidOpen' = [kidOpen EXCEPT ![t] = FALSE]
  /\ pev' = [pev EXCEPT ![t] = @ \cup {Ev("child", rord[t], att[t], "completed")}]
  /\ UNCHANGED <<base, att, why, emitted, retries, steers, lastOut, rkind, ulArmed, ulDue, rlBud>>

(* Time passes the armed usage-limit reset. *)
ResetPasses(t) ==
  /\ ulArmed[t] # 0 /\ ~ulDue[t]
  /\ ulDue' = [ulDue EXCEPT ![t] = TRUE]
  /\ UNCHANGED <<base, att, why, emitted, kidOpen, pev, retries, steers, lastOut, rkind, ulArmed, rlBud>>

-----------------------------------------------------------------------------
(* Engine (weak fairness) *)

StartFromQueue(t) ==
  /\ CanStartFromQueue(t)
  /\ BeginRun(t) /\ PopHead(t)
  /\ NewAttempt(t, 1, "initial")
  /\ retries' = [retries EXCEPT ![t] = 0]
  /\ steers'  = [steers EXCEPT ![t] = 0]
  /\ rkind'   = [rkind EXCEPT ![t] = Head(queue[t]).k]
  \* a newer run supersedes the armed usage-limit run (T3: latestRunId changes)
  /\ ulArmed' = [ulArmed EXCEPT ![t] = 0]
  /\ ulDue'   = [ulDue EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<held, pev, lastOut, rlBud>>

(* The S2 guard: only the current attempt's ROOT node ends the run.         *)
(* Mutant "S2_child" lets a child node end it; "S2_stale" drops the        *)
(* (run, attempt) identity check.                                           *)
EndsRun(t, e) ==
  /\ M("S2_child") \/ e.n = "root"
  /\ M("S2_stale") \/ CurEv(t, e)
  /\ Live(t)

Retryable(o) == o \in {"transient", "crash"}
(* Retry guard: budget left (mutant "L3_retryBudget") and no Stop          *)
(* (mutant "R7_retryAfterStop").                                            *)
MayRetry(t) ==
  /\ M("L3_retryBudget") \/ retries[t] < MaxRetry
  /\ M("R7_retryAfterStop") \/ ~intReq[t]

(* Root terminal ends the run. A failure re-holds the queue; a usage limit *)
(* holds it with reason "limit" and arms the resume for this run.           *)
IngestEnd(t, e) ==
  LET out == CASE e.o = "completed"   -> "completed"
               [] e.o = "interrupted" -> "interrupted"
               [] e.o = "usage_limit" -> "usage_limit"
               [] OTHER               -> "failed"
  IN
  /\ e \in pev[t] /\ EndsRun(t, e)
  /\ ~(Retryable(e.o) /\ MayRetry(t))
  /\ pev' = [pev EXCEPT ![t] = @ \ {e}]
  /\ FinishRun(t)
  /\ held' = [held EXCEPT ![t] =
                CASE out = "failed" /\ @ = "stop" -> "stop"
                  [] out = "failed"      -> "fail"
                  [] out = "usage_limit" /\ StopWinsOverLimit /\ @ = "stop" -> "stop"
                  [] out = "usage_limit" -> "limit"
                  [] OTHER               -> @]
  /\ lastOut' = [lastOut EXCEPT ![t] = out]
  /\ ulArmed' = [ulArmed EXCEPT ![t] =
                  IF out = "usage_limit" /\ ~(StopWinsOverLimit /\ held[t] = "stop") THEN rord[t] ELSE @]
  /\ ulDue'   = [ulDue EXCEPT ![t] = IF out = "usage_limit" THEN FALSE ELSE @]
  /\ kidOpen' = [kidOpen EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<queue, rord, att, why, emitted, retries, steers, rkind, rlBud>>

(* Transient failure or provider crash of the current attempt with budget  *)
(* left: a new attempt in the same run (retry / provider_recovery).        *)
RetryAttempt(t, e) ==
  /\ e \in pev[t] /\ e.n = "root" /\ CurEv(t, e) /\ Retryable(e.o)
  /\ MayRetry(t)
  /\ pev' = [pev EXCEPT ![t] = @ \ {e}]
  /\ NewAttempt(t, Sat(att[t] + 1), IF e.o = "transient" THEN "retry" ELSE "provider_recovery")
  /\ retries' = [retries EXCEPT ![t] = Min(@ + 1, MaxRetry)]
  /\ UNCHANGED <<base, steers, lastOut, rkind, ulArmed, ulDue, rlBud>>

(* A child node ending never changes the run (S2). *)
IngestChild(t, e) ==
  /\ e \in pev[t] /\ e.n = "child" /\ CurEv(t, e)
  /\ pev' = [pev EXCEPT ![t] = @ \ {e}]
  /\ UNCHANGED <<base, att, why, emitted, kidOpen, retries, steers, lastOut, rkind, ulArmed, ulDue, rlBud>>

(* Event of a superseded attempt or an ended run: discarded. Mutant        *)
(* "D_staleDiscard" removes this action (the engine keeps stale events),   *)
(* which must surface as a deadlock (AC-M7).                                *)
IngestStale(t, e) ==
  /\ ~M("D_staleDiscard")
  /\ e \in pev[t] /\ ~CurEv(t, e)
  /\ pev' = [pev EXCEPT ![t] = @ \ {e}]
  /\ UNCHANGED <<base, att, why, emitted, kidOpen, retries, steers, lastOut, rkind, ulArmed, ulDue, rlBud>>

(* UsageLimitRecoveryWorker: the latest run failed on a usage limit, the   *)
(* reset passed, so a resume message (fixed id, created by server) starts  *)
(* a run at once (start_immediately, no active run).                        *)
(* RL11 fix: the guard allows a "limit" hold (T3 holds the queue on a usage *)
(* limit, so a "not held" guard makes F9 unreachable). A "stop" hold never  *)
(* gets here: Stop disarms the resume (R14), and a "fail" hold has no armed *)
(* resume (a later failure supersedes lastOut).                            *)
(* D6 (Tyler): the reset also releases the "limit" hold, so a person       *)
(* message held by the usage limit runs once the resume run ends, without  *)
(* a new person send. Only "limit" is released; "stop" and "fail" stay.    *)
(* FixLimitRelease = FALSE is T3 (the hold waits for a person send);      *)
(* mutant "D6_releaseOnReset" drops the release.                           *)
D6Release == FixLimitRelease /\ ~M("D6_releaseOnReset")
UsageLimitRecovery(t) ==
  /\ ulArmed[t] # 0 /\ ulArmed[t] = rord[t] /\ lastOut[t] = "usage_limit"
  /\ ulDue[t] /\ ~Live(t) /\ rord[t] < MaxRuns
  /\ held[t] \in {"none", "limit"}
  /\ held' = [held EXCEPT ![t] = IF D6Release /\ @ = "limit" THEN "none" ELSE @]
  /\ BeginRun(t)
  /\ NewAttempt(t, 1, "initial")
  /\ retries' = [retries EXCEPT ![t] = 0]
  /\ steers'  = [steers EXCEPT ![t] = 0]
  /\ rkind'   = [rkind EXCEPT ![t] = "resume"]
  /\ ulArmed' = [ulArmed EXCEPT ![t] = 0]
  /\ ulDue'   = [ulDue EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<queue, pev, lastOut, rlBud>>

(* Documented terminal states (AC-M7): nothing is owed. No live run, no    *)
(* provider event in flight, the queue is empty or held, and no armed      *)
(* resume. Done stutters only there, so the checker runs WITHOUT -deadlock *)
(* and any other state with no enabled action is reported.                 *)
RL_Quiescent ==
  \A t \in Threads : /\ ~Live(t) /\ pev[t] = {}
                     /\ (queue[t] = <<>> \/ Held(t))
                     /\ ulArmed[t] = 0
RL_Done == RL_Quiescent /\ UNCHANGED rlVars

-----------------------------------------------------------------------------
RL_Next ==
  \/ \E t \in Threads :
       \/ \E m \in Modes : PersonSend(t, m) \/ RestartActive(t, m)
       \/ Steer(t) \/ RequestInterrupt(t) \/ ServerEnqueue(t)
       \/ \E o \in Outcomes : ProviderTerminal(t, o)
       \/ ChildStart(t) \/ ChildEnd(t) \/ ResetPasses(t)
       \/ StartFromQueue(t) \/ UsageLimitRecovery(t)
       \/ \E e \in Events : IngestEnd(t, e) \/ RetryAttempt(t, e) \/ IngestChild(t, e) \/ IngestStale(t, e)
  \/ RL_Done

RL_Fairness ==
  \A t \in Threads :
    /\ WF_rlVars(StartFromQueue(t))
    /\ WF_rlVars(UsageLimitRecovery(t))
    /\ WF_rlVars(ResetPasses(t))
    /\ WF_rlVars(\E o \in Outcomes : ProviderTerminal(t, o))
    /\ WF_rlVars(\E e \in pev[t] : IngestEnd(t, e) \/ RetryAttempt(t, e) \/ IngestChild(t, e) \/ IngestStale(t, e))

RLSpec == RL_Init /\ [][RL_Next]_rlVars /\ RL_Fairness

-----------------------------------------------------------------------------
(* Safety *)

RL_TypeOK ==
  /\ BaseTypeOK
  /\ att \in [Threads -> Atts] /\ why \in [Threads -> Reasons]
  /\ emitted \in [Threads -> BOOLEAN] /\ kidOpen \in [Threads -> BOOLEAN]
  /\ \A t \in Threads : pev[t] \subseteq Events
  /\ retries \in [Threads -> 0..MaxRetry] /\ steers \in [Threads -> 0..2]
  /\ lastOut \in [Threads -> Ends] /\ rkind \in [Threads -> Kinds \cup {"none"}]
  /\ ulArmed \in [Threads -> 0..MaxRuns] /\ ulDue \in [Threads -> BOOLEAN]

(* Trimming VIEW (Fast_trimming.md): why is read only by its own update and *)
(* the W_*Completed witnesses, so safety configs without those witnesses   *)
(* may merge states that differ only in why. Not for liveness configs.     *)
RL_View == <<base, att, emitted, kidOpen, pev, retries, steers, lastOut, rkind, ulArmed, ulDue, rlBud>>

RL_BoundsNotHit ==
  \A t \in Threads : rord[t] < MaxRuns /\ att[t] < MaxAtt

(* S2 (action property): a run leaves live only in a step that consumes the *)
(* ROOT terminal event of its CURRENT attempt. Covers the three legal ends: *)
(* root terminal, root interrupted (R7), retry budget exhausted.           *)
S2_OnlyRootEndsRun ==
  [][\A t \in Threads :
       (rord'[t] = rord[t] /\ rord[t] \in liveSet[t] /\ rord[t] \notin liveSet'[t])
         => \E e \in pev[t] : e.n = "root" /\ e.r = rord[t] /\ e.a = att[t] /\ e \notin pev'[t]
    ]_rlVars

(* R7 (action property): after Stop reached the run, no new attempt starts *)
(* in it (no retry, recovery or steering restart).                         *)
R7_NoNewAttemptAfterStop ==
  [][\A t \in Threads :
       (rord'[t] = rord[t] /\ Live(t) /\ intReq[t] /\ att'[t] # att[t]) => FALSE
    ]_rlVars

(* A steer joins only an attempt that has not emitted its terminal, so the *)
(* provider sees it (else it is lost).                                     *)
RL_NoLostSteer ==
  [][\A t \in Threads :
       (steers'[t] > steers[t] /\ att'[t] = att[t] /\ rord'[t] = rord[t]) => ~emitted[t]
    ]_rlVars

(* R14 (new): a thread the person stopped never auto-resumes after a     *)
(* usage limit (a held-by-stop thread has no armed resume).              *)
R14_StopDisarmsResume == \A t \in Threads : held[t] = "stop" => ulArmed[t] = 0

(* D6 (action property): a usage-limit resume run never starts with the   *)
(* queue still held by the usage limit.                                    *)
D6_ReleaseAtReset ==
  [][\A t \in Threads :
       (rord'[t] # rord[t] /\ rkind'[t] = "resume") => held'[t] # "limit"
    ]_rlVars

(* Retry budget (state form). *)
RL_RetryBudget == \A t \in Threads : att[t] <= 1 + (MaxPerson + MaxRetry)

-----------------------------------------------------------------------------
(* Liveness *)

(* L3: a queue that is not held eventually drains (or becomes held). *)
L3_QueueDrains ==
  \A t \in Threads : (queue[t] # <<>> /\ ~Held(t)) ~> (queue[t] = <<>> \/ Held(t))

(* Stop quiesces the thread (R7 liveness). *)
RL_StopQuiesces == \A t \in Threads : intReq[t] ~> ~Live(t)

(* R11 / D6: a queue held only by a usage limit is released once the      *)
(* limit resets (the resume run starts), so queued work runs without the    *)
(* person.                                                                 *)
R11_LimitHoldReleased ==
  \A t \in Threads : (held[t] = "limit" /\ queue[t] # <<>>)
                       ~> (held[t] # "limit" \/ queue[t] = <<>>)

-----------------------------------------------------------------------------
(* F witnesses: invariants that MUST be violated (a reachable success).    *)

W_F9_UsageLimitResumed ==   \* a usage-limit failure resumed and completed, no person
  ~\E t \in Threads : ~Live(t) /\ rkind[t] = "resume" /\ lastOut[t] = "completed"
W_SteerApplied ==           \* a native steer joined an attempt that then completed
  ~\E t \in Threads : steers[t] > 0 /\ \E e \in pev[t] : CurEv(t, e) /\ e.n = "root" /\ e.o = "completed"
W_RestartCompleted ==
  ~\E t \in Threads : why[t] = "steering_restart" /\ \E e \in pev[t] : CurEv(t, e) /\ e.o = "completed"
W_RetryCompleted ==
  ~\E t \in Threads : why[t] = "retry" /\ \E e \in pev[t] : CurEv(t, e) /\ e.o = "completed"
W_RecoveryCompleted ==
  ~\E t \in Threads : why[t] = "provider_recovery" /\ \E e \in pev[t] : CurEv(t, e) /\ e.o = "completed"
W_InterruptConfirmed ==
  ~\E t \in Threads : ~Live(t) /\ lastOut[t] = "interrupted"
\* R24 witness: Stop reaches a live run that then ends on a usage limit. The
\* state is reachable, and R14_StopDisarmsResume holds in it (stop kept, nothing armed).
W_R24_StopThenLimit ==
  ~\E t \in Threads : ~Live(t) /\ lastOut[t] = "usage_limit" /\ held[t] = "stop"
\* R11 trace witness: work queued before a usage-limit failure stays held
\* after the auto-resume run completed (T3 behavior, FixLimitRelease = FALSE).
W_R11_HeldAfterResume ==
  ~\E t \in Threads : /\ ~Live(t) /\ rkind[t] = "resume" /\ lastOut[t] = "completed"
                      /\ held[t] = "limit" /\ queue[t] # <<>>

=============================================================================
