--------------------------- MODULE RuntimeContextTransferSpec ---------------------------
(***************************************************************************)
(* Zeron Orchestrator v2: context transfer (fork, merge-back, provider     *)
(* switch, subagent spawn, subagent result), after T3 orchestration v2     *)
(* (thread-lineage-and-context-transfer.md, provider-switching-and-        *)
(* context.md, ContextHandoffDelivery.ts, handoffBudget.ts).               *)
(*                                                                         *)
(* Abstractions                                                            *)
(*  - One global append-only journal `msgs`; message id = position. A      *)
(*    thread's log is the journal filtered by thread, so log order is id   *)
(*    order and a source point is an id watermark.                         *)
(*  - A fork does not copy messages (lazy, as in T3). Hist(t, w) is the    *)
(*    thread's own messages up to w plus its fork source's messages up to *)
(*    the fork point. One level of lineage (Main -> t2).                  *)
(*  - A merge-back or subagent result, when resolved, appends a "ctx" item *)
(*    to the target thread (T3 detailInTurnItem; Zeron MessagePart        *)
(*    MergeBack). Exp() expands ctx items to the message ids they carry.   *)
(*  - Provider threads: per (thread, provider). `pcov`/`pst` is what the   *)
(*    native thread really holds (delivered ids / ids stated omitted).     *)
(*    `ecov`/`seen` is what the engine has persisted (alreadyDelivered     *)
(*    item ids; last-seen watermark). They differ only across crashes.     *)
(*  - The budget is abstract: a handoff fits, is cut to anchors (first     *)
(*    user, last user, last assistant) plus the newest k items with an     *)
(*    explicit coverage header (selectHistory), or fails with a reason.    *)
(*  - Each Resolve is one durable transaction (record + ctx item). Native  *)
(*    fork is atomic. A crash loses in-flight delivery steps and the       *)
(*    provider process; the Run command is durable and is re-driven.       *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
  Threads, Main, NoThread,
  Providers, NoProv,
  CanHandoff,     \* providers that accept handoff context at all (others: unsupported)
  CanInject,      \* providers with native history injection (others: inline preamble)
  NativeFork,     \* providers with native thread fork at a turn boundary
  MainProviders,  \* providers the person may pick on Main (bounds the fork/subagent families)
  MaxMsg,         \* user/assistant journal bound (ctx items come on top, at most MaxX)
  MaxX,           \* context transfer records
  MaxCrash, MaxStale,
  Forks,          \* enable fork + merge_back
  Spawns,         \* enable subagent_spawn + subagent_result
  Cuts,           \* the budget may cut a handoff
  MaxCutK,        \* a cut keeps the anchors plus the newest k items, k <= MaxCutK
  BudgetFail,     \* the budget may fail a handoff (anchors do not fit)
  FixRecoverDelivery, \* R9: recovery settles a delivery left pending by a dead run
  \* Mutants (AC-M6). All FALSE in the real model.
  M_ReadHead,         \* resolution reads the source head, not the recorded source point
  M_InputInHandoff,   \* the switch source point includes the run's own user message
  M_NoDedup,          \* delivery does not filter already-delivered item ids
  M_NoUncertainGuard, \* retry injects into a native thread with a pending delivery
  M_DeltaFromLeaving, \* delta base is the leaving provider's last-seen point
  M_NoAnchors,        \* a cut may drop first user / last user / last assistant
  M_SilentCut,        \* a cut does not state what it omitted
  M_ForkNoRollback,   \* native fork copies the latest native state (no turn boundary)
  M_SkipFailed,       \* the run starts although a transfer failed
  M_ReplaceNoSummary  \* replacing an uncertain native thread does not rebuild context

ASSUME Main \in Threads /\ NoThread \notin Threads /\ NoProv \notin Providers
ASSUME CanInject \subseteq Providers /\ CanHandoff \subseteq Providers /\ NativeFork \subseteq Providers

MaxId   == MaxMsg + MaxX + 2
Ids     == 1..MaxId
XIds    == 1..MaxX
Roles   == {"user", "asst", "ctx"}
XTypes  == {"none", "fork", "merge_back", "provider_switch", "subagent_spawn", "subagent_result"}
XSt     == {"none", "pending", "resolved", "consumed", "failed", "superseded"}
Done    == {"consumed", "failed", "superseded"}
Strats  == {"none", "native_resume", "native_fork", "delta", "full_summary", "portable_replay", "merge_delta", "result"}
Why     == {"none", "unsupported", "budget", "replaced", "uncertain"}
DlSt    == {"none", "pending", "injected", "inline"}

NoX == [ty |-> "none", src |-> NoThread, tgt |-> NoThread, sp |-> 0, bp |-> 0, hd |-> 0, tp |-> NoProv,
        st |-> "none", strat |-> "none", why |-> "none", sel |-> {}, om |-> {}, hdr |-> FALSE,
        dl |-> "none", cr |-> FALSE]
FreshPT(alive) == [alive |-> alive, pcov |-> {}, pst |-> {}, ecov |-> {}, est |-> {}, seen |-> 0, dpend |-> FALSE]
NoRun == [st |-> "idle", p |-> NoProv, u |-> 0, w |-> 0]

VARIABLES
  msgs,       \* journal: Seq of [th, role, cx]
  created, forkOf, kindOf,
  run,        \* per thread: [st: idle|starting|live, p, u (input id), w (start point)]
  pt,         \* per thread, provider: provider thread record
  xf, nx,     \* transfer records
  vol,        \* per thread, volatile delivery progress: none|batched|injected|sent
  dup,        \* ghost: some id reached a native thread twice
  abort,      \* per thread: why the last run start was aborted before provider work
  crashLeft, staleLeft

vars == <<msgs, created, forkOf, kindOf, run, pt, xf, nx, vol, dup, abort, crashLeft, staleLeft>>


-----------------------------------------------------------------------------
Max(S) == IF S = {} THEN 0 ELSE CHOOSE m \in S : \A k \in S : k <= m
Min2(a, b) == IF a < b THEN a ELSE b
Len0 == Len(msgs)
Own(t, w) == {i \in 1..Min2(w, Len0) : msgs[i].th = t}
Hist(t, w) == Own(t, w) \cup (IF forkOf[t].src = NoThread THEN {}
                              ELSE Own(forkOf[t].src, Min2(w, forkOf[t].w)))
Exp(S) == {i \in S : msgs[i].role # "ctx"} \cup UNION {msgs[i].cx : i \in {j \in S : msgs[j].role = "ctx"}}
Visible(t) == Exp(Hist(t, Len0))
RoleIn(S, r) == {i \in S : msgs[i].role = r}
LatestStable(t) == Max(RoleIn(Hist(t, Len0), "asst"))
LatestOwnAsst(t) == Max(RoleIn(Own(t, Len0), "asst"))
Anchors(S) == LET us == RoleIn(S, "user") IN
              (IF us = {} THEN {} ELSE {Max(us), CHOOSE i \in us : \A j \in us : i <= j})
              \cup (IF RoleIn(S, "asst") = {} THEN {} ELSE {Max(RoleIn(S, "asst"))})
LastK(S, k) == {i \in S : Cardinality({j \in S : j > i}) < k}

\* Range of a transfer, from its recorded points (S8: "exactly the messages in its range").
RangeOf(x) ==
  LET r == xf[x] IN
  CASE r.ty \in {"fork", "subagent_spawn"} -> Hist(r.src, r.sp)
    [] r.ty = "provider_switch" /\ r.strat = "delta" -> {i \in Hist(r.src, r.sp) : i > r.bp}
    [] r.ty = "provider_switch" -> Hist(r.src, r.sp)
    [] r.ty = "merge_back" -> {i \in Own(r.src, r.sp) : i > r.bp}
    [] r.ty = "subagent_result" -> {r.sp}
    [] OTHER -> {}
\* What the resolver actually reads (M_ReadHead reads the head instead).
ReadRange(x, strat, bp) ==
  LET r == xf[x]
      sp == IF M_ReadHead THEN Len0 ELSE r.sp IN
  CASE r.ty \in {"fork", "subagent_spawn"} -> Hist(r.src, sp)
    [] r.ty = "provider_switch" /\ strat = "delta" -> {i \in Hist(r.src, sp) : i > bp}
    [] r.ty = "provider_switch" -> Hist(r.src, sp)
    [] r.ty = "merge_back" -> {i \in Own(r.src, sp) : i > r.bp}
    [] r.ty = "subagent_result" -> {r.sp}
    [] OTHER -> {}

Targeting(t) == {x \in XIds : xf[x].tgt = t}
Lineage == {"fork", "subagent_spawn", "provider_switch"}
PendingFor(t) == {x \in Targeting(t) : xf[x].st = "pending"}
ResolvedFor(t, p) == {x \in Targeting(t) : xf[x].st = "resolved" /\ xf[x].tp = p}
Payload(t, p) == LET all == UNION {xf[x].sel : x \in ResolvedFor(t, p)} IN
                 IF M_NoDedup THEN all ELSE all \ pt[t][p].ecov
StatedOm(t, p) == UNION {xf[x].om : x \in ResolvedFor(t, p)}
SeenAfter(t, p) == Max({pt[t][p].seen} \cup {xf[x].sp : x \in {y \in ResolvedFor(t, p) : xf[y].ty \in Lineage}})

-----------------------------------------------------------------------------
TypeOK ==
  /\ msgs \in Seq([th : Threads, role : Roles, cx : SUBSET Ids]) /\ Len(msgs) <= MaxId
  /\ created \in [Threads -> BOOLEAN]
  /\ forkOf \in [Threads -> [src : Threads \cup {NoThread}, w : 0..MaxId]]
  /\ run \in [Threads -> [st : {"idle", "starting", "live"}, p : Providers \cup {NoProv}, u : 0..MaxId, w : 0..MaxId]]
  /\ pt \in [Threads -> [Providers -> [alive : BOOLEAN, pcov : SUBSET Ids, pst : SUBSET Ids,
                                       ecov : SUBSET Ids, est : SUBSET Ids, seen : 0..MaxId, dpend : BOOLEAN]]]
  /\ \A x \in XIds : xf[x].ty \in XTypes /\ xf[x].st \in XSt /\ xf[x].strat \in Strats
                     /\ xf[x].why \in Why /\ xf[x].dl \in DlSt
  /\ nx \in 0..MaxX
  /\ vol \in [Threads -> {"none", "batched", "injected", "sent"}]
  /\ dup \in BOOLEAN /\ abort \in [Threads -> Why]

Init ==
  /\ msgs = <<>>
  /\ created = [t \in Threads |-> t = Main]
  /\ forkOf = [t \in Threads |-> [src |-> NoThread, w |-> 0]]
  /\ kindOf = [t \in Threads |-> "none"]
  /\ run = [t \in Threads |-> NoRun]
  /\ pt = [t \in Threads |-> [p \in Providers |-> FreshPT(FALSE)]]
  /\ xf = [x \in XIds |-> NoX] /\ nx = 0
  /\ vol = [t \in Threads |-> "none"]
  /\ dup = FALSE /\ abort = [t \in Threads |-> "none"]
  /\ crashLeft = MaxCrash /\ staleLeft = MaxStale

NewX(r) == /\ nx < MaxX /\ nx' = nx + 1 /\ xf' = [xf EXCEPT ![nx + 1] = r]
XRec(ty, src, tgt, sp, bp) == [NoX EXCEPT !.ty = ty, !.src = src, !.tgt = tgt, !.sp = sp, !.bp = bp, !.st = "pending"]

-----------------------------------------------------------------------------
(* The person (or parent agent) sends a message on thread t with provider  *)
(* p. Stale unconsumed transfers are superseded; if provider thread (t,p)  *)
(* has not seen the whole history, a provider_switch (catch-up) transfer   *)
(* is recorded. It is resolved lazily, below, before provider work starts. *)
UserSend(t, p) ==
  /\ created[t] /\ run[t].st = "idle" /\ vol[t] = "none" /\ Len0 < MaxMsg
  /\ (t = Main => p \in MainProviders)
  /\ LET u == Len0 + 1
         w == IF M_InputInHandoff THEN u ELSE Len0
         stale == {x \in Targeting(t) : xf[x].st \in {"pending", "resolved"}
                      /\ (xf[x].ty = "provider_switch" \/ (xf[x].st = "resolved" /\ xf[x].tp # p))}
         lineagePending == \E x \in Targeting(t) : xf[x].st = "pending" /\ xf[x].ty \in {"fork", "subagent_spawn"}
         need == ~lineagePending /\ \E i \in Hist(t, Len0) : i > pt[t][p].seen
         xs == [x \in XIds |-> IF x \in stale THEN [xf[x] EXCEPT !.st = "superseded", !.why = "replaced"] ELSE xf[x]]
     IN /\ msgs' = Append(msgs, [th |-> t, role |-> "user", cx |-> {}])
        /\ run' = [run EXCEPT ![t] = [st |-> "starting", p |-> p, u |-> u, w |-> w]]
        /\ IF need
           THEN /\ nx < MaxX /\ nx' = nx + 1
                /\ xf' = [xs EXCEPT ![nx + 1] = [XRec("provider_switch", t, t, w, 0) EXCEPT !.tp = p]]
           ELSE xf' = xs /\ UNCHANGED nx
  /\ abort' = [abort EXCEPT ![t] = "none"]
  /\ UNCHANGED <<created, forkOf, kindOf, pt, vol, dup, crashLeft, staleLeft>>

(* Fork from the latest stable point (last completed assistant message).   *)
(* Cheap: no provider thread, no handoff. Same shape for an agent-created  *)
(* child (subagent_spawn), which forks from the parent's completed runs.   *)
CreateLineage(s, t, ty) ==
  /\ created[s] /\ ~created[t] /\ t # Main /\ forkOf[s].src = NoThread
  /\ LatestStable(s) > 0
  /\ created' = [created EXCEPT ![t] = TRUE]
  /\ forkOf' = [forkOf EXCEPT ![t] = [src |-> s, w |-> LatestStable(s)]]
  /\ kindOf' = [kindOf EXCEPT ![t] = ty]
  /\ NewX(XRec(ty, s, t, LatestStable(s), 0))
  /\ UNCHANGED <<msgs, run, pt, vol, dup, abort, crashLeft, staleLeft>>
Fork(s, t) == Forks /\ CreateLineage(s, t, "fork")
SpawnChild(s, t) == Spawns /\ run[s].st = "live" /\ CreateLineage(s, t, "subagent_spawn")

(* Bring a fork's work back to its source (T3 thread.merge_back): base =   *)
(* the fork point (only the fork's own messages), source = the fork's     *)
(* latest stable point. Refused while the target has active work. An      *)
(* older pending merge for the pair is superseded.                         *)
MergeBack(f) ==
  /\ Forks /\ created[f] /\ kindOf[f] = "fork"
  /\ LET t == forkOf[f].src
         sp == LatestOwnAsst(f)
         prior == {x \in XIds : xf[x].ty = "merge_back" /\ xf[x].src = f}
     IN /\ run[t].st = "idle" /\ sp > 0
        /\ \A x \in prior : xf[x].sp < sp
        /\ nx < MaxX /\ nx' = nx + 1
        /\ xf' = [x \in XIds |->
                    IF x = nx + 1 THEN XRec("merge_back", f, t, sp, 0)
                    ELSE IF x \in prior /\ xf[x].st = "pending"
                         THEN [xf[x] EXCEPT !.st = "superseded", !.why = "replaced"] ELSE xf[x]]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, run, pt, vol, dup, abort, crashLeft, staleLeft>>

(* A child's final answer is pushed to its parent as context.              *)
SubagentResult(c) ==
  /\ Spawns /\ created[c] /\ kindOf[c] = "subagent_spawn" /\ run[c].st = "idle"
  /\ LET t == forkOf[c].src
         a == LatestOwnAsst(c)
     IN /\ a > 0
        /\ \A x \in XIds : xf[x].ty = "subagent_result" /\ xf[x].src = c => xf[x].sp < a
        /\ NewX(XRec("subagent_result", c, t, a, 0))
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, run, pt, vol, dup, abort, crashLeft, staleLeft>>

-----------------------------------------------------------------------------
(* Lazy resolution at run start (T3 resolveStartContext).                  *)
Fail(t, x, why) ==
  /\ xf' = [xf EXCEPT ![x].st = "failed", ![x].why = why, ![x].tp = run[t].p, ![x].hd = Len0]
  /\ run' = IF M_SkipFailed THEN run ELSE [run EXCEPT ![t] = NoRun]
  /\ abort' = IF M_SkipFailed THEN abort ELSE [abort EXCEPT ![t] = why]
  /\ UNCHANGED <<msgs, pt>>

(* Budget outcome for a handoff with read range R (expanded to ids ER).    *)
(*   fit: everything. cut(k): anchors + newest k, rest stated omitted.     *)
Outcomes(ER) ==
  {[o |-> "fit", sel |-> ER, om |-> {}, hdr |-> FALSE]}
  \cup (IF Cuts THEN
          {LET keep == (IF M_NoAnchors THEN {} ELSE Anchors(ER)) \cup LastK(ER, k) IN
             [o |-> "cut", sel |-> keep, om |-> IF M_SilentCut THEN {} ELSE ER \ keep, hdr |-> ~M_SilentCut]
           : k \in 0..Min2(MaxCutK, Cardinality(ER))}
        ELSE {})
  \cup (IF BudgetFail /\ ER # {} THEN {[o |-> "fail", sel |-> {}, om |-> {}, hdr |-> FALSE]} ELSE {})

(* Record a resolved handoff; merge/result also append a ctx item to t.    *)
Resolved(t, x, strat, bp, oc, newPT) ==
  LET p == run[t].p
      isCtx == xf[x].ty \in {"merge_back", "subagent_result"}
  IN /\ xf' = [xf EXCEPT ![x].st = "resolved", ![x].strat = strat, ![x].bp = bp, ![x].tp = p,
                         ![x].sel = oc.sel, ![x].om = oc.om, ![x].hdr = oc.hdr, ![x].hd = Len0]
     /\ msgs' = IF isCtx THEN Append(msgs, [th |-> t, role |-> "ctx", cx |-> oc.sel]) ELSE msgs
     /\ pt' = IF newPT THEN [pt EXCEPT ![t][p] = FreshPT(TRUE)] ELSE pt
     /\ UNCHANGED <<run, abort>>

ResolveCommon(t, x) ==
  /\ run[t].st = "starting" /\ vol[t] = "none" /\ x \in PendingFor(t)
  /\ (xf[x].ty \in {"merge_back", "subagent_result"} =>
        \A y \in PendingFor(t) : xf[y].ty \in {"merge_back", "subagent_result"})

HandoffOrFail(t, x, strat, bp, newPT) ==
  LET ER == Exp(ReadRange(x, strat, bp)) IN
  IF run[t].p \notin CanHandoff THEN Fail(t, x, "unsupported")
  ELSE \E oc \in Outcomes(ER) :
         IF oc.o = "fail" THEN Fail(t, x, "budget") ELSE Resolved(t, x, strat, bp, oc, newPT)

ResolveSwitch(t, x) ==
  /\ ResolveCommon(t, x) /\ xf[x].ty = "provider_switch"
  /\ LET p == run[t].p
         P == pt[t][p]
         others == {pt[t][q].seen : q \in Providers \ {p}}
         bp == IF M_DeltaFromLeaving THEN Max(others) ELSE P.seen
     IN IF P.alive /\ P.seen >= xf[x].sp
        THEN /\ xf' = [xf EXCEPT ![x].st = "consumed", ![x].strat = "native_resume", ![x].tp = p, ![x].hd = Len0]
             /\ UNCHANGED <<msgs, pt, run, abort, staleLeft>>
        ELSE \E stale \in (IF P.alive /\ staleLeft > 0 THEN BOOLEAN ELSE {FALSE}) :
               /\ staleLeft' = IF stale THEN staleLeft - 1 ELSE staleLeft
               /\ IF P.alive /\ ~stale
                  THEN HandoffOrFail(t, x, "delta", bp, FALSE)
                  ELSE HandoffOrFail(t, x, "full_summary", 0, TRUE)
  /\ UNCHANGED <<created, forkOf, kindOf, nx, vol, dup, crashLeft>>

ResolveLineage(t, x, ty) ==
  /\ ResolveCommon(t, x) /\ xf[x].ty = ty
  /\ LET p == run[t].p
         s == xf[x].src
         S == pt[s][p]
         sp == xf[x].sp
         canNative == p \in NativeFork /\ S.alive /\ Exp(Hist(s, sp)) \subseteq S.ecov
         cut(X) == IF M_ForkNoRollback THEN X ELSE {i \in X : i <= sp}
     IN IF canNative
        THEN /\ pt' = [pt EXCEPT ![t][p] = [alive |-> TRUE, pcov |-> cut(S.pcov), pst |-> cut(S.pst),
                                            ecov |-> cut(S.ecov), est |-> cut(S.est), seen |-> sp, dpend |-> FALSE]]
             /\ xf' = [xf EXCEPT ![x].st = "consumed", ![x].strat = "native_fork", ![x].tp = p, ![x].hd = Len0,
                                 ![x].sel = Exp(Hist(s, sp))]
             /\ UNCHANGED <<msgs, run, abort>>
        ELSE HandoffOrFail(t, x, "portable_replay", 0, TRUE)
  /\ UNCHANGED <<created, forkOf, kindOf, nx, vol, dup, crashLeft, staleLeft>>
ResolveFork(t, x)  == ResolveLineage(t, x, "fork")
ResolveSpawn(t, x) == ResolveLineage(t, x, "subagent_spawn")

ResolveCtx(t, x, ty, strat) ==
  /\ ResolveCommon(t, x) /\ xf[x].ty = ty
  /\ HandoffOrFail(t, x, strat, xf[x].bp, ~pt[t][run[t].p].alive)
  /\ UNCHANGED <<created, forkOf, kindOf, nx, vol, dup, crashLeft, staleLeft>>
ResolveMerge(t, x)  == ResolveCtx(t, x, "merge_back", "merge_delta")
ResolveResult(t, x) == ResolveCtx(t, x, "subagent_result", "result")

-----------------------------------------------------------------------------
(* Delivery (T3 deliverContextHandoffs): one batch per run start, deduped  *)
(* against item ids already delivered to this native thread; persisted    *)
(* pending before injection; injected or inline.                           *)
DeliverBatch(t) ==
  LET p == run[t].p IN
  /\ run[t].st = "starting" /\ vol[t] = "none" /\ PendingFor(t) = {}
  /\ ResolvedFor(t, p) # {}
  /\ (pt[t][p].dpend => M_NoUncertainGuard)
  /\ xf' = [x \in XIds |-> IF x \in ResolvedFor(t, p) THEN [xf[x] EXCEPT !.dl = "pending"] ELSE xf[x]]
  /\ pt' = [pt EXCEPT ![t][p].dpend = TRUE]
  /\ vol' = [vol EXCEPT ![t] = "batched"]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, run, nx, dup, abort, crashLeft, staleLeft>>

(* A delivery is pending on this native thread from an attempt that died:  *)
(* it may or may not have reached the provider. Replace the native thread  *)
(* and rebuild its context (T3 ContextHandoffDeliveryUncertainError).      *)
Uncertain(t) ==
  LET p == run[t].p
      old == ResolvedFor(t, p)
      need == ~M_ReplaceNoSummary /\ Hist(t, run[t].w) # {}
  IN
  /\ run[t].st = "starting" /\ vol[t] = "none" /\ PendingFor(t) = {}
  /\ pt[t][p].dpend /\ ~M_NoUncertainGuard
  /\ pt' = [pt EXCEPT ![t][p] = FreshPT(FALSE)]
  /\ LET xs == [x \in XIds |-> IF x \in old THEN [xf[x] EXCEPT !.st = "superseded", !.why = "uncertain"] ELSE xf[x]]
     IN IF need /\ nx < MaxX
        THEN /\ nx' = nx + 1
             /\ xf' = [xs EXCEPT ![nx + 1] = [XRec("provider_switch", t, t, run[t].w, 0) EXCEPT !.tp = p]]
             /\ UNCHANGED <<run, abort>>
        ELSE IF need
        THEN /\ xf' = xs /\ run' = [run EXCEPT ![t] = NoRun] /\ abort' = [abort EXCEPT ![t] = "replaced"]
             /\ UNCHANGED nx
        ELSE /\ xf' = xs /\ UNCHANGED <<nx, run, abort>>
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, vol, dup, crashLeft, staleLeft>>

Inject(t) ==
  LET p == run[t].p
      pay == Payload(t, p) IN
  /\ vol[t] = "batched" /\ p \in CanInject
  /\ dup' = (dup \/ pay \cap pt[t][p].pcov # {})
  /\ pt' = [pt EXCEPT ![t][p].pcov = @ \cup pay, ![t][p].pst = @ \cup StatedOm(t, p)]
  /\ vol' = [vol EXCEPT ![t] = "injected"]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, run, xf, nx, abort, crashLeft, staleLeft>>

Settle(t, p, how, extra) ==
  /\ xf' = [x \in XIds |-> IF x \in ResolvedFor(t, p)
                           THEN [xf[x] EXCEPT !.dl = how, !.st = "consumed"] ELSE xf[x]]
  /\ pt' = [pt EXCEPT ![t][p].ecov = @ \cup Payload(t, p) \cup extra,
                      ![t][p].est = @ \cup StatedOm(t, p),
                      ![t][p].seen = SeenAfter(t, p),
                      ![t][p].dpend = FALSE]

PersistInjected(t) ==
  /\ vol[t] = "injected"
  /\ Settle(t, run[t].p, "injected", {})
  /\ vol' = [vol EXCEPT ![t] = "none"]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, run, nx, dup, abort, crashLeft, staleLeft>>

(* The provider turn starts with the user message (plus the inline batch   *)
(* if the provider cannot inject). Fails nothing silently: every transfer  *)
(* for this run is consumed, or the run was aborted when one failed.      *)
TurnStart(t) ==
  LET p == run[t].p
      u == run[t].u
      inline == vol[t] = "batched"
      pay == IF inline THEN Payload(t, p) ELSE {}
      P == IF pt[t][p].alive THEN pt[t][p] ELSE FreshPT(TRUE)
  IN
  /\ run[t].st = "starting" /\ PendingFor(t) = {}
  /\ \/ vol[t] = "none" /\ ResolvedFor(t, p) = {}
     \/ inline /\ p \notin CanInject
  /\ dup' = (dup \/ (pay \cup {u}) \cap P.pcov # {} \/ u \in pay)
  /\ pt' = [pt EXCEPT ![t][p] = [P EXCEPT !.alive = TRUE, !.pcov = @ \cup pay \cup {u},
                                          !.pst = @ \cup (IF inline THEN StatedOm(t, p) ELSE {}),
                                          !.ecov = IF inline THEN @ ELSE @ \cup {u}]]
  /\ run' = [run EXCEPT ![t].st = "live"]
  /\ vol' = [vol EXCEPT ![t] = IF inline THEN "sent" ELSE "none"]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, xf, nx, abort, crashLeft, staleLeft>>

PersistInline(t) ==
  /\ vol[t] = "sent"
  /\ Settle(t, run[t].p, "inline", {run[t].u})
  /\ vol' = [vol EXCEPT ![t] = "none"]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, run, nx, dup, abort, crashLeft, staleLeft>>

(* The provider finishes the turn: the assistant message joins the log.    *)
Complete(t) ==
  LET p == run[t].p
      a == Len0 + 1 IN
  /\ run[t].st = "live" /\ vol[t] = "none"
  /\ msgs' = Append(msgs, [th |-> t, role |-> "asst", cx |-> {}])
  /\ pt' = [pt EXCEPT ![t][p].pcov = @ \cup {a}, ![t][p].ecov = @ \cup {a}, ![t][p].seen = a]
  /\ run' = [run EXCEPT ![t] = NoRun]
  /\ UNCHANGED <<created, forkOf, kindOf, xf, nx, vol, dup, abort, crashLeft, staleLeft>>

-----------------------------------------------------------------------------
(* Engine crash: volatile progress and provider processes are lost. A live *)
(* turn is interrupted; a starting run is re-driven from its durable Run   *)
(* command. Native threads and durable records survive.                    *)
Crash ==
  /\ crashLeft > 0
  /\ crashLeft' = crashLeft - 1
  /\ vol' = [t \in Threads |-> "none"]
  /\ run' = [t \in Threads |-> IF run[t].st = "live" THEN NoRun ELSE run[t]]
  /\ xf' = [x \in XIds |-> IF xf[x].st \in {"pending", "resolved"} /\ run[xf[x].tgt].st = "starting"
                           THEN [xf[x] EXCEPT !.cr = TRUE] ELSE xf[x]]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, pt, nx, dup, abort, staleLeft>>

(* R9: on recovery, a delivery left pending by a run that did not survive  *)
(* is settled at once: the transfer is superseded (uncertain) and the     *)
(* native thread is marked for replacement. T3 waits for the next send.    *)
RecoverDelivery(t, p) ==
  /\ FixRecoverDelivery /\ run[t].st = "idle" /\ vol[t] = "none"
  /\ \E x \in ResolvedFor(t, p) : xf[x].dl = "pending"
  /\ xf' = [x \in XIds |-> IF x \in ResolvedFor(t, p)
                           THEN [xf[x] EXCEPT !.st = "superseded", !.why = "uncertain"] ELSE xf[x]]
  /\ pt' = [pt EXCEPT ![t][p] = FreshPT(FALSE)]
  /\ UNCHANGED <<msgs, created, forkOf, kindOf, run, nx, vol, dup, abort, crashLeft, staleLeft>>

