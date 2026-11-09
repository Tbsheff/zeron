#!/usr/bin/env python3
"""Build the R44 operational product interface; never fill state images."""
from pathlib import Path
import json
import re

HERE = Path(__file__).resolve().parent
forest = (HERE / 'RuntimeFullForestObserved.tla').read_text()
reporting = (HERE / 'RuntimeNativeReportingSpec.tla').read_text()

def names(source, begin, end):
    section = source.split(begin, 1)[1].split(end, 1)[0]
    section = re.sub(r'\\\*[^\n]*', '', section)
    return re.findall(r'\b[A-Za-z][A-Za-z0-9_]*\b', section)

base_constants = names(forest, 'CONSTANTS', 'VARIABLE')
report_constants = names(reporting, 'CONSTANTS', 'VARIABLES')
report_variables = names(reporting, 'VARIABLES', 'rv ==')
constants = list(dict.fromkeys(base_constants + report_constants +
                               ['ReportingImages', 'ReportingActions', 'UseReporting']))
source_actions = forest.split('NamedSourceAction(root,focus,ev) ==', 1)[1].split('NamedSourceStep ==', 1)[0]
next_body = reporting.split('\nNext ==', 1)[1].split('\nSpec ==', 1)[0]
next_calls = set(re.findall(r'\b([A-Z][A-Za-z0-9_]*)\s*\(', next_body))
report_actions = []
for name, args in re.findall(r'^([A-Z][A-Za-z0-9_]*)(?:\(([^)]*)\))?\s*==', reporting, re.M):
    if name in next_calls and name not in {'DocStatus'}:
        arguments = ','.join('a.' + arg.strip() for arg in args.split(',') if arg.strip())
        report_actions.append((name, f'RP!{name}({arguments})'))
report_cases = '\n [] '.join(f'ev.label="{name}" -> {call}' for name, call in report_actions)
report_substitutions = ',\n  '.join(f'{name} <- ReportingImages[cursor].{name}' for name in report_variables)

