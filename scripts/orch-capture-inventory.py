#!/usr/bin/env python3
"""Require fresh raw capture coverage for each successful integration test."""
import argparse
import json
from pathlib import Path
import re


def check(log, root, test_list=None):
    expected = set(re.findall(r'^test ([^ ]+) \.\.\. ok$', log.read_text(), re.M))
    if test_list is not None:
        selected = re.findall(r'^(.+): test$', test_list.read_text(), re.M)
        if len(selected) != len(set(selected)):
            raise ValueError('ambiguous duplicate test names in capture inventory')
        if not selected or expected != set(selected):
            raise ValueError('selected integration tests did not all pass')
    if not expected:
        raise ValueError('integration log contains no successful tests')
    covered = set()
    groups = []
    declared = set()
    for path in sorted(root.rglob('capture.json')):
        row = json.loads(path.read_text())
        name = row['test_name']
        if name not in expected:
            raise ValueError(f'capture names unselected test: {name}')
        if row.get('capture') == 'skipped':
            if row.get('reason') != 'orchestrator_disabled' or not re.search(r'(^|_)flag_off(_|$)', name):
                raise ValueError(f'unsupported capture exemption: {name}')
        else:
            files = row.get('filenames', row.get('files', []))
            if not files:
                raise ValueError(f'capture has no devices: {name}')
            for file in files:
                source = path.parent / file
                if source.parent != path.parent or source.suffix != '.jsonl' or not source.is_file():
                    raise ValueError(f'invalid capture file for {name}')
                declared.add(source)
                lines = source.read_text().splitlines()
                if not lines or json.loads(lines[0]).get('v') != 2:
                    raise ValueError(f'missing v2 capture for {name}')
        covered.add(name)
        groups.append(dict(test_name=name, manifest=str(path.relative_to(root)), capture=row.get('capture', 'present')))
    orphan = set(root.rglob("*.jsonl")) - declared
    if orphan:
        raise ValueError("capture files lack test provenance")
    missing = expected - covered
    if missing:
        raise ValueError('missing capture tests: ' + ', '.join(sorted(missing)))
    return dict(tests=len(expected), groups=groups)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-list', type=Path, required=True)
    parser.add_argument('--test-log', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    result = check(args.test_log, args.root, args.test_list)
    args.out.write_text(json.dumps(result, indent=2)+'\n')
    print(f'PASS: capture inventory covers {result["tests"]} tests')
