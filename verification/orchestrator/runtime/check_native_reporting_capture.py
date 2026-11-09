#!/usr/bin/env python3
"""Check each real observed prefix and raw-derived identity mutants.

The manifest binds completed raw files by SHA. This does not run target Next
or claim that a source-raced debug build is a final source proof.
"""
import argparse
from copy import deepcopy
import hashlib
import json
from pathlib import Path
from native_reporting_projection import project_native_reporting, fn
from runtime_projection import project

def digest(data): return hashlib.sha256(data).hexdigest()
def fail(field): raise ValueError(field)
def global_rows(raw):
    return [dict(row,partitions=fn([(row['dev'],dict(core=row['core'],mechanism=row['mechanism']))]),wire_commits=fn([]))
            for row in project(raw)]
def read_rows(path): return [json.loads(line) for line in path.read_text().splitlines() if line]

def check(manifest, out, stage="phase3"):
    result=[]
    for item in json.loads(manifest.read_text()):
        source=Path(item['source']); data=source.read_bytes()
        if digest(data)!=item['sha256']:raise ValueError('capture SHA differs: '+item['test'])
        raw=read_rows(source);rows=global_rows(raw)
        dest=out/('actual-'+stage+'-reporting-'+item['test']);dest.mkdir(exist_ok=True)
        context=json.loads((dest/'context.json').read_text())
        focus=next(iter(context['native_slots']));owner=rows[0]['dev']
        images=[]
        for i,row in enumerate(rows):
            images.append(dict(raw_seq=raw[i]['seq'],**project_native_reporting(rows[:i+1],context,focus,owner,fail)))
        (dest/'images.json').write_text(json.dumps(images,indent=2)+'\n')
        mutants=[]
        changes=(('source-sequence','provider_event','stream_seq',999),
                 ('chip-reference','native_doc_commit','chip_ref','wrong-doc'),
                 ('native-owner','provider_event','provider_run_id','wrong-runtime'))
        for name,kind,field,value in changes:
            ids=[f['id'] for r in raw for f in r['snapshot']['facts'] if f['kind']==kind and f['data'].get('chat')==focus
                 and (name!='native-owner' or f['data'].get('native_path'))]
            if not ids:continue
            identity=ids[0];mutant=deepcopy(raw)
            for r in mutant:
                for f in r['snapshot']['facts']:
                    if f['id']==identity:f['data'][field]=value
            encoded=('\n'.join(json.dumps(r,separators=(',',':')) for r in mutant)+'\n').encode()
            mdir=dest/('mutant-'+name);mdir.mkdir(exist_ok=True);(mdir/'capture.jsonl').write_bytes(encoded)
            try:project_native_reporting(global_rows(mutant),context,focus,owner,fail)
            except ValueError as error:failure=str(error)
            else:raise AssertionError('mutant passed: '+name)
            receipt=dict(case=name,source_sha256=item['sha256'],mutant_sha256=digest(encoded),
                         fact_id=identity,field=field,failure=failure)
            (mdir/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n');mutants.append(receipt)
        result.append(dict(item,mapped_prefixes=len(images),reporting_complete_prefixes=sum(x['reporting_complete'] for x in images),
                           last_unsupported=images[-1]['unsupported_fields'],mutants=mutants,failure=None))
    (out/('actual-'+stage+'-reporting-results.json')).write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps([dict(test=x['test'],prefixes=x['mapped_prefixes'],mutants=len(x['mutants'])) for x in result]))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('manifest',type=Path);parser.add_argument('--out',type=Path,default=Path(__file__).resolve().parent)
    parser.add_argument('--stage',choices=['phase3','final'],default='phase3')
    args=parser.parse_args();check(args.manifest,args.out,args.stage)
