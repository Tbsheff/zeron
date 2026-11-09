#!/usr/bin/env python3
"""Capture explicit SQLite snapshots and track a seven-day release evidence window."""
import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_snapshot(db, after_trace=None):
    with sqlite3.connect(db.resolve().as_uri() + '?mode=ro', uri=True) as conn:
        conn.execute('BEGIN')
        lines = [r[0] for r in conn.execute('SELECT line FROM orch_trace ORDER BY seq')]
        if after_trace is not None:
            after_trace()
        raw = [r[0] for r in conn.execute('SELECT line FROM orch_mechanism ORDER BY seq')] if conn.execute("SELECT 1 FROM sqlite_master WHERE name='orch_mechanism'").fetchone() else []
        return lines, raw


def full_check(directory, record):
    link = record.get('full_check')
    checker = ROOT/'verification/orchestrator/runtime/check_full_runtime.py'
    if not link or not checker.is_file() or not record.get('mechanisms'):
        return False
    for name in ('provenance.json', 'context.json', 'mechanism-capture.json'):
        if digest(directory/name) != link.get(name):
            raise ValueError('changed full-check input')
    jar = Path(os.environ.get('TLA_JAR', ROOT/'verification/orchestrator/tools/tla2tools.jar'))
    if not jar.is_file():
        return False
    command = [sys.executable, str(checker), *[str(directory/name) for name in record['mechanisms']],
               '--context', str(directory/'context.json'), '--provenance', str(directory/'provenance.json'),
               '--source-root', str(ROOT), '--model-root', str(ROOT/'verification/orchestrator'),
               '--jar', str(jar), '--out', str(directory/'full-private')]
    with (directory/'full-validation.log').open('w') as log:
        proc = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            return proc.wait(timeout=300) == 0
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()
            return False


def capture(databases, out, usage="trial", provenance=None, context=None):
    if bool(provenance) != bool(context):
        raise ValueError("supply both build provenance and source-map context")
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
        lines, raw = read_snapshot(db)
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
                  source_check="required")
    if provenance is not None:
        data = json.loads(provenance.read_text())
        sidecar = dest/"mechanism-capture.json"
        sidecar.write_text(json.dumps(dict(test_name="dogfood_capture", scope="dogfood" if usage == "dogfood" else "trial",
            synthetic=usage != "dogfood", devices=sorted(watermarks), filenames=list(mechanisms)), indent=2)+"\n")
        data["captures"] = [dict(sidecar_sha256=digest(sidecar), test_name="dogfood_capture",
            scope="dogfood" if usage == "dogfood" else "trial", synthetic=usage != "dogfood",
            files=[dict(sha256=value, bytes=(dest/name).stat().st_size,
                        rows=sum(bool(x.strip()) for x in (dest/name).read_bytes().splitlines()))
                   for name,value in mechanisms.items()])]
        data["mapping_context_sha256"] = digest(context)
        (dest/"provenance.json").write_text(json.dumps(data, indent=2)+"\n")
        (dest/"context.json").write_bytes(context.read_bytes())
        record["full_check"] = {name:digest(dest/name) for name in ("provenance.json", "context.json", "mechanism-capture.json")}
    (dest/'capture.json').write_text(json.dumps(record, indent=2)+'\n')
    passed = full_check(dest, record)
    print(f'Captured {count} steps; current full source check: {passed}')


def status(out):
    out = out.resolve()
    records = []
    for path in sorted(out.glob('*/capture.json')):
        record = json.loads(path.read_text())
        for name, expected in (record['traces'] | record.get('mechanisms', {})).items():
            source = (path.parent/name).resolve()
            if source.parent != path.parent.resolve():
                raise ValueError('capture path escapes capture directory')
            if digest(source) != expected:
                raise ValueError(f'changed trace: {path.parent}')
        captured = datetime.fromisoformat(record['captured_at'])
        if captured.tzinfo is None or captured > datetime.now(timezone.utc):
            raise ValueError('invalid or future capture time')
        marks = {}
        for name in record['traces']:
            source = path.parent/name
            if source.resolve().parent != path.parent.resolve():
                raise ValueError('capture trace path escapes capture directory')
            for line in source.read_text().splitlines():
                row = json.loads(line)
                if row.get('v') != 1 or not isinstance(row.get('dev'), str) or type(row.get('seq')) is not int or row['seq'] < 0:
                    raise ValueError('invalid trace watermark source')
                marks[row['dev']] = max(marks.get(row['dev'], -1), row['seq'])
        record['watermarks'] = marks
        if record.get('usage') == 'dogfood':
            record['_directory'] = path.parent
            records.append(record)
    records.sort(key=lambda r: datetime.fromisoformat(r['captured_at']))
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
    conformance = bool(records) and all(full_check(record['_directory'], record) for record in records)
    print(f'Captured days: {len(days)}; seven-day window: {week}; full ZeronImpl: {conformance}')
    print('Keep flag off until the week, full conformance, proofs, and CI pass')
    return 0 if week and conformance else 1


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action', choices=['capture', 'status'])
    p.add_argument('--usage', choices=['trial', 'dogfood'], default='trial', help='dogfood asserts actual use; synthetic fixtures remain trial')
    p.add_argument('--provenance', type=Path, help='private source/build receipt for the actual engine')
    p.add_argument('--context', type=Path, help='private actual source-map inputs for the strict full checker')
    p.add_argument('--db', type=Path, nargs='+')
    p.add_argument('--out', type=Path, default=Path(os.environ.get('ORCH_DOGFOOD_DIR') or ROOT/'target/orch-dogfood'))
    args = p.parse_args()
    if args.action == 'capture':
        if not args.db:
            p.error('capture requires --db pointing to an explicit snapshot')
        capture(args.db, args.out, args.usage, args.provenance, args.context)
    else:
        raise SystemExit(status(args.out))
