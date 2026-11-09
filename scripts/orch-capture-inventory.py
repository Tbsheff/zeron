#!/usr/bin/env python3
"""Require fresh raw capture coverage for each successful integration test."""
import argparse
import json
from pathlib import Path
import re


def check(log, root):
    expected = set(re.findall(r'^test ([^ ]+) \.\.\. ok$', log.read_text(), re.M))
    if not expected:
        raise ValueError('integration log contains no successful tests')
    covered = set()
    groups = []
    for path in sorted(root.rglob('capture.json')):
        row = json.loads(path.read_text())
        name = row['test_name']
        if name not in expected:
            raise ValueError(f'capture names unselected test: {name}')
        if row.get('capture') == 'skipped':
            if row.get('reason') != 'orchestrator_disabled' or not name.startswith('flag_off_'):
                raise ValueError(f'unsupported capture exemption: {name}')
        else:
            files = row.get('filenames', row.get('files', []))
            if not files:
                raise ValueError(f'capture has no devices: {name}')
            for file in files:
                source = path.parent / file
                if source.parent != path.parent or source.suffix != '.jsonl' or not source.is_file():
                    raise ValueError(f'invalid capture file for {name}')
                lines = source.read_text().splitlines()
                if not lines or json.loads(lines[0]).get('v') != 2:
                    raise ValueError(f'missing v2 capture for {name}')
        covered.add(name)
        groups.append(dict(test_name=name, manifest=str(path.relative_to(root)), capture=row.get('capture', 'present')))
    missing = expected - covered
    if missing:
        raise ValueError('missing capture tests: ' + ', '.join(sorted(missing)))
    return dict(tests=len(expected), groups=groups)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-log', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    result = check(args.test_log, args.root)
    args.out.write_text(json.dumps(result, indent=2)+'\n')
    print(f'PASS: capture inventory covers {result["tests"]} tests')
