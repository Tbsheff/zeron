---------------------------- MODULE Placement ----------------------------
(***************************************************************************)
(* Zeron Orchestrator v2, module Placement (AC-M1).                        *)
(*                                                                         *)
(* Devices: laptops and Cloud session machines. Each chat has one host     *)
(* device (Chat.device_id). Commands are rows in a synced per-chat ledger  *)
(* (Loro doc behind a chat2 edge room): rows replicate eventually, in any  *)
(* order, possibly twice. Only the host drains and executes. A second road *)
(* (RELAY_COMMAND over the device relay) carries the same row straight to  *)
(* the host; both roads claim the id in the host's processed ledger        *)
(* (mark-before-execute, doc_host.rs:5375 / ingest_relayed_command).       *)
(*                                                                         *)
(* Flow: a person on UserDev sends "run" to parent chat P (host            *)
(* ParentHost). The parent run delegates: it writes "kick" into child chat *)
(* K (host ChildHost, another device or Cloud). The child finishes and     *)
(* writes a done row in K. The result reaches P as command "res" with the  *)
(* fixed id ResId = (child, child run) (R6), by the pull road (P's host    *)
(* reads K's room and settles) and/or the push road (K's host writes into  *)
(* P's room).                                                              *)
(*                                                                         *)
(* Mode = "zeron": today's rules (id-only ledger, crash sweep rejects,     *)
(*   every Run carries the TTL, no dispatch gate on the message id).       *)
(* Mode = "v2": the proposed rules (ledger also keyed on the message id,   *)
(*   crash replay completes from the run journal, orchestrator commands    *)
(*   carry no TTL, Cloud sleeps only with a flushed outbox).               *)
(* HostMove: "off" (Zeron today), "naive" or "safe" (future handover).     *)
(* No clocks: order comes from per-device sequence numbers (ids) and set   *)
(* membership only.                                                        *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
  Laptops, Cloud,          \* device ids (strings)
  ParentHost, ChildHost,   \* hosts of P and K
  UserDev,                 \* where the person sends "run" from
  PHolders,                \* laptops that hold a replica of P
  Mode, ResultRoad, HostMove, MoveTarget,
  CrashBudget, OfflineBudget, ExpireBudget, RetryBudget, DupBudget, MaxSeq,
  RollbackBudget,
  OffDevs,                 \* devices that may go offline / Cloud that may sleep
  Mutant

Dev      == Laptops \cup Cloud
NoDev    == "none"
Chats    == {"P", "K"}
Mids     == {"run", "kick", "res"}
OrchMids == {"kick", "res"}
Target(m) == IF m = "kick" THEN "K" ELSE "P"
ResId    == <<"K", 1>>
NoId     == <<"none", 0>>

Mutants == {"none", "NoLedgerCheck", "NoHostCheck", "NoMidGate", "ReplayNoRanCheck",
            "ReplayRequeue", "NoWakeReceipt", "VolatileEmit", "SleepUnflushed", "NoDocGate",
            "SleepUndiscovered"}

ASSUME Mode \in {"v2", "zeron"}
ASSUME ResultRoad \in {"pull", "push", "both"}
ASSUME HostMove \in {"off", "naive", "safe"}
ASSUME Mutant \in Mutants
ASSUME ParentHost \in Laptops /\ ChildHost \in Dev /\ ChildHost # ParentHost
ASSUME UserDev \in PHolders /\ PHolders \subseteq Laptops
ASSUME MoveTarget \in Laptops /\ MoveTarget # ParentHost

MidGate  == Mode = "v2" /\ Mutant # "NoMidGate"
\* v2: the doc (not the rollback-able SQLite ledger) says whether a message already ran.
DocGate  == Mode = "v2" /\ Mutant # "NoDocGate"
PushRoad == ResultRoad \in {"push", "both"}
PullRoad == ResultRoad \in {"pull", "both"}

Holders(c) ==
  IF c = "P"
  THEN PHolders \cup {ParentHost} \cup (IF PushRoad THEN {ChildHost} ELSE {})
                \cup (IF HostMove # "off" THEN {MoveTarget} ELSE {})
  ELSE {ParentHost, ChildHost} \cup (IF HostMove # "off" THEN {MoveTarget} ELSE {})

\* Rows share one record shape so any two rows compare.
Cmd(id, m) == [t |-> "cmd", id |-> id, m |-> m]
Out(id, o) == [t |-> "out", id |-> id, m |-> o]
MsgRow(m)  == [t |-> "msg", id |-> NoId, m |-> m]   \* user/result message landed in transcript
DoneRow    == [t |-> "done", id |-> NoId, m |-> "K"] \* child run finished

HasOut(R, id) == \E r \in R : r.t = "out" /\ r.id = id
AttIds(R, m)  == {r.id : r \in {x \in R : x.t = "cmd" /\ x.m = m}}

InitHost == [c \in Chats |-> IF c = "P" THEN ParentHost ELSE ChildHost]
Cap2(n)  == IF n >= 2 THEN 2 ELSE n + 1

VARIABLES
  edge,       \* [Chats -> rows] chat2 room log (durable, edge)
  rep,        \* [Dev -> [Chats -> rows]] local doc replica (durable, device)
  online,     \* [Dev -> BOOLEAN] connected; Cloud offline = asleep
  host,       \* [Chats -> Dev \cup {NoDev}] registry truth
  view,       \* [Dev -> [Chats -> Dev \cup {NoDev}]] each device's registry view
  processed,  \* [Dev -> SUBSET Id] processed-command ledger (durable, local SQLite)
  procMid,    \* [Dev -> SUBSET Mids] v2: ledger keyed on message id
  journal,    \* [Dev -> SUBSET Mids] run journal: mids this device dispatched (durable)
  executing,  \* [Dev -> SUBSET Id] claimed, not yet dispatched (volatile)
  inflight,   \* [Dev -> SUBSET Id] dispatched, outcome not yet written (volatile)
  net,        \* relay RELAY_COMMAND messages in flight
  relayLog,   \* <<id, to>> pairs an escort already sent
  wake,       \* SUBSET (Dev \X Chats): durable wake / discovery receipts (host, chat)
  issued,     \* SUBSET Mids (ghost: which logical commands exist)
  child,      \* "idle" | "running" | "done" (child run, durable via its journal)
  emitDue,    \* VolatileEmit mutant only: push-road emit pending in memory
  handoff,    \* safe HostMove: release record at the edge
  dispatched, \* [Mids -> 0..2] ghost: provider dispatches (the irreversible effect)
  execBy,     \* ghost: <<id, device>> pairs that dispatched
  badHost,    \* ghost: some device dispatched a chat it does not host
  seq,        \* [Dev -> Nat] per-device sequence numbers (attempt ids <<dev, n>>)
  crashes, offs, expires, retries, dups, moves, rollbacks

vars == <<edge, rep, online, host, view, processed, procMid, journal, executing,
          inflight, net, relayLog, wake, issued, child, emitDue, handoff,
          dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves,
          rollbacks>>

NoHandoff == [on |-> FALSE, ids |-> {}, mids |-> {}, jr |-> {}, rows |-> {}]

Init ==
  /\ edge = [c \in Chats |-> {}]
  /\ rep = [d \in Dev |-> [c \in Chats |-> {}]]
  /\ online = [d \in Dev |-> TRUE]
  /\ host = InitHost
  /\ view = [d \in Dev |-> InitHost]
  /\ processed = [d \in Dev |-> {}]
  /\ procMid = [d \in Dev |-> {}]
  /\ journal = [d \in Dev |-> {}]
  /\ executing = [d \in Dev |-> {}]
  /\ inflight = [d \in Dev |-> {}]
  /\ net = {}
  /\ relayLog = {}
  /\ wake = {}
  /\ issued = {}
  /\ child = "idle"
  /\ emitDue = FALSE
  /\ handoff = NoHandoff
  /\ dispatched = [m \in Mids |-> 0]
  /\ execBy = {}
  /\ badHost = FALSE
  /\ seq = [d \in Dev |-> 0]
  /\ crashes = 0
  /\ offs = [d \in Dev |-> 0]
  /\ expires = 0
  /\ retries = 0
  /\ dups = 0
  /\ moves = 0
  /\ rollbacks = 0

---------------------------------------------------------------------------
(* Shared pieces *)

LedgerFree(h, id, m) == id \notin processed[h] /\ (MidGate => m \notin procMid[h])

ClaimGuard(h, c, id, m) ==
  /\ IF Mutant = "NoHostCheck" THEN h \in Holders(c) ELSE view[h][c] = h
  /\ ~HasOut(rep[h][c], id)
  /\ (Mutant = "NoLedgerCheck" \/ LedgerFree(h, id, m))
  /\ id \notin executing[h] \cup inflight[h]
  /\ DocGate => MsgRow(m) \notin rep[h][c]

DupGuard(h, c, id, m) ==
  /\ MidGate /\ Mutant # "NoLedgerCheck"
  /\ view[h][c] = h
  /\ ~HasOut(rep[h][c], id)
  /\ id \notin processed[h]
  /\ m \in procMid[h]

ClaimEff(h, id, m) ==
  /\ processed' = [processed EXCEPT ![h] = @ \cup {id}]
  /\ procMid' = IF MidGate THEN [procMid EXCEPT ![h] = @ \cup {m}] ELSE procMid
  /\ executing' = [executing EXCEPT ![h] = @ \cup {id}]

DispatchEff(h, id, m) ==
  /\ dispatched' = [dispatched EXCEPT ![m] = Cap2(@)]
  /\ journal' = [journal EXCEPT ![h] = @ \cup {m}]
  /\ execBy' = execBy \cup {<<id, h>>}
  /\ badHost' = (badHost \/ host[Target(m)] # h)
  /\ child' = IF m = "kick" /\ child = "idle" THEN "running" ELSE child

\* Replay candidates: claimed by this device's ledger, no outcome, not live.
ReplayDue(h, c, id) ==
  /\ view[h][c] = h
  /\ id \in processed[h]
  /\ ~HasOut(rep[h][c], id)
  /\ id \notin executing[h] \cup inflight[h]

\* Did this message already run here? Journal (SQLite) or, v2, the landed message in the doc.
RanBefore(h, c, m) == m \in journal[h] \/ (DocGate /\ MsgRow(m) \in rep[h][c])

\* A stopped (asleep) Cloud machine runs nothing; an offline laptop still runs locally.
Awake(d) == d \in Laptops \/ online[d]

Flushed(d) == \A c \in Chats : rep[d][c] \subseteq edge[c]

Idle(d) ==
  /\ executing[d] \cup inflight[d] = {}
  /\ \A c \in Chats : \A r \in rep[d][c] : r.t = "cmd" => ~ReplayDue(d, c, r.id)
  /\ ~(d = ChildHost /\ (child = "running" \/ emitDue))
  /\ \A c \in Chats : view[d][c] = d =>
        \A r \in rep[d][c] : r.t = "cmd" => (r.id \in processed[d] \/ HasOut(rep[d][c], r.id))
  /\ (Mode = "v2" /\ Mutant # "SleepUnflushed") => Flushed(d)
  /\ (Mode = "v2" /\ Mutant # "SleepUndiscovered") => \A c \in Chats : <<d, c>> \notin wake

---------------------------------------------------------------------------
(* Origin of commands *)

IssueRun ==
  /\ "run" \notin issued
  /\ seq[UserDev] < MaxSeq
  /\ rep' = [rep EXCEPT ![UserDev]["P"] = @ \cup {Cmd(<<UserDev, seq[UserDev] + 1>>, "run")}]
  /\ seq' = [seq EXCEPT ![UserDev] = @ + 1]
  /\ issued' = issued \cup {"run"}
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, child, emitDue, handoff,
                 dispatched, execBy, badHost, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* The parent run (on P's host, where "run" landed and ran) delegates to K.
IssueKick(d) ==
  /\ "kick" \notin issued
  /\ view[d]["P"] = d
  /\ MsgRow("run") \in rep[d]["P"]
  /\ Awake(d)
  /\ d \in Holders("K")
  /\ seq[d] < MaxSeq
  /\ rep' = [rep EXCEPT ![d]["K"] = @ \cup {Cmd(<<d, seq[d] + 1>>, "kick")}]
  /\ seq' = [seq EXCEPT ![d] = @ + 1]
  /\ issued' = issued \cup {"kick"}
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, child, emitDue, handoff,
                 dispatched, execBy, badHost, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* Child run finishes on K's host; done row (and, push road, the result row) in one commit.
ChildFinish(h) ==
  /\ Awake(h)
  /\ view[h]["K"] = h
  /\ child = "running"
  /\ child' = "done"
  /\ IF PushRoad /\ Mutant # "VolatileEmit"
     THEN /\ rep' = [rep EXCEPT ![h]["K"] = @ \cup {DoneRow}, ![h]["P"] = @ \cup {Cmd(ResId, "res")}]
          /\ issued' = issued \cup {"res"}
          /\ emitDue' = emitDue
     ELSE /\ rep' = [rep EXCEPT ![h]["K"] = @ \cup {DoneRow}]
          /\ issued' = issued
          /\ emitDue' = (PushRoad /\ Mutant = "VolatileEmit")
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* Mutant VolatileEmit: the push-road result write is a follow-up held in memory.
EmitResult(h) ==
  /\ emitDue /\ h = ChildHost /\ Awake(h)
  /\ rep' = [rep EXCEPT ![h]["P"] = @ \cup {Cmd(ResId, "res")}]
  /\ issued' = issued \cup {"res"}
  /\ emitDue' = FALSE
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, child, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* Pull road: P's host sees K's done row and writes the result command (fixed id).
RemoteChildResult(d) ==
  /\ PullRoad
  /\ view[d]["P"] = d
  /\ Awake(d)
  /\ DoneRow \in rep[d]["K"]
  /\ Cmd(ResId, "res") \notin rep[d]["P"]
  /\ rep' = [rep EXCEPT ![d]["P"] = @ \cup {Cmd(ResId, "res")}]
  /\ issued' = issued \cup {"res"}
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, child, emitDue, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

---------------------------------------------------------------------------
(* Sync: chat2 room and device room *)

\* Push the local outbox. The room commits rows and the host wake receipt in
\* one transaction before the ACK (remote-send-delivery.md).
Push(d, c) ==
  /\ online[d]
  /\ d \in Holders(c)
  /\ ~(rep[d][c] \subseteq edge[c])
  /\ edge' = [edge EXCEPT ![c] = @ \cup rep[d][c]]
  /\ wake' = IF /\ Mutant # "NoWakeReceipt"
                /\ host[c] \in Dev /\ host[c] # d
                /\ \E r \in rep[d][c] \ edge[c] : r.t = "cmd"
             THEN wake \cup {<<host[c], c>>} ELSE wake
  /\ UNCHANGED <<rep, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* Import the room (Loro import: union, re-import is a no-op). Clears that chat's receipt.
Pull(d, c) ==
  /\ online[d]
  /\ d \in Holders(c)
  /\ ~(edge[c] \subseteq rep[d][c])
  /\ rep' = [rep EXCEPT ![d][c] = @ \cup edge[c]]
  /\ wake' = wake \ {<<d, c>>}
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

GoOffline(d) ==
  /\ online[d]
  /\ d \in OffDevs
  /\ offs[d] < OfflineBudget
  /\ d \in Cloud => Idle(d)
  /\ online' = [online EXCEPT ![d] = FALSE]
  /\ offs' = [offs EXCEPT ![d] = @ + 1]
  /\ UNCHANGED <<edge, rep, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, seq, crashes, expires, retries, dups, moves, rollbacks>>

\* Laptops come back on their own; a sleeping Cloud machine wakes only on a receipt.
GoOnline(d) ==
  /\ ~online[d]
  /\ d \in Laptops \/ \E c \in Chats : <<d, c>> \in wake
  /\ online' = [online EXCEPT ![d] = TRUE]
  /\ UNCHANGED <<edge, rep, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* Delivery escort: forward a pending row over the relay to the believed host.
RelaySend(d, id, m) ==
  LET c == Target(m) IN
  /\ m = "run"     \* RELAY_COMMAND carries a person's send; orchestrator entries ride docs only
  /\ online[d]
  /\ Cmd(id, m) \in rep[d][c]
  /\ ~HasOut(rep[d][c], id)
  /\ view[d][c] \in Dev /\ view[d][c] # d
  /\ <<id, view[d][c]>> \notin relayLog
  /\ net' = net \cup {[to |-> view[d][c], id |-> id, m |-> m]}
  /\ relayLog' = relayLog \cup {<<id, view[d][c]>>}
  /\ UNCHANGED <<edge, rep, online, host, view, processed, procMid, journal, executing,
                 inflight, wake, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* Host side of RELAY_COMMAND: claim the forwarded id, adopt the row. A copy may stay (duplicate).
IngestClaim(msg) ==
  LET h == msg.to  c == Target(msg.m) IN
  /\ online[h]
  /\ ClaimGuard(h, c, msg.id, msg.m)
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {Cmd(msg.id, msg.m)}]
  /\ ClaimEff(h, msg.id, msg.m)
  /\ \/ /\ net' = net \ {msg}
        /\ dups' = dups
     \/ /\ dups < DupBudget
        /\ net' = net
        /\ dups' = dups + 1
  /\ UNCHANGED <<edge, online, host, view, journal, inflight, relayLog, wake, issued,
                 child, emitDue, handoff, dispatched, execBy, badHost, seq, crashes,
                 offs, expires, retries, moves, rollbacks>>

IngestDup(msg) ==
  LET h == msg.to  c == Target(msg.m) IN
  /\ online[h]
  /\ DupGuard(h, c, msg.id, msg.m)
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {Cmd(msg.id, msg.m), Out(msg.id, "dup")}]
  /\ processed' = [processed EXCEPT ![h] = @ \cup {msg.id}]
  /\ net' = net \ {msg}
  /\ UNCHANGED <<edge, online, host, view, procMid, journal, executing, inflight, relayLog,
                 wake, issued, child, emitDue, handoff, dispatched, execBy, badHost, seq,
                 crashes, offs, expires, retries, dups, moves, rollbacks>>

IngestDrop(msg) ==
  LET h == msg.to  c == Target(msg.m) IN
  /\ online[h]
  /\ ~ClaimGuard(h, c, msg.id, msg.m)
  /\ ~DupGuard(h, c, msg.id, msg.m)
  /\ net' = net \ {msg}
  /\ UNCHANGED <<edge, rep, online, host, view, processed, procMid, journal, executing,
                 inflight, relayLog, wake, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* Loss happens only while the target is offline (assumption H).
RelayLose(msg) ==
  /\ ~online[msg.to]
  /\ net' = net \ {msg}
  /\ UNCHANGED <<edge, rep, online, host, view, processed, procMid, journal, executing,
                 inflight, relayLog, wake, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

---------------------------------------------------------------------------
(* Host drain *)

\* Mark BEFORE executing (doc_host.rs:5375).
Claim(h, id, m) ==
  LET c == Target(m) IN
  /\ Awake(h)
  /\ Cmd(id, m) \in rep[h][c]
  /\ ClaimGuard(h, c, id, m)
  /\ ClaimEff(h, id, m)
  /\ UNCHANGED <<edge, rep, online, host, view, journal, inflight, net, relayLog, wake,
                 issued, child, emitDue, handoff, dispatched, execBy, badHost, seq,
                 crashes, offs, expires, retries, dups, moves, rollbacks>>

\* v2: another attempt of this message id already claimed here: resolve as duplicate.
ClaimDup(h, id, m) ==
  LET c == Target(m) IN
  /\ Awake(h)
  /\ Cmd(id, m) \in rep[h][c]
  /\ DupGuard(h, c, id, m)
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {Out(id, "dup")}]
  /\ processed' = [processed EXCEPT ![h] = @ \cup {id}]
  /\ UNCHANGED <<edge, online, host, view, procMid, journal, executing, inflight, net,
                 relayLog, wake, issued, child, emitDue, handoff, dispatched, execBy,
                 badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* v2: the message already landed in the doc (it ran): record the outcome, never run again.
ClaimLanded(h, id, m) ==
  LET c == Target(m) IN
  /\ Awake(h)
  /\ DocGate
  /\ Cmd(id, m) \in rep[h][c]
  /\ view[h][c] = h
  /\ ~HasOut(rep[h][c], id)
  /\ id \notin processed[h]
  /\ id \notin executing[h] \cup inflight[h]
  /\ MsgRow(m) \in rep[h][c]
  /\ ~DupGuard(h, c, id, m)
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {Out(id, "applied")}]
  /\ processed' = [processed EXCEPT ![h] = @ \cup {id}]
  /\ UNCHANGED <<edge, online, host, view, procMid, journal, executing, inflight, net,
                 relayLog, wake, issued, child, emitDue, handoff, dispatched, execBy,
                 badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* Update-trial rollback (ARCHITECTURE \S 1, DESIGN \S 2): a crash that also restores
\* the SQLite stores (ledger, receipts, journal) to empty; synced docs are untouched.
Rollback(d) ==
  /\ rollbacks < RollbackBudget
  /\ rollbacks' = rollbacks + 1
  /\ processed' = [processed EXCEPT ![d] = {}]
  /\ procMid' = [procMid EXCEPT ![d] = {}]
  /\ journal' = [journal EXCEPT ![d] = {}]
  /\ executing' = [executing EXCEPT ![d] = {}]
  /\ inflight' = [inflight EXCEPT ![d] = {}]
  /\ UNCHANGED <<edge, rep, online, host, view, net, relayLog, wake, issued, child,
                 emitDue, handoff, dispatched, execBy, badHost, seq, crashes, offs,
                 expires, retries, dups, moves>>

\* write_user_message (idempotent by message id) then dispatch (no message-id gate today).
Exec(h, id, m) ==
  LET c == Target(m) IN
  /\ Awake(h)
  /\ id \in executing[h]
  /\ Cmd(id, m) \in rep[h][c]
  /\ DispatchEff(h, id, m)
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {MsgRow(m)}]
  /\ executing' = [executing EXCEPT ![h] = @ \ {id}]
  /\ inflight' = [inflight EXCEPT ![h] = @ \cup {id}]
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, net, relayLog, wake,
                 issued, emitDue, handoff, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

Resolve(h, id, m) ==
  LET c == Target(m) IN
  /\ Awake(h)
  /\ id \in inflight[h]
  /\ Cmd(id, m) \in rep[h][c]
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {Out(id, "applied")}]
  /\ inflight' = [inflight EXCEPT ![h] = @ \ {id}]
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing, net,
                 relayLog, wake, issued, child, emitDue, handoff, dispatched, execBy,
                 badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* TTL: evaluate_command marks the id and resolves Expired. v2: orchestrator commands have no TTL.
Expire(h, id, m) ==
  LET c == Target(m) IN
  /\ Awake(h)
  /\ expires < ExpireBudget
  /\ Mode = "zeron" \/ m \notin OrchMids
  /\ view[h][c] = h
  /\ Cmd(id, m) \in rep[h][c]
  /\ ~HasOut(rep[h][c], id)
  /\ id \notin processed[h]
  /\ id \notin executing[h] \cup inflight[h]
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {Out(id, "expired")}]
  /\ processed' = [processed EXCEPT ![h] = @ \cup {id}]
  /\ expires' = expires + 1
  /\ UNCHANGED <<edge, online, host, view, procMid, journal, executing, inflight, net,
                 relayLog, wake, issued, child, emitDue, handoff, dispatched, execBy,
                 badHost, seq, crashes, offs, retries, dups, moves, rollbacks>>

\* Engine crash and restart: volatile state is gone; durable ledger, journal, doc stay.
Crash(d) ==
  /\ crashes < CrashBudget
  /\ crashes' = crashes + 1
  /\ executing' = [executing EXCEPT ![d] = {}]
  /\ inflight' = [inflight EXCEPT ![d] = {}]
  /\ emitDue' = IF d = ChildHost THEN FALSE ELSE emitDue
  /\ UNCHANGED <<edge, rep, online, host, view, processed, procMid, journal, net, relayLog,
                 wake, issued, child, handoff, dispatched, execBy, badHost, seq,
                 offs, expires, retries, dups, moves, rollbacks>>

\* Zeron: dead-command sweep (doc_host.rs:5265) rejects a consumed-but-unresolved id.
Sweep(h, id, m) ==
  LET c == Target(m) IN
  /\ Awake(h)
  /\ Mode = "zeron"
  /\ Cmd(id, m) \in rep[h][c]
  /\ ReplayDue(h, c, id)
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {Out(id, "rejected")}]
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* v2 replay: the journal says it ran, so record the outcome.
RecoverDone(h, id, m) ==
  LET c == Target(m) IN
  /\ Awake(h)
  /\ Mode = "v2" /\ Mutant # "ReplayRequeue"
  /\ Cmd(id, m) \in rep[h][c]
  /\ ReplayDue(h, c, id)
  /\ RanBefore(h, c, m)
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {Out(id, "applied")}]
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* v2 replay: it never ran, so run it now and record the outcome.
RecoverRedo(h, id, m) ==
  LET c == Target(m) IN
  /\ Awake(h)
  /\ Mode = "v2" /\ Mutant # "ReplayRequeue"
  /\ Cmd(id, m) \in rep[h][c]
  /\ ReplayDue(h, c, id)
  /\ (Mutant = "ReplayNoRanCheck" \/ ~RanBefore(h, c, m))
  /\ DispatchEff(h, id, m)
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {MsgRow(m), Out(id, "applied")}]
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, executing, inflight, net,
                 relayLog, wake, issued, emitDue, handoff, seq, crashes, offs, expires,
                 retries, dups, moves, rollbacks>>

