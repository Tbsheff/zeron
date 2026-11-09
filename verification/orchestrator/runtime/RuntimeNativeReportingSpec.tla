---------------- MODULE RuntimeNativeReportingSpec ----------------
(* R44 operational task-result target. This is NOT the old provider-process
   model. An open result stream can report results; EOF closes that channel.
   EOF and task closure leave process state unknown. The old process-death
   properties remain separate proof obligations in RuntimeNativeSpec. *)
EXTENDS Naturals,Sequences,FiniteSets,TLC
CONSTANTS Parents,NativeIds,RuntimeIds,NoRuntime,NoNative,ActivationIds,NoActivation,MaxReportRuns,MaxReportAttempts,MaxSourceRevision,OriginIds,NoOrigin,MaxDeliveryGeneration
VARIABLES runs,sources,natives,tasks,docTasks,pending,lastCause,handles,admitted,catalogComplete
rv == <<runs,sources,natives,tasks,docTasks,pending,lastCause,handles,admitted,catalogComplete>>
handleVars == <<handles,admitted>>
Empty == [x \in {} |-> x]
NoCause == [kind |-> "none",source |-> NoRuntime,changed |-> {}]
Init ==
 /\ runs=[p \in Parents |-> [run |-> 0,attempt |-> 0,active |-> FALSE,endBy |-> "none"]]
 /\ sources=Empty /\ natives=Empty /\ tasks=Empty /\ docTasks=Empty
 /\ pending= <<>> /\ lastCause=NoCause
 /\ catalogComplete=[p \in Parents |-> FALSE]
 /\ handles=[p \in Parents |-> NoRuntime] /\ admitted=[p \in Parents |-> [present |-> FALSE]]
Event(kind,sid,k,r,a,out) == [kind |-> kind,source |-> sid,key |-> k,run |-> r,attempt |-> a,out |-> out]
Current(p,r,a) == runs[p].active /\ runs[p].run=r /\ runs[p].attempt=a
FirstForLane(i) == \A j \in 1..(i-1):
 pending[j].source#pending[i].source \/
 (pending[j].key#pending[i].key /\ pending[i].kind#"sourceLoss")
FirstForSource(i) == FirstForLane(i)
RemoveAt(i) == SubSeq(pending,1,i-1) \o SubSeq(pending,i+1,Len(pending))
TypeOK ==
 /\ DOMAIN handles=Parents /\ DOMAIN admitted=Parents
 /\ catalogComplete \in [Parents -> BOOLEAN]
 /\ DOMAIN runs=Parents /\ DOMAIN sources \subseteq RuntimeIds
 /\ DOMAIN natives \subseteq NativeIds /\ DOMAIN tasks \subseteq DOMAIN natives
 /\ DOMAIN docTasks \subseteq DOMAIN tasks
 /\ \A sid \in DOMAIN sources :
      /\ sources[sid].parent \in Parents
      /\ sources[sid].reporting \in {"unknown","connected","disconnected"}
      /\ sources[sid].process="unknown"
      /\ sources[sid].rootActive \in {"unknown","active","inactive"}
      /\ sources[sid].issuedAccess \in 0..2 /\ sources[sid].requestIssued \in BOOLEAN
      /\ sources[sid].origin \in OriginIds\cup{NoOrigin}
      /\ sources[sid].revision \in 0..MaxSourceRevision
      /\ sources[sid].issuedOrigin \in OriginIds\cup{NoOrigin}
      /\ sources[sid].issuedRun \in 0..MaxReportRuns
      /\ sources[sid].issuedAttempt \in 0..MaxReportAttempts
      /\ sources[sid].issuedRevision \in 0..MaxSourceRevision
      /\ sources[sid].issuedRequestAccess \in 0..2
 /\ \A k \in DOMAIN natives :
      /\ natives[k].source \in DOMAIN sources
      /\ natives[k].state \in {"running","reported","unknown"}
      /\ natives[k].background \in BOOLEAN
 /\ \A k \in DOMAIN tasks : tasks[k].status \in {"running","completed","failed","interrupted","cancelled"}

LogicalStart(p) ==
 /\ ~runs[p].active /\ runs[p].run<MaxReportRuns
 /\ runs'=[runs EXCEPT ![p]=[run |-> @.run+1,attempt |-> 1,active |-> TRUE,endBy |-> "none"]]
 /\ lastCause'=NoCause /\ UNCHANGED <<sources,natives,tasks,docTasks,pending>>
 /\ UNCHANGED handleVars
 /\ UNCHANGED catalogComplete

InstallHandle(p,sid,access,revision,origin) ==
 /\ runs[p].active /\ sid \in RuntimeIds\DOMAIN sources
 /\ access \in 0..2 /\ revision \in Nat
 /\ sources'=(sid :> [parent |-> p,run |-> runs[p].run,attempt |-> runs[p].attempt,
      reporting |-> "unknown",rootActive |-> "unknown",process |-> "unknown",
      issuedAccess |-> access,requestIssued |-> FALSE,revision |-> revision,origin |-> origin,retired |-> FALSE,
      issuedOrigin |-> NoOrigin,issuedRun |-> 0,issuedAttempt |-> 0,issuedRevision |-> 0,issuedRequestAccess |-> 0]) @@ sources
 /\ handles'=[handles EXCEPT ![p]=sid] /\ UNCHANGED admitted
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,natives,tasks,docTasks,pending>>
 /\ UNCHANGED catalogComplete

