"""Protect strict common-target routing, source visibility, and proof bounds."""
from contextlib import ExitStack
from copy import deepcopy
from pathlib import Path
import re
import json
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import check_full_runtime as checker
from emit_operational_full_projection import emit, HERE
from runtime_full_projection import FullProjectionError
from runtime_provenance import local_tla_dependencies


def rows_and_map():
    rows=[dict(action='Apply',transaction=dict(id='tx',index=i,size=2,visible=i==1,known=True)) for i in range(2)]
    sources=[dict(raw_row=raw,macro_substep=i,transaction=deepcopy(rows[raw]['transaction']),physical_visible=i==2)
             for i,raw in enumerate([0,1,1])]
    mapped=dict(images=[{}, {}, {}])
    for prefix in ('','reporting_','transfer_'):mapped[prefix+'source_map']=deepcopy(sources)
    return rows,mapped


class StrictPipelineTests(unittest.TestCase):
    def run_pipeline(self,exit_codes=(0,0),context_matches=True,*,saved_context=None,built_context=None):
        rows,mapped=rows_and_map();calls=[]
        args=SimpleNamespace(provenance=Path('receipt'),captures=[Path('capture')],jar=Path('jar'),
            source_root=Path('source'),model_root=Path('models'),context=Path('context'),out=Path('output'))
        provenance=dict(provenance_sha256='a'*64,scopes=[dict(synthetic=True)])
        steps=[dict(row=row,parts={'$fn':[]}) for row in rows]
        def tlc(*args):calls.append(args);return exit_codes[len(calls)-1]
        with ExitStack() as stack:
            stack.enter_context(patch.object(checker,'validate_provenance',return_value=provenance))
            stack.enter_context(patch.object(checker,'validate_context',return_value=saved_context if saved_context is not None else {'source':'context'}))
            stack.enter_context(patch.object(checker,'build_context',return_value=built_context if built_context is not None else {'source':'context' if context_matches else 'changed'}))
            project=stack.enter_context(patch.object(checker,'project_global',return_value=(steps,1)))
            exact=stack.enter_context(patch.object(checker,'emit_global'))
            mapped_call=stack.enter_context(patch.object(checker,'project_full',return_value=mapped))
            emission=stack.enter_context(patch.object(checker,'emit_operational'))
            stack.enter_context(patch.object(checker,'run_tlc',side_effect=tlc))
            result=checker.check(args)
        return result,calls,project,exact,mapped_call,emission

    def test_common_target_runs_after_exact_prerequisite(self):
        result,calls,project,exact,mapped,emission=self.run_pipeline()
        self.assertEqual(result,0)
        self.assertEqual([call[2] for call in calls],['RuntimeGlobalObserved.tla','CapturedOperationalFullRuntime.tla'])
        project.assert_called_once_with([Path('capture')],require_transactions=True)
        emission.assert_called_once()
        self.assertEqual(emission.call_args.kwargs['provenance']['provenance_sha256'],'a'*64)
        for flag in ('universal_refinement','implementation_fair_closure','all_causal_orders','dogfood_coverage','week_coverage'):
            self.assertIs(calls[-1][-1][flag],False)

    def test_raw_failure_never_maps_or_emits_common_target(self):
        result,calls,project,exact,mapped,emission=self.run_pipeline((12,))
        self.assertEqual(result,12);self.assertEqual(len(calls),1)
        mapped.assert_not_called();emission.assert_not_called()

    def test_common_target_failure_is_preserved(self):
        self.assertEqual(self.run_pipeline((0,13))[0],13)

    def test_changed_context_is_rejected(self):
        with self.assertRaisesRegex(ValueError,'not derived'):self.run_pipeline(context_matches=False)

    def test_json_round_trip_preserves_function_pairs(self):
        built=dict(constants=dict(TreeThreads={'$fn':[('root',{'$set':['root','child']})]},
                                  RootDevices={'$fn':[('root','device')]}),
                   reporting_constants=dict(NativeIds={'$set':[['root','task']]}),enabled=True,bound=1)
        saved=json.loads(json.dumps(built))
        self.assertNotEqual(saved,built)
        self.assertEqual(self.run_pipeline(saved_context=saved,built_context=built)[0],0)
        for mutate in (
            lambda value:value['constants']['RootDevices']['$fn'][0].__setitem__(1,'other-device'),
            lambda value:value['reporting_constants']['NativeIds']['$set'][0].__setitem__(1,'other-task'),
            lambda value:value.__setitem__('bound',2),
            lambda value:value.__setitem__('enabled',1),
            lambda value:value.__setitem__('bound',True),
            lambda value:value.__setitem__('extra',None),
            lambda value:value.pop('enabled')):
            changed=deepcopy(saved);mutate(changed)
            with self.assertRaisesRegex(ValueError,'not derived'):
                self.run_pipeline(saved_context=changed,built_context=built)

    def test_only_last_macro_of_final_group_row_is_visible(self):
        rows,mapped=rows_and_map();checker.validate_source_visibility(mapped,rows)
        for prefix in ('','reporting_','transfer_'):
            mutant=deepcopy(mapped);mutant[prefix+'source_map'][1]['physical_visible']=True
            with self.assertRaisesRegex(FullProjectionError,'source_map_visibility'):
                checker.validate_source_visibility(mutant,rows)

    def test_unknown_missing_and_changed_groups_are_rejected(self):
        for change in ('unknown','missing','changed'):
            rows,mapped=rows_and_map();mutant=deepcopy(mapped)
            if change=='unknown':rows=deepcopy(rows);rows[0]['transaction']['known']=False
            elif change=='missing':mutant['transfer_source_map']=mutant['transfer_source_map'][1:]
            else:mutant['reporting_source_map'][0]['transaction']['id']='other'
            with self.assertRaises(FullProjectionError):checker.validate_source_visibility(mutant,rows)


