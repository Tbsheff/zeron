import json
from pathlib import Path
import tempfile
import unittest
from inventory_runtime_captures import inventory


class InventoryTests(unittest.TestCase):
    def test_private_data_never_leaves_inventory(self):
        marker='PRIVATE_PAYLOAD_MUST_NOT_LEAVE'
        row={'dev':marker,'seq':0,'action':'Init','snapshot':{
             'facts':[{'id':marker,'kind':'provider_start','data':{'prompt':marker}}],
             'private_text':marker}}
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/marker;path.write_text(json.dumps(row)+'\n')
            result=inventory(path,'focused-mock-engine')
        self.assertNotIn(marker,json.dumps(result))
        self.assertEqual(result['claim'],'inventory only; no conformance result')

    def test_conflicting_fact_is_refused(self):
        rows=[{'dev':'A','seq':i,'action':'Init' if i==0 else 'RecoveryOpen',
               'snapshot':{'facts':[{'id':'f','kind':'provider_start','data':{'value':i}}]}}
              for i in range(2)]
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'capture';path.write_text(''.join(json.dumps(x)+'\n' for x in rows))
            with self.assertRaises(ValueError):inventory(path,'focused-mock-engine')

    def test_missing_prefix_is_explicit(self):
        row={'dev':'A','seq':8,'action':'RecoveryOpen','snapshot':{'facts':[]}}
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'capture';path.write_text(json.dumps(row)+'\n')
            self.assertFalse(inventory(path,'focused-mock-engine')['complete_local_sequences'])

if __name__=='__main__':unittest.main()
