#!/usr/bin/env python3
"""Check raw evidence joins and source ownership, not a target state walk."""
from copy import deepcopy
import json
from pathlib import Path
import unittest
from native_reporting_projection import project_native_reporting,fn,aset
from test_native_question import base

class ProjectionFailure(ValueError):pass

def fail(field):raise ProjectionFailure(field)
def context():
    return dict(constants=dict(Subs=aset(['1','2']),NSMaxCrash=0),
        native_slots={'root':{'["n"]':'1','["m"]':'2'}},
        activation_order={'root':{'["n"]':['act-n'],'["m"]':['act-m']}},
        provenance=dict(kind='verified_test'))

def fixture():
    rows=[];facts=[];core=base();revision=0
    def row(action='Observe',event=None):
        r=dict(action=action,args=dict(accepted=True,duplicate=False),
            partitions=fn([('dev',dict(core=deepcopy(core),mechanism=dict(facts=fn(deepcopy(facts)),fact_order=[i for i,f in facts])))]))
        if event:r['event']=deepcopy(event)
        rows.append(r)
    def fact(identity,kind,data):facts.append((identity,dict(kind=kind,data=deepcopy(data))));row()
    def install(sid,run):
        nonlocal revision
        revision+=1
        fact('binding-'+sid,'runtime_binding',dict(chat='root',runtime_id=sid,run=run,attempt=1,effective_access=2,
             origin_id='msg',operation='install',validated=True,source_id='engine',source_revision=revision))
    def pe(sid,seq,kind,path=None,status=None,ns=(),bg=None,activation=None,root=True):
        fact(f'{sid}:{seq}','provider_event',dict(chat='root',provider_run_id=sid,stream_seq=seq,event_id=f'{sid}:{seq}',
             event_kind=kind,native_path=path,background=bg,activation_id=activation,status=status,replay_of=None,
             observer=dict(root_turn_active=root,natives=list(ns))))
    def native(path,status='running',activation=None):
        return dict(native_path=[path],native_key=path,status=status,background=True,activation_id=activation,progress_seen=False)
    def bind(path,key):
        fact('native-bind-'+key,'native_binding',dict(chat='root',task_key=key,native_path=[path],tool_id=path,
             doc_id='doc-'+path,chip_doc_id='root',own_background=True,effective_background=True))
    def commit(path,key,status):
        fact('doc-'+key+'-'+status,'native_doc_commit',dict(chat='root',task_key=key,native_path=[path],tool_id=path,
             doc_id='doc-'+path,chip_doc_id='root',chip_ref='doc-'+path,chip_status=status,doc_hash='a'*64,chip_doc_hash='b'*64))
    def start(sid,path,key,run):
        pe(sid,1,'stream_opened',root=None)
        pe(sid,2,'session_started')
        pe(sid,3,'native_spawn',[path],ns=[native(path,'spawned')],bg=True)
        bind(path,key)
        pe(sid,4,'native_started',[path],ns=[native(path,activation='act-'+path)],bg=True,activation='act-'+path)
        n=dict(attempt=dict(run=dict(thread='root',ord=run),n=1),background=True,ancestor='none',status='Running',delivery='Pending',generation=0)
        core['natives']['$fn'].append((['root',key],n));core['native_order'].append(['root',key])
        row('Apply',dict(act='NativeStart',args=dict(parent='root',key=key,background=True)))
        commit(path,key,'running')
    row('Init');fact('catalog','provider_stream_catalog',dict(chat=None,complete=True,streams=[]));install('A',1);start('A','n','1',1)
    first=len(rows)-1
    pe('A',5,'root_terminal',status='completed',ns=[native('n',activation='act-n')],root=False)
    core['threads']['$fn'][0][1]['attempt']['emitted']=True
    row('Apply',dict(act='ProviderTerminal',args=dict(t='root',run=1,attempt=1,outcome='Completed')))
    core['threads']['$fn'][0][1]['runs'][-1]['status']='Completed'
    row('Apply',dict(act='IngestEnd',args=dict(t='root',ev=dict(node='Root',run=1,attempt=1,outcome='Completed'))))
    core['threads']['$fn'][0][1]['runs'].append(deepcopy(base()['threads']['$fn'][0][1]['runs'][0]))
    core['threads']['$fn'][0][1]['attempt']['emitted']=False
    row('Apply',dict(act='StartFromQueue',args=dict(t='root')))
    install('B',2);start('B','m','2',2)
    before_loss=len(rows)-1
    pe('A',6,'stream_closed',ns=[native('n',activation='act-n')],root=None)
    core['natives']['$fn'][0][1].update(status='Failed',delivery='Claimed')
    row('Apply',dict(act='NativeEnd',args=dict(parent='root',key='1',failed=True)))
    commit('n','1','failed')
    return rows,first,before_loss

