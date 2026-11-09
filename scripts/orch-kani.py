#!/usr/bin/env python3
"""Run short Kani checks or require fresh source-linked evidence for every scope."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('verify', ROOT/'scripts/orch-verify.py')
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)
FAST = ['s1_restart_bound3', 's3_stop_bound3', 's5a_narrow']


def source_digest():
    digest = hashlib.sha256()
    files = set((ROOT/'crates/orch/src').rglob('*.rs')) | set((ROOT/'crates').rglob('Cargo.toml'))
    files.update([ROOT/'Cargo.toml', ROOT/'Cargo.lock', ROOT/'rust-toolchain.toml', ROOT/'scripts/orch-kani.py', ROOT/'scripts/orch-verify.py'])
    files.update(p for p in (ROOT/'.cargo').rglob('*') if p.is_file())
    for p in sorted(files):
        if p.is_file():
            digest.update(str(p.relative_to(ROOT)).encode()+b'\0'+p.read_bytes())
    return digest.hexdigest()


def scopes():
    source = (ROOT/'crates/orch/src/proofs.rs').read_text()
    direct = re.findall(r'#\[kani::proof\](?:\s*#\[[^\]]+\])*\s*fn\s+(\w+)', source)
    macros = re.findall(r'(?:wake_boundary_)?harness!\(\s*(\w+)\s*,', source)
    return sorted(set(direct + macros))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('mode', choices=['fast', 'status', 'run-all'])
    args = p.parse_args()
    current = source_digest()
    out = Path(os.environ.get('ORCH_KANI_EVIDENCE_DIR') or ROOT/'target/orch-verify/kani') / current
    out.mkdir(parents=True, exist_ok=True)
    receipts = []
    required = scopes()
    if args.mode != 'status':
        for name in FAST if args.mode == 'fast' else scopes():
            command = ['cargo', 'kani', '-p', 'zeron-orch', '--harness', name, '--jobs', '3',
                       '-Z', 'unstable-options', '--harness-timeout', '290s', '--output-format', 'terse']
            passed = verify.run(name, command, out, receipts, cap=300,
                                env=dict(os.environ, CARGO_BUILD_JOBS='3'))
            log = out/f'{name}.log'
            receipt = dict(name=name, source_sha256=current, result='PASS' if passed else 'UNPROVED',
                           command=command, log_sha256=hashlib.sha256(log.read_bytes()).hexdigest())
            (out/f'{name}.json').write_text(json.dumps(receipt, indent=2)+'\n')
    rows = []
    for name in (required if args.mode == 'status' else FAST if args.mode == 'fast' else scopes()):
        path = out/f'{name}.json'
        log = out/f'{name}.log'
        result = 'UNPROVED'
        if name not in scopes():
            result = 'MISSING_HARNESS'
        elif path.is_file() and log.is_file():
            entry = json.loads(path.read_text())
            valid = (entry.get('source_sha256') == current and entry.get('name') == name
                     and entry.get('result') == 'PASS'
                     and hashlib.sha256(log.read_bytes()).hexdigest() == entry.get('log_sha256')
                     and 'VERIFICATION:- SUCCESSFUL' in log.read_text())
            result = 'PASS' if valid else 'UNPROVED'
        rows.append(dict(name=name, result=result))
        print(f'{result}: {name}')
    (out/f'{args.mode}-summary.json').write_text(json.dumps(dict(source_sha256=current, scopes=rows), indent=2)+'\n')
    return 0 if args.mode == 'status' or all(r['result']=='PASS' for r in rows) else 1


if __name__ == '__main__':
    raise SystemExit(main())
