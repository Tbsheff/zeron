---------------------------- MODULE ZI_ZeronImpl ----------------------------
(***************************************************************************)
(* Zeron Orchestrator v2: implementation-level model (DESIGN 9.1 ZeronImpl)*)
(* Slice: Placement + Delegation, with optional (constant-gated) per-      *)
(* device crash at any depth (RootDevKids), duplicate rows (MaxDup), the   *)
(* ReadReply / ReadObserved split (ReadSplit), questions with (qid, epoch) *)
(* (MaxQ) and the hop-by-hop access clamp (MaxNarrow). Not here: context   *)
(* transfers and native subagents (ZI_CT*, ZI_NS* model them separately),  *)
(* RunLifecycle attempts. All gates off = the original slice exactly.      *)
(*                                                                         *)
(* Zeron mechanisms modelled (DESIGN 1, 4):                                *)
(*  - per-chat command ledger rows (chat doc `commands`): person sends,    *)
(*    the child's first Run row (CreateChild), Orch Stop rows. Rows are    *)
(*    message ids; the doc is one synced set (Placement owns replication). *)
(*  - host drain, receipt-before-mark: the host commits orch_receipts(id)  *)
(*    with the state change in ONE step; the processed_commands mark is a  *)
(*    later, separate step. Recovery = re-drain rows with no receipt.      *)
(*  - held queue: hq[t] is the host's durable ordered queue (Loro rows     *)
(*    already drained + internal wake messages), gated by held[t].         *)
(*  - chat2 doc lists for upward messages: orchOut[c] is c's own doc       *)
(*    `orch_out` list (append-only); the parent's host ingests from it.    *)
(*  - durable outbox per device (orch_outbox): start, offer, publish,      *)
(*    create and stop effects; each doc write is write-then-mark.          *)
(*  - run journal with per-device seq: a StartRun effect dispatches the    *)
(*    provider only if the journal has no (chat, run) record (R17/R18).    *)
(*  - WaitReply / WaitObserved split for delegate_task mode=wait (S7).     *)
(*  - per-device crash (volatile MCP wait handlers die) and offline.       *)
(*                                                                         *)
(* Host of a chat is fixed (S9): Root and RootDevKids on RootDev, others  *)
(* on ChildDev.                                                           *)
(* Every action names the host whose partition it changes (locality rule). *)
(*                                                                         *)
(* Variables mirrored from Delegation keep their names: the refinement     *)
(* mapping (ZI_Refinement.tla) is the identity on them, as DESIGN 9.2 says *)
(* (host(t) run row, host(parent) task row, host(t) cohort rows).          *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
  Threads, Root, NoThread,
  MaxRuns, MaxQueue, MaxDepth, MaxFanout, MaxGen,
  MaxPerson, MaxFail, MaxCrash, MaxStop, MaxOff,
  RootDev, ChildDev,
  RootDevKids,  \* children hosted on RootDev with the root (others on ChildDev); {} = old slice
  MaxDup,       \* duplicate ledger / orch_out rows the relay may deliver (0 = none)
  ReadSplit,    \* TRUE: task_status ack = ReadReply then ReadObserved (DESIGN 3.2); FALSE: one step
  MaxQ,         \* child questions (0 = off): publish on host(c), decide at ingest, (qid, epoch) answers
  MaxNarrow,    \* access narrowings (0 = off): NarrowAccess sends Clamp down hop by hop
  ZMut          \* "none", "NoDedupe" (notice ingest: no receipt, no pending check), "NoJournalGate" (dispatch),
                \* "CrashKeepsReply" (a device crash leaves the volatile WaitReply flag alive)

ASSUME Root \in Threads /\ NoThread \notin Threads /\ RootDev # ChildDev
ASSUME RootDevKids \subseteq Threads \ {Root}

Children == Threads \ {Root}
Devs     == {RootDev, ChildDev}
(* Host is a fixed function of the chat (S9). With RootDevKids # {} a tree  *)
(* spans both devices at every depth (a ChildDev child may wait on a       *)
(* RootDev grandchild), so a crash of one device is a partial crash.       *)
Host(t)  == IF t = Root \/ t \in RootDevKids THEN RootDev ELSE ChildDev
OnDev(d) == {t \in Threads : Host(t) = d}
\* Children on one device are interchangeable (SYMMETRY for configs at 3+ threads).
ZSym     == Permutations(Children \ RootDevKids)
Ords     == 1..MaxRuns
Levels   == 0..1
StopIds  == 1..(IF MaxStop > 0 THEN MaxStop ELSE 1)
Handled  == {"acknowledged", "delivered", "disposed"}
Min(a, b) == IF a < b THEN a ELSE b
Msg(k, r, g) == [k |-> k, r |-> r, g |-> g]
ZM(n) == ZMut = n

(* Deterministic message ids (DESIGN 4): person:t:n, create:c, stop:c:e, notice:c:r. *)
Id(k, t, n)    == [k |-> k, t |-> t, n |-> n]
PersonId(t, n) == Id("person", t, n)
TaskId(c)      == Id("task", c, 0)
StopId(c, s)   == Id("stop", c, s)
NoticeId(c, r) == Id("notice", c, r)
Eff(k, t, r, g) == [k |-> k, t |-> t, r |-> r, g |-> g]

VARIABLES
  \* mirrored host state (identity in the mapping)
  created, parent, depth, access,       \* task row in host(parent) + registry row
  rtask, waitOn,                         \* host(t) run row; waitOn lives in memory too (crash clears)
  tWake, tDel, tCoh, pushes, kidIn,      \* host(parent) task row; pushes, kidIn ghosts
  cDisp, cSt, cDel, cGen,                \* host(t) cohort rows
  held, liveSet, rord, intReq,           \* host(t) run row
  doomed,                                \* ghost
  \* Zeron mechanisms
  hq,        \* [Threads -> Seq(Msg)] host(t) ordered queue (durable)
  cmds,      \* [Threads -> SUBSET Id] chat t's doc `commands` ledger rows (synced)
  orchOut,   \* [Threads -> SUBSET Ords] chat t's own doc `orch_out` notices (synced, append-only)
  rc,        \* [Threads -> SUBSET Id] orch_receipts committed by host(t) for chat t's partition
  mark,      \* [Threads -> SUBSET Id] processed_commands marks (after the receipt)
  obx,       \* [Devs -> SUBSET Eff] orch_outbox per device (durable)
  jr,        \* [Devs -> SUBSET (Threads \X Ords)] run journal: dispatched (chat, run)
  jseq,      \* [Devs -> Nat] per-device journal / trace sequence number
  prov,      \* [Threads -> 0..MaxRuns] run ordinal the provider is running (0 = none)
  replied,   \* [Threads -> BOOLEAN] WaitReply sent, ToolResult not yet observed (volatile)
  up,        \* [Devs -> BOOLEAN] device online
  personRows, personLeft, failLeft, stopLeft, crashes, offs,
  dispatches, \* ghost: [Threads -> [Ords -> 0..2]] provider dispatches per run
  \* Item 2: duplicate rows (a second copy of a row with a new batch id, e.g. relay + sync roads)
  dupC,      \* [Threads -> SUBSET Id] ledger rows of chat t that also have a second copy
  dmark,     \* [Threads -> SUBSET Id] the second copy is marked processed
  dupO,      \* [Threads -> SUBSET Ords] notices in t's orch_out that also have a second copy
  dupLeft,   \* duplicate budget
  \* Item 3: task_status ack split
  readRep,   \* [Threads -> Threads \cup {NoThread}] task_status reply written, ToolResult not yet seen (volatile)
  ackSeen,   \* ghost: [Children -> BOOLEAN] an ack of c was observed in the parent's harness stream
  \* Item 4: questions (DESIGN 3.3). Mirrored names map by identity to Delegation (QSplit).
  qPend, qUsed, answers, qEp,  \* host(q): pending, asked once, answers applied, epoch (owner of the epoch)
  qAt, qVis, qWake, qHe,       \* hop rows: holder of q, its view, its question wake, the epoch it holds
  qOut,      \* [Threads -> SUBSET QRow] question rows in chat t's own orch_out (qup, qclosed)
  qCmd,      \* [Threads -> SUBSET QRow] question rows in chat t's ledger (ans, rel), each hop by hop
  qLeft,     \* question budget
  \* Item 5: access clamp. `access` above is the person's config of the chat (host(t)).
  ceil,      \* [Threads -> Levels] last Clamp ceiling host(t) received (CreateChild carries one)
  runAcc,    \* [Threads -> Levels] access the live run of t was started or reconfigured with
  aCmd,      \* [Threads -> SUBSET [e, v]] Clamp rows in chat t's ledger (clamp:t:e, ceiling v)
  aDone,     \* [Threads -> SUBSET 1..MaxNarrow] clamp receipts at host(t)
  narrowLeft, narrowsAbs  \* budget; ghost: narrowings that lowered some effective (abstract) access

mirror == <<created, parent, depth, access, rtask, waitOn, tWake, tDel, tCoh, pushes, kidIn,
            cDisp, cSt, cDel, cGen, held, liveSet, rord, intReq, doomed>>
impl   == <<hq, cmds, orchOut, rc, mark, obx, jr, jseq, prov, replied, up>>
zbud   == <<personRows, personLeft, failLeft, stopLeft, crashes, offs, dispatches>>
dupv   == <<dupC, dmark, dupO, dupLeft>>
rdv    == <<readRep, ackSeen>>
xv     == <<dupv, rdv>>
qzNoWake == <<qPend, qUsed, answers, qEp, qAt, qVis, qHe, qOut, qCmd, qLeft>>
qz     == <<qzNoWake, qWake>>
azNoRun == <<ceil, aCmd, aDone, narrowLeft, narrowsAbs>>
az     == <<runAcc, azNoRun>>
zvars  == <<mirror, impl, zbud, xv, qz, az>>

-----------------------------------------------------------------------------
RECURSIVE AncN(_, _)
AncN(x, n) == IF n = 0 \/ parent[x] = NoThread THEN {}
              ELSE {parent[x]} \cup AncN(parent[x], n - 1)
Anc(x)  == AncN(x, Cardinality(Threads))
Desc(t) == {d \in Threads : created[d] /\ t \in Anc(d)}
Kids(t) == {c \in Children : created[c] /\ parent[c] = t}

Live(t)  == liveSet[t] # {}
Held(t)  == held[t] # "none"
Awake(t) == up[Host(t)]

(* tSt is not stored: settled <=> host(parent) holds the notice receipt (DESIGN 9.2). *)
Settled(c) == created[c] /\ \E r \in Ords : NoticeId(c, r) \in rc[parent[c]]
UnsettledKids(t) == {c \in Kids(t) : ~Settled(c)}
TaskKids(t, r) == {c \in Kids(t) : tCoh[c] = r}
CohortLive(p, r) == Live(p) /\ rord[p] = r
NotWakeOf(r, m) == ~(m.k = "wake" /\ m.r = r)
Blocked(t) == t \in Children /\ qPend[t]
Chain(x)   == {x} \cup Anc(x)
\* The ancestor of q at depth d (the hop a row addressed to depth d is going to).
AncAt(q, d) == CHOOSE a \in Chain(q) : depth[a] = d
QRow(k, q, e, d) == [k |-> k, q |-> q, e |-> e, d |-> d]
QId(row) == Id(row.k, row.q, row.e)
\* Item 5: what host(t) enforces, and what the abstract access is (DESIGN 9.2 mapping).
EffAcc(t)    == Min(access[t], ceil[t])
AbsAcc(d) == IF \A x \in Chain(d) : access[x] = 1 THEN 1 ELSE 0
SetRunAcc(t) == runAcc' = IF MaxNarrow > 0 THEN [runAcc EXCEPT ![t] = EffAcc(t)] ELSE runAcc

HeadIsWake(t)  == hq[t] # <<>> /\ Head(hq[t]).k = "wake"
WakeMayPass(t) == ~Held(t) \/ held[t] = "fail"            \* R12 fix on
QueueGate(t)   == ~Held(t) \/ (HeadIsWake(t) /\ WakeMayPass(t))
CanStart(t)    == hq[t] # <<>> /\ rord[t] < MaxRuns /\ ~Live(t) /\ QueueGate(t)
StaleWake(t)   == LET m == Head(hq[t]) IN
                  m.k = "wake" /\ ~(cSt[t][m.r] = "queued" /\ cGen[t][m.r] = m.g)
\* The provider for the current run is dispatched (journal written): Done and MCP calls come from it.
Running(t) == Live(t) /\ prov[t] = rord[t]

AddObx(d, E) == [obx EXCEPT ![d] = @ \cup E]

(* Plan delivery (DESIGN T3) on host(p). ob = the new outbox of host(p) before the offer. *)
ZPlan(p, r, S, liveNow, ob) ==
  LET open  == cDisp[p][r] = "open"
      E     == IF open THEN {c \in S : ~(tWake[c] = "settled_only" /\ liveNow)} ELSE {}
      D     == IF open THEN {} ELSE S
      fresh == E # {} /\ cSt[p][r] = "none"
      g     == IF fresh THEN Min(cGen[p][r] + 1, MaxGen) ELSE cGen[p][r]
  IN /\ tDel' = [c \in Children |-> IF c \in E THEN "claimed"
                                    ELSE IF c \in D THEN "disposed" ELSE tDel[c]]
     /\ cDel' = [cDel EXCEPT ![p][r] = @ \cup E]
     /\ cSt'  = [cSt EXCEPT ![p][r] = IF fresh THEN "offered" ELSE @]
     /\ cGen' = [cGen EXCEPT ![p][r] = g]
     /\ obx'  = [obx EXCEPT ![Host(p)] =
                   IF E # {} /\ (fresh \/ cSt[p][r] = "offered") THEN ob \cup {Eff("offer", p, r, g)} ELSE ob]

(* Acknowledge on host(parent[c]) (T7, T8). *)
ZAckBy(c, seen) ==
  LET p == parent[c]  r == tCoh[c]
      rem   == cDel[p][r] \ {c}
      clear == c \in cDel[p][r] /\ rem = {} /\ cSt[p][r] \in {"offered", "queued"}
  IN /\ tDel'   = [tDel EXCEPT ![c] = "acknowledged"]
     /\ pushes' = [pushes EXCEPT ![c] = IF seen THEN Min(@ + 1, 2) ELSE @]
     /\ cDel'   = [cDel EXCEPT ![p][r] = rem]
     /\ cSt'    = [cSt EXCEPT ![p][r] = IF clear THEN "none" ELSE @]
     /\ hq'     = IF clear THEN [hq EXCEPT ![p] = SelectSeq(@, LAMBDA m : NotWakeOf(r, m))] ELSE hq

(* ThreadStop on host(t) (S-2): interrupt request, hold, dispose, drop wakes, *)
(* and one durable WriteDown stop effect per child (R5).                     *)
ZThreadStop(t, s, ob) ==
  LET Tk == {c \in Kids(t) : tDel[c] \notin Handled}
      old(r) == r <= rord[t]
  IN /\ intReq' = [intReq EXCEPT ![t] = @ \/ Live(t)]
     /\ held'   = [held EXCEPT ![t] = "stop"]
     /\ tDel'   = [c \in Children |-> IF c \in Tk THEN "disposed" ELSE tDel[c]]
     /\ cDisp'  = [cDisp EXCEPT ![t] = [r \in Ords |-> IF old(r) THEN "stopped" ELSE @[r]]]
     /\ cSt'    = [cSt EXCEPT ![t] = [r \in Ords |-> IF old(r) THEN "none" ELSE @[r]]]
     /\ cDel'   = [cDel EXCEPT ![t] = [r \in Ords |-> IF old(r) THEN {} ELSE @[r]]]
     /\ hq'     = [hq EXCEPT ![t] = SelectSeq(@, LAMBDA m : m.k # "wake")]
     /\ qWake'  = {q \in qWake : qAt[q] # t}
     /\ obx'    = [obx EXCEPT ![Host(t)] = ob \cup {Eff("stop", c, s, 0) : c \in Kids(t)}]

-----------------------------------------------------------------------------
ZInit ==
  /\ created  = [t \in Threads |-> t = Root]
  /\ parent   = [t \in Threads |-> NoThread]
  /\ depth    = [t \in Threads |-> 0]
  /\ access   = [t \in Threads |-> IF t = Root THEN 1 ELSE 0]
  /\ rtask    = [t \in Threads |-> FALSE]
  /\ waitOn   = [t \in Threads |-> NoThread]
  /\ tWake    = [c \in Children |-> "always"]
  /\ tDel     = [c \in Children |-> "pending"]
  /\ tCoh     = [c \in Children |-> 0]
  /\ pushes   = [c \in Children |-> 0]
  /\ kidIn    = [c \in Children |-> FALSE]
  /\ cDisp    = [t \in Threads |-> [r \in Ords |-> "open"]]
  /\ cSt      = [t \in Threads |-> [r \in Ords |-> "none"]]
  /\ cDel     = [t \in Threads |-> [r \in Ords |-> {}]]
  /\ cGen     = [t \in Threads |-> [r \in Ords |-> 0]]
  /\ held     = [t \in Threads |-> "none"]
  /\ liveSet  = [t \in Threads |-> {}]
  /\ rord     = [t \in Threads |-> 0]
  /\ intReq   = [t \in Threads |-> FALSE]
  /\ doomed   = {}
  /\ hq       = [t \in Threads |-> <<>>]
  /\ cmds     = [t \in Threads |-> {}]
  /\ orchOut  = [t \in Threads |-> {}]
  /\ rc       = [t \in Threads |-> {}]
  /\ mark     = [t \in Threads |-> {}]
  /\ obx      = [d \in Devs |-> {}]
  /\ jr       = [d \in Devs |-> {}]
  /\ jseq     = [d \in Devs |-> 0]
  /\ prov     = [t \in Threads |-> 0]
  /\ replied  = [t \in Threads |-> FALSE]
  /\ up       = [d \in Devs |-> TRUE]
  /\ personRows = 0 /\ personLeft = MaxPerson /\ failLeft = MaxFail /\ stopLeft = MaxStop
  /\ crashes  = [d \in Devs |-> 0]
  /\ offs     = [d \in Devs |-> 0]
  /\ dispatches = [t \in Threads |-> [r \in Ords |-> 0]]
  /\ dupC = [t \in Threads |-> {}] /\ dmark = [t \in Threads |-> {}]
  /\ dupO = [t \in Threads |-> {}] /\ dupLeft = MaxDup
  /\ readRep = [t \in Threads |-> NoThread] /\ ackSeen = [c \in Children |-> FALSE]
  /\ qPend = [c \in Children |-> FALSE] /\ qUsed = [c \in Children |-> FALSE]
  /\ answers = [c \in Children |-> 0] /\ qEp = [c \in Children |-> 0]
  /\ qAt = [c \in Children |-> NoThread] /\ qVis = [c \in Children |-> {}] /\ qWake = {}
  /\ qHe = [c \in Children |-> 0]
  /\ qOut = [t \in Threads |-> {}] /\ qCmd = [t \in Threads |-> {}] /\ qLeft = MaxQ
  /\ ceil = [t \in Threads |-> 1] /\ runAcc = [t \in Threads |-> IF t = Root THEN 1 ELSE 0]
  /\ aCmd = [t \in Threads |-> {}] /\ aDone = [t \in Threads |-> {}]
  /\ narrowLeft = MaxNarrow /\ narrowsAbs = 0

-----------------------------------------------------------------------------
(* Person *)

\* A person (any device) appends a Run row to chat t's ledger. A child chat exists once its first row is written.
PersonWrite(t) ==
  /\ created[t] /\ personRows < MaxPerson
  /\ t \in Children => TaskId(t) \in cmds[t]
  /\ cmds' = [cmds EXCEPT ![t] = @ \cup {PersonId(t, personRows + 1)}]
  /\ personRows' = personRows + 1
  /\ UNCHANGED <<mirror, hq, orchOut, rc, mark, obx, jr, jseq, prov, replied, up,
                 personLeft, failLeft, stopLeft, crashes, offs, dispatches>>

\* Stop from t's UI on host(t). (The relay/ledger road for it is Placement's.)
PersonStop(t) ==
  /\ created[t] /\ stopLeft > 0 /\ Awake(t)
  /\ ZThreadStop(t, MaxStop - stopLeft + 1, obx[Host(t)])
  /\ doomed' = doomed \cup {t} \cup Desc(t)
  /\ stopLeft' = stopLeft - 1
  /\ UNCHANGED <<created, parent, depth, access, rtask, waitOn, tWake, tCoh, pushes, kidIn,
                 cGen, liveSet, rord, cmds, orchOut, rc, mark, jr, jseq, prov, replied, up,
                 personRows, personLeft, failLeft, crashes, offs, dispatches>>

-----------------------------------------------------------------------------
(* Host drain: receipt-before-mark. The receipt and the state change commit together. *)

\* Ledger order: a child's first row (the task) drains before any later row.
FirstRowDone(t) == t \in Children => TaskId(t) \in rc[t]

DrainPerson(t, id) ==
  /\ Awake(t) /\ id \in cmds[t] /\ id.k = "person" /\ id \notin rc[t]
  /\ FirstRowDone(t) /\ Len(hq[t]) < MaxQueue /\ personLeft > 0
  /\ rc'  = [rc EXCEPT ![t] = @ \cup {id}]
  /\ hq'  = [hq EXCEPT ![t] = Append(@, Msg("person", 0, 0))]
  /\ held' = [held EXCEPT ![t] = "none"]
  /\ doomed' = doomed \ {t}
  /\ personLeft' = personLeft - 1
  /\ UNCHANGED <<created, parent, depth, access, rtask, waitOn, tWake, tDel, tCoh, pushes, kidIn,
                 cDisp, cSt, cDel, cGen, liveSet, rord, intReq,
                 cmds, orchOut, mark, obx, jr, jseq, prov, replied, up,
                 personRows, failLeft, stopLeft, crashes, offs, dispatches>>

DrainTask(c) ==
  /\ c \in Children /\ Awake(c) /\ TaskId(c) \in cmds[c] /\ TaskId(c) \notin rc[c]
  /\ rc' = [rc EXCEPT ![c] = @ \cup {TaskId(c)}]
  /\ hq' = [hq EXCEPT ![c] = <<Msg("task", 0, 0)>> \o @]
  /\ UNCHANGED <<mirror, cmds, orchOut, mark, obx, jr, jseq, prov, replied, up, zbud>>

\* S-3 DoStop: an Orch Stop row in c's ledger, keyed by receipt stop:c:s.
DrainStop(c, s) ==
  /\ Awake(c) /\ StopId(c, s) \in cmds[c] /\ StopId(c, s) \notin rc[c]
  /\ ZThreadStop(c, s, obx[Host(c)])
  /\ rc' = [rc EXCEPT ![c] = @ \cup {StopId(c, s)}]
  /\ UNCHANGED <<created, parent, depth, access, rtask, waitOn, tWake, tCoh, pushes, kidIn,
                 cGen, liveSet, rord, doomed, cmds, orchOut, mark, jr, jseq, prov, replied, up, zbud>>

\* The processed_commands mark comes after the receipt (a crash between is harmless).
MarkLedger(t, id) ==
  /\ Awake(t) /\ id \in cmds[t] /\ id \in rc[t] /\ id \notin mark[t]
  /\ mark' = [mark EXCEPT ![t] = @ \cup {id}]
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, rc, obx, jr, jseq, prov, replied, up, zbud>>

-----------------------------------------------------------------------------
(* Runs on host(t) *)

StartFromQueue(t) ==
  /\ Awake(t) /\ CanStart(t) /\ Head(hq[t]).k # "wake"
  /\ liveSet' = [liveSet EXCEPT ![t] = @ \cup {rord[t] + 1}]
  /\ rord'    = [rord EXCEPT ![t] = @ + 1]
  /\ intReq'  = [intReq EXCEPT ![t] = FALSE]
  /\ hq'      = [hq EXCEPT ![t] = Tail(@)]
  /\ rtask'   = [rtask EXCEPT ![t] = Head(hq[t]).k = "task"]
  /\ waitOn'  = [waitOn EXCEPT ![t] = NoThread]
  /\ obx'     = AddObx(Host(t), {Eff("start", t, rord[t] + 1, 0)})
  /\ SetRunAcc(t)
  /\ UNCHANGED <<created, parent, depth, access, tWake, tDel, tCoh, pushes, kidIn,
                 cDisp, cSt, cDel, cGen, held, doomed,
                 cmds, orchOut, rc, mark, jr, jseq, prov, replied, up, zbud, xv, qz, azNoRun>>

\* T6: the wake run accepts its delivery at start.
StartFromQueueWake(t) ==
  LET m == Head(hq[t])  r == m.r
      C == {c \in cDel[t][r] : tDel[c] = "claimed"}
      P == {c \in TaskKids(t, r) : Settled(c) /\ tDel[c] = "pending"}
      next == P # {} /\ cDisp[t][r] = "open"
      g == Min(cGen[t][r] + 1, MaxGen)
  IN
  /\ Awake(t) /\ CanStart(t) /\ HeadIsWake(t) /\ ~StaleWake(t)
  /\ liveSet' = [liveSet EXCEPT ![t] = @ \cup {rord[t] + 1}]
  /\ rord'    = [rord EXCEPT ![t] = @ + 1]
  /\ intReq'  = [intReq EXCEPT ![t] = FALSE]
  /\ hq'      = [hq EXCEPT ![t] = Tail(@)]
  /\ rtask'   = [rtask EXCEPT ![t] = FALSE]
  /\ waitOn'  = [waitOn EXCEPT ![t] = NoThread]
  /\ tDel' = [c \in Children |-> IF c \in C THEN "delivered"
                                 ELSE IF c \in P /\ next THEN "claimed" ELSE tDel[c]]
  /\ pushes' = [c \in Children |-> IF c \in C THEN Min(pushes[c] + 1, 2) ELSE pushes[c]]
  /\ cDel' = [cDel EXCEPT ![t][r] = IF next THEN P ELSE {}]
  /\ cSt'  = [cSt EXCEPT ![t][r] = IF next THEN "offered" ELSE "none"]
  /\ cGen' = [cGen EXCEPT ![t][r] = IF next THEN g ELSE @]
  /\ obx'  = AddObx(Host(t), {Eff("start", t, rord[t] + 1, 0)}
                             \cup (IF next THEN {Eff("offer", t, r, g)} ELSE {}))
  /\ SetRunAcc(t)
  /\ UNCHANGED <<created, parent, depth, access, tWake, tCoh, kidIn, cDisp, held, doomed,
                 cmds, orchOut, rc, mark, jr, jseq, prov, replied, up, zbud, xv, qz, azNoRun>>

\* Effect worker: StartRun -> Harness::run, gated by the run journal (R17/R18).
DispatchRun(t, r) ==
  LET d == Host(t) IN
  /\ up[d] /\ Eff("start", t, r, 0) \in obx[d]
  /\ ZM("NoJournalGate") \/ <<t, r>> \notin jr[d]
  /\ prov' = [prov EXCEPT ![t] = r]
  /\ jr'   = [jr EXCEPT ![d] = @ \cup {<<t, r>>}]
  /\ jseq' = [jseq EXCEPT ![d] = @ + 1]
  /\ dispatches' = [dispatches EXCEPT ![t][r] = Min(@ + 1, 2)]
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, rc, mark, obx, replied, up,
                 personRows, personLeft, failLeft, stopLeft, crashes, offs>>

DispatchMark(t, r) ==
  LET d == Host(t) IN
  /\ up[d] /\ Eff("start", t, r, 0) \in obx[d] /\ <<t, r>> \in jr[d]
  /\ obx' = [obx EXCEPT ![d] = @ \ {Eff("start", t, r, 0)}]
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, rc, mark, jr, jseq, prov, replied, up, zbud>>

(* RL10 EndRun with the R9 sweep on every outcome; a child task run publishes its notice. *)
(* Questions at run end (host(t) only): t's own question is closed by a      *)
(* QuestionClosed row up; questions t holds and has seen are released (Q9)  *)
(* by a Release row down toward host(q); the Q8 / R15 catch-up re-wakes      *)
(* held settled_only questions that only t's wait could have shown.         *)
ZEndRun(t, out) ==
  LET r   == rord[t]
      ob0 == IF t \in Children /\ rtask[t] THEN obx[Host(t)] \cup {Eff("publish", t, r, 0)}
             ELSE obx[Host(t)]
      S   == {c \in TaskKids(t, r) : Settled(c) /\ tDel[c] = "pending"}
      own == t \in Children /\ qPend[t]
      R2  == {q \in Children : qAt[q] = t /\ t \in qVis[q]}
      CU  == {c \in TaskKids(t, r) : tWake[c] = "settled_only" /\ tDel[c] # "disposed"
                                    /\ ~Settled(c) /\ qAt[c] = t /\ qVis[c] = {}}
  IN /\ qPend' = IF t \in Children THEN [qPend EXCEPT ![t] = FALSE] ELSE qPend
     /\ qOut'  = IF own THEN [qOut EXCEPT ![t] = @ \cup {QRow("qclosed", t, qEp[t], depth[t] - 1)}] ELSE qOut
     /\ qCmd'  = [x \in Threads |-> qCmd[x] \cup {QRow("rel", q, qHe[q], depth[t]) :
                                                  q \in {q2 \in R2 : AncAt(q2, depth[t] + 1) = x}}]
     /\ qAt'   = [q \in Children |-> IF q \in R2 THEN NoThread ELSE qAt[q]]
     /\ qVis'  = [q \in Children |-> IF q \in R2 THEN {} ELSE qVis[q]]
     /\ qWake' = (qWake \ R2) \cup (IF MaxQ > 0 THEN CU ELSE {})
     /\ UNCHANGED <<qUsed, answers, qEp, qHe, qLeft, az>>
     /\ liveSet' = [liveSet EXCEPT ![t] = @ \ {r}]
     /\ intReq'  = [intReq EXCEPT ![t] = FALSE]
     /\ held'    = [held EXCEPT ![t] = IF out = "failed" /\ @ # "stop" THEN "fail" ELSE @]
     /\ waitOn'  = [waitOn EXCEPT ![t] = NoThread]
     /\ replied' = [replied EXCEPT ![t] = FALSE]
     /\ readRep' = [readRep EXCEPT ![t] = NoThread]
     /\ UNCHANGED <<dupv, ackSeen>>
     /\ IF S # {}
        THEN /\ ZPlan(t, r, S, FALSE, ob0)
             /\ UNCHANGED cDisp
        ELSE /\ obx' = [obx EXCEPT ![Host(t)] = ob0]
             /\ UNCHANGED <<tDel, cDisp, cSt, cDel, cGen>>

EndRunUnchanged ==
  UNCHANGED <<created, parent, depth, access, rtask, tWake, tCoh, pushes, kidIn, rord, doomed,
              hq, cmds, orchOut, rc, mark, jr, jseq, prov, up>>

RootComplete(t) ==
  /\ Awake(t) /\ Running(t) /\ waitOn[t] = NoThread /\ ~Blocked(t)
  /\ ZEndRun(t, "completed")
  /\ EndRunUnchanged /\ UNCHANGED zbud

RootFail(t) ==
  /\ Awake(t) /\ Running(t) /\ failLeft > 0
  /\ ZEndRun(t, "failed")
  /\ failLeft' = failLeft - 1
  /\ EndRunUnchanged
  /\ UNCHANGED <<personRows, personLeft, stopLeft, crashes, offs, dispatches>>

ConfirmInterrupt(t) ==
  /\ Awake(t) /\ Running(t) /\ intReq[t]
  /\ ZEndRun(t, "interrupted")
  /\ EndRunUnchanged /\ UNCHANGED zbud

-----------------------------------------------------------------------------
(* Delegation on host(t) *)

\* delegate_task from the live run's harness (MCP). The child row and its first Run row are effects.
Delegate(t, c, mode, a) ==
  /\ Awake(t) /\ Running(t) /\ ~intReq[t] /\ waitOn[t] = NoThread /\ ~Blocked(t)
  /\ c \in Children /\ ~created[c]
  /\ depth[t] + 1 <= MaxDepth
  /\ Cardinality(UnsettledKids(t)) < MaxFanout
  /\ a <= EffAcc(t)                                   \* cap_access on host(t): config and last ceiling
  /\ ceil' = IF MaxNarrow > 0 THEN [ceil EXCEPT ![c] = EffAcc(t)] ELSE ceil   \* CreateChild carries the ceiling
  /\ created' = [created EXCEPT ![c] = TRUE]
  /\ parent'  = [parent EXCEPT ![c] = t]
  /\ depth'   = [depth EXCEPT ![c] = depth[t] + 1]
  /\ access'  = [access EXCEPT ![c] = a]
  /\ tWake'   = [tWake EXCEPT ![c] = IF mode = "wait" THEN "settled_only" ELSE "always"]
  /\ tCoh'    = [tCoh EXCEPT ![c] = rord[t]]
  /\ waitOn'  = [waitOn EXCEPT ![t] = IF mode = "wait" THEN c ELSE NoThread]
  /\ doomed'  = IF t \in doomed THEN doomed \cup {c} ELSE doomed
  /\ obx'     = AddObx(Host(t), {Eff("create", c, 0, 0)})
  /\ UNCHANGED <<rtask, tDel, pushes, kidIn, cDisp, cSt, cDel, cGen, held, liveSet, rord, intReq,
                 hq, cmds, orchOut, rc, mark, jr, jseq, prov, replied, up, zbud,
                 xv, qz, runAcc, aCmd, aDone, narrowLeft, narrowsAbs>>

\* Effect worker of host(parent): registry row + the child's first Run row, then mark done.
CreateWrite(c) ==
  LET d == Host(parent[c]) IN
  /\ created[c] /\ up[d] /\ Eff("create", c, 0, 0) \in obx[d] /\ TaskId(c) \notin cmds[c]
  /\ cmds' = [cmds EXCEPT ![c] = @ \cup {TaskId(c)}]
  /\ UNCHANGED <<mirror, hq, orchOut, rc, mark, obx, jr, jseq, prov, replied, up, zbud>>

CreateMark(c) ==
  LET d == Host(parent[c]) IN
  /\ created[c] /\ up[d] /\ Eff("create", c, 0, 0) \in obx[d] /\ TaskId(c) \in cmds[c]
  /\ obx' = [obx EXCEPT ![d] = @ \ {Eff("create", c, 0, 0)}]
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, rc, mark, jr, jseq, prov, replied, up, zbud>>

\* Effect worker of host(c): PublishUp appends the notice to c's own orch_out, then marks done.
PublishWrite(c, r) ==
  LET d == Host(c) IN
  /\ up[d] /\ Eff("publish", c, r, 0) \in obx[d] /\ r \notin orchOut[c]
  /\ orchOut' = [orchOut EXCEPT ![c] = @ \cup {r}]
  /\ UNCHANGED <<mirror, hq, cmds, rc, mark, obx, jr, jseq, prov, replied, up, zbud>>

PublishMark(c, r) ==
  LET d == Host(c) IN
  /\ up[d] /\ Eff("publish", c, r, 0) \in obx[d] /\ r \in orchOut[c]
  /\ obx' = [obx EXCEPT ![d] = @ \ {Eff("publish", c, r, 0)}]
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, rc, mark, jr, jseq, prov, replied, up, zbud>>

\* Effect worker of host(parent): WriteDown appends the Orch Stop row to c's ledger, then marks done.
StopWrite(c, s) ==
  LET d == Host(parent[c]) IN
  /\ created[c] /\ up[d] /\ Eff("stop", c, s, 0) \in obx[d] /\ StopId(c, s) \notin cmds[c]
  /\ cmds' = [cmds EXCEPT ![c] = @ \cup {StopId(c, s)}]
  /\ UNCHANGED <<mirror, hq, orchOut, rc, mark, obx, jr, jseq, prov, replied, up, zbud>>

StopMark(c, s) ==
  LET d == Host(parent[c]) IN
  /\ created[c] /\ up[d] /\ Eff("stop", c, s, 0) \in obx[d] /\ StopId(c, s) \in cmds[c]
  /\ obx' = [obx EXCEPT ![d] = @ \ {Eff("stop", c, s, 0)}]
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, rc, mark, jr, jseq, prov, replied, up, zbud>>

\* T2 DoNotice at host(p): ingest from the child's orch_out, receipt + settle + plan in one step.
IngestNotice(p, c, r) ==
  /\ Awake(p) /\ c \in Kids(p) /\ r \in orchOut[c]
  /\ ZM("NoDedupe") \/ NoticeId(c, r) \notin rc[p]
  /\ rc' = [rc EXCEPT ![p] = @ \cup {NoticeId(c, r)}]
  /\ kidIn' = [kidIn EXCEPT ![c] = @ \/ \E k \in Kids(c) : pushes[k] >= 1]
  /\ ZPlan(p, tCoh[c], IF ZM("NoDedupe") \/ tDel[c] = "pending" THEN {c} ELSE {},
           CohortLive(p, tCoh[c]), obx[Host(p)])
  /\ UNCHANGED <<created, parent, depth, access, rtask, waitOn, tWake, tCoh, pushes,
                 cDisp, held, liveSet, rord, intReq, doomed,
                 hq, cmds, orchOut, mark, jr, jseq, prov, replied, up, zbud>>

\* T4 DoOffer on host(p): an internal wake message, only if the delivery is current.
DoOffer(p, r, g) ==
  LET d == Host(p) IN
  /\ up[d] /\ Eff("offer", p, r, g) \in obx[d]
  /\ IF cSt[p][r] = "offered" /\ cGen[p][r] = g
     THEN /\ hq'  = [hq EXCEPT ![p] = Append(@, Msg("wake", r, g))]
          /\ cSt' = [cSt EXCEPT ![p][r] = "queued"]
     ELSE UNCHANGED <<hq, cSt>>
  /\ obx' = [obx EXCEPT ![d] = @ \ {Eff("offer", p, r, g)}]
  /\ UNCHANGED <<created, parent, depth, access, rtask, waitOn, tWake, tDel, tCoh, pushes, kidIn,
                 cDisp, cDel, cGen, held, liveSet, rord, intReq, doomed,
                 cmds, orchOut, rc, mark, jr, jseq, prov, replied, up, zbud>>

\* T7 task_status read by the live run, one step (ReadSplit = FALSE: the ack counts at reply time).
ReadResult(p, c) ==
  /\ ~ReadSplit /\ ~Blocked(p)
  /\ Awake(p) /\ Running(p) /\ c \in Kids(p) /\ Settled(c) /\ tDel[c] \in {"pending", "claimed"}
  /\ ZAckBy(c, TRUE)
  /\ ackSeen' = [ackSeen EXCEPT ![c] = TRUE]
  /\ UNCHANGED <<dupv, readRep>>
  /\ UNCHANGED <<created, parent, depth, access, rtask, waitOn, tWake, tCoh, kidIn, cDisp, cGen,
                 held, liveSet, rord, intReq, doomed,
                 cmds, orchOut, rc, mark, obx, jr, jseq, prov, replied, up, zbud>>

\* T8 split: the MCP reply is written (WaitReply), then the harness stream shows the ToolResult (WaitObserved).
WaitReply(t) ==
  /\ Awake(t) /\ waitOn[t] # NoThread /\ Running(t) /\ Settled(waitOn[t]) /\ ~replied[t]
  /\ replied' = [replied EXCEPT ![t] = TRUE]
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, rc, mark, obx, jr, jseq, prov, up, zbud>>

WaitObserved(t) ==
  LET c == waitOn[t] IN
  /\ Awake(t) /\ replied[t] /\ c # NoThread /\ Running(t)
  /\ IF tDel[c] \in {"pending", "claimed"}
     THEN ZAckBy(c, TRUE) /\ ackSeen' = [ackSeen EXCEPT ![c] = TRUE]
     ELSE UNCHANGED <<tDel, pushes, cDel, cSt, hq, ackSeen>>
  /\ waitOn'  = [waitOn EXCEPT ![t] = NoThread]
  /\ replied' = [replied EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<dupv, readRep>>
  /\ UNCHANGED <<created, parent, depth, access, rtask, tWake, tCoh, kidIn, cDisp, cGen,
                 held, liveSet, rord, intReq, doomed,
                 cmds, orchOut, rc, mark, obx, jr, jseq, prov, up, zbud>>

\* T9 timer (no reply written yet).
WaitTimeout(t) ==
  LET c == waitOn[t] IN
  /\ Awake(t) /\ c # NoThread /\ Running(t) /\ ~Settled(c) /\ ~replied[t]
  /\ ~(MaxQ > 0 /\ qAt[c] = t)                 \* t holds c's question: the wait returns it instead
  /\ tWake'  = [tWake EXCEPT ![c] = "always"]
  /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
  /\ UNCHANGED <<created, parent, depth, access, rtask, tDel, tCoh, pushes, kidIn,
                 cDisp, cSt, cDel, cGen, held, liveSet, rord, intReq, doomed, impl, zbud>>

(* T7 split (ReadSplit, DESIGN 3.2 "When an ack counts"): the MCP reply is  *)
(* written, then p's own harness stream shows the matching ToolResult in   *)
(* the same live run; only then does the ack count. A crash or a run end in *)
(* between loses readRep, so no ack: the RL10 sweep delivers it by wake.   *)
(* Mutant "AckAtReply" counts the ack at reply time (ACP rule).            *)
ReadReply(p, c) ==
  /\ ReadSplit /\ Awake(p) /\ Running(p) /\ readRep[p] = NoThread /\ ~Blocked(p)
  /\ c \in Kids(p) /\ Settled(c) /\ tDel[c] \in {"pending", "claimed"}
  /\ readRep' = [readRep EXCEPT ![p] = c]
  /\ IF ZM("AckAtReply") THEN ZAckBy(c, TRUE) ELSE UNCHANGED <<tDel, pushes, cDel, cSt, hq>>
  /\ UNCHANGED <<created, parent, depth, access, rtask, waitOn, tWake, tCoh, kidIn, cDisp, cGen,
                 held, liveSet, rord, intReq, doomed,
                 cmds, orchOut, rc, mark, obx, jr, jseq, prov, replied, up, zbud, dupv, ackSeen>>

ReadObserved(p) ==
  LET c == readRep[p] IN
  /\ ReadSplit /\ Awake(p) /\ Running(p) /\ c # NoThread
  /\ readRep' = [readRep EXCEPT ![p] = NoThread]
  /\ IF ~ZM("AckAtReply") /\ tDel[c] \in {"pending", "claimed"}
     THEN ZAckBy(c, TRUE) /\ ackSeen' = [ackSeen EXCEPT ![c] = TRUE]
     ELSE /\ UNCHANGED <<tDel, pushes, cDel, cSt, hq>>
          /\ ackSeen' = [ackSeen EXCEPT ![c] = @ \/ ZM("AckAtReply")]
  /\ UNCHANGED <<created, parent, depth, access, rtask, waitOn, tWake, tCoh, kidIn, cDisp, cGen,
                 held, liveSet, rord, intReq, doomed,
                 cmds, orchOut, rc, mark, obx, jr, jseq, prov, replied, up, zbud, dupv>>

-----------------------------------------------------------------------------
(* Questions (MaxQ > 0), DESIGN 3.3. Every step reads and writes one host.  *)
(* Upward rows (qup, qclosed) go into the sender's own orch_out and are     *)
(* addressed to a hop depth; downward rows (ans, rel) go into the next      *)
(* child's ledger toward host(q). Each receiver commits a receipt           *)
(* (q:c:qid:e style ids) with the step; a row with a receipt is not taken   *)
(* again. Epoch owner: host(q); a holder keeps the epoch of the QuestionUp  *)
(* it ingested (qHe). Mutants: "QNoReceipt" (ingest ignores the receipt),  *)
(* "NoAnswerCheck" (host(q) applies any answer).                            *)

\* Q1 + Q9 trigger 1 on host(c): publish QuestionUp; release every question c holds.
ZRaiseQuestion(c) ==
  LET R1 == {q \in Children : qAt[q] = c} IN
  /\ MaxQ > 0 /\ qLeft > 0 /\ c \in Children /\ Awake(c) /\ Running(c) /\ rtask[c]
  /\ ~qUsed[c] /\ ~intReq[c] /\ waitOn[c] = NoThread /\ readRep[c] = NoThread
  /\ qPend' = [qPend EXCEPT ![c] = TRUE]
  /\ qUsed' = [qUsed EXCEPT ![c] = TRUE]
  /\ qEp'   = [qEp EXCEPT ![c] = 1]
  /\ qLeft' = qLeft - 1
  /\ qAt'   = [q \in Children |-> IF q \in R1 THEN NoThread ELSE qAt[q]]
  /\ qVis'  = [q \in Children |-> IF q \in R1 THEN {} ELSE qVis[q]]
  /\ qWake' = qWake \ R1
  /\ qOut'  = [qOut EXCEPT ![c] = @ \cup {QRow("qup", c, 1, depth[c] - 1)}]
  /\ qCmd'  = [x \in Threads |-> qCmd[x] \cup {QRow("rel", q, qHe[q], depth[c]) :
                                               q \in {q2 \in R1 : AncAt(q2, depth[c] + 1) = x}}]
  /\ UNCHANGED <<mirror, impl, zbud, xv, answers, qHe, az>>

\* Q2 at hop h from child k's orch_out: relay a row addressed above h; if addressed to h,
\* escalate when h is blocked, else hold it (skip the wake while h's wait on q is active).
IngestQUp(h, k, row) ==
  LET q == row.q IN
  /\ MaxQ > 0 /\ Awake(h) /\ k \in Kids(h) /\ row \in qOut[k] /\ row.k = "qup"
  /\ ZM("QNoReceipt") \/ QId(row) \notin rc[h]
  /\ rc' = [rc EXCEPT ![h] = @ \cup {QId(row)}]
  /\ IF row.d < depth[h] \/ Blocked(h)
     THEN /\ qOut' = [qOut EXCEPT ![h] = @ \cup {QRow("qup", q, row.e, IF row.d < depth[h] THEN row.d ELSE depth[h] - 1)}]
          /\ UNCHANGED <<qAt, qVis, qWake, qHe>>
     ELSE /\ qAt'   = [qAt EXCEPT ![q] = h]
          /\ qVis'  = [qVis EXCEPT ![q] = {}]
          /\ qHe'   = [qHe EXCEPT ![q] = row.e]
          /\ qWake' = IF tWake[q] = "settled_only" /\ waitOn[h] = q THEN qWake ELSE qWake \cup {q}
          /\ UNCHANGED qOut
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, mark, obx, jr, jseq, prov, replied, up, zbud, xv,
                 qPend, qUsed, answers, qEp, qCmd, qLeft, az>>

\* Q10 at hop h: the holder drops its wake and view; any other hop passes it up; past the root it ends.
IngestQClosed(h, k, row) ==
  LET q == row.q IN
  /\ MaxQ > 0 /\ Awake(h) /\ k \in Kids(h) /\ row \in qOut[k] /\ row.k = "qclosed"
  /\ QId(row) \notin rc[h]
  /\ rc' = [rc EXCEPT ![h] = @ \cup {QId(row)}]
  /\ IF row.d = depth[h] /\ qAt[q] = h
     THEN /\ qAt'   = [qAt EXCEPT ![q] = NoThread]
          /\ qVis'  = [qVis EXCEPT ![q] = {}]
          /\ qWake' = qWake \ {q}
          /\ UNCHANGED qOut
     ELSE /\ qOut' = IF row.d < depth[h] THEN [qOut EXCEPT ![h] = @ \cup {row}]
                     ELSE IF h # Root THEN [qOut EXCEPT ![h] = @ \cup {QRow("qclosed", q, row.e, depth[h] - 1)}]
                     ELSE qOut
          /\ UNCHANGED <<qAt, qVis, qWake>>
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, mark, obx, jr, jseq, prov, replied, up, zbud, xv,
                 qPend, qUsed, answers, qEp, qHe, qCmd, qLeft, az>>

\* Q3: the question wake starts a run on its holder (queue_after_active).
ZStartQuestion(t, q) ==
  /\ MaxQ > 0 /\ Awake(t) /\ q \in qWake /\ qAt[q] = t
  /\ created[t] /\ ~Live(t) /\ WakeMayPass(t) /\ rord[t] < MaxRuns
  /\ liveSet' = [liveSet EXCEPT ![t] = @ \cup {rord[t] + 1}]
  /\ rord'    = [rord EXCEPT ![t] = @ + 1]
  /\ intReq'  = [intReq EXCEPT ![t] = FALSE]
  /\ rtask'   = [rtask EXCEPT ![t] = FALSE]
  /\ waitOn'  = [waitOn EXCEPT ![t] = NoThread]
  /\ obx'     = AddObx(Host(t), {Eff("start", t, rord[t] + 1, 0)})
  /\ qVis'    = [qVis EXCEPT ![q] = @ \cup {t}]
  /\ qWake'   = qWake \ {q}
  /\ SetRunAcc(t)
  /\ UNCHANGED <<created, parent, depth, access, tWake, tDel, tCoh, pushes, kidIn,
                 cDisp, cSt, cDel, cGen, held, doomed,
                 hq, cmds, orchOut, rc, mark, jr, jseq, prov, replied, up, zbud, xv,
                 qPend, qUsed, answers, qEp, qAt, qHe, qOut, qCmd, qLeft, azNoRun>>

\* Q4 (T8 question branch): the wait shows the held question; completionWake upgrades to always.
ZWaitReturnQuestion(t) ==
  LET c == waitOn[t] IN
  /\ MaxQ > 0 /\ Awake(t) /\ c # NoThread /\ Running(t) /\ ~Settled(c) /\ qAt[c] = t
  /\ qVis'   = [qVis EXCEPT ![c] = @ \cup {t}]
  /\ qWake'  = qWake \ {c}
  /\ tWake'  = [tWake EXCEPT ![c] = "always"]
  /\ waitOn' = [waitOn EXCEPT ![t] = NoThread]
  /\ UNCHANGED <<created, parent, depth, access, rtask, tDel, tCoh, pushes, kidIn,
                 cDisp, cSt, cDel, cGen, held, liveSet, rord, intReq, doomed, impl, zbud, xv,
                 qPend, qUsed, answers, qEp, qAt, qHe, qOut, qCmd, qLeft, az>>

\* Q5 on host(x): only the holder answers, with (qid, held epoch), into the next ledger down.
ZAgentAnswer(q, x) ==
  /\ MaxQ > 0 /\ Awake(x) /\ Running(x) /\ x \in qVis[q] /\ qAt[q] = x /\ ~Blocked(x)
  /\ qVis'  = [qVis EXCEPT ![q] = {}]
  /\ qAt'   = [qAt EXCEPT ![q] = NoThread]
  /\ qWake' = qWake \ {q}
  /\ qCmd'  = [qCmd EXCEPT ![AncAt(q, depth[x] + 1)] = @ \cup {QRow("ans", q, qHe[q], 0)}]
  /\ UNCHANGED <<mirror, impl, zbud, xv, qPend, qUsed, answers, qEp, qHe, qOut, qLeft, az>>

\* Q7 on host(q): the person answers in the child's UI; QuestionClosed goes up.
ZPersonAnswer(q) ==
  /\ MaxQ > 0 /\ Awake(q) /\ qPend[q] /\ qUsed[q] /\ answers[q] < 2
  /\ qPend'   = [qPend EXCEPT ![q] = FALSE]
  /\ answers' = [answers EXCEPT ![q] = @ + 1]
  /\ qOut'    = [qOut EXCEPT ![q] = @ \cup {QRow("qclosed", q, qEp[q], depth[q] - 1)}]
  /\ UNCHANGED <<mirror, impl, zbud, xv, qUsed, qEp, qAt, qVis, qWake, qHe, qCmd, qLeft, az>>

\* Host(k) drains an ans / rel row: relay it one hop down, or apply it if k = q.
\* Q6 / Q6': an answer applies only to the open qid at the current epoch (stale: no-op).
\* Q9: a release at the current epoch moves the epoch and sends q up from the releaser's parent.
DrainQRow(k, row) ==
  LET q == row.q
      ansOK == ZM("NoAnswerCheck") \/ (qPend[q] /\ qEp[q] = row.e)
      relOK == qPend[q] /\ qEp[q] = row.e /\ row.e < MaxGen
  IN
  /\ MaxQ > 0 /\ Awake(k) /\ row \in qCmd[k] /\ QId(row) \notin rc[k]
  /\ rc' = [rc EXCEPT ![k] = @ \cup {QId(row)}]
  /\ IF q # k
     THEN /\ qCmd' = [qCmd EXCEPT ![AncAt(q, depth[k] + 1)] = @ \cup {row}]
          /\ UNCHANGED <<qPend, answers, qEp, qOut>>
     ELSE IF row.k = "ans"
     THEN /\ qPend'   = IF ansOK THEN [qPend EXCEPT ![q] = FALSE] ELSE qPend
          /\ answers' = IF ansOK THEN [answers EXCEPT ![q] = Min(@ + 1, 2)] ELSE answers
          /\ qOut'    = IF ansOK THEN [qOut EXCEPT ![q] = @ \cup {QRow("qclosed", q, row.e, depth[q] - 1)}] ELSE qOut
          /\ UNCHANGED <<qEp, qCmd>>
     ELSE /\ qEp'  = IF relOK THEN [qEp EXCEPT ![q] = row.e + 1] ELSE qEp
          /\ qOut' = IF relOK /\ row.d > 0 THEN [qOut EXCEPT ![q] = @ \cup {QRow("qup", q, row.e + 1, row.d - 1)}] ELSE qOut
          /\ UNCHANGED <<qPend, answers, qCmd>>
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, mark, obx, jr, jseq, prov, replied, up, zbud, xv,
                 qUsed, qAt, qVis, qWake, qHe, qLeft, az>>

-----------------------------------------------------------------------------
(* Access clamp (MaxNarrow > 0), DESIGN 7 / 9.1 / R4. A person narrows      *)
(* chat t's config on host(t); host(t) reconfigures its live run at once    *)
(* (D3) and writes Clamp{ceiling, e} into each child's ledger. Each child   *)
(* host lowers its ceiling, reconfigures its live run, and forwards the     *)
(* clamp to its own children. Mutants: "NoClampSend" (no Clamp rows,        *)
(* DESIGN 9.1's S6 mutant), "ClampNoReconfigure" (the live run keeps its   *)
(* old access).                                                             *)
ZNarrowAccess(t) ==
  LET n == MaxNarrow - narrowLeft + 1
      v == Min(EffAcc(t), access[t] - 1)
  IN
  /\ MaxNarrow > 0 /\ narrowLeft > 0 /\ created[t] /\ Awake(t) /\ access[t] > 0
  /\ access'  = [access EXCEPT ![t] = @ - 1]
  /\ runAcc'  = [runAcc EXCEPT ![t] = Min(@, v)]
  /\ aCmd'    = IF ZM("NoClampSend") THEN aCmd
                ELSE [x \in Threads |-> IF x \in Kids(t) THEN aCmd[x] \cup {[e |-> n, v |-> v]} ELSE aCmd[x]]
  /\ narrowLeft' = narrowLeft - 1
  /\ narrowsAbs' = IF AbsAcc(t) = 1 THEN narrowsAbs + 1 ELSE narrowsAbs
  /\ UNCHANGED <<created, parent, depth, rtask, waitOn, tWake, tDel, tCoh, pushes, kidIn,
                 cDisp, cSt, cDel, cGen, held, liveSet, rord, intReq, doomed,
                 impl, zbud, xv, qz, ceil, aDone>>

DrainClamp(c, row) ==
  LET v == Min(ceil[c], row.v) IN
  /\ MaxNarrow > 0 /\ Awake(c) /\ row \in aCmd[c] /\ row.e \notin aDone[c]
  /\ aDone'  = [aDone EXCEPT ![c] = @ \cup {row.e}]
  /\ ceil'   = [ceil EXCEPT ![c] = v]
  /\ runAcc' = IF ZM("ClampNoReconfigure") THEN runAcc ELSE [runAcc EXCEPT ![c] = Min(@, v)]
  /\ aCmd'   = [x \in Threads |-> IF x \in Kids(c) THEN aCmd[x] \cup {[e |-> row.e, v |-> Min(v, access[c])]}
                                 ELSE aCmd[x]]
  /\ UNCHANGED <<mirror, impl, zbud, xv, qz, narrowLeft, narrowsAbs>>

-----------------------------------------------------------------------------
(* Duplicate rows (MaxDup). The relay and the sync road can both deliver a *)
(* row, with different batch ids, so the doc really holds two rows with    *)
(* one message id. The second copy is delivered after the first was       *)
(* applied, and the receipt check must drop it. Mutant "DupNoReceipt"      *)
(* applies the copy again.                                                  *)

DupRow(t, id) ==
  /\ dupLeft > 0 /\ id \in cmds[t] /\ id \notin dupC[t]
  /\ dupC' = [dupC EXCEPT ![t] = @ \cup {id}]
  /\ dupLeft' = dupLeft - 1
  /\ UNCHANGED <<mirror, impl, zbud, dmark, dupO, rdv>>

DupOut(c, r) ==
  /\ dupLeft > 0 /\ r \in orchOut[c] /\ r \notin dupO[c]
  /\ dupO' = [dupO EXCEPT ![c] = @ \cup {r}]
  /\ dupLeft' = dupLeft - 1
  /\ UNCHANGED <<mirror, impl, zbud, dupC, dmark, rdv>>

\* host(t) drains the second ledger copy: receipt present => drop (mark the copy).
DrainDupRow(t, id) ==
  /\ Awake(t) /\ id \in dupC[t] /\ id \notin dmark[t] /\ id \in rc[t]
  /\ dmark' = [dmark EXCEPT ![t] = @ \cup {id}]
  /\ IF ZM("DupNoReceipt") /\ id.k = "task"
     THEN hq' = [hq EXCEPT ![t] = <<Msg("task", 0, 0)>> \o @]
     ELSE UNCHANGED hq
  /\ UNCHANGED <<mirror, cmds, orchOut, rc, mark, obx, jr, jseq, prov, replied, up, zbud,
                 dupC, dupO, dupLeft, rdv>>

\* host(p) reads the second orch_out copy of notice (c, r): receipt present => drop.
IngestDupNotice(p, c, r) ==
  /\ Awake(p) /\ c \in Kids(p) /\ r \in dupO[c] /\ NoticeId(c, r) \in rc[p]
  /\ IF ZM("DupNoReceipt")
     THEN /\ ZPlan(p, tCoh[c], {c}, CohortLive(p, tCoh[c]), obx[Host(p)])
          /\ UNCHANGED <<created, parent, depth, access, rtask, waitOn, tWake, tCoh, pushes, kidIn,
                         cDisp, held, liveSet, rord, intReq, doomed>>
     ELSE UNCHANGED <<mirror, obx>>
  /\ UNCHANGED <<hq, cmds, orchOut, rc, mark, jr, jseq, prov, replied, up, zbud, xv>>

-----------------------------------------------------------------------------
(* Environment: per-device crash and offline *)

\* Volatile state of device d dies: MCP wait handlers and unobserved replies. Everything else is durable.
\* Only the threads d hosts lose their waits (maps to Delegation's partial CrashEngine).
DeviceCrash(d) ==
  /\ crashes[d] < MaxCrash
  /\ crashes' = [crashes EXCEPT ![d] = @ + 1]
  /\ waitOn'  = [t \in Threads |-> IF Host(t) = d THEN NoThread ELSE waitOn[t]]
  /\ replied' = [t \in Threads |-> IF Host(t) = d /\ ~ZM("CrashKeepsReply") THEN FALSE ELSE replied[t]]
  /\ readRep' = [t \in Threads |-> IF Host(t) = d THEN NoThread ELSE readRep[t]]
  /\ UNCHANGED <<dupv, ackSeen>>
  /\ UNCHANGED <<created, parent, depth, access, rtask, tWake, tDel, tCoh, pushes, kidIn,
                 cDisp, cSt, cDel, cGen, held, liveSet, rord, intReq, doomed,
                 hq, cmds, orchOut, rc, mark, obx, jr, jseq, prov, up,
                 personRows, personLeft, failLeft, stopLeft, offs, dispatches>>

GoOffline(d) ==
  /\ up[d] /\ offs[d] < MaxOff
  /\ up' = [up EXCEPT ![d] = FALSE]
  /\ offs' = [offs EXCEPT ![d] = @ + 1]
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, rc, mark, obx, jr, jseq, prov, replied,
                 personRows, personLeft, failLeft, stopLeft, crashes, dispatches>>

GoOnline(d) ==
  /\ ~up[d]
  /\ up' = [up EXCEPT ![d] = TRUE]
  /\ UNCHANGED <<mirror, hq, cmds, orchOut, rc, mark, obx, jr, jseq, prov, replied, zbud>>

-----------------------------------------------------------------------------
(* Steps that leave the item 2-5 variables alone. *)
ZCore ==
  \/ \E t \in Threads :
       \/ PersonWrite(t)
       \/ WaitReply(t) \/ WaitTimeout(t)
       \/ \E id \in cmds[t] : DrainPerson(t, id) \/ MarkLedger(t, id)
       \/ \E r \in Ords : DispatchRun(t, r) \/ DispatchMark(t, r)
  \/ \E c \in Children :
       \/ DrainTask(c) \/ CreateWrite(c) \/ CreateMark(c)
       \/ \E r \in Ords : PublishWrite(c, r) \/ PublishMark(c, r)
       \/ \E s \in StopIds : StopWrite(c, s) \/ StopMark(c, s)
  \/ \E p \in Threads, c \in Children : \E r \in Ords : IngestNotice(p, c, r)
  \/ \E p \in Threads, r \in Ords, g \in 1..MaxGen : DoOffer(p, r, g)
  \/ \E d \in Devs : GoOffline(d) \/ GoOnline(d)

ZNext ==
  \/ ZCore /\ UNCHANGED <<xv, qz, az>>
  \/ (\/ \E t \in Threads : PersonStop(t)
      \/ \E c \in Children, s \in StopIds : DrainStop(c, s)) /\ UNCHANGED <<xv, qzNoWake, az>>
  \/ \E t \in Threads : StartFromQueue(t) \/ StartFromQueueWake(t) \/ RootComplete(t) \/ RootFail(t)
                       \/ ConfirmInterrupt(t)
  \/ \E t \in Threads, c \in Children, m \in {"wait", "async"}, a \in Levels : Delegate(t, c, m, a)
  \/ (\/ \E t \in Threads :
         \/ WaitObserved(t) \/ ReadObserved(t)
         \/ \E id \in cmds[t] : DupRow(t, id) \/ DrainDupRow(t, id)
         \/ \E r \in orchOut[t] : DupOut(t, r)
      \/ \E p \in Threads, c \in Children :
         \/ ReadResult(p, c) \/ ReadReply(p, c)
         \/ \E r \in Ords : IngestDupNotice(p, c, r)
      \/ \E d \in Devs : DeviceCrash(d)) /\ UNCHANGED <<qz, az>>
  \/ \E t \in Threads :
       \/ ZWaitReturnQuestion(t) \/ ZNarrowAccess(t)
       \/ \E q \in Children : ZStartQuestion(t, q)
       \/ \E row \in qCmd[t] : DrainQRow(t, row)
       \/ \E row \in aCmd[t] : DrainClamp(t, row)
  \/ \E q \in Children : ZRaiseQuestion(q) \/ ZPersonAnswer(q) \/ \E x \in Threads : ZAgentAnswer(q, x)
  \/ \E h \in Threads, k \in Children : \E row \in qOut[k] : IngestQUp(h, k, row) \/ IngestQClosed(h, k, row)

ZSpec == ZInit /\ [][ZNext]_zvars

-----------------------------------------------------------------------------
(* ZeronImpl-level safety (Placement's S9/S10 analogues on the real mechanisms) *)

\* S9 / R17: a run's provider is dispatched at most once (journal gate).
ZI_NoDoubleDispatch == \A t \in Threads, r \in Ords : dispatches[t][r] <= 1
\* S10 / DESIGN 4: receipt-then-mark. A ledger mark never precedes its receipt.
ZI_ReceiptBeforeMark == \A t \in Threads : mark[t] \subseteq rc[t] /\ dmark[t] \subseteq rc[t]
\* Item 3 (S7 at the implementation level): an ack counts only once p's own harness stream showed
\* it, or while that observation is still pending in the same live run. A crash or run end between
\* reply and observation must leave the task unacknowledged.
ZI_AckObserved == \A c \in Children :
  tDel[c] = "acknowledged" => ackSeen[c] \/ (created[c] /\ readRep[parent[c]] = c)
\* S9: every receipt sits in the partition of the chat's own host (ids name chat t or t's children).
ZI_ReceiptsLocalOld == \A t \in Threads : \A id \in rc[t] :
                      id.t = t \/ (id.k = "notice" /\ created[id.t] /\ parent[id.t] = t)
\* S9: every receipt sits in the partition of the chat's own host. Question rows are receipted by
\* each hop on q's ancestor chain (upward rows) or on the path down to q (downward rows).
ZI_ReceiptsLocal == \A t \in Threads : \A id \in rc[t] :
  \/ id.t = t
  \/ id.k = "notice" /\ created[id.t] /\ parent[id.t] = t
  \/ id.k \in {"qup", "qclosed", "ans", "rel"} /\ created[id.t] /\ t \in Anc(id.t)
\* Item 4: the epoch a holder carries is host(q)'s epoch (one agent answerer per epoch), and an
\* answer in flight for an open question carries an epoch host(q) issued.
ZI_HeldEpoch == \A q \in Children : qAt[q] # NoThread => qHe[q] = qEp[q]
\* Item 5. EnforcedAccessOK (DESIGN 9.1, literal): a live run uses no more than its host enforces.
EnforcedAccessOK == \A t \in Threads : Live(t) => runAcc[t] <= EffAcc(t)
\* Item 5, stronger: wherever host(d) enforces more than the abstract access (min of the configs
\* on d's chain), a Clamp for it is still in flight on that chain. Catches a Clamp never sent.
ZI_ClampInFlight == \A d \in Threads : created[d] /\ EffAcc(d) > AbsAcc(d) =>
  \E x \in Chain(d) : \E row \in aCmd[x] : row.e \notin aDone[x]
\* The live run never exceeds the abstract access once no clamp is owed.
ZI_RunAccessOK == \A t \in Threads : Live(t) /\ runAcc[t] > AbsAcc(t) =>
  \E x \in Chain(t) : \E row \in aCmd[x] : row.e \notin aDone[x]
ZI_BoundsNotHit == \A t \in Threads : rord[t] < MaxRuns
=============================================================================
