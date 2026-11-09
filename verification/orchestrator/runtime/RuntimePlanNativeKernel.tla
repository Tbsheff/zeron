-------------------- MODULE RuntimePlanNativeKernel --------------------
(* Independent finite provider/question environment. Logical core records do
   not start a physical session. Only ProviderStart acknowledges a pending
   StartRun/StartAttempt effect. No successor is guarded by Full!Next.

   Runtime mapping must supply provider-session and activation IDs from raw
   observer facts. A missing activation is unsupported, not epoch zero.
   QuestionInput/QuestionApplyAnswer need independent input bridge facts;
   the phase-2 capture does not yet supply these facts. *)
EXTENDS RuntimeRunDelegationSeams
CONSTANTS PlanMaxRuns, PlanMaxEpoch, PlanMaxInbox, PlanMaxCrash

PNNone == "none"
PNSub == [status |-> "none", epoch |-> 0, run |-> 0, attempt |-> 0,
          background |-> FALSE, progress |-> FALSE]
PNParent == [session |-> FALSE, root |-> FALSE, run |-> 0, attempt |-> 0,
             recovering |-> FALSE, end_by |-> "none"]
PNHandle == [present |-> FALSE,serial |-> 0,run |-> 0,attempt |-> 0,access |-> 0]
PNEvent(kind,p,k,ep,r,a,bg,out) == [kind |-> kind,parent |-> p,key |-> k,
  epoch |-> ep,run |-> r,attempt |-> a,background |-> bg,status |-> out]
PNIntent(p,r,a) == [parent |-> p,run |-> r,attempt |-> a]
PNAnswer(c,q,e,a) == [child |-> c,qid |-> q,epoch |-> e,answer |-> a]
PlanNativeInit(Threads,Subs) ==
 [parents |-> [p \in Threads |-> PNParent],
  issued_handle |-> [p \in Threads |-> PNHandle],
  callable_serial |-> [p \in Threads |-> 0],
  accepted_steers |-> [p \in Threads |-> <<>>],
  pending_access |-> [x \in {} |-> x],
  natives |-> [k \in Threads \X Subs |-> PNSub],
  doc_epoch |-> [k \in Threads \X Subs |-> 0],
  doc_how |-> [k \in Threads \X Subs |-> "none"],
  closed_epoch |-> [k \in Threads \X Subs |-> 0],
  pending_start |-> {},stop_requested |-> {},inbox |-> <<>>,replay |-> {},
  inputs |-> {},input_owner |-> [x \in {} |-> x],answer_pending |-> {},answered |-> {},native_pending |-> {},native_enqueued |-> {},
  crash_left |-> PlanMaxCrash,dup_left |-> PlanMaxCrash,int_left |-> PlanMaxCrash,sess_left |-> PlanMaxCrash]

RECURSIVE PlanNativeNoteEffects(_, _)
PlanNativeNoteEffects(p,fx) == IF Len(fx)=0 THEN p ELSE
 LET e == Head(fx)
     next == CASE e.kind="StartRun" -> [p EXCEPT !.pending_start=@ \cup {PNIntent(e.run.thread,e.run.ord,1)},
          !.pending_access=(PNIntent(e.run.thread,e.run.ord,1) :> e.access) @@ @]
       [] e.kind="StartAttempt" -> [p EXCEPT !.pending_start=@ \cup {PNIntent(e.attempt.run.thread,e.attempt.run.ord,e.attempt.n)},
          !.pending_access=(PNIntent(e.attempt.run.thread,e.attempt.run.ord,e.attempt.n) :> e.access) @@ @]
       [] e.kind="InterruptRun" -> [p EXCEPT !.stop_requested=@ \cup {e.run.thread}]
       [] e.kind="InterruptAttempt" -> [p EXCEPT !.stop_requested=@ \cup {e.attempt.run.thread}]
       [] e.kind="RespondInput" -> [p EXCEPT !.answer_pending=@ \cup {PNAnswer(e.child,e.qid,e.epoch,e.answer)}]
       [] e.kind="DeliverNative" -> [p EXCEPT !.native_pending=@ \cup {<<e.parent,e.key,e.generation>>}]
       [] OTHER -> p
 IN PlanNativeNoteEffects(next,Tail(fx))
