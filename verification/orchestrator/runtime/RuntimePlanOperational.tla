---------------- MODULE RuntimePlanOperational ----------------
(* Free operational model. Core commits, CT packet work, source stream events,
   admission facts and document writes are independent operational steps.
   No target relation filters a successor. Old process-state NS stays fixed;
   RuntimeNativeReportingSpec supplies the operational stream/task target. *)
EXTENDS RuntimePlanJoint, RuntimePlanReportingKernel
CONSTANTS PlanSourceIds, PlanOrigins, PlanMaxRevision, PlanMaxPending
VARIABLES rp,terminalFacts
opVars == <<jointVars,rp,terminalFacts>>
OCT == INSTANCE RuntimePlanOperationalTransferKernel WITH
 Threads<-Threads,Main<-Root,NoThread<-"none",Providers<-Providers,NoProv<-NoProv,
 MaxMsg<-PlanMaxMsg,MaxX<-PlanMaxX,MaxCrash<-PlanMaxCrash,Limits<-PlanLimits,
 Mutant<-PlanMutant,ct<-ct
OpInit ==
 /\ core=JointCoreInit /\ physical=PlanNativeInit(Threads,Subs) /\ OCT!Init
 /\ ctTracked=[t \in Threads|->FALSE] /\ ghost=PlanGhostInit
 /\ effects= <<>> /\ personLeft=PlanMaxPerson /\ serverLeft=PlanMaxServer
 /\ stopLeft=PlanMaxStop /\ narrowLeft=PlanMaxNarrow /\ rp=PlanReportingInit(Threads) /\ terminalFacts={}
OpCTReadyRequest(t) == ~ctTracked[t] \/ OCT!ReadyRequest(t)
OpTerminalFact(t,out) == <<t,ct.bindings[t].run,ct.bindings[t].attempt,out>> \in terminalFacts
OpCurrent(t) == IF t \notin DOMAIN core.threads THEN FALSE
 ELSE IF rp.handles[t] \notin DOMAIN rp.sources THEN FALSE
 ELSE LET s==rp.sources[rp.handles[t]] IN ~s.retired /\ IsLive(core,t) /\
 s.run=RunOrd(core,t) /\ s.attempt=core.threads[t].attempt.n
OpIssued(t) == IF ~OpCurrent(t) THEN FALSE ELSE rp.sources[rp.handles[t]].requestIssued
OpAdmission(t) == IF ~OpIssued(t) THEN FALSE ELSE IF ~rp.catalogComplete[t] \/ ~rp.admitted[t].present THEN FALSE
 ELSE LET s==rp.sources[rp.handles[t]] a==rp.admitted[t] IN
 a.source=rp.handles[t] /\ a.run=s.run /\ a.attempt=s.attempt /\
 a.revision=s.revision /\ a.issuedAccess=s.issuedAccess /\ s.issuedAccess<=core.threads[t].access
OpCTIssued(t) == IF ~OpIssued(t) THEN FALSE ELSE
 LET s==rp.sources[rp.handles[t]] IN s.run=ct.bindings[t].run /\ s.attempt=ct.bindings[t].attempt
