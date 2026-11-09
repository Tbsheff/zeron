#!/usr/bin/env python3
"""Check supported core seam transitions in raw MechanismRow v2 capture.

This is a component checker. It does not assert full ZeronImpl conformance.
The first row supplies an observed baseline; each later row must be an
accepted nonduplicate Apply of one supported action. Unknown rows fail.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
from check_trace import tla

HERE = Path(__file__).resolve().parent
SUPPORTED = {'ProviderTerminal', 'RetryAttempt', 'RestartActive', 'Followup', 'OpenRoot', 'PersonSend', 'ServerEnqueue', 'StartFromQueue', 'Steer', 'RequestInterrupt', 'ChildStart', 'ChildEnd', 'IngestChild', 'IngestStale', 'ResetPasses'}
CAP = 2147483647


def fn(pairs):
    return {'$fn': list(pairs)}


def normalize(row):
    if row['v'] != 2:
        raise ValueError('requires mechanism version 2')
    snap = row['snapshot']
    ids = {}
    for item in snap['threads']:
        key = item['local_thread']
        if key in ids or item['chat'] == 'none':
            raise ValueError('duplicate or reserved thread ID')
        ids[key] = item['chat']
    def tid(value):
        if value is None:
            return 'none'
        if value not in ids:
            raise ValueError(f'missing stable thread identity {value}')
        return ids[value]
    state = snap['state']
    threads = []
    for key, th in state['threads']:
        if key != th['id']:
            raise ValueError('thread map key differs from thread id')
        run = th['run_base'] + len(th['runs'])
        if run >= CAP or any(th['attempt'][k] >= CAP for k in ('n', 'retries')):
            raise ValueError('counter outside exact TLC integer tranche')
        queue = []
        for msg in th['queue']:
            queue.append({'kind': msg} if isinstance(msg, str)
                         else {'kind': next(iter(msg)), 'value': next(iter(msg.values()))})
        blocked = any(q['child'] == key and q['pending'] for _, q in state['questions'])
        task = {'present': False} if th['task'] is None else {'present': True, **th['task']}
        threads.append((tid(key), dict(live=any(r['status'] == 'Live' for r in th['runs']),
            run=run, attempt=th['attempt'], parent=tid(th['parent']), depth=th['depth'],
            access=th['access'], reconfigure=th['reconfigure'], late_wait=tid(th['late_wait']),
            ul=dict(armed=th['ul']['armed'] or 0, due=th['ul']['due']),
            wait_on=tid(th['wait_on']), int_req=th['int_req'],
            held=th['held'], blocked=blocked, queue=queue, task=task,
            inbox={'$set': th['inbox']})))
    natives = []
    for (parent, key), task in state['natives']:
        attempt = task['attempt']
        if attempt['run']['thread'] != parent:
            raise ValueError('native owner differs from attempt thread')
        natives.append(([tid(parent), str(key)], dict(parent=tid(parent),
            run=attempt['run']['ord'], attempt=attempt['n'], background=task['background'],
            status=task['status'], delivery=task['delivery'])))
                                                                          
                                                                          
    return dict(config=state['config'], threads=fn(threads), natives=fn(natives), transfers=fn([])), tid


def project(rows):
    if len(rows) < 2:
        raise ValueError('requires baseline plus at least one transition')
    output = []
    previous_seq = {}
    device = rows[0]['dev']
    for i, row in enumerate(rows):
        if row['dev'] != device:
            raise ValueError('component tranche requires one device; no shadow merge inference')
        if row['seq'] <= previous_seq.get(device, -1):
            raise ValueError('non-increasing mechanism sequence')
        previous_seq[device] = row['seq']
        state, tid = normalize(row)
        event = dict(act='Baseline', args={'baseline': True})
        if i:
            args = row['args']
            if row['action'] != 'Apply' or args['duplicate'] or not args['accepted']:
                raise ValueError('unsupported mechanism row; component checker cannot skip it')
            event = json.loads(json.dumps(args['event']))
            if event['act'] not in SUPPORTED:
                raise ValueError(f'unsupported core action {event["act"]}')
            for key in ('t', 'parent', 'child'):
                if key in event['args']:
                    event['args'][key] = tid(event['args'][key])
        output.append(dict(state=state, event=event))
    return output


def emit(rows, output):
    output.mkdir(parents=True, exist_ok=True)
    shutil.copy(HERE / 'RuntimeCoreSeams.tla', output)
    (output / 'SeamsObserved.tla').write_text(
        '---- MODULE SeamsObserved ----\nEXTENDS RuntimeCoreSeams, TLC\n'
        'Rows == ' + tla(project(rows)) + '\nVARIABLE cursor\n'
        'Init == cursor = 1\nNext == IF cursor < Len(Rows) THEN cursor\' = cursor + 1 ELSE UNCHANGED cursor\n'
        'Spec == Init /\\ [][Next]_cursor\n'
        'StepOK == IF cursor = 1 THEN TRUE ELSE Apply(Rows[cursor-1].state, Rows[cursor].state, Rows[cursor].event)\n====\n')
    (output / 'SeamsObserved.cfg').write_text('SPECIFICATION Spec\nINVARIANT StepOK\nCHECK_DEADLOCK TRUE\n')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('capture', type=Path)
    p.add_argument('--out', required=True, type=Path)
    p.add_argument('--jar', required=True, type=Path)
    args = p.parse_args()
    rows = [json.loads(line) for line in args.capture.read_text().splitlines() if line.strip()]
    emit(rows, args.out)
    result = subprocess.run(['java', '-XX:ActiveProcessorCount=4', '-cp', str(args.jar.resolve()),
        'tlc2.TLC', '-workers', '1', '-config', 'SeamsObserved.cfg', 'SeamsObserved.tla'],
        cwd=args.out, timeout=300)
    raise SystemExit(result.returncode)


if __name__ == '__main__':
    main()