RebindHandle(sid,access,revision,origin) ==
 /\ sid \in DOMAIN sources /\ access \in 0..2 /\ revision>sources[sid].revision
 /\ LET p==sources[sid].parent IN
      /\ runs[p].active
      /\ sources'=[sources EXCEPT ![sid].run=runs[p].run,![sid].attempt=runs[p].attempt,
                    ![sid].issuedAccess=access,![sid].revision=revision,![sid].origin=origin,![sid].retired=FALSE]
      /\ handles'=[handles EXCEPT ![p]=sid] /\ UNCHANGED admitted
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,natives,tasks,docTasks,pending>>
 /\ UNCHANGED catalogComplete

ConfigHandle(sid,access,revision,origin) ==
 /\ sid \in DOMAIN sources /\ access \in 0..2 /\ revision>sources[sid].revision
 /\ sources'=[sources EXCEPT ![sid].issuedAccess=access,![sid].revision=revision,![sid].origin=origin]
 /\ UNCHANGED admitted
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,natives,tasks,docTasks,pending,handles>>
 /\ UNCHANGED catalogComplete

IssueRequest(sid,access,revision,origin) ==
 /\ sid \in DOMAIN sources /\ access \in 0..2 /\ revision>=sources[sid].revision
 /\ LET p==sources[sid].parent IN
      /\ handles[p]=sid /\ ~sources[sid].retired /\ Current(p,sources[sid].run,sources[sid].attempt)
      /\ sources'=[sources EXCEPT ![sid].requestIssued=TRUE,![sid].issuedAccess=access,![sid].revision=revision,![sid].origin=origin,
                    ![sid].issuedOrigin=origin,![sid].issuedRun=sources[sid].run,![sid].issuedAttempt=sources[sid].attempt,
                    ![sid].issuedRevision=revision,![sid].issuedRequestAccess=access]
      /\ UNCHANGED admitted
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,natives,tasks,docTasks,pending,handles>>
 /\ UNCHANGED catalogComplete

RetireHandle(sid,revision,remove) ==
 /\ sid \in DOMAIN sources /\ revision>sources[sid].revision
 /\ LET p==sources[sid].parent IN
      /\ sources'=[sources EXCEPT ![sid].retired=TRUE,![sid].revision=revision]
      /\ handles'=IF remove /\ handles[p]=sid THEN [handles EXCEPT ![p]=NoRuntime] ELSE handles
      /\ UNCHANGED admitted
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,natives,tasks,docTasks,pending>>
 /\ UNCHANGED catalogComplete

AdmissionCurrent(p) ==
 /\ admitted[p].present /\ handles[p]#NoRuntime /\ handles[p] \in DOMAIN sources
 /\ LET h==sources[handles[p]] a==admitted[p] IN
      /\ ~h.retired /\ h.requestIssued /\ a.source=handles[p] /\ a.run=h.run /\ a.attempt=h.attempt
      /\ a.revision=h.revision /\ a.issuedAccess=h.issuedAccess /\ Current(p,h.run,h.attempt)
Admit(sid) ==
 /\ sid \in DOMAIN sources
 /\ LET h==sources[sid] p==h.parent IN
      /\ handles[p]=sid /\ ~h.retired /\ h.requestIssued /\ Current(p,h.run,h.attempt)
      /\ admitted'=[admitted EXCEPT ![p]=[present |-> TRUE,source |-> sid,
                      run |-> h.run,attempt |-> h.attempt,revision |-> h.revision,issuedAccess |-> h.issuedAccess]]
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,sources,natives,tasks,docTasks,pending,handles>>
 /\ UNCHANGED catalogComplete

