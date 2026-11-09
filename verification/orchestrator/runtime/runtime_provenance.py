"""Validate source/build/capture identity before a full observed-path check."""
import ast
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
HERE=Path(__file__).resolve().parent
HASH=re.compile(r'[0-9a-f]{64}')


def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()


def hashes(value):
    if not isinstance(value,dict) or not value:return False
    return all(isinstance(k,str) and isinstance(v,str) and HASH.fullmatch(v) for k,v in value.items())


def verify_paths(mapping,root):
    root=root.resolve()
    for relative,expected in mapping.items():
        part=Path(relative)
        if part.is_absolute() or '..' in part.parts:raise ValueError('invalid provenance path')
        path=(root/part).resolve()
        if not path.is_relative_to(root) or sha(path)!=expected:raise ValueError('provenance input changed')


def local_python_dependencies(start):
    found=set();pending=[start]
    while pending:
        path=pending.pop()
        if path.name in found:continue
        found.add(path.name)
        for node in ast.walk(ast.parse(path.read_text())):
            names=[]
            if isinstance(node,ast.ImportFrom) and node.module:names=[node.module.split('.')[0]]
            elif isinstance(node,ast.Import):names=[x.name.split('.')[0] for x in node.names]
            for name in names:
                sibling=path.parent/(name+'.py')
                if sibling.exists() and sibling.name not in found:pending.append(sibling)
    return found


def local_tla_dependencies(start):
    """Bind every local module that the common target can execute."""
    found=set();pending=[start]
    while pending:
        path=pending.pop()
        if path.name in found:continue
        found.add(path.name);text=path.read_text()
        names=re.findall(r'\bINSTANCE\s+(\w+)',text)
        for extended in re.findall(r'^EXTENDS ([^\n]+)',text,re.M):
            names.extend(name.strip() for name in extended.split(','))
        for name in names:
            sibling=path.parent/(name+'.tla')
            if sibling.is_file() and sibling.name not in found:pending.append(sibling)
    return found


