#!/usr/bin/env python3
"""Capture explicit SQLite snapshots and track a seven-day release evidence window."""
import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def capture(databases, out, usage="trial"):
    out = out.resolve()
    live = Path.home() / '.zeron'
    if any(db.resolve().is_relative_to(live.resolve()) for db in databases):
        raise ValueError('use explicit snapshots outside the live ~/.zeron profile')
    now = datetime.now(timezone.utc)
    dest = out / now.strftime('%Y-%m-%dT%H%M%SZ')
    dest.mkdir(parents=True, exist_ok=False)
    paths = []
    count = 0
    watermarks = {}
    mechanisms = {}
    for index, db in enumerate(databases):
        with sqlite3.connect(db.resolve().as_uri() + '?mode=ro', uri=True) as conn:
            lines = [r[0] for r in conn.execute('SELECT line FROM orch_trace ORDER BY seq')]
            raw = [r[0] for r in conn.execute('SELECT line FROM orch_mechanism ORDER BY seq')] if conn.execute("SELECT 1 FROM sqlite_master WHERE name='orch_mechanism'").fetchone() else []
        if not lines:
            raise ValueError('snapshot contains no orchestrator traces')
        trace = dest / f'device-{index}.jsonl'
        trace.write_text('\n'.join(lines) + '\n')
        paths.append(trace)
        count += len(lines)
        parsed = [json.loads(line) for line in lines]
        for row in parsed:
            watermarks[row['dev']] = max(watermarks.get(row['dev'], -1), row['seq'])
        if raw:
            mechanism = dest / f'mechanism-device-{index}.jsonl'
            mechanism.write_text('\n'.join(raw) + '\n')
            mechanisms[mechanism.name] = digest(mechanism)
    merged = dest / 'merged.jsonl'
    env = dict(os.environ, ORCH_TRACE_MERGED=str(merged))
    with (dest/'validation.log').open('w') as log:
        subprocess.run([str(ROOT/'tools/orch-trace'), *map(str, paths)], cwd=ROOT, env=env,
                       stdout=log, stderr=subprocess.STDOUT, check=True)
    record = dict(captured_at=now.isoformat(), traces={p.name: digest(p) for p in paths},
                  rows=count, usage=usage, watermarks=watermarks, mechanisms=mechanisms, reducer_and_receipt_projection=True,
                  full_zeronimpl_conformance=False)
    (dest/'capture.json').write_text(json.dumps(record, indent=2)+'\n')
    print(f'Captured {count} steps; full ZeronImpl conformance remains open')


def status(out):
    out = out.resolve()
    records = []
    for path in sorted(out.glob('*/capture.json')):
        record = json.loads(path.read_text())
        for name, expected in (record['traces'] | record.get('mechanisms', {})).items():
            if digest(path.parent/name) != expected:
                raise ValueError(f'changed trace: {path.parent}')
        if record.get('usage') == 'dogfood':
            records.append(record)
    active = []
    previous = {}
    for record in records:
        marks = record.get('watermarks', {})
        if any(seq < previous.get(dev, -1) for dev, seq in marks.items()):
            raise ValueError('dogfood sequence regressed')
        if any(seq > previous.get(dev, -1) for dev, seq in marks.items()):
            active.append(record)
        previous.update(marks)
    records = active
    days = sorted(set(datetime.fromisoformat(r['captured_at']).date() for r in records))
    elapsed = (datetime.fromisoformat(records[-1]['captured_at']) - datetime.fromisoformat(records[0]['captured_at'])) if records else timedelta(0)
    continuous = len(days) >= 7 and all((b-a).days == 1 for a,b in zip(days[-7:], days[-6:]))
    week = continuous and elapsed >= timedelta(days=7)
    conformance = False
    print(f'Captured days: {len(days)}; seven-day window: {week}; full ZeronImpl: {conformance}')
    print('Keep flag off until the week, full conformance, proofs, and CI pass')
    return 0 if week and conformance else 1


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action', choices=['capture', 'status'])
    p.add_argument('--usage', choices=['trial', 'dogfood'], default='trial', help='dogfood asserts actual use; synthetic fixtures remain trial')
    p.add_argument('--db', type=Path, nargs='+')
    p.add_argument('--out', type=Path, default=Path(os.environ.get('ORCH_DOGFOOD_DIR') or ROOT/'target/orch-dogfood'))
    args = p.parse_args()
    if args.action == 'capture':
        if not args.db:
            p.error('capture requires --db pointing to an explicit snapshot')
        capture(args.db, args.out, args.usage)
    else:
        raise SystemExit(status(args.out))
