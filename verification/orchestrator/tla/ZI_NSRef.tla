------------------------------- MODULE ZI_NSRef -------------------------------
(***************************************************************************)
(* Safety refinement ZI_NS => NativeSubagents (DESIGN 9.2 pattern).        *)
(* Checked as PROPERTY NSSafe == NS!Init /\ [][NS!Next]_(NS!vars).         *)
(*                                                                         *)
(* Mapping (NativeSubagents name <- ZI_NS expression):                     *)
(*   rst, rord, endBy <- the run row, or the unfinished journal entry's    *)
(*                       target if it owes the run-row write               *)
(*   est, eep, erun, ebg, how, closedEp <- doc {t}--sub--{k}, or the       *)
(*                       entry's target doc if it owes that doc write      *)
(*   task             <- task row, or the entry's target if it owes that   *)
(*                       task-row write                                    *)
(*   recovering, harness vars, inbox (pipe), replay, budgets <- identity   *)
(* Commit points: ReadEvent = Handle, RecoverEntry = Recover. Record       *)
(* writes and MarkEntry are stutters. A receipt-dropped replay maps to a   *)
(* Handle whose guards make it a no-op.                                     *)
(***************************************************************************)
EXTENDS ZI_NS

Owes(s)   == s \in pend.T /\ s \notin wDoc
OwesT(s)  == s \in pend.T /\ s \notin wTask
CurDoc(s) == IF Owes(s) THEN pend.doc[s] ELSE doc[s]
CurRun    == IF pend.runT /\ ~wRun THEN pend.run ELSE runRow

NS == INSTANCE NativeSubagents WITH
  rst <- CurRun.rst, rord <- CurRun.ord, endBy <- CurRun.by,
  est <- [s \in Subs |-> CurDoc(s).st],
  eep <- [s \in Subs |-> CurDoc(s).ep],
  erun <- [s \in Subs |-> CurDoc(s).run],
  ebg <- [s \in Subs |-> CurDoc(s).bg],
  how <- [s \in Subs |-> CurDoc(s).how],
  closedEp <- [s \in Subs |-> CurDoc(s).cep],
  task <- [s \in Subs |-> IF OwesT(s) THEN pend.task[s] ELSE trow[s]],
  M_AnyEndEndsRun <- FALSE, M_NoRunGuard <- FALSE, M_NoOrphanClose <- FALSE,
  M_NoSessClose <- FALSE, M_NoEpochGuard <- FALSE, M_ReopenTerminal <- FALSE,
  M_NoTaskMirror <- FALSE, M_NoRecoveryClose <- FALSE, M_OrphanCloseBg <- FALSE

NSSafe == NS!Init /\ [][NS!Next]_(NS!vars)

\* NativeSubagents invariants read through the mapping (implied by NSSafe; listed for quick failures).
NSOnlyRootEndsRun == NS!OnlyRootEndsRun
NSOrphansClosed   == NS!OrphansClosed
NSNoStaleClose    == NS!NoStaleClose
NSTaskMirror      == NS!TaskMirror
NSNoStuckTask     == NS!NoStuckTask
NSNoFalseOrphan   == NS!NoFalseOrphan
=============================================================================
