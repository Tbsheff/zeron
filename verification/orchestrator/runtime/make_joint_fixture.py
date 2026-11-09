"""Build synthetic raw-schema fixtures; never write on import."""
from copy import deepcopy
import argparse
import json
from pathlib import Path


def build():
    cfg=dict(max_depth=2,max_fanout=2,max_queue=4,max_retry=2,run_window=8)
    empty=dict(config=cfg,threads=[],outbox=[],stop_receipts=[],stops=0,questions=[],question_receipts=[],natives=[])
    snap=dict(state=empty,receipts=[],outbox=[],processed=[],threads=[],facts=[])
    rows=[dict(v=2,dev='A',seq=0,action='Init',args={},origin=dict(kind='sqlite',commit='sqlite:A:0',parents=[]),snapshot=snap)]
    
    def apply(key,event,change,effects=()):
        row=deepcopy(rows[-1]);seq=len(rows)
        row.update(seq=seq,action='Apply',args=dict(key=key,command_id=None,event=event,accepted=True,duplicate=False),origin=dict(kind='sqlite',commit=f'sqlite:A:{seq}',parents=[f'sqlite:A:{seq-1}']))
        change(row['snapshot'])
        ids=[f'{key}#{i}' for i in range(len(effects))]
        row['snapshot']['receipts'].append(dict(key=key,seq=seq,result=dict(ok='accepted',seq=seq,effects=ids)))
        row['snapshot']['outbox'].extend(dict(effect_id=fxid,seq=seq,idx=i,effect=fx,done=False) for i,(fxid,fx) in enumerate(zip(ids,effects)))
        rows.append(row)
    
    def opened(s):
        s['threads']=[dict(local_thread=0,chat='root',host=None,shadow=None,host_known=False)]
        s['state']['threads']=[[0,dict(id=0,parent=None,depth=0,access=2,reconfigure=False,queue=[],held='None',run_base=0,runs=[],int_req=False,attempt=dict(n=0,reason='Initial',emitted=False,kid_open=False,retries=0,steers=0),inbox=[],ul=dict(armed=None,due=False),wait_on=None,late_wait=None,task=None)]]
    apply('open',dict(act='OpenRoot',args=dict(t=0,access=2,reconfigure=False)),opened)
    apply('person',dict(act='PersonSend',args=dict(t=0,mode='Queue',msg=1)),lambda s:s['state']['threads'][0][1]['queue'].append({'Person':1}))
    def started(s):
        th=s['state']['threads'][0][1];th['queue']=[];th['attempt']['n']=1
        th['runs']=[dict(start={'Queued':{'Person':1}},status='Live',cohort=dict(disp='Open',st='None',del_=[],generation=0))]
        th['runs'][0]['cohort']['del']=th['runs'][0]['cohort'].pop('del_')
    apply('start',dict(act='StartFromQueue',args=dict(t=0)),started,[{'StartRun':dict(run=dict(thread=0,ord=1),start={'Queued':{'Person':1}},access=2,delivered=[])}])
    return rows


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--out',required=True,type=Path)
    out=p.parse_args().out;out.mkdir(parents=True,exist_ok=True)
    rows=build()
    (out/'joint-synthetic-valid.jsonl').write_text(''.join(json.dumps(r)+'\n' for r in rows))
    wrong=deepcopy(rows);wrong[-1]['snapshot']['outbox'][0]['effect']['StartRun']['access']=1
    (out/'joint-synthetic-wrong-effect.jsonl').write_text(''.join(json.dumps(r)+'\n' for r in wrong))

if __name__=='__main__':main()