OpRD(ev) ==
 /\ UNCHANGED terminalFacts
 /\ RDGuard(core,ev)

 /\ (ev.act="Delegate" => PlanMutant="call_before_issue" \/ OpAdmission(ev.args.t))
 /\ LET r == RDRawResult(core,ev) next == Finalize(r.state)
        fx == r.fx \o ReconcileEffects(r.state)
    IN /\ core'=next /\ effects'=fx
       /\ PlanReportingCoreBoundary(core,next,rp,rp',ev,fx) /\ UNCHANGED physical
 /\ personLeft'=IF ev.act="PersonSend" THEN personLeft-1 ELSE personLeft
 /\ serverLeft'=IF ev.act="ServerEnqueue" THEN serverLeft-1 ELSE serverLeft
 /\ stopLeft'=IF ev.act="Stop" THEN stopLeft-1 ELSE stopLeft
 /\ narrowLeft'=IF ev.act="NarrowAccess" THEN narrowLeft-1 ELSE narrowLeft
 /\ IF ev.act \in {"StartFromQueue","StartFromQueueWake","UsageLimitRecovery"}
    THEN LET t == ev.args.t nextCore == Finalize(RDRawResult(core,ev).state)
             ord == RunOrd(nextCore,t) attempt == nextCore.threads[t].attempt.n
         IN IF ENABLED OCT!LogicalPending(t,ord,attempt)
            THEN /\ OCT!LogicalPending(t,ord,attempt)
                 /\ ctTracked'=[ctTracked EXCEPT ![t]=TRUE]
            ELSE /\ UNCHANGED ct /\ ctTracked'=[ctTracked EXCEPT ![t]=FALSE]
    ELSE IF ev.act="Delegate"
    THEN /\ IF ENABLED OCT!CreateLineage(ev.args.t,ev.args.child,"subagent_spawn")
            THEN OCT!CreateLineage(ev.args.t,ev.args.child,"subagent_spawn") ELSE UNCHANGED ct
         /\ UNCHANGED ctTracked
    ELSE IF ev.act="IngestEnd" /\ ctTracked[ev.args.t]
    THEN /\ UNCHANGED ct
         /\ ctTracked'=[ctTracked EXCEPT ![ev.args.t]=FALSE]
    ELSE UNCHANGED <<ct,ctTracked>>
 /\ ghost'=[PlanGhostCT(PlanGhostNext(core,core',ev,effects',ghost),ct,ct',core') EXCEPT
      !.ct_tracked={t \in Threads:ctTracked'[t]}]
OpAutoAbandon(t) ==
 /\ core.threads[t].wait_on#"none"
 /\ \E e \in core.threads[t].inbox:
      e.node="Root" /\ RDCurrent(core,t,e) /\ e.outcome="Completed"
 /\ OpRD(Ev("WaitAbandon",[t |-> t]))
OpCoreNext ==
 \/ \E t \in DOMAIN core.threads :
    \/ personLeft>0 /\ OpRD(Ev("PersonSend",[t |-> t,msg |-> "1",mode |-> "Queue"]))
    \/ serverLeft>0 /\ OpRD(Ev("ServerEnqueue",[t |-> t,msg |-> "2"]))
    \/ CoreRoom(t) /\ OpRD(Ev("StartFromQueue",[t |-> t]))
    \/ CoreRoom(t) /\ OpRD(Ev("StartFromQueueWake",[t |-> t]))
    \/ OpRD(Ev("WaitTimeout",[t |-> t]))
    \/ OpRD(Ev("WaitReturn",[t |-> t]))
    \/ OpAutoAbandon(t)
    \/ stopLeft>0 /\ OpRD(Ev("Stop",[t |-> t]))
    \/ narrowLeft>0 /\ AttemptRoom(t) /\ \E a \in 0..1:
          OpRD(Ev("NarrowAccess",[t |-> t,access |-> a]))
    \/ \E e \in core.threads[t].inbox :
       \/ OpRD(Ev("IngestEnd",[t |-> t,ev |-> e]))
       \/ AttemptRoom(t) /\ OpRD(Ev("RetryAttempt",[t |-> t,ev |-> e]))
       \/ OpRD(Ev("IngestChild",[t |-> t,ev |-> e]))
       \/ OpRD(Ev("IngestStale",[t |-> t,ev |-> e]))
    \/ \E c \in Threads \ DOMAIN core.threads,m \in {"Wait","Async"},a \in 0..2:
       OpRD(Ev("Delegate",[t |-> t,child |-> c,mode |-> m,access |-> a,reconfigure |-> FALSE]))
 \/ \E m \in core.outbox :
    CASE m.kind="Notice" -> OpRD(Ev("DoNotice",[child |-> m.child,run |-> m.run,how |-> "Ack"]))
    [] m.kind="Offer" -> OpRD(Ev("DoOffer",[parent |-> m.parent,run |-> m.run,generation |-> m.generation,how |-> "Ack"]))
    [] m.kind="Stop" -> OpRD(Ev("DoStop",[child |-> m.child,stop |-> m.stop,how |-> "Ack"]))
    [] OTHER -> FALSE


OpSource(ev) ==
 /\ (ev.act="ReportingIssue" => OpCTReadyRequest(rp.sources[ev.args.source].parent))
 /\ PlanReportingSourceStep(core,rp,rp',ev)
 /\ terminalFacts'=IF ev.act \in {"ReportingRootEnd","ReportingEOFRootTerminal"}
    THEN LET e==rp'.pending[Len(rp'.pending)] IN
      terminalFacts \cup {<<rp'.sources[e.source].parent,e.run,e.attempt,e.out>>}
    ELSE terminalFacts
 /\ UNCHANGED <<core,physical,ct,effects,personLeft,serverLeft,stopLeft,narrowLeft,ctTracked>>
 /\ ghost'=IF ev.act \in {"ReportingInstall","ReportingConfig","ReportingIssue","ReportingRebind"}
    THEN LET sid==ev.args.source s==rp'.sources[sid] IN
      PlanGhostProviderAccess(ghost,s.parent,s.run,s.attempt,s.issuedAccess)
    ELSE ghost
OpNative(ev) ==
 /\ UNCHANGED terminalFacts
 /\ NativeQuestionApply(core,core',ev,effects')
 /\ PlanReportingCoreBoundary(core,core',rp,rp',ev,effects')
 /\ UNCHANGED <<physical,personLeft,serverLeft,stopLeft,narrowLeft>>
 /\ IF ev.act="StartQuestion" THEN
      LET t==ev.args.t ord==RunOrd(core',t) at==core'.threads[t].attempt.n IN
      IF ENABLED OCT!LogicalPending(t,ord,at)
      THEN /\ OCT!LogicalPending(t,ord,at)
           /\ ctTracked'=[ctTracked EXCEPT ![t]=TRUE]
      ELSE /\ UNCHANGED ct /\ ctTracked'=[ctTracked EXCEPT ![t]=FALSE]
    ELSE UNCHANGED <<ct,ctTracked>>
 /\ ghost'=[PlanGhostCT(PlanGhostNext(core,core',ev,effects',ghost),ct,ct',core') EXCEPT
      !.ct_tracked={t \in Threads:ctTracked'[t]}]
OpBridge(ev) ==
 /\ UNCHANGED terminalFacts
 /\ (IF ev.act="QuestionApply" THEN ev.args.event.act="AgentAnswer" => OpAdmission(ev.args.event.args.agent) ELSE TRUE)
 /\ PlanReportingBridgeStep(core,rp,core',rp',ev,effects')
 /\ UNCHANGED <<physical,personLeft,serverLeft,stopLeft,narrowLeft>>
 /\ IF ev.act="QuestionApply" /\ ev.args.event.act="StartQuestion" THEN
      LET t==ev.args.event.args.t ord==RunOrd(core',t) at==core'.threads[t].attempt.n IN
      IF ENABLED OCT!LogicalPending(t,ord,at)
      THEN /\ OCT!LogicalPending(t,ord,at)
           /\ ctTracked'=[ctTracked EXCEPT ![t]=TRUE]
      ELSE /\ UNCHANGED ct /\ ctTracked'=[ctTracked EXCEPT ![t]=FALSE]
    ELSE UNCHANGED <<ct,ctTracked>>
 /\ ghost'=[PlanGhostCT(PlanGhostNext(core,core',ev,effects',ghost),ct,ct',core') EXCEPT
      !.ct_tracked={t \in Threads:ctTracked'[t]}]
OpRootObserve(ev) ==
 /\ ev.act="ProviderTerminal"
 /\ OpRD(ev)
OpReportCoreNext == \E ev \in PlanReportingCoreEvents(core,rp):
 IF ev.act="ProviderTerminal" THEN OpRootObserve(ev) ELSE OpNative(ev)
OpPersist(k) ==
 /\ PlanReportingPersist(core,rp,rp',k)
 /\ UNCHANGED <<jointVars,terminalFacts>>
OpDiscard(i) ==
 /\ (PlanReportingLossConsumed(core,rp,rp',i) \/ PlanReportingRejectNative(core,rp,rp',i) \/ PlanReportingIgnoreRoot(core,rp,rp',i))
 /\ UNCHANGED <<jointVars,terminalFacts>>
OpCTStep(A) ==
 /\ A /\ UNCHANGED <<core,physical,effects,personLeft,serverLeft,stopLeft,narrowLeft,ctTracked,rp,terminalFacts>>
 /\ ghost'=[PlanGhostCT(ghost,ct,ct',core) EXCEPT !.ct_tracked={t \in Threads:ctTracked[t]}]
OpCTRelease(t) == (IF t \notin DOMAIN core.threads THEN TRUE ELSE ~IsLive(core,t) \/ RunOrd(core,t)#ct.bindings[t].run \/ core.threads[t].attempt.n#ct.bindings[t].attempt) /\ OCT!IngestEnd(t)
OpCTNext == OpCTStep(
 \/ \E t \in Threads:
      OpCTRelease(t) \/
      OCT!ObserveUser(t) \/ OCT!PinSwitch(t) \/ OCT!CommitPrepared(t) \/ OCT!AbortObserved(t) \/
      (OpTerminalFact(t,"completed") /\ OCT!ConfirmAccepted(t,ct.runs[t].p,ct.packets[t].ids \cup {ct.runs[t].u},ct.packets[t].om)) \/
      (OpTerminalFact(t,"completed") /\ OCT!CompleteObserved(t)) \/
      ((OpTerminalFact(t,"failed") \/ OpTerminalFact(t,"interrupted")) /\ OCT!FailObserved(t))
 \/ \E t \in Threads,p \in Providers,u \in 1..PlanMaxMsg:OCT!PrepareObserved(t,p,u)
 \/ \E t \in Threads,x \in 1..PlanMaxX,l \in PlanLimits:OCT!ResolveObserved(t,x,l))
OpSourceNext == \E ev \in PlanReportingEvents(core,rp,Threads,Subs,PlanSourceIds,PlanOrigins,PlanMaxRevision,PlanMaxPending):OpSource(ev)
OpBridgeNext == \E ev \in PlanReportingQuestionEvents(core,rp,Threads\{Root},{1},PlanMaxEpoch) \cup PlanReportingDeliveryEvents(core,rp,PlanMaxEpoch):
 /\ (IF ev.act="QuestionApply" THEN ev.args.event.act="StartQuestion" => CoreRoom(ev.args.event.args.t) ELSE TRUE)
 /\ OpBridge(ev)
OpNext == OpBridgeNext \/ OpCoreNext \/ OpSourceNext \/ OpReportCoreNext \/ OpCTNext \/
 (\E k \in DOMAIN core.natives:OpPersist(k)) \/ (\E i \in 1..Len(rp.pending):OpDiscard(i))
OpSpec == OpInit /\ [][OpNext]_opVars
OpImage(focus) == PlanImage(core,PlanNativeImage(JointCoreInit,PlanNativeInit(Threads,Subs),focus),OCT!CTImage,ghost,focus)
OpReportImage == PlanReportingImage(core,rp)
OpIssuedBeforeDelegate == [][\A c \in DOMAIN core'.threads\DOMAIN core.threads:OpAdmission(core'.threads[c].parent)]_opVars
OpNativeWitness == DOMAIN core.natives={}
OpEarlyCallWitness == ~\E c \in DOMAIN core.threads\{Root}:
 LET t==core.threads[c].parent sid==rp.handles[t] IN OpAdmission(t) /\ rp.sources[sid].reporting="unknown"
OpTransferWitness == ~\E x \in 1..PlanMaxX:ct.ledger[x].st="consumed"
OpQuestionWitness == DOMAIN core.questions={}
OpBackgroundWitness == ~\E k \in DOMAIN core.natives:core.natives[k].background /\ core.natives[k].status="Running" /\ ~IsLive(core,k[1])
OpDelayedDocWitness == ~\E k \in DOMAIN core.natives:
 core.natives[k].status#"Running" /\ (k \notin DOMAIN rp.docs \/ rp.docs[k].status="running")
=============================================================================
