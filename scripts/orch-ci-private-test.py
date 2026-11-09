#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest

spec=importlib.util.spec_from_file_location('handoff',Path(__file__).with_name('orch-ci-private.py'))
handoff=importlib.util.module_from_spec(spec)
spec.loader.exec_module(handoff)

class HandoffTests(unittest.TestCase):
    def test_restores_raw_and_binary_after_checkout_clean(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)/'repo';root.mkdir()
            handoff.ROOT=root
            evidence=root/'target/orch-verify';evidence.mkdir(parents=True)
            raw=evidence/'mechanism-unit/device-a.jsonl';raw.parent.mkdir()
            raw.write_text('private test fixture\n')
            binary=root/'target/debug/deps/orch_v2-test';binary.parent.mkdir(parents=True)
            binary.write_text('test executable bytes');binary.chmod(0o700)
            (evidence/'test.log').write_text('Running tests (target/debug/deps/orch_v2-test)\n')
            stamp={'run':'1','sha':'a','runner':'local'}
            private=Path(tmp)/'private'
            manifest=handoff.seal(private,evidence,stamp)
            shutil.rmtree(root/'target')
            handoff.restore(private,stamp)
            self.assertEqual(raw.read_text(),'private test fixture\n')
            self.assertEqual(binary.read_text(),'test executable bytes')
            self.assertTrue(binary.stat().st_mode & 0o100)
            with self.assertRaises(OSError):
                handoff.restore(private,dict(stamp,run='2'))
            stored=private/handoff.key(stamp)/manifest['files'][0]['stored']
            stored.write_text('changed')
            with self.assertRaisesRegex(ValueError,'changed'):
                handoff.restore(private,stamp)

if __name__=='__main__':
    unittest.main()