PNRoom(p) == Len(p.inbox)<PlanMaxInbox
PNEmit(p,e) == [p EXCEPT !.inbox=Append(@,e)]
PNCurrent(c,t,r,a) == t \in DOMAIN c.threads /\ IsLive(c,t) /\ RunOrd(c,t)=r /\ c.threads[t].attempt.n=a
PlanNativeHandleMatches(p,t,r,a) == p.issued_handle[t].present /\ p.issued_handle[t].run=r /\ p.issued_handle[t].attempt=a
PlanNativeHandleCurrent(c,p,t) == p.issued_handle[t].present /\
 PNCurrent(c,t,p.issued_handle[t].run,p.issued_handle[t].attempt)
PlanNativeHandleAllows(c,p,t,access) == PlanNativeHandleCurrent(c,p,t) /\ access<=p.issued_handle[t].access
PlanNativeCallerCurrent(c,p,t) == PlanNativeHandleCurrent(c,p,t) /\
 p.callable_serial[t]=p.issued_handle[t].serial
PNRootLive(p,t) == p.parents[t].session /\ p.parents[t].root /\ ~p.parents[t].recovering
PNKillForeground(p,t) == [p EXCEPT !.natives=[k \in DOMAIN @ |->
 IF k[1]=t /\ @[k].status="running" /\ ~@[k].background THEN [@[k] EXCEPT !.status="dead"] ELSE @[k]]]
PNKillAll(p,t) == [p EXCEPT !.natives=[k \in DOMAIN @ |->
 IF k[1]=t /\ @[k].status="running" THEN [@[k] EXCEPT !.status="dead"] ELSE @[k]]]

(* Install is an independently scheduled source operation, not an effect
   of the logical intent. MCP can run after this step but before stream
   return. Captures need an actual handle/admission fact for this boundary. *)
PlanProviderHandleInstall(c,p,c2,p2,a,fx) ==
 LET intent == PNIntent(a.parent,a.run,a.attempt) h == p.issued_handle[a.parent] IN
 /\ intent \in p.pending_start /\ intent \in DOMAIN p.pending_access
 /\ PNCurrent(c,a.parent,a.run,a.attempt) /\ a.access=p.pending_access[intent]
 /\ ~PlanNativeHandleMatches(p,a.parent,a.run,a.attempt) /\ c2=c /\ fx= <<>>
 /\ p2=[p EXCEPT !.issued_handle[a.parent]=[present |-> TRUE,serial |-> h.serial+1,
       run |-> a.run,attempt |-> a.attempt,access |-> a.access]]
(* The actual request, including its source token, enters harness.run only
   after CT preparation. The joined caller checks the exact CT-ready request.
   Installation alone neither issues a token nor starts a result stream. *)
PlanProviderHarnessEnter(c,p,c2,p2,a,fx) ==
 /\ PlanNativeHandleMatches(p,a.parent,a.run,a.attempt)
 /\ PNIntent(a.parent,a.run,a.attempt) \in p.pending_start
 /\ p.callable_serial[a.parent] # p.issued_handle[a.parent].serial
 /\ c2=c /\ fx= <<>>
 /\ p2=[p EXCEPT !.callable_serial[a.parent]=p.issued_handle[a.parent].serial]
(* Accepted configuration may change access before the provider confirms
   the new attempt. Queuing a steer never rebinds the handle attempt. *)
PlanProviderSteerAccepted(c,p,c2,p2,a,fx) ==
 LET intent == PNIntent(a.parent,a.run,a.attempt) h == p.issued_handle[a.parent]
     row == [intent |-> intent,serial |-> h.serial] IN
 /\ h.present /\ intent \in p.pending_start /\ intent \in DOMAIN p.pending_access
 /\ ~PlanNativeHandleMatches(p,a.parent,a.run,a.attempt)
 /\ a.access=p.pending_access[intent] /\ a.reconfigure \in BOOLEAN
 /\ Len(p.accepted_steers[a.parent])<PlanMaxInbox
 /\ ~\E i \in 1..Len(p.accepted_steers[a.parent]):p.accepted_steers[a.parent][i].intent=intent
 /\ c2=c /\ fx= <<>>
 /\ p2=[p EXCEPT !.accepted_steers[a.parent]=Append(@,row),
       !.issued_handle[a.parent].access=IF a.reconfigure THEN a.access ELSE @]
(* Actual Steered consumes the oldest accepted request on that source
   handle. It may be stale relative to the current logical attempt. *)