\* Mutant ReplayRequeue: replay re-queues a fresh attempt and leaves the old one open.
RecoverRequeue(h, id, m) ==
  LET c == Target(m) IN
  /\ Awake(h)
  /\ Mutant = "ReplayRequeue"
  /\ Cmd(id, m) \in rep[h][c]
  /\ ReplayDue(h, c, id)
  /\ seq[h] < MaxSeq
  /\ rep' = [rep EXCEPT ![h][c] = @ \cup {Cmd(<<h, seq[h] + 1>>, m)}]
  /\ seq' = [seq EXCEPT ![h] = @ + 1]
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, crashes, offs, expires, retries, dups, moves, rollbacks>>

\* retry_delivery (doc_host.rs): any device, person-driven. A dead attempt
\* (rejected/expired), message not landed, no live attempt in this device's
\* view: mint a FRESH id with the same message id.
RetryUser(d, m) ==
  LET c == Target(m)  R == rep[d][c] IN
  /\ retries < RetryBudget
  /\ seq[d] < MaxSeq
  /\ d \in Holders(c) \cap Laptops   \* a person clicks retry on a laptop
  /\ \E id \in AttIds(R, m) : Out(id, "rejected") \in R \/ Out(id, "expired") \in R
  /\ MsgRow(m) \notin R
  /\ \A j \in AttIds(R, m) : HasOut(R, j) \/ j \in processed[d]
  /\ rep' = [rep EXCEPT ![d][c] = @ \cup {Cmd(<<d, seq[d] + 1>>, m)}]
  /\ seq' = [seq EXCEPT ![d] = @ + 1]
  /\ retries' = retries + 1
  /\ UNCHANGED <<edge, online, host, view, processed, procMid, journal, executing,
                 inflight, net, relayLog, wake, issued, child, emitDue, handoff,
                 dispatched, execBy, badHost, crashes, offs, expires, dups, moves, rollbacks>>