text = r'''---------------- MODULE RuntimeOperationalFullObserved ----------------
(* R44 replaces only the old native PROCESS model in this operational gate.
   RL/D/CT/placement still use the versioned full source forest. Reporting
   streams, immutable runtime ownership, task activation, issued request access,
   and actual attempt admission use RuntimeNativeReportingSpec. EOF does not
   prove process death. The old process theorem remains a separate gate.

   Images and ReportingImages are source-mapped states, never predicted states.
   Each adjacent image pair has one explicit core action and one explicit
   reporting action (either can stutter). Expand macros into image steps first.
   A finite prefix can check closure only for obligations reached in that prefix;
   the free implementation model must prove general fair closure separately. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS __CONSTANTS__
VARIABLE cursor
F == INSTANCE RuntimeForestCompositionSpec WITH Image <- Images[cursor]
RP == INSTANCE RuntimeNativeReportingSpec WITH
  __REPORT_SUBS__
OperationalVars == <<Images[cursor],ReportingImages[cursor]>>
NativeLayerEnabled == UseReporting /\ ~UseNS
ParentRoots(p) == {root \in Roots:p \in TreeThreads[root]}
ParentRoot(p) == CHOOSE root \in ParentRoots(p):TRUE
ParentPresent(p) == IF Cardinality(ParentRoots(p))#1 THEN FALSE
 ELSE IF ParentRoot(p) \notin F!ActiveRoots THEN FALSE
 ELSE Images[cursor][<<ParentRoot(p),ParentRoot(p)>>].created[p]
Core(p) == Images[cursor][<<ParentRoot(p),ParentRoot(p)>>]
LogicalJoin ==
 /\ Parents=UNION {TreeThreads[root]:root \in Roots}
 /\ \A p \in Parents:Cardinality(ParentRoots(p))=1
 /\ \A p \in Parents:
      IF ParentPresent(p) THEN
       /\ RP!runs[p].run=Core(p).rord[p]
       /\ RP!runs[p].attempt=Core(p).att[p]
       /\ RP!runs[p].active=(Core(p).liveSet[p]#{})
      ELSE RP!runs[p].run=0 /\ RP!runs[p].attempt=0 /\ ~RP!runs[p].active
CurrentInstalled(p) ==
 IF ~ParentPresent(p) THEN FALSE
 ELSE IF p \notin DOMAIN RP!handles THEN FALSE
 ELSE IF RP!handles[p] \notin DOMAIN RP!sources THEN FALSE
 ELSE LET sid==RP!handles[p] s==RP!sources[sid] IN
   RP!runs[p].active /\ ~s.retired /\ s.parent=p /\
   s.run=RP!runs[p].run /\ s.attempt=RP!runs[p].attempt
EnforcedRequest(p) == ReportingImages[cursor].runs[p].active /\
 ~Core(p).emitted[p] /\ ~Core(p).intReq[p] /\ p \notin Core(p).owed
IssuedAccessJoin == \A p \in Parents:
 IF CurrentInstalled(p) THEN LET s==RP!sources[RP!handles[p]] IN
   /\ s.issuedAccess \in 0..2
   /\ (EnforcedRequest(p) => s.issuedAccess<=Core(p).access[p])
   /\ Core(p).attAcc[p]=s.issuedAccess
   /\ Core(p).attAccAt[p]=[run |-> s.run,attempt |-> s.attempt]
 ELSE TRUE
(* Provider input uses the current issued handle. This joins current state;
   shared pending inputs across steering do not pin the request origin attempt. *)
IssuedSourceCurrent(p) ==
 IF ~CurrentInstalled(p) THEN FALSE
 ELSE ReportingImages[cursor].sources[ReportingImages[cursor].handles[p]].requestIssued
AdmissionCurrent(p) ==
 IF ~CurrentInstalled(p) THEN FALSE
 ELSE IF p \notin DOMAIN RP!admitted THEN FALSE
 ELSE IF ~RP!admitted[p].present THEN FALSE
 ELSE LET a==RP!admitted[p] sid==RP!handles[p] s==RP!sources[sid] IN
   /\ s.requestIssued
   /\ a.source=sid /\ a.run=s.run /\ a.attempt=s.attempt
   /\ a.revision=s.revision /\ a.issuedAccess=s.issuedAccess
   /\ s.issuedAccess \in 0..2 /\ s.issuedAccess<=Core(p).access[p]
NativeIdentityJoin ==
 /\ \A k \in NativeIds:Len(k)=2 /\ k[1] \in Parents
 /\ \A k \in DOMAIN RP!natives:
     /\ RP!natives[k].parent=k[1]
     /\ RP!natives[k].source \in DOMAIN RP!sources
     /\ RP!sources[RP!natives[k].source].parent=k[1]
     /\ RP!natives[k].activation \in ActivationIds\cup{NoActivation}
 /\ \A k \in DOMAIN RP!tasks:
     /\ RP!tasks[k].parent=k[1]
     /\ RP!tasks[k].source=RP!natives[k].source
     /\ RP!tasks[k].run=RP!natives[k].run
     /\ RP!tasks[k].attempt=RP!natives[k].attempt
     /\ RP!tasks[k].activation=RP!natives[k].activation
     /\ RP!tasks[k].background=RP!natives[k].background
 /\ \A k \in DOMAIN RP!docTasks:RP!docTasks[k].source=RP!tasks[k].source
JoinedIdentities == LogicalJoin /\ IssuedAccessJoin /\ NativeIdentityJoin
SourceOwnershipStable ==
 /\ DOMAIN RP!sources \subseteq DOMAIN RP!sources'
 /\ \A sid \in DOMAIN RP!sources:RP!sources[sid].parent=RP!sources'[sid].parent
NamedSourceAction(root,focus,ev) ==__SOURCE_ACTIONS__
NamedReportingAction(ev) == LET a==ev.args IN
 CASE ev.label="Stutter" -> UNCHANGED RP!rv
 [] __REPORT_CASES__
 [] OTHER -> FALSE
AgentActionJoined(ev) == LET a==ev.args IN
 CASE ev.label \in {"C_Delegate","C_FollowUp"} -> AdmissionCurrent(a.t)
 [] ev.label="C_ReadResult" -> AdmissionCurrent(a.p)
 [] ev.label \in {"C_RaiseQuestion","C_RaiseQuestionS"} -> IssuedSourceCurrent(a.c)
 [] ev.label \in {"C_AgentAnswer","C_AgentAnswerS"} -> AdmissionCurrent(a.x)
 [] OTHER -> TRUE
AgentStepsJoined == \A i \in F!Indexes:AgentActionJoined(Actions[cursor][i])
NamedSourceStep ==
 /\ \A i \in F!Indexes:
      /\ NamedSourceAction(i[1],i[2],Actions[cursor][i])
      /\ AgentActionJoined(Actions[cursor][i])
 /\ \A i \in F!Indexes'\F!Indexes:
      Actions[cursor][i].label="RegisterRoot" /\ F!Full(i[1],i[2])!Init'
NamedStep == IF cursor'=cursor THEN UNCHANGED OperationalVars
 ELSE /\ NamedSourceStep /\ NamedReportingAction(ReportingActions[cursor])
      /\ SourceOwnershipStable /\ JoinedIdentities'
Init == cursor=1
Next == IF cursor<Len(Images) THEN cursor'=cursor+1 ELSE UNCHANGED cursor
Spec == Init /\ [][Next]_cursor
MappedStep == [F!ForestNext]_(Images[cursor]) /\ [RP!Next]_(RP!rv)
              /\ [RP!SafetyStep]_(RP!rv)
              /\ AgentStepsJoined /\ SourceOwnershipStable /\ JoinedIdentities'
InitialImage == NativeLayerEnabled /\ Len(Images)=Len(ReportingImages)
                 /\ Len(Actions)=Len(Images)-1 /\ Len(ReportingActions)=Len(Images)-1
                 /\ F!ForestInit /\ RP!Init /\ JoinedIdentities
(* Fix the next cursor before ENABLED checks this exact image pair.
   ENABLED does not choose an abstract successor. *)
MappedAt == /\ (cursor=1 => InitialImage)
            /\ IF cursor<Len(Images) THEN ENABLED (cursor'=cursor+1 /\ MappedStep) ELSE TRUE
NamedAt == IF cursor<Len(Images) THEN ENABLED (cursor'=cursor+1 /\ NamedStep) ELSE TRUE
MappedSpec == []MappedAt
NamedPath == []NamedAt
MappedType == NativeLayerEnabled /\ F!ForestType /\ RP!Safety /\ JoinedIdentities
ReportingSafety == RP!Safety
ReportingTransitionAt == IF cursor<Len(Images)
 THEN ENABLED (cursor'=cursor+1 /\ [RP!SafetyStep]_(RP!rv)) ELSE TRUE
ReportingTransitionSafety == []ReportingTransitionAt
OnlyRootEndsRun == RP!OnlyRootEndsRun
OrphansClosed == RP!OrphansClosed
NoFalseProcessDeath == RP!NoFalseProcessDeath
LossClosesOnlySource == RP!LossClosesOnlySource
BackgroundSurvivesTurn == RP!BackgroundSurvivesTurn
ReportingLossPending(k) == IF k \notin DOMAIN RP!tasks THEN FALSE
 ELSE IF RP!tasks[k].status#"running" THEN FALSE
 ELSE RP!sources[RP!tasks[k].source].reporting="disconnected"
ReportingClosed(k) == IF k \notin DOMAIN RP!tasks THEN TRUE ELSE RP!tasks[k].status#"running"
ReportingFairClosure == \A k \in NativeIds:ReportingLossPending(k) ~> ReportingClosed(k)
FairReportingClosure == ReportingFairClosure
====
'''
text = (text.replace('__CONSTANTS__', ', '.join(constants))
        .replace('__REPORT_SUBS__', report_substitutions)
        .replace('__SOURCE_ACTIONS__', source_actions)
        .replace('__REPORT_CASES__', report_cases))