Refuse(p) ==
 /\ admitted'=[admitted EXCEPT ![p]=[present |-> FALSE]]
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,sources,natives,tasks,docTasks,pending,handles>>
 /\ UNCHANGED catalogComplete

LogicalAttempt(p) ==
 /\ runs[p].active /\ runs[p].attempt<MaxReportAttempts
 /\ runs'=[runs EXCEPT ![p].attempt=@+1]
 /\ LET fg=={k \in DOMAIN tasks:tasks[k].parent=p /\ tasks[k].run=runs[p].run /\
                 tasks[k].attempt=runs[p].attempt /\ ~tasks[k].background /\ tasks[k].status="running"} IN
      /\ tasks'=[k \in DOMAIN tasks |-> IF k \in fg THEN [tasks[k] EXCEPT !.status="interrupted",!.cause="orphan"] ELSE tasks[k]]
 /\ UNCHANGED admitted
 /\ lastCause'=NoCause /\ UNCHANGED <<sources,natives,docTasks,pending,handles>>
 /\ UNCHANGED catalogComplete

ObserveStreamOpen(sid) ==
 /\ sid \in DOMAIN sources /\ sources[sid].reporting="unknown"
 /\ sources'=[sources EXCEPT ![sid].reporting="connected"]
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,natives,tasks,docTasks,pending>>
 /\ UNCHANGED handleVars
 /\ UNCHANGED catalogComplete

ObserveRootStart(sid) ==
 /\ sid \in DOMAIN sources /\ sources[sid].reporting="connected"
 /\ sources'=[sources EXCEPT ![sid].rootActive="active"]
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,natives,tasks,docTasks,pending>>
 /\ UNCHANGED handleVars
 /\ UNCHANGED catalogComplete

