---------------- MODULE RuntimeOperationalFullObserved ----------------
(* Exact finite path over the reusable three-product operational target.
   The source cursor is unconstrained by target guards. Each property checks
   the given adjacent pair, including every declared macro intermediate.
   Finite replay does not establish general implementation fair closure. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS Roots, TreeThreads, RootDevices, NoThread, MaxRuns, MaxQueue, Mut, RLMaxPerson, RLMaxServer, RLMaxRetry, RLMaxStop, RLMaxUL, RLMaxCrash, RLMaxAtt, FixLimitRelease, MaxDepth, MaxFanout, MaxGen, DMaxPerson, MaxFail, DMaxCrash, MaxAbandon, DMaxStop, MaxNarrow, MaxInterrupt, DupDelivery, Crash, LateAck, Questions, FixWakeSkipsFailHold, StopCheckAnc, PartialCrash, QSplit, MaxFollowUp, UseNS, Subs, NoSub, MaxEp, NSMaxCrash, MaxDup, NSMaxInterrupt, MaxSessEnd, MaxInbox, Background, SilentKill, UseCT, Providers, NoProv, CanHandoff, CanInject, NativeFork, MainProviders, MaxMsg, MaxX, CTMaxCrash, MaxStale, Forks, Spawns, Cuts, MaxCutK, BudgetFail, InPlace, ToolNodes, UsePL, Devices, MaxOffline, Parents, NativeIds, RuntimeIds, NoRuntime, NoNative, ActivationIds, NoActivation, MaxReportRuns, MaxReportAttempts, MaxSourceRevision, OriginIds, NoOrigin, MaxDeliveryGeneration, Chats, Harnesses, Inputs, Requests, Pins, PacketIds, SessionIds, ReceiptIds, DocumentIds, MessageIds, NoMessage, NoRequest, NoPin, NoPacket, MaxCTRun, MaxCTAttempt, UseReporting, UseTransferReporting, Images, Actions, ReportingImages, ReportingActions, TransferImages, TransferActions
VARIABLE cursor
O == INSTANCE RuntimeOperationalCompositionSpec WITH
 CoreImage <- Images[cursor], ReportingImage <- ReportingImages[cursor], TransferImage <- TransferImages[cursor]
RP == INSTANCE RuntimeNativeReportingSpec WITH
 runs <- ReportingImages[cursor].runs,
  sources <- ReportingImages[cursor].sources,
  natives <- ReportingImages[cursor].natives,
  tasks <- ReportingImages[cursor].tasks,
  docTasks <- ReportingImages[cursor].docTasks,
  pending <- ReportingImages[cursor].pending,
  lastCause <- ReportingImages[cursor].lastCause,
  handles <- ReportingImages[cursor].handles,
  admitted <- ReportingImages[cursor].admitted,
  catalogComplete <- ReportingImages[cursor].catalogComplete
OperationalVars == <<Images[cursor],ReportingImages[cursor],TransferImages[cursor]>>
NativeLayerEnabled == O!NativeLayerEnabled
TransferLayerEnabled == O!TransferLayerEnabled
InitialImage == /\ O!Init
 /\ Len(Images)=Len(ReportingImages) /\ Len(Images)=Len(TransferImages)
 /\ Len(Actions)=Len(Images)-1 /\ Len(ReportingActions)=Len(Images)-1
 /\ Len(TransferActions)=Len(Images)-1
Init == cursor=1
Next == IF cursor<Len(Images) THEN cursor'=cursor+1 ELSE UNCHANGED cursor
Spec == Init /\ [][Next]_cursor
MappedAt == /\ (cursor=1 => InitialImage)
 /\ IF cursor<Len(Images) THEN ENABLED (cursor'=cursor+1 /\ [O!Next]_OperationalVars) ELSE TRUE
NamedAt == IF cursor<Len(Images)
 THEN ENABLED (cursor'=cursor+1 /\ O!NamedStep(Actions[cursor],ReportingActions[cursor],TransferActions[cursor])) ELSE TRUE
MappedSpec == []MappedAt
NamedPath == []NamedAt
MappedType == O!Safety
ReportingSafety == O!ReportingSafety
TransferSafety == O!TransferSafety
ReportingTransitionAt == IF cursor<Len(Images)
 THEN ENABLED (cursor'=cursor+1 /\ [RP!SafetyStep]_(RP!rv)) ELSE TRUE
ReportingTransitionSafety == []ReportingTransitionAt
OnlyRootEndsRun == O!OnlyRootEndsRun
OrphansClosed == O!OrphansClosed
NoFalseProcessDeath == O!NoFalseProcessDeath
LossClosesOnlySource == O!LossClosesOnlySource
BackgroundSurvivesTurn == O!BackgroundSurvivesTurn
AncestorTerminalClosesForeground == O!AncestorTerminalClosesForeground
ReportingFairClosure == O!ReportingFairClosure
FairReportingClosure == ReportingFairClosure
(* The component checker also consumes this exact action table. *)
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
AgentActionJoined(ev) == O!AgentActionJoined(ev)
====
