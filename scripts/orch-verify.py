#!/usr/bin/env python3
"""Strict local/push verification. Every child writes a log and a receipt."""
import hashlib
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
FORMAL = ROOT / 'verification/orchestrator/tla'


def run(name, command, out, receipts, cwd=ROOT, cap=300, env=None):
    start = time.monotonic()
    log = out / f'{name}.log'
    result = 'FAIL'
    with log.open('w') as stream:
        try:
            proc = subprocess.Popen(command, cwd=cwd, stdout=stream, stderr=subprocess.STDOUT,
                                    env=env, start_new_session=True)
            try:
                result = 'PASS' if proc.wait(timeout=cap) == 0 else 'FAIL'
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
                    proc.wait()
                result = 'TIMEOUT'
        except OSError as error:
            stream.write(str(error) + '\n')
    row = dict(name=name, result=result, seconds=round(time.monotonic()-start, 2),
               command=command, log=os.path.relpath(log, ROOT))
    receipts.append(row)
    (out / 'receipts.json').write_text(json.dumps(receipts, indent=2) + '\n')
    print(f'{result}: {name} ({row["seconds"]}s)', flush=True)
    return result == 'PASS'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('tier', choices=['fast', 'nightly', 'traces', 'integration'])
    parser.add_argument('--out', type=Path, default=ROOT / 'target/orch-verify')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    receipts = []
    if args.tier in ('fast', 'integration'):
        sources = sorted((ROOT/'crates').rglob('*.rs')) + [ROOT/'Cargo.lock', ROOT/'rust-toolchain.toml']
        (out/'rust-source-sha256.json').write_text(json.dumps({str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sources if p.is_file()}, indent=2)+'\n')
    env = dict(os.environ, CARGO_BUILD_JOBS='3', FAST_WORKERS='4', WORKERS='4',
               TLC_JAR=os.environ.get('TLA_JAR', str(ROOT / 'verification/orchestrator/tools/tla2tools.jar')),
               ORCH_TRACE_OUT=str(out / 'traces'), ORCH_CAPTURE_MECHANISM='1',
               ORCH_MECHANISM_OUT=str(out / 'mechanism'))
    ok = True

    def check(name, cmd, **kwargs):
        nonlocal ok
        passed = run(name, cmd, out, receipts, env=kwargs.pop("env", env), **kwargs)
        ok = passed and ok
        return passed

    if args.tier in ('fast', 'traces', 'integration'):
        if args.tier in ('fast', 'integration'):
            shutil.rmtree(out / 'traces', ignore_errors=True)
            shutil.rmtree(out / 'mechanism', ignore_errors=True)
            shutil.rmtree(out / 'mechanism-unit', ignore_errors=True)
            shutil.rmtree(out / 'mechanism-driver', ignore_errors=True)
            check('provenance-source-tests', [sys.executable, 'scripts/orch-provenance-test.py'])
            check('private-handoff-tests', [sys.executable, 'scripts/orch-ci-private-test.py'])
            check('dogfood-tests', [sys.executable, 'scripts/orch-dogfood-test.py'])
            check('trace-merge-tests', [sys.executable, '-m', 'unittest', 'discover', '-s', 'scripts/tests', '-p', 'test_orch_trace.py'])
            check('core', ['cargo', 'test', '--locked', '-p', 'zeron-orch'], cap=1800)
            check('engine', ['cargo', 'test', '--locked', '-p', 'zeron-engine', '--lib', 'orchestrator::'], cap=1800, env=dict(env, ORCH_MECHANISM_OUT=str(out/'mechanism-unit')))
            for name, scope in [('engine-stop', 'sessions::stop_tests'),
                                ('engine-update-lifecycle', 'update_trial::'),
                                ('engine-native-path', 'native_paths_keep_reused_local_ids_distinct'),
                                ('engine-question-issuance', 'retained_input_issuance_cannot_cross_attempt_rebind'),
                                ('engine-source-steer', 'source_steer_pins_callbacks_before_drive_consumes_boundaries'),
                                ('engine-transfer', 'transfer'), ('engine-exact-save', 'exact_save')]:
                check(name, ['cargo', 'test', '--locked', '-p', 'zeron-engine', '--lib', scope,
                             '--', '--test-threads=3'], cap=1800,
                      env=dict(env, ORCH_MECHANISM_OUT=str(out/'mechanism-unit')))
            check('harness-all-targets', ['cargo', 'check', '--locked', '-p', 'zeron-harness', '--all-targets'], cap=1800)
            check('harness-all-lib', ['cargo', 'test', '--locked', '-p', 'zeron-harness', '--lib', '--', '--test-threads=3'], cap=1800)
            check('harness-strict-resume', ['cargo', 'test', '--locked', '-p', 'zeron-harness', 'strict_resume', '--', '--test-threads=3'], cap=1800)
            integration = ['cargo', 'test', '--locked', '-p', 'zeron-engine', '--test', 'orch_v2', '--test', 'orch_v2_env', '--test', 'orch_remote', '--test', 'orch_questions_native', '--test', 'orch_host_gate']
            check('driver-components', ['cargo', 'test', '--locked', '-p', 'zeron-engine', '--test', 'orch_run_delegation_capture'], cap=1800, env=dict(env, ORCH_MECHANISM_OUT=str(out/'mechanism-driver')))
            check('capture-test-list', integration + ['--', '--list'], cap=1800)
            check('engine-integration', integration + ['--no-fail-fast', '--', '--test-threads=3'], cap=1800)
            check('capture-inventory', [sys.executable, 'scripts/orch-capture-inventory.py', '--test-log', str(out/'engine-integration.log'), '--test-list', str(out/'capture-test-list.log'), '--root', str(out/'mechanism'), '--out', str(out/'capture-inventory.json')])
            check('trace-runner-build', ['cargo', 'build', '--locked', '-p', 'zeron-engine', '--example', 'orch_trace'], cap=1800)
        if args.tier == 'fast':
            check('formal-fast', ['node', 'Fast_run.mjs'], cwd=FORMAL, cap=7200)
            (out / 'formal-source-sha256.json').write_text(json.dumps({str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(FORMAL.glob('*.tla'))}, indent=2) + '\n')
            for proof in ['ResultS3Proof']:
                check(f'proof-{proof}', ['tlapm', '--strict', '--nofp', '--threads', '4', f'{proof}.tla'], cwd=FORMAL)
            run('kani-fast', [sys.executable, 'scripts/orch-kani.py', 'fast'], out, receipts, env=env, cap=960)
            receipts[-1]['required'] = False
            (out/'receipts.json').write_text(json.dumps(receipts, indent=2)+'\n')
        trace_root = out / 'traces'
        recordings = [[path] for path in sorted(trace_root.glob('*.jsonl'))]
        flat = trace_root / 'orch-trace'
        recordings += [[path] for path in sorted(flat.glob('*.jsonl'))]
        for group in sorted(trace_root.rglob('*')):
            if group.is_dir() and group != flat:
                paths = sorted(group.glob('*.jsonl'))
                if paths:
                    recordings.append(paths)
        if not recordings:
            check('missing-integration-traces', [sys.executable, '-c', 'raise SystemExit("no integration JSONL traces exported")'])
        for idx, paths in enumerate(recordings):
            prefix = f'trace-{idx:03}'
            merged = out / f'{prefix}.jsonl'
            if not check(prefix+'-merge', [sys.executable, 'scripts/orch-trace.py', '--out', str(merged), *map(str, paths)]):
                continue
            check(prefix+'-rust', [str(ROOT / 'target/debug/examples/orch_trace'), str(merged)])
            model_dir = out / prefix
            check(prefix+'-emit', [sys.executable, 'scripts/orch-trace-model.py', str(merged), '--out', str(model_dir)])
            shutil.copy2(FORMAL / 'TraceLedger.tla', model_dir / 'TraceLedger.tla')
            check(prefix+'-tla', [str(FORMAL.parent / 'node_modules/.bin/tla-ts'), '-workers', '4',
                                  '-config', 'TraceReplay.cfg', 'TraceReplay.tla'], cwd=model_dir)
    else:
        jar = env['TLC_JAR']
        jobs = [('RunLifecycle', 'MC_RL_N1'), ('Delegation', 'MC_Del_N1'),
                ('ContextTransfer', 'CT_Switch_N1'), ('ContextTransfer', 'CT_Fork_N1'),
                ('Placement', 'MC_PL_BaseN1'), ('NativeSubagents', 'NS_N1_Subs'),
                ('ZI_Refinement', 'ZI_Ref_N1')]
        for module, cfg in jobs:
            check(cfg, [str(FORMAL.parent / 'node_modules/.bin/tla-ts'), '-workers', '4',
                        '-deadlock', '-config', cfg+'.cfg', module+'.tla'], cwd=FORMAL)
        check('composition-million', ['java', '-Xmx4g', '-cp', jar, 'tlc2.TLC',
                                     '-simulate', 'num=1000000', '-depth', '100', '-workers', '4',
                                     '-deadlock', '-metadir', str(out / 'tlc-meta'),
                                     '-config', 'COMP_Sim.cfg', 'Composition.tla'], cwd=FORMAL)
    summary = '# Orchestrator verification\n\n' + '\n'.join(
        f'- {r["result"]}: {r["name"]} ({r["seconds"]} s); {r["log"]}' for r in receipts)
    (out / 'summary.md').write_text(summary + '\n')
    return 0 if ok else 1


if __name__ == '__main__':
    raise SystemExit(main())