(* Documented terminal state (AC-M7): no run in progress, and the journal *)
(* or transfer budget is spent.                                            *)
Quiescent ==
  /\ \A t \in Threads : run[t].st = "idle" /\ vol[t] = "none"
  /\ (Len0 >= MaxMsg \/ nx >= MaxX)
Terminated == Quiescent /\ UNCHANGED vars

Engine ==
  \/ \E t \in Threads, x \in XIds :
       ResolveSwitch(t, x) \/ ResolveFork(t, x) \/ ResolveSpawn(t, x) \/ ResolveMerge(t, x) \/ ResolveResult(t, x)
  \/ \E t \in Threads : DeliverBatch(t) \/ Uncertain(t) \/ Inject(t) \/ PersistInjected(t)
                        \/ TurnStart(t) \/ PersistInline(t)
  \/ \E t \in Threads, p \in Providers : RecoverDelivery(t, p)

Env ==
  \/ \E t \in Threads, p \in Providers : UserSend(t, p)
  \/ \E s, t \in Threads : Fork(s, t) \/ SpawnChild(s, t)
  \/ \E f \in Threads : MergeBack(f) \/ SubagentResult(f)
  \/ \E t \in Threads : Complete(t)
  \/ Crash

Next == Engine \/ Env \/ Terminated
Spec == Init /\ [][Next]_vars
ResolveAny(t, x) == ResolveSwitch(t, x) \/ ResolveFork(t, x) \/ ResolveSpawn(t, x) \/ ResolveMerge(t, x) \/ ResolveResult(t, x)
DeliverAny(t) == DeliverBatch(t) \/ Uncertain(t) \/ Inject(t) \/ PersistInjected(t) \/ TurnStart(t) \/ PersistInline(t)
FairResolve == \A t \in Threads, x \in XIds : WF_vars(ResolveAny(t, x))
FairDeliver == \A t \in Threads : WF_vars(DeliverAny(t))
FairRecover == \A t \in Threads, p \in Providers : WF_vars(RecoverDelivery(t, p))
FairSpec == Spec /\ FairResolve /\ FairDeliver /\ FairRecover

