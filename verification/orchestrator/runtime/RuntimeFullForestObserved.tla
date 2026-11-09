---------------- MODULE RuntimeFullForestObserved ----------------
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS Roots, TreeThreads, RootDevices, NoThread, MaxRuns, MaxQueue, Mut, RLMaxPerson, RLMaxServer, RLMaxRetry, RLMaxStop, RLMaxUL, RLMaxCrash, RLMaxAtt, FixLimitRelease, MaxDepth, MaxFanout, MaxGen, DMaxPerson, MaxFail, DMaxCrash, MaxAbandon, DMaxStop, MaxNarrow, MaxInterrupt, DupDelivery, Crash, LateAck, Questions, FixWakeSkipsFailHold, StopCheckAnc, PartialCrash, QSplit, MaxFollowUp, UseNS, Subs, NoSub, MaxEp, NSMaxCrash, MaxDup, NSMaxInterrupt, MaxSessEnd, MaxInbox, Background, SilentKill, UseCT, Providers, NoProv, CanHandoff, CanInject, NativeFork, MainProviders, MaxMsg, MaxX, CTMaxCrash, MaxStale, Forks, Spawns, Cuts, MaxCutK, BudgetFail, InPlace, ToolNodes, UsePL, Devices, MaxOffline, Images, Actions
VARIABLE cursor
F == INSTANCE RuntimeForestCompositionSpec WITH Image <- Images[cursor]
Init == cursor=1
Next == IF cursor<Len(Images) THEN cursor'=cursor+1 ELSE UNCHANGED cursor
Spec == Init /\ [][Next]_cursor
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
NamedSourceStep == IF cursor'=cursor THEN UNCHANGED Images[cursor]
 ELSE /\ \A i \in F!Indexes : NamedSourceAction(i[1],i[2],Actions[cursor][i])
      /\ \A i \in F!Indexes' \ F!Indexes :
           Actions[cursor][i].label="RegisterRoot" /\ F!Full(i[1],i[2])!Init' 
NamedPath == [][NamedSourceStep]_cursor
MappedSpec == F!ForestInit /\ [][F!ForestNext]_(Images[cursor])
MappedType == F!ForestType
====
