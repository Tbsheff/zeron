#!/usr/bin/env python3
"""Check every accepted remote reducer step in a complete raw capture.

This is an explicit component check. Other rows remain in the input and supply
immediate pre-state; this command does not claim full-runtime conformance.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
from check_trace import tla
from remote_projection import REMOTE_ACTIONS, project_remote, project_remote_event

HERE = Path(__file__).resolve().parent

def bindings(snapshot):
    ids = {}
    for row in snapshot['threads']:
        if row['local_thread'] in ids or row['chat'] == 'none' or row['chat'] in ids.values():
            raise ValueError('ambiguous stable thread binding')
        ids[row['local_thread']] = row['chat']
    def tid(key):
        if key is None:
            return 'none'
        if key not in ids:
            raise ValueError(f'missing stable thread binding {key}')
        return ids[key]
    return tid

def project_steps(rows):
    steps = []
    for i,row in enumerate(rows):
        if row['v'] != 2 or (i and (row['dev'] != rows[i-1]['dev'] or row['seq'] != rows[i-1]['seq']+1)):
            raise ValueError('requires contiguous single-device raw v2 capture')
        if not i or row['action'] != 'Apply':
            continue
        a = row['args']; ev = a['event']
        if ev['act'] not in REMOTE_ACTIONS:
            continue
        if not a['accepted'] or a['duplicate']:
            continue                                                                  
        before, after = rows[i-1]['snapshot'],row['snapshot']
        tid = bindings(after)
        rc = [r for r in after['receipts'] if r['key']==a['key']]
        if len(rc) != 1 or rc[0]['result']['ok'] != 'accepted':
            raise ValueError('missing accepted receipt')
        ids = rc[0]['result']['effects']
        effects = [next(x for x in after['outbox'] if x['effect_id']==key) for key in ids]
        if [x['idx'] for x in effects] != list(range(len(effects))):
            raise ValueError('receipt effect order differs from outbox index')
        event = project_remote_event(ev,tid,before_snapshot=before)
        from remote_projection import effect
        steps.append(dict(before=project_remote(before,tid),after=project_remote(after,tid),
                          event=event,fx=[effect(x['effect'],tid) for x in effects]))
    if not steps:
        raise ValueError('no accepted remote steps in capture')
    return steps

def emit(steps, out):
    out.mkdir(parents=True,exist_ok=True)
    for name in ('RuntimeState.tla','RuntimeRemoteSeams.tla','RuntimeNativeQuestionSeams.tla'):
        shutil.copy(HERE/name,out/name)
    (out/'RemoteObserved.tla').write_text('---- MODULE RemoteObserved ----\nEXTENDS RuntimeRemoteSeams,TLC\nRows == '+tla(steps)+'\nVARIABLE cursor\nInit == cursor=1\nNext == IF cursor<Len(Rows) THEN cursor\'=cursor+1 ELSE UNCHANGED cursor\nSpec == Init /\\ [][Next]_cursor\nStepOK == RemoteApply(Rows[cursor].before,Rows[cursor].after,Rows[cursor].event,Rows[cursor].fx)\n====\n')
    (out/'RemoteObserved.cfg').write_text('SPECIFICATION Spec\nINVARIANT StepOK\nCHECK_DEADLOCK TRUE\n')

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('capture',type=Path);p.add_argument('--out',type=Path,required=True);p.add_argument('--jar',type=Path,required=True)
    a=p.parse_args(); rows=[json.loads(x) for x in a.capture.read_text().splitlines() if x.strip()]
    steps=project_steps(rows);emit(steps,a.out)
    print(json.dumps({'component':'remote','steps':len(steps),'actions':sorted({x['event']['act'] for x in steps})}),flush=True)
    r=subprocess.run(['java','-XX:ActiveProcessorCount=4','-cp',str(a.jar.resolve()),'tlc2.TLC','-workers','1','-config','RemoteObserved.cfg','RemoteObserved.tla'],cwd=a.out,timeout=300)
    raise SystemExit(r.returncode)
if __name__=='__main__': main()
