---------------- MODULE RuntimeOperationalCompositionSpec ----------------
(* R44/R48 operational replacement, separate from unchanged process-native and
   single-current-request CT theorems. CoreImage retains the original 105-field
   forest, with native/CT fields fixed at their declared Init values. Reporting
   and retained request maps are mandatory state products, not metadata flags.
   EOF proves loss of reporting only. A saved result can confirm an old request
   without modifying the current request. Implementations must enumerate their
   own successors and check this target as a property. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS Roots, TreeThreads, RootDevices, NoThread, MaxRuns, MaxQueue, Mut, RLMaxPerson, RLMaxServer, RLMaxRetry, RLMaxStop, RLMaxUL, RLMaxCrash, RLMaxAtt, FixLimitRelease, MaxDepth, MaxFanout, MaxGen, DMaxPerson, MaxFail, DMaxCrash, MaxAbandon, DMaxStop, MaxNarrow, MaxInterrupt, DupDelivery, Crash, LateAck, Questions, FixWakeSkipsFailHold, StopCheckAnc, PartialCrash, QSplit, MaxFollowUp, UseNS, Subs, NoSub, MaxEp, NSMaxCrash, MaxDup, NSMaxInterrupt, MaxSessEnd, MaxInbox, Background, SilentKill, UseCT, Providers, NoProv, CanHandoff, CanInject, NativeFork, MainProviders, MaxMsg, MaxX, CTMaxCrash, MaxStale, Forks, Spawns, Cuts, MaxCutK, BudgetFail, InPlace, ToolNodes, UsePL, Devices, MaxOffline, Parents, NativeIds, RuntimeIds, NoRuntime, NoNative, ActivationIds, NoActivation, MaxReportRuns, MaxReportAttempts, MaxSourceRevision, OriginIds, NoOrigin, MaxDeliveryGeneration, Chats, Harnesses, Inputs, Requests, Pins, PacketIds, SessionIds, ReceiptIds, DocumentIds, MessageIds, NoMessage, NoRequest, NoPin, NoPacket, MaxCTRun, MaxCTAttempt, UseReporting, UseTransferReporting
VARIABLES CoreImage, ReportingImage, TransferImage
F == INSTANCE RuntimeForestCompositionSpec WITH Image <- CoreImage
RP == INSTANCE RuntimeNativeReportingSpec WITH
  runs <- ReportingImage.runs,
  sources <- ReportingImage.sources,
  natives <- ReportingImage.natives,
  tasks <- ReportingImage.tasks,
  docTasks <- ReportingImage.docTasks,
  pending <- ReportingImage.pending,
  lastCause <- ReportingImage.lastCause,
  handles <- ReportingImage.handles,
  admitted <- ReportingImage.admitted,
  catalogComplete <- ReportingImage.catalogComplete
TR == INSTANCE RuntimeTransferReportingSpec WITH
  logicalCreated <- TransferImage.logicalCreated,
  loaded <- TransferImage.loaded,
  durable <- TransferImage.durable,
  requests <- TransferImage.requests,
  current <- TransferImage.current,
  logical <- TransferImage.logical,
  ranges <- TransferImage.ranges,
  bindings <- TransferImage.bindings,
  outcomes <- TransferImage.outcomes,
  receipts <- TransferImage.receipts,
  uncertain <- TransferImage.uncertain,
  uses <- TransferImage.uses,
  catalogKnown <- TransferImage.catalogKnown,
  savedDocuments <- TransferImage.savedDocuments,
  packets <- TransferImage.packets,
  logicalUses <- TransferImage.logicalUses
vars == <<CoreImage,ReportingImage,TransferImage>>
NativeLayerEnabled == UseReporting /\ ~UseNS
TransferLayerEnabled == UseTransferReporting /\ ~UseCT
ProductsEnabled == NativeLayerEnabled /\ TransferLayerEnabled
ParentRoots(p) == {root \in Roots:p \in TreeThreads[root]}
ParentRoot(p) == CHOOSE root \in ParentRoots(p):TRUE
ParentPresent(p) == IF Cardinality(ParentRoots(p))#1 THEN FALSE
 ELSE IF ParentRoot(p) \notin F!ActiveRoots THEN FALSE
 ELSE CoreImage[<<ParentRoot(p),ParentRoot(p)>>].created[p]
Core(p) == CoreImage[<<ParentRoot(p),ParentRoot(p)>>]
LogicalJoin ==
 /\ Parents=UNION {TreeThreads[root]:root \in Roots}
 /\ \A p \in Parents:Cardinality(ParentRoots(p))=1
 /\ \A p \in Parents:
      IF ParentPresent(p) THEN
       /\ ReportingImage.runs[p].run=Core(p).rord[p]
       /\ ReportingImage.runs[p].attempt=Core(p).att[p]
       /\ ReportingImage.runs[p].active=(Core(p).liveSet[p]#{})
      ELSE ReportingImage.runs[p].run=0 /\ ReportingImage.runs[p].attempt=0 /\ ~ReportingImage.runs[p].active
CurrentInstalled(p) ==
 IF ~ParentPresent(p) THEN FALSE
 ELSE IF p \notin DOMAIN ReportingImage.handles THEN FALSE
 ELSE IF ReportingImage.handles[p] \notin DOMAIN ReportingImage.sources THEN FALSE
 ELSE LET sid==ReportingImage.handles[p] s==ReportingImage.sources[sid] IN
   ReportingImage.runs[p].active /\ ~s.retired /\ s.parent=p /\
   s.run=ReportingImage.runs[p].run /\ s.attempt=ReportingImage.runs[p].attempt
EnforcedRequest(p) == ReportingImage.runs[p].active /\
 ~Core(p).emitted[p] /\ ~Core(p).intReq[p] /\ p \notin Core(p).owed
IssuedAccessJoin == \A p \in Parents:
 IF CurrentInstalled(p) THEN LET s==ReportingImage.sources[ReportingImage.handles[p]] IN
   /\ s.issuedAccess \in 0..2
   /\ (EnforcedRequest(p) => s.issuedAccess<=Core(p).access[p])
   /\ Core(p).attAcc[p]=s.issuedAccess
   /\ Core(p).attAccAt[p]=[run |-> s.run,attempt |-> s.attempt]
 ELSE TRUE
(* Provider input uses the current issued source. The source hook must pin
   the request issue attempt; an installed current handle alone does not prove
   that a delayed question belongs to that attempt. No MCP receipt is invented. *)
IssuedSourceCurrent(p) ==
 IF ~CurrentInstalled(p) THEN FALSE
 ELSE ReportingImage.sources[ReportingImage.handles[p]].requestIssued
AdmissionCurrent(p) ==
 IF ~CurrentInstalled(p) THEN FALSE
 ELSE IF p \notin DOMAIN ReportingImage.admitted THEN FALSE
 ELSE IF ~ReportingImage.admitted[p].present THEN FALSE
 ELSE LET a==ReportingImage.admitted[p] sid==ReportingImage.handles[p] s==ReportingImage.sources[sid] IN
   /\ s.requestIssued
   /\ a.source=sid /\ a.run=s.run /\ a.attempt=s.attempt
   /\ a.revision=s.revision /\ a.issuedAccess=s.issuedAccess
   /\ s.issuedAccess \in 0..2 /\ s.issuedAccess<=Core(p).access[p]
NativeIdentityJoin ==
 /\ \A k \in NativeIds:Len(k)=2 /\ k[1] \in Parents
 /\ \A k \in DOMAIN ReportingImage.natives:
     /\ ReportingImage.natives[k].parent=k[1]
     /\ ReportingImage.natives[k].source \in DOMAIN ReportingImage.sources
     /\ ReportingImage.sources[ReportingImage.natives[k].source].parent=k[1]
     /\ ReportingImage.natives[k].activation \in ActivationIds\cup{NoActivation}
 /\ \A k \in DOMAIN ReportingImage.tasks:
     /\ ReportingImage.tasks[k].parent=k[1]
     /\ ReportingImage.tasks[k].source=ReportingImage.natives[k].source
     /\ ReportingImage.tasks[k].run=ReportingImage.natives[k].run
     /\ ReportingImage.tasks[k].attempt=ReportingImage.natives[k].attempt
     /\ ReportingImage.tasks[k].activation=ReportingImage.natives[k].activation
     /\ ReportingImage.tasks[k].background=ReportingImage.natives[k].background
 /\ \A k \in DOMAIN ReportingImage.docTasks:ReportingImage.docTasks[k].source=ReportingImage.tasks[k].source
JoinedIdentities == LogicalJoin /\ IssuedAccessJoin /\ NativeIdentityJoin
SourceOwnershipStable ==
 /\ DOMAIN ReportingImage.sources \subseteq DOMAIN ReportingImage.sources'
 /\ \A sid \in DOMAIN ReportingImage.sources:ReportingImage.sources[sid].parent=ReportingImage.sources'[sid].parent

LegacyInitialFrames == \A i \in F!Indexes:
 /\ F!Full(i[1],i[2])!NS!Init /\ F!Full(i[1],i[2])!CT!Init
 /\ CoreImage[i].ctTrk=[t \in TreeThreads[i[1]] |-> FALSE]
 /\ CoreImage[i].consumedBy=[x \in F!Full(i[1],i[2])!CT!XIds |-> "none"]

TransferIdentityJoin ==
 /\ Chats=Parents
 /\ TransferImage.logicalCreated={p \in Parents:ParentPresent(p)}
 /\ \A p \in Parents:
      IF ParentPresent(p) THEN
       /\ TransferImage.logical[p].run=Core(p).rord[p]
       /\ TransferImage.logical[p].attempt=Core(p).att[p]
       /\ TransferImage.logical[p].live=(Core(p).liveSet[p]#{})
      ELSE TransferImage.logical[p].run=0 /\ TransferImage.logical[p].attempt=0 /\ ~TransferImage.logical[p].live
 /\ \A q \in DOMAIN TransferImage.requests:
      TransferImage.requests[q].chat \in Parents
 /\ \A u \in DOMAIN TransferImage.bindings:
      u[1] \in DOMAIN TransferImage.requests /\
      TransferImage.requests[u[1]].chat \in Parents
IssuedTransferJoin == \A sid \in DOMAIN ReportingImage.sources:
 LET source==ReportingImage.sources[sid] IN
 IF ~source.requestIssued THEN TRUE
 ELSE /\ source.issuedOrigin \in Inputs
      /\ source.issuedRevision<=source.revision
      /\ \E q \in DOMAIN TransferImage.requests:
           /\ TransferImage.requests[q].chat=source.parent
           /\ TransferImage.requests[q].input=source.issuedOrigin
           /\ <<source.issuedRun,source.issuedAttempt>> \in TransferImage.uses[q]

JoinedProducts == JoinedIdentities /\ TransferIdentityJoin /\ IssuedTransferJoin /\ LegacyInitialFrames

OC(root,focus) == INSTANCE RuntimeOperationalCoreSpec WITH
  Threads <- TreeThreads[root], Root <- root, NsT <- focus, RootDev <- RootDevices[root],
  queue <- CoreImage[<<root,focus>>].queue,
  held <- CoreImage[<<root,focus>>].held,
  liveSet <- CoreImage[<<root,focus>>].liveSet,
  rord <- CoreImage[<<root,focus>>].rord,
  intReq <- CoreImage[<<root,focus>>].intReq,
  att <- CoreImage[<<root,focus>>].att,
  why <- CoreImage[<<root,focus>>].why,
  emitted <- CoreImage[<<root,focus>>].emitted,
  kidOpen <- CoreImage[<<root,focus>>].kidOpen,
  pev <- CoreImage[<<root,focus>>].pev,
  retries <- CoreImage[<<root,focus>>].retries,
  steers <- CoreImage[<<root,focus>>].steers,
  lastOut <- CoreImage[<<root,focus>>].lastOut,
  rkind <- CoreImage[<<root,focus>>].rkind,
  ulArmed <- CoreImage[<<root,focus>>].ulArmed,
  ulDue <- CoreImage[<<root,focus>>].ulDue,
  rPersonLeft <- CoreImage[<<root,focus>>].rPersonLeft,
  serverLeft <- CoreImage[<<root,focus>>].serverLeft,
  rStopLeft <- CoreImage[<<root,focus>>].rStopLeft,
  ulLeft <- CoreImage[<<root,focus>>].ulLeft,
  rCrashLeft <- CoreImage[<<root,focus>>].rCrashLeft,
  created <- CoreImage[<<root,focus>>].created,
  parent <- CoreImage[<<root,focus>>].parent,
  depth <- CoreImage[<<root,focus>>].depth,
  access <- CoreImage[<<root,focus>>].access,
  rtask <- CoreImage[<<root,focus>>].rtask,
  waitOn <- CoreImage[<<root,focus>>].waitOn,
  zWait <- CoreImage[<<root,focus>>].zWait,
  tSt <- CoreImage[<<root,focus>>].tSt,
  tWake <- CoreImage[<<root,focus>>].tWake,
  tDel <- CoreImage[<<root,focus>>].tDel,
  tCoh <- CoreImage[<<root,focus>>].tCoh,
  pushes <- CoreImage[<<root,focus>>].pushes,
  kidIn <- CoreImage[<<root,focus>>].kidIn,
  qPend <- CoreImage[<<root,focus>>].qPend,
  qAt <- CoreImage[<<root,focus>>].qAt,
  qVis <- CoreImage[<<root,focus>>].qVis,
  qWake <- CoreImage[<<root,focus>>].qWake,
  qUsed <- CoreImage[<<root,focus>>].qUsed,
  answers <- CoreImage[<<root,focus>>].answers,
  qEp <- CoreImage[<<root,focus>>].qEp,
  cDisp <- CoreImage[<<root,focus>>].cDisp,
  cSt <- CoreImage[<<root,focus>>].cSt,
  cDel <- CoreImage[<<root,focus>>].cDel,
  cGen <- CoreImage[<<root,focus>>].cGen,
  outbox <- CoreImage[<<root,focus>>].outbox,
  replayable <- CoreImage[<<root,focus>>].replayable,
  rcpt <- CoreImage[<<root,focus>>].rcpt,
  replayBad <- CoreImage[<<root,focus>>].replayBad,
  receiptBad <- CoreImage[<<root,focus>>].receiptBad,
  doomed <- CoreImage[<<root,focus>>].doomed,
  dPersonLeft <- CoreImage[<<root,focus>>].dPersonLeft,
  failLeft <- CoreImage[<<root,focus>>].failLeft,
  dCrashLeft <- CoreImage[<<root,focus>>].dCrashLeft,
  abandonLeft <- CoreImage[<<root,focus>>].abandonLeft,
  dStopLeft <- CoreImage[<<root,focus>>].dStopLeft,
  narrowLeft <- CoreImage[<<root,focus>>].narrowLeft,
  interruptLeft <- CoreImage[<<root,focus>>].interruptLeft,
  nRst <- CoreImage[<<root,focus>>].nRst,
  nRord <- CoreImage[<<root,focus>>].nRord,
  nEndBy <- CoreImage[<<root,focus>>].nEndBy,
  nRec <- CoreImage[<<root,focus>>].nRec,
  nEst <- CoreImage[<<root,focus>>].nEst,
  nEep <- CoreImage[<<root,focus>>].nEep,
  nErun <- CoreImage[<<root,focus>>].nErun,
  nEbg <- CoreImage[<<root,focus>>].nEbg,
  nHow <- CoreImage[<<root,focus>>].nHow,
  nClosedEp <- CoreImage[<<root,focus>>].nClosedEp,
  nTask <- CoreImage[<<root,focus>>].nTask,
  nSess <- CoreImage[<<root,focus>>].nSess,
  nProot <- CoreImage[<<root,focus>>].nProot,
  nPst <- CoreImage[<<root,focus>>].nPst,
  nPep <- CoreImage[<<root,focus>>].nPep,
  nPrun <- CoreImage[<<root,focus>>].nPrun,
  nPbg <- CoreImage[<<root,focus>>].nPbg,
  nPprog <- CoreImage[<<root,focus>>].nPprog,
  nInbox <- CoreImage[<<root,focus>>].nInbox,
  nReplay <- CoreImage[<<root,focus>>].nReplay,
  nCrashLeft <- CoreImage[<<root,focus>>].nCrashLeft,
  nDupLeft <- CoreImage[<<root,focus>>].nDupLeft,
  nIntLeft <- CoreImage[<<root,focus>>].nIntLeft,
  nSessLeft <- CoreImage[<<root,focus>>].nSessLeft,
  xMsgs <- CoreImage[<<root,focus>>].xMsgs,
  xCreated <- CoreImage[<<root,focus>>].xCreated,
  xForkOf <- CoreImage[<<root,focus>>].xForkOf,
  xKindOf <- CoreImage[<<root,focus>>].xKindOf,
  xRun <- CoreImage[<<root,focus>>].xRun,
  xPt <- CoreImage[<<root,focus>>].xPt,
  xXf <- CoreImage[<<root,focus>>].xXf,
  xNx <- CoreImage[<<root,focus>>].xNx,
  xVol <- CoreImage[<<root,focus>>].xVol,
  xDup <- CoreImage[<<root,focus>>].xDup,
  xAbort <- CoreImage[<<root,focus>>].xAbort,
  xCrashLeft <- CoreImage[<<root,focus>>].xCrashLeft,
  xStaleLeft <- CoreImage[<<root,focus>>].xStaleLeft,
  ctTrk <- CoreImage[<<root,focus>>].ctTrk,
  consumedBy <- CoreImage[<<root,focus>>].consumedBy,
  attAcc <- CoreImage[<<root,focus>>].attAcc,
  attAccAt <- CoreImage[<<root,focus>>].attAccAt,
  waitAtt <- CoreImage[<<root,focus>>].waitAtt,
  f9w <- CoreImage[<<root,focus>>].f9w,
  owed <- CoreImage[<<root,focus>>].owed,
  host <- CoreImage[<<root,focus>>].host,
  online <- CoreImage[<<root,focus>>].online,
  offLeft <- CoreImage[<<root,focus>>].offLeft,
  currentAdmission <- [p \in Parents |-> AdmissionCurrent(p)], currentIssued <- [p \in Parents |-> IssuedSourceCurrent(p)]
CoreNext == F!ForestNext /\
 \A i \in F!Indexes:[OC(i[1],i[2])!GuardedNext]_(F!Full(i[1],i[2])!vars)
NamedSourceAction(root,focus,ev) == LET a == ev.args IN
 CASE ev.label="Stutter" -> UNCHANGED F!Full(root,focus)!vars
    [] ev.label="C_PersonSend" -> F!Full(root,focus)!C_PersonSend(a.t,a.m)
    [] ev.label="C_Steer" -> F!Full(root,focus)!C_Steer(a.t)
    [] ev.label="C_Restart" -> F!Full(root,focus)!C_Restart(a.t,a.m)
    [] ev.label="C_Stop" -> F!Full(root,focus)!C_Stop(a.t)
    [] ev.label="C_InterruptChat" -> F!Full(root,focus)!C_InterruptChat(a.t)
    [] ev.label="C_ServerEnqueue" -> F!Full(root,focus)!C_ServerEnqueue(a.t)
    [] ev.label="C_Narrow" -> F!Full(root,focus)!C_Narrow(a.t)
    [] ev.label="C_ClampRestart" -> F!Full(root,focus)!C_ClampRestart(a.d)
    [] ev.label="C_PersonAnswer" -> F!Full(root,focus)!C_PersonAnswer(a.q)
    [] ev.label="C_ProviderTerminal" -> F!Full(root,focus)!C_ProviderTerminal(a.t,a.o)
    [] ev.label="C_ChildStart" -> F!Full(root,focus)!C_ChildStart(a.t)
    [] ev.label="C_ChildEnd" -> F!Full(root,focus)!C_ChildEnd(a.t)
    [] ev.label="C_ResetPasses" -> F!Full(root,focus)!C_ResetPasses(a.t)
    [] ev.label="C_StartQueue" -> F!Full(root,focus)!C_StartQueue(a.t)
    [] ev.label="C_StartWake" -> F!Full(root,focus)!C_StartWake(a.t)
    [] ev.label="C_StartQuestion" -> F!Full(root,focus)!C_StartQuestion(a.t,a.q)
    [] ev.label="C_ULRecovery" -> F!Full(root,focus)!C_ULRecovery(a.t)
    [] ev.label="C_IngestEnd" -> F!Full(root,focus)!C_IngestEnd(a.t,a.e)
    [] ev.label="C_RetryAttempt" -> F!Full(root,focus)!C_RetryAttempt(a.t,a.e)
    [] ev.label="C_IngestChild" -> F!Full(root,focus)!C_IngestChild(a.t,a.e)
    [] ev.label="C_IngestStale" -> F!Full(root,focus)!C_IngestStale(a.t,a.e)
    [] ev.label="C_Delegate" -> F!Full(root,focus)!C_Delegate(a.t,a.c,a.m,a.a)
    [] ev.label="C_FollowUp" -> F!Full(root,focus)!C_FollowUp(a.t,a.c,a.m)
    [] ev.label="C_ReadResult" -> F!Full(root,focus)!C_ReadResult(a.p,a.c)
    [] ev.label="C_RaiseQuestion" -> F!Full(root,focus)!C_RaiseQuestion(a.c)
    [] ev.label="C_AgentAnswer" -> F!Full(root,focus)!C_AgentAnswer(a.q,a.x)
    [] ev.label="C_RaiseQuestionS" -> F!Full(root,focus)!C_RaiseQuestionS(a.c)
    [] ev.label="C_AgentAnswerS" -> F!Full(root,focus)!C_AgentAnswerS(a.q,a.x)
    [] ev.label="C_PersonAnswerS" -> F!Full(root,focus)!C_PersonAnswerS(a.q)
    [] ev.label="C_DoQHop" -> F!Full(root,focus)!C_DoQHop(a.e)
    [] ev.label="C_WaitReturn" -> F!Full(root,focus)!C_WaitReturn(a.t)
    [] ev.label="C_WaitReturnQuestion" -> F!Full(root,focus)!C_WaitReturnQuestion(a.t)
    [] ev.label="C_WaitTimeout" -> F!Full(root,focus)!C_WaitTimeout(a.t)
    [] ev.label="C_WaitAbandon" -> F!Full(root,focus)!C_WaitAbandon(a.t)
    [] ev.label="C_LateWaitAck" -> F!Full(root,focus)!C_LateWaitAck(a.t)
    [] ev.label="C_DoNotice" -> F!Full(root,focus)!C_DoNotice(a.e,a.how)
    [] ev.label="C_DoOffer" -> F!Full(root,focus)!C_DoOffer(a.e,a.how)
    [] ev.label="C_DoStop" -> F!Full(root,focus)!C_DoStop(a.e,a.how)
    [] ev.label="C_CrashEngine" -> F!Full(root,focus)!C_CrashEngine
    [] ev.label="C_GoOffline" -> F!Full(root,focus)!C_GoOffline(a.d)
    [] ev.label="C_GoOnline" -> F!Full(root,focus)!C_GoOnline(a.d)
    [] ev.label="C_NsSpawn" -> F!Full(root,focus)!C_NsSpawn(a.s,a.bg)
    [] ev.label="C_NsResume" -> F!Full(root,focus)!C_NsResume(a.s)
    [] ev.label="C_NsProgress" -> F!Full(root,focus)!C_NsProgress(a.s)
    [] ev.label="C_NsSubEnd" -> F!Full(root,focus)!C_NsSubEnd(a.s,a.out)
    [] ev.label="C_NsSessionEnd" -> F!Full(root,focus)!C_NsSessionEnd
    [] ev.label="C_NsReplay" -> F!Full(root,focus)!C_NsReplay(a.e)
    [] ev.label="C_NsHandle" -> F!Full(root,focus)!C_NsHandle
    [] ev.label="C_NsCrash" -> F!Full(root,focus)!C_NsCrash
    [] ev.label="C_NsRecover" -> F!Full(root,focus)!C_NsRecover
    [] ev.label="C_CtFork" -> F!Full(root,focus)!C_CtFork(a.s,a.f)
    [] ev.label="C_CtMergeBack" -> F!Full(root,focus)!C_CtMergeBack(a.f)
    [] ev.label="C_CtSubagentResult" -> F!Full(root,focus)!C_CtSubagentResult(a.c)
    [] ev.label="C_CtResolve" -> F!Full(root,focus)!C_CtResolve(a.t,a.x)
    [] ev.label="C_CtDeliver" -> F!Full(root,focus)!C_CtDeliver(a.t)
    [] ev.label="C_CtRecoverDelivery" -> F!Full(root,focus)!C_CtRecoverDelivery(a.t,a.p)
    [] ev.label="C_CtCrash" -> F!Full(root,focus)!C_CtCrash
    [] ev.label="C_CtFreeSend" -> F!Full(root,focus)!C_CtFreeSend(a.t,a.p)
    [] ev.label="C_ObserveProviderTerminal" -> F!Full(root,focus)!C_ObserveProviderTerminal(a.t,a.o)
    [] ev.label="C_ObserveProviderStart" -> F!Full(root,focus)!C_ObserveProviderStart(a.t)
    [] ev.label="C_ObserveAccess" -> F!Full(root,focus)!C_ObserveAccess(a.t,a.a)
    [] OTHER -> FALSE

NamedReportingAction(ev) == LET a==ev.args IN
 CASE ev.label="Stutter" -> UNCHANGED RP!rv
 [] ev.label="LogicalStart" -> RP!LogicalStart(a.p)
 [] ev.label="InstallHandle" -> RP!InstallHandle(a.p,a.sid,a.access,a.revision,a.origin)
 [] ev.label="RebindHandle" -> RP!RebindHandle(a.sid,a.access,a.revision,a.origin)
 [] ev.label="ConfigHandle" -> RP!ConfigHandle(a.sid,a.access,a.revision,a.origin)
 [] ev.label="IssueRequest" -> RP!IssueRequest(a.sid,a.access,a.revision,a.origin)
 [] ev.label="RetireHandle" -> RP!RetireHandle(a.sid,a.revision,a.remove)
 [] ev.label="Admit" -> RP!Admit(a.sid)
 [] ev.label="Refuse" -> RP!Refuse(a.p)
 [] ev.label="LogicalAttempt" -> RP!LogicalAttempt(a.p)
 [] ev.label="ObserveStreamOpen" -> RP!ObserveStreamOpen(a.sid)
 [] ev.label="ObserveRootStart" -> RP!ObserveRootStart(a.sid)
 [] ev.label="ObserveSpawn" -> RP!ObserveSpawn(a.sid,a.k,a.ancestor,a.ownBackground)
 [] ev.label="ObserveNativeEnd" -> RP!ObserveNativeEnd(a.k,a.out)
 [] ev.label="ObserveRootEnd" -> RP!ObserveRootEnd(a.sid,a.out)
 [] ev.label="EngineEOFRootTerminal" -> RP!EngineEOFRootTerminal(a.sid,a.out)
 [] ev.label="ObserveStreamClose" -> RP!ObserveStreamClose(a.sid)
 [] ev.label="RejectSpawn" -> RP!RejectSpawn(a.i)
 [] ev.label="RecordDeliveryGeneration" -> RP!RecordDeliveryGeneration(a.k,a.generation)
 [] ev.label="Handle" -> RP!Handle(a.i)
 [] ev.label="HandleLossNative" -> RP!HandleLossNative(a.i,a.k)
 [] ev.label="BindActivation" -> RP!BindActivation(a.k,a.activation)
 [] ev.label="PersistTask" -> RP!PersistTask(a.k,a.status)
 [] ev.label="ObserveCatalog" -> RP!ObserveCatalog(a.ps)
 [] OTHER -> FALSE
NamedTransferAction(ev) == LET a==ev.args IN
 CASE ev.label="Stutter" -> UNCHANGED TR!ctVars
 [] ev.label="OpenLogical" -> TR!OpenLogical(a.c)
 [] ev.label="LoadDocument" -> TR!LoadDocument(a.c)
 [] ev.label="ObserveSavedDocuments" -> TR!ObserveSavedDocuments(a.catalog)
 [] ev.label="SaveDocument" -> TR!SaveDocument(a.c,a.d)
 [] ev.label="LogicalStart" -> TR!LogicalStart(a.c,a.r,a.a)
 [] ev.label="LogicalAttempt" -> TR!LogicalAttempt(a.c,a.r,a.a)
 [] ev.label="LogicalFinish" -> TR!LogicalFinish(a.c,a.r,a.a)
 [] ev.label="PinRange" -> TR!PinRange(a.p,a.range)
 [] ev.label="PrepareRequest" -> TR!PrepareRequest(a.q,a.c,a.h,a.i,a.p,a.packet,a.selected,a.r,a.a)
 [] ev.label="BindSession" -> TR!BindSession(a.q,a.r,a.a,a.session)
 [] ev.label="RecordOutcome" -> TR!RecordOutcome(a.q,a.r,a.a,a.outcome)
 [] ev.label="SaveConfirmation" -> TR!SaveConfirmation(a.id,a.q,a.packet,a.session,a.input,a.r,a.a,a.document)
 [] OTHER -> FALSE
AgentActionJoined(ev) == LET a==ev.args IN
 CASE ev.label \in {"C_Delegate","C_FollowUp"} -> AdmissionCurrent(a.t)
 [] ev.label="C_ReadResult" -> AdmissionCurrent(a.p)
 [] ev.label \in {"C_RaiseQuestion","C_RaiseQuestionS"} -> IssuedSourceCurrent(a.c)
 [] ev.label \in {"C_AgentAnswer","C_AgentAnswerS"} -> AdmissionCurrent(a.x)
 [] OTHER -> TRUE

NamedCoreStep(events) ==
 /\ \A i \in F!Indexes:
      NamedSourceAction(i[1],i[2],events[i]) /\ AgentActionJoined(events[i])
 /\ \A i \in F!Indexes'\F!Indexes:
      events[i].label="RegisterRoot" /\ F!Full(i[1],i[2])!Init'
Init == ProductsEnabled /\ F!ForestInit /\ RP!Init /\ TR!Init /\ JoinedProducts
Next ==
 /\ CoreNext /\ [RP!Next]_(RP!rv) /\ [TR!Next]_(TR!ctVars)
 /\ [RP!SafetyStep]_(RP!rv) /\ SourceOwnershipStable /\ JoinedProducts'
NamedStep(coreEvents,reportEvent,transferEvent) ==
 /\ NamedCoreStep(coreEvents)
 /\ NamedReportingAction(reportEvent) /\ NamedTransferAction(transferEvent)
 /\ SourceOwnershipStable /\ JoinedProducts'
Spec == Init /\ [][Next]_vars
Safety == ProductsEnabled /\ F!ForestType /\ RP!Safety /\ TR!Safety /\ JoinedProducts
ReportingSafety == RP!Safety
TransferSafety == TR!Safety
ReportingTransitionSafety == [][RP!SafetyStep]_(RP!rv)
OnlyRootEndsRun == RP!OnlyRootEndsRun
OrphansClosed == RP!OrphansClosed
NoFalseProcessDeath == RP!NoFalseProcessDeath
LossClosesOnlySource == RP!LossClosesOnlySource
BackgroundSurvivesTurn == RP!BackgroundSurvivesTurn
AncestorTerminalClosesForeground == RP!AncestorTerminalClosesForeground
ReportingFairClosure == RP!FairClosure
FairReportingClosure == ReportingFairClosure
====
