#!/usr/bin/env python3
"""Inventory strict context/full-map blockers. This command does not run TLC."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
from build_runtime_context import build
from check_global_runtime import project_global
from runtime_full_projection import project_full,FullProjectionError

HERE=Path(__file__).resolve().parent
ACTIONS={'Init','RecoveryOpen','Apply','Observe','ObserveCatalog','CompleteEffect','MarkCommand','CommandDecision','Context','QuestionIssuance'}
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()

def inventory(root):
    results=[]
    for sidecar in sorted(root.glob('*/capture.json')):
        data=json.loads(sidecar.read_text())
        if data.get('capture')!='present':continue
        names=data.get('files',data.get('filenames',[]))
        if not names:continue
        if any(Path(n).name!=n for n in names):raise ValueError('invalid declared filename')
        paths=[sidecar.parent/n for n in names]
        item=dict(manifest_sha256=sha(sidecar),captures=len(paths))
        try:
            context=build(paths);item['context']='ready'
            steps,_=project_global(paths)
            rows=[dict(s['row'],partitions=s['parts']) for s in steps]
            mapped=project_full(rows,context)
            item.update(status='forest_images_only',rows=len(rows),images=len(mapped['images']),full_conformance=False)
        except FullProjectionError as error:
            action=error.action if error.action in ACTIONS else 'Unsupported'
            field=error.field if re.fullmatch(r'[A-Za-z0-9_.]+',error.field) else 'source.unsupported'
            item.update(status='blocked',row=error.row,action=action,field=field)
        except (ValueError,KeyError,TypeError,OSError):
            item.update(status='blocked',field='source.inputs')
        results.append(item)
    sources=['runtime_full_projection.py','runtime_action_projection.py','build_runtime_context.py','composition_history_projection.py','composition_transfer_projection.py']
    return dict(scope='whole completed groups; context and forest projection only; no TLC or full conformance result',
        groups=len(results),context_ready=sum(r.get('context')=='ready' for r in results),
        blockers=dict(Counter(r['field'] for r in results if r['status']=='blocked')),
        projected_groups=sum(r['status']=='forest_images_only' for r in results),results=results,
        source_sha256={n:sha(HERE/n) for n in sources})

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root',type=Path);parser.add_argument('--out',required=True,type=Path)
    args=parser.parse_args();result=inventory(args.root)
    args.out.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({k:v for k,v in result.items() if k not in {'results','source_sha256'}}))
if __name__=='__main__':main()
