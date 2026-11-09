#!/usr/bin/env python3
"""Derive versioned specification sources; never modify checked v2 sources.

This stage adds three access levels and R41 attempt boundaries. It retains
full CT semantics, including its fail/anchor guards. No CT guard is relaxed
to fit runtime behavior. The runtime-to-full-spec map is a separate gate.
"""
from pathlib import Path
import argparse,hashlib,json
HERE=Path(__file__).resolve().parent
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument("--source-root",type=Path)
args=parser.parse_args()
SOURCE=args.source_root or next((p for p in (HERE.parent,HERE.parent/"tla") if (p/"Composition.tla").is_file()),None)
if SOURCE is None:parser.error("cannot find original models; use --source-root")
SOURCE=SOURCE.resolve()
names={'RunBase':'RuntimeSpecRunBase','RunLifecycle':'RuntimeRunLifecycleSpec',
 'Delegation':'RuntimeDelegationSpec','NativeSubagents':'RuntimeNativeSpec',
 'ContextTransfer':'RuntimeContextTransferSpec','Composition':'RuntimeCompositionSpec'}
manifest=[]
for old,new in names.items():
 source=(SOURCE/(old+'.tla')).read_text()
 text=source
                                                              
 import re
 for before,after in names.items():
  text=re.sub(r'(?<=MODULE )'+before+r'\b',after,text)
  text=re.sub(r'(?<=EXTENDS )'+before+r'\b',after,text)
  text=re.sub(r'(?<=INSTANCE )'+before+r'\b',after,text)
 if old=='Delegation':
  text=text.replace('UNION {{OfferE(p, r, cGen[p][r]) : r \\in {r3 \\in Ords : cSt[p][r3] = "offered"}} : p \\in Threads}', '(UNION {{OfferE(p, r, cGen[p][r]) : r \\in {r3 \\in Ords : cSt[p][r3] = "offered"}} : p \\in Threads})')
  text=text.replace('Levels   == 0..1','Levels   == 0..2')
  text=text.replace('IF t = Root THEN 1 ELSE 0','IF t = Root THEN 2 ELSE 0')
 if old=='NativeSubagents':
  action=r'''
(* R41: an attempt boundary keeps the root run ordinal and background work.
   It closes foreground records of the superseded attempt. Provider events
   for that old root cannot close the next attempt. *)
PAttemptFail ==
  /\ Room /\ proot
  /\ pst'=[s \in Subs |-> IF s \in FgRunning THEN "dead" ELSE pst[s]]
  /\ proot'=FALSE
  /\ Emit(Ev("rootEnd",NoSub,0,rord,FALSE,"interrupted"))
  /\ UNCHANGED <<engv,subv,sess,pep,prun,pbg,pprog,replay,budv>>

PObservedEnd(out) ==
 /\ Room /\ proot /\ out \in {"completed","failed","interrupted"}
 /\ (out="completed" => FgRunning={})
 /\ (out="interrupted" => intLeft>0)
 /\ pst'=[s \in Subs |-> IF out#"completed" /\ s \in FgRunning THEN "dead" ELSE pst[s]]
 /\ proot'=FALSE
 /\ intLeft'=IF out="interrupted" THEN intLeft-1 ELSE intLeft
 /\ Emit(Ev("rootEnd",NoSub,0,rord,FALSE,out))
 /\ UNCHANGED <<engv,subv,sess,pep,prun,pbg,pprog,replay,crashLeft,dupLeft,sessLeft>>

LogicalStart ==
  /\ ~recovering /\ rst="idle" /\ ~proot
  /\ rst'="live" /\ rord'=rord+1 /\ endBy'="none"
  /\ UNCHANGED <<recovering,subv,provv,inbox,replay,budv>>

ObserveStart ==
  /\ ~recovering /\ rst="live" /\ ~proot
  /\ sess'=TRUE /\ proot'=TRUE
  /\ UNCHANGED <<engv,subv,pst,pep,prun,pbg,pprog,inbox,replay,budv>>

AttemptBoundary ==
  /\ ~recovering /\ rst="live"
  /\ LET fg == {s \in Subs : est[s]=Active /\ ~ebg[s]} IN
       /\ CloseSet(fg,"interrupted","orphan")
  /\ proot'=FALSE
  /\ UNCHANGED <<sess,pst>>
  /\ inbox'=SelectSeq(inbox,LAMBDA e: ~(e.k="rootEnd" /\ e.r=rord))
  /\ UNCHANGED <<rst,rord,endBy,recovering,eep,erun,ebg,pep,prun,pbg,pprog,replay,budv>>

'''
  text=text.replace('=============================================================================',action+'=============================================================================')
 if old=='Composition':
  text=text.replace('InPlaceSet == InPlace \\cup (IF UseNS THEN {NsT} ELSE {})','InPlaceSet == InPlace')
  text=text.replace('NSStart(t) == IF IsNs(t) THEN NS!StartRun ELSE UNCHANGED nsv',
    'NSStart(t) == IF IsNs(t) THEN NS!LogicalStart ELSE UNCHANGED nsv\nNSAttemptBoundary(t) == IF IsNs(t) THEN NS!AttemptBoundary ELSE UNCHANGED nsv')
  text=text.replace('o \\in {"completed", "failed", "usage_limit"} /\\ NS!PRootEnd',
                    'o \\in {"completed", "failed", "usage_limit"} /\\ NS!PRootEnd\n       \\/ o \\in {"transient", "crash"} /\\ NS!PAttemptFail')
  text=text.replace('/\\ RL!RestartActive(t, m) /\\ On(t) /\\ ~IsNs(t)',
                    '/\\ RL!RestartActive(t, m) /\\ On(t)')
  text=text.replace('A_Restart(t, m) ==\n  /\\ RL!RestartActive(t, m) /\\ On(t)\n  /\\ AbandonWaits({t})\n  /\\ UNCHANGED <<dNoWait, ext, ghost, plv>>',
    'A_Restart(t, m) ==\n  /\\ RL!RestartActive(t, m) /\\ On(t)\n  /\\ AbandonWaits({t}) /\\ NSAttemptBoundary(t)\n  /\\ UNCHANGED <<dNoWait, ctv, ctTrk, ghost, plv>>')
  text=text.replace('A_RetryAttempt(t, e) ==\n  /\\ RL!RetryAttempt(t, e) /\\ On(t)\n  /\\ AbandonWaits({t})\n  /\\ UNCHANGED <<dNoWait, ext, ghost, plv>>',
    'A_RetryAttempt(t, e) ==\n  /\\ RL!RetryAttempt(t, e) /\\ On(t)\n  /\\ AbandonWaits({t}) /\\ NSAttemptBoundary(t)\n  /\\ UNCHANGED <<dNoWait, ctv, ctTrk, ghost, plv>>')
  text=text.replace('/\\ AbandonWaits({d})\n     ELSE UNCHANGED',
                    '/\\ AbandonWaits({d}) /\\ NSAttemptBoundary(d)\n     ELSE /\\ UNCHANGED nsv\n          /\\ UNCHANGED')
  text=text.replace('dNoWait, ext, ghostNoOwed, plv>>','dNoWait, ctv, ctTrk, ghostNoOwed, plv>>')
                                                                           
  start=text.index('NSProvEnd(t, o) ==')
  end=text.index('(* The run end handles NS',start)
  text=text[:start]+r"""NSRootOutcome(o) == IF o \in {"completed","interrupted"} THEN o ELSE "failed"
NSProvEnd(t,o) == IF IsNs(t) THEN NS!PObservedEnd(NSRootOutcome(o)) ELSE UNCHANGED nsv
NSRecordedEnd(t,o) == IF IsNs(t) THEN
 /\ ~nProot
 /\ \E e \in {nInbox[i]:i \in 1..Len(nInbox)} : e.k="rootEnd" /\ e.r=rord[t] /\ e.out=NSRootOutcome(o)
 /\ UNCHANGED nsv
 ELSE UNCHANGED nsv
"""+text[end:]
  text=text.replace(r'/\ NSProvEnd(t, o)'+'\n'+r'  /\ CTProvEnd(t, o)', r'/\ NSRecordedEnd(t, o)'+'\n'+r'  /\ CTProvEnd(t, o)')
                                                                       
  text=text.replace('AgentOK(t) == Live(t) /\\ ~emitted[t] /\\ On(t) /\\ TurnOn(t) /\\ t \\notin owed', 'AgentOK(t) == Live(t) /\\ ~emitted[t] /\\ On(t) /\\ t \\notin owed')
                                                                       
                                                                         
  text=text.replace('  /\\ o = "completed" /\\ ~M("C_completeWhileWaiting") => waitOn[t] = NoThread /\\ ~D!Blocked(t)\n','')
  text=text.replace('CASE out = "completed"   -> Dx!RootComplete(t)',
    'CASE out = "completed"   -> IF M("C_completeWhileWaiting") THEN Dx!EndRun(t,"completed") /\\ DEndUnch ELSE Dx!RootComplete(t)')
  text=text.replace('(* A completed root terminal needs D!RootComplete\'s guard (no open wait,',
    '(* R45: a root terminal can wait in pev while the wait fiber releases. *)\n(* A completed logical finish needs D!RootComplete\'s guard (no open wait,')
                                                             
  text=text.replace('attAcc,     \\* access in force when the current attempt started (D3)',
                    'attAcc, attAccAt, \\* observed access and its actual run/attempt key')
  text=text.replace('consumedBy, attAcc, ghost, plv>>','consumedBy, attAcc, attAccAt, ghost, plv>>')
  begin=text.index('GhostStep ==')
  stop=text.index("  /\\ consumedBy'",begin)
  text=text[:begin]+"GhostStep ==\n  /\\ UNCHANGED <<attAcc,attAccAt>>\n"+text[stop:]
  text=text.replace('/\\ owed = {} /\\ attAcc = [t \\in Threads |-> 0]',
    '/\\ owed = {} /\\ attAccAt = [t \\in Threads |-> [run |-> 0,attempt |-> 0]]\n  /\\ attAcc = [t \\in Threads |-> 0]')
  text=text.replace('/\\ owed \\subseteq Threads /\\ attAcc \\in [Threads -> D!Levels]',
    '/\\ attAccAt \\in [Threads -> [run:0..MaxRuns,attempt:0..RLMaxAtt]]\n  /\\ owed \\subseteq Threads /\\ attAcc \\in [Threads -> D!Levels]')
  text=text.replace('(Live(t) /\\ ~emitted[t] /\\ ~intReq[t] /\\ t \\notin owed) => attAcc[t] <= access[t]',
    '(Live(t) /\\ ~emitted[t] /\\ ~intReq[t] /\\ t \\notin owed /\\\n      attAccAt[t]=[run |-> rord[t],attempt |-> att[t]]) => attAcc[t] <= access[t]')
  observed=r"""
(* Actual raw root terminal arrives before its accepted engine callback. *)
C_ObserveProviderTerminal(t,o) ==
 /\ IsNs(t) /\ Live(t) /\ On(t) /\ NSProvEnd(t,o)
 /\ UNCHANGED <<coreVars,ctv,ctTrk,ghost,plv,attAcc,attAccAt,consumedBy>>

(* R46: an actual later provider stream event starts the physical root. *)
C_ObserveProviderStart(t) ==
 /\ IsNs(t) /\ Live(t) /\ On(t) /\ NS!ObserveStart
 /\ UNCHANGED <<coreVars,ctv,ctTrk,consumedBy,waitAtt,f9w,owed,plv,attAcc,attAccAt>>

(* R46: exact installed request access. This does not prove provider
   stream state or enforced access; provider I/O has a separate gate. *)
C_ObserveAccess(t,a) ==
 /\ Live(t) /\ On(t) /\ ~emitted[t] /\ a \in D!Levels /\ a<=access[t]
 /\ attAcc'=[attAcc EXCEPT ![t]=a]
 /\ attAccAt'=[attAccAt EXCEPT ![t]=[run |-> rord[t],attempt |-> att[t]]]
 /\ owed'=IF t \in InPlace THEN owed \ {t} ELSE owed
 /\ UNCHANGED nsv
 /\ UNCHANGED <<coreVars,ctv,ctTrk,consumedBy,waitAtt,f9w,plv>>

"""
  text=text.replace('\nStep ==\n','\n'+observed+'Step ==\n',1)
  text=text.replace('\\/ C_Steer(t) \\/ C_Stop(t)',
    '\\/ C_ObserveProviderStart(t)\n       \\/ (\\E a \\in D!Levels : C_ObserveAccess(t,a))\n       \\/ (\\E o \\in RL!Outcomes : C_ObserveProviderTerminal(t,o))\n       \\/ C_Steer(t) \\/ C_Stop(t)')
  text=text.replace('(* FINDINGS from composing',
    '(* VERSION NOTE: R41 attempt boundaries and 3 access levels are added.\n   Original findings below describe the checked v2 baseline and remain\n   source history; C11 no longer applies in this version. *)\n\n(* FINDINGS from composing')
 out=HERE/(new+'.tla');out.write_text(text)
 manifest.append(dict(source=old+'.tla',source_path=str(SOURCE/(old+'.tla')),generated=new+'.tla',
  source_sha256=hashlib.sha256(source.encode()).hexdigest(),generated_sha256=hashlib.sha256(text.encode()).hexdigest()))
(HERE/'full-spec-source-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps({'generated':list(names.values()),'runtime_refinement_proved':False,'ct_guards_changed':False}))
