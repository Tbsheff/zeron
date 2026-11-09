"""Check source-bound R44 labels and the declared loss macro."""
from copy import deepcopy
import unittest

from runtime_full_projection import FullProjectionError,fn,aset
from runtime_reporting_actions import reporting_actions,equal


def state():
    return dict(catalogComplete=fn([('p',False)]),runs=fn([('p',dict(run=1,attempt=1,active=True,endBy='none'))]),
        sources=fn([]),natives=fn([]),tasks=fn([]),docTasks=fn([]),handles=fn([('p','none')]),
        admitted=fn([('p',dict(present=False))]),pending=[],lastCause=dict(kind='none',source='none',changed=aset([])))


def row(action='Observe',event=None,fact=None):
    mechanism=dict(facts=fn([] if fact is None else [('f',fact)]),fact_order=[] if fact is None else ['f'])
    result=dict(raw_row=7,raw_seq=12,dev='d',action=action,mechanism=mechanism)
    if event:result.update(event=event,args=dict(accepted=True,duplicate=False))
    return result


def loss_case():
    key=['p','9'];before=state()
    task=dict(source='a',parent='p',run=1,attempt=1,status='running',background=True,
              cause='none',activation='none',generation=0)
    before['tasks']=fn([(key,task)])
    before['pending']=[dict(kind='spawn',source='other',key=['q','1'],run=1,attempt=1,out='none'),
                       dict(kind='sourceLoss',source='a',key=[],run=1,attempt=1,out='failed')]
    after=deepcopy(before);after['tasks']['$fn'][0][1].update(status='failed',cause='reporting_loss')
    after['pending'].pop(1);after['lastCause']=dict(kind='sourceLoss',source='a',changed=aset([]))
    source=row('Apply',dict(act='NativeEnd',args=dict(parent='p',key='9',failed=True)))
    return key,before,after,source


class ReportingActionTests(unittest.TestCase):
    def test_stutter_requires_semantic_equality(self):
        before=state();after=deepcopy(before)
        before['handles']=fn([('p','none'),('q','none')]);after['handles']=fn([('q','none'),('p','none')])
        result=reporting_actions(row(),row(),{},before,after)
        self.assertEqual(result['actions'],[dict(label='Stutter',args={})])
        after['pending']=[dict(kind='unobserved')]
        with self.assertRaisesRegex(FullProjectionError,'reporting_action.missing_source'):
            reporting_actions(row(),row(),{},before,after)

    def test_runtime_label_preserves_actual_access_and_revision(self):
        before=state();after=deepcopy(before);after['handles']=fn([('p','runtime-a')])
        fact=dict(kind='runtime_binding',data=dict(operation='install',chat='p',runtime_id='runtime-a',
            effective_access=1,source_revision=19,origin_id='request-b',validated=True))
        result=reporting_actions(row(),row(fact=fact),{},before,after)
        self.assertEqual(result['actions'],[dict(label='InstallHandle',args=dict(p='p',sid='runtime-a',
            access=1,revision=19,origin='request-b'))])

    def test_final_loss_keeps_exact_source_and_lane(self):
        key,before,after,source=loss_case()
        result=reporting_actions(row(),source,{},before,after)
        self.assertEqual(result['actions'],[dict(label='HandleLossNative',args=dict(i=2,k=key)),
                                            dict(label='Handle',args=dict(i=2))])
        middle=result['images'][0]
        self.assertEqual(middle['pending'],before['pending'])
        self.assertEqual(middle['tasks'],after['tasks'])
        self.assertEqual(middle['lastCause'],dict(kind='sourceLoss',source='a',changed=aset([key])))
        self.assertEqual(result['images'][1],after)
        self.assertEqual(result['source_map'],[dict(raw_row=7,raw_seq=12,macro_substep=1,substep=1),
                                              dict(raw_row=7,raw_seq=12,macro_substep=2,substep=2)])
        self.assertEqual(before['tasks']['$fn'][0][1]['status'],'running')

    def test_loss_cannot_close_another_source(self):
        key,before,after,source=loss_case();before['pending'][1]['source']='other'
        with self.assertRaisesRegex(FullProjectionError,'pending_sourceLoss'):
            reporting_actions(row(),source,{},before,after)

    def test_loss_requires_accepted_nonduplicate_input(self):
        key,before,after,source=loss_case();source['args']['duplicate']=True
        with self.assertRaisesRegex(FullProjectionError,'missing_source'):
            reporting_actions(row(),source,{},before,after)

    def test_missing_schema_reports_field_only(self):
        before=state();after=deepcopy(before);after['pending']=[1]
        source=row(fact=dict(kind='runtime_binding',data=dict(validated=True)))
        with self.assertRaisesRegex(FullProjectionError,'reporting_action.source_schema'):
            reporting_actions(row(),source,{},before,after)

    def test_loss_without_consumption_is_one_step(self):
        key,before,after,source=loss_case();after['pending']=deepcopy(before['pending'])
        after['lastCause']['changed']=aset([key])
        result=reporting_actions(row(),source,{},before,after)
        self.assertEqual(result['actions'],[dict(label='HandleLossNative',args=dict(i=2,k=key))])


