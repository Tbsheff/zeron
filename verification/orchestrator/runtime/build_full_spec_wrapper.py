#!/usr/bin/env python3
"""Build strict full-spec interfaces. Inputs must come from the raw mapper.

A complete semantic state image is an interface, not evidence. Callers must
retain the raw capture-to-image map and reject missing physical observations.
"""
from pathlib import Path
import re,json
HERE=Path(__file__).resolve().parent
full=(HERE/'RuntimeCompositionSpec.tla').read_text();base=(HERE/'RuntimeSpecRunBase.tla').read_text()
def clean(s):return re.sub(r'\\\*[^\n]*','',s)
def names(s,key,end):
 b=clean(s).split(key,1)[1].split(end,1)[0]
 return re.findall(r'\b[A-Za-z][A-Za-z0-9_]*\b',b)
const=names(base,'CONSTANTS','ASSUME')+names(full,'CONSTANTS','ASSUME')
var=names(base,'VARIABLES','base ==')+names(full,'VARIABLES','---')
assert len(var)==len(set(var))
constants=', '.join(dict.fromkeys(const))
subs=',\n  '.join(f'{v} <- Images[cursor].{v}' for v in var)
actions=[]
for name,args in re.findall(r'^(C_[A-Za-z0-9_]+)(?:\(([^)]*)\))?\s*==',full,re.M):
 if name in {'C_Done','C_Quiescent','C_TypeOK','C_BoundsNotHit','C_RetryBudget','C_EnforcedAccess','C_WaitInLiveRun','C_NativeInRun','C_EndNotStuck','C_StartNotStuck','C_S2','C_StopHoldOnlyPerson','C_NoPhantomAck','C_ConsumeAtRunStart','C_R7','C_NoLostSteer','C_D6','C_S11','C_HostFixed'}:continue
 actual=','.join('a.'+x.strip() for x in args.split(',') if x.strip())
 actions.append((name,f'Full!{name}({actual})' if args else f'Full!{name}'))
cases='\n    [] '.join(f'ev.label="{label}" -> {expr}' for label,expr in actions)
observed=f'''---------------------- MODULE RuntimeFullObserved ----------------------
(* Strict source-step interface. Images MUST be produced by the raw mapper.
   This module never searches for a model path to match an observed hash.
   Each finite macro step must be expanded to named source actions first. *)
EXTENDS Naturals, Sequences, TLC
CONSTANTS {constants}, Images, Actions
VARIABLE cursor
Full == INSTANCE RuntimeCompositionSpec WITH
  {subs}
Init == cursor=1
Next == IF cursor<Len(Images) THEN cursor'=cursor+1 ELSE UNCHANGED cursor
Spec == Init /\\ [][Next]_cursor
NamedSourceAction(ev) == LET a == ev.args IN
 CASE {cases}
    [] OTHER -> FALSE
NamedSourceStep == IF cursor'=cursor THEN UNCHANGED Full!vars
                   ELSE NamedSourceAction(Actions[cursor])
NamedPath == [][NamedSourceStep]_cursor
MappedSpec == Full!Init /\\ [][Full!Next]_(Full!vars)
MappedType == Full!C_TypeOK
====
'''
(HERE/'RuntimeFullObserved.tla').write_text(observed)
forestconst=', '.join(x for x in dict.fromkeys(const) if x not in {'Threads','Root','NsT','RootDev'})
fsubs=',\n  '.join(f'{v} <- Image[<<root,focus>>].{v}' for v in var)
ns=names(full,'nsv    == <<','>>')
shared=[v for v in var if v not in ns]
fields=', '.join('"'+v+'"' for v in shared)
forest=f'''-------------------- MODULE RuntimeForestCompositionSpec --------------------
(* A product of full Composition instances, one per root and native parent.
   Shared RL/D/CT/placement fields agree within each root. Each NS focus has
   its own native state. Actual raw mapping and cross-root CT checks remain
   required. Empty/missing native focus sets are not allowed. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS Roots, TreeThreads, RootDevices, {forestconst}
VARIABLE Image
ActiveRoots == {{root \\in Roots : <<root,root>> \\in DOMAIN Image}}
Indexes == UNION {{{{<<root,t>>:t \\in TreeThreads[root]}}:root \\in ActiveRoots}}
Full(root,focus) == INSTANCE RuntimeCompositionSpec WITH
  Threads <- TreeThreads[root], Root <- root, NsT <- focus, RootDev <- RootDevices[root],
  {fsubs}
SharedFields == {{{fields}}}
SharedInRoot == \\A root \\in ActiveRoots :
 \\A f,g \\in TreeThreads[root] : \\A field \\in SharedFields :
  Image[<<root,f>>][field]=Image[<<root,g>>][field]
ForestInit ==
 /\\ DOMAIN Image=Indexes /\\ SharedInRoot
 /\\ \\A root \\in Roots : root \\in TreeThreads[root]
 /\\ \\A i \\in Indexes : Full(i[1],i[2])!Init
ForestNext ==
 /\\ DOMAIN Image'=Indexes' /\\ SharedInRoot'
 /\\ ActiveRoots \\subseteq ActiveRoots'
 /\\ \\A i \\in Indexes : [Full(i[1],i[2])!Next]_(Full(i[1],i[2])!vars)
 /\\ \\A i \\in Indexes' \\ Indexes : Full(i[1],i[2])!Init' 
ForestSpec == ForestInit /\\ [][ForestNext]_Image
ForestType == SharedInRoot /\\ \\A i \\in Indexes : Full(i[1],i[2])!C_TypeOK
====
'''
(HERE/'RuntimeForestCompositionSpec.tla').write_text(forest)
                                                                                
f_cases=cases.replace('Full!', 'F!Full(root,focus)!')
forest_observed=f'''---------------- MODULE RuntimeFullForestObserved ----------------
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS Roots, TreeThreads, RootDevices, {forestconst}, Images, Actions
VARIABLE cursor
F == INSTANCE RuntimeForestCompositionSpec WITH Image <- Images[cursor]
Init == cursor=1
Next == IF cursor<Len(Images) THEN cursor'=cursor+1 ELSE UNCHANGED cursor
Spec == Init /\\ [][Next]_cursor
NamedSourceAction(root,focus,ev) == LET a == ev.args IN
 CASE ev.label="Stutter" -> UNCHANGED F!Full(root,focus)!vars
    [] {f_cases}
    [] OTHER -> FALSE
NamedSourceStep == IF cursor'=cursor THEN UNCHANGED Images[cursor]
 ELSE /\\ \\A i \\in F!Indexes : NamedSourceAction(i[1],i[2],Actions[cursor][i])
      /\\ \\A i \\in F!Indexes' \\ F!Indexes :
           Actions[cursor][i].label="RegisterRoot" /\\ F!Full(i[1],i[2])!Init' 
NamedPath == [][NamedSourceStep]_cursor
MappedSpec == F!ForestInit /\\ [][F!ForestNext]_(Images[cursor])
MappedType == F!ForestType
====
'''
(HERE/'RuntimeFullForestObserved.tla').write_text(forest_observed)
(HERE/'full-spec-interface.json').write_text(json.dumps(dict(constants=const,variables=var,native_variables=ns,named_actions=[x[0] for x in actions],runtime_mapping_ready=False),indent=2)+'\n')
print(json.dumps({'semantic_fields':len(var),'named_actions':len(actions),'runtime_mapping_ready':False}))
