---------------------------- MODULE ZI_PlImpl ----------------------------
(***************************************************************************)
(* ZI_ZeronImpl plus explicit replication, for the refinement into         *)
(* Placement (ZI_PlacementRef.tla).                                        *)
(*                                                                         *)
(* ZI keeps one synced set per chat ledger. Placement moves rows replica   *)
(* by replica (Push to the chat2 room, Pull from it). This module adds     *)
(* that layer and changes no ZI action:                                    *)
(*   - ZI steps: every ZNext step, unchanged, with one extra gate (below). *)
(*   - Sync steps: SyncPush / SyncPull (Placement Push / Pull) and         *)
(*     StageRes (Placement RemoteChildResult). They are ZI stutters.       *)
(*                                                                         *)
(* Gate (the restriction): a host commits a receipt for a row only when    *)
(* the row is in its own replica:                                          *)
(*   - person row of the root: in RootDev's replica of P,                  *)
(*   - first Run row of the child (task): in ChildDev's replica of K,      *)
(*   - the child's notice: the parent host staged the result row from its  *)
(*     replica of K (the done row reached RootDev).                        *)
(* So the result is: ZI with replication made explicit and drains gated on *)
(* replicated rows refines Placement. ZI behaviors that drain a row before *)
(* it could replicate are cut.                                             *)
(*                                                                         *)
(* Aux variables hold only what ZI has no concept of: the room contents    *)
(* (xedge), the rows each device imported (pulled), the staged result row  *)
(* (staged), and the mutant budget. Every other Placement variable is      *)
(* derived from ZI mechanism variables in ZI_PlacementRef.                 *)
(*                                                                         *)
(* Slice: one child, one person row (MaxPerson = 1), MaxQ = MaxNarrow = 0, *)
(* RootDevKids = {}. A second person row or a second child has no          *)
(* Placement counterpart.                                                  *)
(***************************************************************************)
EXTENDS ZI_ZeronImpl

CONSTANTS
  PDev,  \* device the person sends from (Placement UserDev)
  PMut   \* "none", "ReDrain" (re-drain a receipted task row), "NoReplicaGate" (drain before sync)

ASSUME PDev \in {RootDev, ChildDev}
ASSUME PMut \in {"none", "ReDrain", "NoReplicaGate"}
ASSUME Cardinality(Threads \ {Root}) = 1 /\ RootDevKids = {}

Kid    == CHOOSE c \in Children : TRUE
PChats == {"P", "K"}

\* Placement row shapes (Placement.tla: Cmd, Out, MsgRow, DoneRow). ZI_PlacementRef checks they agree.
PNoId       == <<"none", 0>>
PCmd(id, m) == [t |-> "cmd", id |-> id, m |-> m]
POut(id, o) == [t |-> "out", id |-> id, m |-> o]
PMsg(m)     == [t |-> "msg", id |-> PNoId, m |-> m]
PDone       == [t |-> "done", id |-> PNoId, m |-> "K"]

\* Placement attempt ids: <<device, seq>>. The person row is PDev's first; the kick is RootDev's next.
RunId  == <<PDev, 1>>
KickId == <<RootDev, IF PDev = RootDev THEN 2 ELSE 1>>
ResId  == <<"K", 1>>

VARIABLES
  xedge,    \* [PChats -> rows] chat2 room log
  pulled,   \* [Devs -> [PChats -> rows]] rows a device imported from the room
  staged,   \* BOOLEAN: RootDev wrote the result row into P (pull road)
  redrain   \* mutant budget (0..1)

aux   == <<xedge, pulled, staged, redrain>>
pvars == <<zvars, aux>>

-----------------------------------------------------------------------------
(* Facts read from ZI mechanism variables *)

RunIn       == PersonId(Root, 1) \in cmds[Root]
KickIn      == TaskId(Kid) \in cmds[Kid]
RunClaimed  == PersonId(Root, 1) \in rc[Root]
KickClaimed == TaskId(Kid) \in rc[Kid]
ResClaimed  == \E r \in Ords : NoticeId(Kid, r) \in rc[Root]
RunDisp     == <<Root, 1>> \in jr[RootDev]
RunDone     == RunDisp /\ Eff("start", Root, 1, 0) \notin obx[RootDev]
KickDisp    == <<Kid, 1>> \in jr[ChildDev]
KickDone    == KickDisp /\ Eff("start", Kid, 1, 0) \notin obx[ChildDev]
ChildOut    == orchOut[Kid] # {}
ResPushed   == pushes[Kid] >= 1

\* Rows device d wrote itself into chat c's replica.
Own(d, c) ==
  IF c = "P"
  THEN (IF d = PDev /\ RunIn THEN {PCmd(RunId, "run")} ELSE {})
       \cup (IF d = RootDev
             THEN (IF RunDisp THEN {PMsg("run")} ELSE {})
                  \cup (IF RunDone THEN {POut(RunId, "applied")} ELSE {})
                  \cup (IF staged THEN {PCmd(ResId, "res")} ELSE {})
                  \cup (IF ResPushed THEN {PMsg("res")} ELSE {})
             ELSE {})
  ELSE IF d = RootDev
  THEN (IF KickIn THEN {PCmd(KickId, "kick")} ELSE {})
  ELSE (IF KickDisp THEN {PMsg("kick")} ELSE {})
       \cup (IF KickDone THEN {POut(KickId, "applied")} ELSE {})
       \cup (IF ChildOut THEN {PDone} ELSE {})

RepOf(d, c) == Own(d, c) \cup pulled[d][c]
PRep        == [d \in Devs |-> [c \in PChats |-> RepOf(d, c)]]

\* Placement Holders(c) for these constants (pull road, no host move).
PlHolders(c) == IF c = "P" THEN {RootDev, PDev} ELSE {RootDev, ChildDev}

-----------------------------------------------------------------------------
(* Sync steps (ZI stutters) *)

\* A chat with one holder needs no room traffic: skip it (no observable behavior).
SyncPush(d, c) ==
  /\ Cardinality(PlHolders(c)) > 1
  /\ up[d] /\ d \in PlHolders(c)
  /\ ~(RepOf(d, c) \subseteq xedge[c])
  /\ xedge' = [xedge EXCEPT ![c] = @ \cup RepOf(d, c)]
  /\ UNCHANGED <<zvars, pulled, staged, redrain>>

SyncPull(d, c) ==
  /\ Cardinality(PlHolders(c)) > 1
  /\ up[d] /\ d \in PlHolders(c)
  /\ ~(xedge[c] \subseteq RepOf(d, c))
  /\ pulled' = [pulled EXCEPT ![d][c] = @ \cup xedge[c]]
  /\ UNCHANGED <<zvars, xedge, staged, redrain>>

\* Pull road: RootDev sees the done row in its replica of K and writes the result row into P.
StageRes ==
  /\ ~staged
  /\ PDone \in RepOf(RootDev, "K")
  /\ staged' = TRUE
  /\ UNCHANGED <<zvars, xedge, pulled, redrain>>

\* Mutant ReDrain: a recovery rescan re-drains the child's task row although it has a receipt.
MutReDrain ==
  /\ PMut = "ReDrain" /\ redrain = 0
  /\ Awake(Kid) /\ KickClaimed
  /\ hq' = [hq EXCEPT ![Kid] = <<Msg("task", 0, 0)>> \o @]
  /\ redrain' = 1
  \* Lists every ZI tuple except hq. A new ZI tuple left out here fails loudly (unassigned variable).
  /\ UNCHANGED <<mirror, cmds, orchOut, rc, mark, obx, jr, jseq, prov, replied, up, zbud, xv, qz, az,
                 xedge, pulled, staged>>

-----------------------------------------------------------------------------
(* Gate on ZI steps: a new receipt needs the row in the host's replica. *)

Gate ==
  /\ (PersonId(Root, 1) \in rc'[Root] /\ ~RunClaimed) => PCmd(RunId, "run") \in RepOf(RootDev, "P")
  /\ (TaskId(Kid) \in rc'[Kid] /\ ~KickClaimed) => PCmd(KickId, "kick") \in RepOf(ChildDev, "K")
  /\ ((\E r \in Ords : NoticeId(Kid, r) \in rc'[Root]) /\ ~ResClaimed) => staged

PInit ==
  /\ ZInit
  /\ xedge = [c \in PChats |-> {}]
  /\ pulled = [d \in Devs |-> [c \in PChats |-> {}]]
  /\ staged = FALSE
  /\ redrain = 0

PNext ==
  \/ ZNext /\ (PMut = "NoReplicaGate" \/ Gate) /\ UNCHANGED aux
  \/ \E d \in Devs, c \in PChats : SyncPush(d, c) \/ SyncPull(d, c)
  \/ StageRes
  \/ MutReDrain

PSpec == PInit /\ [][PNext]_pvars
=============================================================================
