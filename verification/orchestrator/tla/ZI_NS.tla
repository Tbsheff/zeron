-------------------------------- MODULE ZI_NS --------------------------------
(***************************************************************************)
(* Zeron Orchestrator v2: implementation-level model of harness-native     *)
(* subagents (DESIGN 9.1 ZeronImpl, slice: NativeSubagents, sections 3.6   *)
(* and 6). Refined to NativeSubagents in ZI_NSRef.tla.                     *)
(*                                                                         *)
(* Zeron mechanisms modelled:                                              *)
(*  - three durable record kinds written in SEPARATE steps: the chat's run *)
(*    row (rst, rord, endBy), the subagent doc {t}--sub--{k} per toolUseId *)
(*    (status, epoch, run, background, close reason, closed epoch), and    *)
(*    the unified task row per native task (F10).                          *)
(*  - run journal (write-ahead): the engine reads the next harness event   *)
(*    from the provider pipe and, in ONE durable step, appends a journal   *)
(*    entry that holds the target value of every record the event changes *)
(*    plus the event receipt. Record writes follow, one record per step,   *)
(*    doc before task row. Each record keeps the journal position it last  *)
(*    applied (here: the bits wRun / wDoc / wTask), so a re-applied write  *)
(*    is a no-op. The entry is marked done after every write.              *)
(*  - one entry in progress at a time: the engine reads the next event,    *)
(*    starts a run, or writes the recovery entry only when the journal has *)
(*    no unfinished entry (it computes targets from durable rows).         *)
(*  - event receipts keyed by (kind, toolUseId, epoch, run): a replayed    *)
(*    event (at-least-once) is dropped. The reducer has NO epoch compare   *)
(*    and NO run-ordinal compare: the receipt is the guard (the doc is     *)
(*    keyed by toolUseId only, as in sessions.rs).                         *)
(*  - engine crash: the provider process and the pipe die; the journal,    *)
(*    receipts and rows survive. Recovery re-reads the journal, finishes   *)
(*    the unfinished entry, then writes a recovery entry that closes every *)
(*    open subagent and ends a live run. Receipts of the dead provider     *)
(*    session are dropped then (no event outlives its provider process).   *)
(* One chat on one device: the run journal is that device's journal.      *)
(*                                                                         *)
(* The harness (provider) side is NativeSubagents' environment, unchanged. *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
  Subs, NoSub,
  MaxRuns, MaxEp,
  MaxCrash, MaxDup, MaxInterrupt, MaxSessEnd,
  MaxInbox,
  Background, SilentKill,
  NMut   \* "none", "NoReceipt", "MarkBeforeTask", "VolatileJournal"

ASSUME NoSub \notin Subs
ASSUME NMut \in {"none", "NoReceipt", "MarkBeforeTask", "VolatileJournal"}

Active   == "running"
Terminal == {"completed", "failed", "interrupted", "cancelled"}
SubSt    == {"none", Active} \cup Terminal
Outs     == {"completed", "failed"}

Ev(k, s, ep, r, bg, out) == [k |-> k, s |-> s, ep |-> ep, r |-> r, bg |-> bg, out |-> out]
Key(e) == <<e.k, e.s, e.ep, e.r>>
Keyed  == {"spawn", "end", "rootEnd"}   \* sessEnd: never replayed, two may share an id

Doc(st, ep, r, bg, h, cep) == [st |-> st, ep |-> ep, run |-> r, bg |-> bg, how |-> h, cep |-> cep]
NoDoc == Doc("none", 0, 0, FALSE, "none", 0)
RunRow(st, ord, by) == [rst |-> st, ord |-> ord, by |-> by]
NoRunW == RunRow("idle", 0, "none")
NoPend == [on |-> FALSE, runT |-> FALSE, run |-> NoRunW, T |-> {},
           doc |-> [s \in Subs |-> NoDoc], task |-> [s \in Subs |-> "none"]]

VARIABLES
  \* engine, durable
  runRow,      \* chat run row
  doc,         \* [Subs -> Doc] subagent docs {t}--sub--{k}
  trow,        \* [Subs -> SubSt] unified task rows (status)
  pend,        \* unfinished journal entry (NoPend if none)
  wRun, wDoc, wTask,  \* records that applied the unfinished entry
  rcpt,        \* event receipts
  recovering,  \* engine restarted after a crash, recovery not yet journaled
  \* provider (harness) truth: as NativeSubagents
  sess, proot, pst, pep, prun, pbg, pprog,
  \* provider pipe (volatile) and at-least-once replay source
  inbox, replay,
  crashLeft, dupLeft, intLeft, sessLeft

eng   == <<runRow, doc, trow, pend, wRun, wDoc, wTask, rcpt, recovering>>
provv == <<sess, proot, pst, pep, prun, pbg, pprog>>
budv  == <<crashLeft, dupLeft, intLeft, sessLeft>>
vars  == <<eng, provv, inbox, replay, budv>>

-----------------------------------------------------------------------------
Init ==
  /\ runRow = RunRow("idle", 0, "none")
  /\ doc = [s \in Subs |-> NoDoc] /\ trow = [s \in Subs |-> "none"]
  /\ pend = NoPend /\ wRun = FALSE /\ wDoc = {} /\ wTask = {}
  /\ rcpt = {} /\ recovering = FALSE
  /\ sess = FALSE /\ proot = FALSE
  /\ pst = [s \in Subs |-> "none"] /\ pep = [s \in Subs |-> 0]
  /\ prun = [s \in Subs |-> 0] /\ pbg = [s \in Subs |-> FALSE] /\ pprog = [s \in Subs |-> FALSE]
  /\ inbox = <<>> /\ replay = {}
  /\ crashLeft = MaxCrash /\ dupLeft = MaxDup /\ intLeft = MaxInterrupt /\ sessLeft = MaxSessEnd

Idle == pend = NoPend          \* journal has no unfinished entry
rord == runRow.ord             \* the harness reads the run ordinal from the run row

Room == Len(inbox) < MaxInbox
Emit(e) == inbox' = Append(inbox, e)
RECURSIVE KillEnds(_)
KillEnds(S) == IF S = {} THEN <<>>
               ELSE LET x == CHOOSE x \in S : TRUE
                    IN <<Ev("end", x, pep[x], prun[x], FALSE, "interrupted")>> \o KillEnds(S \ {x})
ProvRunning(s) == pst[s] = "running"
FgRunning == {s \in Subs : ProvRunning(s) /\ ~pbg[s]}

-----------------------------------------------------------------------------
(* Run start: one run-row write, through the engine loop (journal idle).   *)
StartRun ==
  /\ ~recovering /\ Idle /\ runRow.rst = "idle" /\ rord < MaxRuns /\ ~proot
  /\ runRow' = RunRow("live", rord + 1, "none")
  /\ sess' = TRUE /\ proot' = TRUE
  /\ UNCHANGED <<doc, trow, pend, wRun, wDoc, wTask, rcpt, recovering,
                 pst, pep, prun, pbg, pprog, inbox, replay, budv>>

(* Harness actions: NativeSubagents' environment. *)
PSpawn(s, bg) ==
  /\ Room /\ proot /\ pst[s] = "none" /\ (bg => Background)
  /\ pst' = [pst EXCEPT ![s] = "running"] /\ pep' = [pep EXCEPT ![s] = 1]
  /\ prun' = [prun EXCEPT ![s] = rord] /\ pbg' = [pbg EXCEPT ![s] = bg]
  /\ Emit(Ev("spawn", s, 1, rord, bg, "none"))
  /\ UNCHANGED <<eng, sess, proot, pprog, replay, budv>>

PResume(s) ==
  /\ Room /\ proot /\ pst[s] = "done" /\ pep[s] < MaxEp
  /\ pst' = [pst EXCEPT ![s] = "running"] /\ pep' = [pep EXCEPT ![s] = @ + 1]
  /\ prun' = [prun EXCEPT ![s] = rord] /\ pbg' = [pbg EXCEPT ![s] = FALSE]
  /\ pprog' = [pprog EXCEPT ![s] = FALSE]
  /\ Emit(Ev("spawn", s, pep[s] + 1, rord, FALSE, "none"))
  /\ UNCHANGED <<eng, sess, proot, replay, budv>>

PProgress(s) ==
  /\ Room /\ ProvRunning(s) /\ ~pprog[s]
  /\ pprog' = [pprog EXCEPT ![s] = TRUE]
  /\ Emit(Ev("progress", s, pep[s], prun[s], pbg[s], "none"))
  /\ UNCHANGED <<eng, sess, proot, pst, pep, prun, pbg, replay, budv>>

PSubEnd(s, out) ==
  /\ Room /\ ProvRunning(s) /\ (pbg[s] \/ proot)
  /\ pst' = [pst EXCEPT ![s] = "done"]
  /\ Emit(Ev("end", s, pep[s], prun[s], pbg[s], out))
  /\ UNCHANGED <<eng, sess, proot, pep, prun, pbg, pprog, replay, budv>>

PRootEnd ==
  /\ Room /\ proot /\ FgRunning = {}
  /\ proot' = FALSE
  /\ Emit(Ev("rootEnd", NoSub, 0, rord, FALSE, "completed"))
  /\ UNCHANGED <<eng, sess, pst, pep, prun, pbg, pprog, replay, budv>>

PInterrupt(silent) ==
  /\ proot /\ intLeft > 0 /\ (silent => SilentKill)
  /\ LET killed == FgRunning
     IN /\ pst' = [s \in Subs |-> IF s \in killed THEN "dead" ELSE pst[s]]
        /\ inbox' = inbox \o (IF silent THEN <<>> ELSE KillEnds(killed))
                          \o <<Ev("rootEnd", NoSub, 0, rord, FALSE, "interrupted")>>
  /\ proot' = FALSE /\ intLeft' = intLeft - 1
  /\ UNCHANGED <<eng, sess, pep, prun, pbg, pprog, replay, crashLeft, dupLeft, sessLeft>>

PSessionEnd ==
  /\ Room /\ sess /\ ~proot /\ sessLeft > 0
  /\ sess' = FALSE /\ sessLeft' = sessLeft - 1
  /\ pst' = [s \in Subs |-> IF pst[s] = "running" THEN "dead" ELSE pst[s]]
  /\ Emit(Ev("sessEnd", NoSub, 0, rord, FALSE, "none"))
  /\ UNCHANGED <<eng, proot, pep, prun, pbg, pprog, replay, crashLeft, dupLeft, intLeft>>

(* Engine crash: provider process and pipe die. Journal, receipts, rows    *)
(* survive (NMut VolatileJournal: the unfinished entry was only in memory). *)
Crash ==
  /\ crashLeft > 0 /\ ~recovering
  /\ crashLeft' = crashLeft - 1 /\ recovering' = TRUE
  /\ sess' = FALSE /\ proot' = FALSE
  /\ pst' = [s \in Subs |-> IF pst[s] = "running" THEN "dead" ELSE pst[s]]
  /\ inbox' = <<>>
  /\ IF NMut = "VolatileJournal"
     THEN pend' = NoPend /\ wRun' = FALSE /\ wDoc' = {} /\ wTask' = {}
     ELSE UNCHANGED <<pend, wRun, wDoc, wTask>>
  /\ UNCHANGED <<runRow, doc, trow, rcpt, pep, prun, pbg, pprog, replay, dupLeft, intLeft, sessLeft>>

Replay(e) ==
  /\ Room /\ dupLeft > 0 /\ e \in replay /\ ~recovering
  /\ Emit(e) /\ dupLeft' = dupLeft - 1
  /\ replay' = IF dupLeft = 1 THEN {} ELSE replay
  /\ UNCHANGED <<eng, provv, crashLeft, intLeft, sessLeft>>

-----------------------------------------------------------------------------
(* Journal entry builder: target values only for touched records.          *)
Entry(runT, run, T, ND, NT) ==
  IF ~runT /\ T = {} THEN NoPend
  ELSE [on |-> TRUE, runT |-> runT, run |-> IF runT THEN run ELSE NoRunW, T |-> T,
        doc |-> [s \in Subs |-> IF s \in T THEN ND[s] ELSE NoDoc],
        task |-> [s \in Subs |-> IF s \in T THEN NT[s] ELSE "none"]]

CloseEntry(runT, run, S, st, h) ==
  Entry(runT, run, S, [s \in Subs |-> [doc[s] EXCEPT !.st = st, !.how = h, !.cep = doc[s].ep]],
        [s \in Subs |-> st])

(* Engine reducer (sessions.rs style): reads durable rows, no epoch or run *)
(* compare; replays are stopped by the receipt check in ReadEvent.          *)
Plan(e) ==
  CASE e.k = "spawn" ->
         Entry(FALSE, NoRunW, {e.s}, [s \in Subs |-> Doc(Active, e.ep, e.r, e.bg, "none", doc[s].cep)],
               [s \in Subs |-> Active])
    [] e.k = "end" ->
         IF doc[e.s].st = Active
         THEN Entry(FALSE, NoRunW, {e.s},
                    [s \in Subs |-> [doc[s] EXCEPT !.st = e.out, !.cep = e.ep,
                       !.how = IF runRow.rst = "live" /\ doc[s].run = rord THEN "event" ELSE "late"]],
                    [s \in Subs |-> e.out])
         ELSE NoPend
    [] e.k = "rootEnd" ->
         IF runRow.rst = "live"
         THEN CloseEntry(TRUE, RunRow("idle", rord, "root"),
                         {s \in Subs : doc[s].st = Active /\ ~doc[s].bg /\ doc[s].run = rord},
                         "interrupted", "orphan")
         ELSE NoPend
    [] e.k = "sessEnd" ->
         CloseEntry(FALSE, NoRunW, {s \in Subs : doc[s].st = Active}, "cancelled", "session")
    [] OTHER -> NoPend

(* Read the pipe head; journal entry + receipt in one durable step.        *)
ReadEvent ==
  /\ ~recovering /\ Idle /\ inbox # <<>>
  /\ LET e == Head(inbox)
         dupe == e.k \in Keyed /\ Key(e) \in rcpt /\ NMut # "NoReceipt"
     IN /\ inbox' = Tail(inbox)
        /\ replay' = IF dupLeft > 0 /\ e.k # "sessEnd" THEN replay \cup {e} ELSE {}
        /\ rcpt' = IF e.k \in Keyed THEN rcpt \cup {Key(e)} ELSE rcpt
        /\ pend' = IF dupe THEN NoPend ELSE Plan(e)
  /\ UNCHANGED <<runRow, doc, trow, wRun, wDoc, wTask, recovering, provv, budv>>

(* Recovery entry: after the unfinished entry is done. End a live run      *)
(* (interrupted) and cancel every open subagent.                            *)
RecoverEntry ==
  /\ recovering /\ Idle
  /\ recovering' = FALSE
  /\ pend' = CloseEntry(runRow.rst = "live", RunRow("idle", rord, "recovery"),
                        {s \in Subs : doc[s].st = Active}, "cancelled", "recovery")
  /\ rcpt' = {} /\ replay' = {}
  /\ UNCHANGED <<runRow, doc, trow, wRun, wDoc, wTask, provv, inbox, budv>>

(* Record writes, one record per step. Allowed during recovery.            *)
WriteRun ==
  /\ pend.runT /\ ~wRun
  /\ runRow' = pend.run /\ wRun' = TRUE
  /\ UNCHANGED <<doc, trow, pend, wDoc, wTask, rcpt, recovering, provv, inbox, replay, budv>>

WriteDoc(s) ==
  /\ s \in pend.T /\ s \notin wDoc
  /\ doc' = [doc EXCEPT ![s] = pend.doc[s]] /\ wDoc' = wDoc \cup {s}
  /\ UNCHANGED <<runRow, trow, pend, wRun, wTask, rcpt, recovering, provv, inbox, replay, budv>>

WriteTask(s) ==
  /\ s \in pend.T /\ s \in wDoc /\ s \notin wTask
  /\ trow' = [trow EXCEPT ![s] = pend.task[s]] /\ wTask' = wTask \cup {s}
  /\ UNCHANGED <<runRow, doc, pend, wRun, wDoc, rcpt, recovering, provv, inbox, replay, budv>>

(* Mark the entry done once every record applied it                        *)
(* (NMut MarkBeforeTask: does not wait for the task rows).                 *)
MarkEntry ==
  /\ pend.on /\ (pend.runT => wRun) /\ wDoc = pend.T
  /\ wTask = pend.T \/ NMut = "MarkBeforeTask"
  /\ pend' = NoPend /\ wRun' = FALSE /\ wDoc' = {} /\ wTask' = {}
  /\ UNCHANGED <<runRow, doc, trow, rcpt, recovering, provv, inbox, replay, budv>>

-----------------------------------------------------------------------------
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

Engine ==
  \/ ReadEvent \/ RecoverEntry \/ WriteRun \/ MarkEntry
  \/ \E s \in Subs : WriteDoc(s) \/ WriteTask(s)

Next == Env \/ Engine
Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Implementation-level invariants.                                        *)
(* F10 on rows: the task row differs from the doc only while the           *)
(* unfinished journal entry still owes that task-row write.                *)
ZN_TaskLagsOnlyInJournal == \A s \in Subs : trow[s] # doc[s].st => s \in pend.T \ wTask
ZN_DocBeforeTask == wTask \subseteq wDoc
ZN_WritesInEntry == wDoc \subseteq pend.T /\ (wRun => pend.runT)

(* Witnesses (must be VIOLATED): the two-write crash window is reached and *)
(* a replayed event is dropped by its receipt.                             *)
W_CrashBetweenWrites == ~(recovering /\ \E s \in pend.T : s \in wDoc /\ s \notin wTask)
W_ReplayDropped == ~(\E i \in 1..Len(inbox) : inbox[i].k \in Keyed /\ Key(inbox[i]) \in rcpt)
=============================================================================
