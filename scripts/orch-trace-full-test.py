import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('full_wrapper', Path(__file__).with_name('orch-trace-full.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class FullWrapperTests(unittest.TestCase):
    def fixture(self, root, count=2):
        log = root/'tests.log'
        log.write_text('test real_case ... ok\ntest result: ok. 1 passed\n')
        for index in range(count):
            group = root/'captures'/str(index)
            group.mkdir(parents=True)
            (group/'device.jsonl').write_text('{"v":2,"dev":"same-device","seq":1}\n')
            (group/'capture.json').write_text(json.dumps(dict(capture='present', test_name='real_case', filenames=['device.jsonl'])))
        targets = ['orch_v2', 'orch_v2_env', 'orch_remote', 'orch_questions_native', 'orch_host_gate']
        receipt = root/'receipt.json'
        receipt.write_text(json.dumps(dict(scope='runtime-conformance', build=dict(binaries=[dict(target=t) for t in targets]), test_receipt_path=str(log))))
        return ['full', '--integration', str(root/'captures'), '--provenance', str(receipt), '--out', str(root/'private')]

    def test_missing_producer_receipt_blocks(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(sys, 'argv', ['full', '--integration', tmp]), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(m.main(), 1)

    def test_app_build_cannot_replace_integration_producer(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            receipt = root/'receipt.json'
            receipt.write_text(json.dumps(dict(scope='app-build-only', build=dict(binaries=[]))))
            with patch.object(sys, 'argv', ['full', '--integration', tmp, '--provenance', str(receipt)]):
                with self.assertRaises(ValueError):
                    m.main()

    def test_reused_devices_stay_separate_and_failures_aggregate(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            argv = self.fixture(root)
            calls = []
            def run(command):
                calls.append(command)
                code = 7 if 'check_full_runtime.py' in command[1] else 0
                return dict(exit_code=code, status='fail' if code else 'pass')
            with patch.object(sys, 'argv', argv), patch.object(m.StageRunner, 'run', side_effect=run):
                self.assertEqual(m.main(), 7)
            strict = [c for c in calls if 'check_full_runtime.py' in c[1]]
            self.assertEqual(len(strict), 2)
            self.assertNotEqual(strict[0][strict[0].index('--provenance')+1], strict[1][strict[1].index('--provenance')+1])
            self.assertEqual([sum(c.count(str(p.resolve())) for p in (root/'captures').glob('*/device.jsonl')) for c in strict], [1, 1])
            self.assertNotIn('mapping_context_sha256', json.loads((root/'receipt.json').read_text()))
            self.assertEqual(json.loads((root/'private/receipt.json').read_text())['counts']['fail'], 2)
            calls.clear()
            def fail_bind(command):
                calls.append(command)
                code = 2 if 'context' in command else 0
                return dict(exit_code=code, status='fail' if code else 'pass')
            with patch.object(sys, 'argv', argv), patch.object(m.StageRunner, 'run', side_effect=fail_bind):
                self.assertEqual(m.main(), 2)
            self.assertFalse(any('check_full_runtime.py' in c[1] for c in calls))
            self.assertEqual(len(calls), 6)

    def test_deadline_saves_partial_and_unrun_slots(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            argv = self.fixture(root, 3)+['--seconds', '0.04']
            def slow(command):
                interim = json.loads((root/'private/receipt.json').read_text())
                self.assertGreater(interim['counts'].get('running', 0), 0)
                self.assertGreater(interim['ran'], 0)
                time.sleep(0.08)
                return dict(exit_code=124, status='timeout')
            with patch.object(sys, 'argv', argv), patch.object(m.StageRunner, 'run', side_effect=slow):
                self.assertEqual(m.main(), 124)
            counts = json.loads((root/'private/receipt.json').read_text())['counts']
            self.assertEqual(counts, {'pass':0, 'fail':0, 'timeout':2, 'not-run':1})

    def test_child_timeout_exit_is_counted(self):
        runner = m.StageRunner(time.monotonic()+3, 1)
        result = runner.run([sys.executable, '-c', 'raise SystemExit(124)'])
        self.assertEqual(result, dict(exit_code=124, status='timeout'))

    def test_stage_timeout_kills_nested_child(self):
        with tempfile.TemporaryDirectory() as tmp:
            pidfile = Path(tmp)/'child.pid'
            child = 'import os,signal,time;from pathlib import Path;signal.signal(signal.SIGTERM,signal.SIG_IGN);Path('+repr(str(pidfile))+').write_text(str(os.getpid()));time.sleep(30)'
            parent = 'import subprocess,sys,time;subprocess.Popen([sys.executable,"-c",'+repr(child)+']);time.sleep(30)'
            runner = m.StageRunner(time.monotonic()+3, 0.5)
            result = runner.run([sys.executable, '-c', parent])
            self.assertEqual(result['status'], 'timeout')
            self.assertFalse(runner.processes)
            pid = pidfile.read_text()
            probe = subprocess.run(['ps', '-p', pid, '-o', 'stat='], capture_output=True, text=True)
            self.assertTrue(probe.returncode != 0 or probe.stdout.strip().startswith('Z'))


if __name__ == '__main__':
    unittest.main()
