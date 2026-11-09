---------------- MODULE RuntimePlanDispatchKernel ----------------
(* Independent outbox drain steps. No target relation is imported.
   The caller supplies immutable rows [effect, key]; key is wire::order_key,
   or "none" when that function returns None. IDs are local durable row IDs.
   Snapshot order is durable seq/idx order. Claims protect an ID, not a
   thread. Different workers can execute different IDs for the same thread.
   A Pending check under claim admits execution. It does not assert that
   an external completion cannot occur after that check.
   This kernel is not yet joined to RuntimePlanOperational. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC

PDKEmpty == [x \in {} |-> x]
PDKPut(f,k,v) == (k :> v) @@ f
PDKDrop(f,k) == [x \in DOMAIN f \ {k} |-> f[x]]
PDKWorker == [phase |-> "idle",snapshot |-> <<>>,cursor |-> 1,
              active |-> 0,busy |-> {},result |-> "none"]
PDKInit(Workers) == [rows |-> PDKEmpty,order |-> <<>>,claims |-> PDKEmpty,
                    workers |-> [w \in Workers |-> PDKWorker]]

RECURSIVE PDKAppend(_, _)
PDKAppend(p,rows) == IF Len(rows)=0 THEN p ELSE
 LET id==Len(p.order)+1 r==Head(rows)
     next==[p EXCEPT !.order=Append(@,id),
       !.rows=PDKPut(@,id,[effect |-> r.effect,key |-> r.key,status |-> "pending"])]
 IN PDKAppend(next,Tail(rows))

(* Driver::record_engine_interrupt appends and retires only its fresh
   matching InterruptRun rows in one transaction. The caller must check
   exact current/live attempt and ~int_req, apply InterruptChat, and use
   this function in the same step. A duplicate appends no rows. Faults
   before commit leave the whole state unchanged. No old row is retired. *)
PDKAppendCleanup(p,rows,run) ==
 LET next==PDKAppend(p,rows)
 IN [next EXCEPT !.rows=[id \in DOMAIN next.rows |->
      IF id>Len(p.order) THEN
        IF next.rows[id].effect.kind="InterruptRun" THEN
          IF next.rows[id].effect.run=run
          THEN [next.rows[id] EXCEPT !.status="done"] ELSE next.rows[id]
        ELSE next.rows[id]
      ELSE next.rows[id]]]

RECURSIVE PDKPending(_, _)
PDKPending(p,ids) == IF Len(ids)=0 THEN <<>> ELSE
 (IF p.rows[Head(ids)].status="pending" THEN <<Head(ids)>> ELSE <<>>) \o
 PDKPending(p,Tail(ids))
PDKBusy(row,busy) == IF row.key="none" THEN busy ELSE busy \cup {row.key}
PDKAdvance(worker) == [worker EXCEPT !.phase="scan",!.cursor=@+1,
                       !.active=0,!.result="none"]
PDKCompleteOnError(e) == e.kind \in {"StartRun","StartAttempt","SteerRun"}

PDKSnapshot(p,p2,w) ==
 /\ p.workers[w].phase="idle"
 /\ p2=[p EXCEPT !.workers[w]=[PDKWorker EXCEPT !.phase="scan",
                            !.snapshot=PDKPending(p,p.order)]]
PDKEndScan(p,p2,w) ==
 /\ p.workers[w].phase="scan"
 /\ p.workers[w].cursor>Len(p.workers[w].snapshot)
 /\ p2=[p EXCEPT !.workers[w]=PDKWorker]

(* Claim insertion precedes the durable Pending read. A stale snapshot
   cannot admit an ID whose status was already completed at that read. *)
PDKClaim(p,p2,w) ==
 LET worker==p.workers[w] id==worker.snapshot[worker.cursor] row==p.rows[id] IN
 /\ worker.phase="scan" /\ worker.cursor<=Len(worker.snapshot)
 /\ row.key \notin worker.busy /\ id \notin DOMAIN p.claims
 /\ p2=[p EXCEPT !.claims=PDKPut(@,id,w),
                  !.workers[w]=[worker EXCEPT !.phase="checking",!.active=id]]
PDKSkipBusy(p,p2,w) ==
 LET worker==p.workers[w] id==worker.snapshot[worker.cursor] row==p.rows[id] IN
 /\ worker.phase="scan" /\ worker.cursor<=Len(worker.snapshot)
 /\ row.key \in worker.busy
 /\ p2=[p EXCEPT !.workers[w]=PDKAdvance(worker)]
PDKClaimDenied(p,p2,w) ==
 LET worker==p.workers[w] id==worker.snapshot[worker.cursor] row==p.rows[id] IN
 /\ worker.phase="scan" /\ worker.cursor<=Len(worker.snapshot)
 /\ row.key \notin worker.busy /\ id \in DOMAIN p.claims
 /\ p2=[p EXCEPT !.workers[w]=[worker EXCEPT !.phase="blocked",!.active=id]]
(* The owner can release between a denied claim and the caller's second
   claimed.contains read. That read, not the snapshot, sets local busy. *)
PDKDeniedCheck(p,p2,w) ==
 LET worker==p.workers[w] id==worker.active row==p.rows[id] IN
 /\ worker.phase="blocked"
 /\ p2=[p EXCEPT !.workers[w]=[PDKAdvance(worker) EXCEPT
       !.busy=IF id \in DOMAIN p.claims THEN PDKBusy(row,worker.busy) ELSE worker.busy]]
PDKCheck(p,p2,w) ==
 LET worker==p.workers[w] id==worker.active IN
 /\ worker.phase="checking" /\ p.claims[id]=w
 /\ IF p.rows[id].status="pending"
       THEN p2=[p EXCEPT !.workers[w].phase="ready"]
       ELSE p2=[p EXCEPT !.claims=PDKDrop(@,id),!.workers[w].phase="blocked"]
PDKCheckError(p,p2,w) ==
 LET worker==p.workers[w] id==worker.active IN
 /\ worker.phase="checking" /\ p.claims[id]=w
 /\ p2=[p EXCEPT !.claims=PDKDrop(@,id),!.workers[w]=PDKWorker]
(* The caller supplies its actual only/dead guard for these branches. *)
PDKExclude(p,p2,w) ==
 LET worker==p.workers[w] id==worker.snapshot[worker.cursor] row==p.rows[id] IN
 /\ worker.phase="scan" /\ worker.cursor<=Len(worker.snapshot)
 /\ p2=[p EXCEPT !.workers[w]=[PDKAdvance(worker) EXCEPT !.busy=PDKBusy(row,worker.busy)]]
PDKDead(p,p2,w) ==
 LET worker==p.workers[w] id==worker.active IN
 /\ worker.phase="ready" /\ p.claims[id]=w
 /\ p2=[p EXCEPT !.claims=PDKDrop(@,id),!.workers[w].phase="blocked"]

(* The joint model must execute the exact immutable effect in this row,
   then call PDKExecuted in the same step. No historical-intent guard can
   replace this effect identity. Result is ok or error. *)
PDKExecuting(p,w,id) == p.workers[w].phase="ready" /\
                       p.workers[w].active=id /\ p.claims[id]=w
PDKExecuted(p,p2,w,result) ==
 /\ result \in {"ok","error"}
 /\ PDKExecuting(p,w,p.workers[w].active)
 /\ p2=[p EXCEPT !.workers[w].phase="executed",!.workers[w].result=result]
PDKRetry(p,p2,w) ==
 LET worker==p.workers[w] id==worker.active row==p.rows[id] IN
 /\ worker.phase="executed" /\ worker.result="error"
 /\ ~PDKCompleteOnError(row.effect) /\ p.claims[id]=w
 /\ p2=[p EXCEPT !.claims=PDKDrop(@,id),!.workers[w]=
       [PDKAdvance(worker) EXCEPT !.busy=PDKBusy(row,worker.busy)]]
PDKComplete(p,p2,w) ==
 LET worker==p.workers[w] id==worker.active IN
 /\ worker.phase="executed" /\ p.claims[id]=w
 /\ worker.result="ok" \/ PDKCompleteOnError(p.rows[id].effect)
 /\ p2=[p EXCEPT !.rows[id].status="done",!.workers[w].phase="completed"]
PDKCompleteError(p,p2,w) ==
 LET worker==p.workers[w] id==worker.active IN
 /\ worker.phase="executed" /\ p.claims[id]=w
 /\ worker.result="ok" \/ PDKCompleteOnError(p.rows[id].effect)
 /\ p2=[p EXCEPT !.claims=PDKDrop(@,id),!.workers[w]=PDKWorker]
PDKRelease(p,p2,w) ==
 LET worker==p.workers[w] id==worker.active IN
 /\ worker.phase="completed" /\ p.claims[id]=w
 /\ p2=[p EXCEPT !.claims=PDKDrop(@,id),!.workers[w]=PDKAdvance(worker)]

(* Pre-fix diagnostic only: publication and cleanup were separate steps.
   Do not include this relation in the fixed implementation Next. *)
PDKLegacyCleanupComplete(p,p2,id) ==
 /\ id \in DOMAIN p.rows
 /\ p2=[p EXCEPT !.rows[id].status="done"]
=====================================================================