class NativeReportingTests(unittest.TestCase):
    def project(self,rows,ctx=None):return project_native_reporting(rows,ctx or context(),'root','dev',fail)
    def append_fact(self,rows,identity,kind,data):
        row=deepcopy(rows[-1]);row.update(action='Observe');row.pop('event',None)
        mechanism=dict(row['partitions']['$fn'])['dev']['mechanism']
        mechanism['facts']['$fn'].append((identity,dict(kind=kind,data=data)));mechanism['fact_order'].append(identity)
        return rows+[row]
    def test_maps_all_thirteen_engine_fields_with_source_evidence(self):
        rows,first,_=fixture();p=self.project(rows[:first+1])
        self.assertEqual(len(p['engine_fields']),13)
        self.assertFalse(p['unsupported_fields'])
        self.assertEqual(dict(p['engine_fields']['nEep']['$fn'])['1'],1)
        self.assertEqual(dict(p['engine_fields']['nTask']['$fn'])['1'],'running')
        self.assertTrue(p['reporting_complete'])
    def test_closing_old_stream_keeps_new_root_and_native_live(self):
        rows,_,_=fixture();p=self.project(rows);s=p['reporting_state']
        self.assertEqual(dict(s['sources']['$fn'])['A']['reporting'],'disconnected')
        self.assertEqual(dict(s['sources']['$fn'])['B']['reporting'],'connected')
        self.assertTrue(dict(s['runs']['$fn'])['root']['active'])
        tasks={tuple(k):v for k,v in s['tasks']['$fn']}
        self.assertEqual(tasks[('root','1')]['status'],'failed')
        self.assertEqual(tasks[('root','2')]['status'],'running')
        self.assertTrue(all(x['process']=='unknown' for k,x in s['sources']['$fn']))
    def test_installed_source_has_no_invented_issued_snapshot(self):
        rows,first,_=fixture();source=dict(self.project(rows[:first+1])['reporting_state']['sources']['$fn'])['A']
        self.assertFalse(source['requestIssued'])
        self.assertEqual({k:source[k] for k in ('issuedOrigin','issuedRun','issuedAttempt','issuedRevision','issuedRequestAccess')},
                         dict(issuedOrigin='none',issuedRun=0,issuedAttempt=0,issuedRevision=0,issuedRequestAccess=0))
    def test_config_and_rebind_preserve_exact_last_issued_request(self):
        rows,_,_=fixture()
        binding=dict(chat='root',runtime_id='B',run=2,attempt=1,effective_access=2,
                     origin_id='issued',operation='issue',validated=True,source_id='engine',source_revision=3)
        rows=self.append_fact(rows,'B-issue','runtime_binding',binding)
        source=dict(self.project(rows)['reporting_state']['sources']['$fn'])['B']
        snapshot={k:source[k] for k in ('issuedOrigin','issuedRun','issuedAttempt','issuedRevision','issuedRequestAccess')}
        self.assertEqual(snapshot,dict(issuedOrigin='issued',issuedRun=2,issuedAttempt=1,issuedRevision=3,issuedRequestAccess=2))
        for operation,revision,run,attempt,origin in [('config',4,2,1,'queued'),('rebind',5,3,2,'confirmed')]:
            rows=self.append_fact(rows,'B-'+operation,'runtime_binding',dict(binding,operation=operation,
                 source_revision=revision,run=run,attempt=attempt,effective_access=0,origin_id=origin))
            source=dict(self.project(rows)['reporting_state']['sources']['$fn'])['B']
            self.assertEqual({k:source[k] for k in snapshot},snapshot)
            self.assertEqual((source['origin'],source['issuedAccess'],source['revision']),(origin,0,revision))
        self.assertEqual((source['run'],source['attempt']),(3,2))
    def test_catalog_cannot_regress_to_incomplete(self):
        rows,first,_=fixture();rows=rows[:first+1]
        rows=self.append_fact(rows,'incomplete-catalog','provider_stream_catalog',dict(chat=None,complete=False,streams=[]))
        with self.assertRaisesRegex(ProjectionFailure,'reporting.catalog.completeness_regressed'):
            self.project(rows)
    def root_loss_fixture(self,close_native):
        rows,first,_=fixture();rows=rows[:first+1];seq=4
        def provider(kind,status=None,path=None,root=False):
            nonlocal rows,seq
            seq+=1
            data=dict(chat='root',provider_run_id='A',stream_seq=seq,event_id=f'A:{seq}',event_kind=kind,
                 native_path=path,background=True if path else None,activation_id='act-n' if path else None,
                 status=status,replay_of=None,observer=dict(root_turn_active=root,natives=[]))
            rows=self.append_fact(rows,f'A:{seq}','provider_event',data)
        if close_native:
            provider('native_terminal','completed',['n'],True)
            row=deepcopy(rows[-1]);row.update(action='Apply',event=dict(act='NativeEnd',args=dict(parent='root',key='1',failed=False)))
            dict(row['partitions']['$fn'])['dev']['core']['natives']['$fn'][0][1].update(status='Completed')
            rows.append(row)
        provider('root_terminal','completed');provider('stream_closed',root=None)
        row=deepcopy(rows[-1]);row.update(action='Apply',event=dict(act='ProviderTerminal',args=dict(t='root',run=1,attempt=1,outcome='Completed')))
        dict(row['partitions']['$fn'])['dev']['core']['threads']['$fn'][0][1]['attempt']['emitted']=True
        rows.append(row)
        row=deepcopy(rows[-1]);row.update(action='Apply',event=dict(act='IngestEnd',args=dict(t='root',ev=dict(node='Root',run=1,attempt=1,outcome='Completed'))))
        dict(row['partitions']['$fn'])['dev']['core']['threads']['$fn'][0][1]['runs'][-1]['status']='Completed'
        return rows,row
    def test_root_ingest_owns_final_empty_source_loss(self):
        rows,ingest=self.root_loss_fixture(True)
        self.assertEqual([e['kind'] for e in self.project(rows)['reporting_state']['pending']],['rootEnd','sourceLoss'])
        state=self.project(rows+[ingest])['reporting_state']
        self.assertEqual(state['pending'],[])
        self.assertEqual(state['lastCause'],dict(kind='sourceLoss',source='A',changed=aset([])))
        self.assertEqual(dict(state['runs']['$fn'])['root']['endBy'],'root')
        self.assertEqual(dict(state['sources']['$fn'])['A']['process'],'unknown')
        for action in ('Observe','MarkCommand'):
            idle=deepcopy(ingest);idle['action']=action;idle.pop('event')
            self.assertEqual(self.project(rows+[ingest,idle])['reporting_state'],state)
    def test_root_ingest_keeps_loss_while_owned_native_runs(self):
        rows,ingest=self.root_loss_fixture(False)
        state=self.project(rows+[ingest])['reporting_state']
        self.assertEqual([e['kind'] for e in state['pending']],['sourceLoss'])
        self.assertEqual(state['lastCause']['kind'],'rootEnd')
        self.assertEqual(dict(state['sources']['$fn'])['A']['process'],'unknown')
        idle=deepcopy(ingest);idle['action']='Observe';idle.pop('event')
        self.assertEqual(self.project(rows+[ingest,idle])['reporting_state'],state)
    def test_unknown_activation_blocks_only_original_epoch_fields(self):
        rows,first,_=fixture()
        for row in rows:
            for _,f in dict(row['partitions']['$fn'])['dev']['mechanism']['facts']['$fn']:
                if f['kind']=='provider_event':
                    f['data']['activation_id']=None
                    for n in f['data']['observer']['natives']:n['activation_id']=None
        p=self.project(rows[:first+1])
        self.assertNotIn('nEep',p['engine_fields']);self.assertIn('nEep',p['unsupported_fields'])
        self.assertIsNotNone(p['reporting_state'])
    def test_rejects_fake_persisted_chip_reference(self):
        rows,first,_=fixture()
        for row in rows:
            for _,f in dict(row['partitions']['$fn'])['dev']['mechanism']['facts']['$fn']:
                if f['kind']=='native_doc_commit':f['data']['chip_ref']='other-doc'
        with self.assertRaisesRegex(ProjectionFailure,'nTask.chip_ref'):self.project(rows[:first+1])
    def test_rejects_reordered_accepted_source_handles(self):
        rows,_,_=fixture()
        rows=[r for r in rows if r.get('event',{}).get('act')!='IngestEnd']
        with self.assertRaisesRegex(ProjectionFailure,'nEndBy.accepted_root_end|nInbox.source_loss_order'):self.project(rows)
    def test_refuses_process_recovery_without_scope_or_fact(self):
        rows,first,_=fixture();ctx=context();ctx['constants']['NSMaxCrash']=1
        p=self.project(rows[:first+1],ctx)
        self.assertNotIn('nRec',p['engine_fields']);self.assertIn('nRec',p['unsupported_fields'])
    def test_preserves_actual_delivery_generation_as_non_epoch(self):
        rows,first,_=fixture()
        for row in rows:
            for key,n in dict(row['partitions']['$fn'])['dev']['core']['natives']['$fn']:n['generation']=17
        p=self.project(rows[:first+1]);self.assertEqual(dict(p['engine_fields']['nEep']['$fn'])['1'],1)
    def test_admission_without_causal_revision_stays_explicitly_partial(self):
        rows,_,_=fixture();data=dict(chat='root',runtime_id='B',provider_run_id='B',handle_run=2,handle_attempt=1,
             current_run=2,current_attempt=1,effective_access=2,origin_id='msg',disposition='accepted',validated=True)
        p=self.project(self.append_fact(rows,'admit','agent_admission',data))
        self.assertFalse(p['reporting_complete']);self.assertIn('reporting.admitted.revision',p['unsupported_fields'])
        self.assertEqual(dict(p['reporting_state']['admitted']['$fn'])['root']['source'],'B')
    def test_stale_binding_fact_does_not_replace_newer_handle(self):
        rows,_,_=fixture();old=dict(chat='root',runtime_id='A',run=1,attempt=1,effective_access=2,
             origin_id='msg',operation='install',validated=True,source_id='engine',source_revision=1)
        p=self.project(self.append_fact(rows,'old-binding-duplicate','runtime_binding',old))
        self.assertEqual(dict(p['reporting_state']['handles']['$fn'])['root'],'B')
    def test_duplicate_source_revision_with_different_body_fails(self):
        rows,_,_=fixture();bad=dict(chat='root',runtime_id='A',run=1,attempt=1,effective_access=0,
             origin_id='other',operation='config',validated=True,source_id='engine',source_revision=1)
        with self.assertRaisesRegex(ProjectionFailure,'runtime_binding_revision_conflict'):
            self.project(self.append_fact(rows,'bad-revision','runtime_binding',bad))
    def test_absent_catalog_does_not_prove_no_other_sources(self):
        rows,first,_=fixture()
        for row in rows:
            m=dict(row['partitions']['$fn'])['dev']['mechanism']
            m['facts']['$fn']=[(i,f) for i,f in m['facts']['$fn'] if i!='catalog']
            m['fact_order']=[i for i in m['fact_order'] if i!='catalog']
        p=self.project(rows[:first+1]);self.assertFalse(p['reporting_complete'])
        self.assertIn('reporting.catalog',p['unsupported_fields'])
    def test_unrelated_observation_keeps_actual_consumption_cause(self):
        rows,first,_=fixture()
        at=next(i for i,r in enumerate(rows) if r.get('event',{}).get('act')=='NativeStart')
        before=self.project(rows[:at+1])['reporting_state']['lastCause']
        idle=deepcopy(rows[at]);idle['action']='Observe';idle.pop('event')
        after=self.project(rows[:at+1]+[idle])['reporting_state']['lastCause']
        self.assertEqual(before,dict(kind='spawn',source='A',changed=aset([])))
        self.assertEqual(after,before)
    def test_cross_path_completion_can_overtake_pending_terminal(self):
        rows,first,_=fixture();rows=rows[:first+1]
        terminal=dict(chat='root',provider_run_id='A',stream_seq=5,event_id='A:5',event_kind='native_terminal',
             native_path=['n'],background=True,activation_id='act-n',status='completed',replay_of=None,
             observer=dict(root_turn_active=True,natives=[]))
        rows=self.append_fact(rows,'A:5','provider_event',terminal)
        spawn=dict(terminal,stream_seq=6,event_id='A:6',event_kind='native_spawn',native_path=['m'],activation_id=None,status='spawned')
        rows=self.append_fact(rows,'A:6','provider_event',spawn)
        rows=self.append_fact(rows,'bind-m','native_binding',dict(chat='root',task_key='2',native_path=['m'],tool_id='m',
             doc_id='doc-m',chip_doc_id='root',own_background=True,effective_background=True))
        core=dict(rows[-1]['partitions']['$fn'])['dev']['core'];new=deepcopy(core['natives']['$fn'][0][1])
        core['natives']['$fn'].append((['root','2'],new));core['native_order'].append(['root','2'])
        rows[-1].update(action='Apply',event=dict(act='NativeStart',args=dict(parent='root',key='2',background=True)))
        p=self.project(rows)
        self.assertTrue(p['reporting_complete'])
        self.assertIn('source FIFO',p['unsupported_fields']['nInbox'])
        self.assertEqual(p['reporting_state']['pending'][0]['kind'],'nativeEnd')
    def test_accepted_engine_eof_terminal_joins_closed_bound_source(self):
        rows,first,_=fixture();rows=rows[:first+1]
        closed=dict(chat='root',provider_run_id='A',stream_seq=5,event_id='A:5',event_kind='stream_crashed',
             native_path=None,background=None,activation_id=None,status=None,replay_of=None,
             observer=dict(root_turn_active=None,natives=[]))
        rows=self.append_fact(rows,'A:5','provider_event',closed)
        terminal=deepcopy(rows[-1]);terminal.update(action='Apply',event=dict(act='ProviderTerminal',args=dict(t='root',run=1,attempt=1,outcome='Failed')))
        dict(terminal['partitions']['$fn'])['dev']['core']['threads']['$fn'][0][1]['attempt']['emitted']=True
        p=self.project(rows+[terminal]);state=p['reporting_state']
        self.assertEqual([e['kind'] for e in state['pending']],['sourceLoss','rootEnd'])
        self.assertEqual(state['pending'][-1]['source'],'A')
        self.assertEqual(state['pending'][-1]['out'],'failed')
        self.assertEqual(dict(state['sources']['$fn'])['A']['process'],'unknown')
        self.assertEqual(p['provenance'][-2]['kind'],'engineEOFRootTerminal')

if __name__=='__main__':unittest.main()
