#!/usr/bin/env python3
"""Keep same-run private evidence across a clean self-hosted checkout."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def identity():
    fields = ['GITHUB_REPOSITORY', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_SHA', 'RUNNER_NAME']
    result = {name:os.environ[name] for name in fields}
    if subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip() != result['GITHUB_SHA']:
        raise ValueError('checkout differs from workflow SHA')
    return result


def key(identity):
    return hashlib.sha256(json.dumps(identity,sort_keys=True).encode()).hexdigest()


def binaries(evidence):
    found = set()
    for path in evidence.rglob('*.log'):
        for text in re.findall(r'target/debug/(?:deps|examples)/[^\s)"\\]+', path.read_text(errors='replace')):
            candidate = (ROOT/text).resolve()
            if candidate.is_file() and os.access(candidate,os.X_OK):
                found.add(candidate)
    for path in evidence.rglob('*provenance*.json'):
        data = json.loads(path.read_text())
        for binary in data.get('build',{}).get('binaries',[]):
            candidate = Path(binary['path']).resolve()
            if sha(candidate) != binary['sha256']:
                raise ValueError('build binary changed before private handoff')
            found.add(candidate)
    return sorted(found)


def seal(root,evidence,stamp):
    dest = root/key(stamp)
    dest.mkdir(parents=True,mode=0o700,exist_ok=False)
    entries = []
    selected = [(p,p.relative_to(ROOT)) for p in sorted(evidence.rglob('*')) if p.is_file()]
    selected += [(p,p.relative_to(ROOT)) for p in binaries(evidence) if p.is_relative_to(ROOT)]
    for index,(source,relative) in enumerate(selected):
        if source.is_symlink():
            raise ValueError('private evidence contains a symlink')
        stored = f'files/{index}'
        target = dest/stored
        target.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
        shutil.copy2(source,target)
        entries.append(dict(path=str(relative),stored=stored,sha256=sha(target)))
    external = [dict(path=str(p),sha256=sha(p)) for p in binaries(evidence) if not p.is_relative_to(ROOT)]
    if not entries:
        raise ValueError('no same-run evidence to seal')
    manifest = dict(v=1,identity=stamp,files=entries,external_binaries=external)
    (dest/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    return manifest


def restore(root,stamp):
    dest = root/key(stamp)
    manifest = json.loads((dest/'manifest.json').read_text())
    if manifest.get('v') != 1 or manifest['identity'] != stamp:
        raise ValueError('private evidence belongs to another run or runner')
    for entry in manifest['files']:
        relative = Path(entry['path'])
        source = (dest/entry['stored']).resolve()
        target = (ROOT/relative).resolve()
        if not source.is_relative_to(dest.resolve()) or not target.is_relative_to((ROOT/'target').resolve()):
            raise ValueError('invalid private evidence path')
        if sha(source) != entry['sha256']:
            raise ValueError('private evidence changed')
    for entry in manifest['external_binaries']:
        if sha(Path(entry['path'])) != entry['sha256']:
            raise ValueError('external build binary changed')
    for entry in manifest['files']:
        target = ROOT/entry['path']
        target.parent.mkdir(parents=True,exist_ok=True)
        shutil.copy2(dest/entry['stored'],target)
        if sha(target) != entry['sha256']:
            raise ValueError('restored evidence differs')
    return manifest


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action',choices=['seal','restore'])
    p.add_argument('--root',type=Path,default=Path.home()/'.local/share/zeron-orch-ci-private')
    p.add_argument('--evidence',type=Path,default=ROOT/'target/orch-verify')
    a = p.parse_args()
    os.umask(0o077)
    stamp = identity()
    result = seal(a.root,a.evidence,stamp) if a.action == 'seal' else restore(a.root,stamp)
    print(json.dumps(dict(private_handoff=a.action,files=len(result['files']),manifest_sha256=sha(a.root/key(stamp)/'manifest.json'))))


if __name__ == '__main__':
    main()
