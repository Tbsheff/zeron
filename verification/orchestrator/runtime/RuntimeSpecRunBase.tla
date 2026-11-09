------------------------------ MODULE RuntimeSpecRunBase ------------------------------
(***************************************************************************)
(* Shared run core for Zeron Orchestrator v2. RunLifecycle and Delegation  *)
(* both EXTEND this module, so a later Composition module can EXTEND both  *)
(* and share one copy of the per-thread queue and run slot.                *)
(*                                                                         *)
(* Vars: queue (FIFO of messages), held (hold reason), liveSet (run        *)
(* ordinals whose status is live; S1 says at most one), rord (latest run   *)
(* ordinal), intReq (interrupt requested on the current run; R7).          *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
  Threads, Root, NoThread,
  MaxRuns,      \* run ordinals 1..MaxRuns (BoundsNotHit checks it never bites)
  MaxQueue,     \* queue length bound for person sends
  Mut           \* mutant switch: "none", or the name of ONE guard to remove

ASSUME Root \in Threads /\ NoThread \notin Threads

VARIABLES queue, held, liveSet, rord, intReq
base == <<queue, held, liveSet, rord, intReq>>

Ords  == 1..MaxRuns
Holds == {"none", "stop", "fail", "limit"}   \* why a queue is held
Kinds == {"person", "server", "task", "wake", "resume"}
Msg(k, r, g) == [k |-> k, r |-> r, g |-> g]
Msgs == [k : Kinds, r : 0..MaxRuns, g : 0..8]

Min(a, b) == IF a < b THEN a ELSE b
Max(a, b) == IF a > b THEN a ELSE b
M(name) == Mut = name                         \* TRUE iff this guard is removed

Live(t)  == liveSet[t] # {}
Held(t)  == held[t] # "none"

BaseInit ==
  /\ queue   = [t \in Threads |-> <<>>]
  /\ held    = [t \in Threads |-> "none"]
  /\ liveSet = [t \in Threads |-> {}]
  /\ rord    = [t \in Threads |-> 0]
  /\ intReq  = [t \in Threads |-> FALSE]

BaseTypeOK ==
  /\ \A t \in Threads : queue[t] \in Seq(Msgs)
  /\ held \in [Threads -> Holds]
  /\ liveSet \in [Threads -> SUBSET Ords]
  /\ rord \in [Threads -> 0..MaxRuns]
  /\ intReq \in [Threads -> BOOLEAN]

(* The S1 guard: a run starts only when no run of the thread is live.      *)
(* Mutant "S1_start" removes it.                                           *)
NoActiveRun(t) == M("S1_start") \/ ~Live(t)

CanStartFromQueue(t) ==
  /\ ~Held(t) /\ queue[t] # <<>> /\ rord[t] < MaxRuns /\ NoActiveRun(t)

(* Begin run rord+1. Callers set queue and their own variables. *)
BeginRun(t) ==
  /\ liveSet' = [liveSet EXCEPT ![t] = @ \cup {rord[t] + 1}]
  /\ rord'    = [rord EXCEPT ![t] = @ + 1]
  /\ intReq'  = [intReq EXCEPT ![t] = FALSE]

PopHead(t) == queue' = [queue EXCEPT ![t] = Tail(@)]

(* The current run's status leaves live. Only the root node of the current *)
(* attempt may cause this (S2); callers carry that guard.                  *)
FinishRun(t) ==
  /\ liveSet' = [liveSet EXCEPT ![t] = @ \ {rord[t]}]
  /\ intReq'  = [intReq EXCEPT ![t] = FALSE]

(* Stop with holdQueue: interrupt REQUEST only (R7); the run stays live    *)
(* until the provider confirms.                                            *)
MarkInterrupt(t) ==
  /\ intReq' = [intReq EXCEPT ![t] = @ \/ Live(t)]
  /\ held'   = [held EXCEPT ![t] = "stop"]

S1_AtMostOneActiveRun == \A t \in Threads : Cardinality(liveSet[t]) <= 1

=============================================================================