PlanProviderSteered(c,p,c2,p2,a,fx) ==
 /\ p.accepted_steers[a.parent] # <<>> /\ p.parents[a.parent].session
 /\ LET row == Head(p.accepted_steers[a.parent]) i == row.intent IN
    /\ p.issued_handle[a.parent].present /\ row.serial=p.issued_handle[a.parent].serial
    /\ a.run=i.run /\ a.attempt=i.attempt /\ c2=c /\ fx= <<>>
    /\ p2=[p EXCEPT !.accepted_steers[a.parent]=Tail(@),!.pending_start=@ \ {i},
       !.issued_handle[a.parent].run=i.run,!.issued_handle[a.parent].attempt=i.attempt,
       !.parents[a.parent].root=TRUE,!.parents[a.parent].run=i.run,!.parents[a.parent].attempt=i.attempt]
PlanProviderStart(c,p,c2,p2,a,fx) ==
 /\ a.parent \in DOMAIN p.parents /\ a.run<=PlanMaxRuns
 /\ PNIntent(a.parent,a.run,a.attempt) \in p.pending_start
 /\ PlanNativeHandleMatches(p,a.parent,a.run,a.attempt)
 /\ p.callable_serial[a.parent]=p.issued_handle[a.parent].serial
 /\ PNCurrent(c,a.parent,a.run,a.attempt) /\ ~p.parents[a.parent].root
 /\ c2=c /\ fx= <<>>
 /\ p2=[p EXCEPT !.pending_start=@ \ {PNIntent(a.parent,a.run,a.attempt)},
      !.parents[a.parent]=[session |-> TRUE,root |-> TRUE,run |-> a.run,
        attempt |-> a.attempt,recovering |-> FALSE,end_by |-> "none"]]
PlanProviderSpawn(c,p,c2,p2,a,fx) ==
 LET k == <<a.parent,a.key>> n == p.natives[k] root == p.parents[a.parent] IN
 /\ k \in DOMAIN p.natives /\ PNRootLive(p,a.parent) /\ PNRoom(p)
 /\ n.status \in {"none","done","dead"} /\ n.epoch<PlanMaxEpoch
 /\ a.epoch=n.epoch+1 /\ c2=c /\ fx= <<>>
 /\ p2=PNEmit([p EXCEPT !.natives[k]=[status |-> "running",epoch |-> a.epoch,
      run |-> root.run,attempt |-> root.attempt,background |-> a.background,progress |-> FALSE]],
      PNEvent("spawn",a.parent,a.key,a.epoch,root.run,root.attempt,a.background,"none"))
PlanProviderProgress(c,p,c2,p2,a,fx) ==
 LET k == <<a.parent,a.key>> n == p.natives[k] IN
 /\ n.status="running" /\ ~n.progress /\ PNRoom(p) /\ c2=c /\ fx= <<>>
 /\ p2=PNEmit([p EXCEPT !.natives[k].progress=TRUE],
      PNEvent("progress",a.parent,a.key,n.epoch,n.run,n.attempt,n.background,"none"))
PlanProviderNativeEnd(c,p,c2,p2,a,fx) ==
 LET k == <<a.parent,a.key>> n == p.natives[k] IN
 /\ n.status="running" /\ p.parents[a.parent].session /\ PNRoom(p)
 /\ a.status \in {"completed","failed"} /\ c2=c /\ fx= <<>>
 /\ p2=PNEmit([p EXCEPT !.natives[k].status="done"],
      PNEvent("end",a.parent,a.key,n.epoch,n.run,n.attempt,n.background,a.status))
PlanProviderRootEnd(c,p,c2,p2,a,fx) ==
 LET root == p.parents[a.parent] IN
 /\ PNRootLive(p,a.parent) /\ PNRoom(p) /\ c2=c /\ fx= <<>>
 /\ a.status \in {"completed","failed","interrupted"}
 /\ (a.status="interrupted" => p.int_left>0)
 /\ (a.status="completed" => ~\E k \in DOMAIN p.natives:
        k[1]=a.parent /\ p.natives[k].status="running" /\ ~p.natives[k].background)
 /\ p2=PNEmit([PNKillForeground(p,a.parent) EXCEPT !.parents[a.parent].root=FALSE,!.int_left=IF a.status="interrupted" THEN @-1 ELSE @],
      PNEvent("rootEnd",a.parent,PNNone,0,root.run,root.attempt,FALSE,a.status))