-----------------------------------------------------------------------------
(* S8 (a): no context from after the source point. Each handoff reads only *)
(* its recorded range; no native thread holds anything its thread cannot   *)
(* see (a fork sees its source only up to the fork point).                 *)
Used == {x \in XIds : xf[x].st \in {"resolved", "consumed"} /\ xf[x].strat \notin {"none", "native_resume"}}
NoFuture ==
  /\ \A x \in Used : xf[x].sel \cup xf[x].om \subseteq Exp(RangeOf(x))
  /\ \A t \in Threads, p \in Providers : created[t] => pt[t][p].pcov \subseteq Visible(t)

(* S8 (b): exactly the range, or a stated subset: the omitted part is      *)
(* listed under a coverage header, and the anchors are kept.               *)
ExactOrStated ==
  \A x \in Used :
    LET ER == Exp(RangeOf(x)) IN
    /\ xf[x].sel \cap xf[x].om = {}
    /\ xf[x].sel \cup xf[x].om = ER
    /\ xf[x].om # {} => (xf[x].hdr /\ Anchors(ER) \subseteq xf[x].sel)

(* S8 (c): no message reaches a native thread twice.                       *)
NoDuplicate == ~dup

(* S8 (d): no silent drop. When a turn is live, its native thread holds    *)
(* (or was told it omits) every message of the thread up to the start      *)
(* point, and the run's own input.                                         *)
NoSilentDrop ==
  \A t \in Threads :
    run[t].st = "live" =>
      LET P == pt[t][run[t].p] IN
      Exp(Hist(t, run[t].w)) \cup {run[t].u} \subseteq P.pcov \cup P.pst

(* L5, conditional form: a transfer whose target run has started is        *)
(* eventually consumed, failed with a reason, or superseded. (A lazy fork  *)
(* that nobody messages stays pending by design.)                          *)
TransferResolves ==
  \A x \in XIds :
    (xf[x].st \in {"pending", "resolved"} /\ xf[x].tgt \in Threads /\ run[xf[x].tgt].st = "starting")
      ~> (xf[x].st \in Done \/ (xf[x].tgt \in Threads /\ abort[xf[x].tgt] # "none"))

(* DESIGN.md names. X1 CreateTransfer = UserSend/Fork/SpawnChild/MergeBack/ *)
(* SubagentResult; X2 ResolveNative = native_resume/native_fork branches;   *)
(* X3 ResolvePortable = Resolved; X4 FailTransfer = Fail; X5 Consume =      *)
(* Settle (and native branches); X6 Supersede = UserSend/MergeBack/         *)
(* Uncertain/RecoverDelivery.                                               *)
ContextOrder == NoFuture /\ ExactOrStated /\ NoDuplicate /\ NoSilentDrop

-----------------------------------------------------------------------------
(* Witnesses (must be VIOLATED).                                           *)
W_F5_MergeBack == ~(\E x \in XIds : xf[x].ty = "merge_back" /\ xf[x].st = "consumed" /\ xf[x].sel # {}
                                    /\ xf[x].sel \subseteq pt[xf[x].tgt][xf[x].tp].pcov)
W_F6_BudgetedSwitch == ~(\E x \in XIds : xf[x].ty = "provider_switch" /\ xf[x].st = "consumed"
                           /\ xf[x].strat \in {"delta", "full_summary"} /\ xf[x].om # {} /\ xf[x].hdr
                           /\ xf[x].sel # {})
W_ABA_Delta == ~(\E x, y \in XIds : xf[x].ty = "provider_switch" /\ xf[x].st = "consumed" /\ xf[x].strat = "delta"
                   /\ xf[x].bp > 0 /\ xf[x].sel # {}
                   /\ xf[y].ty = "provider_switch" /\ xf[y].st = "consumed" /\ xf[y].tp # xf[x].tp /\ y < x)
W_ConcurrentAppend == ~(\E x \in XIds : xf[x].ty \in {"fork", "merge_back"} /\ xf[x].st = "consumed"
                          /\ \E i \in 1..Len0 : i > xf[x].sp /\ i <= xf[x].hd /\ msgs[i].th = xf[x].src
                                                 /\ msgs[i].role # "ctx")
W_CrashMidResolve == ~(\E x \in XIds : xf[x].cr /\ xf[x].st = "consumed" /\ xf[x].strat # "native_resume")
W_Unsupported == ~(\E x \in XIds : xf[x].st = "failed" /\ xf[x].why = "unsupported")
W_BudgetFail == ~(\E x \in XIds : xf[x].st = "failed" /\ xf[x].why = "budget")
W_ReplacedThenFull == ~(\E x, y \in XIds : xf[x].why = "uncertain" /\ xf[y].st = "consumed"
                          /\ xf[y].strat = "full_summary" /\ x < y)
W_NativeFork == ~(\E x \in XIds : xf[x].strat = "native_fork" /\ xf[x].st = "consumed")
W_SubagentResult == ~(\E x \in XIds : xf[x].ty = "subagent_result" /\ xf[x].st = "consumed" /\ xf[x].sel # {})
W_Inline == ~(\E x \in XIds : xf[x].dl = "inline" /\ xf[x].sel # {})
MergeDeltaDelivered == W_F5_MergeBack
SwitchWithPacket    == W_F6_BudgetedSwitch

(* Coverage (AC-M5): each must be VIOLATED.                                *)
Cov_UserSend        == ~(\E t \in Threads, p \in Providers : ENABLED UserSend(t, p))
Cov_Fork            == ~(\E s, t \in Threads : ENABLED Fork(s, t))
Cov_SpawnChild      == ~(\E s, t \in Threads : ENABLED SpawnChild(s, t))
Cov_MergeBack       == ~(\E f \in Threads : ENABLED MergeBack(f))
Cov_SubagentResult  == ~(\E f \in Threads : ENABLED SubagentResult(f))
Cov_ResolveSwitch   == ~(\E t \in Threads, x \in XIds : ENABLED ResolveSwitch(t, x))
Cov_ResolveFork     == ~(\E t \in Threads, x \in XIds : ENABLED ResolveFork(t, x))
Cov_ResolveSpawn    == ~(\E t \in Threads, x \in XIds : ENABLED ResolveSpawn(t, x))
Cov_ResolveMerge    == ~(\E t \in Threads, x \in XIds : ENABLED ResolveMerge(t, x))
Cov_ResolveResult   == ~(\E t \in Threads, x \in XIds : ENABLED ResolveResult(t, x))
Cov_DeliverBatch    == ~(\E t \in Threads : ENABLED DeliverBatch(t))
Cov_Uncertain       == ~(\E t \in Threads : ENABLED Uncertain(t))
Cov_Inject          == ~(\E t \in Threads : ENABLED Inject(t))
Cov_PersistInjected == ~(\E t \in Threads : ENABLED PersistInjected(t))
Cov_TurnStart       == ~(\E t \in Threads : ENABLED TurnStart(t))
Cov_PersistInline   == ~(\E t \in Threads : ENABLED PersistInline(t))
Cov_Complete        == ~(\E t \in Threads : ENABLED Complete(t))
Cov_Crash           == ~ENABLED Crash
Cov_RecoverDelivery == ~(\E t \in Threads, p \in Providers : ENABLED RecoverDelivery(t, p))
Cov_Terminated      == ~ENABLED Terminated
=============================================================================
