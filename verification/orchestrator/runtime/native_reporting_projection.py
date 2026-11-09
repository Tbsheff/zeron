"""Project native reporting identity and engine handles from observed prefixes.

This map never calls a reducer or chooses a target successor. Runtime UUIDs
own result sources. Conversation session IDs do not own native task rows.
"""
from copy import deepcopy
import json

ENGINE_FIELDS = ('nRst','nRord','nEndBy','nRec','nEst','nEep','nErun','nEbg',
                 'nHow','nClosedEp','nTask','nInbox','nReplay')

def fn(pairs):
    return {'$fn': list(pairs)}

def aset(items):
    return {'$set': list(items)}

def entries(value):
    if not isinstance(value,dict) or set(value)!={'$fn'}:
        raise ValueError('expected finite function')
    return value['$fn']

def known(value):
    if isinstance(value,dict) and set(value)=={'present'} and value['present'] is False:
        return None
    if isinstance(value,dict) and set(value)=={'present','value'}:
        return known(value['value']) if value['present'] else None
    return value

def decoded(value):
    value=known(value)
    if isinstance(value,list):return [decoded(x) for x in value]
    if isinstance(value,dict):
        if set(value)=={'$fn'}:return {'$fn':[(decoded(k),decoded(v)) for k,v in entries(value)]}
        if set(value)=={'$set'}:return {'$set':[decoded(x) for x in value['$set']]}
        return {k:decoded(v) for k,v in value.items()}
    return value

def path_id(path):
    return json.dumps(path,separators=(',',':'))

def native_status(status):
    return {'Running':'running','Completed':'completed','Failed':'failed',
            'Orphaned':'interrupted','Cancelled':'cancelled'}.get(status)

