---------------------------- MODULE ZI_PlacementRef ----------------------------
(***************************************************************************)
(* Safety refinement ZI_PlImpl => Placement (Mode "v2", pull road, no host *)
(* move). Checked as PROPERTY PlSafe == PL!Init /\ [][PL!Next]_PL!vars.    *)
(*                                                                         *)
(* Fit: chat P = root r on RootDev (Placement ParentHost), chat K = child  *)
(* on ChildDev (ChildHost), both laptops, no Cloud. Person on PDev         *)
(* (UserDev). Mids: "run" = PersonId(r, 1), "kick" = TaskId(child),        *)
(* "res" = the child's notice.                                             *)
(*                                                                         *)
(* Step map (ZI step -> Placement step; every other ZI step is a stutter): *)
(*   PersonWrite(r)            -> IssueRun                                 *)
(*   DrainPerson(r)            -> Claim(RootDev, run)   (receipt = claim)  *)
(*   DispatchRun(r, 1)         -> Exec(RootDev, run)                       *)
(*   DispatchMark(r, 1)        -> Resolve(RootDev, run)                    *)
(*   CreateWrite(child)        -> IssueKick(RootDev)                       *)
(*   DrainTask(child)          -> Claim(ChildDev, kick)                    *)
(*   DispatchRun(child, 1)     -> Exec(ChildDev, kick)                     *)
(*   DispatchMark(child, 1)    -> Resolve(ChildDev, kick)                  *)
(*   PublishWrite(child, 1)    -> ChildFinish(ChildDev)  (done row)        *)
(*   IngestNotice(r, child, 1) -> Claim(RootDev, res)                      *)
(*   pushes 0 -> 1 (wake start, task_status ack, wait ack) -> Exec(res)    *)
(*   GoOffline / GoOnline      -> GoOffline / GoOnline                     *)
(*   SyncPush/SyncPull/StageRes (ZI_PlImpl) -> Push / Pull / RemoteChildResult *)
(*                                                                         *)
(* Derived from ZI mechanisms: processed, procMid (rc), journal (jr,       *)
(* pushes), executing, inflight (rc, jr, obx), dispatched (dispatches,     *)
(* pushes), execBy, child (jr, orchOut), issued, seq, online (up), rep     *)
(* (own rows from cmds, jr, obx, orchOut, pushes).                         *)
(* Aux (ZI has no such state): edge, imported rows, staged result row.     *)
(* Constant: host, view (S9: fixed host), net, relayLog, crash and budget  *)
(* counters.                                                               *)
(*                                                                         *)
(* Not mapped: Resolve(res) (ZI has no outcome row for an ingested notice),*)
(* Placement Crash / Recover* / Sweep / Rollback (a ZI DeviceCrash loses   *)
(* only MCP wait handlers, so it is a stutter), relay, retry, expiry, dup. *)
(***************************************************************************)
EXTENDS ZI_PlImpl

PlHost  == [c \in PChats |-> IF c = "P" THEN RootDev ELSE ChildDev]
NoHand  == [on |-> FALSE, ids |-> {}, mids |-> {}, jr |-> {}, rows |-> {}]
\* Dispatches of every run of thread t, capped at 2.
Cnt2(t) == Min(Cardinality({r \in Ords : dispatches[t][r] >= 1})
               + Cardinality({r \in Ords : dispatches[t][r] >= 2}), 2)
IfS(b, S) == IF b THEN S ELSE {}

MProcessed == [d \in Devs |-> IF d = RootDev
                              THEN IfS(RunClaimed, {RunId}) \cup IfS(ResClaimed, {ResId})
                              ELSE IfS(KickClaimed, {KickId})]
MProcMid   == [d \in Devs |-> IF d = RootDev
                              THEN IfS(RunClaimed, {"run"}) \cup IfS(ResClaimed, {"res"})
                              ELSE IfS(KickClaimed, {"kick"})]
MJournal   == [d \in Devs |-> IF d = RootDev
                              THEN IfS(RunDisp, {"run"}) \cup IfS(ResPushed, {"res"})
                              ELSE IfS(KickDisp, {"kick"})]
MExecuting == [d \in Devs |-> IF d = RootDev
                              THEN IfS(RunClaimed /\ ~RunDisp, {RunId}) \cup IfS(ResClaimed /\ ~ResPushed, {ResId})
                              ELSE IfS(KickClaimed /\ ~KickDisp, {KickId})]
MInflight  == [d \in Devs |-> IF d = RootDev
                              THEN IfS(RunDisp /\ ~RunDone, {RunId}) \cup IfS(ResPushed, {ResId})
                              ELSE IfS(KickDisp /\ ~KickDone, {KickId})]
\* Wake receipt: a command row in the room that the chat's host has not imported yet.
MWake      == {<<PlHost[c], c>> :
                 c \in {x \in PChats : \E w \in xedge[x] : w.t = "cmd" /\ w \notin RepOf(PlHost[x], x)}}
MIssued    == IfS(RunIn, {"run"}) \cup IfS(KickIn, {"kick"}) \cup IfS(staged, {"res"})
MChild     == IF ChildOut THEN "done" ELSE IF Cnt2(Kid) >= 1 THEN "running" ELSE "idle"
MDispatched == [m \in {"run", "kick", "res"} |->
                 IF m = "run" THEN Min(dispatches[Root][1], 2)
                 ELSE IF m = "kick" THEN Cnt2(Kid) ELSE pushes[Kid]]
MExecBy    == IfS(RunDisp, {<<RunId, RootDev>>}) \cup IfS(KickDisp, {<<KickId, ChildDev>>})
              \cup IfS(ResPushed, {<<ResId, RootDev>>})
MSeq       == [d \in Devs |-> (IF d = PDev /\ RunIn THEN 1 ELSE 0) + (IF d = RootDev /\ KickIn THEN 1 ELSE 0)]

PL == INSTANCE Placement WITH
  Laptops <- Devs, Cloud <- {}, ParentHost <- RootDev, ChildHost <- ChildDev,
  UserDev <- PDev, PHolders <- {PDev},
  Mode <- "v2", ResultRoad <- "pull", HostMove <- "off", MoveTarget <- ChildDev,
  CrashBudget <- 0, OfflineBudget <- MaxOff, ExpireBudget <- 0, RetryBudget <- 0,
  DupBudget <- 0, MaxSeq <- 2, RollbackBudget <- 0, OffDevs <- Devs, Mutant <- "none",
  edge <- xedge, rep <- PRep, online <- up, host <- PlHost,
  view <- [d \in Devs |-> PlHost],
  processed <- MProcessed, procMid <- MProcMid, journal <- MJournal,
  executing <- MExecuting, inflight <- MInflight,
  net <- {}, relayLog <- {}, wake <- MWake, issued <- MIssued, child <- MChild,
  emitDue <- FALSE, handoff <- NoHand, dispatched <- MDispatched, execBy <- MExecBy,
  badHost <- FALSE, seq <- MSeq,
  crashes <- 0, offs <- offs, expires <- 0, retries <- 0, dups <- 0, moves <- 0, rollbacks <- 0

PlSafe == PL!Init /\ [][PL!Next]_(PL!vars)

\* Placement invariants read through the mapping (implied by PlSafe; listed for quick failures).
PL_TypeOK == PL!TypeOK
PL_S9     == PL!S9
PL_S3Safe == PL!S3Safe
PL_S10    == PL!S10

\* The row constructors of ZI_PlImpl equal Placement's.
RowShapeOK ==
  /\ PCmd(RunId, "run") = PL!Cmd(RunId, "run")
  /\ POut(KickId, "applied") = PL!Out(KickId, "applied")
  /\ PMsg("res") = PL!MsgRow("res")
  /\ PDone = PL!DoneRow
  /\ PNoId = PL!NoId /\ ResId = PL!ResId
ASSUME RowShapeOK

\* Witnesses (must be violated): the child's result reaches the parent across devices (F11),
\* and, with the person on another device, the run row reached the host by sync.
W_F11    == PL!F11Witness
W_KickRan == ~(MDispatched["kick"] >= 1)
=============================================================================
