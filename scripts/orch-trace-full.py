#!/usr/bin/env python3
"""Check complete captured runtime groups with the source-linked model."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import signal
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed, TimeoutError

ROOT = Path(__file__).resolve().parents[1]


class StageRunner:
    def __init__(self, deadline, cap=300):
        self.deadline = deadline
        self.cap = cap
        self.processes = set()
        self.stopped = threading.Event()

    def clean(self, process):
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            pass
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()

    def cancel(self):
        self.stopped.set()
        for process in list(self.processes):
            self.clean(process)

    def run(self, command):
        remaining = self.deadline-time.monotonic()
        if remaining <= 0 or self.stopped.is_set():
            return dict(exit_code=124, status='not-run')
        process = subprocess.Popen(command, cwd=ROOT, start_new_session=True)
        self.processes.add(process)
        try:
            if self.stopped.is_set():
                self.clean(process)
                return dict(exit_code=124, status='not-run')
            try:
                code = process.wait(timeout=min(self.cap, max(0.001, self.deadline-time.monotonic())))
                if self.stopped.is_set():
                    return dict(exit_code=124, status='timeout')
                return dict(exit_code=code, status='pass' if code == 0 else 'timeout' if code == 124 else 'fail')
            except subprocess.TimeoutExpired:
                self.clean(process)
                return dict(exit_code=124, status='timeout')
        finally:
            self.processes.discard(process)


def main():
    started = time.monotonic()
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--integration', type=Path, required=True)
    p.add_argument('--provenance', type=Path, default=os.environ.get('ORCH_INTEGRATION_PROVENANCE'))
    p.add_argument('--jar', type=Path, default=os.environ.get('TLA_JAR', str(ROOT/'verification/orchestrator/tools/tla2tools.jar')))
    p.add_argument('--out', type=Path, default=ROOT/'target/orch-verify/full-private')
    p.add_argument('--seconds', type=float, default=900)
    p.add_argument('--stage-seconds', type=float, default=300)
    a = p.parse_args()
    if not 0 < a.seconds <= 900 or not 0 < a.stage_seconds <= 300:
        p.error('limits must be positive and at most 900 total / 300 per stage')
    a.provenance = a.provenance or a.integration.parent/'integration-provenance.json'
    if not a.provenance.is_file():
        print(json.dumps(dict(check='full-runtime', status='blocked', field='integration.provenance_missing')))
        return 1
    data = json.loads(a.provenance.read_text())
    required = {'orch_v2', 'orch_v2_env', 'orch_remote', 'orch_questions_native', 'orch_host_gate'}
    if data.get('scope') != 'runtime-conformance' or not required <= {b['target'] for b in data['build']['binaries']}:
        raise ValueError('integration test binaries are not bound')
    test_log = Path(data['test_receipt_path']).read_text()
    if 'FAILED' in test_log or 'test result: ok.' not in test_log:
        raise ValueError('integration tests did not pass')
    groups = []
    for sidecar in sorted(a.integration.glob('*/capture.json')):
        meta = json.loads(sidecar.read_text())
        if meta.get('capture') != 'present':
            continue
        if meta.get('scope') in ('unit', 'driver-component'):
            continue
        if f"test {meta['test_name']} ... ok" not in test_log:
            raise ValueError('capture producer test did not pass')
        files = meta.get('filenames')
        if not files or not isinstance(files, list):
            raise ValueError('complete capture group has no file inventory')
        captures = []
        for name in files:
            path = (sidecar.parent/name).resolve()
            if path.parent != sidecar.parent.resolve() or not path.is_file():
                raise ValueError('capture inventory is incomplete')
            captures.append(path)
        groups.append(captures)
    if not groups:
        print(json.dumps(dict(check='full-runtime', status='blocked', field='integration.complete_groups_missing')))
        return 1
    a.out.mkdir(parents=True, exist_ok=True)
    a.out.chmod(0o700)
    runtime = ROOT/'verification/orchestrator/runtime'
    runner = StageRunner(started+a.seconds, a.stage_seconds)
    results = [dict(group=i, stage='not-started', exit_code=124, status='not-run') for i in range(len(groups))]

    receipt_lock = threading.RLock()

    def save():
        with receipt_lock:
            write_receipt()

    def write_receipt():
        counts = {status:sum(r['status']==status for r in results) for status in ('pass','fail','timeout','not-run','running')}
        if not counts['running']:
            del counts['running']
        receipt = dict(groups=results, counts=counts, ran=len(results)-counts['not-run'], passed=all(r['status']=='pass' for r in results), seconds=round(time.monotonic()-started, 2), limit_seconds=a.seconds, stage_limit_seconds=a.stage_seconds, workers=2)
        temporary = a.out/'receipt.tmp'
        temporary.write_text(json.dumps(receipt, indent=2)+'\n')
        temporary.replace(a.out/'receipt.json')

    def group(index, captures):
        if runner.stopped.is_set() or time.monotonic() >= runner.deadline:
            return results[index]
        with receipt_lock:
            results[index] = dict(group=index, stage='validate', exit_code=124, status='running')
            save()
        private = a.out/f'group-{index:03d}'
        private.mkdir(parents=True, exist_ok=True)
        private.chmod(0o700)
        receipt = private/'provenance.json'
        receipt.write_bytes(a.provenance.read_bytes())
        receipt.chmod(0o600)
        context = private/'context.json'
        commands = [
            ('validate', [sys.executable, str(ROOT/'scripts/orch-provenance.py'), 'validate', '--receipt', str(a.provenance), '--capture-root', str(captures[0].parent), '--jar', str(a.jar)]),
            ('context-generate', [sys.executable, str(runtime/'build_runtime_context.py'), *map(str, captures), '--out', str(context)]),
            ('context-bind', [sys.executable, str(ROOT/'scripts/orch-provenance.py'), 'context', '--receipt', str(receipt), '--context', str(context)]),
            ('strict-runtime', [sys.executable, str(runtime/'check_full_runtime.py'), *map(str, captures), '--context', str(context), '--provenance', str(receipt), '--jar', str(a.jar), '--out', str(private), '--source-root', str(ROOT), '--model-root', str(runtime.parent)])]
        result = dict(exit_code=124, status='not-run')
        ran = False
        for stage, command in commands:
            result = runner.run(command)
            if result['status'] == 'not-run' and ran:
                result = dict(exit_code=124, status='timeout')
            ran = ran or result['status'] != 'not-run'
            if result['exit_code']:
                break
        return dict(group=index, stage=stage, **result)

    interrupted = []
    def stop(signum, frame):
        interrupted.append(signum)
        runner.cancel()
        raise KeyboardInterrupt
    old = {sig:signal.signal(sig, stop) for sig in (signal.SIGTERM, signal.SIGINT)}
    pool = ThreadPoolExecutor(max_workers=2)
    futures = {}
    save()
    try:
        futures = {pool.submit(group, i, paths):i for i, paths in enumerate(groups)}
        try:
            for future in as_completed(futures, timeout=max(0.001, runner.deadline-time.monotonic())):
                index = futures[future]
                with receipt_lock:
                    try:
                        results[index] = future.result()
                    except (ValueError, KeyError, OSError):
                        results[index] = dict(group=index, stage='input', exit_code=1, status='fail')
                    save()
        except TimeoutError:
            runner.cancel()
    except KeyboardInterrupt:
        pass
    finally:
        runner.cancel()
        pool.shutdown(wait=True, cancel_futures=True)
        for future, index in futures.items():
            if future.done() and not future.cancelled():
                try:
                    results[index] = future.result()
                except (ValueError, KeyError, OSError):
                    results[index] = dict(group=index, stage='input', exit_code=1, status='fail')
        save()
        for sig, handler in old.items():
            signal.signal(sig, handler)
    if interrupted:
        return 128+interrupted[0]
    return next((r['exit_code'] for r in results if r['exit_code']), 0)



if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (ValueError, KeyError, OSError):
        print(json.dumps(dict(check='full-runtime', status='blocked', field='integration.input_identity_invalid')))
        raise SystemExit(1)
