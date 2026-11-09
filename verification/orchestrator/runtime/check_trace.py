#!/usr/bin/env python3
"""Check complete, independently projected observations against ZI actions.

Input is a model projection, NOT raw MechanismRow. It is deliberately a
separate format so raw capture cannot be silently treated as full conformance.
TLA sets/functions use {"$set": [...]} / {"$fn": [[key,value], ...]}.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent


def tla(value):
    if isinstance(value, bool):
        return 'TRUE' if value else 'FALSE'
    if isinstance(value, int) and value >= 0:
        return str(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=True)
    if isinstance(value, list):
        return '<<' + ', '.join(map(tla, value)) + '>>'
    if isinstance(value, dict):
        if set(value) == {'$set'}:
            return '{' + ', '.join(map(tla, value['$set'])) + '}'
        if set(value) == {'$fn'}:
            pairs = value['$fn']
            if not pairs:
                return '[x \\in {} |-> x]'
            if len({json.dumps(k, sort_keys=True) for k, _ in pairs}) != len(pairs):
                raise ValueError('duplicate function domain')
            return '(' + ' @@ '.join('(' + tla(k) + ' :> ' + tla(v) + ')' for k, v in pairs) + ')'
        if not value:
            return '[x \\in {} |-> x]'
        if any(not k.isidentifier() for k in value):
            raise ValueError('invalid record fields')
        return '[' + ', '.join(k + ' |-> ' + tla(v) for k, v in value.items()) + ']'
    raise ValueError('unsupported TLA value')


def emit(data, output):
    if data.get('format') != 'zi-observed-projection-v1':
        raise ValueError('requires zi-observed-projection-v1, not raw capture')
    rows = data['rows']
    if not rows:
        raise ValueError('empty observations')
    expected = set(json.loads((HERE / 'zi-state-fields.json').read_text()))
    for i, row in enumerate(rows):
        if set(row['state']) != expected:
            raise ValueError(f'row {i}: state fields differ: {set(row["state"]) ^ expected}')
        if set(row['action']) != {'name', 'args'}:
            raise ValueError(f'row {i}: invalid named action')
    output.mkdir(parents=True, exist_ok=True)
    shutil.copy(HERE / 'ZI_ObservedTrace.tla', output)
    shutil.copy(HERE.parent / 'ZI_ZeronImpl.tla', output)
    (output / 'ObservedRun.tla').write_text(
        '---- MODULE ObservedRun ----\nEXTENDS ZI_ObservedTrace\n'
        'ObservedRows == ' + tla(rows) + '\n====\n')
    constants = '\n'.join('  ' + k + ' = ' + tla(v) for k, v in data['constants'].items())
    (output / 'ObservedRun.cfg').write_text(
        'CONSTANT Trace <- ObservedRows\nCONSTANTS\n' + constants + '\n'
        'SPECIFICATION TraceSpec\nINVARIANT TraceEnabled\n'
        'PROPERTY ZSpec\nCHECK_DEADLOCK TRUE\n')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('projection', type=Path)
    p.add_argument('--jar', type=Path, required=True)
    p.add_argument('--out', type=Path, required=True)
    args = p.parse_args()
    emit(json.loads(args.projection.read_text()), args.out)
                                                                          
    result = subprocess.run(['java', '-XX:ActiveProcessorCount=4', '-cp', str(args.jar.resolve()),
                             'tlc2.TLC', '-workers', '1', '-config', 'ObservedRun.cfg',
                             'ObservedRun.tla'], cwd=args.out, timeout=300)
    raise SystemExit(result.returncode)


if __name__ == '__main__':
    main()