for name in report_variables:
    text=re.sub(r'RP!'+name+r'\b','ReportingImages[cursor].'+name,text)

                                                                        
                                                                               
transfer = (HERE / 'RuntimeTransferReportingSpec.tla').read_text()
core = (HERE / 'RuntimeCompositionSpec.tla').read_text()
forest_model = (HERE / 'RuntimeForestCompositionSpec.tla').read_text()
transfer_constants = names(transfer, 'CONSTANTS', 'VARIABLES')
transfer_variables = names(transfer, 'VARIABLES', 'ctVars ==')
all_target_constants = list(dict.fromkeys(
    [c for c in base_constants if c not in {'Images', 'Actions'}]
    + report_constants + transfer_constants + ['UseReporting', 'UseTransferReporting']))

def action_cases(module, content, next_start, next_end, ignored=()):
    body = content.split(next_start, 1)[1].split(next_end, 1)[0]
    calls = set(re.findall(r'\b([A-Z][A-Za-z0-9_]*)\s*\(', body))
    result = []
    for name, args in re.findall(r'^([A-Z][A-Za-z0-9_]*)(?:\(([^)]*)\))?\s*==', content, re.M):
        if name in calls and name not in ignored:
            arguments = ','.join('a.' + arg.strip() for arg in args.split(',') if arg.strip())
            result.append((name, f'{module}!{name}({arguments})'))
    return '\n [] '.join(f'ev.label="{name}" -> {call}' for name, call in result), result