ObserveSpawn(sid,k,ancestor,ownBackground) ==
 /\ sid \in DOMAIN sources /\ sources[sid].reporting="connected"
 /\ k \in NativeIds\DOMAIN natives
 /\ ancestor=NoNative \/ (ancestor \in DOMAIN natives /\ natives[ancestor].source=sid /\ natives[ancestor].state="running")
 /\ sources[sid].rootActive="active" \/ (ancestor#NoNative /\ natives[ancestor].background)
 /\ LET bg == ownBackground \/ (ancestor#NoNative /\ natives[ancestor].background)
        row == [source |-> sid,parent |-> sources[sid].parent,run |-> IF ancestor=NoNative THEN sources[sid].run ELSE natives[ancestor].run,
                attempt |-> IF ancestor=NoNative THEN sources[sid].attempt ELSE natives[ancestor].attempt,
                activation |-> NoActivation,ancestor |-> ancestor,ownBackground |-> ownBackground,
                background |-> bg,state |-> "running"] IN
       /\ natives'=(k :> row) @@ natives
       /\ pending'=Append(pending,Event("spawn",sid,k,row.run,row.attempt,"none"))
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,sources,tasks,docTasks>>
 /\ UNCHANGED handleVars
 /\ UNCHANGED catalogComplete

ObserveNativeEnd(k,out) ==
 /\ k \in DOMAIN natives /\ natives[k].state \in {"running","reported"}
 /\ natives[k].source \in DOMAIN sources /\ sources[natives[k].source].reporting="connected"
 /\ out \in {"completed","failed","interrupted","cancelled"}
 /\ natives'=[natives EXCEPT ![k].state="reported"]
 /\ pending'=Append(pending,Event("nativeEnd",natives[k].source,k,natives[k].run,natives[k].attempt,out))
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,sources,tasks,docTasks>>
 /\ UNCHANGED handleVars
 /\ UNCHANGED catalogComplete

ObserveRootEnd(sid,out) ==
 /\ sid \in DOMAIN sources /\ sources[sid].reporting="connected" /\ sources[sid].rootActive="active"
 /\ out \in {"completed","failed","interrupted"}
 /\ sources'=[sources EXCEPT ![sid].rootActive="inactive"]
 /\ pending'=Append(pending,Event("rootEnd",sid,NoNative,sources[sid].run,sources[sid].attempt,out))
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,natives,tasks,docTasks>>
 /\ UNCHANGED handleVars
 /\ UNCHANGED catalogComplete

EngineEOFRootTerminal(sid,out) ==
 /\ sid \in DOMAIN sources /\ sources[sid].reporting="disconnected"
 /\ out \in {"failed","interrupted"}
 /\ Current(sources[sid].parent,sources[sid].run,sources[sid].attempt)
 /\ ~\E i \in 1..Len(pending):pending[i].kind="rootEnd" /\ pending[i].source=sid /\
      pending[i].run=sources[sid].run /\ pending[i].attempt=sources[sid].attempt
 /\ pending'=Append(pending,Event("rootEnd",sid,NoNative,sources[sid].run,sources[sid].attempt,out))
 /\ lastCause'=NoCause
 /\ UNCHANGED <<runs,sources,natives,tasks,docTasks,handles,admitted,catalogComplete>>
ObserveStreamClose(sid) ==
 /\ sid \in DOMAIN sources /\ sources[sid].reporting="connected"
 /\ sources'=[sources EXCEPT ![sid].reporting="disconnected",![sid].rootActive="unknown"]
 /\ natives'=[k \in DOMAIN natives |-> IF natives[k].source=sid /\ natives[k].state="running"
                                            THEN [natives[k] EXCEPT !.state="unknown"] ELSE natives[k]]
 /\ pending'=Append(pending,Event("sourceLoss",sid,NoNative,sources[sid].run,sources[sid].attempt,"failed"))
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,tasks,docTasks>>
 /\ UNCHANGED handleVars
 /\ UNCHANGED catalogComplete

SpawnAdmissible(e) ==
 LET n==natives[e.key] p==n.parent IN
 IF n.ancestor=NoNative THEN Current(p,e.run,e.attempt)
 ELSE n.ancestor \in DOMAIN tasks /\ tasks[n.ancestor].status="running" /\
      (tasks[n.ancestor].background \/ Current(p,e.run,e.attempt))
RejectSpawn(i) ==
 /\ i \in 1..Len(pending) /\ FirstForSource(i) /\ pending[i].kind="spawn"
 /\ ~SpawnAdmissible(pending[i])
 /\ pending'=RemoveAt(i) /\ lastCause'=NoCause
 /\ UNCHANGED <<runs,sources,natives,tasks,docTasks,handles,admitted>>
 /\ UNCHANGED catalogComplete

RecordDeliveryGeneration(k,generation) ==
 /\ k \in DOMAIN tasks /\ tasks[k].status#"running"
 /\ generation=tasks[k].generation+1 /\ generation<=MaxDeliveryGeneration
 /\ tasks'=[tasks EXCEPT ![k].generation=generation] /\ lastCause'=NoCause
 /\ UNCHANGED <<runs,sources,natives,docTasks,pending,handles,admitted>>
 /\ UNCHANGED catalogComplete

(* The engine closes foreground descendants in the same accepted native End.
   Provider observation alone does not change their engine task state. *)
RECURSIVE DescendsWithin(_,_,_)
DescendsWithin(k,a,n) ==
 IF n=0 \/ k \notin DOMAIN natives THEN FALSE
 ELSE LET parent==natives[k].ancestor IN
      IF parent=NoNative THEN FALSE
      ELSE parent=a \/ DescendsWithin(parent,a,n-1)
ForegroundDescendants(k,sid) == {d \in DOMAIN tasks:
 tasks[d].source=sid /\ tasks[d].status="running" /\ ~tasks[d].background /\
 DescendsWithin(d,k,Cardinality(DOMAIN natives))}
Handle(i) ==
 /\ i \in 1..Len(pending) /\ FirstForSource(i)
 /\ pending'=RemoveAt(i)
 /\ LET e==pending[i] p==sources[e.source].parent
        owned == {k \in DOMAIN tasks:tasks[k].source=e.source /\ tasks[k].status="running"}
        fg == {k \in owned:~tasks[k].background /\ tasks[k].run=e.run /\ tasks[k].attempt=e.attempt}
        close == IF e.kind="sourceLoss" THEN {} ELSE
                 IF e.kind="rootEnd" /\ Current(p,e.run,e.attempt) THEN fg ELSE
                 IF e.kind="nativeEnd" /\ e.key \in DOMAIN tasks /\ tasks[e.key].status="running"
                    THEN ForegroundDescendants(e.key,e.source) ELSE {}
    IN /\ CASE e.kind="spawn" ->
             /\ e.key \notin DOMAIN tasks
             /\ SpawnAdmissible(e)
             /\ tasks'=(e.key :> [source |-> e.source,parent |-> p,run |-> e.run,attempt |-> e.attempt,
                    status |-> "running",background |-> natives[e.key].background,
                    activation |-> natives[e.key].activation,generation |-> 0,cause |-> "none"]) @@ tasks
             /\ UNCHANGED runs
          [] e.kind="nativeEnd" ->
             /\ tasks'=IF e.key \in DOMAIN tasks /\ tasks[e.key].source=e.source /\ tasks[e.key].status="running"
                 THEN [k \in DOMAIN tasks |->
                    IF k=e.key THEN [tasks[k] EXCEPT !.status=IF e.out="completed" THEN "completed" ELSE "failed",!.cause="reported"]
                    ELSE IF k \in close THEN [tasks[k] EXCEPT !.status="interrupted",!.cause="orphan"] ELSE tasks[k]] ELSE tasks
             /\ UNCHANGED runs
          [] e.kind \in {"rootEnd","sourceLoss"} ->
             /\ (e.kind="sourceLoss" => owned={})
             /\ tasks'=[k \in DOMAIN tasks |-> IF k \in close THEN [tasks[k] EXCEPT
                    !.status=IF e.kind="sourceLoss" THEN "failed" ELSE "interrupted",
                    !.cause=IF e.kind="sourceLoss" THEN "reporting_loss" ELSE "orphan"] ELSE tasks[k]]
             /\ runs'=IF e.kind="rootEnd" /\ Current(p,e.run,e.attempt)
                       THEN [runs EXCEPT ![p].active=FALSE,![p].endBy="root"] ELSE runs
       /\ lastCause'=[kind |-> e.kind,source |-> e.source,changed |-> close]
 /\ UNCHANGED <<sources,natives,docTasks,handles,admitted>>
 /\ UNCHANGED catalogComplete
