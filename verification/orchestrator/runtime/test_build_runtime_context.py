"""Test context derivation with synthetic source facts; no capture claim."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from build_runtime_context import build,derive,completed,catalog_authority,bind_transfer_reporting,POLICY
from runtime_full_projection import FullProjectionError,Projection,fn


def fixture():
    policy={key:None for key in POLICY.values()}
    policy['in_place_access']=[]
    provider=dict(provider_id='p',accepts_handoff=True,native_fork=False,inject_history=False,
                  person_selectable=True,delivery_path='portable_prompt',
                  effective_native_fork=False,effective_native_injection=False)
    facts=[dict(kind='capability_catalog',data=dict(scope='selected_harnesses',complete=True,
                                                  providers=[provider],policy=policy,validated=True)),
           dict(kind='access_config',data=dict(chat='r',validated=True,configured_access=2)),
           dict(kind='received_ceiling',data=dict(chat='r',validated=True,source_kind='root_initial',access=2)),
           dict(kind='engine_lifecycle',data=dict(device='d',engine_instance_id='i',state='started'))]
    th=dict(parent='none',access=2,run_base=0,runs=[],attempt=dict(n=0),queue=[],reconfigure=False)
    core=dict(config=dict(max_depth=2,max_fanout=8,max_queue=64,max_retry=2,run_window=8),
              threads=fn([('r',th)]),natives=fn([]),questions=fn([]))
    facts[0]['data']['core_config']=deepcopy(core['config'])
    mechanism=dict(bindings=fn([('r',dict(host_known=True,host='d'))]),
                   facts=fn(enumerate(facts)),fact_order=list(range(len(facts))))
    part=dict(core=core,mechanism=mechanism)
    row=dict(dev='d',action='Init',partitions=fn([('d',part)]))
    return [row],dict(kind='capture',capture_sha256=['0'*64],generator='build_runtime_context.py',generator_sha256='1'*64)


def transfer_fixture(rows,context):
                                                                           
                                                                             
    return dict(transfer_constants=dict(UseTransferReporting=True),
                transfer_images=[{} for row in rows],
                transfer_actions=[dict(label='Stutter',args={}) for row in rows[1:]],
                transfer_source_map=[dict(raw_row=i,macro_substep=0) for i,row in enumerate(rows)],
                transfer_scope='explicit synthetic context-unit fixture; no capture claim')


class ContextTests(unittest.TestCase):
    def setUp(self):
        mock=patch('runtime_transfer_reporting_projection.project_transfer_reporting',side_effect=transfer_fixture)
        self.reader=mock.start();self.addCleanup(mock.stop)

    def test_derives_complete_catalog(self):
        rows,provenance=fixture();result=derive(rows,provenance)
        self.assertEqual(result['tree_threads'],{'r':['r']})
        self.assertEqual(result['constants']['CanInject'],{'$set':[]})
        self.assertEqual(result['constants']['MaxQueue'],64)
        self.assertEqual(result['constants']['RLMaxRetry'],2)
        self.assertEqual(result['provenance'],provenance)
        self.assertIs(result['constants']['UseCT'],False)
        self.assertIs(result['transfer_constants']['UseTransferReporting'],True)
        self.reader.assert_called_once()

    def test_capability_catalog_must_match_actual_config(self):
        rows,provenance=fixture()
        catalog=rows[0]['partitions']['$fn'][0][1]['mechanism']['facts']['$fn'][0][1]['data']
        catalog['core_config']['max_queue']=65
        with self.assertRaisesRegex(FullProjectionError,'capability_catalog_config'):derive(rows,provenance)


    def test_observed_queue_cannot_raise_configured_cap(self):
        rows,provenance=fixture()
        thread=rows[0]['partitions']['$fn'][0][1]['core']['threads']['$fn'][0][1]
        thread['queue']=[{}]*65
        self.assertEqual(derive(rows,provenance)['constants']['MaxQueue'],64)


    def test_null_model_policy_uses_separate_named_contract(self):
        rows,provenance=fixture()
        data=rows[0]['partitions']['$fn'][0][1]['mechanism']['facts']['$fn'][0][1]['data']
        data['policy']['forks']={'present':False}
        result=derive(rows,provenance)
        self.assertIs(result['constants']['Forks'],True)
        self.assertIn('scope_contract',result)
        self.assertIsNone(data['policy']['cuts'])

    def test_declared_injection_does_not_enable_effective_injection(self):
        rows,provenance=fixture()
        provider=rows[0]['partitions']['$fn'][0][1]['mechanism']['facts']['$fn'][0][1]['data']['providers'][0]
        provider['declared_inject_history']=True
        self.assertEqual(derive(rows,provenance)['constants']['CanInject'],{'$set':[]})
        provider['effective_native_injection']=True
        with self.assertRaisesRegex(FullProjectionError,'effective_native_injection'):derive(rows,provenance)

    def test_unknown_host_refuses(self):
        rows,provenance=fixture()
        rows[0]['partitions']['$fn'][0][1]['mechanism']['bindings']['$fn'][0][1]['host_known']=False
        with self.assertRaisesRegex(FullProjectionError,'host.unknown'):derive(rows,provenance)


    def test_stream_close_is_not_legacy_process_death(self):
        rows,provenance=fixture()
        mechanism=rows[0]['partitions']['$fn'][0][1]['mechanism']
        facts=mechanism['facts']['$fn'];identity=len(facts)
        facts.append((identity,dict(kind='provider_event',data=dict(chat='r',provider_run_id='runtime-a',
            event_kind='stream_closed',observer=dict(natives=[]),provider_alive=None))))
        mechanism['fact_order'].append(identity)
        result=derive(rows,provenance)
        self.assertIs(result['constants']['UseNS'],False)
        self.assertEqual(result['constants']['MaxSessEnd'],0)
        self.assertIs(result['scope_contract']['reporting_product_required'],True)


    def test_reporting_domains_use_actual_ids(self):
        rows,provenance=fixture()
        mechanism=rows[0]['partitions']['$fn'][0][1]['mechanism']
        additions=[
            dict(kind='runtime_binding',data=dict(chat='r',runtime_id='installed-a',source_id='instance-a',source_revision=7,origin_id='origin-a',run=1,attempt=1,effective_access=2,operation='install',validated=True)),
            dict(kind='provider_start',data=dict(chat='r',outcome='engine_start_ok',run=1,attempt=1,effective_access=2,
                                               provider_return_id=dict(present=True,value='returned-b'))),
            dict(kind='provider_stream_catalog',data=dict(streams=[dict(provider_run_id='catalog-c')])),
            dict(kind='native_binding',data=dict(chat='r',native_path=['a','b'],task_key='99')),
            dict(kind='provider_event',data=dict(chat='r',provider_run_id='installed-a',event_kind='nested',
                observer=dict(natives=[dict(native_path=['a','b'],activation_id='activation-d')]))),
        ]
        for fact in additions:
            identity=len(mechanism['fact_order'])
            mechanism['facts']['$fn'].append((identity,fact));mechanism['fact_order'].append(identity)
        report=derive(rows,provenance)['reporting_constants']
        self.assertIs(report['UseReporting'],True)
        self.assertEqual(report['Parents'],{'$set':['r']})
        self.assertEqual(report['NativeIds'],{'$set':[['r','99']]})
        self.assertEqual(report['RuntimeIds'],{'$set':['catalog-c','installed-a','returned-b']})
        self.assertEqual(report['ActivationIds'],{'$set':['activation-d']})
        self.assertEqual(report['OriginIds'],{'$set':['origin-a']})
        self.assertEqual(report['NoActivation'],'none')
        self.assertEqual(report['MaxReportAttempts'],0)
        self.assertEqual(report['MaxSourceRevision'],7)
        self.assertEqual(report['NoNative'],[])
        self.assertEqual(report['MaxDeliveryGeneration'],0)

    def test_unknown_activation_stays_unknown_in_reporting_product(self):
        rows,provenance=fixture()
        mechanism=rows[0]['partitions']['$fn'][0][1]['mechanism'];identity=len(mechanism['fact_order'])
        mechanism['facts']['$fn'].append((identity,dict(kind='native_binding',
            data=dict(chat='r',native_path=['a'],task_key='9'))))
        mechanism['fact_order'].append(identity)
        result=derive(rows,provenance)
        self.assertEqual(result['reporting_constants']['ActivationIds'],{'$set':[]})
        self.assertEqual(result['reporting_constants']['NoActivation'],'none')
        self.assertEqual(result['activation_order'],{})


    def test_two_native_paths_cannot_share_task_identity(self):
        rows,provenance=fixture()
        mechanism=rows[0]['partitions']['$fn'][0][1]['mechanism']
        for path in (['a'],['b']):
            identity=len(mechanism['fact_order'])
            mechanism['facts']['$fn'].append((identity,dict(kind='native_binding',
                data=dict(chat='r',native_path=path,task_key='9'))))
            mechanism['fact_order'].append(identity)
        with self.assertRaisesRegex(FullProjectionError,'native.task_key_alias'):derive(rows,provenance)


    def test_reporting_runtime_identity_cannot_be_unknown(self):
        rows,provenance=fixture()
        mechanism=rows[0]['partitions']['$fn'][0][1]['mechanism'];identity=len(mechanism['fact_order'])
        mechanism['facts']['$fn'].append((identity,dict(kind='runtime_binding',
            data=dict(chat='r',runtime_id=dict(present=False),validated=True))))
        mechanism['fact_order'].append(identity)
        with self.assertRaisesRegex(FullProjectionError,'reporting.runtime_identity'):derive(rows,provenance)

    def test_reporting_revision_uses_source_counter(self):
        rows,provenance=fixture()
        mechanism=rows[0]['partitions']['$fn'][0][1]['mechanism'];identity=len(mechanism['fact_order'])
        mechanism['facts']['$fn'].append((identity,dict(kind='runtime_binding',
            data=dict(chat='r',runtime_id='runtime-a',source_revision=True,validated=True))))
        mechanism['fact_order'].append(identity)
        with self.assertRaisesRegex(FullProjectionError,'reporting.source_revision'):derive(rows,provenance)


    def test_reporting_contract_binds_target_and_properties(self):
        rows,provenance=fixture();contract=derive(rows,provenance)['scope_contract']
        report=contract['reporting_product']
        self.assertEqual(report['module'],'RuntimeNativeReportingSpec')
        self.assertEqual(len(report['module_sha256']),64)
        self.assertIn('FairReportingClosure',report['properties'])
        self.assertEqual(report['wrapper'],'RuntimeOperationalFullObserved')
        self.assertIn('unproved',contract['legacy_process_theorem'])


    def test_group_requires_every_completed_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);a=root/'a.jsonl';b=root/'b.jsonl'
            a.write_text('{}\n');b.write_text('{}\n')
            (root/'capture.json').write_text(json.dumps(dict(files=['a.jsonl','b.jsonl'])))
            completed([a,b])
            with self.assertRaisesRegex(FullProjectionError,'incomplete_group'):completed([a])



    def test_pending_child_catalog_needs_exact_creation_seam(self):
        rows,provenance=fixture();row=rows[0]
        part=row['partitions']['$fn'][0][1]
        child=dict(parent='r',depth=1,access=2,run_base=0,runs=[],
                   task=dict(present=True,run=1,st='Running'))
        part['core']['threads']['$fn'].append(('c',child))
        part['mechanism']['bindings']['$fn'].append(('c',dict(host_known=True,host='d2')))
        identity=len(part['mechanism']['facts']['$fn'])
        part['mechanism']['facts']['$fn'].append((identity,dict(kind='registry_host',
            data=dict(chat='c',host='d2',parent='r',validated=True))))
        part['mechanism']['fact_order'].append(identity)
        part['mechanism']['effects']=fn([('create',dict(effect=dict(kind='CreateChild',parent='r',child='c',depth=1,access=2)))])
        other=deepcopy(part);other['core']['threads']=fn([]);other['mechanism']['bindings']=fn([])
        row['partitions']['$fn'].append(('d2',other))
        probe=Projection(dict(constants={},provenance=provenance))
        parts,routes,threads,pending=catalog_authority(probe,row)
        self.assertNotIn('c',threads)
        self.assertEqual(pending['c']['effect_id'],'create')
        part['mechanism']['effects']=fn([])
        with self.assertRaisesRegex(FullProjectionError,'created.pending_remote'):
            catalog_authority(probe,row)


    def test_used_in_place_pair_requires_actual_receipt(self):
        rows,provenance=fixture()
        first=rows[0]['partitions']['$fn'][0][1]['core']['threads']['$fn'][0][1]
        first['reconfigure']=True;first['runs']=[dict(status='Live',cohort=dict(generation=0))]
        first['attempt']['n']=1
        next_row=deepcopy(rows[0]);next_row.update(action='Apply',args=dict(accepted=True,duplicate=False),
                                                event=dict(act='NarrowAccess',args=dict(t='r')))
        with self.assertRaisesRegex(FullProjectionError,'InPlace_pair_missing'):
            derive(rows+[next_row],provenance)


    def test_missing_native_binding_refuses(self):
        rows,provenance=fixture()
        rows[0]['partitions']['$fn'][0][1]['core']['natives']=fn([(['r','9'],dict(background=True,generation=0))])
        with self.assertRaisesRegex(FullProjectionError,'native.binding_missing'):derive(rows,provenance)


class TransferBindingTests(unittest.TestCase):
    def test_real_reader_required_before_legacy_switch(self):
        rows,provenance=fixture()
        row=rows[0];row['partitions']['$fn'][0][1]['core']['threads']=fn([])
        context=dict(constants=dict(UseCT=True,MaxRuns=0,RLMaxAtt=0),tree_threads={'r':['r']},provenance=provenance)
        result=bind_transfer_reporting(rows,context)
        self.assertIs(result['constants']['UseCT'],False)
        self.assertIs(result['transfer_constants']['UseTransferReporting'],True)
        self.assertEqual(result['transfer_projection_evidence']['images'],1)

    def test_missing_reader_source_refuses_legacy_switch(self):
        rows,provenance=fixture()
        context=dict(constants=dict(UseCT=True,MaxRuns=0,RLMaxAtt=0),tree_threads={'r':['r']},provenance=provenance)
        with self.assertRaisesRegex(FullProjectionError,'init.preexisting'):
            bind_transfer_reporting(rows,context)
        self.assertIs(context['constants']['UseCT'],True)
        self.assertNotIn('transfer_constants',context)

    def test_flag_without_source_path_refuses(self):
        rows,provenance=fixture()
        context=dict(constants=dict(UseCT=True))
        with patch('runtime_transfer_reporting_projection.project_transfer_reporting',return_value={
                'transfer_constants':dict(UseTransferReporting=True)}):
            with self.assertRaisesRegex(FullProjectionError,'source_path'):
                bind_transfer_reporting(rows,context)
        self.assertIs(context['constants']['UseCT'],True)

    def test_disabled_product_refuses_legacy_switch(self):
        rows,provenance=fixture();result=transfer_fixture(rows,{})
        result['transfer_constants']['UseTransferReporting']=False
        context=dict(constants=dict(UseCT=True))
        with patch('runtime_transfer_reporting_projection.project_transfer_reporting',return_value=result):
            with self.assertRaisesRegex(FullProjectionError,'required'):
                bind_transfer_reporting(rows,context)
        self.assertIs(context['constants']['UseCT'],True)

    def test_missing_row_refuses_source_path(self):
        rows,provenance=fixture();rows.append(deepcopy(rows[0]))
        result=transfer_fixture(rows[:1],{})
        context=dict(constants=dict(UseCT=True))
        with patch('runtime_transfer_reporting_projection.project_transfer_reporting',return_value=result):
            with self.assertRaisesRegex(FullProjectionError,'source_path'):
                bind_transfer_reporting(rows,context)
        self.assertIs(context['constants']['UseCT'],True)


class QuestionBuildIntegrationTests(unittest.TestCase):
    def raw_path(self,root,missing_issue=False):
        from test_runtime_question_issuance import path
        raw=path()
        for index,row in enumerate(raw):
            row['seq']=index
            if missing_issue:
                row['snapshot']['facts']=[f for f in row['snapshot']['facts'] if f['kind']!='question_input_issued']
        source=root/'raw.jsonl';source.write_text(''.join(json.dumps(r)+'\n' for r in raw))
        (root/'capture.json').write_text(json.dumps(dict(files=[source.name])))
        return raw,source

    def test_raw_issuance_precedes_generic_normalization_and_binds_helper(self):
        with tempfile.TemporaryDirectory() as temporary:
            raw,source=self.raw_path(Path(temporary));order=[]
            def normalized(paths):
                order.append('normalized')
                return ([dict(row={},parts={}) for r in raw],1)
            def derive_context(rows,provenance):
                return dict(provenance=provenance,source_map=[dict(device='device',device_row=i) for i in range(len(rows))])
            from build_runtime_context import question_issuance as actual
            def checked(rows):
                order.append('raw_issuance')
                self.assertEqual(rows[-1]['args']['event']['args']['child'],7)
                return actual(rows)
            with patch('build_runtime_context.merge_raw',return_value=dict(rows=raw)), \
                 patch('build_runtime_context.question_issuance',side_effect=checked), \
                 patch('build_runtime_context.project_global',side_effect=normalized), \
                 patch('build_runtime_context.derive',side_effect=derive_context):
                result=build([source])
            self.assertEqual(order,['raw_issuance','normalized'])
            self.assertEqual(result['question_issuance_evidence'][0]['child'],'child')
            self.assertEqual(result['question_issuance_evidence'][0]['raw_row'],3)
            import hashlib
            expected=hashlib.sha256((Path(__file__).parent/'runtime_question_issuance.py').read_bytes()).hexdigest()
            self.assertEqual(result['question_issuance_contract']['module_sha256'],expected)
            self.assertEqual(result['source_map'][3]['raw_seq'],3)

    def test_missing_raw_issue_stops_before_normalization_with_safe_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            raw,source=self.raw_path(Path(temporary),missing_issue=True)
            with patch('build_runtime_context.merge_raw',return_value=dict(rows=raw)), \
                 patch('build_runtime_context.project_global') as normalized:
                with self.assertRaises(FullProjectionError) as caught:build([source])
            self.assertEqual(caught.exception.action,'QuestionIssuance')
            self.assertEqual(caught.exception.row,3)
            self.assertEqual(caught.exception.field,'question_issuance.issued_identity')
            normalized.assert_not_called()


if __name__=='__main__':unittest.main()