---------------------------------------------------------------------------
(* Host move (not in Zeron today). Only P moves, ParentHost -> MoveTarget. *)

MoveNaive ==
  /\ HostMove = "naive"
  /\ moves = 0
  /\ host' = [host EXCEPT !["P"] = MoveTarget]
  /\ view' = [view EXCEPT ![MoveTarget]["P"] = MoveTarget]
  /\ moves' = 1
  /\ UNCHANGED <<edge, rep, online, processed, procMid, journal, executing, inflight, net,
                 relayLog, wake, issued, child, emitDue, handoff, dispatched, execBy,
                 badHost, seq, crashes, offs, expires, retries, dups, rollbacks>>

\* Safe step 1: the old host stops draining P, publishes its rows and ledger.
Release ==
  LET h == host["P"]  PIds == AttIds(rep[h]["P"], "run") \cup AttIds(rep[h]["P"], "res") IN
  /\ HostMove = "safe"
  /\ moves = 0
  /\ h = ParentHost
  /\ view[h]["P"] = h
  /\ online[h]
  /\ (executing[h] \cup inflight[h]) \cap PIds = {}
  /\ host' = [host EXCEPT !["P"] = NoDev]
  /\ view' = [view EXCEPT ![h]["P"] = NoDev]
  /\ edge' = [edge EXCEPT !["P"] = @ \cup rep[h]["P"]]
  /\ handoff' = [on |-> TRUE, ids |-> processed[h] \cap PIds,
                 mids |-> procMid[h] \cap {"run", "res"},
                 jr |-> journal[h] \cap {"run", "res"}, rows |-> rep[h]["P"]]
  /\ moves' = 1
  /\ UNCHANGED <<rep, online, processed, procMid, journal, executing, inflight, net,
                 relayLog, wake, issued, child, emitDue, dispatched, execBy, badHost,
                 seq, crashes, offs, expires, retries, dups, rollbacks>>

