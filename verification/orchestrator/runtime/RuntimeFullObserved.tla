---------------------- MODULE RuntimeFullObserved ----------------------
(* Strict source-step interface. Images MUST be produced by the raw mapper.
   This module never searches for a model path to match an observed hash.
   Each finite macro step must be expanded to named source actions first. *)
EXTENDS Naturals, Sequences, TLC
CONSTANTS Threads, Root, NoThread, MaxRuns, MaxQueue, Mut, RLMaxPerson, RLMaxServer, RLMaxRetry, RLMaxStop, RLMaxUL, RLMaxCrash, RLMaxAtt, FixLimitRelease, MaxDepth, MaxFanout, MaxGen, DMaxPerson, MaxFail, DMaxCrash, MaxAbandon, DMaxStop, MaxNarrow, MaxInterrupt, DupDelivery, Crash, LateAck, Questions, FixWakeSkipsFailHold, StopCheckAnc, PartialCrash, QSplit, MaxFollowUp, UseNS, NsT, Subs, NoSub, MaxEp, NSMaxCrash, MaxDup, NSMaxInterrupt, MaxSessEnd, MaxInbox, Background, SilentKill, UseCT, Providers, NoProv, CanHandoff, CanInject, NativeFork, MainProviders, MaxMsg, MaxX, CTMaxCrash, MaxStale, Forks, Spawns, Cuts, MaxCutK, BudgetFail, InPlace, ToolNodes, UsePL, Devices, RootDev, MaxOffline, Images, Actions
VARIABLE cursor
Full == INSTANCE RuntimeCompositionSpec WITH
  queue <- Images[cursor].queue,
  held <- Images[cursor].held,
  liveSet <- Images[cursor].liveSet,
  rord <- Images[cursor].rord,
  intReq <- Images[cursor].intReq,
  att <- Images[cursor].att,
  why <- Images[cursor].why,
  emitted <- Images[cursor].emitted,
  kidOpen <- Images[cursor].kidOpen,
  pev <- Images[cursor].pev,
  retries <- Images[cursor].retries,
  steers <- Images[cursor].steers,
  lastOut <- Images[cursor].lastOut,
  rkind <- Images[cursor].rkind,
  ulArmed <- Images[cursor].ulArmed,
  ulDue <- Images[cursor].ulDue,
  rPersonLeft <- Images[cursor].rPersonLeft,
  serverLeft <- Images[cursor].serverLeft,
  rStopLeft <- Images[cursor].rStopLeft,
  ulLeft <- Images[cursor].ulLeft,
  rCrashLeft <- Images[cursor].rCrashLeft,
  created <- Images[cursor].created,
  parent <- Images[cursor].parent,
  depth <- Images[cursor].depth,
  access <- Images[cursor].access,
  rtask <- Images[cursor].rtask,
  waitOn <- Images[cursor].waitOn,
  zWait <- Images[cursor].zWait,
  tSt <- Images[cursor].tSt,
  tWake <- Images[cursor].tWake,
  tDel <- Images[cursor].tDel,
  tCoh <- Images[cursor].tCoh,
  pushes <- Images[cursor].pushes,
  kidIn <- Images[cursor].kidIn,
  qPend <- Images[cursor].qPend,
  qAt <- Images[cursor].qAt,
  qVis <- Images[cursor].qVis,
  qWake <- Images[cursor].qWake,
  qUsed <- Images[cursor].qUsed,
  answers <- Images[cursor].answers,
  qEp <- Images[cursor].qEp,
  cDisp <- Images[cursor].cDisp,
  cSt <- Images[cursor].cSt,
  cDel <- Images[cursor].cDel,
  cGen <- Images[cursor].cGen,
  outbox <- Images[cursor].outbox,
  replayable <- Images[cursor].replayable,
  rcpt <- Images[cursor].rcpt,
  replayBad <- Images[cursor].replayBad,
  receiptBad <- Images[cursor].receiptBad,
  doomed <- Images[cursor].doomed,
  dPersonLeft <- Images[cursor].dPersonLeft,
  failLeft <- Images[cursor].failLeft,
  dCrashLeft <- Images[cursor].dCrashLeft,
  abandonLeft <- Images[cursor].abandonLeft,
  dStopLeft <- Images[cursor].dStopLeft,
  narrowLeft <- Images[cursor].narrowLeft,
  interruptLeft <- Images[cursor].interruptLeft,
  nRst <- Images[cursor].nRst,
  nRord <- Images[cursor].nRord,
  nEndBy <- Images[cursor].nEndBy,
  nRec <- Images[cursor].nRec,
  nEst <- Images[cursor].nEst,
  nEep <- Images[cursor].nEep,
  nErun <- Images[cursor].nErun,
  nEbg <- Images[cursor].nEbg,
  nHow <- Images[cursor].nHow,
  nClosedEp <- Images[cursor].nClosedEp,
  nTask <- Images[cursor].nTask,
  nSess <- Images[cursor].nSess,
  nProot <- Images[cursor].nProot,
  nPst <- Images[cursor].nPst,
  nPep <- Images[cursor].nPep,
  nPrun <- Images[cursor].nPrun,
  nPbg <- Images[cursor].nPbg,
  nPprog <- Images[cursor].nPprog,
  nInbox <- Images[cursor].nInbox,
  nReplay <- Images[cursor].nReplay,
  nCrashLeft <- Images[cursor].nCrashLeft,
  nDupLeft <- Images[cursor].nDupLeft,
  nIntLeft <- Images[cursor].nIntLeft,
  nSessLeft <- Images[cursor].nSessLeft,
  xMsgs <- Images[cursor].xMsgs,
  xCreated <- Images[cursor].xCreated,
  xForkOf <- Images[cursor].xForkOf,
  xKindOf <- Images[cursor].xKindOf,
  xRun <- Images[cursor].xRun,
  xPt <- Images[cursor].xPt,
  xXf <- Images[cursor].xXf,
  xNx <- Images[cursor].xNx,
  xVol <- Images[cursor].xVol,
  xDup <- Images[cursor].xDup,
  xAbort <- Images[cursor].xAbort,
  xCrashLeft <- Images[cursor].xCrashLeft,
  xStaleLeft <- Images[cursor].xStaleLeft,
  ctTrk <- Images[cursor].ctTrk,
  consumedBy <- Images[cursor].consumedBy,
  attAcc <- Images[cursor].attAcc,
  attAccAt <- Images[cursor].attAccAt,
  waitAtt <- Images[cursor].waitAtt,
  f9w <- Images[cursor].f9w,
  owed <- Images[cursor].owed,
  host <- Images[cursor].host,
  online <- Images[cursor].online,
  offLeft <- Images[cursor].offLeft
