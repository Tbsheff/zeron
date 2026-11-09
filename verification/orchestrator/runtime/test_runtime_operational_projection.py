"""Check the global reporting join without a target successor relation."""
from copy import deepcopy
import unittest

from runtime_full_projection import FullProjectionError,fn,aset
from runtime_operational_projection import initial_chat,merge_chat_states,join_pending,prefix_routes,eligible_row,require_source_authority


CONTEXT=dict(reporting_constants=dict(NoRuntime='none'))


def event(parent,kind='spawn'):
    return dict(kind=kind,source='source-'+parent,key=[parent,'1'],run=1,attempt=1,out='none')


def states():return {parent:initial_chat(parent,CONTEXT) for parent in ('a','b')}


class OperationalProjectionTests(unittest.TestCase):
    def test_preserves_global_pending_order_across_parent_order(self):
        current=states();a=event('a');b=event('b')
        current['a']['pending']=[a];current['b']['pending']=[b]
        self.assertEqual(join_pending([b,a],current,5,'Observe'),[b,a])

    def test_removes_one_lane_and_appends_new_observation(self):
        current=states();a=event('a');b=event('b');new=event('a','nativeEnd')
        current['a']['pending']=[new];current['b']['pending']=[b]
        self.assertEqual(join_pending([a,b],current,5,'Apply'),[b,new])

    def test_rejects_multiple_unordered_new_events(self):
        current=states();current['a']['pending']=[event('a')];current['b']['pending']=[event('b')]
        with self.assertRaisesRegex(FullProjectionError,'multiple_pending_append'):
            join_pending([],current,5,'Observe')

    def test_rejects_ambiguous_duplicate_event_removal(self):
        current=states();a=event('a');current['a']['pending']=[a]
        with self.assertRaisesRegex(FullProjectionError,'ambiguous_pending_removal'):
            join_pending([a,event('b'),a],current,5,'Apply')

    def test_rejects_runtime_alias_between_parents(self):
        current=states()
        current['a']['sources']=fn([('runtime',dict(parent='a'))])
        current['b']['sources']=fn([('runtime',dict(parent='b'))])
        with self.assertRaisesRegex(FullProjectionError,'sources_alias'):
            merge_chat_states(current,states(),None,5,'Observe',CONTEXT)

    def test_keeps_distinct_parent_native_keys(self):
        current=states()
        for parent in current:current[parent]['natives']=fn([([parent,'1'],dict(parent=parent))])
        result=merge_chat_states(current,states(),None,5,'Observe',CONTEXT)
        self.assertEqual([key for key,value in result['natives']['$fn']],[['a','1'],['b','1']])

    def test_new_chat_work_resets_old_global_cause(self):
        old=states();old['a']['lastCause']=dict(kind='rootEnd',source='source-a',changed=aset([]))
        current=deepcopy(old);current['b']['docTasks']=fn([(['b','1'],dict(source='source-b',status='done'))])
        previous=dict(pending=[],lastCause=old['a']['lastCause'])
        result=merge_chat_states(current,old,previous,5,'Observe',CONTEXT)
        self.assertEqual(result['lastCause'],current['b']['lastCause'])

    def test_rejects_two_distinct_causes_in_one_source_row(self):
        old=states();current=deepcopy(old)
        for parent in current:current[parent]['lastCause']=dict(kind='rootEnd',source='source-'+parent,changed=aset([]))
        with self.assertRaisesRegex(FullProjectionError,'multiple_last_cause'):
            merge_chat_states(current,old,None,5,'Apply',CONTEXT)

    def test_old_catalog_cannot_prove_new_chat_inventory(self):
        catalog=dict(kind='provider_stream_catalog',data=dict(complete=True,streams=[]))
        source=dict(dev='d',action='Observe',partitions=fn([('d',dict(mechanism=dict(
            facts=fn([('old',catalog),('new',catalog)]),fact_order=['old','new'])))]))
        focused,excluded=eligible_row(source,'d',4,{('d','old'):2,('d','new'):5})
        self.assertEqual(excluded,['old'])
        self.assertEqual(dict(focused['partitions']['$fn'])['d']['mechanism']['fact_order'],['new'])
        self.assertEqual(dict(source['partitions']['$fn'])['d']['mechanism']['fact_order'],['old','new'])

    def test_late_owner_cannot_authorize_earlier_execution(self):
        source=dict(action='Apply',args=dict(accepted=True,duplicate=False),
            event=dict(act='StartFromQueue',args=dict(t='a')),
            partitions=fn([('d',dict(core=dict(threads=fn([('a',{})]))))]))
        with self.assertRaisesRegex(FullProjectionError,'physical_owner_unknown'):
            require_source_authority(source,3,{},[],['a'])
        require_source_authority(source,3,{'a':'d'},[],['a'])

    def test_unknown_current_binding_has_no_future_owner(self):
        source=dict(action='Init',partitions=fn([('d',dict(mechanism=dict(
            bindings=fn([('a',dict(host_known=False,host='d'))]),facts=fn([]),fact_order=[])))]))
        self.assertEqual(prefix_routes(source,0),{})


    def test_unobserved_partition_stays_unknown_inventory(self):
        observed=initial_chat('a',CONTEXT)
        self.assertEqual(observed['catalogComplete'],fn([('a',False)]))
        self.assertEqual(observed['sources'],fn([]))
        self.assertEqual(observed['handles'],fn([('a','none')]))


if __name__=='__main__':unittest.main()