\* Safe step 2: the new host takes over only after it holds every released row.
Accept ==
  LET t == MoveTarget IN
  /\ HostMove = "safe"
  /\ handoff.on
  /\ host["P"] = NoDev
  /\ online[t]
  /\ handoff.rows \subseteq rep[t]["P"]
  /\ processed' = [processed EXCEPT ![t] = @ \cup handoff.ids]
  /\ procMid' = [procMid EXCEPT ![t] = @ \cup handoff.mids]
  /\ journal' = [journal EXCEPT ![t] = @ \cup handoff.jr]
  /\ host' = [host EXCEPT !["P"] = t]
  /\ view' = [view EXCEPT ![t]["P"] = t]
  /\ UNCHANGED <<edge, rep, online, executing, inflight, net, relayLog, wake, issued,
                 child, emitDue, handoff, dispatched, execBy, badHost, seq, crashes,
                 offs, expires, retries, dups, moves, rollbacks>>

RegistrySync(d, c) ==
  /\ HostMove # "off"
  /\ online[d]
  /\ view[d][c] # host[c]
  /\ view' = [view EXCEPT ![d][c] = host[c]]
  /\ UNCHANGED <<edge, rep, online, host, processed, procMid, journal, executing, inflight,
                 net, relayLog, wake, issued, child, emitDue, handoff, dispatched, execBy,
                 badHost, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>

