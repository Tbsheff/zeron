import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from inventory_full_runtime_groups import inventory
from runtime_full_projection import FullProjectionError

class FullInventoryTests(unittest.TestCase):
    def test_named_blocker_does_not_export_private_path(self):
        with tempfile.TemporaryDirectory() as folder:
            group=Path(folder)/'private-marker';group.mkdir()
            (group/'capture.json').write_text(json.dumps(dict(capture='present',filenames=['private-raw.jsonl'])))
            with patch('inventory_full_runtime_groups.build',side_effect=FullProjectionError(2,'Apply','access.configured')):
                result=inventory(Path(folder))
            self.assertEqual(result['blockers'],{'access.configured':1})
            self.assertNotIn('private-marker',json.dumps(result))
            self.assertNotIn('private-raw',json.dumps(result))
    def test_projected_group_is_not_a_missing_product_blocker(self):
        with tempfile.TemporaryDirectory() as folder:
            group=Path(folder)/'g';group.mkdir()
            (group/'capture.json').write_text(json.dumps(dict(capture='present',filenames=['raw.jsonl'])))
            with patch('inventory_full_runtime_groups.build',return_value={}), patch('inventory_full_runtime_groups.project_global',return_value=([],1)), patch('inventory_full_runtime_groups.project_full',return_value={'images':[{}]}):
                result=inventory(Path(folder))
            self.assertEqual(result['blockers'],{})
            self.assertEqual(result['projected_groups'],1)
            self.assertFalse(result['results'][0]['full_conformance'])
    def test_failed_test_prefix_is_excluded(self):
        with tempfile.TemporaryDirectory() as folder:
            group=Path(folder)/'g';group.mkdir()
            (group/'capture.json').write_text(json.dumps(dict(capture='failed_test',filenames=['missing.jsonl'])))
            with patch('inventory_full_runtime_groups.build') as build:
                self.assertEqual(inventory(Path(folder))['groups'],0)
                build.assert_not_called()
if __name__=='__main__':unittest.main()
