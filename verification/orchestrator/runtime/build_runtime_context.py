#!/usr/bin/env python3
"""Build a private finite context from completed raw capture groups.

Catalogs name actual captured IDs. Bounds count captured inputs or read actual
runtime configuration. No target reducer or successor relation is consulted.
Missing source capability, placement, access or identity facts stop generation.
Named checker scope contracts bind finite flags outside product configuration.
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path

from check_global_runtime import project_global
from merge_runtime import merge as merge_raw
from runtime_question_issuance import question_issuance, QuestionIssuanceError
from runtime_full_projection import Projection, FullProjectionError, aset, entries, fn

HERE=Path(__file__).resolve().parent
POLICY={
 'Forks':'forks','Spawns':'spawns','Cuts':'cuts','BudgetFail':'budget_fail',
 'Questions':'questions','QSplit':'split_questions','StopCheckAnc':'stop_check_anc',
 'LateAck':'late_ack','PartialCrash':'partial_crash','FixLimitRelease':'fix_limit_release',
 'FixWakeSkipsFailHold':'fix_wake_skips_fail_hold','ToolNodes':'tool_nodes',
}


                                                                            
                                                                              
SCOPE_FLAGS={
 'Forks':(True,'enable supported fork/merge action family'),
 'Spawns':(True,'enable supported delegated context action family'),
 'Cuts':(True,'enable explicit packet selection and omission'),
 'BudgetFail':(True,'enable explicit transfer budget refusal'),
 'Questions':(True,'enable source question operations'),
 'QSplit':(True,'use host-local question messages'),
 'StopCheckAnc':(False,'source tool guard checks local stop state'),
 'LateAck':(True,'allow stale reply arrival; current identity still guards Ack'),
 'PartialCrash':(True,'allow device-local failure sets'),
 'FixLimitRelease':(True,'retain target usage-limit release requirement'),
 'FixWakeSkipsFailHold':(True,'retain target wake hold rule'),
 'ToolNodes':(True,'enable source child-node lifecycle'),
 'DupDelivery':(True,'allow duplicate source delivery'),
 'Background':(True,'allow source background native work'),
 'SilentKill':(True,'allow native closure without a separate tool end'),
 'Crash':(True,'allow source engine crash and recovery'),
}


def scope_contract():
    manifest=HERE/'full-spec-source-manifest.json'
    contract=dict(name='runtime-v2-full-observed-enabled-families',version=1,
        scope='finite captured path; no universal refinement, process-death or dogfood claim',
        source_manifest_sha256=sha(manifest),
        flags={key:dict(value=value,rationale=reason) for key,(value,reason) in SCOPE_FLAGS.items()},
        mutant=dict(value='none',rationale='all target mutants are disabled'),
        bounds='runtime Config for structural caps; captured accepted input counts and ID catalogs for finite scenario bounds',
        legacy_process_model=False,reporting_product_required=True,
        legacy_transfer_model=False,transfer_product_required=True,
        legacy_transfer_theorem='separate and unproved by retained request observations',
        transfer_product=dict(module='RuntimeTransferReportingSpec',
            module_sha256=sha(HERE/'RuntimeTransferReportingSpec.tla'),
            reader='runtime_transfer_reporting_projection.py',
            reader_sha256=sha(HERE/'runtime_transfer_reporting_projection.py'),
            wrapper='RuntimeOperationalFullObserved',
            properties=['MappedSpec','NamedPath','MappedType','TransferSafety',
                        'TransferLayerEnabled']),
        legacy_process_theorem='separate and unproved by stream observations',
        reporting_product=dict(module='RuntimeNativeReportingSpec',
            module_sha256=sha(HERE/'RuntimeNativeReportingSpec.tla'),
            wrapper='RuntimeOperationalFullObserved',
            properties=['MappedSpec','NamedPath','MappedType','ReportingSafety',
                        'NativeLayerEnabled','FairReportingClosure']),
        activation_boundary='unknown activation stays NoActivation; legacy process-NS epoch gate remains separate',
        process_boundary='stream EOF is not proof of provider process death')
    contract['sha256']=hashlib.sha256(json.dumps(contract,sort_keys=True,separators=(',',':')).encode()).hexdigest()
    return contract


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def known(value):
    if isinstance(value,dict) and 'present' in value:
        return value.get('value') if value['present'] else None
    return value


def fail(field,row=0,action='Context'):
    raise FullProjectionError(row,action,field)


def completed(paths):
    """Require every selected file in each completed exporter group."""
    selected={p.resolve() for p in paths}
    if len(selected)!=len(paths):fail('captures.duplicate')
    for path in paths:
        sidecar=path.parent/'capture.json'
        if not sidecar.is_file():fail('captures.completed_group')
        data=json.loads(sidecar.read_text())
        names=data.get('files',data.get('filenames',[]))
        if not names or any(not isinstance(x,str) or Path(x).name!=x for x in names):
            fail('captures.completed_group')
        group={(path.parent/name).resolve() for name in names}
        if path.resolve() not in group or not group<=selected:fail('captures.incomplete_group')
        if any(not p.is_file() for p in group):fail('captures.group_file')


def catalog_authority(probe,row):
    """Keep pending child names only with the exact captured creation seam.

    A pending name is not an executing target state. Its catalog parent comes
    from registry + parent task + CreateChild. No provider truth is supplied.
    """
    try:
        parts,routes,threads=probe.authority(row)
        return parts,routes,threads,{}
    except FullProjectionError as error:
        if error.field!='created.pending_remote':raise
    parts=dict(entries(row['partitions']));routes={};registry={}
    for device,part in parts.items():
        for chat,binding in entries(part['mechanism']['bindings']):
            if binding['host_known']:
                host=binding['host']
                if chat in routes and routes[chat]!=host:fail('host.conflict')
                routes[chat]=host
    for device,identity,fact in probe.facts(parts):
        if fact['kind']!='registry_host':continue
        data=fact['data']
        if data.get('validated') is not True:fail('host.validated')
        chat=data['chat'];host=data['host']
        if chat in routes and routes[chat]!=host:fail('host.conflict')
        routes[chat]=host;registry[(device,chat)]=data
    threads={};pending={}
    for chat,host in routes.items():
        if host not in parts:fail('host.partition')
        target=dict(entries(parts[host]['core']['threads']))
        if chat in target:threads[chat]=target[chat];continue
        witnesses=[]
        for device,part in parts.items():
            local=dict(entries(part['core']['threads']))
            if chat not in local or (device,chat) not in registry:continue
            th=local[chat];parent=th['parent'];route=registry[(device,chat)]
            if known(route.get('parent'))!=parent or routes.get(parent)!=device or parent not in local:continue
            task=th['task']
            if th['run_base']+len(th['runs'])!=0 or not task['present'] or task['run']!=1 or task['st']!='Running':continue
            matches=[identity for identity,e in entries(part['mechanism']['effects'])
                if e['effect'].get('kind')=='CreateChild' and e['effect'].get('parent')==parent
                and e['effect'].get('child')==chat and e['effect'].get('depth')==th['depth']
                and e['effect'].get('access')==th['access']]
            if len(matches)==1:witnesses.append(dict(parent=parent,source_device=device,effect_id=matches[0],host=host))
        if len(witnesses)!=1:fail('created.pending_remote')
        pending[chat]=witnesses[0]
    observed={chat for p in parts.values() for chat,th in entries(p['core']['threads'])}
    if not observed<=routes.keys():fail('host.unknown')
    return parts,routes,threads,pending


def derive(rows,provenance):
    """Derive catalogs and fixed finite constants from normalized source rows."""
    if not rows:fail('rows.empty')
    probe=Projection(dict(constants={},provenance=provenance))
    probe.prefix=rows
    probe.row=len(rows)-1;probe.action=rows[-1].get('action','Unknown')
    parts,routes,threads,pending=catalog_authority(probe,rows[-1])
    parents={t:th['parent'] for t,th in threads.items()}
    parents.update((chat,data['parent']) for chat,data in pending.items())
    if not threads:fail('created.root')
    roots=sorted(t for t,parent in parents.items() if parent=='none')
    if not roots:fail('created.root')
    tree_threads={r:[] for r in roots}
    for chat in sorted(parents):
        path=[];cursor=chat
        while parents[cursor]!='none':
            if cursor in path:fail('parent.cycle')
            path.append(cursor);cursor=parents[cursor]
            if cursor not in parents:fail('parent.unknown')
        if cursor not in tree_threads:fail('created.root')
        tree_threads[cursor].append(chat)
    configs=[p['core']['config'] for p in parts.values()]
    if not configs or any(c!=configs[0] for c in configs):fail('constants.config_changed')
    cfg=configs[0]
    if any(type(cfg.get(k)) is not int or cfg[k]<0 for k in ('max_depth','max_fanout','max_queue','max_retry','run_window')):
        fail('constants.config')
    facts=probe.facts(parts)
    catalogs=[f['data'] for d,i,f in facts if f['kind']=='capability_catalog']
    if not catalogs:fail('constants.capability_catalog')
    providers={};policy=None
    for catalog in catalogs:
        if catalog.get('scope')!='selected_harnesses' or catalog.get('complete') is not True:
            fail('constants.capability_catalog_complete')
        if catalog.get('validated') is not True:fail('constants.capability_catalog_validated')
        if catalog.get('core_config')!=cfg:fail('constants.capability_catalog_config')
        current=catalog.get('policy')
        if not isinstance(current,dict):fail('constants.policy')
        if policy is not None and current!=policy:fail('constants.policy_changed')
        policy=current
        for provider in catalog['providers']:
            name=provider.get('provider_id')
            if not isinstance(name,str) or not name:fail('constants.provider_id')
            if provider.get('delivery_path')!='portable_prompt':fail('constants.provider_delivery_path')
            for key in ('effective_native_fork','effective_native_injection'):
                if known(provider.get(key)) is not False:fail('constants.provider_'+key)
            for key in ('accepts_handoff','native_fork','inject_history','person_selectable'):
                if type(known(provider.get(key))) is not bool:fail('constants.provider_'+key)
            if known(provider['native_fork']) or known(provider['inject_history']):fail('constants.provider_effective_capabilities')
            if name in providers and providers[name]!=provider:fail('constants.provider_changed')
            providers[name]=provider
    if not providers:fail('constants.Providers')
    for target,key in POLICY.items():
        captured=known(policy.get(key))
        if captured is not None and captured is not SCOPE_FLAGS[target][0]:fail('constants.policy_conflict_'+key)
    inplace=known(policy.get('in_place_access'))
    if inplace is not None and (not isinstance(inplace,list) or not set(inplace)<=providers.keys()):
        fail('constants.policy_in_place_access')
                                                                          
    counts=Counter();accepted=[];all_threads={};devices=set(parts)
    event_counts=Counter();activation_order={};native_slots={};native_keys=set();used_providers=set()
    max_attempt=max_generation=max_run=0
    max_queue=cfg['max_queue'];message_ids=set();transfer_ids=set();inplace_chats={}
    source_map=[];seen_facts=set();device_rows=Counter()
    used_pairs=set();accepted_pairs=set();prior_threads={};runtime_ids=set();origin_ids=set();max_source_revision=0;max_delivery_generation=0
    def runtime_id(value,index):
        identity=known(value)
        if not isinstance(identity,str) or not identity or identity=='none':
            fail('reporting.runtime_identity',index)
        runtime_ids.add(identity)
    for index,row in enumerate(rows):
        source_map.append(dict(row=index,device=row['dev'],device_row=device_rows[row['dev']],action=row.get('action','Unknown')))
        device_rows[row['dev']]+=1
        before_threads=dict(prior_threads)
        for dev,part in entries(row['partitions']):
            if part['core']['config']!=cfg:fail('constants.config_changed',index)
            for chat,th in entries(part['core']['threads']):
                all_threads[chat]=th
                max_run=max(max_run,th['run_base']+len(th['runs']))
                max_attempt=max(max_attempt,th['attempt']['n'])
                flag=th['reconfigure']
                if chat in inplace_chats and inplace_chats[chat]!=flag:fail('constants.InPlace_changed',index)
                inplace_chats[chat]=flag
                for run in th['runs']:max_generation=max(max_generation,run['cohort']['generation'])
            for key,native in entries(part['core']['natives']):
                native_keys.add((key[0],key[1]))
                generation=native['generation']
                if type(generation) is not int or generation<0:fail('reporting.delivery_generation',index)
                max_delivery_generation=max(max_delivery_generation,generation)
            mechanism=part['mechanism'];source_facts=dict(entries(mechanism['facts']))
            for identity in mechanism['fact_order']:
                scoped=(dev,identity)
                if scoped in seen_facts:continue
                seen_facts.add(scoped);fact=source_facts[identity];data=fact['data'];kind=fact['kind']
                if kind in {'runtime_binding','agent_admission'}:
                    origin=known(data.get('origin_id'))
                    if origin is not None:
                        if not isinstance(origin,str) or not origin or origin=='none':fail('reporting.origin_identity',index)
                        origin_ids.add(origin)
                if kind=='runtime_binding':
                    if data.get('validated') is not True:fail('reporting.runtime_binding_validated',index)
                    runtime_id(data.get('runtime_id'),index)
                    revision=data.get('source_revision')
                    if type(revision) is not int or revision<0:fail('reporting.source_revision',index)
                    max_source_revision=max(max_source_revision,revision)
                elif kind=='provider_start' and data.get('outcome')=='engine_start_ok':
                    runtime_id(data.get('provider_return_id'),index)
                elif kind=='provider_stream_catalog':
                    for stream in data['streams']:runtime_id(stream.get('provider_run_id'),index)
                elif kind=='native_binding':
                    chat=data['chat'];path=data['native_path'];task=data['task_key']
                    if not isinstance(task,str) or not task.isdecimal():fail('native.task_key',index)
                    if not isinstance(path,list) or not path or any(not isinstance(x,str) or not x for x in path):fail('native.path',index)
                    path_key=json.dumps(path,separators=(',',':'))
                    previous=native_slots.setdefault(chat,{}).setdefault(path_key,task)
                    if previous!=task:fail('native.path_rebound',index)
                elif kind=='reconfigure_pair':
                    if data.get('accepted') is True and data.get('validated') is True:
                        if not data.get('before_digest') or not data.get('after_digest'):fail('constants.InPlace_pair_identity',index)
                        accepted_pairs.add((data['chat'],data['run'],data['attempt']))
                elif kind=='provider_event':
                    runtime_id(data.get('provider_run_id'),index)
                    event_counts[data['event_kind']]+=1
                    observer=data['observer'];natives=observer.get('natives',[])
                    if isinstance(natives,dict):natives=[v for k,v in entries(natives)]
                    for native in natives:
                        key=json.dumps(native['native_path'],separators=(',',':'))
                        activation=known(native.get('activation_id'))
                        if activation is not None:
                            if not isinstance(activation,str) or not activation or activation=='none':
                                fail('native.activation_identity',index)
                            order=activation_order.setdefault(data['chat'],{}).setdefault(key,[])
                            if activation not in order:order.append(activation)
                elif kind=='transfer_operation':
                    if data.get('succeeded'):counts['CT:'+data['operation']]+=1
                    args=data.get('args',{})
                    if 'harness' in args:used_providers.add(args['harness'])
                    for state in (data.get('before',{}),data.get('after',{})):
                        for entry in state.get('entries',[]):message_ids.add(entry['id'])
                        ledger=state.get('ledger',fn([]))
                        for key,value in entries(ledger):transfer_ids.add(key)
                elif kind in {'transfer_catalog','transfer_state'}:
                    states=data['chats'] if kind=='transfer_catalog' else [data]
                    for state in states:
                        for entry in state.get('entries',[]):message_ids.add(entry['id'])
                        for key,value in entries(state.get('ledger',fn([]))):transfer_ids.add(key)
        if row.get('action')=='Apply' and row['args']['accepted'] and not row['args']['duplicate']:
            event=row['event'];counts[event['act']]+=1;accepted.append(event)
            if event['act']=='NarrowAccess':
                affected=event['args']['t']
                for chat,th in before_threads.items():
                    cursor=chat;seen=set()
                    while cursor in before_threads and cursor not in seen:
                        if cursor==affected:
                            if th['reconfigure'] and any(run['status']=='Live' for run in th['runs']):
                                used_pairs.add((chat,th['run_base']+len(th['runs']),th['attempt']['n']))
                            break
                        seen.add(cursor);cursor=before_threads[cursor]['parent']
        prior_threads=dict(all_threads)
    if not used_pairs<=accepted_pairs:fail('constants.InPlace_pair_missing')
    if not used_providers<=providers.keys():fail('constants.provider_catalog_missing')
    bound_keys={(chat,key) for chat,paths in native_slots.items() for key in paths.values()}
    if len(bound_keys)!=sum(len(paths) for paths in native_slots.values()):fail('native.task_key_alias')
    if not native_keys<=bound_keys:fail('native.binding_missing')
                                                                           
                                                                               
                                                                             
    for root,tree in tree_threads.items():
        probe.constants={'Devices':aset(sorted(devices))}
        probe.observation_fields(rows[-1],parts,routes,{t:threads[t] for t in tree if t in threads},tree)
    def n(*names):return sum(counts[name] for name in names)
    def provider_set(flag):return aset(sorted(k for k,v in providers.items() if known(v[flag])))
    constants=dict(
        Roots=aset(roots),TreeThreads=fn((r,aset(ts)) for r,ts in tree_threads.items()),
        RootDevices=fn((r,routes[r]) for r in roots),Devices=aset(sorted(devices)),
        NoThread='none',NoSub='none',NoProv='none',Mut='none',
        MaxRuns=max_run,MaxQueue=max_queue,RLMaxRetry=cfg['max_retry'],RLMaxAtt=max_attempt,
        MaxDepth=cfg['max_depth'],MaxFanout=cfg['max_fanout'],MaxGen=max_generation,
        RLMaxPerson=n('PersonSend','Steer','RestartActive'),RLMaxServer=n('ServerEnqueue'),
        RLMaxStop=n('RequestInterrupt','Stop'),RLMaxUL=sum(e['act']=='ProviderTerminal' and e['args'].get('outcome')=='UsageLimit' for e in accepted),
        RLMaxCrash=sum(e['act']=='ProviderTerminal' and e['args'].get('outcome')=='Crash' for e in accepted),
        DMaxPerson=n('PersonSend'),MaxFail=0,DMaxCrash=n('CrashEngine'),MaxAbandon=n('WaitAbandon'),
        DMaxStop=n('Stop'),MaxNarrow=n('NarrowAccess'),MaxInterrupt=n('InterruptChat'),
        MaxFollowUp=max_run if n('Followup') else 0,UseNS=False,UseCT=True,UsePL=True,
        Subs=aset(sorted({key for chat,key in native_keys})),
        MaxEp=max((len(xs) for paths in activation_order.values() for xs in paths.values()),default=0),
        NSMaxCrash=0,MaxDup=0,NSMaxInterrupt=0,MaxSessEnd=0,
        MaxInbox=sum(event_counts.values()),Providers=aset(sorted(providers)),
        CanHandoff=provider_set('accepts_handoff'),CanInject=provider_set('inject_history'),
        NativeFork=provider_set('native_fork'),MainProviders=provider_set('person_selectable'),
        MaxMsg=len(message_ids),MaxX=len(transfer_ids),CTMaxCrash=n('CT:Crash'),MaxStale=n('CT:Stale'),
        MaxCutK=len(message_ids),InPlace=aset(sorted({chat for chat,run,attempt in used_pairs})),
        MaxOffline=sum(f['kind']=='engine_lifecycle' and f['data']['state']=='stopped' for d,i,f in facts),

    )
    constants.update((target,value) for target,(value,reason) in SCOPE_FLAGS.items())
    expected=set(json.loads((HERE/'full-spec-interface.json').read_text())['constants'])
    expected-= {'Threads','Root','NsT','RootDev'}
    if set(constants)!=expected|{'Roots','TreeThreads','RootDevices'}:fail('constants.interface')
    reporting_constants=dict(UseReporting=True,
        Parents=aset(sorted(parents)),
        NativeIds=aset([list(pair) for pair in sorted(bound_keys)]),
        RuntimeIds=aset(sorted(runtime_ids)),OriginIds=aset(sorted(origin_ids)),
        ActivationIds=aset(sorted({identity for paths in activation_order.values()
                                  for order in paths.values() for identity in order})),
        NoNative=[],NoRuntime='none',NoActivation='none',NoOrigin='none',
        MaxReportRuns=max_run,MaxReportAttempts=max_attempt,MaxSourceRevision=max_source_revision,MaxDeliveryGeneration=max_delivery_generation)
    context=dict(constants=constants,reporting_constants=reporting_constants,
                tree_threads=tree_threads,native_slots=native_slots,
                activation_order=activation_order,provenance=provenance,
                source_map=source_map,pending_catalog=pending,scope_contract=scope_contract(),
                in_place_scope='accepted actual request pairs' if used_pairs else 'empty set: no accepted live in-place narrowing pair used',
                scope='finite captured input catalog; no universal refinement claim')
    return bind_transfer_reporting(rows,context)


def bind_transfer_reporting(rows,context):
    """Require a complete source request map before disabling the old CT layer."""
    from runtime_transfer_reporting_projection import project_transfer_reporting
    transfer=project_transfer_reporting(rows,context)
    constants=transfer.get('transfer_constants',{})
    images=transfer.get('transfer_images')
    actions=transfer.get('transfer_actions')
    source_map=transfer.get('transfer_source_map')
    if constants.get('UseTransferReporting') is not True:
        fail('transfer_reporting.required')
    if (not isinstance(images,list) or not images or not isinstance(actions,list)
            or len(actions)!=len(images)-1 or not isinstance(source_map,list)
            or len(source_map)!=len(images)):
        fail('transfer_reporting.source_path')
    seen_rows=[]
    for source in source_map:
        if not isinstance(source,dict):fail('transfer_reporting.source_path')
        row=source.get('raw_row');substep=source.get('macro_substep')
        if (type(row) is not int or not 0<=row<len(rows)
                or type(substep) is not int or substep<0):
            fail('transfer_reporting.source_path')
        if seen_rows and row<seen_rows[-1]:fail('transfer_reporting.source_path')
        seen_rows.append(row)
    if set(seen_rows)!=set(range(len(rows))):fail('transfer_reporting.source_path')
    context['transfer_constants']=constants
    context['constants']['UseCT']=False
    context['transfer_projection_scope']=transfer['transfer_scope']
    context['transfer_projection_evidence']=dict(
        images=len(images),actions=len(actions),
        source_path_sha256=hashlib.sha256(json.dumps(
            dict(images=images,actions=actions,source_map=source_map),
            sort_keys=True,separators=(',',':')).encode()).hexdigest(),
        boundary='source-derived path only; independent target checks remain required')
    return context


def build(paths):
    completed(paths)
    hashes=sorted(sha(path) for path in paths)
    if len(set(hashes))!=len(hashes):fail('captures.duplicate_content')
                                                                              
    raw_rows=merge_raw(paths)['rows']
    try:issuance=question_issuance(raw_rows)
    except QuestionIssuanceError as error:
        raise FullProjectionError(error.row,error.action,error.field) from None
    steps,_=project_global(paths)
    rows=[dict(step['row'],partitions=step['parts']) for step in steps]
    provenance=dict(kind='capture',capture_sha256=hashes,generator=Path(__file__).name,
                    generator_sha256=sha(Path(__file__)))
    result=derive(rows,provenance)
    result['question_issuance_contract']=dict(
        module='runtime_question_issuance.py',module_sha256=sha(HERE/'runtime_question_issuance.py'),
        basis='accepted nonduplicate Raise receipt; strictly earlier issued then route; exact live raw pre-attempt',
        order='validated raw causal DAG before generic ID normalization',
        boundary='source issuance evidence; no hidden provider turn identity or target successor claim')
    result['question_issuance_evidence']=issuance
    source_rows={}
    for path in paths:
        digest=sha(path)
        for line,text in enumerate(path.read_text().splitlines(),1):
            if not text.strip():continue
            raw=json.loads(text);key=(raw['dev'],raw['seq'])
            if key in source_rows:fail('captures.source_row_duplicate')
            source_rows[key]=dict(capture_sha256=digest,raw_seq=raw['seq'],line=line)
    by_device={}
    for (device,seq),source in sorted(source_rows.items()):by_device.setdefault(device,[]).append(source)
    for source in result['source_map']:
        source.update(by_device[source['device']][source['device_row']])
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('captures',nargs='+',type=Path);parser.add_argument('--out',required=True,type=Path)
    args=parser.parse_args()
    try:
        context=build(args.captures)
    except FullProjectionError as error:
        result=dict(check='runtime-context',status='blocked',row=error.row,action=error.action,field=error.field)
        args.out.parent.mkdir(parents=True,exist_ok=True)
        args.out.with_suffix('.blocked.json').write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps(result));raise SystemExit(2)
    except (ValueError,KeyError,TypeError,OSError):
        print(json.dumps(dict(check='runtime-context',status='blocked',field='source.inputs')));raise SystemExit(2)
    args.out.parent.mkdir(parents=True,exist_ok=True)
    args.out.write_text(json.dumps(context,indent=2,sort_keys=True)+'\n')
    print(json.dumps(dict(check='runtime-context',status='ready',captures=len(args.captures),rows=len(context['source_map']))))


if __name__=='__main__':main()