---------------------------------------------------------------------------
(* Next, fairness, documented terminal states *)

AllRows == (UNION {edge[c] : c \in Chats}) \cup (UNION {rep[d][c] : d \in Dev, c \in Chats})
AllAtt(m) == AttIds(AllRows, m)
DeadOut(id) == Out(id, "rejected") \in AllRows \/ Out(id, "expired") \in AllRows

\* AC-M7: the only states with nothing left to do.
TerminalOK ==
  \/ dispatched["res"] >= 1
  \/ \E m \in issued : dispatched[m] = 0 /\ \A id \in AllAtt(m) : DeadOut(id)
Terminal == TerminalOK /\ UNCHANGED vars

CmdIn(h, c, r) == r \in rep[h][c] /\ r.t = "cmd"

ClaimA       == \E h \in Dev : \E c \in Chats : \E r \in rep[h][c] : r.t = "cmd" /\ Claim(h, r.id, r.m)
ClaimLandedA == \E h \in Dev : \E c \in Chats : \E r \in rep[h][c] : r.t = "cmd" /\ ClaimLanded(h, r.id, r.m)
ClaimDupA    == \E h \in Dev : \E c \in Chats : \E r \in rep[h][c] : r.t = "cmd" /\ ClaimDup(h, r.id, r.m)
ExecA        == \E h \in Dev : \E c \in Chats : \E r \in rep[h][c] : r.t = "cmd" /\ Exec(h, r.id, r.m)
ResolveA     == \E h \in Dev : \E c \in Chats : \E r \in rep[h][c] : r.t = "cmd" /\ Resolve(h, r.id, r.m)
ExpireA      == \E h \in Dev : \E c \in Chats : \E r \in rep[h][c] : r.t = "cmd" /\ Expire(h, r.id, r.m)
SweepA       == \E h \in Dev : \E c \in Chats : \E r \in rep[h][c] : r.t = "cmd" /\ Sweep(h, r.id, r.m)
RecoverDoneA == \E h \in Dev : \E c \in Chats : \E r \in rep[h][c] : r.t = "cmd" /\ RecoverDone(h, r.id, r.m)
RecoverRedoA == \E h \in Dev : \E c \in Chats : \E r \in rep[h][c] : r.t = "cmd" /\ RecoverRedo(h, r.id, r.m)
RequeueA     == \E h \in Dev : \E c \in Chats : \E r \in rep[h][c] : r.t = "cmd" /\ RecoverRequeue(h, r.id, r.m)
RelaySendA   == \E d \in Dev : \E c \in Chats : \E r \in rep[d][c] : r.t = "cmd" /\ RelaySend(d, r.id, r.m)
IngestClaimA == \E msg \in net : IngestClaim(msg)
IngestDupA   == \E msg \in net : IngestDup(msg)
IngestDropA  == \E msg \in net : IngestDrop(msg)
RelayLoseA   == \E msg \in net : RelayLose(msg)
IngestA      == IngestClaimA \/ IngestDupA \/ IngestDropA