transfer_cases, transfer_actions = action_cases('TR', transfer, '\nNext ==', '\nSpec ==')
core_step = core.split('\nStep ==', 1)[1].split('\n\n(* Coverage only', 1)[0].rstrip()
for action, index, guard in [('C_Delegate',0,'currentAdmission'), ('C_FollowUp',0,'currentAdmission'),
                              ('C_ReadResult',0,'currentAdmission'), ('C_AgentAnswer',1,'currentAdmission'),
                              ('C_AgentAnswerS',1,'currentAdmission'), ('C_RaiseQuestion',0,'currentIssued'),
                              ('C_RaiseQuestionS',0,'currentIssued')]:
    def guarded(match, index=index, guard=guard):
        args = match.group(1).split(',')
        return '(' + guard + '[' + args[index].strip() + '] /\\ ' + match.group(0) + ')'
    core_step = re.sub(r'\b' + action + r'\(([^)]*)\)', guarded, core_step)
core_helper = '''---------------- MODULE RuntimeOperationalCoreSpec ----------------
(* This is a target contract. The implementation enumerates its own steps.
   The exact source Step is retained, with admitted caller guards on agent
   actions and issued-source guards on provider input. R44/R48 use independent
   reporting and retained-request products; old native and CT slots stay Init. *)
EXTENDS RuntimeCompositionSpec
VARIABLES currentAdmission, currentIssued
GuardedStep ==
''' + core_step + '\nGuardedNext == GuardedStep \\/ C_Done\n====\n'

                                                                        
core_instance = forest_model.split('Full(root,focus) ==', 1)[1].split('\nSharedFields ==', 1)[0]
core_instance = core_instance.replace('INSTANCE RuntimeCompositionSpec', 'INSTANCE RuntimeOperationalCoreSpec')
core_instance = core_instance.replace('Image[', 'CoreImage[')
core_instance += ',\n  currentAdmission <- [p \\in Parents |-> AdmissionCurrent(p)], currentIssued <- [p \\in Parents |-> IssuedSourceCurrent(p)]'
joins = text.split('ParentRoots(p) ==', 1)[1].split('NamedSourceAction(root,focus,ev) ==', 1)[0]
joins = 'ParentRoots(p) ==' + joins
joins = joins.replace('Images[cursor]', 'CoreImage').replace('ReportingCoreImage', 'ReportingImage')
                                                                             
joins = joins.replace('ReportingImages[cursor]', 'ReportingImage')
joins = joins.replace('(* Provider input uses the current issued handle. This joins current state;\n   shared pending inputs across steering do not pin the request origin attempt. *)',
 '''(* Provider input uses the current issued source. The source hook must pin
   the request issue attempt; an installed current handle alone does not prove
   that a delayed question belongs to that attempt. No MCP receipt is invented. *)''')
report_target_subs = ',\n  '.join(f'{v} <- ReportingImage.{v}' for v in report_variables)
transfer_target_subs = ',\n  '.join(f'{v} <- TransferImage.{v}' for v in transfer_variables)
legacy_frames = r'''
LegacyInitialFrames == \A i \in F!Indexes:
 /\ F!Full(i[1],i[2])!NS!Init /\ F!Full(i[1],i[2])!CT!Init
 /\ CoreImage[i].ctTrk=[t \in TreeThreads[i[1]] |-> FALSE]
 /\ CoreImage[i].consumedBy=[x \in F!Full(i[1],i[2])!CT!XIds |-> "none"]
'''
                                                                               
                                                                            
transfer_joins = r'''
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
'''
common = '''---------------- MODULE RuntimeOperationalCompositionSpec ----------------
(* R44/R48 operational replacement, separate from unchanged process-native and
   single-current-request CT theorems. CoreImage retains the original 105-field
   forest, with native/CT fields fixed at their declared Init values. Reporting
   and retained request maps are mandatory state products, not metadata flags.
   EOF proves loss of reporting only. A saved result can confirm an old request
   without modifying the current request. Implementations must enumerate their
   own successors and check this target as a property. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS ''' + ', '.join(all_target_constants) + '''
VARIABLES CoreImage, ReportingImage, TransferImage
F == INSTANCE RuntimeForestCompositionSpec WITH Image <- CoreImage
RP == INSTANCE RuntimeNativeReportingSpec WITH
  ''' + report_target_subs + '''
TR == INSTANCE RuntimeTransferReportingSpec WITH
  ''' + transfer_target_subs + '''
vars == <<CoreImage,ReportingImage,TransferImage>>
NativeLayerEnabled == UseReporting /\\ ~UseNS
TransferLayerEnabled == UseTransferReporting /\\ ~UseCT
ProductsEnabled == NativeLayerEnabled /\\ TransferLayerEnabled
''' + joins + legacy_frames + transfer_joins + '''
OC(root,focus) ==''' + core_instance + '''
CoreNext == F!ForestNext /\\
 \\A i \\in F!Indexes:[OC(i[1],i[2])!GuardedNext]_(F!Full(i[1],i[2])!vars)
NamedSourceAction(root,focus,ev) ==''' + source_actions + '''
NamedReportingAction(ev) == LET a==ev.args IN
 CASE ev.label="Stutter" -> UNCHANGED RP!rv
 [] ''' + report_cases + '''
 [] OTHER -> FALSE
NamedTransferAction(ev) == LET a==ev.args IN
 CASE ev.label="Stutter" -> UNCHANGED TR!ctVars
 [] ''' + transfer_cases + '''
 [] OTHER -> FALSE
''' + text.split('AgentActionJoined(ev) ==',1)[1].split('AgentStepsJoined ==',1)[0].join(['AgentActionJoined(ev) ==','']) + r'''
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
'''
                                                                               
