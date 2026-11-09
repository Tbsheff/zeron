"""Strict raw capture map for the versioned Composition forest.

This module reads complete core rows and immutable I/O facts. It does not call
an implementation reducer, select a Next successor, or accept a supplied model
image. Missing physical observations stop the map at a named semantic field.
The finite configuration must carry its captured or verified-test provenance.
"""
from copy import deepcopy
import json
from pathlib import Path

INTERFACE = json.loads(Path(__file__).with_name('full-spec-interface.json').read_text())
VARIABLES = INTERFACE['variables']

class FullProjectionError(ValueError):
    def __init__(self, row, action, field):
        self.row, self.action, self.field = row, action, field
        super().__init__(f'row={row} action={action} field={field}')


def fn(pairs):
    return {'$fn': list(pairs)}


def aset(values):
    return {'$set': list(values)}


def entries(value):
    if not isinstance(value, dict) or set(value) != {'$fn'}:
        raise ValueError('expected finite function')
    return value['$fn']


def values(value):
    if not isinstance(value, dict) or set(value) != {'$set'}:
        raise ValueError('expected finite set')
    return value['$set']


def enum(value):
    return {'UsageLimit':'usage_limit','SteeringRestart':'steering_restart',
            'ProviderRecovery':'provider_recovery','SettledOnly':'settled_only',
            'Cancelled':'cancelled'}.get(value, value.lower())


def message(value):
    kind = value['kind']
    if kind == 'Queued':
        return message(value['message'])
    if kind=='Questions':
        return dict(k='wake',r=0,g=0)
    if kind in {'Person','Server','Task','Resume'}:
        return dict(k=kind.lower(),r=0,g=0)
    if kind == 'Wake':
        return dict(k='wake',r=value['run'],g=value['generation'])
    raise ValueError('message kind has no full-source image')


def event(value):
    return dict(n=enum(value['node']),r=value['run'],a=value['attempt'],o=enum(value['outcome']))