Init == cursor=1
Next == IF cursor<Len(Images) THEN cursor'=cursor+1 ELSE UNCHANGED cursor
Spec == Init /\ [][Next]_cursor
NamedSourceAction(ev) == LET a == ev.args IN
 CASE ev.label="C_PersonSend" -> Full!C_PersonSend(a.t,a.m)
    [] ev.label="C_Steer" -> Full!C_Steer(a.t)
    [] ev.label="C_Restart" -> Full!C_Restart(a.t,a.m)
    [] ev.label="C_Stop" -> Full!C_Stop(a.t)
    [] ev.label="C_InterruptChat" -> Full!C_InterruptChat(a.t)
    [] ev.label="C_ServerEnqueue" -> Full!C_ServerEnqueue(a.t)
    [] ev.label="C_Narrow" -> Full!C_Narrow(a.t)
    [] ev.label="C_ClampRestart" -> Full!C_ClampRestart(a.d)
    [] ev.label="C_PersonAnswer" -> Full!C_PersonAnswer(a.q)
    [] ev.label="C_ProviderTerminal" -> Full!C_ProviderTerminal(a.t,a.o)
    [] ev.label="C_ChildStart" -> Full!C_ChildStart(a.t)
    [] ev.label="C_ChildEnd" -> Full!C_ChildEnd(a.t)
    [] ev.label="C_ResetPasses" -> Full!C_ResetPasses(a.t)
    [] ev.label="C_StartQueue" -> Full!C_StartQueue(a.t)
    [] ev.label="C_StartWake" -> Full!C_StartWake(a.t)
    [] ev.label="C_StartQuestion" -> Full!C_StartQuestion(a.t,a.q)
    [] ev.label="C_ULRecovery" -> Full!C_ULRecovery(a.t)
    [] ev.label="C_IngestEnd" -> Full!C_IngestEnd(a.t,a.e)
    [] ev.label="C_RetryAttempt" -> Full!C_RetryAttempt(a.t,a.e)
    [] ev.label="C_IngestChild" -> Full!C_IngestChild(a.t,a.e)
    [] ev.label="C_IngestStale" -> Full!C_IngestStale(a.t,a.e)
    [] ev.label="C_Delegate" -> Full!C_Delegate(a.t,a.c,a.m,a.a)
    [] ev.label="C_FollowUp" -> Full!C_FollowUp(a.t,a.c,a.m)
    [] ev.label="C_ReadResult" -> Full!C_ReadResult(a.p,a.c)
    [] ev.label="C_RaiseQuestion" -> Full!C_RaiseQuestion(a.c)
    [] ev.label="C_AgentAnswer" -> Full!C_AgentAnswer(a.q,a.x)
    [] ev.label="C_RaiseQuestionS" -> Full!C_RaiseQuestionS(a.c)
    [] ev.label="C_AgentAnswerS" -> Full!C_AgentAnswerS(a.q,a.x)
    [] ev.label="C_PersonAnswerS" -> Full!C_PersonAnswerS(a.q)
    [] ev.label="C_DoQHop" -> Full!C_DoQHop(a.e)
    [] ev.label="C_WaitReturn" -> Full!C_WaitReturn(a.t)
    [] ev.label="C_WaitReturnQuestion" -> Full!C_WaitReturnQuestion(a.t)
    [] ev.label="C_WaitTimeout" -> Full!C_WaitTimeout(a.t)
    [] ev.label="C_WaitAbandon" -> Full!C_WaitAbandon(a.t)
    [] ev.label="C_LateWaitAck" -> Full!C_LateWaitAck(a.t)
    [] ev.label="C_DoNotice" -> Full!C_DoNotice(a.e,a.how)
    [] ev.label="C_DoOffer" -> Full!C_DoOffer(a.e,a.how)
    [] ev.label="C_DoStop" -> Full!C_DoStop(a.e,a.how)
    [] ev.label="C_CrashEngine" -> Full!C_CrashEngine
    [] ev.label="C_GoOffline" -> Full!C_GoOffline(a.d)
    [] ev.label="C_GoOnline" -> Full!C_GoOnline(a.d)
    [] ev.label="C_NsSpawn" -> Full!C_NsSpawn(a.s,a.bg)
    [] ev.label="C_NsResume" -> Full!C_NsResume(a.s)
    [] ev.label="C_NsProgress" -> Full!C_NsProgress(a.s)
    [] ev.label="C_NsSubEnd" -> Full!C_NsSubEnd(a.s,a.out)
    [] ev.label="C_NsSessionEnd" -> Full!C_NsSessionEnd
    [] ev.label="C_NsReplay" -> Full!C_NsReplay(a.e)
    [] ev.label="C_NsHandle" -> Full!C_NsHandle
    [] ev.label="C_NsCrash" -> Full!C_NsCrash
    [] ev.label="C_NsRecover" -> Full!C_NsRecover
    [] ev.label="C_CtFork" -> Full!C_CtFork(a.s,a.f)
    [] ev.label="C_CtMergeBack" -> Full!C_CtMergeBack(a.f)
    [] ev.label="C_CtSubagentResult" -> Full!C_CtSubagentResult(a.c)
    [] ev.label="C_CtResolve" -> Full!C_CtResolve(a.t,a.x)
    [] ev.label="C_CtDeliver" -> Full!C_CtDeliver(a.t)
    [] ev.label="C_CtRecoverDelivery" -> Full!C_CtRecoverDelivery(a.t,a.p)
    [] ev.label="C_CtCrash" -> Full!C_CtCrash
    [] ev.label="C_CtFreeSend" -> Full!C_CtFreeSend(a.t,a.p)
    [] ev.label="C_ObserveProviderTerminal" -> Full!C_ObserveProviderTerminal(a.t,a.o)
    [] ev.label="C_ObserveProviderStart" -> Full!C_ObserveProviderStart(a.t)
    [] ev.label="C_ObserveAccess" -> Full!C_ObserveAccess(a.t,a.a)
    [] OTHER -> FALSE
NamedSourceStep == IF cursor'=cursor THEN UNCHANGED Full!vars
                   ELSE NamedSourceAction(Actions[cursor])
NamedPath == [][NamedSourceStep]_cursor
MappedSpec == Full!Init /\ [][Full!Next]_(Full!vars)
MappedType == Full!C_TypeOK
====
