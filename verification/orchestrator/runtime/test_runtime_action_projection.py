import unittest
from copy import deepcopy
from runtime_action_projection import source_actions
from runtime_full_projection import FullProjectionError,fn

CTX={'tree_threads':{'r':['r','c']}}
def forest(image):return fn([(['r','r'],image)])
def row(act,args=None):return dict(seq=1,action='Apply',args=dict(accepted=True,duplicate=False),event=dict(act=act,args=args or {'t':'r'}))
class ActionTests(unittest.TestCase):
    def labels(self,item,before=None,after=None):
        return source_actions({},item,CTX,forest(before or {'marker':0}),forest(after or {'marker':1}))['$fn'][0][1]
    def test_direct_run_labels(self):
        for act,label in [('Stop','C_Stop'),('StartFromQueue','C_StartQueue'),('StartFromQueueWake','C_StartWake'),('UsageLimitRecovery','C_ULRecovery')]:
            self.assertEqual(self.labels(row(act))['label'],label)
    def test_person_mode_from_input(self):
        self.assertEqual(self.labels(row('PersonSend',dict(t='r',mode='Queue')))['args'],dict(t='r',m='queue'))
    def test_ingest_identity_from_input(self):
        ev=dict(node='Root',run=4,attempt=2,outcome='UsageLimit')
        self.assertEqual(self.labels(row('IngestEnd',dict(t='r',ev=ev)))['args']['e'],dict(n='root',r=4,a=2,o='usage_limit'))
    def test_unchanged_is_only_stutter(self):
        self.assertEqual(self.labels(row('Unknown'),{'marker':1},{'marker':1})['label'],'Stutter')
    def test_rejected_change_refused(self):
        item=row('Stop');item['args']['accepted']=False
        with self.assertRaisesRegex(FullProjectionError,'rejected_or_duplicate_changed'):self.labels(item)
    def test_unknown_change_refused(self):
        with self.assertRaisesRegex(FullProjectionError,'unmapped_event'):self.labels(row('Unknown'))
    def test_registration_requires_actual_open(self):
        actual=row('OpenRoot',dict(t='r'))
        self.assertEqual(source_actions({},actual,CTX,fn([]),forest({}))['$fn'][0][1]['label'],'RegisterRoot')
        with self.assertRaisesRegex(FullProjectionError,'registration_cause'):source_actions({},row('Stop'),CTX,fn([]),forest({}))
    def test_followup_access_macro_refused(self):
        before=dict(access=fn([('c',2)]),marker=0);after=deepcopy(before);after['marker']=1
        with self.assertRaisesRegex(FullProjectionError,'macro.followup_access'):self.labels(row('Followup',dict(parent='r',child='c',access=1)),before,after)
    def test_exact_delivery_mode(self):
        self.assertEqual(self.labels(row('DoOffer',dict(parent='r',run=2,generation=3,how='Ack')))['args'],dict(e=dict(k='offer',t='r',r=2,g=3),how='ack'))
    def test_keep_does_not_prove_crash(self):
        with self.assertRaisesRegex(FullProjectionError,'macro.delivery_without_ack'):
            self.labels(row('DoOffer',dict(parent='r',run=2,generation=3,how='Keep')))
    def test_access_comes_from_validated_fact(self):
        item=dict(seq=1,action='Observe',args=dict(fact_id='f'),mechanism=dict(facts=fn([('f',dict(kind='runtime_binding',data=dict(chat='r',operation='issue',validated=True,effective_access={'present':True,'value':1})))])))
        self.assertEqual(self.labels(item),dict(label='C_ObserveAccess',args=dict(t='r',a=1)))
        item['mechanism']['facts']['$fn'][0][1]['data']['validated']=False
        with self.assertRaisesRegex(FullProjectionError,'access_identity'):self.labels(item)
if __name__=='__main__':unittest.main()