Next ==
  \/ IssueRun
  \/ \E d \in Dev : IssueKick(d) \/ ChildFinish(d) \/ EmitResult(d) \/ RemoteChildResult(d)
                    \/ GoOffline(d) \/ GoOnline(d) \/ Crash(d) \/ Rollback(d)
  \/ \E d \in Dev, c \in Chats : Push(d, c) \/ Pull(d, c) \/ RegistrySync(d, c)
  \/ \E d \in Dev, m \in Mids : RetryUser(d, m)
  \/ RelaySendA
  \/ IngestClaimA \/ IngestDupA \/ IngestDropA \/ RelayLoseA
  \/ ClaimA \/ ClaimDupA \/ ClaimLandedA \/ ExecA \/ ResolveA \/ ExpireA
  \/ SweepA \/ RecoverDoneA \/ RecoverRedoA \/ RequeueA
  \/ MoveNaive \/ Release \/ Accept
  \/ Terminal

Spec == Init /\ [][Next]_vars

Fairness ==
  /\ WF_vars(IssueRun)
  /\ WF_vars(\E d \in Dev : IssueKick(d))
  /\ WF_vars(\E d \in Dev : ChildFinish(d))
  /\ WF_vars(\E d \in Dev : EmitResult(d))
  /\ WF_vars(\E d \in Dev : RemoteChildResult(d))
  /\ WF_vars(\E d \in Dev : GoOnline(d))
  /\ WF_vars(\E d \in Dev, c \in Chats : Push(d, c))
  /\ WF_vars(\E d \in Dev, c \in Chats : Pull(d, c))
  /\ WF_vars(\E d \in Dev, c \in Chats : RegistrySync(d, c))
  /\ WF_vars(RelaySendA)
  /\ WF_vars(IngestA)
  /\ WF_vars(ClaimA)
  /\ WF_vars(ClaimDupA)
  /\ WF_vars(ClaimLandedA)
  /\ WF_vars(ExecA)
  /\ WF_vars(ResolveA)
  /\ WF_vars(SweepA)
  /\ WF_vars(RecoverDoneA)
  /\ WF_vars(RecoverRedoA)
  /\ WF_vars(Accept)