class RootLossMacroTests(unittest.TestCase):
    def case(self):
        before=state()
        before['pending']=[dict(kind='spawn',source='other',key=['q','1'],run=1,attempt=1,out='none'),
            dict(kind='rootEnd',source='a',key=[],run=1,attempt=1,out='completed'),
            dict(kind='sourceLoss',source='a',key=[],run=1,attempt=1,out='failed')]
        after=deepcopy(before);after['pending']=after['pending'][:1]
        after['runs']['$fn'][0][1].update(active=False,endBy='root')
        after['lastCause']=dict(kind='sourceLoss',source='a',changed=aset([]))
        source=row('Apply',dict(act='IngestEnd',args=dict(t='p',ev=dict(run=1,attempt=1))))
        return before,after,source

    def test_root_end_then_exact_empty_loss(self):
        before,after,source=self.case();result=reporting_actions(row(),source,{},before,after)
        self.assertEqual(result['actions'],[dict(label='Handle',args=dict(i=2)),dict(label='Handle',args=dict(i=2))])
        middle=result['images'][0]
        self.assertEqual(middle['pending'],[before['pending'][0],before['pending'][2]])
        self.assertEqual(middle['runs'],after['runs'])
        self.assertEqual(middle['lastCause'],dict(kind='rootEnd',source='a',changed=aset([])))
        self.assertEqual(result['images'][1],after)
        self.assertEqual(len(before['pending']),3)
        self.assertEqual(reporting_actions(source,row('MarkCommand'),{},after,deepcopy(after))['actions'],
                         [dict(label='Stutter',args={})])

    def test_other_source_loss_cannot_join_root(self):
        before,after,source=self.case();before['pending'][2]['source']='other'
        with self.assertRaisesRegex(FullProjectionError,'root_loss_source'):
            reporting_actions(row(),source,{},before,after)

    def test_running_task_keeps_loss_pending(self):
        before,after,source=self.case()
        after['tasks']=fn([(['p','9'],dict(source='a',status='running',cause='none'))])
        with self.assertRaisesRegex(FullProjectionError,'root_loss_running_task'):
            reporting_actions(row(),source,{},before,after)

    def test_root_orphan_effect_stays_on_first_step(self):
        before,after,source=self.case();key=['p','9']
        before['tasks']=fn([(key,dict(source='a',status='running',cause='none'))])
        after['tasks']=fn([(key,dict(source='a',status='failed',cause='orphan'))])
        result=reporting_actions(row(),source,{},before,after)
        self.assertEqual(result['images'][0]['lastCause']['changed'],aset([key]))
        self.assertEqual(result['images'][0]['tasks'],after['tasks'])

    def test_prior_same_source_event_blocks_empty_loss(self):
        before,after,source=self.case()
        before['pending'].insert(2,dict(kind='nativeEnd',source='a',key=['p','9'],run=1,attempt=1,out='completed'))
        after['pending'].append(deepcopy(before['pending'][2]))
        with self.assertRaisesRegex(FullProjectionError,'root_loss_lane'):
            reporting_actions(row(),source,{},before,after)

    def test_extra_pending_delta_refuses_macro(self):
        before,after,source=self.case();after['pending']=[]
        with self.assertRaisesRegex(FullProjectionError,'root_loss_pending_delta'):
            reporting_actions(row(),source,{},before,after)


if __name__=='__main__':unittest.main()