class Projection:
    def __init__(self, context):
        self.context = deepcopy(context)
        self.constants = context['constants']
        if context.get('provenance', {}).get('kind') not in {'capture','verified_test'}:
            raise FullProjectionError(0,'Context','constants.provenance')
        self.row = 0
        self.action = 'Init'
        self.prefix = []

    def fail(self, field):
        raise FullProjectionError(self.row,self.action,field)

    def facts(self, parts):
        result=[]
        seen={}
        for dev,part in parts.items():
            facts=dict(entries(part['mechanism']['facts']))
            for identity in part['mechanism']['fact_order']:
                fact=facts[identity]
                scoped=(dev,identity)
                if scoped in seen:
                    if seen[scoped] != fact:
                        self.fail('immutable_fact')
                    continue
                seen[scoped]=fact
                result.append((dev,scoped,fact))
        return result

    def authority(self, row):
        parts=dict(entries(row['partitions']))
        routes={}
        for dev,part in parts.items():
            for chat,binding in entries(part['mechanism']['bindings']):
                if binding['host_known']:
                    host=binding['host']
                    if chat in routes and routes[chat] != host:
                        self.fail('host.conflict')
                    routes[chat]=host
        for dev,identity,fact in self.facts(parts):
            if fact['kind'] != 'registry_host':
                continue
            data=fact['data']
            if data.get('validated') is not True:
                self.fail('host.validated')
            if data['chat'] in routes and routes[data['chat']] != data['host']:
                self.fail('host.conflict')
            routes[data['chat']]=data['host']
        threads={}
        observed={chat for p in parts.values() for chat,_ in entries(p['core']['threads'])}
        for chat,host in routes.items():
            if host not in parts:
                self.fail('host.partition')
            core=dict(entries(parts[host]['core']['threads']))
            if chat not in core:
                if chat not in observed:
                                                                                
                    continue
                                                                              
                                                                               
                                                                                 
                self.fail('created.pending_remote')
            threads[chat]=core[chat]
        observed={chat for p in parts.values() for chat,_ in entries(p['core']['threads'])}
        if not observed <= routes.keys():
            self.fail('host.unknown')
        return parts,routes,threads

    def local_creation_controls(self,routes):
        """Read accepted input and exact durable effect, never child poststate.

        wire/tasks.rs builds Delegate.access from ChildSpec.config or its real
        default. delegation.rs checks it against parent and copies it unchanged
        into CreateChild. A remote receiver still needs its validated wire fact.
        """
        configured={};ceilings={}
        for row in self.prefix:
            if row.get('action')!='Apply' or not row.get('args',{}).get('accepted') or row['args'].get('duplicate'):
                continue
            ev=row.get('event',{});args=ev.get('args',{})
            if ev.get('act')=='OpenRoot':
                chat=args['t'];access=args['access']
                if routes.get(chat)==row['dev']:
                    ceilings[chat]=min(access,ceilings.get(chat,access))
            elif ev.get('act')=='Delegate':
                child=args['child'];parent=args['t'];access=args['access']
                if routes.get(child)!=row['dev'] or routes.get(parent)!=row['dev']:continue
                effects=[e for e in row['fx'] if e.get('kind')=='CreateChild' and e.get('child')==child and e.get('parent')==parent]
                if len(effects)!=1 or effects[0]['access']!=access:self.fail('access.local_create_effect')
                configured[child]=access
                ceilings[child]=min(access,ceilings.get(child,access))
        return configured,ceilings

    def observation_fields(self, row, parts, routes, present, tree):
        facts=self.facts(parts)
        def known(value):
            if isinstance(value,dict) and 'present' in value:
                return value['value'] if value['present'] else None
            return value
        config,ceiling=self.local_creation_controls(routes)
        starts={};instances={};current={};bindings={};selected_requests={};policies={}
        for dev,identity,fact in facts:
            data=fact['data'];kind=fact['kind']
            if kind in {'access_config','received_ceiling','provider_start'} and routes.get(data.get('chat'))!=dev:
                continue
            if kind=='access_config':
                chat=data['chat']
                if data.get('validated') is not True:self.fail('access.config_validated')
                selected=known(data.get('configured_access'))
                if selected is None:
                    if data.get('policy_source')=='request_default':
                        selected=known(data.get('submitted_access'))
                if selected is not None:policies[chat]=selected
                request=known(data.get('submitted_access'))
                if request is not None:selected_requests[chat]=request
            elif kind=='received_ceiling':
                if data.get('validated') is not True:self.fail('access.ceiling_validated')
                chat=data['chat'];access=data['access']
                wire=known(data.get('wire_id'))
                if data.get('source_kind') in {'create','clamp','followup'}:
                    if not wire:self.fail('access.ceiling_wire')
                    commits=dict(entries(row.get('wire_commits',fn([]))))
                    if wire not in commits:self.fail('access.ceiling_origin')
                    commit=commits[wire]
                    if commit['origin_device']!=data['origin_device'] or commit['target_host']!=data['receiver_device']:
                        self.fail('access.ceiling_route')
                    effect=known(data.get('effect_id'))
                    if effect is not None and effect!=commit['effect_id']:self.fail('access.ceiling_effect')
                    if data.get('wire_body_digest')!=commit['wire_body_digest']:self.fail('access.ceiling_body')
                if chat in ceiling and access>ceiling[chat] and data.get('source_kind')!='root_initial':
                    self.fail('access.ceiling_widened')
                ceiling[chat]=min(access,ceiling.get(chat,access))
            elif kind=='runtime_binding':
                if data.get('validated') is not True:self.fail('attAcc.binding_validated')
                chat=data['chat']; source=data['source_id']; revision=data['source_revision']
                if data['operation'] in {'install','config','rebind','issue'}:
                    run=known(data['run']); attempt=known(data['attempt']); access=known(data['effective_access'])
                    if None in (run,attempt,access):self.fail('attAcc.binding_identity')
                    if chat in bindings and bindings[chat][0]!=source:self.fail('attAcc.binding_instance_order')
                    if chat not in bindings or revision>bindings[chat][1]:
                        bindings[chat]=(source,revision,run,attempt,access)
                        selected_requests[chat]=access
                    if data['operation']=='issue':
                        if chat not in ceiling:self.fail('access.received_ceiling')
                        if access>ceiling[chat]:self.fail('access.issued_ceiling')
                        if chat in policies and access>policies[chat]:self.fail('access.issued_policy')
            elif kind=='provider_start' and data['outcome']=='engine_start_ok':
                access=known(data['effective_access'])
                if access is None:self.fail('attAcc.observed_access')
                if data['chat'] not in selected_requests:self.fail('access.configured_request')
                if access>selected_requests[data['chat']]:self.fail('access.provider_request')
                if data['chat'] in policies and access>policies[data['chat']]:self.fail('access.provider_policy')
                stamp=(data['chat'],data['run'],data['attempt'])
                if stamp in starts and starts[stamp][0]!=identity:self.fail('attAcc.duplicate_dispatch')
                starts[stamp]=(identity,access)
            elif kind=='engine_lifecycle':
                device=data['device'];instance=data['engine_instance_id'];state=data['state']
                if state not in {'started','stopping','stopped'}:self.fail('online.engine_lifecycle')
                old=instances.get((device,instance))
                if old=='stopped' and state!='stopped':self.fail('online.instance_reopened')
                if state=='started' and old is None:current[device]=instance
                instances[(device,instance)]=state
        logical_access={}
        for chat in tree:
            if chat not in present:logical_access[chat]=0;continue
            chain=[];cursor=chat
            while cursor!='none':
                if cursor in chain:self.fail('parent.cycle')
                if cursor not in present:self.fail('access.ancestor')
                chain.append(cursor);cursor=present[cursor]['parent']
                                                                           
                                                                                 
                                                                                
                                                                               
            logical_access[chat]=present[chat]['access']
            if chat not in ceiling:self.fail('access.received_ceiling')
            if logical_access[chat]>ceiling[chat]:self.fail('access.control_ceiling')
            if logical_access[chat] not in range(3):self.fail('access.level')
        online=[]
        for device in self.constants['Devices']['$set']:
            if device not in current:self.fail('online.engine_instance')
            online.append((device,instances[(device,current[device])]!='stopped'))
                                                                              
                                                                           
        latest={chat:((value[2],value[3]),value[4]) for chat,value in bindings.items()}
        return dict(access=fn(logical_access.items()),online=fn(online),
                    attAcc=fn((t,latest[t][1] if t in latest else 0) for t in tree),
                    attAccAt=fn((t,dict(run=latest[t][0][0],attempt=latest[t][0][1]) if t in latest else dict(run=0,attempt=0)) for t in tree))

    def native_observation_fields(self, parts, routes, present, root, focus):
        """Map physical source observations; reducer state is not provider truth.

        A complete empty source catalog proves the initial physical state. An
        opened stream with unknown alive/root flags cannot prove that state.
        Activation IDs map to ordinals only through the declared captured order.
        """
        owner=routes.get(focus,routes[root])
        facts=[fact for dev,identity,fact in self.facts(parts) if dev==owner]
        catalogs=[f['data'] for f in facts if f['kind']=='provider_stream_catalog']
        if not catalogs:self.fail('nSess.provider_catalog')
        catalog=catalogs[-1]
        if catalog.get('complete') is not True:self.fail('nSess.catalog_incomplete')
        def known(value,field):
            if isinstance(value,dict) and 'present' in value:
                if not value['present']:self.fail(field)
                return value['value']
            if value is None:self.fail(field)
            return value
        streams=[x for x in catalog['streams'] if x['chat']==focus]
        if not streams:
            raw_natives=[n for key,n in entries(parts[owner].get('core',{}).get('natives',fn([]))) if key[0]==focus]
            if raw_natives:self.fail('nPst.catalog_missing_history')
        byrun={}
        for f in facts:
            d=f['data']
            if f['kind']=='provider_start' and d.get('chat')==focus and d.get('outcome')=='engine_start_ok':
                pid=known(d.get('provider_return_id'),'nPrun.provider_binding')
                if pid in byrun and byrun[pid]!=(d['run'],d['attempt']):self.fail('nPrun.provider_binding_conflict')
                byrun[pid]=(d['run'],d['attempt'])
        active=[]
        for stream in streams:
            alive=known(stream.get('provider_alive'),'nSess.provider_alive')
            root=known(stream.get('root_turn_active'),'nProot.root_turn_active')
            if not isinstance(alive,bool) or not isinstance(root,bool):self.fail('nSess.boolean')
            if root and not alive:self.fail('nProot.dead_session')
            if alive:active.append(stream)
        sessions={known(x.get('provider_session_id'),'nSess.session_identity') for x in active}
        if len(sessions)>1:self.fail('nSess.multiple_live_sessions')
        slots=self.context.get('native_slots',{})
        subs=values(self.constants['Subs'])
        observed={}
        for stream in streams:
            natives=stream.get('natives',[])
            if isinstance(natives,dict) and '$fn' in natives:natives=[v for k,v in entries(natives)]
            for native in natives:
                path=native['native_path'];key=json.dumps(path,separators=(',',':'))
                slot=slots.get(focus,{}).get(key)
                if slot not in subs:self.fail('nPst.native_path_binding')
                if slot in observed:self.fail('nPst.reused_native_slot')
                pid=stream['provider_run_id']
                if pid not in byrun:self.fail('nPrun.provider_binding')
                activation=known(native.get('activation_id'),'nPep.activation_identity')
                order=self.context.get('activation_order',{}).get(focus,{}).get(key,[])
                if activation not in order:self.fail('nPep.activation_order')
                if len(set(order))!=len(order):self.fail('nPep.activation_order_duplicate')
                background=known(native.get('background'),'nPbg.spawn_background')
                                                                                
                                                                                 
                for ancestor in natives:
                    ap=ancestor['native_path']
                    if len(ap)<len(path) and path[:len(ap)]==ap:
                        background=background or known(ancestor.get('background'),'nPbg.ancestor_background')
                status=native['status']
                physical={'spawned':'running','running':'running','completed':'done',
                          'failed':'dead','interrupted':'dead','cancelled':'dead'}.get(status)
                if physical is None:self.fail('nPst.provider_status')
                if physical=='running' and not known(stream.get('provider_alive'),'nSess.provider_alive'):
                    self.fail('nPst.stream_closed_native_running')
                if not isinstance(background,bool):self.fail('nPbg.boolean')
                observed[slot]=dict(st=physical,ep=order.index(activation)+1,run=byrun[pid][0],
                                    bg=background,progress=native['progress_seen'])
        def field(name,key,initial):
            return name,fn((sub,observed[sub][key] if sub in observed else initial) for sub in subs)
        return dict(nSess=bool(active),nProot=any(x['root_turn_active']==True or x['root_turn_active']=={'present':True,'value':True} for x in active),
                    **dict([field('nPst','st','none'),field('nPep','ep',0),field('nPrun','run',0),
                            field('nPbg','bg',False),field('nPprog','progress',False)]))

    def engine(self, row, root, focus):
        parts,routes,all_threads=self.authority(row)
        tree=self.context['tree_threads'][root]
        children=[t for t in tree if t != root]
        if root not in all_threads:
            self.fail('created.root')
        present={t:all_threads[t] for t in tree if t in all_threads}
        for t,th in present.items():
            if th['run_base'] != 0:
                self.fail('run_history.pruned')
            if th['parent'] != 'none' and th['parent'] not in tree:
                self.fail('parent.outside_tree')
        def fmap(get, absent):
            return fn((t,get(t,present[t]) if t in present else deepcopy(absent)) for t in tree)
        def cmap(get, absent):
            return fn((t,get(t,present[t]) if t in present else deepcopy(absent)) for t in children)
        def runs(th): return th['runs']
        def ord_(th): return th['run_base']+len(runs(th))
        def live(th): return [i+1 for i,r in enumerate(runs(th)) if r['status']=='Live']
        def last(th):
            ended=[r for r in runs(th) if r['status']!='Live']
            return enum(ended[-1]['status']) if ended else 'none'
        def latest(th): return runs(th)[-1] if runs(th) else None
        out=dict(
            queue=fmap(lambda t,h:[message(m) for m in h['queue']],[]),
            held=fmap(lambda t,h:enum(h['held']),'none'),
            liveSet=fmap(lambda t,h:aset(live(h)),aset([])),
            rord=fmap(lambda t,h:ord_(h),0),
            intReq=fmap(lambda t,h:h['int_req'],False),
            att=fmap(lambda t,h:h['attempt']['n'],0),
            why=fmap(lambda t,h:enum(h['attempt']['reason']),'initial'),
            emitted=fmap(lambda t,h:h['attempt']['emitted'],False),
            kidOpen=fmap(lambda t,h:h['attempt']['kid_open'],False),
            pev=fmap(lambda t,h:aset(event(e) for e in values(h['inbox'])),aset([])),
            retries=fmap(lambda t,h:h['attempt']['retries'],0),
            steers=fmap(lambda t,h:h['attempt']['steers'],0),
            lastOut=fmap(lambda t,h:last(h),'none'),
            rkind=fmap(lambda t,h:message(latest(h)['start'])['k'] if latest(h) else 'none','none'),
            ulArmed=fmap(lambda t,h:h['ul']['armed'],0),
            ulDue=fmap(lambda t,h:h['ul']['due'],False),
            created=fmap(lambda t,h:True,False),
            parent=fmap(lambda t,h:h['parent'],'none'),
            depth=fmap(lambda t,h:h['depth'],0),
            rtask=fmap(lambda t,h:bool(latest(h) and message(latest(h)['start'])['k']=='task'),False),
            waitOn=fmap(lambda t,h:h['wait_on'],'none'),
            zWait=fmap(lambda t,h:h['late_wait'],'none'),
            host=fn((t,routes[t] if t in present else 'none') for t in tree),
        )
        def task(t,h,key,default):
            parent=h['parent']
            if parent=='none':self.fail('task.parent')
            owner=routes.get(parent)
            if owner not in parts:self.fail('task.parent_authority')
            local=dict(entries(parts[owner]['core']['threads']))
            if t not in local:self.fail('task.parent_shadow')
            source=local[t]['task']
            return source[key] if source['present'] else default
        for target,key,default in [('tSt','st','none'),('tWake','wake','always'),('tDel','delivery','pending'),('tCoh','cohort',0),('pushes','pushes',0)]:
            out[target]=cmap(lambda t,h,k=key,d=default:enum(task(t,h,k,d)) if isinstance(task(t,h,k,d),str) else task(t,h,k,d),default)
        maxruns=self.constants['MaxRuns']
        for target,key,default in [('cDisp','disp','open'),('cSt','st','none'),('cDel','del',aset([])),('cGen','generation',0)]:
            def cohorts(t,h,k=key,d=default):
                if len(h['runs'])>maxruns:self.fail('MaxRuns')
                def value(i):
                    if i>len(h['runs']):return deepcopy(d)
                    v=h['runs'][i-1]['cohort'][k]
                    return enum(v) if isinstance(v,str) else v
                return fn((i,value(i)) for i in range(1,maxruns+1))
            out[target]=fmap(cohorts,fn((i,deepcopy(default)) for i in range(1,maxruns+1)))
        questions={}
        for t in children:
            if t not in present:continue
            host=routes[t]
            found=[q for (child,qid),q in entries(parts[host]['core']['questions']) if child==t]
            if len(found)>1:self.fail('questions.multiple_lifecycles')
            if found:questions[t]=found[0]
        for target,key,default in [('qPend','pending',False),('qAt','holder','none'),('qVis','views',aset([])),('qEp','epoch',0)]:
            out[target]=fn((t,aset(v for v in values(questions[t]['views']) if v in values(questions[t]['seen'])) if target=='qVis' and t in questions else questions[t][key] if t in questions else deepcopy(default)) for t in children)
        out['qUsed']=fn((t,t in questions) for t in children)
                                                                             
                                                                                   
                                                                                 
                                                                               
        if self.context.get('require_observation_fields',True):
            out.update(self.observation_fields(row,parts,routes,present,tree))
            if self.constants.get('UseNS',True):
                out.update(self.native_observation_fields(parts,routes,present,root,focus))
        missing=[v for v in VARIABLES if v not in out]
        return out,missing

    def remaining_fields(self,image,root,focus):
        """Read complete causal history; no target action supplies values."""
        from composition_history_projection import history_fields
        from composition_transfer_projection import transfer_fields
        image.update(history_fields(self.prefix,self.context,root,focus))
        if self.constants.get('UseCT',True):
            image.update(transfer_fields(self.prefix,self.context,root))
        else:
            if self.context.get('transfer_constants',{}).get('UseTransferReporting') is not True:
                self.fail('transfer_reporting.required_product')
                                                                          
                                                                                
            tree=self.context['tree_threads'][root]
            no_thread=self.constants['NoThread'];no_provider=self.constants['NoProv']
            providers=values(self.constants['Providers'])
            no_x=dict(ty='none',src=no_thread,tgt=no_thread,sp=0,bp=0,hd=0,tp=no_provider,
                st='none',strat='none',why='none',sel=aset([]),om=aset([]),hdr=False,dl='none',cr=False)
            fresh=dict(alive=False,pcov=aset([]),pst=aset([]),ecov=aset([]),est=aset([]),seen=0,dpend=False)
            image.update(xMsgs=[],xCreated=fn((t,t==root) for t in tree),
                xForkOf=fn((t,dict(src=no_thread,w=0)) for t in tree),
                xKindOf=fn((t,'none') for t in tree),
                xRun=fn((t,dict(st='idle',p=no_provider,u=0,w=0)) for t in tree),
                xPt=fn((t,fn((p,deepcopy(fresh)) for p in providers)) for t in tree),
                xXf=fn((x,deepcopy(no_x)) for x in range(1,self.constants['MaxX']+1)),
                xNx=0,xVol=fn((t,'none') for t in tree),xDup=False,
                xAbort=fn((t,'none') for t in tree),xCrashLeft=self.constants['CTMaxCrash'],
                xStaleLeft=self.constants['MaxStale'],ctTrk=fn((t,False) for t in tree),
                consumedBy=fn((x,'none') for x in range(1,self.constants['MaxX']+1)))
        if not self.constants.get('UseNS',True):
            if not self.context.get('reporting_constants'):self.fail('reporting.required_product')
                                                                           
                                                                                
            subs=values(self.constants['Subs'])
            image.update(nRst='idle',nRord=0,nEndBy='none',nRec=False,
                         nSess=False,nProot=False,nInbox=[],nReplay=aset([]))
            for name,initial in [('nEst','none'),('nTask','none'),('nHow','none'),
                 ('nEep',0),('nErun',0),('nClosedEp',0),('nEbg',False),
                 ('nPst','none'),('nPep',0),('nPrun',0),('nPbg',False),('nPprog',False)]:
                image[name]=fn((sub,deepcopy(initial)) for sub in subs)
            for name,bound in [('nCrashLeft','NSMaxCrash'),('nDupLeft','MaxDup'),
                               ('nIntLeft','NSMaxInterrupt'),('nSessLeft','MaxSessEnd')]:
                image[name]=self.constants[bound]
        return image

    def run(self,rows):
        images=[]; actions=[]; source_map=[]
        active=set()
        for i,row in enumerate(rows):
            self.row=i
            self.action=row.get('action','Unknown')
            self.prefix.append(row)
            parts,routes,threads=self.authority(row)
            forest=[]; now=set()
            for root,tree in self.context['tree_threads'].items():
                if root not in threads:
                    if any(t in threads for t in tree):self.fail('created.root')
                    continue
                now.add(root)
                for focus in tree:
                    image,missing=self.engine(row,root,focus)
                    self.remaining_fields(image,root,focus)
                    missing=[v for v in VARIABLES if v not in image]
                    if missing:self.fail(missing[0])
                    forest.append(([root,focus],image))
            if not active<=now:self.fail('created.root_removed')
            images.append(fn(forest))
            source_map.append(dict(image=i,raw_row=i,macro_substep=0))
            if i:
                from runtime_action_projection import source_actions
                actions.append(source_actions(rows[i-1],row,self.context,images[-2],images[-1]))
            active=now
        return dict(constants=self.constants,images=images,actions=actions,source_map=source_map)


