#!/usr/bin/env python3
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec=importlib.util.spec_from_file_location('provenance',Path(__file__).with_name('orch-provenance.py'))
provenance=importlib.util.module_from_spec(spec)
spec.loader.exec_module(provenance)

class SourceModels(unittest.TestCase):
    def test_tracks_declared_config_and_omits_generated_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);runtime=root/'runtime';runtime.mkdir()
            (runtime/'Model.tla').write_text('source')
            cfg=runtime/'Model.cfg';cfg.write_text('INVARIANT Safe')
            (runtime/'full-spec-source-manifest.json').write_text('[]')
            entries=[dict(path=p.name,sha256=provenance.sha(p)) for p in runtime.iterdir()]
            (runtime/'runtime-vendor-allowlist.json').write_text(json.dumps(dict(files=entries)))
            before=provenance.model_hashes(root)
            output=root/'tla/Fast_out';output.mkdir(parents=True)
            (output/'Model.cfg').write_text('transient output')
            (output/'summary.json').write_text('{}')
            self.assertEqual(before,provenance.model_hashes(root))
            cfg.write_text('INVARIANT Other')
            with self.assertRaisesRegex(ValueError,'allowlist'):
                provenance.model_hashes(root)

if __name__=='__main__':
    unittest.main()