PlanProviderSessionEnd(c,p,c2,p2,a,fx) ==
 /\ p.parents[a.parent].session /\ ~p.parents[a.parent].root /\ p.sess_left>0 /\ PNRoom(p)
 /\ c2=c /\ fx= <<>>
 /\ p2=PNEmit([PNKillAll(p,a.parent) EXCEPT !.parents[a.parent].session=FALSE,!.sess_left=@-1],
      PNEvent("sessEnd",a.parent,PNNone,0,p.parents[a.parent].run,p.parents[a.parent].attempt,FALSE,"none"))
(* A confirmed provider stop is a physical process fact. Root Done does
   not imply it. A reducer InterruptRun/Attempt only requests it; the actual
   stop acknowledgement is a separate transition. *)
PlanProviderStopConfirmed(c,p,c2,p2,a,fx) ==
 /\ a.parent \in p.stop_requested /\ p.parents[a.parent].session
 /\ ~p.parents[a.parent].root /\ PNRoom(p)
 /\ c2=c /\ fx= <<>>
 /\ p2=PNEmit([PNKillAll(p,a.parent) EXCEPT !.stop_requested=@ \ {a.parent},
      !.parents[a.parent].session=FALSE,!.parents[a.parent].root=FALSE],
      PNEvent("sessEnd",a.parent,PNNone,0,p.parents[a.parent].run,p.parents[a.parent].attempt,FALSE,"cancelled"))

(* An interrupted root terminal is an observed callback. A parked session
   has no active root and cannot create a second root terminal. *)
PlanProviderInterruptedTerminal(c,p,c2,p2,a,fx) ==
 LET root == p.parents[a.parent] IN
 /\ a.parent \in p.stop_requested /\ PNRootLive(p,a.parent) /\ PNRoom(p)
 /\ c2=c /\ fx= <<>>
 /\ p2=PNEmit([PNKillForeground(p,a.parent) EXCEPT !.parents[a.parent].root=FALSE],
      PNEvent("rootEnd",a.parent,PNNone,0,root.run,root.attempt,FALSE,"interrupted"))