def project_full(rows,context):
    """Map globally merged projected rows, or fail a named missing field.

    context={constants,tree_threads,provenance:{kind:capture|verified_test,...}}.
    Constants are fixed before execution. No successors or model states are
    accepted in context. source_map retains exact raw row and macro index.
    """
    rows=transaction_image_rows(rows)
    reporting=None;transfer=None
    if not context['constants'].get('UseNS',True):
        from runtime_operational_projection import project_operational_reporting
        reporting=project_operational_reporting(rows,context)
    if not context['constants'].get('UseCT',True):
        from runtime_transfer_reporting_projection import project_transfer_reporting
        transfer=project_transfer_reporting(rows,context)
        if context.get('transfer_constants')!=transfer['transfer_constants']:
            raise FullProjectionError(0,'Context','transfer_reporting.constants_source')
    mapped=Projection(context).run(rows)
    if reporting is None and transfer is None:return annotate_transactions(mapped,rows)
    if reporting is None or transfer is None:
        raise FullProjectionError(0,'Context','operational.required_products')
    return annotate_transactions(align_operational_paths(mapped,reporting,transfer,context),rows)


def transaction_image_rows(rows):
    """Use own raw microstate and only committed foreign partitions.

    The merger makes each physical group contiguous. Internal Apply/Complete
    rows remain proof microsteps. They cannot become visible foreign state.
    Historical component inputs without known groups keep their prior scope.
    """
    result=[]
    for index,row in enumerate(rows):
        item=dict(row)
        tx=row.get('transaction',{})
        if tx.get('known') is True:
            if 'committed_partitions' not in row:
                raise FullProjectionError(index,row.get('action','Unknown'),'transaction.committed_partitions')
            raw=dict(entries(row['partitions']))
            committed=dict(entries(row['committed_partitions']))
            owner=row['dev']
            if owner not in raw:
                raise FullProjectionError(index,row.get('action','Unknown'),'transaction.own_partition')
            if any(dev not in committed or raw[dev]!=committed[dev] for dev in raw if dev!=owner):
                raise FullProjectionError(index,row.get('action','Unknown'),'transaction.foreign_uncommitted')
            committed[owner]=raw[owner]
            item['partitions']=fn(sorted(committed.items()))
        result.append(item)
    return result


