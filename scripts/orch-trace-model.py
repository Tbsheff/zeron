#!/usr/bin/env python3
"""Emit a TLA trace for the receipt/replay projection. Does not check full ZeronImpl."""
import argparse
import json
from pathlib import Path


def tla(value):
    if isinstance(value, bool):
        return 'TRUE' if value else 'FALSE'
    if isinstance(value, str):
        return json.dumps(value)
    if isinstance(value, list):
        return '<<' + ', '.join(tla(v) for v in value) + '>>'
    if isinstance(value, dict):
        return '[' + ', '.join(f'{k} |-> {tla(v)}' for k, v in value.items()) + ']'
    raise ValueError(f'unsupported TLA value {value!r}')


def emit(path, out):
    rows = []
    for line in Path(path).read_text().splitlines():
        r = json.loads(line)
        rows.append(dict(dev=r['dev'], key=(r.get('cmd') if (r.get('cmd') or '').startswith(('remote-recv:', 'remote-send:')) else (r.get('msg') or {}).get('id', '')),
                         act=r['act'], dup=r['dup'], post=r['post'],
                         fx=[e['id'] for e in r['fx']]))
    if not rows:
        raise ValueError('empty trace')
    out.mkdir(parents=True, exist_ok=True)
    (out / 'TraceReplay.tla').write_text(
        '---- MODULE TraceReplay ----\nEXTENDS TraceLedger\n'
        + 'TraceRows == ' + tla(rows) + '\n====\n')
    (out / 'TraceReplay.cfg').write_text(
        'CONSTANT Rows <- TraceRows\nSPECIFICATION Spec\nINVARIANT LedgerStepOK\nCHECK_DEADLOCK FALSE\n')


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('trace', type=Path)
    p.add_argument('--out', required=True, type=Path)
    args = p.parse_args()
    emit(args.trace, args.out)
