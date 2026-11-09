#!/usr/bin/env python3
"""Check complete captured mechanism paths against the source-derived forest.

All raw exact-step checks and the complete semantic map must pass. Missing
physical observations fail. This finite-path result does not prove universal
refinement or liveness, all causal orders, or dogfood coverage.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
from check_global_runtime import project_global, emit as emit_global
from private_runner import run_tlc
from runtime_provenance import validate as validate_provenance, validate_context
from runtime_full_projection import project_full, FullProjectionError
from build_runtime_context import build as build_context
from emit_operational_full_projection import emit as emit_operational

HERE=Path(__file__).resolve().parent


def validate_source_visibility(mapped,rows):
    """Retain every raw row and its exact physical visibility in all products."""
    for prefix in ('','reporting_','transfer_'):
        sources=mapped.get(prefix+'source_map',[])
        if not sources or any(type(source.get('raw_row')) is not int for source in sources):
            raise FullProjectionError(0,'Context','transaction.source_map_absent')
        raw_indices=[source['raw_row'] for source in sources]
        if set(raw_indices)!=set(range(len(rows))) or raw_indices!=sorted(raw_indices):
            raise FullProjectionError(0,'Context','transaction.source_map_coverage')
        last={raw:index for index,raw in enumerate(raw_indices)}
        for index,source in enumerate(sources):
            raw=source['raw_row'];tx=rows[raw].get('transaction',{})
            expected=tx.get('visible') is True and last[raw]==index
            if (tx.get('known') is not True or source.get('transaction')!=tx
                    or source.get('physical_visible') is not expected):
                raise FullProjectionError(raw,rows[raw].get('action','Context'),'transaction.source_map_visibility')


def check(a):
    provenance=validate_provenance(a.provenance,a.captures,a.jar,a.source_root,a.model_root)
    context=validate_context(a.context,a.provenance,a.captures)
                                                                                   
    encoded=lambda value:json.dumps(value,sort_keys=True,separators=(',',':'),allow_nan=False)
    if encoded(context)!=encoded(build_context(a.captures)):
        raise ValueError('mapping context is not derived from selected captures')
    steps,devices=project_global(a.captures,require_transactions=True)
    rows=[dict(step['row'],partitions=step['parts']) for step in steps]
                                                                       
    exact=a.out/'mechanism-private';emit_global(a.captures,exact)
    code=run_tlc(exact,a.jar,'RuntimeGlobalObserved.tla','RuntimeGlobalObserved.cfg',
                 {'check':'full-runtime-mechanism-prerequisite','rows':len(rows),'full_source_trace':False})
    if code:return code
    mapped=project_full(rows,context)
    validate_source_visibility(mapped,rows)
    full=a.out/'source-private';emit_operational(mapped,full,provenance=provenance)
    return run_tlc(full,a.jar,'CapturedOperationalFullRuntime.tla','CapturedOperationalFullRuntime.cfg',
       {'check':'full-runtime-source-path','rows':len(rows),'images':len(mapped['images']),
        'devices':devices,'provenance_sha256':provenance['provenance_sha256'],
        'capture_groups':len(provenance['scopes']),
        'synthetic_groups':sum(x['synthetic'] for x in provenance['scopes']),
        'physical_transactions':True,'question_issuance_join':True,
        'universal_refinement':False,'implementation_fair_closure':False,
        'all_causal_orders':False,'dogfood_coverage':False,'week_coverage':False})


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('captures',nargs='+',type=Path);p.add_argument('--context',required=True,type=Path)
    p.add_argument('--out',required=True,type=Path);p.add_argument('--jar',required=True,type=Path)
    p.add_argument('--provenance',required=True,type=Path);p.add_argument('--source-root',required=True,type=Path)
    p.add_argument('--model-root',type=Path,default=HERE.parent)
    a=p.parse_args()
    raise SystemExit(check(a))

if __name__=='__main__':
    try:main()
    except FullProjectionError as error:
        action=error.action if error.action in {'Init','RecoveryOpen','Apply','Observe','ObserveCatalog','CompleteEffect','MarkCommand','CommandDecision','Context','QuestionIssuance'} else 'Unsupported'
        field=error.field if re.fullmatch(r'[A-Za-z0-9_.]+',error.field) else 'source.unsupported'
        print(json.dumps(dict(check='full-runtime-source-path',status='fail',row=error.row,action=action,field=field)))
        raise SystemExit(2)
    except (ValueError,KeyError,TypeError,OSError,subprocess.SubprocessError):
        print(json.dumps(dict(check='full-runtime-source-path',status='fail',reason='incomplete or inconsistent source-linked inputs')))
        raise SystemExit(2)
