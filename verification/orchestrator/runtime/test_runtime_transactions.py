"""Physical grouping tests use synthetic rows; they make no runtime claim."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from check_global_runtime import project_global
from check_trace import tla
from merge_runtime import merge
from test_merge_runtime import row
import test_merge_runtime


def grouped(rows):
    for r in rows:r['origin']['transaction']=r['origin']['commit']
    return rows

class TransactionTests(unittest.TestCase):
    def run_rows(self,*streams,strict=True):
        with tempfile.TemporaryDirectory() as folder:
            paths=[]
            for i,rows in enumerate(streams):
                p=Path(folder)/str(i);p.write_text(''.join(json.dumps(r)+'\n' for r in rows));paths.append(p)
            return merge(paths,require_transactions=strict)
    def sample(self):
        a=grouped([row('A',0),row('A',1,'Apply',{'key':'callback'}),row('A',2,'CompleteEffect',{'effect_id':'fx'})])
        a[2]['origin']['transaction']='sqlite:A:1'
        return a
    def test_atomic_rows_are_contiguous_and_only_last_is_visible(self):
        result=self.run_rows(self.sample(),grouped([row('B',0),row('B',1,'Observe')]))
        order=[(r['dev'],r['seq']) for r in result['rows']]
        self.assertEqual(order.index(('A',2)),order.index(('A',1))+1)
        vis={(v['dev'],v['seq']):v for v in result['visibility']}
        self.assertFalse(vis['A',1]['visible']);self.assertTrue(vis['A',2]['visible'])
        self.assertEqual(vis['A',1]['size'],2)
    def test_strict_rejects_historical_rows(self):
        with self.assertRaises(ValueError):self.run_rows([row('A',0)])
    def test_mixed_missing_transaction_rejects(self):
        a=self.sample();del a[1]['origin']['transaction']
        with self.assertRaises(ValueError):self.run_rows(a)
    def test_malformed_origin_rejects(self):
        a=self.sample();a[1]['origin']=None
        with self.assertRaises(ValueError):self.run_rows(a)
    def test_cross_device_group_rejects(self):
        a=self.sample();a[2]['origin']['transaction']='sqlite:B:1'
        with self.assertRaises(ValueError):self.run_rows(a)
    def test_noncanonical_group_rejects(self):
        a=self.sample();a[1]['origin']['transaction']='sqlite:A:01'
        with self.assertRaises(ValueError):self.run_rows(a)
    def test_future_group_start_rejects(self):
        a=self.sample();a[1]['origin']['transaction']='sqlite:A:2'
        with self.assertRaises(ValueError):self.run_rows(a)
    def test_reused_closed_group_rejects(self):
        a=self.sample();a.append(row('A',3,'Observe'));a[3]['origin']['transaction']='sqlite:A:0'
        with self.assertRaises(ValueError):self.run_rows(a)
    def test_external_write_cannot_split_uncommitted_intent_group(self):
        a,b=test_merge_runtime.MergeRuntimeTest().fixture();grouped(a);grouped(b)
                                                                            
                                                                           
                                                                              
        for r in a[1:]:r['origin']['transaction']='sqlite:A:1'
        with self.assertRaisesRegex(ValueError,'physical commit DAG'):
            self.run_rows(a,b)
    def test_projection_keeps_committed_partitions_until_group_end(self):
        a=self.sample();b=grouped([row('B',0)])
        with tempfile.TemporaryDirectory() as folder:
            paths=[]
            for i,rows in enumerate((a,b)):
                p=Path(folder)/str(i);p.write_text(''.join(json.dumps(r)+'\n' for r in rows));paths.append(p)
            def projected(rows):return [dict(dev=r['dev'],seq=r['seq'],action=r['action']) for r in rows]
            with patch('check_global_runtime.project',side_effect=projected):
                steps,_=project_global(paths,require_transactions=True)
            internal=next(s for s in steps if s['row']['dev']=='A' and s['row']['seq']==1)
            final=next(s for s in steps if s['row']['dev']=='A' and s['row']['seq']==2)
            self.assertEqual(dict(internal['row']['committed_partitions']['$fn'])['A']['seq'],0)
            self.assertEqual(dict(final['row']['committed_partitions']['$fn'])['A']['seq'],2)
            self.assertFalse(internal['row']['transaction']['visible'])
            self.assertTrue(final['row']['transaction']['visible'])
            tla(steps)                                                  
    def test_historical_mode_does_not_claim_physical_grouping(self):
        result=self.run_rows([row('A',0)],strict=False)
        self.assertFalse(result['physical_transactions'])
        self.assertIsNone(result['visibility'][0]['id'])
if __name__=='__main__':unittest.main()