def annotate_transactions(projection,rows):
    """Bind every macro image to its raw physical group and visibility.

    Only the last macro image of the last captured row in a known transaction
    is a visible commit. The earlier images do not assert a Pending window.
    """
    for name in ['source_map','reporting_source_map','transfer_source_map']:
        if name not in projection:continue
        sources=projection[name]
        last={source['raw_row']:index for index,source in enumerate(sources)}
        for index,source in enumerate(sources):
            raw=source['raw_row']
            if 'transaction' not in rows[raw]:continue
            tx=deepcopy(rows[raw]['transaction'])
            source['transaction']=tx
            source['physical_visible']=tx.get('known') is True and tx.get('visible') is True and last[raw]==index
    return projection


def align_operational_paths(core,reporting,transfer,context):
    """Align only declared per-source macros; never derive target images.

    A loss-closure macro commits its core task closure on its first substep.
    A saved CT observation expands while unrelated products stutter. Two
    independent multi-step macros in one raw row need a declared causal order.
    """
    def grouped(product,prefix):
        groups={}
        images=product[prefix+'_images'];actions=product[prefix+'_actions']
        maps=product[prefix+'_source_map']
        if len(images)!=len(maps) or len(actions)!=len(images)-1:
            raise FullProjectionError(0,'Context',prefix+'.alignment_cardinality')
        for i,(image,source) in enumerate(zip(images,maps)):
            groups.setdefault(source['raw_row'],[]).append((image,actions[i-1] if i else None,source))
        return groups
    rp=grouped(reporting,'reporting');tr=grouped(transfer,'transfer')
    result=dict(constants=core['constants'],reporting_constants=context['reporting_constants'],
        transfer_constants=transfer['transfer_constants'],images=[],actions=[],source_map=[],
        reporting_images=[],reporting_actions=[],reporting_source_map=[],
        transfer_images=[],transfer_actions=[],transfer_source_map=[])
    empty=dict(label='Stutter',args={})
    def core_stutter(image):return fn((key,deepcopy(empty)) for key,_ in entries(image))
    for raw,image in enumerate(core['images']):
        if raw not in rp or raw not in tr:
            raise FullProjectionError(raw,'Context','operational.raw_row_alignment')
        if raw==0:
            if len(rp[0])!=1:
                raise FullProjectionError(0,'Init','operational.initial_reporting_macro')
            result['images'].append(image);result['reporting_images'].append(rp[0][0][0]);result['transfer_images'].append(tr[0][0][0])
            result['source_map'].append(dict(raw_row=0,macro_substep=0))
            result['reporting_source_map'].append(rp[0][0][2]);result['transfer_source_map'].append(tr[0][0][2])
                                                                            
                                                                              
            for sub,(tr_image,tr_action,tr_source) in enumerate(tr[0][1:],1):
                result['images'].append(deepcopy(image));result['actions'].append(core_stutter(image))
                result['reporting_images'].append(deepcopy(rp[0][0][0]));result['reporting_actions'].append(deepcopy(empty))
                result['reporting_source_map'].append(dict(raw_row=0,macro_substep=sub,frame=True))
                result['transfer_images'].append(tr_image);result['transfer_actions'].append(tr_action);result['transfer_source_map'].append(tr_source)
                result['source_map'].append(dict(raw_row=0,macro_substep=sub,initial_observation=True))
            continue
        nr,nt=len(rp[raw]),len(tr[raw]);count=max(nr,nt)
        if nr>1 and nt>1:
            raise FullProjectionError(raw,'Context','operational.concurrent_macros')
        core_at=0 if nr>1 else count-1
        for sub in range(count):
            before=result['images'][-1]
            result['images'].append(image if sub>=core_at else before)
            result['actions'].append(core['actions'][raw-1] if sub==core_at else core_stutter(before))
            for name,groups,n in [('reporting',rp,nr),('transfer',tr,nt)]:
                if n>1:
                    next_image,action,source=groups[raw][sub]
                elif sub==core_at:
                    next_image,action,source=groups[raw][0]
                else:
                    next_image=result[name+'_images'][-1];action=empty
                    source=dict(raw_row=raw,macro_substep=sub,frame=True)
                result[name+'_images'].append(deepcopy(next_image));result[name+'_actions'].append(deepcopy(action));result[name+'_source_map'].append(deepcopy(source))
            result['source_map'].append(dict(raw_row=raw,macro_substep=sub,core_commit=sub==core_at))
    result['reporting_provenance']=reporting.get('reporting_provenance',[])
    result['transfer_scope']=transfer.get('transfer_scope')
    return result


def inspect_engine(row,context,index=0):
    """Save direct-field coverage without claiming a complete source image."""
    p=Projection(context);p.row=index;p.action=row.get('action','Unknown')
    result=[]
    for root,tree in context['tree_threads'].items():
        for focus in tree:
            image,missing=p.engine(row,root,focus)
            result.append(dict(root=root,focus=focus,mapped_fields=sorted(image),missing_fields=missing))
    return result
