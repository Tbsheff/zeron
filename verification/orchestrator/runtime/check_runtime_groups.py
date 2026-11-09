#!/usr/bin/env python3
"""Run the partial raw safety checker on completed exporter groups.

Keep --out private. Save only safe result metadata in --receipt. This debug
batch does not turn a component/model-path pass into full conformance.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import subprocess
import sys
from inventory_runtime_groups import collect
HERE=Path(__file__).resolve().parent


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('root',type=Path)
    p.add_argument('--out',type=Path,required=True);p.add_argument('--receipt',type=Path,required=True)
    p.add_argument('--jar',type=Path,required=True);p.add_argument('--jobs',type=int,choices=[1,2],default=1)
    a=p.parse_args();_,index,skipped=collect(a.root,'engine-integration');a.out.mkdir(parents=True,exist_ok=True)
    def check(item):
        dest=a.out/item['sha256'];dest.mkdir(parents=True,exist_ok=True)
        command=[sys.executable,str(HERE/'check_runtime.py'),item['path'],'--out',str(dest),'--jar',str(a.jar)]
        try:
            run=subprocess.run(command,capture_output=True,text=True,timeout=310)
            (dest/'runner-private.log').write_text(run.stdout+run.stderr)
            records=[]
            for line in run.stdout.splitlines():
                try:records.append(json.loads(line))
                except json.JSONDecodeError:pass
            last=records[-1] if records else {}
            safe={key:last[key] for key in ('status','row','predicate','rows','reason') if key in last}
            return dict(sha256=item['sha256'],exit_code=run.returncode,**safe)
        except subprocess.TimeoutExpired:
            return dict(sha256=item['sha256'],exit_code=124,status='timeout')
    with ThreadPoolExecutor(max_workers=a.jobs) as pool:results=list(pool.map(check,index))
    result=dict(check='runtime-partial-safety-debug-batch',captures=len(results),
        passed=sum(x['exit_code']==0 for x in results),skipped_manifests=skipped,
        full_conformance=False,source_claim='debug captures; source identity not final',results=results)
    a.receipt.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({k:v for k,v in result.items() if k!='results'}))
if __name__=='__main__':main()