for v in report_variables:
    common = re.sub(r'RP!' + v + r'\b', 'ReportingImage.' + v, common)
for v in transfer_variables:
    common = re.sub(r'TR!' + v + r'\b', 'TransferImage.' + v, common)
observed_constants = all_target_constants + ['Images','Actions','ReportingImages','ReportingActions','TransferImages','TransferActions']
observed = '''---------------- MODULE RuntimeOperationalFullObserved ----------------
(* Exact finite path over the reusable three-product operational target.
   The source cursor is unconstrained by target guards. Each property checks
   the given adjacent pair, including every declared macro intermediate.
   Finite replay does not establish general implementation fair closure. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS ''' + ', '.join(observed_constants) + '''
VARIABLE cursor
O == INSTANCE RuntimeOperationalCompositionSpec WITH
 CoreImage <- Images[cursor], ReportingImage <- ReportingImages[cursor], TransferImage <- TransferImages[cursor]
RP == INSTANCE RuntimeNativeReportingSpec WITH
 ''' + report_substitutions + '''
OperationalVars == <<Images[cursor],ReportingImages[cursor],TransferImages[cursor]>>
NativeLayerEnabled == O!NativeLayerEnabled
TransferLayerEnabled == O!TransferLayerEnabled
InitialImage == /\\ O!Init
 /\\ Len(Images)=Len(ReportingImages) /\\ Len(Images)=Len(TransferImages)
 /\\ Len(Actions)=Len(Images)-1 /\\ Len(ReportingActions)=Len(Images)-1
 /\\ Len(TransferActions)=Len(Images)-1
Init == cursor=1
Next == IF cursor<Len(Images) THEN cursor'=cursor+1 ELSE UNCHANGED cursor
Spec == Init /\\ [][Next]_cursor
MappedAt == /\\ (cursor=1 => InitialImage)
 /\\ IF cursor<Len(Images) THEN ENABLED (cursor'=cursor+1 /\\ [O!Next]_OperationalVars) ELSE TRUE
NamedAt == IF cursor<Len(Images)
 THEN ENABLED (cursor'=cursor+1 /\\ O!NamedStep(Actions[cursor],ReportingActions[cursor],TransferActions[cursor])) ELSE TRUE
MappedSpec == []MappedAt
NamedPath == []NamedAt
MappedType == O!Safety
ReportingSafety == O!ReportingSafety
TransferSafety == O!TransferSafety
ReportingTransitionAt == IF cursor<Len(Images)
 THEN ENABLED (cursor'=cursor+1 /\\ [RP!SafetyStep]_(RP!rv)) ELSE TRUE
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
 [] ''' + report_cases + '''
 [] OTHER -> FALSE
AgentActionJoined(ev) == O!AgentActionJoined(ev)
====
'''
for v in report_variables:
    observed = re.sub(r'RP!' + v + r'\b', 'ReportingImages[cursor].' + v, observed)
(HERE / 'RuntimeOperationalCoreSpec.tla').write_text(core_helper)
(HERE / 'RuntimeOperationalCompositionSpec.tla').write_text(common)
(HERE / 'RuntimeOperationalFullObserved.tla').write_text(observed)
print(json.dumps(dict(common_module='RuntimeOperationalCompositionSpec',
                      transfer_variables=transfer_variables,
                      transfer_actions=[name for name,_ in transfer_actions],
                      runtime_refinement_proved=False)))