LiveSpec == Spec /\ Fairness

---------------------------------------------------------------------------
(* Properties *)

TypeOK ==
  /\ online \in [Dev -> BOOLEAN]
  /\ dispatched \in [Mids -> 0..2]
  /\ child \in {"idle", "running", "done"}
  /\ issued \subseteq Mids
  /\ \A d \in Dev : journal[d] \subseteq Mids /\ procMid[d] \subseteq Mids
  /\ \A d \in Dev : seq[d] \in 0..MaxSeq

\* S9: one executor per command, only the host executes, never twice.
OneExecutor == \A p, q \in execBy : p[1] = q[1] => p[2] = q[2]
HostOnly == ~badHost
NoDoubleDispatch == \A m \in Mids : dispatched[m] <= 1
S9 == OneExecutor /\ HostOnly /\ NoDoubleDispatch
OneHost == S9   \* DESIGN.md name

\* S3 across devices: the parent gets the child's result at most once, and only after the child finished.
S3Safe == dispatched["res"] <= 1 /\ (dispatched["res"] >= 1 => child = "done")

\* S10: replay is idempotent. ReplayOnce is the batch closure of the per-item
\* replay actions (Sweep / RecoverDone / RecoverRedo / RecoverRequeue) on one
\* device's durable state; applying it to its own result changes nothing.
Snap(d) == [rp |-> rep[d], jr |-> journal[d], ds |-> dispatched, sq |-> seq[d]]
DueSet(d, s) == {x \in Chats \X Mids \X {r.id : r \in UNION {s.rp[c] : c \in Chats}} :
                   /\ Cmd(x[3], x[2]) \in s.rp[x[1]]
                   /\ Target(x[2]) = x[1]
                   /\ view[d][x[1]] = d
                   /\ x[3] \in processed[d]
                   /\ ~HasOut(s.rp[x[1]], x[3])}
