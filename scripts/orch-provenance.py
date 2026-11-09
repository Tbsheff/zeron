#!/usr/bin/env python3
"""Bind a frozen build and its exported captures to exact source bytes."""
import argparse
import hashlib
import json
import os
import plistlib
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_hashes():
    files = subprocess.check_output(['git', 'ls-files', '-co', '--exclude-standard', '-z'], cwd=ROOT).decode().split('\0')
    return {name: sha(ROOT/name) for name in sorted(set(files)) if name and
            (name.endswith('.rs') or Path(name).name in {'Cargo.toml', 'Cargo.lock', 'rust-toolchain', 'rust-toolchain.toml'} or '.cargo/config' in name or name.endswith('.sh') or name.startswith('scripts/orch') or name in {'.github/workflows/orchestrator-verify.yml', 'docs/orchestrator-v2/TRY.md'})}


def model_hashes(root):
    runtime = root/'runtime'
    allowlist = runtime/'runtime-vendor-allowlist.json'
    entries = json.loads(allowlist.read_text())['files']
    selected = {allowlist}
    for entry in entries:
        path = (runtime/entry['path']).resolve()
        if not path.is_relative_to(runtime.resolve()) or sha(path) != entry['sha256']:
            raise ValueError('vendored model differs from source allowlist')
        selected.add(path)
    manifest = json.loads((runtime/'full-spec-source-manifest.json').read_text())
    for entry in manifest:
        path = (root/'tla'/entry['source']).resolve()
        if not path.is_relative_to((root/'tla').resolve()) or sha(path) != entry['source_sha256']:
            raise ValueError('original formal source differs from linked source manifest')
        selected.add(path)
    root = root.resolve()
    return {str(path.resolve().relative_to(root)):sha(path) for path in sorted(selected)}


