-------------------- MODULE RuntimeForestCompositionSpec --------------------
(* A product of full Composition instances, one per root and native parent.
   Shared RL/D/CT/placement fields agree within each root. Each NS focus has
   its own native state. Actual raw mapping and cross-root CT checks remain
   required. Empty/missing native focus sets are not allowed. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS Roots, TreeThreads, RootDevices, NoThread, MaxRuns, MaxQueue, Mut, RLMaxPerson, RLMaxServer, RLMaxRetry, RLMaxStop, RLMaxUL, RLMaxCrash, RLMaxAtt, FixLimitRelease, MaxDepth, MaxFanout, MaxGen, DMaxPerson, MaxFail, DMaxCrash, MaxAbandon, DMaxStop, MaxNarrow, MaxInterrupt, DupDelivery, Crash, LateAck, Questions, FixWakeSkipsFailHold, StopCheckAnc, PartialCrash, QSplit, MaxFollowUp, UseNS, Subs, NoSub, MaxEp, NSMaxCrash, MaxDup, NSMaxInterrupt, MaxSessEnd, MaxInbox, Background, SilentKill, UseCT, Providers, NoProv, CanHandoff, CanInject, NativeFork, MainProviders, MaxMsg, MaxX, CTMaxCrash, MaxStale, Forks, Spawns, Cuts, MaxCutK, BudgetFail, InPlace, ToolNodes, UsePL, Devices, MaxOffline
VARIABLE Image
ActiveRoots == {root \in Roots : <<root,root>> \in DOMAIN Image}
Indexes == UNION {{<<root,t>>:t \in TreeThreads[root]}:root \in ActiveRoots}
Full(root,focus) == INSTANCE RuntimeCompositionSpec WITH
  Threads <- TreeThreads[root], Root <- root, NsT <- focus, RootDev <- RootDevices[root],
  queue <- Image[<<root,focus>>].queue,
  held <- Image[<<root,focus>>].held,
  liveSet <- Image[<<root,focus>>].liveSet,
  rord <- Image[<<root,focus>>].rord,
  intReq <- Image[<<root,focus>>].intReq,
  att <- Image[<<root,focus>>].att,
  why <- Image[<<root,focus>>].why,
  emitted <- Image[<<root,focus>>].emitted,
  kidOpen <- Image[<<root,focus>>].kidOpen,
  pev <- Image[<<root,focus>>].pev,
  retries <- Image[<<root,focus>>].retries,
  steers <- Image[<<root,focus>>].steers,
  lastOut <- Image[<<root,focus>>].lastOut,
  rkind <- Image[<<root,focus>>].rkind,
  ulArmed <- Image[<<root,focus>>].ulArmed,
  ulDue <- Image[<<root,focus>>].ulDue,
  rPersonLeft <- Image[<<root,focus>>].rPersonLeft,
  serverLeft <- Image[<<root,focus>>].serverLeft,
  rStopLeft <- Image[<<root,focus>>].rStopLeft,
  ulLeft <- Image[<<root,focus>>].ulLeft,
  rCrashLeft <- Image[<<root,focus>>].rCrashLeft,
  created <- Image[<<root,focus>>].created,
  parent <- Image[<<root,focus>>].parent,
  depth <- Image[<<root,focus>>].depth,
  access <- Image[<<root,focus>>].access,
  rtask <- Image[<<root,focus>>].rtask,
  waitOn <- Image[<<root,focus>>].waitOn,
  zWait <- Image[<<root,focus>>].zWait,
  tSt <- Image[<<root,focus>>].tSt,
  tWake <- Image[<<root,focus>>].tWake,
  tDel <- Image[<<root,focus>>].tDel,
  tCoh <- Image[<<root,focus>>].tCoh,
  pushes <- Image[<<root,focus>>].pushes,
  kidIn <- Image[<<root,focus>>].kidIn,
  qPend <- Image[<<root,focus>>].qPend,
  qAt <- Image[<<root,focus>>].qAt,
  qVis <- Image[<<root,focus>>].qVis,
  qWake <- Image[<<root,focus>>].qWake,
  qUsed <- Image[<<root,focus>>].qUsed,
  answers <- Image[<<root,focus>>].answers,
  qEp <- Image[<<root,focus>>].qEp,
  cDisp <- Image[<<root,focus>>].cDisp,
  cSt <- Image[<<root,focus>>].cSt,
  cDel <- Image[<<root,focus>>].cDel,
  cGen <- Image[<<root,focus>>].cGen,
  outbox <- Image[<<root,focus>>].outbox,
  replayable <- Image[<<root,focus>>].replayable,
  rcpt <- Image[<<root,focus>>].rcpt,
  replayBad <- Image[<<root,focus>>].replayBad,
  receiptBad <- Image[<<root,focus>>].receiptBad,
  doomed <- Image[<<root,focus>>].doomed,
  dPersonLeft <- Image[<<root,focus>>].dPersonLeft,
  failLeft <- Image[<<root,focus>>].failLeft,
  dCrashLeft <- Image[<<root,focus>>].dCrashLeft,
  abandonLeft <- Image[<<root,focus>>].abandonLeft,
  dStopLeft <- Image[<<root,focus>>].dStopLeft,
  narrowLeft <- Image[<<root,focus>>].narrowLeft,
  interruptLeft <- Image[<<root,focus>>].interruptLeft,
  nRst <- Image[<<root,focus>>].nRst,
  nRord <- Image[<<root,focus>>].nRord,
  nEndBy <- Image[<<root,focus>>].nEndBy,
  nRec <- Image[<<root,focus>>].nRec,
  nEst <- Image[<<root,focus>>].nEst,
  nEep <- Image[<<root,focus>>].nEep,
  nErun <- Image[<<root,focus>>].nErun,
  nEbg <- Image[<<root,focus>>].nEbg,
  nHow <- Image[<<root,focus>>].nHow,
  nClosedEp <- Image[<<root,focus>>].nClosedEp,
  nTask <- Image[<<root,focus>>].nTask,
  nSess <- Image[<<root,focus>>].nSess,
  nProot <- Image[<<root,focus>>].nProot,
  nPst <- Image[<<root,focus>>].nPst,
  nPep <- Image[<<root,focus>>].nPep,
  nPrun <- Image[<<root,focus>>].nPrun,
  nPbg <- Image[<<root,focus>>].nPbg,
  nPprog <- Image[<<root,focus>>].nPprog,
  nInbox <- Image[<<root,focus>>].nInbox,
  nReplay <- Image[<<root,focus>>].nReplay,
  nCrashLeft <- Image[<<root,focus>>].nCrashLeft,
  nDupLeft <- Image[<<root,focus>>].nDupLeft,
  nIntLeft <- Image[<<root,focus>>].nIntLeft,
  nSessLeft <- Image[<<root,focus>>].nSessLeft,
  xMsgs <- Image[<<root,focus>>].xMsgs,
  xCreated <- Image[<<root,focus>>].xCreated,
  xForkOf <- Image[<<root,focus>>].xForkOf,
  xKindOf <- Image[<<root,focus>>].xKindOf,
  xRun <- Image[<<root,focus>>].xRun,
  xPt <- Image[<<root,focus>>].xPt,
  xXf <- Image[<<root,focus>>].xXf,
  xNx <- Image[<<root,focus>>].xNx,
  xVol <- Image[<<root,focus>>].xVol,
  xDup <- Image[<<root,focus>>].xDup,
  xAbort <- Image[<<root,focus>>].xAbort,
  xCrashLeft <- Image[<<root,focus>>].xCrashLeft,
  xStaleLeft <- Image[<<root,focus>>].xStaleLeft,
  ctTrk <- Image[<<root,focus>>].ctTrk,
  consumedBy <- Image[<<root,focus>>].consumedBy,
  attAcc <- Image[<<root,focus>>].attAcc,
  attAccAt <- Image[<<root,focus>>].attAccAt,
  waitAtt <- Image[<<root,focus>>].waitAtt,
  f9w <- Image[<<root,focus>>].f9w,
  owed <- Image[<<root,focus>>].owed,
  host <- Image[<<root,focus>>].host,
  online <- Image[<<root,focus>>].online,
  offLeft <- Image[<<root,focus>>].offLeft
SharedFields == {"queue", "held", "liveSet", "rord", "intReq", "att", "why", "emitted", "kidOpen", "pev", "retries", "steers", "lastOut", "rkind", "ulArmed", "ulDue", "rPersonLeft", "serverLeft", "rStopLeft", "ulLeft", "rCrashLeft", "created", "parent", "depth", "access", "rtask", "waitOn", "zWait", "tSt", "tWake", "tDel", "tCoh", "pushes", "kidIn", "qPend", "qAt", "qVis", "qWake", "qUsed", "answers", "qEp", "cDisp", "cSt", "cDel", "cGen", "outbox", "replayable", "rcpt", "replayBad", "receiptBad", "doomed", "dPersonLeft", "failLeft", "dCrashLeft", "abandonLeft", "dStopLeft", "narrowLeft", "interruptLeft", "xMsgs", "xCreated", "xForkOf", "xKindOf", "xRun", "xPt", "xXf", "xNx", "xVol", "xDup", "xAbort", "xCrashLeft", "xStaleLeft", "ctTrk", "consumedBy", "attAcc", "attAccAt", "waitAtt", "f9w", "owed", "host", "online", "offLeft"}
SharedInRoot == \A root \in ActiveRoots :
 \A f,g \in TreeThreads[root] : \A field \in SharedFields :
  Image[<<root,f>>][field]=Image[<<root,g>>][field]
ForestInit ==
 /\ DOMAIN Image=Indexes /\ SharedInRoot
 /\ \A root \in Roots : root \in TreeThreads[root]
 /\ \A i \in Indexes : Full(i[1],i[2])!Init
ForestNext ==
 /\ DOMAIN Image'=Indexes' /\ SharedInRoot'
 /\ ActiveRoots \subseteq ActiveRoots'
 /\ \A i \in Indexes : [Full(i[1],i[2])!Next]_(Full(i[1],i[2])!vars)
 /\ \A i \in Indexes' \ Indexes : Full(i[1],i[2])!Init' 
ForestSpec == ForestInit /\ [][ForestNext]_Image
ForestType == SharedInRoot /\ \A i \in Indexes : Full(i[1],i[2])!C_TypeOK
====