ReplayOnce(d, s) ==
  LET D == DueSet(d, s)
      ran(y) == y[2] \in s.jr \/ (DocGate /\ MsgRow(y[2]) \in s.rp[y[1]])
      redo == {x[2] : x \in {y \in D : Mutant = "ReplayNoRanCheck" \/ ~ran(y)}}
  IN IF D = {} THEN s
     ELSE IF Mode = "zeron"
     THEN [s EXCEPT !.rp = [c \in Chats |-> s.rp[c] \cup
                             {Out(x[3], "rejected") : x \in {y \in D : y[1] = c}}]]
     ELSE IF Mutant = "ReplayRequeue"
     THEN [s EXCEPT !.rp = [c \in Chats |-> s.rp[c] \cup
                             {Cmd(<<d, s.sq + 1>>, x[2]) : x \in {y \in D : y[1] = c}}],
                    !.sq = s.sq + 1]
     ELSE [s EXCEPT !.rp = [c \in Chats |-> s.rp[c]
                             \cup {Out(x[3], "applied") : x \in {y \in D : y[1] = c}}
                             \cup {MsgRow(m) : m \in {mm \in redo : Target(mm) = c}}],
                    !.jr = s.jr \cup {x[2] : x \in D},
                    !.ds = [m \in Mids |-> IF m \in redo THEN Cap2(s.ds[m]) ELSE s.ds[m]]]
ReplayIdem == \A d \in Dev : ReplayOnce(d, ReplayOnce(d, Snap(d))) = ReplayOnce(d, Snap(d))

\* An attempt never gets two different outcomes.
OutUnique == \A r1, r2 \in AllRows :
               (r1.t = "out" /\ r2.t = "out" /\ r1.id = r2.id) => r1.m = r2.m
S10 == ReplayIdem /\ OutUnique
ReplayIsStutter == S10   \* DESIGN.md name

\* Liveness (L-style). LRun: a person's send runs or is visibly expired.
RunExpiredSeen == \E id \in AllAtt("run") : Out(id, "expired") \in edge["P"]
LRun   == ("run" \in issued) ~> (dispatched["run"] >= 1 \/ RunExpiredSeen)
LKick  == ("kick" \in issued) ~> (dispatched["kick"] >= 1)
LChild == (dispatched["kick"] >= 1) ~> (child = "done")
LRes   == (child = "done") ~> (dispatched["res"] >= 1)

\* Witnesses: negated success states the checker must violate.
F11Witness == ~(dispatched["res"] >= 1 /\ ChildHost # ParentHost)
\* A retry after expiry runs the message exactly once.
RetryWitness == ~(retries >= 1 /\ dispatched["run"] = 1)
\* A safe handover completes and the parent still gets the result.
HandoverWitness == ~(moves = 1 /\ host["P"] = MoveTarget /\ dispatched["res"] >= 1)
\* Coverage witnesses (AC-M5) for actions the dot dump could not reach in memory.
DupWitness == ~(\E d \in Dev : \E c \in Chats : \E r \in rep[d][c] : r.t = "out" /\ r.m = "dup")
SyncWitness == ~(\E d \in Dev \ {MoveTarget} : view[d]["P"] = MoveTarget)
\* AC-M5 asks only that an action be enabled somewhere: IngestDup's guard holds.
IngestDupWitness == ~(\E msg \in net : online[msg.to] /\ DupGuard(msg.to, Target(msg.m), msg.id, msg.m))
RequeueWitness == ~(\E d \in Dev : \E c \in Chats : \E r \in rep[d][c] :
                     r.t = "cmd" /\ r.id[1] = ParentHost /\ r.m = "run")

---------------------------------------------------------------------------
(* S10, action form (DESIGN.md \S 12 "ReplayIsStutter"). The name ReplayIsStutter *)
(* above is the state form; this is the step form. A step that applies or       *)
(* replays attempt id of message m on host h, when a receipt already exists     *)
(* (an outcome row for id, or m already ran here: journal or landed message),   *)
(* changes no domain variable. It may only add outcome rows, or command rows    *)
(* that get an outcome in the same step (a closed duplicate).                   *)
S10Ids == {r.id : r \in AllRows} \cup {msg.id : msg \in net}
S10Receipt(h, id, m) == HasOut(rep[h][Target(m)], id) \/ RanBefore(h, Target(m), m)
S10Apply(h, id, m) ==
  \/ Claim(h, id, m) \/ ClaimDup(h, id, m) \/ ClaimLanded(h, id, m) \/ Exec(h, id, m)
  \/ RecoverDone(h, id, m) \/ RecoverRedo(h, id, m) \/ RecoverRequeue(h, id, m) \/ Sweep(h, id, m)
  \/ \E msg \in net : msg.to = h /\ msg.id = id /\ msg.m = m /\ (IngestClaim(msg) \/ IngestDup(msg))
S10RowsClosed == \A d \in Dev, c \in Chats : \A r \in rep'[d][c] \ rep[d][c] :
                   r.t = "out" \/ (r.t = "cmd" /\ HasOut(rep'[d][c], r.id))
S10DomainVars == <<edge, dispatched, execBy, badHost, journal, child, issued, seq, executing, inflight>>
S10Stutter == UNCHANGED S10DomainVars /\ S10RowsClosed
ReplayStep == \A h \in Dev, m \in Mids, id \in S10Ids :
                (S10Receipt(h, id, m) /\ S10Apply(h, id, m)) => S10Stutter
ReplayStepIsStutter == [][ReplayStep]_vars
=============================================================================
