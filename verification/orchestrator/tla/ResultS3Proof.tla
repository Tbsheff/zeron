------------------------- MODULE ResultS3Proof -------------------------
(* Unbounded proof of the result-state projection. This module does not *)
(* yet prove that Delegation!D_Next refines Next. See proof report.       *)
EXTENDS Integers, TLAPS
CONSTANT Children
VARIABLES tDel, pushes
vars == <<tDel, pushes>>
DelSt == {"pending", "claimed", "acknowledged", "delivered", "disposed"}
Handled == {"acknowledged", "delivered", "disposed"}
TypeOK == /\ tDel \in [Children -> DelSt]
          /\ pushes \in [Children -> 0..2]
Safe == \A c \in Children : (pushes[c] <= 1 /\
          (pushes[c] = 1 => tDel[c] \in {"delivered", "acknowledged"}))
Inv == TypeOK /\ Safe
Init == /\ tDel = [c \in Children |-> "pending"]
        /\ pushes = [c \in Children |-> 0]

(* AckBy(c, TRUE), whose callers check unhandled delivery. *)
Ack(c) == /\ c \in Children /\ tDel[c] \in {"pending", "claimed"}
          /\ tDel' = [tDel EXCEPT ![c] = "acknowledged"]
          /\ pushes' = [pushes EXCEPT ![c] = IF @ + 1 < 2 THEN @ + 1 ELSE 2]

(* Plan: E claims pending tasks; D disposes pending tasks. *)
Plan(E, D) == /\ E \subseteq {c \in Children : tDel[c] = "pending"}
              /\ D \subseteq {c \in Children : tDel[c] = "pending"}
              /\ tDel' = [c \in Children |-> IF c \in E THEN "claimed"
                                             ELSE IF c \in D THEN "disposed" ELSE tDel[c]]
              /\ UNCHANGED pushes

(* ThreadStop: only unhandled results can be disposed. *)
Stop(K) == /\ K \subseteq {c \in Children : tDel[c] \notin Handled}
           /\ tDel' = [c \in Children |-> IF c \in K THEN "disposed" ELSE tDel[c]]
           /\ UNCHANGED pushes

(* StartFromQueueWake: accept claimed C, replan pending P. *)
Wake(C, P) == /\ C \subseteq {c \in Children : tDel[c] = "claimed"}
              /\ P \subseteq {c \in Children : tDel[c] = "pending"}
              /\ tDel' = [c \in Children |-> IF c \in C THEN "delivered"
                                             ELSE IF c \in P THEN "claimed" ELSE tDel[c]]
              /\ pushes' = [c \in Children |-> IF c \in C THEN
                    (IF pushes[c] + 1 < 2 THEN pushes[c] + 1 ELSE 2) ELSE pushes[c]]

(* FollowUp: the next (child, run) task reuses its handled row. *)
FollowUp(c) == /\ c \in Children /\ tDel[c] \in Handled
               /\ tDel' = [tDel EXCEPT ![c] = "pending"]
               /\ pushes' = [pushes EXCEPT ![c] = 0]
Next == \/ \E c \in Children : Ack(c) \/ FollowUp(c)
        \/ \E E, D \in SUBSET Children : Plan(E, D)
        \/ \E K \in SUBSET Children : Stop(K)
        \/ \E C, P \in SUBSET Children : Wake(C, P)
Spec == Init /\ [][Next]_vars

THEOREM InitInv == Init => Inv
 BY DEF Init, Inv, TypeOK, Safe, DelSt
THEOREM AckInv == ASSUME NEW c, Inv, Ack(c) PROVE Inv'
 BY SMT DEF Inv, TypeOK, Safe, Ack, DelSt
THEOREM PlanInv == ASSUME NEW E, NEW D, Inv, Plan(E,D) PROVE Inv'
 BY SMT DEF Inv, TypeOK, Safe, Plan, DelSt
THEOREM StopInv == ASSUME NEW K, Inv, Stop(K) PROVE Inv'
 BY SMT DEF Inv, TypeOK, Safe, Stop, DelSt, Handled
THEOREM WakeInv == ASSUME NEW C, NEW P, Inv, Wake(C,P) PROVE Inv'
 BY SMT DEF Inv, TypeOK, Safe, Wake, DelSt
THEOREM FollowUpInv == ASSUME NEW c, Inv, FollowUp(c) PROVE Inv'
 BY SMT DEF Inv, TypeOK, Safe, FollowUp, DelSt
THEOREM StepInv == Inv /\ [Next]_vars => Inv'
 BY AckInv, PlanInv, StopInv, WakeInv, FollowUpInv DEF Next, vars, Inv, TypeOK, Safe
THEOREM ResultSafety == Spec => []Safe
<1>1. Init => Inv BY InitInv
<1>2. Inv /\ [Next]_vars => Inv' BY StepInv
<1>3. Inv => Safe BY DEF Inv
<1>. QED BY <1>1, <1>2, <1>3, PTL DEF Spec
=======================================================================
