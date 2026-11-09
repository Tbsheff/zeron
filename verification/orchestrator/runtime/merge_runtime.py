#!/usr/bin/env python3
"""Build a clock-free v2 causal DAG from complete device captures.

A durable wire row is its own commit node. A late recovery observation proves
that node exists; the observation is not mislabelled as the send time.
Requires doc_commit.effect_id to link the captured write to its actual outbox
intent. Missing links fail. This merge is not a global conformance check.
"""
import argparse
import heapq
import json
import re
from pathlib import Path


def merge(paths, *, require_transactions=False):
    rows={};bydev={};effects={};docs={};wire_digests={};intent_wires={};observations=[];receives=[];receive_facts={};receive_fact_nodes={}
    for path in paths:
        for raw in Path(path).read_text().splitlines():
            if not raw.strip():continue
            r=json.loads(raw)
            if not isinstance(r,dict) or r.get('v')!=2 or not isinstance(r.get('dev'),str) or type(r.get('seq')) is not int or r['seq']<0 or not isinstance(r.get('origin'),dict):
                raise ValueError('invalid mechanism header')
            key=('sqlite',r['dev'],r['seq'])
            if key in rows:raise ValueError('duplicate mechanism sequence')
            rows[key]=r;bydev.setdefault(r['dev'],[]).append(key)
    if not rows:raise ValueError('empty mechanism capture')
    present=['transaction' in r.get('origin',{}) for r in rows.values()]
    grouped=all(present)
    if (any(present) and not grouped) or (require_transactions and not grouped):
        raise ValueError('physical transaction grouping missing')
    group_of={};group_rows={}
    for dev,keys in bydev.items():
        keys.sort(key=lambda k:k[2])
        if [k[2] for k in keys]!=list(range(len(keys))) or rows[keys[0]]['action']!='Init':
            raise ValueError('requires complete per-device Init sequence')
        previous_outbox=set();previous_facts={};last_group=None;closed=set()
        for key in keys:
            transaction=rows[key]['origin'].get('transaction') if grouped else f'sqlite:{dev}:{key[2]}'
            if not isinstance(transaction,str) or not transaction.startswith(f'sqlite:{dev}:'):
                raise ValueError('invalid physical transaction device')
            tail=transaction[len(f'sqlite:{dev}:'):]
            if not tail.isdecimal() or str(int(tail))!=tail:
                raise ValueError('invalid physical transaction identity')
            group=('transaction',dev,int(tail))
            if group!=last_group:
                if group in closed or group[2]!=key[2]:
                    raise ValueError('physical transaction is not a contiguous first-row group')
                if last_group is not None:closed.add(last_group)
                last_group=group
            group_of[key]=group;group_rows.setdefault(group,[]).append(key)
        for key in keys:
            r=rows[key];snap=r['snapshot']
            expected_parents=[] if key[2]==0 else [f'sqlite:{dev}:{key[2]-1}']
            executing=r.get('args',{}).get('effect_id')
            producer=effects.get((dev,executing)) if isinstance(executing,str) else None
            if producer is not None:
                parent=f'sqlite:{dev}:{producer[2]}'
                if parent not in expected_parents:expected_parents.append(parent)
            expected_origin=dict(kind='sqlite',commit=f'sqlite:{dev}:{key[2]}',parents=expected_parents)
            if grouped:expected_origin['transaction']=r['origin']['transaction']
            if r.get('origin')!=expected_origin:
                raise ValueError('SQLite origin differs from captured local chain')
            current_outbox={x['effect_id'] for x in snap['outbox']}
            if not previous_outbox<=current_outbox:raise ValueError('outbox intent disappeared')
            for fx in current_outbox-previous_outbox:effects[(dev,fx)]=key
            previous_outbox=current_outbox
            facts={x['id']:x for x in snap['facts']}
            for fact_id,old in previous_facts.items():
                if facts.get(fact_id)!=old:raise ValueError('immutable fact changed or disappeared')
            for fact_id in facts.keys()-previous_facts.keys():
                fact=facts[fact_id]
                if fact['kind']=='remote_receive':
                    d=fact['data'];receipt=d['receipt_id'];identity=(dev,receipt)
                    if d.get('validated') is not True or d.get('receiver_device')!=dev:
                        raise ValueError('receiver payload was not validated on its device')
                    if identity in receive_facts and receive_facts[identity]!=d:
                        raise ValueError('receiver receipt has conflicting payload observations')
                    receive_facts[identity]=d
                    receive_fact_nodes.setdefault(identity,key)
                if fact['kind']!='doc_commit':continue
                d=fact['data']
                if not d.get('effect_id') or d.get('device')!=dev:
                    raise ValueError('doc commit lacks captured executing intent')
                wire=d['wire_id'];entry=(dev,d['effect_id'],d['target_host'])
                body_digest=d.get('wire_body_digest')
                if not isinstance(body_digest,str) or re.fullmatch(r'sha256:[0-9a-f]{64}',body_digest) is None:
                    raise ValueError('wire commit lacks actual serialized payload digest')
                if wire in wire_digests and wire_digests[wire]!=body_digest:
                    raise ValueError('one wire row has different observed payload digests')
                wire_digests[wire]=body_digest
                if wire in docs and docs[wire]!=entry:raise ValueError('wire commit has conflicting origin')
                intent=(dev,d['effect_id'])
                if intent in intent_wires and intent_wires[intent]!=wire:
                    raise ValueError('one immutable send intent produced different wire rows')
                intent_wires[intent]=wire
                docs[wire]=entry;observations.append((wire,key))
            previous_facts=facts
            if r['action']=='Apply' and r['args']['key'].startswith('remote-recv:'):
                receives.append((r['args']['key'][len('remote-recv:'):],key))
    nodes=set(rows)|{('doc',wire) for wire in docs}
    edges={n:set() for n in nodes};parents={n:set() for n in nodes};indegree={n:0 for n in nodes}
    def edge(a,b):
        if a!=b and b not in edges[a]:
            edges[a].add(b);parents[b].add(a);indegree[b]+=1
    for keys in bydev.values():
        keys.sort(key=lambda k:k[2])
        for a,b in zip(keys,keys[1:]):edge(a,b)
    for wire,(dev,fx,target_host) in docs.items():
        if (dev,fx) not in effects:raise ValueError('wire commit has no captured outbox intent')
        edge(effects[(dev,fx)],('doc',wire))
    for wire,obs in observations:edge(('doc',wire),obs)
    for identity,observed in receive_facts.items():
        wire=observed['wire_id']
        if wire not in docs or observed['origin_device']!=docs[wire][0] or observed['receiver_device']!=docs[wire][2] or observed['wire_body_digest']!=wire_digests[wire]:
            raise ValueError('receiver observation lacks matching captured producer payload')
        edge(('doc',wire),receive_fact_nodes[identity])
    for wire,recv in receives:
        if wire not in docs:raise ValueError('receive has no captured durable wire commit')
        if docs[wire][2]!=recv[1]:raise ValueError('receive device differs from captured target host')
        observed=receive_facts.get((recv[1],'remote-recv:'+wire))
        if observed is None or receive_fact_nodes[(recv[1],'remote-recv:'+wire)][2]>recv[2] or observed['wire_id']!=wire or observed['origin_device']!=docs[wire][0] or observed['wire_body_digest']!=wire_digests[wire]:
            raise ValueError('applied receive lacks matching actual payload observation')
        edge(('doc',wire),recv)
                                                                            
                                                                               
    def unit(n):return group_of[n] if n[0]=='sqlite' else n
    units={unit(n) for n in nodes};unit_edges={n:set() for n in units};unit_degree={n:0 for n in units}
    for a,afters in edges.items():
        for b in afters:
            x,y=unit(a),unit(b)
            if x!=y and y not in unit_edges[x]:unit_edges[x].add(y);unit_degree[y]+=1
    ready=[n for n,count in unit_degree.items() if count==0];heapq.heapify(ready);order=[];visited=0
    while ready:
        n=heapq.heappop(ready);visited+=1
        order.extend(group_rows[n] if n[0]=='transaction' else [n])
        for after in sorted(unit_edges[n]):
            unit_degree[after]-=1
            if unit_degree[after]==0:heapq.heappush(ready,after)
    if visited!=len(units):raise ValueError('physical commit DAG contains a cycle')
    visibility={key:dict(id=rows[key]['origin'].get('transaction'),index=i,size=len(keys),visible=i==len(keys)-1)
                for keys in group_rows.values() for i,key in enumerate(keys)}
    return dict(format='runtime-causal-dag-v1',full_conformance=False,
        wires=[dict(wire_id=wire,origin_device=dev,effect_id=fx,target_host=host,
                    intent_seq=effects[(dev,fx)][2],wire_body_digest=wire_digests[wire])
               for wire,(dev,fx,host) in sorted(docs.items())],
        physical_transactions=grouped,
        visibility=[dict(dev=n[1],seq=n[2],**visibility[n]) for n in order if n[0]=='sqlite'],
        rows=[rows[n] for n in order if n[0]=='sqlite'],
        nodes=[dict(id=list(n),parents=[list(p) for p in sorted(parents[n])]) for n in order])


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('captures',nargs='+',type=Path);p.add_argument('--out',required=True,type=Path)
    a=p.parse_args()
    try:result=merge(a.captures)
    except (ValueError,KeyError,TypeError):
        print(json.dumps({'status':'fail','check':'runtime-causal-merge','reason':'missing or conflicting causal evidence'}));raise SystemExit(2)
    a.out.parent.mkdir(parents=True,exist_ok=True);a.out.write_text(json.dumps(result)+'\n')
    print(json.dumps({'status':'pass','check':'runtime-causal-merge','rows':len(result['rows']),'devices':len({r['dev'] for r in result['rows']}),'full_conformance':False}))
if __name__=='__main__':main()
