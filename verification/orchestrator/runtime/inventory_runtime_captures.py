#!/usr/bin/env python3
"""Save metadata for real or synthetic mechanism inputs. Never check conformance.

No snapshot, event argument, chat ID, wire ID, or private path is emitted.
The caller must state the source class; hashes bind the private raw inputs.
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path

BOUNDARY_KINDS={'registry_host','access_config','received_ceiling','device_reachability',
 'provider_start','journal_observed','tool_reply','tool_observed','doc_commit',
 'transfer_state','transfer_operation'}


def inventory(path,source_class):
    content=path.read_bytes();rows=[json.loads(x) for x in content.splitlines() if x.strip()]
    if not rows:raise ValueError('empty capture')
    acts=Counter();applies=Counter();facts={};fact_counts=Counter();operations=Counter()
    for row in rows:
        acts[row['action']]+=1
        if row['action']=='Apply':
            a=row['args'];result='duplicate' if a['duplicate'] else 'accepted' if a['accepted'] else 'rejected'
            applies[(a['event']['act'],result)]+=1
        for fact in row['snapshot']['facts']:
            key=(row['dev'],fact['id'])
            if key in facts:
                if facts[key]!=fact:raise ValueError('immutable fact changed')
                continue
            facts[key]=fact;fact_counts[fact['kind']]+=1
            if fact['kind']=='transfer_operation':operations[fact['data']['operation']]+=1
    bydev={d:[r for r in rows if r['dev']==d] for d in {r['dev'] for r in rows}}
    complete=all(part[0]['action']=='Init' and [r['seq'] for r in part]==list(range(len(part))) for part in bydev.values())
    return {'sha256':hashlib.sha256(content).hexdigest(),'source_class':source_class,
      'rows':len(rows),'devices':len(bydev),'complete_local_sequences':complete,
      'mechanism_actions':dict(sorted(acts.items())),
      'core_actions':[{'action':a,'outcome':o,'count':n} for (a,o),n in sorted(applies.items())],
      'fact_kinds':dict(sorted(fact_counts.items())),
      'transfer_operations':dict(sorted(operations.items())),
      'absent_boundary_kinds':sorted(BOUNDARY_KINDS-set(fact_counts)),
      'claim':'inventory only; no conformance result'}


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('captures',type=Path,nargs='+');p.add_argument('--out',type=Path,required=True)
    p.add_argument('--source-class',choices=['synthetic','focused-mock-engine','engine-integration','dogfood'],required=True)
    a=p.parse_args();result=[inventory(x,a.source_class) for x in a.captures]
    a.out.parent.mkdir(parents=True,exist_ok=True);a.out.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({'check':'runtime-capture-inventory','captures':len(result),'rows':sum(x['rows'] for x in result),'claim':'inventory only'}))
if __name__=='__main__':
    try:main()
    except (ValueError,KeyError,TypeError):
        print(json.dumps({'check':'runtime-capture-inventory','status':'fail','reason':'invalid capture schema'}));raise SystemExit(2)