class NativeReportingProjection:
    def __init__(self,context,focus,owner,fail):
        self.context,self.focus,self.owner,self.fail=context,focus,owner,fail
        self.sources={};self.bindings={};self.commits={};self.source_events=[]
        self.pending=[];self.handled=[];self.replays=[];self.seen_facts={}
        self.by_runtime={};self.native_sources={};self.native_activations={}
        self.binding_revisions={}
        self.binding_revision_rows={}
        self.handle_revision={};self.handles={focus:'none'};self.admitted={focus:dict(present=False)}
        self.binding_data={}
        self.catalog_seen=False;self.catalog_streams=[]
        self.closure={};self.engine_epoch={};self.closed_epoch={};self.end_by='none'
        self.runs={};self.tasks={};self.physical={};self.last_cause=dict(kind='none',source='none',changed=aset([]))
        self.provenance=[];self.unsupported={};self.row_index=0

    def stop(self,field):
        self.fail(field)
        raise ValueError(field)                                            

    def missing(self,field,reason):
        self.unsupported.setdefault(field,reason)

    def slot(self,path):
        slot=self.context.get('native_slots',{}).get(self.focus,{}).get(path_id(path))
        if slot is None:self.stop('nEst.native_path_binding')
        return str(slot)

    def epoch(self,path,activation,field):
        order=self.context.get('activation_order',{}).get(self.focus,{}).get(path_id(path),[])
        if activation is None:
            self.missing(field,'activation identity is not observed')
            return None
        if len(order)!=len(set(order)) or activation not in order:self.stop(field+'.activation_order')
        return order.index(activation)+1

    def source(self,sid):
        if sid not in self.sources:
            if sid not in self.by_runtime:self.stop('nErun.provider_binding')
            parent,run,attempt=self.by_runtime[sid]
            self.sources[sid]=dict(parent=parent,run=run,attempt=attempt,
                reporting='unknown',rootActive='unknown',process='unknown',
                issuedOrigin='none',issuedRun=0,issuedAttempt=0,issuedRevision=0,issuedRequestAccess=0)
            binding=self.binding_data.get(sid)
            if binding is None:self.missing('reporting.sources.runtime_binding','actual source handle binding is absent')
            else:self.sources[sid].update(issuedAccess=binding['effective_access'],revision=binding['source_revision'],
                                         origin=binding['origin_id'],retired=binding['operation'] in {'retire','remove'},requestIssued=binding['operation']=='issue')
        return self.sources[sid]

    def fact(self,identity,kind,d):
        if d.get('chat') not in {self.focus,None} and kind not in {'provider_stream_catalog','engine_lifecycle'}:return
        if kind=='runtime_binding':
            if d.get('chat')!=self.focus:return
            if d.get('validated') is not True:self.stop('nErun.runtime_binding_validated')
            if d.get('run') is None or d.get('attempt') is None:self.stop('nErun.runtime_binding_attempt')
            sid=d['runtime_id'];stamp=(d['chat'],d['run'],d['attempt'])
            source_id,revision=d.get('source_id'),d.get('source_revision')
            if not isinstance(source_id,str) or type(revision) is not int or revision<1:self.stop('nErun.runtime_binding_revision')
            prior=self.binding_revision_rows.setdefault((source_id,revision),deepcopy(d))
            if prior!=d:self.stop('nErun.runtime_binding_revision_conflict')
            previous=self.binding_revisions.get((source_id,sid),0)
            if revision<=previous:return
            self.binding_revisions[(source_id,sid)]=revision
            op=d['operation']
            if op not in {'install','issue','rebind','retire','remove','config'}:self.stop('nErun.runtime_binding_operation')
            self.by_runtime.setdefault(sid,stamp)
            if op in {'install','rebind'}:
                if op=='install' and sid in self.by_runtime and self.by_runtime[sid]!=stamp:self.stop('nErun.runtime_reinstalled')
                self.by_runtime[sid]=stamp
                if sid in self.sources:
                    self.sources[sid].update(run=stamp[1],attempt=stamp[2])
            self.binding_data[sid]=deepcopy(d)
            source=self.source(sid)
            source.update(issuedAccess=d['effective_access'],revision=revision,origin=d['origin_id'],
                          retired=op in {'retire','remove'} or (source.get('retired',False) and op not in {'install','rebind'}),
                          requestIssued=op=='issue' or source.get('requestIssued',False))
                                                                        
                                                                          
            if op=='issue':
                source.update(issuedOrigin=d['origin_id'],issuedRun=d['run'],issuedAttempt=d['attempt'],
                              issuedRevision=revision,issuedRequestAccess=d['effective_access'])
            handle_key=(source_id,self.focus)
            if revision>self.handle_revision.get(handle_key,0):
                self.handle_revision[handle_key]=revision
                if op!='remove':self.handles[self.focus]=sid
                elif op=='remove' and self.handles[self.focus]==sid:self.handles[self.focus]='none'
        elif kind=='agent_admission' and d.get('chat')==self.focus:
            if d.get('validated') is not True:self.stop('reporting.admitted.validated')
            if d.get('disposition') not in {'accepted','refused'}:self.stop('reporting.admitted.disposition')
            if d['disposition']=='accepted':
                sid=d.get('runtime_id')
                if not sid or sid!=d.get('provider_run_id'):self.stop('reporting.admitted.runtime_identity')
                if (d.get('handle_run'),d.get('handle_attempt'))!=(d.get('current_run'),d.get('current_attempt')):self.stop('reporting.admitted.current_attempt')
                admitted=dict(present=True,source=sid,run=d['handle_run'],attempt=d['handle_attempt'],issuedAccess=d['effective_access'])
                revision=d.get('handle_source_revision')
                if revision is None:self.missing('reporting.admitted.revision','actual admission has no causal source revision')
                else:
                    if type(revision) is not int or revision<1:self.stop('reporting.admitted.revision_type')
                    candidates=[b for (source,rev),b in self.binding_revision_rows.items() if rev==revision
                        and b['runtime_id']==sid and (d.get('handle_source_id') is None or source==d['handle_source_id'])]
                    if len(candidates)!=1:self.missing('reporting.admitted.revision_binding','causal handle snapshot is not observed')
                    else:
                        b=candidates[0]
                        if (b['run'],b['attempt'],b['effective_access'],b['origin_id'])!=(d['handle_run'],d['handle_attempt'],d['effective_access'],d.get('origin_id')):self.stop('reporting.admitted.binding_conflict')
                        admitted['revision']=revision
                if self.handles[self.focus]!=sid:self.missing('reporting.admitted.current_handle','actual admission handle installation is not observed')
                self.admitted[self.focus]=admitted
            else:self.admitted[self.focus]=dict(present=False)
        elif kind=='provider_start' and d.get('chat')==self.focus and d.get('outcome')=='engine_start_ok':
            sid=d.get('provider_return_id')
            if not sid:self.stop('nErun.provider_binding')
            stamp=(self.focus,d['run'],d['attempt'])
                                                                              
                                                                   
            self.by_runtime.setdefault(sid,stamp)
        elif kind=='native_binding' and d.get('chat')==self.focus:
            key=str(d['task_key']);old=self.bindings.setdefault(key,deepcopy(d))
            if old!=d:self.stop('nEst.native_binding_changed')
            if self.slot(d['native_path'])!=key:self.stop('nEst.native_slot_conflict')
        elif kind=='native_doc_commit' and d.get('chat')==self.focus:
            key=str(d['task_key']);binding=self.bindings.get(key)
            if binding is None:self.stop('nTask.native_binding')
            for field in ('native_path','tool_id','doc_id','chip_doc_id'):
                if d.get(field)!=binding[field]:self.stop('nTask.'+field)
            if d.get('chip_ref')!=binding['doc_id']:self.stop('nTask.chip_ref')
            if d.get('chip_status') not in {'running','done','completed','failed','interrupted','cancelled'}:self.stop('nTask.chip_status')
            for field in ('doc_hash','chip_doc_hash'):
                value=d.get(field)
                if not isinstance(value,str) or len(value)!=64 or any(c not in '0123456789abcdef' for c in value):self.stop('nTask.'+field)
            self.commits[key]=deepcopy(d)
        elif kind=='provider_event' and d.get('chat')==self.focus:
            self.provider_event(identity,d)
        elif kind=='provider_stream_catalog':
            if self.catalog_seen and d.get('complete') is False:
                self.stop('reporting.catalog.completeness_regressed')
            if d.get('complete') is True:
                self.catalog_seen=True
                self.catalog_streams=[s for s in d.get('streams',[]) if s.get('chat')==self.focus]

    def provider_event(self,identity,d):
        sid=d['provider_run_id'];source=self.source(sid)
        seq=d['stream_seq'];previous=[x['stream_seq'] for x in self.source_events if x['source']==sid]
        if type(seq) is not int or seq!=(previous[-1]+1 if previous else 1):self.stop('nInbox.source_sequence')
        event_id=d['event_id']
        if any(x['event_id']==event_id for x in self.source_events):self.stop('nInbox.source_event_id')
        replay=d.get('replay_of')
        if 'replay_of' not in d:self.missing('nReplay','source replay provenance is absent')
        if replay is not None and replay not in {x['event_id'] for x in self.source_events}:self.stop('nReplay.source_identity')
        event=dict(source=sid,event_id=event_id,stream_seq=seq,kind=d['event_kind'],
                   path=d.get('native_path'),activation=d.get('activation_id'),status=d.get('status'),replay_of=replay)
        self.source_events.append(event)
        if replay is not None:self.replays.append(event)
        kind=d['event_kind'];observer=d['observer']
        if kind=='stream_opened':source['reporting']='connected'
        elif kind in {'stream_closed','stream_crashed','session_closed','session_crashed'}:
            source.update(reporting='disconnected',rootActive='unknown')
            for native in self.physical.values():
                if native['source']==sid and native['state']=='running':native['state']='unknown'
            self.pending.append(dict(kind='sourceLoss',source=sid,key='none',run=source['run'],attempt=source['attempt'],out='failed',event_id=event_id))
        else:
            if source['reporting']=='disconnected':self.stop('nInbox.closed_source_reported')
            source['reporting']='connected'
            root=observer.get('root_turn_active')
            if root is not None and type(root) is not bool:self.stop('nProot.boolean')
            if root is not None:source['rootActive']='active' if root else 'inactive'
        natives=observer.get('natives',[])
        if isinstance(natives,dict):natives=[v for k,v in entries(natives)]
        for native in natives:
            path=native['native_path'];key=self.slot(path)
            activation=native.get('activation_id')
            if activation is not None:self.native_activations[(sid,key)]=(path,activation)
            if (self.focus,key) in self.physical and activation is not None:
                self.physical[(self.focus,key)]['activation']=activation
        if kind in {'native_spawn','native_started','native_terminal','native_progress'}:
            path=d.get('native_path')
            if not path:self.stop('nInbox.native_path')
            key=self.slot(path);nid=(self.focus,key)
            if key in self.native_sources and self.native_sources[key]!=sid:self.stop('nEst.native_source_reused')
            self.native_sources[key]=sid
            own=d.get('background')
            if own is None:
                current=next((n for n in natives if n['native_path']==path),None)
                own=current.get('background') if current else None
            binding=self.bindings.get(key)
            if own is None and binding is not None:own=binding['own_background']
            if type(own) is not bool:self.missing('nEbg','source background is not observed')
            ancestor=self.slot(path[:-1]) if len(path)>1 else 'none'
            effective=own
            if ancestor!='none':
                owner=self.physical.get((self.focus,ancestor))
                if owner is None:self.stop('nEbg.ancestor_source')
                if owner['source']!=sid:self.stop('nEbg.ancestor_runtime')
                effective=bool(own) or owner['background']
            if kind=='native_spawn':
                if nid in self.physical:self.stop('nEst.native_spawn_reused')
                lifetime=self.physical[(self.focus,ancestor)] if ancestor!='none' else source
                row=dict(source=sid,parent=self.focus,run=lifetime['run'],attempt=lifetime['attempt'],
                    ancestor=[self.focus,ancestor] if ancestor!='none' else 'none',ownBackground=own,
                    background=effective,state='running',activation=d.get('activation_id') or 'none')
                self.physical[nid]=row
                self.pending.append(dict(kind='spawn',source=sid,key=list(nid),run=row['run'],attempt=row['attempt'],out='none',event_id=event_id))
            elif kind=='native_terminal':
                if nid not in self.physical:self.stop('nInbox.native_spawn_missing')
                self.physical[nid]['state']='reported'
                self.pending.append(dict(kind='nativeEnd',source=sid,key=list(nid),run=self.physical[nid]['run'],attempt=self.physical[nid]['attempt'],out=d['status'],event_id=event_id))
        elif kind=='root_terminal':
            self.pending.append(dict(kind='rootEnd',source=sid,key='none',run=source['run'],attempt=source['attempt'],out=d['status'],event_id=event_id))
        self.provenance.append(dict(row=self.row_index,fact=identity,event_id=event_id,source=sid))

    def consume(self,kind,key=None,run=None,attempt=None):
        matches=[(i,e) for i,e in enumerate(self.pending) if e['kind']==kind
                 and (key is None or e['key']==[self.focus,str(key)])
                 and (run is None or e['run']==run) and (attempt is None or e['attempt']==attempt)]
        if len(matches)!=1:self.stop('nInbox.accepted_'+kind+'_identity')
        i,e=matches[0]
        prior=[old for old in self.pending[:i] if old['source']==e['source']]
        if any(old['key']==e['key'] or old['kind']=='sourceLoss' for old in prior):self.stop('nInbox.source_lane_order')
        if prior:self.missing('nInbox','actual cross-path completion overtakes the original NS source FIFO')
        self.pending.pop(i);self.handled.append(deepcopy(e))
        self.last_cause=dict(kind=e['kind'],source=e['source'],changed=aset([]))
        self.provenance.append(dict(row=self.row_index,accepted_event=e['event_id'],source=e['source'],kind=kind))
        return e

    def observe_core(self,row,before,core):
        threads=dict(entries(core['threads']));th=threads.get(self.focus)
        current=dict(run=th['run_base']+len(th['runs']) if th else 0,
                     attempt=th['attempt']['n'] if th else 0,
                     active=bool(th and th['runs'] and th['runs'][-1]['status']=='Live'),endBy=self.end_by)
        previous=dict(entries(before['threads'])).get(self.focus) if before else None
        before_natives={str(k[1]):v for k,v in entries(before['natives']) if k[0]==self.focus} if before else {}
        natives={str(k[1]):v for k,v in entries(core['natives']) if k[0]==self.focus}
        accepted=row.get('action')=='Apply' and row['args']['accepted'] and not row['args']['duplicate']
        ev=row.get('event',{});act=ev.get('act');args=ev.get('args',{})
                                                                           
                                                                                
                                                                             
        if accepted and act=='ProviderTerminal' and args.get('t')==self.focus and args.get('outcome') in {'Failed','Interrupted'}:
            stamp=(args['run'],args['attempt'])
            pending_root=[e for e in self.pending if e['kind']=='rootEnd' and (e['run'],e['attempt'])==stamp]
            if not pending_root:
                closed=[sid for sid,h in self.sources.items() if h['reporting']=='disconnected' and (h['run'],h['attempt'])==stamp]
                if len(closed)==1:
                    sid=closed[0];identity='accepted-eof-root:'+str(self.row_index)
                    self.pending.append(dict(kind='rootEnd',source=sid,key='none',run=stamp[0],attempt=stamp[1],
                        out='failed' if args['outcome']=='Failed' else 'interrupted',event_id=identity))
                    self.last_cause=dict(kind='none',source='none',changed=aset([]))
                    self.provenance.append(dict(row=self.row_index,accepted_event=identity,source=sid,
                        kind='engineEOFRootTerminal',basis='accepted ProviderTerminal plus closed source and exact bound attempt'))
                    self.missing('nInbox','engine EOF terminal does not prove the old NS process event')
                elif len(closed)>1:self.stop('reporting.root_callback.ambiguous_source')
        if previous is not None and th is not None and (previous['attempt']['n']!=th['attempt']['n'] or
                previous['run_base']+len(previous['runs'])!=current['run'] or
                bool(previous['runs'] and previous['runs'][-1]['status']=='Live')!=current['active']):
            self.last_cause=dict(kind='none',source='none',changed=aset([]))
        for key,n in natives.items():
            old=before_natives.get(key);sid=self.native_sources.get(key)
            if sid is None:
                self.missing('nEst','native task lacks observed source identity')
                continue
            binding=self.bindings.get(key)
            if binding is None:self.stop('nEst.native_binding')
            activation=self.native_activations.get((sid,key))
            ep=self.epoch(binding['native_path'],activation[1] if activation else None,'nEep')
            if old is None:
                if not accepted or act not in {'NativeStart','NativeStartNested'} or str(args.get('key'))!=key:self.stop('nEst.unobserved_insert')
                spawned=self.consume('spawn',key)
                if (n['attempt']['run']['ord'],n['attempt']['n'])!=(spawned['run'],spawned['attempt']):self.stop('nErun.source_attempt')
                self.engine_epoch[key]=ep
            status=native_status(n['status'])
            if status is None:self.stop('nEst.native_status')
            if old is None or old['status']!=n['status']:
                cause='none';r44cause='none';changed=[]
                if n['status']=='Orphaned':cause='orphan';r44cause='orphan'
                elif old is not None and old['status']=='Running':
                    if not accepted or act!='NativeEnd' or str(args.get('key'))!=key:self.stop('nHow.accepted_terminal')
                    matches=[e for e in self.pending if e['kind']=='nativeEnd' and e['key']==[self.focus,key]]
                    if matches:
                        event=self.consume('nativeEnd',key)
                        expected='completed' if not args['failed'] else {'failed','interrupted','cancelled'}
                        if (event['out']!=expected if isinstance(expected,str) else event['out'] not in expected):self.stop('nHow.terminal_outcome')
                        cause='late' if n['delivery']=='Claimed' else 'event';r44cause='reported'
                    else:
                        loss=[e for e in self.pending if e['kind']=='sourceLoss' and e['source']==sid]
                        if len(loss)!=1 or not args['failed']:self.stop('nHow.terminal_source')
                        if any(e['source']==sid for e in self.pending[:self.pending.index(loss[0])]):self.stop('nInbox.source_loss_order')
                        cause='session';r44cause='reporting_loss';changed=[[self.focus,key]]
                        self.last_cause=dict(kind='sourceLoss',source=sid,changed=aset(changed))
                    self.closed_epoch[key]=self.engine_epoch.get(key)
                self.closure[key]=cause
                if n['status']=='Orphaned':self.closed_epoch[key]=self.engine_epoch.get(key)
                task=dict(source=sid,parent=self.focus,run=n['attempt']['run']['ord'],attempt=n['attempt']['n'],
                          status=status,background=n['background'],cause=r44cause,
                          activation=activation[1] if activation else 'none',generation=n['generation'])
                self.tasks[(self.focus,key)]=task
            elif (self.focus,key) in self.tasks:
                if old['generation']!=n['generation']:self.last_cause=dict(kind='none',source='none',changed=aset([]))
                self.tasks[(self.focus,key)].update(status=status,background=n['background'],generation=n['generation'])
        if previous and previous['runs'] and previous['runs'][-1]['status']=='Live' and not current['active']:
            if not accepted or act!='IngestEnd' or args.get('t')!=self.focus:self.stop('nEndBy.accepted_root_end')
            terminal=args['ev']
            event=self.consume('rootEnd',run=terminal['run'],attempt=terminal['attempt'])
            self.end_by='root';current['endBy']='root'
            changed=[list(k) for k,t in self.tasks.items() if t['source']==event['source'] and t['cause']=='orphan'
                     and k[1] in before_natives and before_natives[k[1]]['status']=='Running']
            self.last_cause=dict(kind='rootEnd',source=event['source'],changed=aset(changed))
        elif current['active'] and (not previous or not previous['runs'] or previous['runs'][-1]['status']!='Live'):
            self.end_by='none';current['endBy']='none'
                                                                           
                                                                             
                                                                              
                                                          
        for loss in list(self.pending):
            if loss['kind']=='sourceLoss' and not any(t['source']==loss['source'] and t['status']=='running' for t in self.tasks.values()):
                if not any(e['source']==loss['source'] for e in self.pending[:self.pending.index(loss)]):
                    self.pending.remove(loss);self.handled.append(loss)
                    self.last_cause=dict(kind='sourceLoss',source=loss['source'],changed=aset([]))
        self.runs[self.focus]=current
        return th,natives

    def run(self,prefix):
        if not prefix:self.stop('nEst.empty_prefix')
        last_core=None;th=None;natives={}
        for index,row in enumerate(prefix):
            self.row_index=index
            parts=dict(entries(row['partitions']))
            if self.owner not in parts:self.stop('nEst.owner_partition')
            part=parts[self.owner];mechanism=part['mechanism'];facts=dict(entries(mechanism['facts']))
            observed_before=deepcopy((self.sources,self.physical,self.pending,self.handles,self.admitted,self.commits,self.catalog_seen))
            for identity in mechanism['fact_order']:
                fact=decoded(facts[identity]);old=self.seen_facts.get(identity)
                if old is not None:
                    if old!=fact:self.stop('nInbox.immutable_fact')
                    continue
                self.seen_facts[identity]=deepcopy(fact)
                self.fact(identity,fact['kind'],fact['data'])
            if observed_before!=(self.sources,self.physical,self.pending,self.handles,self.admitted,self.commits,self.catalog_seen):
                self.last_cause=dict(kind='none',source='none',changed=aset([]))
            th,natives=self.observe_core(row,last_core,part['core'])
            last_core=part['core']
        subs=self.context['constants']['Subs']['$set']
        if not self.catalog_seen:self.missing('reporting.catalog','no complete source catalog is observed')
        if any(s['provider_run_id'] not in self.sources for s in self.catalog_streams):
            self.missing('reporting.sources.history','catalog includes a source without ordered observed history')
        def mapping(name,get,initial):
            result=[]
            for sub in subs:
                key=str(sub)
                if key not in natives:result.append((sub,deepcopy(initial)))
                else:
                    value=get(key,natives[key])
                    if value is None:
                        self.missing(name,'required native source evidence is absent')
                        return None
                    result.append((sub,value))
            return fn(result)
        fields=dict(nRst='live' if th and th['runs'] and th['runs'][-1]['status']=='Live' else 'idle',
                    nRord=th['run_base']+len(th['runs']) if th else 0,nEndBy=self.end_by)
        if self.context['constants'].get('NSMaxCrash')==0:
            fields['nRec']=False
            self.provenance.append(dict(scope='NSMaxCrash=0',field='nRec',meaning='no original NS process crash in this finite scope'))
        else:self.missing('nRec','no original NS process recovery boundary fact')
        for name,get,initial in (
            ('nEst',lambda k,n:native_status(n['status']),'none'),
            ('nErun',lambda k,n:n['attempt']['run']['ord'],0),
            ('nEbg',lambda k,n:n['background'],False),
            ('nEep',lambda k,n:self.engine_epoch.get(k),0),
            ('nHow',lambda k,n:self.closure.get(k),'none'),
            ('nClosedEp',lambda k,n:self.closed_epoch.get(k,0),0),
            ('nTask',lambda k,n:{'done':'completed'}.get(self.commits[k]['chip_status'],self.commits[k]['chip_status']) if k in self.commits else None,'none')):
            value=mapping(name,get,initial)
            if value is not None:fields[name]=value
        if any(e['kind']=='native_progress' for e in self.source_events):
            self.missing('nInbox','native progress has no independent handle/consumption receipt')
        else:
            pending=[]
            for e in self.pending:
                if e['kind']=='sourceLoss':
                    self.missing('nInbox','reporting loss is not original NS process death');break
                key=e['key'][1] if isinstance(e['key'],list) else 'none'
                epoch=0
                if key!='none':
                    pair=self.native_activations.get((e['source'],key))
                    epoch=self.epoch(pair[0] if pair else [],pair[1] if pair else None,'nInbox')
                    if epoch is None:break
                pending.append(dict(k={'spawn':'spawn','nativeEnd':'end','rootEnd':'rootEnd'}[e['kind']],s=key,ep=epoch,r=e['run'],bg=self.physical[(self.focus,key)]['background'] if key!='none' else False,out=e['out']))
            else:fields['nInbox']=pending
        if not self.replays and 'nReplay' not in self.unsupported:fields['nReplay']=aset([])
        else:self.missing('nReplay','replay needs original event activation and payload join')
        docs=[]
        for key,commit in self.commits.items():
            if (self.focus,key) not in self.tasks:self.stop('nTask.commit_without_task')
            docs.append(([self.focus,key],dict(source=self.tasks[(self.focus,key)]['source'],status=commit['chip_status'])))
        reporting=dict(catalogComplete=fn([(self.focus,self.catalog_seen)]),runs=fn(self.runs.items()),sources=fn(self.sources.items()),handles=fn(self.handles.items()),admitted=fn(self.admitted.items()),
            natives=fn((list(k),dict(v,ancestor=[] if v['ancestor']=='none' else v['ancestor'])) for k,v in self.physical.items()),tasks=fn((list(k),v) for k,v in self.tasks.items()),
            docTasks=fn(docs),pending=[{k:([] if k=='key' and v=='none' else v) for k,v in e.items() if k!='event_id'} for e in self.pending],lastCause=self.last_cause)
        return dict(engine_fields=fields,reporting_state=reporting,unsupported_fields=deepcopy(self.unsupported),
                    reporting_complete=not any(k.startswith('reporting.') for k in self.unsupported),provenance=self.provenance)


def project_native_reporting(prefix,context,focus,owner,fail):
    """Read observed global prefixes; return old fields plus new R44 state.

    Unsupported old process/activation fields remain named in unsupported_fields.
    Malformed identity, order, ownership, or accepted-action joins call fail.
    """
    return NativeReportingProjection(context,focus,owner,fail).run(prefix)
