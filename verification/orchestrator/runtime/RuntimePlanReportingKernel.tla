---------------- MODULE RuntimePlanReportingKernel ----------------
(* Independent operational result-stream kernel. No target Init/Next is used.
   A source ID is one actual runtime handle, not a conversation ID. The caller
   bounds IDs, revisions and events. EOF says nothing about process death.
   Activation remains unknown in this finite scope. Durable document saves
   are separate steps; a core terminal never writes the document image. *)
EXTENDS RuntimeNativeQuestionSeams, RuntimeRunDelegationSeams

PRKEmpty == [x \in {} |-> x]
PRKNone == "none"
PRKNoNative == <<>>
PRKNoCause == [kind |-> "none",source |-> PRKNone,changed |-> {}]
PRKPut(f,k,v) == (k :> v) @@ f
PRKRemove(q,i) == SubSeq(q,1,i-1) \o SubSeq(q,i+1,Len(q))
PRKFirst(p,i) == \A j \in 1..(i-1):
 p.pending[j].source#p.pending[i].source \/
 (p.pending[i].kind#"sourceLoss" /\ p.pending[j].kind#"sourceLoss" /\ p.pending[j].key#p.pending[i].key)
PRKCurrent(c,t,r,a) == IF t \notin DOMAIN c.threads THEN FALSE ELSE
 IsLive(c,t) /\ RunOrd(c,t)=r /\ c.threads[t].attempt.n=a
PRKStatus(n) == CASE n.status="Running" -> "running"
 [] n.status="Completed" -> "completed" [] n.status="Orphaned" -> "interrupted"
 [] n.status="Cancelled" -> "cancelled" [] OTHER -> "failed"
PRKDocStatus(n) == IF n.status="Running" THEN "running" ELSE IF n.status="Completed" THEN "done" ELSE "failed"
PRKEvent(kind,sid,k,r,a,out) == [kind |-> kind,source |-> sid,key |-> k,run |-> r,attempt |-> a,out |-> out]

PlanReportingInit(Threads) ==
 [sources |-> PRKEmpty,natives |-> PRKEmpty,docs |-> PRKEmpty,meta |-> PRKEmpty,
  pending |-> <<>>,handles |-> [t \in Threads |-> PRKNone],
  admitted |-> [t \in Threads |-> [present |-> FALSE]],
  endBy |-> [t \in Threads |-> "none"],lastCause |-> PRKNoCause,
  catalogComplete |-> [t \in Threads |-> FALSE],intents |-> {},access |-> PRKEmpty,inputs |-> {},input_owner |-> PRKEmpty,
  input_source |-> PRKEmpty,answer_pending |-> {},answered |-> {},native_pending |-> {},native_enqueued |-> {},root_running |-> PRKEmpty,eof_root |-> PRKEmpty,stop_pending |-> {},interrupted |-> {}]

RECURSIVE PlanReportingNoteEffects(_, _)
PlanReportingNoteEffects(p,fx) == IF Len(fx)=0 THEN p ELSE
 LET e==Head(fx)
     id==IF e.kind="StartRun" THEN <<e.run.thread,e.run.ord,1>>
         ELSE IF e.kind="StartAttempt" THEN <<e.attempt.run.thread,e.attempt.run.ord,e.attempt.n>> ELSE <<>>
     next==IF id# <<>> THEN [p EXCEPT !.intents=@ \cup {id},!.access=PRKPut(@,id,e.access)]
       ELSE CASE e.kind="RespondInput" -> [p EXCEPT !.answer_pending=@ \cup {[child |-> e.child,qid |-> e.qid,epoch |-> e.epoch,answer |-> e.answer]}]
         [] e.kind="InterruptRun" -> [p EXCEPT !.stop_pending=@ \cup {e.run.thread}]
         [] e.kind="InterruptAttempt" -> [p EXCEPT !.stop_pending=@ \cup {e.attempt.run.thread}]
         [] e.kind="DeliverNative" -> [p EXCEPT !.native_pending=@ \cup {<<e.parent,e.key,e.generation>>}]
         [] OTHER -> p
 IN PlanReportingNoteEffects(next,Tail(fx))

PlanReportingImage(c,p) ==
 [catalogComplete |-> p.catalogComplete,runs |-> [t \in DOMAIN p.handles |->
     [run |-> IF t \in DOMAIN c.threads THEN RunOrd(c,t) ELSE 0,
      attempt |-> IF t \in DOMAIN c.threads THEN c.threads[t].attempt.n ELSE 0,
      active |-> IF t \in DOMAIN c.threads THEN IsLive(c,t) ELSE FALSE,endBy |-> p.endBy[t]]],
  sources |-> p.sources,handles |-> p.handles,admitted |-> p.admitted,
  natives |-> p.natives,
  tasks |-> [k \in DOMAIN c.natives |->
    [source |-> p.meta[k].source,parent |-> k[1],run |-> c.natives[k].attempt.run.ord,
     attempt |-> c.natives[k].attempt.n,status |-> PRKStatus(c.natives[k]),
     background |-> c.natives[k].background,cause |-> p.meta[k].cause,
     activation |-> PRKNone,generation |-> c.natives[k].generation]],
  docTasks |-> p.docs,pending |-> p.pending,lastCause |-> p.lastCause]

(* Actual dispatch emits a Start intent. Installing its handle and issuing
   its prepared request are separate source steps. The joint caller must
   require its own packet-ready predicate at Issue. Rebind is an observed
   Steered event; accepting a prompt does not advance this binding.
   issuedAccess names the selected installed access for compatibility. The
   issuedOrigin/Run/Attempt/Revision/RequestAccess fields hold the last request
   passed to the harness; Config and observed Rebind do not issue a request. *)
PlanReportingSourceStep(c,p,p2,ev) ==
 LET a == ev.args IN
 CASE ev.act="ReportingCatalog" ->
    /\ a.parents \subseteq DOMAIN p.handles
    /\ p2=[p EXCEPT !.catalogComplete=[t \in DOMAIN @ |-> @[t] \/ t \in a.parents],!.lastCause=PRKNoCause]
 [] ev.act="ReportingInstall" ->
    /\ a.source \notin DOMAIN p.sources
    /\ <<a.parent,a.run,a.attempt>> \in p.intents
    /\ PRKCurrent(c,a.parent,a.run,a.attempt)
    /\ a.access=p.access[<<a.parent,a.run,a.attempt>>]
    /\ IF p.handles[a.parent]=PRKNone THEN TRUE ELSE
         LET h==p.sources[p.handles[a.parent]] IN <<h.run,h.attempt>># <<a.run,a.attempt>>
    /\ p2=[p EXCEPT !.sources=PRKPut(@,a.source,
         [parent |-> a.parent,run |-> a.run,attempt |-> a.attempt,reporting |-> "unknown",
          rootActive |-> "unknown",process |-> "unknown",issuedAccess |-> a.access,
          requestIssued |-> FALSE,revision |-> a.revision,origin |-> a.origin,retired |-> FALSE,
          issuedOrigin |-> PRKNone,issuedRun |-> 0,issuedAttempt |-> 0,issuedRevision |-> 0,issuedRequestAccess |-> 0]),
         !.handles[a.parent]=a.source,!.root_running=PRKPut(@,a.source,FALSE),!.lastCause=PRKNoCause]
 [] ev.act \in {"ReportingIssue","ReportingConfig","ReportingRebind"} ->
    /\ a.source \in DOMAIN p.sources
    /\ LET h==p.sources[a.source] t==h.parent IN
       /\ p.handles[t]=a.source /\ ~h.retired /\ a.revision>h.revision
       /\ (ev.act="ReportingIssue" =>
           /\ <<t,h.run,h.attempt>> \in p.intents
           /\ PRKCurrent(c,t,h.run,h.attempt)
           /\ a.access=p.access[<<t,h.run,h.attempt>>])
       /\ (ev.act="ReportingConfig" => \E x \in p.intents:x[1]=t /\ p.access[x]=a.access)
       /\ (ev.act="ReportingRebind" =>
           /\ h.reporting="connected" /\ h.requestIssued
           /\ <<t,a.run,a.attempt>> \in p.intents /\ a.access=h.issuedAccess
           /\ <<h.run,h.attempt>># <<a.run,a.attempt>>)
       /\ p2=[p EXCEPT !.sources[a.source]=[h EXCEPT
             !.run=IF ev.act="ReportingRebind" THEN a.run ELSE @,
             !.attempt=IF ev.act="ReportingRebind" THEN a.attempt ELSE @,
             !.issuedAccess=a.access,!.revision=a.revision,!.origin=a.origin,
             !.requestIssued=IF ev.act="ReportingIssue" THEN TRUE ELSE @,
             !.issuedOrigin=IF ev.act="ReportingIssue" THEN a.origin ELSE @,
             !.issuedRun=IF ev.act="ReportingIssue" THEN h.run ELSE @,
             !.issuedAttempt=IF ev.act="ReportingIssue" THEN h.attempt ELSE @,
             !.issuedRevision=IF ev.act="ReportingIssue" THEN a.revision ELSE @,
             !.issuedRequestAccess=IF ev.act="ReportingIssue" THEN a.access ELSE @],
             !.root_running[a.source]=IF ev.act \in {"ReportingIssue","ReportingRebind"} THEN TRUE ELSE @,!.lastCause=PRKNoCause]
 [] ev.act="ReportingRetire" ->
    /\ a.source \in DOMAIN p.sources /\ a.revision>p.sources[a.source].revision
    /\ LET t==p.sources[a.source].parent IN
       p2=[p EXCEPT !.sources[a.source].retired=TRUE,!.sources[a.source].revision=a.revision,
           !.handles[t]=IF a.remove /\ @=a.source THEN PRKNone ELSE @,!.lastCause=PRKNoCause]
 [] ev.act="ReportingAdmit" ->
    /\ a.source \in DOMAIN p.sources
    /\ LET h==p.sources[a.source] t==h.parent IN
       /\ p.handles[t]=a.source /\ ~h.retired /\ h.requestIssued /\ p.catalogComplete[t]
       /\ PRKCurrent(c,t,h.run,h.attempt)
       /\ p2=[p EXCEPT !.admitted[t]=[present |-> TRUE,source |-> a.source,
            run |-> h.run,attempt |-> h.attempt,revision |-> h.revision,issuedAccess |-> h.issuedAccess],!.lastCause=PRKNoCause]
 [] ev.act="ReportingRefuse" ->
    /\ a.parent \in DOMAIN p.admitted
    /\ p2=[p EXCEPT !.admitted[a.parent]=[present |-> FALSE],!.lastCause=PRKNoCause]
 [] ev.act="ReportingOpen" ->
    /\ a.source \in DOMAIN p.sources /\ p.sources[a.source].requestIssued
    /\ p.sources[a.source].reporting="unknown"
    /\ p2=[p EXCEPT !.sources[a.source].reporting="connected",!.lastCause=PRKNoCause]
 [] ev.act="ReportingRootStart" ->
    /\ a.source \in DOMAIN p.sources /\ p.sources[a.source].reporting="connected"
    /\ p2=[p EXCEPT !.sources[a.source].rootActive="active",!.lastCause=PRKNoCause]
 [] ev.act="ReportingSpawn" ->
    /\ a.source \in DOMAIN p.sources /\ a.key \notin DOMAIN p.natives
    /\ LET h==p.sources[a.source] anc==a.ancestor IN
       /\ h.reporting="connected" /\ a.key[1]=h.parent
       /\ IF anc=PRKNoNative THEN h.rootActive="active"
           ELSE /\ anc \in DOMAIN p.natives /\ p.natives[anc].source=a.source
                /\ p.natives[anc].state="running"
                /\ h.rootActive="active" \/ p.natives[anc].background
       /\ LET owner==IF anc=PRKNoNative THEN h ELSE p.natives[anc]
              row==[source |-> a.source,parent |-> h.parent,run |-> owner.run,attempt |-> owner.attempt,
               ancestor |-> anc,ownBackground |-> a.background,
               background |-> a.background \/ (anc#PRKNoNative /\ p.natives[anc].background),
               state |-> "running",activation |-> PRKNone]
          IN p2=[p EXCEPT !.natives=PRKPut(@,a.key,row),
              !.pending=Append(@,PRKEvent("spawn",a.source,a.key,row.run,row.attempt,"none")),!.lastCause=PRKNoCause]
 [] ev.act="ReportingNativeEnd" ->
    /\ a.key \in DOMAIN p.natives /\ p.natives[a.key].state \in {"running","reported"}
    /\ LET n==p.natives[a.key] IN
       /\ p.sources[n.source].reporting="connected"
       /\ p2=[p EXCEPT !.natives[a.key].state="reported",
           !.pending=Append(@,PRKEvent("nativeEnd",n.source,a.key,n.run,n.attempt,a.outcome)),!.lastCause=PRKNoCause]
 [] ev.act="ReportingRootEnd" ->
    /\ a.source \in DOMAIN p.sources /\ p.sources[a.source].reporting="connected"
    /\ p.sources[a.source].rootActive="active"
    /\ LET h==p.sources[a.source] IN
       p2=[p EXCEPT !.sources[a.source].rootActive="inactive",!.root_running[a.source]=FALSE,
          !.pending=Append(@,PRKEvent("rootEnd",a.source,PRKNoNative,h.run,h.attempt,a.outcome)),!.lastCause=PRKNoCause]
 [] ev.act="ReportingInterrupt" ->
    /\ a.source \in DOMAIN p.sources /\ p.sources[a.source].parent \in p.stop_pending
    /\ p.sources[a.source].reporting="connected"
    /\ p2=[p EXCEPT !.interrupted=@ \cup {a.source}]
 [] ev.act="ReportingEOFRootTerminal" ->
    /\ a.source \in DOMAIN p.eof_root
    /\ LET h==p.sources[a.source] e==p.eof_root[a.source] IN
       /\ h.reporting="disconnected" /\ PRKCurrent(c,h.parent,e.run,e.attempt)
       /\ ~\E i \in 1..Len(p.pending):p.pending[i].kind="rootEnd" /\ p.pending[i].source=a.source /\
             p.pending[i].run=e.run /\ p.pending[i].attempt=e.attempt
       /\ p2=[p EXCEPT !.pending=Append(@,PRKEvent("rootEnd",a.source,PRKNoNative,e.run,e.attempt,e.outcome)),
             !.eof_root=[sid \in DOMAIN @\{a.source} |-> @[sid]],!.lastCause=PRKNoCause]
 [] ev.act="ReportingEOF" ->
    /\ a.source \in DOMAIN p.sources /\ p.sources[a.source].reporting="connected"
    /\ LET h==p.sources[a.source] IN
       p2=[p EXCEPT !.sources[a.source].reporting="disconnected",!.sources[a.source].rootActive="unknown",
          !.eof_root=IF p.root_running[a.source] THEN PRKPut(@,a.source,[run |-> h.run,attempt |-> h.attempt,outcome |-> IF a.source \in p.interrupted THEN "interrupted" ELSE "failed"]) ELSE @,!.root_running[a.source]=FALSE,
          !.natives=[k \in DOMAIN @ |-> IF @[k].source=a.source /\ @[k].state="running"
                       THEN [@[k] EXCEPT !.state="unknown"] ELSE @[k]],
          !.pending=Append(@,PRKEvent("sourceLoss",a.source,PRKNoNative,h.run,h.attempt,"failed")),!.lastCause=PRKNoCause]
 [] OTHER -> FALSE

(* The core relation owns core mutation. This bridge reads its actual before
   and after states and consumes only the matching observed source event.
   A source-loss entry remains until each real task has closed. *)
PlanReportingCoreBoundary(c,c2,p,p2,ev,fx) ==
 LET inserted==DOMAIN c2.natives\DOMAIN c.natives
     terminal=={k \in DOMAIN c.natives \cap DOMAIN c2.natives:
                 c.natives[k].status="Running" /\ c2.natives[k].status \notin {"Running","Orphaned"}}
     roots=={t \in DOMAIN c.threads \cap DOMAIN c2.threads:IsLive(c,t) /\ ~IsLive(c2,t)}
     matches=={i \in 1..Len(p.pending):
       /\ PRKFirst(p,i)
       /\ LET e==p.pending[i] IN
          \/ e.kind="spawn" /\ ev.act \in {"NativeStart","NativeStartNested"} /\ e.key= <<ev.args.parent,ev.args.key>>
          \/ e.kind="nativeEnd" /\ ev.act="NativeEnd" /\ e.key= <<ev.args.parent,ev.args.key>>
          \/ e.kind="rootEnd" /\ ev.act="IngestEnd" /\ e.source \in DOMAIN p.sources
                /\ p.sources[e.source].parent \in roots
                /\ ev.args.ev.run=e.run /\ ev.args.ev.attempt=e.attempt}
     lossKeys=={k \in terminal:~\E i \in matches:p.pending[i].kind="nativeEnd" /\ p.pending[i].key=k}
 IN /\ Cardinality(inserted)<=1 /\ Cardinality(terminal)<=1 /\ Cardinality(roots)<=1 /\ Cardinality(matches)<=1
    /\ \A k \in inserted:\E i \in matches:p.pending[i].kind="spawn" /\ p.pending[i].key=k
          /\ c2.natives[k].attempt.run.ord=p.pending[i].run /\ c2.natives[k].attempt.n=p.pending[i].attempt
    /\ \A t \in roots:\E i \in matches:p.pending[i].kind="rootEnd" /\ p.sources[p.pending[i].source].parent=t
    /\ \A k \in terminal\lossKeys:\E i \in matches:
           p.pending[i].kind="nativeEnd" /\ p.pending[i].key=k /\
           (c2.natives[k].status="Completed")=(p.pending[i].out="completed")
    /\ \A k \in lossKeys:\E i \in 1..Len(p.pending):
          PRKFirst(p,i) /\ p.pending[i].kind="sourceLoss" /\ p.pending[i].source=p.meta[k].source
          /\ c2.natives[k].status="Failed"
    /\ LET i==IF matches={} THEN 0 ELSE CHOOSE j \in matches:TRUE
           consumed==IF i=0 THEN [kind |-> "none",source |-> PRKNone] ELSE p.pending[i]
           meta==[k \in DOMAIN c2.natives |->
             IF k \in inserted THEN [source |-> consumed.source,cause |-> "none"]
             ELSE IF c2.natives[k].status="Orphaned" /\ c.natives[k].status="Running"
                  THEN [p.meta[k] EXCEPT !.cause="orphan"]
             ELSE IF k \in terminal THEN [p.meta[k] EXCEPT !.cause=IF k \in lossKeys THEN "reporting_loss" ELSE "reported"]
             ELSE p.meta[k]]
           changed==IF consumed.kind="rootEnd" THEN
              {k \in DOMAIN c.natives \cap DOMAIN c2.natives:c.natives[k].status="Running" /\ c2.natives[k].status="Orphaned"}
             ELSE lossKeys
           by==[t \in DOMAIN p.endBy |-> IF t \in roots THEN "root"
              ELSE IF t \in DOMAIN c2.threads /\ IsLive(c2,t) /\
                      (IF t \notin DOMAIN c.threads THEN TRUE ELSE ~IsLive(c,t)) THEN "none" ELSE p.endBy[t]]
           base==[p EXCEPT !.meta=meta,!.endBy=by,
                 !.pending=IF i=0 THEN @ ELSE PRKRemove(@,i),
                 !.lastCause=IF lossKeys#{} THEN [kind |-> "sourceLoss",source |-> p.meta[CHOOSE k \in lossKeys:TRUE].source,changed |-> lossKeys]
                    ELSE IF consumed.kind \in {"rootEnd","spawn","nativeEnd"} THEN [kind |-> consumed.kind,source |-> consumed.source,changed |-> changed]
                    ELSE IF \E t \in DOMAIN c2.threads:
                      IF t \notin DOMAIN c.threads THEN RunOrd(c2,t)>0
                      ELSE RunOrd(c2,t)#RunOrd(c,t) \/ c2.threads[t].attempt.n#c.threads[t].attempt.n
                    THEN PRKNoCause
                    ELSE IF \E k \in DOMAIN c.natives \cap DOMAIN c2.natives:c.natives[k].generation#c2.natives[k].generation
                    THEN PRKNoCause ELSE p.lastCause]
       IN p2=PlanReportingNoteEffects(base,fx)

PlanReportingPersist(c,p,p2,k) ==
 /\ k \in DOMAIN c.natives /\ k \in DOMAIN p.meta
 /\ p2=[p EXCEPT !.docs=PRKPut(@,k,[source |-> p.meta[k].source,status |-> PRKDocStatus(c.natives[k])]),!.lastCause=PRKNoCause]

PlanReportingLossConsumed(c,p,p2,i) ==
 /\ i \in 1..Len(p.pending) /\ PRKFirst(p,i) /\ p.pending[i].kind="sourceLoss"
 /\ ~\E k \in DOMAIN c.natives:p.meta[k].source=p.pending[i].source /\ c.natives[k].status="Running"
 /\ p2=[p EXCEPT !.pending=PRKRemove(@,i),
         !.lastCause=[kind |-> "sourceLoss",source |-> p.pending[i].source,changed |-> {}]]

PlanReportingCoreEvent(c,e) ==
 LET k==e.key
     a==IF e.kind="spawn" THEN
          IF e.key \in DOMAIN c.natives THEN "NativeStart"
          ELSE "NativeStart"
        ELSE "NativeEnd"
 IN [act |-> a,args |-> IF e.kind="spawn"
       THEN [parent |-> k[1],key |-> k[2],background |-> FALSE]
       ELSE [parent |-> k[1],key |-> k[2],failed |-> e.out#"completed"],
     native_order |-> IF e.kind="spawn" /\ k \notin DOMAIN c.natives THEN Append(c.native_order,k) ELSE c.native_order,
     question_order |-> c.question_order,thread_order |-> c.thread_order]
PlanReportingNativeEvent(c,p,e) ==
 IF e.kind#"spawn" THEN PlanReportingCoreEvent(c,e)
 ELSE LET n==p.natives[e.key] anc==n.ancestor IN
      [act |-> IF anc=PRKNoNative THEN "NativeStart" ELSE "NativeStartNested",
       args |-> IF anc=PRKNoNative THEN [parent |-> e.key[1],key |-> e.key[2],background |-> n.ownBackground]
                ELSE [parent |-> e.key[1],key |-> e.key[2],ancestor |-> anc[2],background |-> n.ownBackground],
       native_order |-> IF e.key \notin DOMAIN c.natives THEN Append(c.native_order,e.key) ELSE c.native_order,
       question_order |-> c.question_order,thread_order |-> c.thread_order]
PlanReportingCoreEvents(c,p) ==
 LET first=={i \in 1..Len(p.pending):PRKFirst(p,i)}
     native=={PlanReportingNativeEvent(c,p,p.pending[i]):i \in {j \in first:p.pending[j].kind \in {"spawn","nativeEnd"}}}
     losses=={[act |-> "NativeEnd",args |-> [parent |-> k[1],key |-> k[2],failed |-> TRUE],
               native_order |-> c.native_order,question_order |-> c.question_order,thread_order |-> c.thread_order]:
          k \in {key \in DOMAIN c.natives:c.natives[key].status="Running" /\
             \E i \in first:p.pending[i].kind="sourceLoss" /\ p.pending[i].source=p.meta[key].source}}
     roots=={[act |-> "ProviderTerminal",args |-> [t |-> p.sources[p.pending[i].source].parent,
                 run |-> p.pending[i].run,attempt |-> p.pending[i].attempt,
                 outcome |-> IF p.pending[i].out="completed" THEN "Completed" ELSE IF p.pending[i].out="failed" THEN "Failed" ELSE "Interrupted"],
              thread_order |-> c.thread_order]:i \in {j \in first:p.pending[j].kind="rootEnd"}}
 IN native \cup losses \cup roots

PlanReportingRejectNative(c,p,p2,i) ==
 /\ i \in 1..Len(p.pending) /\ PRKFirst(p,i)
 /\ p.pending[i].kind \in {"spawn","nativeEnd"}
 /\ LET ev==PlanReportingNativeEvent(c,p,p.pending[i]) IN NQRejectName(c,ev)#"accepted"
 /\ p2=[p EXCEPT !.pending=PRKRemove(@,i),!.lastCause=PRKNoCause]
PlanReportingIgnoreRoot(c,p,p2,i) ==
 /\ i \in 1..Len(p.pending) /\ PRKFirst(p,i) /\ p.pending[i].kind="rootEnd"
 /\ LET e==p.pending[i] IN ~PRKCurrent(c,p.sources[e.source].parent,e.run,e.attempt)
 /\ p2=[p EXCEPT !.pending=PRKRemove(@,i),
        !.lastCause=[kind |-> "rootEnd",source |-> p.pending[i].source,changed |-> {}]]

PRKQuestionReady(c,p,sid) ==
 LET h==p.sources[sid] IN h.reporting="connected" /\ h.rootActive="active" /\ PRKCurrent(c,h.parent,h.run,h.attempt)
PRKQEvent(c,act,args,order) == [act |-> act,args |-> args,question_order |-> order,native_order |-> c.native_order]
PRKAnswer(a) == [child |-> a.child,qid |-> a.qid,epoch |-> a.epoch,answer |-> a.answer]
PlanReportingBridgeStep(c,p,c2,p2,ev,fx) ==
 LET a==ev.args IN
 CASE ev.act="QuestionInput" ->
   /\ a.source \in DOMAIN p.sources /\ PRKQuestionReady(c,p,a.source)
   /\ p.sources[a.source].parent=a.child /\ <<a.child,a.qid>> \notin p.inputs
   /\ c2=c /\ fx= <<>>
   /\ p2=[p EXCEPT !.inputs=@ \cup {<<a.child,a.qid>>},
       !.input_owner=PRKPut(@,<<a.child,a.qid>>,<<a.child,p.sources[a.source].run,p.sources[a.source].attempt>>),
       !.input_source=PRKPut(@,<<a.child,a.qid>>,a.source)]
 [] ev.act="QuestionApply" ->
   /\ (a.event.act="RaiseQuestion" => <<a.event.args.child,a.event.args.qid>> \in p.inputs)
   /\ NativeQuestionApply(c,c2,a.event,fx)
   /\ PlanReportingCoreBoundary(c,c2,p,p2,a.event,fx)
 [] ev.act="QuestionApplyAnswer" ->
   /\ PRKAnswer(a) \in p.answer_pending /\ <<a.child,a.qid>> \in p.inputs
   /\ LET sid==p.input_source[<<a.child,a.qid>>] h==p.sources[sid] IN
       /\ h.reporting="connected" /\ h.rootActive="active"
       /\ p.input_owner[<<a.child,a.qid>>]= <<a.child,h.run,h.attempt>>
   /\ ~\E old \in p.answered:old.child=a.child /\ old.qid=a.qid
   /\ c2=c /\ fx= <<>>
   /\ p2=[p EXCEPT !.answer_pending=@\{PRKAnswer(a)},!.answered=@ \cup {PRKAnswer(a)}]
 [] ev.act="NativeEnqueue" ->
   /\ <<a.parent,a.key,a.generation>> \in p.native_pending
   /\ <<a.parent,a.key>> \in DOMAIN c.natives
   /\ c.natives[<<a.parent,a.key>>].delivery="Claimed"
   /\ c.natives[<<a.parent,a.key>>].generation=a.generation
   /\ <<a.parent,a.key>> \notin p.native_enqueued
   /\ LET ce==[act |-> "ServerEnqueue",args |-> [t |-> a.parent,msg |-> a.msg],thread_order |-> c.thread_order]
          raw==RDRawResult(c,ce) next==Finalize(raw.state)
          base==PlanReportingNoteEffects(p,raw.fx \o ReconcileEffects(raw.state))
      IN /\ RDGuard(c,ce) /\ c2=next /\ fx=raw.fx \o ReconcileEffects(raw.state)
         /\ p2=[base EXCEPT !.native_enqueued=@ \cup {<<a.parent,a.key>>}]
 [] ev.act="NativeDeliveryApply" ->
   /\ a.event.act \in {"NativeClaim","NativeRecover","NativeDelivered"}
   /\ (a.event.act="NativeDelivered" =>
       IF <<a.event.args.parent,a.event.args.key>> \in DOMAIN c.natives THEN
        (c.natives[<<a.event.args.parent,a.event.args.key>>].delivery="Claimed" /\
         c.natives[<<a.event.args.parent,a.event.args.key>>].generation=a.event.args.generation)
         => <<a.event.args.parent,a.event.args.key>> \in p.native_enqueued ELSE TRUE)
   /\ NativeQuestionApply(c,c2,a.event,fx)
   /\ PlanReportingCoreBoundary(c,c2,p,p2,a.event,fx)
 [] OTHER -> FALSE

PlanReportingQuestionEvents(c,p,Threads,Qids,MaxEpoch) ==
 LET present==Threads \cap DOMAIN c.threads
     ready=={sid \in DOMAIN p.sources:p.sources[sid].parent \in present /\ PRKQuestionReady(c,p,sid)}
     inputs==IF DOMAIN c.questions={} /\ p.inputs={} THEN
       {[act |-> "QuestionInput",args |-> [source |-> sid,child |-> p.sources[sid].parent,qid |-> q]]:sid \in ready,q \in Qids} ELSE {}
     raises==IF DOMAIN c.questions={} THEN
       {PRKQEvent(c,"RaiseQuestion",[child |-> k[1],qid |-> k[2]],Append(c.question_order,k)):k \in p.inputs} ELSE {}
     starts=={PRKQEvent(c,"StartQuestion",[t |-> t],c.question_order):t \in present}
     messages=={m \in c.outbox:m.kind \in {"QuestionUp","AnswerDown","QuestionRelease","QuestionClosed"} /\ m.epoch<=MaxEpoch}
     deliveries=={PRKQEvent(c,"DeliverQuestion",[message |-> m,how |-> "Ack"],c.question_order):m \in messages}
     pending=={k \in DOMAIN c.questions:c.questions[k].pending /\ c.questions[k].epoch<=MaxEpoch}
     agents=={PRKQEvent(c,"AgentAnswer",[agent |-> t,child |-> k[1],qid |-> k[2],epoch |-> c.questions[k].epoch,answer |-> "1"],c.question_order):t \in present,k \in pending}
     persons=={PRKQEvent(c,"PersonAnswer",[child |-> k[1],qid |-> k[2],epoch |-> c.questions[k].epoch,answer |-> "1"],c.question_order):k \in pending \cap p.inputs}
     apply=={[act |-> "QuestionApply",args |-> [event |-> e]]:e \in {v \in raises\cup starts\cup deliveries\cup agents\cup persons:NQRejectName(c,v)="accepted"}}
     answers=={[act |-> "QuestionApplyAnswer",args |-> a]:a \in p.answer_pending}
 IN inputs\cup apply\cup answers
PlanReportingDeliveryEvents(c,p,MaxGeneration) ==
 LET keys==DOMAIN c.natives
     claims=={PRKQEvent(c,"NativeClaim",[parent |-> k[1],key |-> k[2]],c.question_order):k \in keys}
     recovers=={PRKQEvent(c,"NativeRecover",[parent |-> k[1],key |-> k[2]],c.question_order):k \in {v \in keys:c.natives[v].delivery="Claimed" /\ c.natives[v].generation<MaxGeneration}}
     acks=={PRKQEvent(c,"NativeDelivered",[parent |-> k[1],key |-> k[2],generation |-> g],c.question_order):k \in keys,g \in 0..MaxGeneration}
     apply=={[act |-> "NativeDeliveryApply",args |-> [event |-> e]]:e \in {v \in claims\cup recovers\cup acks:NQRejectName(c,v)="accepted"}}
     queues=={[act |-> "NativeEnqueue",args |-> [parent |-> x[1],key |-> x[2],generation |-> x[3],msg |-> "1"]]:
       x \in {v \in p.native_pending:<<v[1],v[2]>> \in keys /\ c.natives[<<v[1],v[2]>>].delivery="Claimed" /\
         c.natives[<<v[1],v[2]>>].generation=v[3] /\ <<v[1],v[2]>> \notin p.native_enqueued}}
 IN apply\cup queues

(* Finite generation uses caller-provided alphabets. No target action is used.
   Native IDs are stable <<parent,key>> pairs, including nested descendants. *)
PlanReportingEvents(c,p,Threads,Keys,SourceIds,Origins,MaxRevision,MaxPending) ==
 LET revisions==1..MaxRevision
     ids=={<<t,k>>:t \in Threads,k \in Keys}
     empty==[x \in {} |-> x]
     bind=={[act |-> "ReportingInstall",args |-> [source |-> sid,parent |-> x[1],run |-> x[2],attempt |-> x[3],access |-> p.access[x],revision |-> r,origin |-> o]]:
        sid \in SourceIds\DOMAIN p.sources,x \in p.intents,r \in revisions,o \in Origins}
     simple=={[act |-> act,args |-> [source |-> sid]]:act \in {"ReportingOpen","ReportingRootStart","ReportingAdmit","ReportingEOF","ReportingInterrupt","ReportingEOFRootTerminal"},sid \in DOMAIN p.sources}
     roots=={[act |-> "ReportingRootEnd",args |-> [source |-> sid,outcome |-> out]]:sid \in DOMAIN p.sources,out \in {"completed","failed","interrupted"}}
     spawns=={[act |-> "ReportingSpawn",args |-> [source |-> sid,key |-> k,ancestor |-> anc,background |-> bg]]:
         sid \in DOMAIN p.sources,k \in ids\DOMAIN p.natives,anc \in {PRKNoNative}\cup DOMAIN p.natives,bg \in BOOLEAN}
     ends=={[act |-> "ReportingNativeEnd",args |-> [key |-> k,outcome |-> out]]:k \in DOMAIN p.natives,out \in {"completed","failed"}}
     configs=={[act |-> act,args |-> [source |-> sid,access |-> acc,revision |-> r,origin |-> o]]:
         act \in {"ReportingIssue","ReportingConfig"},sid \in DOMAIN p.sources,acc \in 0..2,r \in revisions,o \in Origins}
     rebinds=={[act |-> "ReportingRebind",args |-> [source |-> sid,run |-> x[2],attempt |-> x[3],access |-> p.sources[sid].issuedAccess,revision |-> r,origin |-> o]]:
         sid \in DOMAIN p.sources,x \in p.intents,r \in revisions,o \in Origins}
     retires=={[act |-> "ReportingRetire",args |-> [source |-> sid,revision |-> r,remove |-> b]]:sid \in DOMAIN p.sources,r \in revisions,b \in BOOLEAN}
 IN {[act |-> "ReportingCatalog",args |-> [parents |-> Threads]]} \cup bind \cup simple \cup configs \cup rebinds \cup retires \cup IF Len(p.pending)<MaxPending THEN roots\cup spawns\cup ends ELSE {}
====================================================================
