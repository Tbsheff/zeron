#!/usr/bin/env python3
"""Check local exact steps and the strict partial global authority product.

Requires full device captures and captured wire-to-effect causal links. This
checks one finite DAG order. It does not prove all orders, old Composition
refinement, pending-child registry semantics, or complete CT operations.
Generated data and TLC output must remain in a private directory.
"""
import argparse
import json
from pathlib import Path
import shutil
from check_trace import tla
from check_runtime import MODULES, core_predicates
from merge_runtime import merge
from runtime_projection import project
from private_runner import run_tlc

HERE=Path(__file__).resolve().parent


def project_global(paths, *, require_transactions=False):
    dag=merge(paths,require_transactions=require_transactions);merged=dag['rows']
    wires={w['wire_id']:w for w in dag['wires']}
    available={};wire_prefix={}
    for node in dag['nodes']:
        ident=node['id']
        if ident[0]=='doc':
            available=dict(available);available[ident[1]]=wires[ident[1]]
        else:
            wire_prefix[(ident[1],ident[2])]={'$fn':list(sorted(available.items()))}
    devices={r['dev'] for r in merged}
    local={d:project([r for r in merged if r['dev']==d]) for d in devices}
    visibility={(v['dev'],v['seq']):v for v in dag['visibility']}
    indices={d:0 for d in devices}; latest={}; committed={}; steps=[]
    for raw in merged:
        dev=raw['dev']; row=dict(local[dev][indices[dev]]); indices[dev]+=1
        row['wire_commits']=wire_prefix[(dev,raw['seq'])]
        v=visibility[(dev,raw['seq'])]
        row['transaction']=dict(id=v['id'] or 'unknown',index=v['index'],size=v['size'],visible=v['visible'],known=dag['physical_transactions'])
        before=latest.get(dev,row)
        latest=dict(latest);latest[dev]=row
        if v['visible']:committed=dict(committed);committed[dev]=row
        committed_parts={'$fn':list(sorted(committed.items()))}
                                                                           
                                                                          
        output_row=dict(row,committed_partitions=committed_parts)
        steps.append({'before':before,'row':output_row,'parts':{'$fn':list(sorted(latest.items()))},
                      'committed_parts':committed_parts})
    return steps,len(devices)


def emit(paths,out):
    steps,device_count=project_global(paths)
    out.mkdir(parents=True,exist_ok=True)
    for name in MODULES+('RuntimeGlobalRefinement',):
        shutil.copy(HERE/(name+'.tla'),out)
    source='''---- MODULE RuntimeGlobalObserved ----
EXTENDS RuntimeRunDelegationSeams, RuntimeRemoteSeams, RuntimeCaptureFrames
G == INSTANCE RuntimeGlobalRefinement
CT == INSTANCE RuntimeTransferSeams
Rows == ROWS
VARIABLE cursor
CORE
LocalAt == LET x == Rows[cursor] a == x.before b == x.row IN
 IF b.action="Init" THEN CoreInitial(b.core) /\\ TxInitial(b.tx) /\\ CT!TransferInitial(b.transfer)
 ELSE /\\ TxStep(a.tx,b.tx,b.action,b.args)
      /\\ CoreStep(a,b)
      /\\ CT!TransferApply(a.transfer,b.transfer,b.transfer_event)
GlobalAt == IF cursor=1 THEN G!GlobalInitial(Rows[1].parts)
 ELSE G!GlobalStep(Rows[cursor-1].parts,Rows[cursor].parts,Rows[cursor].row)
Init == cursor=1
Next == IF cursor<Len(Rows) THEN cursor'=cursor+1 ELSE UNCHANGED cursor
Spec == Init /\\ [][Next]_cursor
GlobalSpec == []GlobalAt
====
'''
    source=source.replace('ROWS',tla(steps)).replace('CORE',core_predicates()).replace('=<<','= <<')
    (out/'RuntimeGlobalObserved.tla').write_text(source)
    (out/'RuntimeGlobalObserved.cfg').write_text('SPECIFICATION Spec\nINVARIANT LocalAt\nPROPERTY GlobalSpec\nCHECK_DEADLOCK TRUE\n')
    return len(steps),device_count


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('captures',nargs='+',type=Path);p.add_argument('--out',type=Path,required=True);p.add_argument('--jar',type=Path,required=True)
    a=p.parse_args()
    count,devices=emit(a.captures,a.out)
    metadata={'check':'runtime-global-safety-path','rows':count,'devices':devices,
              'full_old_composition_refinement':False,'full_ct_operations':False,
              'all_causal_orders':False,'pending_child_registry':False}
    print(json.dumps(metadata),flush=True)
    raise SystemExit(run_tlc(a.out,a.jar,'RuntimeGlobalObserved.tla','RuntimeGlobalObserved.cfg',metadata))

if __name__=='__main__':
    try:main()
    except (ValueError,KeyError,TypeError):
        print(json.dumps({'status':'fail','check':'runtime-global-safety-path','reason':'unsupported or incomplete capture schema'}))
        raise SystemExit(2)