def version(command):
    return subprocess.check_output(command, stderr=subprocess.STDOUT, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['begin', 'built', 'app', 'finish', 'context', 'validate'])
    parser.add_argument('--receipt', type=Path, required=True)
    parser.add_argument('--model-root', type=Path, default=ROOT/'verification/orchestrator')
    parser.add_argument('--build-log', type=Path)
    parser.add_argument('--build-only', action='store_true', help='Record only the app build; this cannot prove model conformance')
    parser.add_argument('--bundle', type=Path)
    parser.add_argument('--capture-root', type=Path, action='append', default=[])
    parser.add_argument('--test-receipt', type=Path)
    parser.add_argument('--jar', type=Path)
    parser.add_argument('--context', type=Path)
    parser.add_argument('--command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.action == 'begin':
        models = {} if args.build_only else model_hashes(args.model_root)
        data = dict(v=1, scope='app-build-only' if args.build_only else 'runtime-conformance', source_before=source_hashes(), models_before=models, models=models)
    else:
        data = json.loads(args.receipt.read_text())
        if source_hashes() != data['source_before'] or (data.get('scope') != 'app-build-only' and model_hashes(args.model_root) != data['models_before']):
            raise ValueError('source or model changed during build/capture')
    if args.action == 'built':
        if not args.build_log or not args.command:
            parser.error('built requires --build-log and --command')
        binaries = {}
        for line in args.build_log.read_text().splitlines():
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if row.get('reason') == 'compiler-artifact' and row.get('executable'):
                path = Path(row['executable']).resolve()
                binaries[str(path)] = dict(target=row['target']['name'], path=str(path), sha256=sha(path))
        if not binaries:
            raise ValueError('no actual test executables in Cargo build receipt')
        data['build'] = dict(command=args.command, env={k:os.environ[k] for k in
            ('CARGO_BUILD_JOBS', 'ORCH_CAPTURE_MECHANISM', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS') if k in os.environ},
            rustc_version=version(['rustc','--version','--verbose']), cargo_version=version(['cargo','--version']),
            lock_sha256=sha(ROOT/'Cargo.lock'), binaries=list(binaries.values()))
    if args.action == 'app':
        if data.get('scope') == 'app-build-only' and 'build' not in data:
            if not args.command:
                parser.error('build-only app requires the exact --command')
            data['build'] = dict(command=args.command, env={k:os.environ[k] for k in
                ('CARGO_BUILD_JOBS','ZERON_ORCH_V2','ORCH_CAPTURE_MECHANISM','ZERON_DEV_DATA_DIR',
                 'ZERON_DEV_IPC_PORT','ZERON_DEV_BUNDLE_ID','ZERON_DEV_BUILD_ONLY','RUSTFLAGS') if k in os.environ},
                rustc_version=version(['rustc','--version','--verbose']),cargo_version=version(['cargo','--version']),
                lock_sha256=sha(ROOT/'Cargo.lock'),binaries=[])
        if not args.bundle or not args.test_receipt or (not args.jar and data.get('scope') != 'app-build-only'):
            parser.error('app requires --bundle, --test-receipt stage log, and --jar')
        info = args.bundle/'Contents/Info.plist'
        meta = plistlib.loads(info.read_bytes())
        flags = meta['LSEnvironment']
        if (meta['CFBundleIdentifier'] != 'sh.zeron.app.orch-v2' or
            Path(flags['ZERON_DATA_DIR']).resolve() != (Path.home()/'.zeron-orch-v2').resolve() or
            str(flags['ZERON_IPC_PORT']) != '27872' or
            flags.get('ZERON_ORCH_V2') != '1' or flags.get('ORCH_CAPTURE_MECHANISM') != '1'):
            raise ValueError('app does not use the authorized isolated profile and flags')
        subprocess.run(['codesign','--verify','--deep','--strict',str(args.bundle)],check=True)
        for name in ('zeron','Zeron Engine'):
            path = (args.bundle/'Contents/MacOS'/name).resolve()
            data['build']['binaries'].append(dict(target=name,path=str(path),sha256=sha(path)))
        data.update(source_after=source_hashes(), models_after={} if data.get('scope') == 'app-build-only' else model_hashes(args.model_root), captures=[],
            test_receipt_path=str(args.test_receipt.resolve()), test_receipt_sha256=sha(args.test_receipt),
            tools=dict(python_version=sys.version, **(dict(tla_jar_sha256=sha(args.jar),java_version=version(['java','-version'])) if args.jar else {})),
            app=dict(bundle=str(args.bundle.resolve()),info_plist_sha256=sha(info),bundle_id=meta['CFBundleIdentifier'],
                     profile=flags['ZERON_DATA_DIR'],ipc_port=str(flags['ZERON_IPC_PORT']),
                     build_only=True))
    if args.action == 'finish':
        if data.get('scope') == 'app-build-only':
            raise ValueError('build-only receipt cannot certify runtime conformance')
        for binary in data['build']['binaries']:
            if sha(Path(binary['path'])) != binary['sha256']:
                raise ValueError('test executable changed during capture')
        if not args.test_receipt or not args.jar or not args.capture_root:
            parser.error('finish requires --test-receipt, --jar and --capture-root')
        groups = []
        for root in args.capture_root:
            for sidecar in sorted(root.rglob('capture.json')):
                meta = json.loads(sidecar.read_text())
                if meta.get('capture') == 'skipped':
                    continue
                files = []
                for name in meta.get('filenames', meta.get('files', [])):
                    path = (sidecar.parent/name).resolve()
                    if path.parent != sidecar.parent.resolve():
                        raise ValueError('capture path escapes group')
                    files.append(dict(sha256=sha(path), bytes=path.stat().st_size,
                                      rows=sum(bool(line.strip()) for line in path.read_bytes().splitlines())))
                if not files:
                    raise ValueError('capture group has no raw files')
                groups.append(dict(sidecar_sha256=sha(sidecar), files=files,
                                   test_name=meta['test_name'], scope=meta.get('scope','engine-integration'),
                                   synthetic=meta.get('synthetic', True)))
        data.update(source_after=source_hashes(), models_after=model_hashes(args.model_root), captures=groups,
                    test_receipt_path=str(args.test_receipt.resolve()), test_receipt_sha256=sha(args.test_receipt),
                    tools=dict(tla_jar_sha256=sha(args.jar), java_version=version(['java','-version']), python_version=sys.version))
    if args.action == 'finish' and args.context:
        data['mapping_context_sha256'] = sha(args.context)
    if args.action == 'context':
        if data.get('scope') != 'runtime-conformance' or not args.context or 'captures' not in data:
            raise ValueError('context requires finished runtime capture provenance')
        for binary in data['build']['binaries']:
            if sha(Path(binary['path'])) != binary['sha256']:
                raise ValueError('test executable changed after capture')
        if sha(Path(data['test_receipt_path'])) != data['test_receipt_sha256']:
            raise ValueError('test receipt changed after capture')
        data['mapping_context_sha256'] = sha(args.context)
    if args.action == 'validate':
        if data.get('scope') != 'runtime-conformance' or not args.jar or not args.capture_root:
            raise ValueError('validate requires runtime captures and tool identity')
        captures = []
        for root in args.capture_root:
            for sidecar in root.rglob('capture.json'):
                meta = json.loads(sidecar.read_text())
                captures.extend((sidecar.parent/name).resolve() for name in meta['filenames'])
        sys.path.insert(0, str(args.model_root/'runtime'))
        from runtime_provenance import validate
        validate(args.receipt, captures, args.jar, ROOT, args.model_root)
        return
    args.receipt.parent.mkdir(parents=True, exist_ok=True)
    args.receipt.write_text(json.dumps(data, indent=2)+'\n')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.SubprocessError):
        print(json.dumps(dict(check='runtime-provenance', status='fail', field='input_identity')))
        raise SystemExit(1)
