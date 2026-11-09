#!/usr/bin/env python3
"""Check a complete single-device observed safety path in the new runtime model.

Checks full core state/effects, SQLite transaction frames, transfer checkpoint
frames and the independent RuntimeComposition safety product. This is not the
old Composition refinement, full CT operation conformance or a dogfood-week
coverage receipt. Unsupported actions and rejected core guards fail closed.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
from check_trace import tla
from runtime_projection import project
from private_runner import run_tlc

HERE=Path(__file__).resolve().parent
MODULES=('RuntimeState','RuntimeNativeQuestionSeams','RuntimeRemoteSeams',
         'RuntimeRunDelegationSeams','RuntimeCaptureFrames','RuntimeTransferSeams',
         'RuntimeComposition','RuntimeRefinement')


SOURCE=r'''---- MODULE RuntimeObserved ----
EXTENDS RuntimeRunDelegationSeams, RuntimeRemoteSeams, RuntimeCaptureFrames
ABS == INSTANCE RuntimeRefinement
CT == INSTANCE RuntimeTransferSeams
Rows == ROWS
VARIABLE cursor
CoreInitial(s) ==
 /\ DOMAIN s.threads={} /\ s.thread_order=<<>> /\ s.outbox={} /\ s.stop_receipts={}
 /\ s.stops=0 /\ DOMAIN s.questions={} /\ DOMAIN s.natives={}
 /\ s.question_receipts={} /\ s.native_order=<<>> /\ s.question_order=<<>>
TxInitial(s) == DOMAIN s.receipts={} /\ DOMAIN s.outbox={} /\ s.processed={}
 /\ (\A id \in DOMAIN s.facts : s.facts[id].kind="transfer_catalog")
 /\ DOMAIN s.command_receipts={} /\ DOMAIN s.command_decisions={}
AcceptedCore(a,b) ==
 CASE b.family="run" -> RunDelegationApply(a.core,b.core,b.event,b.fx)
 [] b.family="remote" -> RemoteApply(a.core,b.core,b.event,b.fx)
 [] b.family="native_question" -> NativeQuestionApply(a.core,b.core,b.event,b.fx)
 [] OTHER -> FALSE
CoreStep(a,b) ==
 IF b.action="Apply"
 THEN IF b.args.duplicate THEN a.core=b.core /\ b.fx=<<>>
      ELSE IF b.args.accepted THEN AcceptedCore(a,b)
           ELSE IF b.family="run" THEN a.core=b.core /\ b.fx=<<>> /\ RunDelegationReject(a.core,b.event,b.reject_reason)
                ELSE IF b.family="remote" THEN a.core=b.core /\ b.fx=<<>> /\ RemoteReject(a.core,b.event,b.reject_reason)
                ELSE IF b.family="native_question" THEN a.core=b.core /\ b.fx=<<>> /\ NativeQuestionReject(a.core,b.event,b.reject_reason)
                ELSE FALSE
 ELSE a.core=b.core /\ b.fx=<<>>
Initial == CoreInitial(Rows[1].core) /\ TxInitial(Rows[1].tx)
 /\ CT!TransferInitial(Rows[1].transfer)
StepOK == IF cursor=1 THEN Initial ELSE
 LET a == Rows[cursor-1] b == Rows[cursor] IN
 /\ TxStep(a.tx,b.tx,b.action,b.args)
 /\ CoreStep(a,b)
 /\ CT!TransferApply(a.transfer,b.transfer,b.transfer_event)
Init == cursor=1
Next == IF cursor<Len(Rows) THEN cursor'=cursor+1 ELSE UNCHANGED cursor
Spec == Init /\ [][Next]_cursor
CompositionAt == IF cursor=1 THEN ABS!CompositionInitial(Rows[1])
 ELSE ABS!CompositionStep(Rows[cursor-1],Rows[cursor])
CompositionSpec == []CompositionAt
====
'''

def core_predicates():
    return SOURCE.split("VARIABLE cursor\n",1)[1].split("Initial ==",1)[0]


def emit(rows,out):
    projected=project(rows)
    if rows[0]['action'] != 'Init':
        raise ValueError('whole safety path requires captured Init')
    out.mkdir(parents=True,exist_ok=True)
    for name in MODULES:
        shutil.copy(HERE/(name+'.tla'),out)

    (out/'RuntimeObserved.tla').write_text(SOURCE.replace('=<<', '= <<').replace('ROWS',tla(projected)))
    (out/'RuntimeObserved.cfg').write_text('SPECIFICATION Spec\nINVARIANT StepOK\nPROPERTY CompositionSpec\nCHECK_DEADLOCK TRUE\n')
    return projected


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('capture',type=Path);p.add_argument('--out',type=Path,required=True);p.add_argument('--jar',type=Path,required=True)
    a=p.parse_args(); rows=[json.loads(x) for x in a.capture.read_text().splitlines() if x.strip()]
    if len(rows)<2: raise ValueError('requires Init plus transition')
    projected=emit(rows,a.out)
    print(json.dumps({'check':'runtime-safety-path','rows':len(rows),
        'actions':sorted({r['event']['act'] for r in projected if r['action']=='Apply'}),
        'full_old_composition_refinement':False,'full_ct_operations':False}),flush=True)
    code=run_tlc(a.out,a.jar,'RuntimeObserved.tla','RuntimeObserved.cfg',{'check':'runtime-safety-path','rows':len(rows)})
    raise SystemExit(code)
if __name__=='__main__':
    try:main()
    except (ValueError,KeyError,TypeError):
        print(json.dumps({'status':'fail','check':'runtime-safety-path','reason':'unsupported or incomplete capture schema'}))
        raise SystemExit(2)
