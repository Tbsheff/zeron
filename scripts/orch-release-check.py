#!/usr/bin/env python3
"""Keep the required branch gate red until the full acceptance evidence exists."""
from pathlib import Path
import hashlib
import json
import os
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from importlib.util import spec_from_file_location, module_from_spec
spec = spec_from_file_location('verify', ROOT / 'scripts/orch-verify.py')
verify = module_from_spec(spec)
spec.loader.exec_module(verify)


def main():
    out = ROOT / 'target/orch-verify/release'
    out.mkdir(parents=True, exist_ok=True)
    dogfood = Path(os.environ.get('ORCH_DOGFOOD_DIR') or ROOT/'target/orch-dogfood')
    rows = []
    ok = True
    formal = ROOT / 'verification/orchestrator/tla'
    (out/'formal-source-sha256.json').write_text(json.dumps({str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(formal.glob('*.tla'))}, indent=2)+'\n')
    for proof in ['PlacementProof', 'PlacementS10Proof']:
        ok = verify.run(f'proof-{proof}', ['tlapm', '--strict', '--nofp', '--threads', '4', f'{proof}.tla'], out, rows, cwd=formal, cap=1200) and ok
    runtime_runner = ROOT/'verification/orchestrator/runtime/RuntimePlanRun.py'
    if runtime_runner.is_file():
        for module in ['RuntimePlanRefinement', 'RuntimePlanLiveness']:
            ok = verify.run(module, [sys.executable, str(runtime_runner), module,
                '--out', str(out/module), '--cap', '300'], out, rows, cap=310) and ok
    else:
        rows.append(dict(name='joint runtime safety and fair liveness', result='MISSING'))
        print('Blocked: joint runtime safety and fair liveness runner is missing')
        ok = False
    full_proof = formal / 'proof-wrapper/FullDelegationS3Proof.tla'
    full_checker = ROOT / 'scripts/orch-trace-full.py'
    required = [(full_proof, 'full S3 proof'), (full_checker, 'full ZeronImpl runtime conformance')]
    for path, label in required:
        if not path.is_file():
            print(f'Blocked: {label} is missing')
            rows.append(dict(name=label, result='MISSING', path=str(path.relative_to(ROOT))))
            ok = False
    if full_proof.is_file():
        proof_out = out / 's3-proof'
        generated = verify.run('s3-source-generate', [sys.executable,
            str(formal/'proof-wrapper/generate_s3_source.py'), '--source-dir', str(formal),
            '--output-dir', str(proof_out)], out, rows)
        ok = generated and ok
        if generated:
            checked = verify.run('s3-source-check', [sys.executable,
                str(formal/'proof-wrapper/generate_s3_source.py'), '--source-dir', str(formal),
                '--output-dir', str(proof_out), '--check'], out, rows)
            ok = checked and ok
            import shutil
            shutil.copy2(full_proof, proof_out / full_proof.name)
            ok = verify.run('full-s3', ['tlapm', '--strict', '--nofp', '--threads', '4', full_proof.name], out, rows,
                            cwd=proof_out) and ok
    if full_checker.is_file():
        ok = verify.run('full-conformance', [sys.executable, str(full_checker),
                        '--integration', str(ROOT/'target/orch-verify/mechanism')], out, rows, cap=915) and ok
    ok = verify.run('dogfood-week', [sys.executable, str(ROOT/'scripts/orch-dogfood.py'), 'status', '--out', str(dogfood)], out, rows) and ok
    (out/'receipts.json').write_text(json.dumps(rows, indent=2)+'\n')
    return 0 if ok else 1


if __name__ == '__main__':
    raise SystemExit(main())