HandleLossNative(i,k) ==
 /\ i \in 1..Len(pending) /\ FirstForSource(i) /\ pending[i].kind="sourceLoss"
 /\ k \in DOMAIN tasks /\ tasks[k].source=pending[i].source /\ tasks[k].status="running"
 /\ tasks'=[tasks EXCEPT ![k].status="failed",![k].cause="reporting_loss"]
 /\ lastCause'=[kind |-> "sourceLoss",source |-> pending[i].source,changed |-> {k}]
 /\ UNCHANGED <<runs,sources,natives,docTasks,pending,handles,admitted>>
 /\ UNCHANGED catalogComplete

BindActivation(k,activation) ==
 /\ k \in DOMAIN natives /\ activation \in ActivationIds /\ natives[k].activation=NoActivation
 /\ natives'=[natives EXCEPT ![k].activation=activation]
 /\ tasks'=IF k \in DOMAIN tasks THEN [tasks EXCEPT ![k].activation=activation] ELSE tasks
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,sources,docTasks,pending,handles,admitted>>
 /\ UNCHANGED catalogComplete

DocStatus(st) == IF st="running" THEN "running" ELSE IF st="completed" THEN "done" ELSE "failed"
PersistTask(k,status) ==
 /\ k \in DOMAIN tasks /\ status=DocStatus(tasks[k].status)
 /\ docTasks'=(k :> [source |-> tasks[k].source,status |-> status]) @@ docTasks
 /\ lastCause'=NoCause /\ UNCHANGED <<runs,sources,natives,tasks,pending>>
 /\ UNCHANGED handleVars
 /\ UNCHANGED catalogComplete

ObserveCatalog(ps) ==
 /\ ps \subseteq Parents
 /\ catalogComplete'=[p \in Parents |-> catalogComplete[p] \/ p \in ps]
 /\ lastCause'=NoCause
 /\ UNCHANGED <<runs,sources,natives,tasks,docTasks,pending,handles,admitted>>