PlanProviderCrash(c,p,c2,p2,a,fx) ==
 /\ p.crash_left>0 /\ ~p.parents[a.parent].recovering /\ c2=c /\ fx= <<>>
 /\ p2=[PNKillAll(p,a.parent) EXCEPT !.crash_left=@-1,
    !.parents[a.parent].session=FALSE,!.parents[a.parent].root=FALSE,
    !.parents[a.parent].recovering=TRUE,
    !.inbox=SelectSeq(@,LAMBDA e:e.parent # a.parent)]

PNCoreEvent(c,p,e,a) ==
 [act |-> IF e.kind="spawn" THEN "NativeStart" ELSE "NativeEnd",
  args |-> IF e.kind="spawn" THEN [parent |-> e.parent,key |-> e.key,background |-> e.background]
           ELSE [parent |-> e.parent,key |-> e.key,failed |-> e.status # "completed"],
  native_order |-> IF e.kind="spawn" /\ <<e.parent,e.key>> \notin DOMAIN c.natives
       THEN Append(c.native_order,<<e.parent,e.key>>) ELSE c.native_order,
  question_order |-> c.question_order]
PlanNativeObserve(c,p,c2,p2,a,fx) ==
 /\ p.inbox # <<>>
 /\ LET e == Head(p.inbox) k == <<e.parent,e.key>> IN
    /\ e.kind \in {"spawn","progress","end"}
    /\ IF e.kind="progress" THEN c2=c /\ fx= <<>> /\
         p2=[p EXCEPT !.inbox=Tail(@),!.replay=@ \cup {e}]
       ELSE LET ev == PNCoreEvent(c,p,e,a) IN
         IF NQRejectName(c,ev) # "accepted" THEN
           /\ NativeQuestionReject(c,ev,NQRejectName(c,ev)) /\ c2=c /\ fx= <<>>
           /\ p2=[p EXCEPT !.inbox=Tail(@),!.replay=@ \cup {e}]
         ELSE /\ NativeQuestionApply(c,c2,ev,fx)
              /\ p2=PlanNativeNoteEffects([p EXCEPT !.inbox=Tail(@),!.replay=@ \cup {e},
                   !.doc_epoch[k]=IF e.kind="spawn" /\ k \notin DOMAIN c.natives THEN e.epoch ELSE @,
                   !.doc_how[k]=IF e.kind="end" /\ c.natives[k].status="Running" THEN
                       IF c2.natives[k].delivery="Claimed" THEN "late" ELSE "event" ELSE @,
                   !.closed_epoch[k]=IF e.kind="end" /\ c.natives[k].status="Running" THEN e.epoch ELSE @],fx)


PlanQuestionInput(c,p,c2,p2,a,fx) ==
 /\ a.child \in DOMAIN p.parents /\ PNRootLive(p,a.child)
 /\ PNCurrent(c,a.child,p.parents[a.child].run,p.parents[a.child].attempt)
 /\ <<a.child,a.qid>> \notin p.inputs /\ c2=c /\ fx= <<>>
 /\ p2=[p EXCEPT !.inputs=@ \cup {<<a.child,a.qid>>},
      !.input_owner=(<<a.child,a.qid>> :> PNIntent(a.child,p.parents[a.child].run,p.parents[a.child].attempt)) @@ @]
PlanQuestionApply(c,p,c2,p2,ev,fx) ==
 /\ ev.act \in NQActions \ {"NativeStart","NativeStartNested","NativeEnd","NativeClaim","NativeDelivered","NativeRecover"}
 /\ (ev.act="RaiseQuestion" => <<ev.args.child,ev.args.qid>> \in p.inputs)
 /\ NativeQuestionApply(c,c2,ev,fx)
 /\ p2=PlanNativeNoteEffects(p,fx)
PlanQuestionApplyAnswer(c,p,c2,p2,a,fx) ==
 LET ans == PNAnswer(a.child,a.qid,a.epoch,a.answer) IN
 /\ ans \in p.answer_pending /\ <<a.child,a.qid>> \in p.inputs
 /\ PNRootLive(p,a.child)
 /\ p.input_owner[<<a.child,a.qid>>]=PNIntent(a.child,p.parents[a.child].run,p.parents[a.child].attempt)
 /\ ~\E old \in p.answered: old.child=a.child /\ old.qid=a.qid
 /\ c2=c /\ fx= <<>>
 /\ p2=[p EXCEPT !.answer_pending=@ \ {ans},!.answered=@ \cup {ans}]

PlanNativeEnqueue(c,p,c2,p2,a,fx) ==
 LET k == <<a.parent,a.key>> target == <<a.parent,a.key,a.generation>> IN
 /\ target \in p.native_pending /\ k \in DOMAIN c.natives
 /\ c.natives[k].delivery="Claimed" /\ c.natives[k].generation=a.generation /\ fx= <<>>
 /\ IF k \in p.native_enqueued THEN c2=c /\ p2=p
    ELSE /\ a.parent \in DOMAIN c.threads /\ Len(c.threads[a.parent].queue)<c.config.max_queue
         /\ c2=Finalize([c EXCEPT !.threads[a.parent].queue=Append(@,[kind |-> "Server",value |-> a.msg])])
         /\ p2=[p EXCEPT !.native_enqueued=@ \cup {k}]
PlanNativeDeliveryApply(c,p,c2,p2,ev,fx) ==
 /\ ev.act \in {"NativeClaim","NativeRecover","NativeDelivered"}
 /\ (ev.act="NativeDelivered" =>
      IF <<ev.args.parent,ev.args.key>> \in DOMAIN c.natives THEN
        (c.natives[<<ev.args.parent,ev.args.key>>].delivery="Claimed" /\
         c.natives[<<ev.args.parent,ev.args.key>>].generation=ev.args.generation)
           => <<ev.args.parent,ev.args.key>> \in p.native_enqueued ELSE TRUE)
 /\ NativeQuestionApply(c,c2,ev,fx)
 /\ p2=PlanNativeNoteEffects(p,fx)

PNOutcome(out) == CASE out="completed" -> "Completed" [] out="interrupted" -> "Interrupted" [] out="failed" -> "Failed" [] OTHER -> "Crash"
PlanNativeRootObserve(c,p,c2,p2,a,fx) ==
 /\ p.inbox # <<>>
 /\ LET e == Head(p.inbox)
        ev == [act |-> "ProviderTerminal",args |-> [t |-> e.parent,run |-> e.run,attempt |-> e.attempt,outcome |-> PNOutcome(e.status)],thread_order |-> c.thread_order]
    IN /\ e.kind="rootEnd"
       /\ IF RDRejectName(c,ev)="accepted" THEN RunDelegationApply(c,c2,ev,fx)
          ELSE /\ RunDelegationReject(c,ev,RDRejectName(c,ev)) /\ c2=c /\ fx= <<>>
       /\ p2=PlanNativeNoteEffects([p EXCEPT !.inbox=Tail(@),!.replay=@ \cup {e}],fx)
PlanNativeSessionObserve(c,p,c2,p2,a,fx) ==
 /\ p.inbox # <<>> /\ c2=c /\ fx= <<>>
 /\ LET e == Head(p.inbox) IN
    /\ e.kind="sessEnd"
    /\ p2=[p EXCEPT !.inbox=Tail(@),!.parents[e.parent].recovering=TRUE]
PlanNativeRecover(c,p,c2,p2,a,fx) ==
 /\ p.parents[a.parent].recovering
 /\ RunDelegationApply(c,c2,[act |-> "CrashEngine",args |-> [baseline |-> TRUE],thread_order |-> c.thread_order],fx)
 /\ p2=[PlanNativeNoteEffects(p,fx) EXCEPT !.parents[a.parent].recovering=FALSE]

PlanNativeSteps(c,p,c2,p2,ev,fx) ==
 CASE ev.act="ProviderHandleInstall" -> PlanProviderHandleInstall(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderHarnessEnter" -> PlanProviderHarnessEnter(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderSteerAccepted" -> PlanProviderSteerAccepted(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderSteered" -> PlanProviderSteered(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderStart" -> PlanProviderStart(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderSpawn" -> PlanProviderSpawn(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderProgress" -> PlanProviderProgress(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderNativeEnd" -> PlanProviderNativeEnd(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderRootEnd" -> PlanProviderRootEnd(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderSessionEnd" -> PlanProviderSessionEnd(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderStopConfirmed" -> PlanProviderStopConfirmed(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderInterruptedTerminal" -> PlanProviderInterruptedTerminal(c,p,c2,p2,ev.args,fx)
 [] ev.act="ProviderCrash" -> PlanProviderCrash(c,p,c2,p2,ev.args,fx)
 [] ev.act="EngineNativeObserve" -> PlanNativeObserve(c,p,c2,p2,ev.args,fx)
 [] ev.act="EngineRootObserve" -> PlanNativeRootObserve(c,p,c2,p2,ev.args,fx)
 [] ev.act="EngineSessionObserve" -> PlanNativeSessionObserve(c,p,c2,p2,ev.args,fx)
 [] ev.act="EngineRecover" -> PlanNativeRecover(c,p,c2,p2,ev.args,fx)
 [] ev.act="QuestionInput" -> PlanQuestionInput(c,p,c2,p2,ev.args,fx)
 [] ev.act="QuestionApply" -> PlanQuestionApply(c,p,c2,p2,ev.args.event,fx)
 [] ev.act="QuestionApplyAnswer" -> PlanQuestionApplyAnswer(c,p,c2,p2,ev.args,fx)
 [] ev.act="NativeEnqueue" -> PlanNativeEnqueue(c,p,c2,p2,ev.args,fx)
 [] ev.act="NativeDeliveryApply" -> PlanNativeDeliveryApply(c,p,c2,p2,ev.args.event,fx)
 [] OTHER -> FALSE

(* A source run action calls this after its own exact reducer step. It
   records engine orphan closure metadata, never provider truth. by is the
   explicit source cause: root/recovery/none. Native backgrounds remain as
   the source reducer left them; session/crash omissions can thus fail an
   independent native safety property rather than being repaired here. *)
PlanNativeNoteCore(c,c2,p,by) == [p EXCEPT
 !.parents=[t \in DOMAIN @ |-> IF t \in DOMAIN c.threads /\ t \in DOMAIN c2.threads /\
   IsLive(c,t) /\ ~IsLive(c2,t) THEN [@[t] EXCEPT !.end_by=by] ELSE @[t]],
 !.doc_how=[k \in DOMAIN @ |-> IF k \in DOMAIN c2.natives /\ c2.natives[k].status="Orphaned" /\
   (k \notin DOMAIN c.natives \/ c.natives[k].status # "Orphaned") THEN "orphan" ELSE @[k]],
 !.closed_epoch=[k \in DOMAIN @ |-> IF k \in DOMAIN c2.natives /\ c2.natives[k].status="Orphaned" /\
   (k \notin DOMAIN c.natives \/ c.natives[k].status # "Orphaned") THEN p.doc_epoch[k] ELSE @[k]]]
PlanNativeCoreBoundary(c,c2,p,fx) ==
 LET ended == [p EXCEPT !.parents=[t \in DOMAIN @ |->
        IF t \in DOMAIN c.threads /\ t \in DOMAIN c2.threads /\ IsLive(c,t) /\ ~IsLive(c2,t)
        THEN [p.parents[t] EXCEPT !.end_by=IF p.parents[t].recovering THEN "recovery" ELSE "root"] ELSE p.parents[t]]]
 IN PlanNativeNoteEffects([PlanNativeNoteCore(c,c2,ended,"root") EXCEPT !.parents=ended.parents],fx)
PNImageEvent(e) == [k |-> e.kind,s |-> e.key,ep |-> e.epoch,r |-> e.run,bg |-> e.background,out |-> e.status]

PNStatus(n) == CASE n.status="Running" -> "running" [] n.status="Completed" -> "completed"
 [] n.status="Failed" -> "failed" [] n.status="Orphaned" -> "interrupted" [] OTHER -> "cancelled"
PNDoc(c,k) == IF k \in DOMAIN c.natives THEN PNStatus(c.natives[k]) ELSE "none"
PlanNativeImage(c,p,focus) ==
 LET subs == {k[2]:k \in {x \in DOMAIN p.natives:x[1]=focus}}
     on == focus \in DOMAIN c.threads
     q == p.parents[focus]
 IN [nRst |-> IF on /\ IsLive(c,focus) THEN "live" ELSE "idle",
  nRord |-> IF on THEN RunOrd(c,focus) ELSE 0,nEndBy |-> q.end_by,nRec |-> q.recovering,
  nEst |-> [s \in subs |-> PNDoc(c,<<focus,s>>)],nTask |-> [s \in subs |-> PNDoc(c,<<focus,s>>)],
  nEep |-> [s \in subs |-> p.doc_epoch[<<focus,s>>]],
  nErun |-> [s \in subs |-> IF <<focus,s>> \in DOMAIN c.natives THEN c.natives[<<focus,s>>].attempt.run.ord ELSE 0],
  nEbg |-> [s \in subs |-> IF <<focus,s>> \in DOMAIN c.natives THEN c.natives[<<focus,s>>].background ELSE FALSE],
  nHow |-> [s \in subs |-> p.doc_how[<<focus,s>>]],nClosedEp |-> [s \in subs |-> p.closed_epoch[<<focus,s>>]],
  nSess |-> q.session,nProot |-> q.root,
  nPst |-> [s \in subs |-> p.natives[<<focus,s>>].status],
  nPep |-> [s \in subs |-> p.natives[<<focus,s>>].epoch],
  nPrun |-> [s \in subs |-> p.natives[<<focus,s>>].run],
  nPbg |-> [s \in subs |-> p.natives[<<focus,s>>].background],
  nPprog |-> [s \in subs |-> p.natives[<<focus,s>>].progress],
  nInbox |-> LET es == SelectSeq(p.inbox,LAMBDA e:e.parent=focus) IN [i \in 1..Len(es) |-> PNImageEvent(es[i])],
  nReplay |-> {PNImageEvent(e):e \in {x \in p.replay:x.parent=focus}},
  nCrashLeft |-> p.crash_left,nDupLeft |-> p.dup_left,nIntLeft |-> p.int_left,nSessLeft |-> p.sess_left]

(* Finite, independently enabled choices for the free-running runner. These
   sets use only the current state and finite input IDs. They do not consult
   a target Next relation or an observed successor. One immutable request is
   admitted in this tranche; source actions can still route its epochs. *)
PNNormalized(c,act,args,qorder) == [act |-> act,args |-> args,question_order |-> qorder,native_order |-> c.native_order]
PNQWrap(e) == [act |-> "QuestionApply",args |-> [event |-> e]]
PNDWrap(e) == [act |-> "NativeDeliveryApply",args |-> [event |-> e]]
PlanQuestionEvents(c,p,Threads,Qids) ==
 LET present == Threads \cap DOMAIN c.threads
     ready == {t \in present:PNRootLive(p,t) /\ PNCurrent(c,t,p.parents[t].run,p.parents[t].attempt)}
     inputs == IF DOMAIN c.questions={} /\ p.inputs={} THEN
       {[act |-> "QuestionInput",args |-> [child |-> t,qid |-> q]]:t \in ready,q \in Qids} ELSE {}
     raises == IF DOMAIN c.questions={} THEN
       {PNNormalized(c,"RaiseQuestion",[child |-> k[1],qid |-> k[2]],Append(c.question_order,k)):k \in p.inputs} ELSE {}
     starts == {PNNormalized(c,"StartQuestion",[t |-> t],c.question_order):t \in present}
     messages == {m \in c.outbox:m.kind \in {"QuestionUp","AnswerDown","QuestionRelease","QuestionClosed"} /\ m.epoch<=PlanMaxEpoch}
     deliveries == {PNNormalized(c,"DeliverQuestion",[message |-> m,how |-> "Ack"],c.question_order):m \in messages}
     pending == {k \in DOMAIN c.questions:c.questions[k].pending /\ c.questions[k].epoch<=PlanMaxEpoch}
     agents == {PNNormalized(c,"AgentAnswer",[agent |-> t,child |-> k[1],qid |-> k[2],epoch |-> c.questions[k].epoch,answer |-> "1"],c.question_order):t \in present,k \in pending}
     persons == {PNNormalized(c,"PersonAnswer",[child |-> k[1],qid |-> k[2],epoch |-> c.questions[k].epoch,answer |-> "1"],c.question_order):k \in pending \cap p.inputs}
     apply == {PNQWrap(e):e \in {v \in raises \cup starts \cup deliveries \cup agents \cup persons:NQRejectName(c,v)="accepted"}}
     responses == {[act |-> "QuestionApplyAnswer",args |-> a]:a \in {v \in p.answer_pending:
        <<v.child,v.qid>> \in p.inputs /\ PNRootLive(p,v.child) /\
        p.input_owner[<<v.child,v.qid>>]=PNIntent(v.child,p.parents[v.child].run,p.parents[v.child].attempt) /\ ~\E old \in p.answered:old.child=v.child /\ old.qid=v.qid}}
 IN inputs \cup apply \cup responses
PlanNativeDeliveryEvents(c,p) ==
 LET keys == DOMAIN c.natives
     claims == {PNNormalized(c,"NativeClaim",[parent |-> k[1],key |-> k[2]],c.question_order):k \in keys}
     recovering == {k \in keys:c.natives[k].delivery="Claimed" /\ c.natives[k].generation<PlanMaxEpoch}
     recovers == {PNNormalized(c,"NativeRecover",[parent |-> k[1],key |-> k[2]],c.question_order):k \in recovering}
     acknowledgements == {PNNormalized(c,"NativeDelivered",[parent |-> k[1],key |-> k[2],generation |-> g],c.question_order):k \in keys,g \in 0..PlanMaxEpoch}
     accepted == {e \in claims \cup recovers \cup acknowledgements:NQRejectName(c,e)="accepted"}
     ackable == {e \in accepted:IF e.act="NativeDelivered" THEN
       (c.natives[<<e.args.parent,e.args.key>>].delivery="Claimed" /\ c.natives[<<e.args.parent,e.args.key>>].generation=e.args.generation)
         => <<e.args.parent,e.args.key>> \in p.native_enqueued ELSE TRUE}
     enqueues == {[act |-> "NativeEnqueue",args |-> [parent |-> x[1],key |-> x[2],generation |-> x[3],msg |-> "1"]]:
       x \in {v \in p.native_pending:<<v[1],v[2]>> \in keys /\ c.natives[<<v[1],v[2]>>].delivery="Claimed" /\
            c.natives[<<v[1],v[2]>>].generation=v[3] /\ <<v[1],v[2]>> \notin p.native_enqueued /\
            v[1] \in DOMAIN c.threads /\ Len(c.threads[v[1]].queue)<c.config.max_queue}}
 IN {PNDWrap(e):e \in ackable} \cup enqueues

(* Fair-runner obligations: weak fairness for EngineNativeObserve and each
   continuously pending ProviderStart/QuestionApplyAnswer; strong fairness
   for each durable question transport delivery while its row recurs.
   A direct fair question witness is Input -> Raise -> DeliverUp -> Start
   -> AgentAnswer -> DeliverAnswer -> ApplyAnswer -> DeliverClosed.
   A native late-delivery witness is StartIntent -> ProviderStart -> Spawn
   -> Observe -> RootEnd -> engine root ingest -> NativeEnd -> Observe
   -> durable parent enqueue -> NativeDelivered. Actual queue receipts, not
   provider reply acks, discharge the final delivery obligation. *)
=============================================================================
