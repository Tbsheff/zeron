import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from runtime_provenance import validate,verify_paths,validate_context,sha
from unittest.mock import patch

class ProvenanceTests(unittest.TestCase):
    def test_changed_model_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);file=root/'model.tla';file.write_text('old')
            old=hashlib.sha256(file.read_bytes()).hexdigest();file.write_text('changed')
            with self.assertRaises(ValueError):verify_paths({'model.tla':old},root)

    def test_outside_input_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(ValueError):verify_paths({'../outside':'0'*64},Path(folder))

    def test_mid_run_source_edit_is_rejected_first(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);receipt=root/'receipt.json'
            receipt.write_text(json.dumps(dict(v=1,source_before={'a.rs':'0'*64},source_after={'a.rs':'1'*64},models={})))
            with self.assertRaisesRegex(ValueError,'source changed'):
                validate(receipt,[],root/'absent.jar',root,root)

class ContextTests(unittest.TestCase):
    def test_exact_generated_context_is_bound(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);capture=root/'capture.jsonl';capture.write_text('{}\n')
            generator=root/'build_runtime_context.py';generator.write_text('# generator\n')
            context=root/'context.json';context.write_text(json.dumps({'provenance':dict(kind='capture',capture_sha256=[sha(capture)],generator=generator.name,generator_sha256=sha(generator))}))
            receipt=root/'receipt.json';receipt.write_text(json.dumps({'mapping_context_sha256':sha(context)}))
            with patch('runtime_provenance.HERE',root):
                self.assertEqual(validate_context(context,receipt,[capture])['provenance']['kind'],'capture')
                capture.write_text('{"changed":true}\n')
                with self.assertRaisesRegex(ValueError,'capture identity'):
                    validate_context(context,receipt,[capture])

if __name__=='__main__':unittest.main()
