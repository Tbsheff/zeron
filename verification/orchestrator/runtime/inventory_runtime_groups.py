#!/usr/bin/env python3
"""Inventory only completed exporter groups; keep raw paths out of receipts."""
import argparse
import hashlib
import json
from pathlib import Path
from inventory_runtime_captures import inventory


def collect(root,source_class):
    result=[];index=[];skipped=0
    for sidecar in sorted(root.glob('*/capture.json')):
        data=json.loads(sidecar.read_text())
        if 'capture' in data and data['capture']!='present':skipped+=1;continue
        names=data.get('files',data.get('filenames',[]))
        if not names:skipped+=1;continue
        manifest=hashlib.sha256(sidecar.read_bytes()).hexdigest()
        for name in names:
            if Path(name).name!=name:raise ValueError('invalid exporter filename')
            path=sidecar.parent/name;item=inventory(path,source_class)
            item['manifest_sha256']=manifest;result.append(item)
            index.append(dict(sha256=item['sha256'],path=str(path),manifest_sha256=manifest))
    return result,index,skipped


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('root',type=Path)
    p.add_argument('--source-class',required=True,choices=['synthetic','focused-mock-engine','engine-integration','dogfood'])
    p.add_argument('--out',type=Path,required=True);p.add_argument('--private-index',type=Path)
    a=p.parse_args();result,index,skipped=collect(a.root,a.source_class)
    a.out.write_text(json.dumps(result,indent=2)+'\n')
    if a.private_index:a.private_index.write_text(json.dumps(index,indent=2)+'\n')
    print(json.dumps(dict(check='runtime-group-inventory',captures=len(result),rows=sum(x['rows'] for x in result),skipped_manifests=skipped,claim='inventory only')))
if __name__=='__main__':
    try:main()
    except (ValueError,KeyError,TypeError):
        print(json.dumps(dict(check='runtime-group-inventory',status='fail',reason='invalid completed exporter group')));raise SystemExit(2)