Next ==
 \/ \E ps \in SUBSET Parents:ObserveCatalog(ps)
 \/ \E p \in Parents:LogicalStart(p) \/ LogicalAttempt(p) \/ Refuse(p)
 \/ \E p \in Parents,sid \in RuntimeIds:\E access \in 0..2,revision \in 0..MaxSourceRevision,origin \in OriginIds\cup{NoOrigin}:InstallHandle(p,sid,access,revision,origin)
 \/ \E sid \in RuntimeIds:ObserveStreamOpen(sid) \/ ObserveRootStart(sid) \/ (\E out \in {"completed","failed","interrupted"}:ObserveRootEnd(sid,out)) \/ ObserveStreamClose(sid)
 \/ \E sid \in RuntimeIds,k \in NativeIds,a \in NativeIds\cup{NoNative},bg \in BOOLEAN:ObserveSpawn(sid,k,a,bg)
 \/ \E k \in NativeIds,out \in {"completed","failed","interrupted","cancelled"}:ObserveNativeEnd(k,out)
 \/ \E k \in NativeIds,st \in {"running","done","failed"}:PersistTask(k,st)
 \/ \E i \in 1..Len(pending):Handle(i) \/ RejectSpawn(i) \/ (\E k \in NativeIds:HandleLossNative(i,k))
 \/ \E k \in NativeIds,g \in 0..MaxDeliveryGeneration:RecordDeliveryGeneration(k,g)
 \/ \E k \in NativeIds,a \in ActivationIds:BindActivation(k,a)
 \/ \E sid \in RuntimeIds,out \in {"failed","interrupted"}:EngineEOFRootTerminal(sid,out)
 \/ \E sid \in RuntimeIds:Admit(sid)
 \/ \E sid \in RuntimeIds,a \in 0..2,r \in 0..MaxSourceRevision,origin \in OriginIds\cup{NoOrigin}: RebindHandle(sid,a,r,origin) \/ ConfigHandle(sid,a,r,origin) \/ IssueRequest(sid,a,r,origin) \/ (\E remove \in BOOLEAN:RetireHandle(sid,r,remove))
Spec == Init /\ [][Next]_rv
OnlyRootEndsRun == \A p \in Parents:runs[p].endBy \in {"none","root"}
NoFalseProcessDeath == \A sid \in DOMAIN sources:sources[sid].process="unknown"
LossClosesOnlySource == lastCause.kind#"sourceLoss" \/
 \A k \in lastCause.changed:tasks[k].source=lastCause.source /\ tasks[k].cause="reporting_loss"
OrphansClosed == \A k \in DOMAIN tasks: tasks[k].status="running" /\ ~tasks[k].background =>
   Current(tasks[k].parent,tasks[k].run,tasks[k].attempt)
AncestorTerminalClosesForeground == \A k \in DOMAIN tasks:
 tasks[k].status="running" /\ ~tasks[k].background =>
 \A a \in DOMAIN tasks:DescendsWithin(k,a,Cardinality(DOMAIN natives)) => tasks[a].status="running"
BackgroundSurvivesTurn == lastCause.kind#"rootEnd" \/
 \A k \in lastCause.changed:~tasks[k].background
ChangedTasks == {k \in DOMAIN tasks \cap DOMAIN tasks':tasks[k]#tasks'[k]}
OnlyRootEndsRunStep == \A p \in Parents:
 (runs[p].active /\ ~runs'[p].active) =>
 (lastCause'.kind="rootEnd" /\ \E i \in 1..Len(pending):
  pending[i].kind="rootEnd" /\ sources[pending[i].source].parent=p /\
  pending[i].run=runs[p].run /\ pending[i].attempt=runs[p].attempt /\ pending'=RemoveAt(i))
LossClosesOnlySourceStep == lastCause'.kind#"sourceLoss" \/
 /\ DOMAIN tasks'=DOMAIN tasks
 /\ lastCause'.changed=ChangedTasks
 /\ \A k \in ChangedTasks:tasks[k].source=lastCause'.source /\ tasks[k].status="running" /\
       tasks'[k].status="failed" /\ tasks'[k].cause="reporting_loss"
BackgroundSurvivesTurnStep == lastCause'.kind#"rootEnd" \/
 /\ DOMAIN tasks'=DOMAIN tasks
 /\ \A k \in DOMAIN tasks:tasks[k].background => tasks'[k]=tasks[k]
SafetyStep == OnlyRootEndsRunStep /\ LossClosesOnlySourceStep /\ BackgroundSurvivesTurnStep
TransitionSafety == [][SafetyStep]_rv
SourceWorker(sid) == \E i \in 1..Len(pending):pending[i].source=sid /\
 (Handle(i) \/ RejectSpawn(i) \/ (\E k \in NativeIds:HandleLossNative(i,k)))
FairSpec == Spec /\ \A sid \in RuntimeIds:WF_rv(SourceWorker(sid))
FairClosure == \A sid \in RuntimeIds:
 (sid \in DOMAIN sources /\ sources[sid].reporting="disconnected") ~>
 (\A k \in DOMAIN tasks:tasks[k].source=sid => tasks[k].status#"running")
Safety == TypeOK /\ AncestorTerminalClosesForeground /\ OnlyRootEndsRun /\ NoFalseProcessDeath /\ LossClosesOnlySource /\ OrphansClosed /\ BackgroundSurvivesTurn
====