class CommonEmitterTests(unittest.TestCase):
    def projection(self):
        text=(HERE/'RuntimeOperationalFullObserved.tla').read_text()
        names=set(re.findall(r'\b[A-Za-z][A-Za-z0-9_]*\b',text.split('CONSTANTS',1)[1].split('VARIABLE',1)[0]))
        names-={'Images','Actions','ReportingImages','ReportingActions','TransferImages','TransferActions'}
        result=dict(constants={name:0 for name in names},reporting_constants={},transfer_constants={})
        result['constants'].update(UseNS=False,UseCT=False,UseReporting=True,UseTransferReporting=True,UsePL=True)
        for prefix in ('','reporting_','transfer_'):
            result[prefix+'images']=[{}];result[prefix+'actions']=[]
            result[prefix+'source_map']=[dict(raw_row=0,macro_substep=0)]
        return result

    def test_emits_all_common_named_type_and_safety_checks(self):
        with tempfile.TemporaryDirectory() as folder:
            out=Path(folder);receipt=emit(self.projection(),out)
            cfg=(out/'CapturedOperationalFullRuntime.cfg').read_text()
            for name in ('MappedType','MappedSpec','NamedPath','ReportingTransitionSafety','NativeLayerEnabled',
                         'TransferLayerEnabled','ReportingSafety','TransferSafety','OnlyRootEndsRun','OrphansClosed',
                         'NoFalseProcessDeath','LossClosesOnlySource','BackgroundSurvivesTurn','AncestorTerminalClosesForeground'):
                self.assertIn(name,cfg)
            self.assertIs(receipt['implementation_refinement'],False)
            self.assertIs(receipt['implementation_fair_closure'],False)

    def test_missing_required_product_is_rejected(self):
        for flag in ('UseReporting','UseTransferReporting','UsePL'):
            projection=self.projection();projection['constants'][flag]=False
            with tempfile.TemporaryDirectory() as folder, self.assertRaisesRegex(ValueError,'product_flags'):
                emit(projection,Path(folder))

    def test_provenance_closure_contains_all_common_targets(self):
        names=local_tla_dependencies(HERE/'RuntimeOperationalFullObserved.tla')
        self.assertTrue({'RuntimeOperationalCompositionSpec.tla','RuntimeOperationalCoreSpec.tla',
            'RuntimeNativeReportingSpec.tla','RuntimeTransferReportingSpec.tla'}<=names)


if __name__=='__main__':unittest.main()
