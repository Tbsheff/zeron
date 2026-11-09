#!/usr/bin/env python3
"""Emit one exact three-product path for the strict source-bound CLI.

Images and action labels must come from source projections. This emitter checks
cardinality, declared macro alignment, enabled products, and source hashes. It
never constructs an abstract successor or chooses a named target action.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil

from check_trace import tla

HERE = Path(__file__).resolve().parent


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def emit(projection, out, *, provenance=None):
    if isinstance(projection, Path):
        projection = json.loads(projection.read_text())
    out = Path(out)
    values = {}
    for group in ('constants', 'reporting_constants', 'transfer_constants'):
        for key, value in projection[group].items():
            if key in values and values[key] != value:
                raise ValueError('operational.constants_conflict')
            values[key] = value
    for key, expected in [('UseNS', False), ('UseCT', False),
                          ('UseReporting', True), ('UseTransferReporting', True), ('UsePL', True)]:
        if values.get(key) is not expected:
            raise ValueError('operational.product_flags')
    count = len(projection['images'])
    if not count:
        raise ValueError('operational.empty_path')
    for prefix in ('', 'reporting_', 'transfer_'):
        if len(projection[prefix + 'images']) != count or len(projection[prefix + 'actions']) != count - 1:
            raise ValueError('operational.path_cardinality')
        if len(projection[prefix + 'source_map']) != count:
            raise ValueError('operational.source_map_cardinality')
    previous = -1
    for index in range(count):
        origins = [projection[p + 'source_map'][index] for p in ('', 'reporting_', 'transfer_')]
        if any(type(origin.get('raw_row')) is not int for origin in origins):
            raise ValueError('operational.raw_row_absent')
        raw = origins[0]['raw_row']
        if any(origin['raw_row'] != raw for origin in origins) or raw < previous:
            raise ValueError('operational.raw_row_alignment')
        if any('macro_substep' not in origin and 'substep' not in origin for origin in origins):
            raise ValueError('operational.macro_source_absent')
        previous = raw
    for target, key in [('Images', 'images'), ('Actions', 'actions'),
                        ('ReportingImages', 'reporting_images'), ('ReportingActions', 'reporting_actions'),
                        ('TransferImages', 'transfer_images'), ('TransferActions', 'transfer_actions')]:
        values[target] = projection[key]
    wrapper = (HERE / 'RuntimeOperationalFullObserved.tla').read_text()
    declared = set(re.findall(r'\b[A-Za-z][A-Za-z0-9_]*\b', wrapper.split('CONSTANTS', 1)[1].split('VARIABLE', 1)[0]))
    if set(values) != declared:
        raise ValueError('operational.constant_domain')
    out.mkdir(parents=True, exist_ok=True)
    copied = {}

    def copy_module(name):
        if name in copied:
            return
        source = HERE / (name + '.tla')
        if not source.is_file():
            return                                                  
        copied[name] = sha(source)
        text = source.read_text()
        shutil.copy(source, out / source.name)
        for extended in re.findall(r'^EXTENDS ([^\n]+)', text, re.M):
            for dependency in extended.split(','):
                copy_module(dependency.strip())
        for dependency in re.findall(r'\bINSTANCE\s+(\w+)', text):
            copy_module(dependency)

    copy_module('RuntimeOperationalFullObserved')
    base_manifest = json.loads((HERE / 'full-spec-source-manifest.json').read_text())
    for entry in base_manifest:
        source = HERE / entry['generated']
        if sha(source) != entry['generated_sha256']:
            raise ValueError('operational.legacy_source_manifest')
    declarations = []
    overrides = []
    for key, value in sorted(values.items()):
        if not re.fullmatch(r'[A-Za-z][A-Za-z0-9_]*', key):
            raise ValueError('operational.constant_name')
        captured = 'Captured_' + key
        declarations.append(captured + ' == ' + tla(value))
        overrides.append(key + ' <- ' + captured)
    source = ('---- MODULE CapturedOperationalFullRuntime ----\n'
              'EXTENDS RuntimeOperationalFullObserved\n' + '\n'.join(declarations) + '\n====\n')
    (out / 'CapturedOperationalFullRuntime.tla').write_text(source)
    config = ('SPECIFICATION Spec\nINVARIANT MappedType\n'
              'PROPERTIES MappedSpec NamedPath ReportingTransitionSafety\n'
              'INVARIANTS NativeLayerEnabled TransferLayerEnabled ReportingSafety TransferSafety\n'
              'OnlyRootEndsRun OrphansClosed NoFalseProcessDeath LossClosesOnlySource\n'
              'BackgroundSurvivesTurn AncestorTerminalClosesForeground\n'
              'CHECK_DEADLOCK TRUE\nCONSTANTS\n' + '\n'.join(overrides) + '\n')
    (out / 'CapturedOperationalFullRuntime.cfg').write_text(config)
    maps = {prefix + 'source_map': projection[prefix + 'source_map']
            for prefix in ('', 'reporting_', 'transfer_')}
    (out / 'source-map-private.json').write_text(json.dumps(maps, indent=2) + '\n')
    manifest = dict(kind='exact-operational-three-product-path', images=count,
                    source_modules=copied, generator_sha256=sha(Path(__file__)),
                    projection_sha256=hashlib.sha256(json.dumps(projection, sort_keys=True, separators=(',', ':')).encode()).hexdigest(),
                    provenance=provenance, full_runtime_conformance=False,
                    implementation_refinement=False, implementation_fair_closure=False)
    (out / 'emission-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest


emit_operational_full_projection = emit


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('projection', type=Path)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    try:
        result = emit(args.projection, args.out)
        print(json.dumps(dict(status='emitted', images=result['images'], full_runtime_conformance=False)))
    except (ValueError, KeyError, TypeError, OSError):
        print(json.dumps(dict(status='blocked', field='operational.emission_inputs')))
        raise SystemExit(2)
