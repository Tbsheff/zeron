#!/usr/bin/env python3
"""Emit a synthetic two-device path with independent local roots."""
import copy
import json
from pathlib import Path
import argparse
from make_joint_fixture import build

def make(out):
    rows=build()
    out.mkdir(parents=True,exist_ok=True)
    for dev in ('A','B'):
        result=copy.deepcopy(rows)
        for row in result:
            seq=row['seq'];row['dev']=dev
            row['origin']['commit']=f'sqlite:{dev}:{seq}'
            row['origin']['parents']=[] if not seq else [f'sqlite:{dev}:{seq-1}']
            for thread in row['snapshot']['threads']:
                thread.update(chat='root-'+dev,host=dev,shadow=False,host_known=True)
        (out/f'synthetic-{dev}.jsonl').write_text(''.join(json.dumps(r)+'\n' for r in result))
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--out',required=True,type=Path)
    make(p.parse_args().out)
