------------------------------- MODULE ZI_CTRef -------------------------------
(***************************************************************************)
(* Safety refinement ZI_CT => ContextTransfer (DESIGN 9.2 pattern).        *)
(* Checked as PROPERTY CTSafe == CT!Init /\ [][CT!Next]_(CT!vars).         *)
(*                                                                         *)
(* Mapping (ContextTransfer name <- ZI_CT expression):                     *)
(*   xf[x]  <- host(target) transfer row, with the settle receipt applied: *)
(*             a row named in dset[tgt] maps to st consumed, dl = how      *)
(*   msgs, created, forkOf, kindOf, run, pt (delivery rows + native truth),*)
(*   nx, vol, dup, abort, budgets <- identity                              *)
(*   syncW <- not mapped (replica watermarks are implementation only)      *)
(* Commit points: ZPersistInjected / ZPersistInline = PersistInjected /    *)
(* PersistInline. MarkRows and Sync are stutters.                          *)
(*                                                                         *)
(* Gap: ContextTransfer's native fork has no host condition. A provider    *)
(* cannot fork a native thread held on another device, so with CrossHost   *)
(* the refinement holds only with NativeFork = {}.                         *)
(***************************************************************************)
EXTENDS ZI_CT

MXf == [x \in XIds |-> IF xf[x].tgt \in Threads /\ x \in dset[xf[x].tgt].ids
                       THEN [xf[x] EXCEPT !.dl = dset[xf[x].tgt].how, !.st = "consumed"]
                       ELSE xf[x]]

CT == INSTANCE ContextTransfer WITH xf <- MXf

CTSafe == CT!Init /\ [][CT!Next]_(CT!vars)

\* ContextTransfer invariants read through the mapping (implied by CTSafe; listed for quick failures).
CTNoFuture      == CT!NoFuture
CTExactOrStated == CT!ExactOrStated
CTNoDuplicate   == CT!NoDuplicate
CTNoSilentDrop  == CT!NoSilentDrop
=============================================================================
