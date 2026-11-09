"""Project finite counters and history from a complete causal capture prefix."""
from collections import Counter
from copy import deepcopy
import json

from runtime_full_projection import FullProjectionError, aset, entries, fn, message, values
from run_delegation_projection import RUN_DELEGATION_ACTIONS
from remote_projection import REMOTE_ACTIONS
from runtime_projection import NQ_ACTIONS

COUNTERS = {
    'rPersonLeft':'RLMaxPerson', 'serverLeft':'RLMaxServer', 'rStopLeft':'RLMaxStop',
    'ulLeft':'RLMaxUL', 'rCrashLeft':'RLMaxCrash', 'dPersonLeft':'DMaxPerson',
    'failLeft':'MaxFail', 'dCrashLeft':'DMaxCrash', 'abandonLeft':'MaxAbandon',
    'dStopLeft':'DMaxStop', 'narrowLeft':'MaxNarrow', 'interruptLeft':'MaxInterrupt',
    'nCrashLeft':'NSMaxCrash', 'nDupLeft':'MaxDup', 'nIntLeft':'NSMaxInterrupt',
    'nSessLeft':'MaxSessEnd', 'xCrashLeft':'CTMaxCrash', 'xStaleLeft':'MaxStale',
    'offLeft':'MaxOffline',
}
FACT_KINDS = {
    'provider_start','journal_observed','doc_commit','access_config','received_ceiling',
    'tool_reply','tool_observed','device_reachability','transfer_checkpoint',
    'transfer_state','transfer_operation','transfer_catalog','registry_host',
    'engine_lifecycle','remote_receive','admission_state','transport_connection',
    'provider_event','provider_stream_catalog','native_binding','native_doc_commit',
    'transfer_provider','capability_catalog','runtime_binding',
    'agent_admission',
}
PROVIDER_EVENTS = {
    'stream_opened','stream_event','stream_closed','stream_crashed','session_started',
    'session_closed','root_started','root_output','root_terminal','native_spawn',
    'native_progress','native_started','native_terminal',
}
TRANSFER_OPERATIONS = {'ForkPin','TransferPin','MergeBackPinAndQueue','PrepareRun',
                       'RejectResume','BindProvider'}
FRAME_ACTIONS = {'Init','Apply','CompleteEffect','MarkCommand','CommandDecision',
                 'Observe','ObserveCatalog','RecoveryOpen'}


def frozen(value):
    return json.dumps(value,sort_keys=True,separators=(',',':'))


def known(value):
    if isinstance(value,dict) and 'present' in value:
        return value.get('value') if value['present'] else None
    return value


def pairs(value):
    result=entries(value)
    keys=[frozen(key) for key,_ in result]
    if len(keys)!=len(set(keys)):
        raise ValueError('duplicate finite function key')
    return result


def ordinal(thread):
    return thread['run_base']+len(thread['runs'])


def live(thread):
    return any(run['status']=='Live' for run in thread['runs'])