def validate(receipt_path,captures,jar,source_root,model_root):
    data=json.loads(receipt_path.read_text())
    if data.get('v')!=1:raise ValueError('unsupported provenance version')
    before=data['source_before'];after=data['source_after'];models=data['models']
    if not hashes(before) or before!=after:raise ValueError('source changed during capture')
    if not hashes(models) or data.get('models_before')!=models or data.get('models_after')!=models:
        raise ValueError('model identity not fixed during capture')
    verify_paths(before,source_root);verify_paths(models,model_root)
    listed=subprocess.run(['git','-C',str(source_root),'ls-files','-co','--exclude-standard','-z'],
                          check=True,capture_output=True,timeout=10).stdout.decode().split('\0')
    source_inputs={name for name in listed if name and
        (name.endswith('.rs') or Path(name).name in {'Cargo.toml','Cargo.lock','rust-toolchain','rust-toolchain.toml'}
         or '.cargo/config' in name)}
    if not source_inputs<=set(before):raise ValueError('Rust or build input omitted from provenance')
    names={Path(k).name for k in models}
    required=local_python_dependencies(HERE/'check_full_runtime.py')
    required|=local_tla_dependencies(HERE/'RuntimeOperationalFullObserved.tla')
    required|={'RuntimeState.tla','RuntimeRunDelegationSeams.tla','RuntimeRemoteSeams.tla',
      'RuntimeNativeQuestionSeams.tla','RuntimeCaptureFrames.tla','RuntimeTransferSeams.tla',
      'RuntimeComposition.tla','RuntimeRefinement.tla','RuntimeGlobalRefinement.tla',
      'RuntimeForestCompositionSpec.tla','RuntimeFullForestObserved.tla',
      'full-spec-source-manifest.json','full-spec-interface.json','runtime-vendor-allowlist.json',
      'build_full_spec_stage.py','build_full_spec_wrapper.py','build_runtime_context.py'}
    manifest=json.loads((HERE/'full-spec-source-manifest.json').read_text())
    allowlist=json.loads((HERE/'runtime-vendor-allowlist.json').read_text())
    allowed={x['path'] for x in allowlist['files']}|{'runtime-vendor-allowlist.json'}|{x['source'] for x in manifest}
    if not names<=allowed:raise ValueError('non-source model input in provenance')
    required|={x['generated'] for x in manifest}|{x['source'] for x in manifest}
    if not required<=names:raise ValueError('model dependencies missing from provenance')
    for name in required:
        current=HERE/name
        if current.is_file() and not any(Path(key).name==name and value==sha(current) for key,value in models.items()):
            raise ValueError('executed model or adapter differs from provenance')
    for entry in manifest:
        if not any(Path(key).name==entry['source'] and value==entry['source_sha256'] for key,value in models.items()):
            raise ValueError('original formal source differs from generated manifest')
    build=data['build']
    if not build['command'] or not build['rustc_version'] or not build['cargo_version']:
        raise ValueError('build identity missing')
    if build['env'].get('ORCH_CAPTURE_MECHANISM')!='1':raise ValueError('capture build flag missing')
    if build['lock_sha256']!=sha(source_root/'Cargo.lock') or not build['binaries']:
        raise ValueError('build inputs or executable identities missing')
    for binary in build['binaries']:
        if not binary['target'] or not HASH.fullmatch(binary['sha256']):
            raise ValueError('binary identity missing')
        path=Path(binary['path'])
        if not path.is_absolute():path=source_root/path
        if sha(path)!=binary['sha256']:raise ValueError('executed binary identity differs')
    tools=data['tools']
    java=subprocess.run(['java','-version'],check=True,capture_output=True,text=True,timeout=10)
    java_version=(java.stdout+java.stderr).strip()
    if tools['tla_jar_sha256']!=sha(jar) or tools['java_version']!=java_version or tools['python_version']!=sys.version:
        raise ValueError('checker tool identity differs')
    if not HASH.fullmatch(data['test_receipt_sha256']):raise ValueError('test receipt identity missing')
    test_path=Path(data['test_receipt_path'])
    if not test_path.is_absolute():test_path=source_root/test_path
    if sha(test_path)!=data['test_receipt_sha256']:raise ValueError('actual test receipt differs')
    provided={sha(path):(path.stat().st_size,sum(bool(x.strip()) for x in path.read_bytes().splitlines())) for path in captures}
    if len(provided)!=len(captures):raise ValueError('duplicate capture input')
    selected=[];covered=set()
    for group in data['captures']:
        if not HASH.fullmatch(group['sidecar_sha256']):raise ValueError('export sidecar identity missing')
        group_files={x['sha256']:(x['bytes'],x['rows']) for x in group['files']}
        overlap=set(group_files)&set(provided)
        if overlap:
            if overlap!=set(group_files):raise ValueError('capture group is incomplete')
            if any(provided[key]!=group_files[key] for key in overlap):raise ValueError('capture dimensions differ')
            if not group['scope'] or not isinstance(group['synthetic'],bool):raise ValueError('capture scope missing')
            selected.append(dict(scope=group['scope'],synthetic=group['synthetic']))
            covered|=overlap
    if covered!=set(provided):raise ValueError('capture not bound to build receipt')
    return dict(provenance_sha256=sha(receipt_path),scopes=selected)


def validate_context(context_path, receipt_path, captures):
    """Bind generated finite identities to the exact selected capture bytes."""
    receipt=json.loads(receipt_path.read_text())
    if receipt.get('mapping_context_sha256')!=sha(context_path):
        raise ValueError('mapping context differs from provenance')
    context=json.loads(context_path.read_text())
    source=context.get('provenance',{})
    if source.get('kind')!='capture' or source.get('capture_sha256')!=sorted(sha(p) for p in captures):
        raise ValueError('mapping context capture identity differs')
    generator=HERE/'build_runtime_context.py'
    if source.get('generator')!=generator.name or not generator.is_file() or source.get('generator_sha256')!=sha(generator):
        raise ValueError('mapping context generator differs')
    return context
