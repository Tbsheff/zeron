import json
from pathlib import Path
import tempfile
import unittest
from inventory_runtime_groups import collect

class GroupTests(unittest.TestCase):
    def test_failed_prefix_is_not_a_complete_test_group(self):
        with tempfile.TemporaryDirectory() as folder:
            group=Path(folder)/'group';group.mkdir()
            (group/'capture.json').write_text(json.dumps({'capture':'failed_test','filenames':['absent.jsonl']}))
            captures,index,skipped=collect(Path(folder),'engine-integration')
            self.assertEqual((captures,index,skipped),([],[],1))
if __name__=='__main__':unittest.main()
