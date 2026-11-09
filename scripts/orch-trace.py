#!/usr/bin/env python3
"""Merge clock-free orchestrator traces. Validate each partition with Rust separately."""
import argparse
import heapq
import json
from pathlib import Path


def merge(paths):
    nodes, devices, sends, receives = {}, {}, {}, []
    for path in paths:
        for line_no, raw in enumerate(Path(path).read_text().splitlines(), 1):
            if not raw.strip():
                continue
            row = json.loads(raw)
            if row.get('v') != 1 or not isinstance(row.get('dev'), str):
                raise ValueError(f'{path}:{line_no}: invalid trace header')
            seq = row.get('seq')
            if type(seq) is not int or seq < 0:
                raise ValueError(f'{path}:{line_no}: invalid sequence')
            key = row['dev'], seq
            if key in nodes:
                raise ValueError(f'duplicate sequence {key}')
            nodes[key] = row
            devices.setdefault(key[0], []).append(key)
            msg = row.get('msg')
            if msg:
                if msg.get('dir') not in ('send', 'recv') or not msg.get('id'):
                    raise ValueError(f'{key}: invalid message')
                if msg['dir'] == 'send':
                    sends.setdefault(msg['id'], []).append(key)
                else:
                    receives.append((key, msg['id']))
            for fx in row.get('fx', []):
                sends.setdefault(fx['id'], []).append(key)
    if not nodes:
        raise ValueError('empty trace set')
    edges = {key: set() for key in nodes}
    indegree = dict.fromkeys(nodes, 0)

    def edge(before, after):
        if before != after and after not in edges[before]:
            edges[before].add(after)
            indegree[after] += 1

    for dev, keys in devices.items():
        keys.sort()
        if [k[1] for k in keys] != list(range(len(keys))):
            raise ValueError(f'{dev}: missing sequence or Init')
        if nodes[keys[0]].get('act') != 'Init':
            raise ValueError(f'{dev}: first action must be Init')
        for a, b in zip(keys, keys[1:]):
            edge(a, b)
    for key, msg_id in receives:
        producers = sends.get(msg_id, [])
        by_device = {}
        for producer in producers:
            if producer[0] != key[0]:
                by_device[producer[0]] = min(producer, by_device.get(producer[0], producer))
        remote = list(by_device.values())
        if (nodes[key].get('cmd') or '').startswith('remote-recv:') and not remote:
            raise ValueError(f'{msg_id}: missing remote send trace')
        if len(remote) > 1:
            raise ValueError(f'{msg_id}: ambiguous remote origin')
        for producer in remote:
            edge(producer, key)
    ready = [key for key, n in indegree.items() if n == 0]
    heapq.heapify(ready)
    ordered = []
    while ready:
        key = heapq.heappop(ready)
        ordered.append(nodes[key])
        for nxt in sorted(edges[key]):
            indegree[nxt] -= 1
            if indegree[nxt] == 0:
                heapq.heappush(ready, nxt)
    if len(ordered) != len(nodes):
        raise ValueError('sequence and message edges form a cycle')
    return ordered


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('traces', nargs='+', type=Path)
    p.add_argument('--out', required=True, type=Path)
    args = p.parse_args()
    rows = merge(args.traces)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(''.join(json.dumps(row, sort_keys=True) + '\n' for row in rows))
    print(f'Merged {len(rows)} rows from {len(set(r["dev"] for r in rows))} devices')


if __name__ == '__main__':
    main()
