"""Synthetic causal-DAG tests; no engine runtime claim."""
import json
from pathlib import Path
import tempfile
import unittest
from merge_runtime import merge


def row(dev,seq,action='Init',args=None,outbox=None,facts=None):
    return dict(v=2,dev=dev,seq=seq,action=action,args=args or {},
                origin=dict(kind='sqlite',commit=f'sqlite:{dev}:{seq}',
                            parents=[] if not seq else [f'sqlite:{dev}:{seq-1}']),
                snapshot=dict(outbox=outbox or [],facts=facts or []))

class MergeRuntimeTest(unittest.TestCase):
    def run_rows(self,a,b):
        with tempfile.TemporaryDirectory() as d:
            paths=[]
            for name,rs in [('a',a),('b',b)]:
                p=Path(d)/name;p.write_text(''.join(json.dumps(r)+'\n' for r in rs));paths.append(p)
            return merge(paths)
    def fixture(self):
        fx=dict(effect_id='intent#0')
        fact=dict(id='doc:wire',kind='doc_commit',data=dict(wire_id='wire',effect_id='intent#0',device='A',target_host='B',wire_body_digest='sha256:'+'0'*64))
                                                                             
                                                                               
        a=[row('A',0),row('A',1,'Apply',dict(key='intent'),[fx]),
           row('A',2,'RecoveryOpen',outbox=[fx]),row('A',3,'Observe',dict(fact_id='doc:wire'),[fx],[fact])]
        received=dict(id='recv:wire',kind='remote_receive',data=dict(wire_id='wire',wire_body_digest='sha256:'+'0'*64,origin_device='A',receiver_device='B',receipt_id='remote-recv:wire',validated=True))
        b=[row('B',0),row('B',1,'Apply',dict(key='remote-recv:wire'),facts=[received])]
        return a,b
    def test_late_observation(self):
        a,b=self.fixture();result=self.run_rows(a,b)
        nodes={tuple(x['id']):{tuple(p) for p in x['parents']} for x in result['nodes']}
        self.assertIn(('sqlite','A',1),nodes[('doc','wire')])
        self.assertIn(('doc','wire'),nodes[('sqlite','B',1)])
        self.assertNotIn(('sqlite','A',3),nodes[('sqlite','B',1)])
        self.assertEqual(result['wires'],[dict(wire_id='wire',origin_device='A',effect_id='intent#0',target_host='B',intent_seq=1,wire_body_digest='sha256:'+'0'*64)])
    def test_missing_intent_link(self):
        a,b=self.fixture();del a[-1]['snapshot']['facts'][0]['data']['effect_id']
        with self.assertRaises(ValueError):self.run_rows(a,b)
    def test_wrong_receive_host(self):
        a,b=self.fixture();a[-1]['snapshot']['facts'][0]['data']['target_host']='C'
        with self.assertRaises(ValueError):self.run_rows(a,b)
    def test_receive_observation_cannot_arrive_after_apply(self):
        a,b=self.fixture();fact=b[-1]['snapshot']['facts'][0]
        b[-1]['snapshot']['facts']=[]
        b.append(row('B',2,'Observe',facts=[fact]))
        with self.assertRaises(ValueError):self.run_rows(a,b)

    def test_receiver_changed_payload(self):
        a,b=self.fixture();b[-1]['snapshot']['facts'][0]['data']['wire_body_digest']='sha256:'+'2'*64
        with self.assertRaises(ValueError):self.run_rows(a,b)

    def test_same_wire_changed_payload(self):
        a,b=self.fixture()
        fact=dict(id='doc:changed',kind='doc_commit',data=dict(wire_id='wire',effect_id='intent#0',device='A',target_host='B',wire_body_digest='sha256:'+'1'*64))
        a[-1]['snapshot']['facts'].append(fact)
        with self.assertRaises(ValueError):self.run_rows(a,b)

    def test_mutated_retry_wire(self):
        a,b=self.fixture()
        fact=dict(id='doc:changed',kind='doc_commit',data=dict(wire_id='changed',effect_id='intent#0',device='A',target_host='B',wire_body_digest='sha256:'+'0'*64))
        a[-1]['snapshot']['facts'].append(fact)
        with self.assertRaises(ValueError):self.run_rows(a,b)

    def test_false_origin(self):
        a,b=self.fixture();b[-1]['origin']['parents']=[]
        with self.assertRaises(ValueError):self.run_rows(a,b)

    def test_missing_source(self):
        a,b=self.fixture();a[-1]['snapshot']['facts']=[]
        with self.assertRaises(ValueError):self.run_rows(a,b)

    def test_effect_completion_has_exact_producer_parent(self):
        a,b=self.fixture();a[-1]['args']['effect_id']='intent#0'
        a[-1]['origin']['parents'].append('sqlite:A:1')
        self.assertEqual(len(self.run_rows(a,b)['rows']),6)
        a[-1]['origin']['parents'][-1]='sqlite:A:0'
        with self.assertRaises(ValueError):self.run_rows(a,b)

if __name__=='__main__':unittest.main()