class History:
    def __init__(self,context,root,focus):
        self.context=context
        self.constants=context['constants']
        if context.get('provenance',{}).get('kind') not in {'capture','verified_test'}:
            raise FullProjectionError(0,'Context','constants.provenance')
        self.tree=list(context['tree_threads'][root])
        if root not in self.tree or focus not in self.tree or len(set(self.tree))!=len(self.tree):
            raise FullProjectionError(0,'Context','history.tree')
        self.root,self.focus=root,focus
        self.legacy_native=self.constants.get('UseNS',True)
        if self.legacy_native is not True:
            if self.legacy_native is not False or context.get('reporting_constants',{}).get('UseReporting') is not True:
                raise FullProjectionError(0,'Context','reporting.required_product')
            if any(self.constants.get(key)!=0 for key in ('NSMaxCrash','MaxDup','NSMaxInterrupt','MaxSessEnd')):
                raise FullProjectionError(0,'Context','history.legacy_native_scope')
        self.children=[t for t in self.tree if t!=root]
        self.row=0
        self.action='Init'
        self.spent=Counter()
        self.facts={}
        self.effects={}
        self.starts={}
        self.journals={}
        self.entered_dispatches=set()
        self.replies={}
        self.provider_events={}
        self.apply_receipts={}
        self.lifecycle={}
        self.current_instance={}
        self.entered={t:False for t in self.children}
        self.question_keys={}
        self.wake=set()
        self.answers=Counter()
        self.stop_obligations=set()
        self.wait_attempt={t:0 for t in self.tree}
        self.resume_wake={t:0 for t in self.tree}
        self.clamp_pending=set()
        self.notice_receipts=set()
        self.delivery_receipts=set()
        self.replayable={}
        self.replay_bad=False
        self.receipt_bad=False
        self.previous={}
        self.previous_questions={}
        self.last_parts={}
        self.routes={}
        self.transfer_facts=[]
        self.transfer_order=[]
        self.transfer_seen=set()
        self.transfer_prepares=[]
        self.runtime_bindings=[]
        self.previous_fact_keys={}
        self.previous_effect_keys={}
        for name,bound in COUNTERS.items():
            if type(self.constants.get(bound)) is not int or self.constants[bound]<0:
                self.fail(name+'.bound')

    def fail(self,field):
        raise FullProjectionError(self.row,self.action,field)

    def spend(self,name):
        self.spent[name]+=1
        if self.spent[name]>self.constants[COUNTERS[name]]:
            self.fail(name+'.exhausted')

    def snapshots(self,row):
        parts=dict(pairs(row['partitions']))
        routes={}
        for dev,part in parts.items():
            for chat,binding in pairs(part['mechanism']['bindings']):
                if binding['host_known']:
                    if chat in routes and routes[chat]!=binding['host']:
                        self.fail('history.host_conflict')
                    routes[chat]=binding['host']
            for identity,fact in pairs(part['mechanism']['facts']):
                if fact['kind']=='registry_host':
                    data=fact['data']
                    if data.get('validated') is not True:self.fail('history.host_validated')
                    if data['chat'] in routes and routes[data['chat']]!=data['host']:
                        self.fail('history.host_conflict')
                    routes[data['chat']]=data['host']
        observed={t for part in parts.values() for t,th in pairs(part['core']['threads']) if t in self.tree}
        if not observed<=routes.keys():self.fail('history.host_unknown')
        threads={}
        for chat in self.tree:
            host=routes.get(chat)
            if host is None:
                continue
            if host not in parts:
                self.fail('history.host_partition')
            local=dict(pairs(parts[host]['core']['threads']))
            if chat in local:
                threads[chat]=local[chat]
        self.routes=routes
        questions={}
        for dev,part in parts.items():
            for key,question in pairs(part['core']['questions']):
                if key[0] not in self.children or routes.get(key[0])!=dev:
                    continue
                child,qid=key
                if child in self.question_keys and self.question_keys[child]!=qid:
                    self.fail('questions.multiple_lifecycles')
                self.question_keys[child]=qid
                questions[(child,qid)]=question
        return parts,threads,questions

    def descendants(self,threads,t):
        result={t}
        changed=True
        while changed:
            added={c for c,th in threads.items() if th['parent'] in result}-result
            changed=bool(added)
            result.update(added)
        return result

    def effect_image(self,item,threads):
        kind=item['kind']
        if kind=='Offer':
            result=dict(k='offer',t=item['parent'],r=item['run'],g=item['generation'])
        elif kind=='Notice':
            result=dict(k='notice',t=item['child'],r=item['run'],g=0)
        elif kind=='Stop':
            result=dict(k='stop',t=item['child'],r=item['stop'],g=0)
        elif kind in {'QuestionUp','QuestionRelease','QuestionClosed','AnswerDown'}:
            hop=item.get('via',item.get('to','none'))
            if hop!='none' and hop not in threads:
                self.fail('outbox.hop')
            depth=0 if hop=='none' else threads[hop]['depth']
            result=dict(k={'QuestionUp':'qup','QuestionRelease':'rel',
                           'QuestionClosed':'qclosed','AnswerDown':'ans'}[kind],
                        t=item['child'],r=item['epoch'],g=0 if kind=='AnswerDown' else depth)
        else:
            self.fail('outbox.message_kind')
        if result['t'] not in self.tree:
            return None
        return result

    def domain(self,threads,questions):
        result=[]
        for t in self.tree:
            if t not in threads:
                result.append((t,None))
                continue
            th=threads[t]
            result.append((t,dict(parent=th['parent'],depth=th['depth'],access=th['access'],
                queue=[message(m) for m in th['queue']],held=th['held'],
                live=[th['run_base']+i+1 for i,r in enumerate(th['runs']) if r['status']=='Live'],
                ordinal=ordinal(th),int_req=th['int_req'],wait=th['wait_on'],late=th['late_wait'],
                task=th['task'],cohorts=[r['cohort'] for r in th['runs']],
                kind=message(th['runs'][-1]['start'])['k'] if th['runs'] else 'none')))
        qv=[(key,dict(pending=q['pending'],holder=q['holder'],epoch=q['epoch'],
                     visible=sorted(set(values(q['views']))&set(values(q['seen'])))))
            for key,q in questions.items()]
        return frozen([result,qv,sorted(self.wake),sorted(self.answers.items()),sorted(self.notice_receipts),
                       sorted(self.delivery_receipts)])

    def accepted(self,row,threads,questions):
        if row['action']!='Apply':
            return
        args=row['args']
        if type(args.get('accepted')) is not bool or type(args.get('duplicate')) is not bool:
            self.fail('history.receipt_flags')
        ev=row.get('event')
        if not isinstance(ev,dict) or ev.get('act') not in RUN_DELEGATION_ACTIONS|REMOTE_ACTIONS|NQ_ACTIONS:
            self.fail('history.event_family')
        receipt=(row['dev'],args['key'])
        if args.get('duplicate'):
            if receipt not in self.apply_receipts or self.apply_receipts[receipt]!=args['accepted']:
                self.fail('replayable.source_receipt')
            if self.domain(self.previous,self.previous_questions)!=self.domain(threads,questions):
                self.replay_bad=True
                self.receipt_bad=True
            return
        if receipt in self.apply_receipts:self.fail('replayable.receipt_reused')
        self.apply_receipts[receipt]=args['accepted']
        if not args.get('accepted'):
            return
        act=ev['act'];a=ev['args']
        t=a.get('t',a.get('parent',a.get('child')))
        relevant=t in self.tree
        if relevant:
            for name,acts in {
                'rPersonLeft':{'PersonSend','Steer','RestartActive'},
                'serverLeft':{'ServerEnqueue'},'rStopLeft':{'Stop','RequestInterrupt'},
                'dPersonLeft':{'PersonSend'},'dStopLeft':{'Stop'},
                'abandonLeft':{'WaitAbandon'},'narrowLeft':{'NarrowAccess'},
                'interruptLeft':{'InterruptChat'},
            }.items():
                if act in acts:self.spend(name)
            if act=='ProviderTerminal':
                if a['outcome']=='UsageLimit':self.spend('ulLeft')
                if a['outcome']=='Crash':self.spend('rCrashLeft')
            if act=='PersonSend':self.stop_obligations.discard(t)
            if act=='Stop':self.stop_obligations.update(self.descendants(self.previous,t))
            if act in {'Delegate','Followup','RemoteFollowup'}:
                child=a['child']
                if child in self.entered:self.entered[child]=False
                if t in self.stop_obligations:self.stop_obligations.add(child)
                if a.get('mode')=='Wait':
                    if t not in self.previous:self.fail('waitAtt.before_attempt')
                    self.wait_attempt[t]=self.previous[t]['attempt']['n']
            if act=='UsageLimitRecovery' and t in self.previous:
                if any(m['kind']=='Wake' for m in self.previous[t]['queue']):self.resume_wake[t]=1
            if act=='StartFromQueueWake' and self.resume_wake[t]==1:self.resume_wake[t]=2
            if act in {'NarrowAccess','RemoteClamp'}:
                for child in self.descendants(self.previous,t):
                    th=self.previous.get(child)
                    if th is None:continue
                    changed=a['access']<th['access']
                    eligible=live(th) and not th['attempt']['emitted'] and not th['int_req'] and not th['reconfigure']
                    if changed and eligible:
                        self.clamp_pending.add(child)
            if act=='Stop':
                self.wake.difference_update(k for k,q in self.previous_questions.items() if q['holder']==t)
            if act=='StartQuestion':
                self.wake.difference_update(k for k,q in self.previous_questions.items() if q['holder']==t)
            if act=='WaitReturn' and t in self.previous:
                child=self.previous[t]['wait_on']
                self.wake.difference_update(k for k,q in self.previous_questions.items()
                    if k[0]==child and q['pending'] and q['holder']==t)
        if act=='CrashEngine' and row['dev'] in {self.routes.get(t) for t in self.tree}:self.spend('dCrashLeft')
        for child in self.tree:
            before=self.previous.get(child);after=threads.get(child)
            if before and after and after['attempt']['n']>before['attempt']['n']:
                self.clamp_pending.discard(child)
        if act in {'AgentAnswer'}:
            self.wake.discard((a['child'],a['qid']))
        for key,q in questions.items():
            old=self.previous_questions.get(key)
            if old and old['pending'] and not q['pending']:
                closure=act in {'PersonAnswer','ImportAnswer','ImportRelease','DeliverQuestion'}
                if closure:
                    self.answers[key]+=1
                    if self.answers[key]>2:self.fail('answers.lifecycle_bound')
            if old and old['holder']!='none' and q['holder']=='none' and q['epoch']>old['epoch']:
                self.wake.discard(key)
            if act in {'ImportQuestion','DeliverQuestion'} and q['pending'] and q['holder']!='none':
                if old is None or q['holder']!=old['holder'] or q['epoch']!=old['epoch']:
                    parent=q['holder'];child=key[0]
                    task=threads.get(child,{}).get('task',{})
                    waited=parent in threads and threads[parent]['wait_on']==child and task.get('wake')=='SettledOnly'
                    if not waited:self.wake.add(key)
        delivered=None
        if act in {'DoNotice','DoOffer','DoStop'}:
            if act=='DoNotice':delivered=dict(kind='Notice',child=a.get('c',a.get('child')),run=a['run'])
            elif act=='DoOffer':delivered=dict(kind='Offer',parent=a.get('p',a.get('parent')),run=a['run'],generation=a['generation'])
            else:delivered=dict(kind='Stop',child=a.get('c',a.get('child')),stop=a['stop'])
        elif act=='DeliverQuestion':delivered=a['message']
        elif act=='RemoteNotice':delivered=dict(kind='Notice',child=a['child'],run=a['run'])
        elif act=='RemoteStop':delivered=dict(kind='Stop',child=a['child'],stop=a['stop'])
        if delivered is not None:
            relevant=delivered.get('child',delivered.get('parent')) in self.tree
            if relevant:
                identity=frozen(delivered)
                domain=self.domain(self.previous,self.previous_questions)
                repeated=identity in self.delivery_receipts
                in_replay=identity in self.replayable
                if delivered['kind']=='Notice':self.notice_receipts.add((delivered['child'],delivered['run']))
                self.delivery_receipts.add(identity)
                if a.get('how')=='Keep':self.replayable[identity]=deepcopy(delivered)
                else:self.replayable.pop(identity,None)
                after=self.domain(threads,questions)
                if repeated and domain!=after:self.receipt_bad=True
                if in_replay and domain!=after:self.replay_bad=True

    def facts_next(self,parts,threads):
        for dev,part in parts.items():
            mechanism=part['mechanism']
            for key,value in pairs(mechanism['effects']):
                scoped=(dev,key)
                effect=value['effect']
                if scoped in self.effects and self.effects[scoped]!=effect:self.fail('outbox.effect_rebound')
                self.effects[scoped]=deepcopy(effect)
            effect_keys={k for k,v in pairs(mechanism['effects'])}
            if not self.previous_effect_keys.get(dev,set())<=effect_keys:self.fail('outbox.effect_disappeared')
            self.previous_effect_keys[dev]=effect_keys
            facts=dict(pairs(mechanism['facts']))
            if not self.previous_fact_keys.get(dev,set())<=set(facts):self.fail('history.fact_disappeared')
            self.previous_fact_keys[dev]=set(facts)
            if len(mechanism['fact_order'])!=len(facts) or set(mechanism['fact_order'])!=set(facts):self.fail('history.fact_order')
            for identity in mechanism['fact_order']:
                fact=facts[identity];scoped=(dev,identity)
                if scoped in self.facts:
                    if fact!=self.facts[scoped]:self.fail('history.immutable_fact')
                    continue
                self.facts[scoped]=deepcopy(fact)
                kind=fact['kind'];data=fact['data']
                if kind not in FACT_KINDS:self.fail('history.fact_kind')
                if kind=='agent_admission':
                    if data.get('validated') is not True:self.fail('history.admission_validated')
                    claimed=known(data.get('runtime_id'));provider=known(data.get('provider_run_id'))
                    handle=(known(data.get('handle_run')),known(data.get('handle_attempt')))
                    current=(known(data.get('current_run')),known(data.get('current_attempt')))
                    accepted=provider is not None and claimed==provider and handle==current
                    if accepted and any(type(n) is not int or n<1 for n in handle):
                        self.fail('history.admission_attempt')
                    if data.get('disposition')!=('accepted' if accepted else 'refused') or data.get('reason')!=('current_runtime_attempt' if accepted else 'runtime_attempt_mismatch'):
                        self.fail('history.admission_guard')
                elif kind=='capability_catalog':
                    if data.get('scope')!='selected_harnesses' or data.get('complete') is not True:
                        self.fail('history.capability_catalog')
                elif kind=='engine_lifecycle':
                    device=data['device'];instance=data['engine_instance_id'];state=data['state']
                    if device!=dev or state not in {'started','stopping','stopped'}:self.fail('offLeft.lifecycle')
                    old=self.lifecycle.get((device,instance))
                    if old=='stopped' and state!='stopped':self.fail('offLeft.instance_reopened')
                    if state=='started' and old is None:self.current_instance[device]=instance
                    if state=='stopped' and old!='stopped' and self.current_instance.get(device)==instance:
                        self.spend('offLeft')
                    self.lifecycle[(device,instance)]=state
                elif kind=='provider_event':
                    if data['event_kind'] not in PROVIDER_EVENTS:self.fail('history.provider_event_kind')
                    event_id=(dev,data['event_id'])
                    if event_id in self.provider_events:self.fail('history.provider_event_identity')
                    replay=known(data.get('replay_of'))
                    if replay is not None:
                        if (dev,replay) not in self.provider_events:self.fail('nDupLeft.replay_identity')
                        source=self.provider_events[(dev,replay)]
                        if (source['chat'],source['provider_run_id'],source['payload_hash'])!=(data['chat'],data['provider_run_id'],data['payload_hash']):
                            self.fail('nDupLeft.replay_payload')
                        if self.legacy_native and data['chat']==self.focus:self.spend('nDupLeft')
                    elif self.legacy_native and data['chat']==self.focus and data['event_kind']=='root_terminal' and known(data.get('status'))=='interrupted':
                        self.spend('nIntLeft')
                    self.provider_events[event_id]=deepcopy(data)
                elif kind in {'tool_reply','tool_observed'}:
                    key=tuple(data[k] for k in ('parent','parent_run','parent_attempt','child','task_run','reply_id'))
                    if kind=='tool_reply':self.replies[(dev,key)]=deepcopy(data)
                    else:
                        if self.replies.get((dev,key))!=data:self.fail('kidIn.tool_reply')
                        if data['parent'] in self.entered:
                            parent=threads.get(data['parent'])
                            if parent is None or ordinal(parent)!=data['parent_run'] or parent['attempt']['n']!=data['parent_attempt']:
                                self.fail('kidIn.tool_attempt')
                            child=threads.get(data['child'])
                            if child is None or child['parent']!=data['parent'] or type(data['task_run']) is not int or data['task_run']<1:
                                self.fail('kidIn.tool_child_identity')
                            self.entered[data['parent']]=True
                elif kind=='provider_start':
                    self.starts[(dev,data['effect_id'])]=deepcopy(data)
                elif kind=='journal_observed':
                    self.journals[(dev,data['effect_id'])]=deepcopy(data)
                elif kind=='runtime_binding':
                    if data.get('operation') not in {'install','issue','rebind','retire','remove','config'}:
                        self.fail('ctTrk.runtime_binding_operation')
                    self.runtime_bindings.append(deepcopy(data))
                elif kind in {'transfer_catalog','transfer_state'}:
                    states=data['chats'] if kind=='transfer_catalog' else [data]
                    for state in states:
                        ids={key for key,item in pairs(state['ledger'])}
                        if not ids<=self.transfer_seen:self.fail('consumedBy.unordered_catalog')
                elif kind=='transfer_operation':
                    self.transfer_facts.append((self.row,dev,identity,deepcopy(data)))
                    self.transfer_next(data)
        for key,start in self.starts.items():
            if key in self.entered_dispatches:continue
            if start['chat'] not in self.entered or start['outcome']!='engine_start_ok':continue
            effect=self.effects.get(key);journal=self.journals.get(key)
            if effect is None:self.fail('kidIn.start_effect')
            if effect['kind']!='StartRun' or not effect['delivered']:continue
            if journal is None:continue
            if journal['journal_seq_after']<=journal['journal_seq_before']:self.fail('kidIn.journal_advance')
            if any(journal[k]!=start[k] for k in ('chat','run','attempt','message_id','journal_seq_before','journal_seq_after')):self.fail('kidIn.journal_identity')
            if effect['run']['thread']!=start['chat'] or effect['run']['ord']!=start['run']:
                self.fail('kidIn.effect_run_identity')
            if any(item['child'] not in threads or threads[item['child']]['parent']!=start['chat'] or type(item['run']) is not int or item['run']<1 for item in effect['delivered']):
                self.fail('kidIn.delivered_child_identity')
            self.entered_dispatches.add(key)
            th=threads.get(start['chat'])
            task_matches=th and (not th['task']['present'] or th['task']['run']<=start['run'])
            if task_matches and ordinal(th)==start['run'] and th['attempt']['n']==start['attempt']:
                self.entered[start['chat']]=True

    def step(self,row,index):
        self.row=index;self.action=row.get('action','Unknown')
        if self.action not in FRAME_ACTIONS:self.fail('history.frame_action')
        parts,threads,questions=self.snapshots(row)
        self.accepted(row,threads,questions)
        self.facts_next(parts,threads)
        self.previous=deepcopy(threads)
        self.previous_questions=deepcopy(questions)
        self.last_parts=parts

    def transfer_next(self,data):
        operation=data.get('operation')
        if operation not in TRANSFER_OPERATIONS:self.fail('ctTrk.operation_kind')
        required={'operation','args','before','after','succeeded','journal'}
        if set(data)!=required:self.fail('ctTrk.operation_schema')
        if type(data['succeeded']) is not bool:self.fail('ctTrk.operation_result')
        before=data['before'];after=data['after']
        if before['chat_id']!=after['chat_id']:self.fail('ctTrk.chat_identity')
        previous=before
        before_ids={key for key,item in pairs(before['ledger'])}
        if not before_ids<=self.transfer_seen:self.fail('consumedBy.unordered_before')
        for change in data['journal']:
            if change['before']!=previous:self.fail('consumedBy.journal_chain')
            current=change['after']
            old_ids={key for key,item in pairs(previous['ledger'])}
            new_ids={key for key,item in pairs(current['ledger'])}-old_ids
            unknown=new_ids-self.transfer_seen
            if len(unknown)>1:self.fail('consumedBy.ambiguous_pin_order')
            for identity in unknown:
                self.transfer_seen.add(identity)
                self.transfer_order.append(identity)
            previous=current
        if previous!=after:self.fail('consumedBy.journal_after')
        if operation=='PrepareRun':self.transfer_prepares.append(deepcopy(data))

    def transfer_result(self):
        maximum=self.constants.get('MaxX')
        if type(maximum) is not int or maximum<0:self.fail('consumedBy.bound')
        if len(self.transfer_order)>maximum:self.fail('consumedBy.exhausted')
        tracked={t:False for t in self.tree}
        consumed={x:'none' for x in range(1,maximum+1)}
        index={identity:i+1 for i,identity in enumerate(self.transfer_order)}
        for data in self.transfer_prepares:
            args=data['args'];chat=args['chat']
            if chat not in self.tree:continue
            if data['after']['chat_id']!=chat:self.fail('ctTrk.prepare_chat')
            identities=set()
            for start in self.starts.values():
                if start['chat']==chat and start['message_id']==args['input']:
                    identities.add((start['run'],start['attempt']))
            for binding in self.runtime_bindings:
                if binding['chat']==chat and binding['origin_id']==args['input']:
                    if binding.get('validated') is not True:self.fail('ctTrk.runtime_binding_validated')
                    run,attempt=known(binding.get('run')),known(binding.get('attempt'))
                    if run is None or attempt is None:self.fail('ctTrk.runtime_binding_attempt')
                    identities.add((run,attempt))
            runs={run for run,attempt in identities}
            if any(type(run) is not int or run<1 or type(attempt) is not int or attempt<1 for run,attempt in identities):
                self.fail('ctTrk.run_attempt_type')
            if len(runs)!=1:self.fail('ctTrk.prepare_run_identity')
            run=next(iter(runs));th=self.previous.get(chat)
            if th is None or run<=th['run_base'] or run>ordinal(th):self.fail('ctTrk.retained_run')
            start=th['runs'][run-th['run_base']-1]
            kind=message(start['start'])['k']
            tracked[chat]=ordinal(th)==run and start['status']=='Live'
            before={key:item for key,item in pairs(data['before']['committed'])}
            after={key:item for key,item in pairs(data['after']['committed'])}
            records={key:item for key,item in pairs(data['after']['ledger'])}
            run_packets=dict(pairs(data['after']['runs']))
            receipts=dict(pairs(data['after']['receipts']))
            rejected=values(data['before']['rejected']) if isinstance(data['before']['rejected'],dict) else data['before']['rejected']
            provider=known(args.get('provider_session'))
            lineage='replacement' if args['harness']+':'+args['input'] in rejected else 'fresh'
            delivery=args['harness']+':'+(provider if provider is not None and provider!='none' else lineage+':'+args['input'])
            run_key=delivery+':'+args['input']
            for identity,record in after.items():
                if record.get('status')!='Consumed' or before.get(identity,{}).get('status')=='Consumed':continue
                if identity not in index or records.get(identity)!=record:self.fail('consumedBy.committed_identity')
                if record['target']!=chat:self.fail('consumedBy.target')
                if not data['succeeded']:self.fail('consumedBy.failed_prepare')
                received=receipts.get(delivery)
                if received is None:self.fail('consumedBy.delivery_receipt')
                received=values(received) if isinstance(received,dict) else received
                if not set(record['selected'])<=set(received):self.fail('consumedBy.selected_receipt')
                packet=run_packets.get(run_key+':part:'+identity,run_packets.get(run_key))
                if packet is None or packet!=record['packet']:self.fail('consumedBy.run_packet')
                consumed[index[identity]]=kind
        return dict(ctTrk=fn(tracked.items()),consumedBy=fn(consumed.items()))

    def result(self):
        outbox={};seen={}
        for dev,part in self.last_parts.items():
            for item in values(part['core']['outbox']):
                mapped=self.effect_image(item,self.previous)
                if mapped is None:continue
                identity=frozen(item);target=frozen(mapped)
                if target in seen and seen[target]!=identity:self.fail('outbox.identity_collision')
                seen[target]=identity
                if not any(d==dev and fx.get('kind')=='Send' and fx['message']==item for (d,k),fx in self.effects.items()):
                    self.fail('outbox.producer_effect')
                outbox[identity]=mapped
        replay=[]
        for identity,item in self.replayable.items():
            if identity in outbox:replay.append(outbox[identity])
        receipts=set(self.notice_receipts)
        offset=max(self.constants['DMaxStop'],1)
        rcpt={(t,offset+r) for t,r in receipts}
        for part in self.last_parts.values():
            rcpt.update((t,n) for t,n in values(part['core']['stop_receipts']) if t in self.tree)
        result={name:self.constants[bound]-self.spent[name] for name,bound in COUNTERS.items()}
        result.update(kidIn=fn(self.entered.items()),qWake=aset(sorted(k[0] for k in self.wake)),
            answers=fn((t,self.answers[(t,self.question_keys[t])]) if t in self.question_keys else (t,0) for t in self.children),
            outbox=aset(outbox.values()),replayable=aset(replay),rcpt=aset([t,n] for t,n in sorted(rcpt)),
            replayBad=self.replay_bad,receiptBad=self.receipt_bad,doomed=aset(sorted(self.stop_obligations)),
            waitAtt=fn(self.wait_attempt.items()),f9w=fn(self.resume_wake.items()),owed=aset(sorted(self.clamp_pending)))
        result.update(self.transfer_result())
        return result


def source_history(prefix,context,root,focus):
    if not prefix:raise FullProjectionError(0,'Init','history.empty_prefix')
    try:
        history=History(context,root,focus)
    except FullProjectionError:
        raise
    except (KeyError,TypeError,ValueError,IndexError):
        raise FullProjectionError(0,'Context','history.context_schema') from None
    try:
        for index,row in enumerate(prefix):history.step(row,index)
        return history
    except FullProjectionError:
        raise
    except (KeyError,TypeError,ValueError,IndexError):
        history.fail('history.source_schema')


def transfer_identity_map(prefix,context,root,focus):
    history=source_history(prefix,context,root,focus)
    maximum=context['constants'].get('MaxX')
    if type(maximum) is not int or maximum<0:history.fail('consumedBy.bound')
    if len(history.transfer_order)>maximum:history.fail('consumedBy.exhausted')
    return fn((identity,i+1) for i,identity in enumerate(history.transfer_order))


def history_fields(prefix,context,root,focus,*,image=None):
    history=source_history(prefix,context,root,focus)
    try:
        return history.result()
    except FullProjectionError:
        raise
    except (KeyError,TypeError,ValueError,IndexError):
        history.fail('history.source_schema')
