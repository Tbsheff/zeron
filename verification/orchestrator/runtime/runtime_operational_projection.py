"""Join source-derived native reporting states for the full chat forest.

Each chat map reads its owner's complete captured prefix. This join preserves
old pending events in their observed order and appends only one newly observed
source event per raw row. It never uses a target transition relation.
"""
from collections import Counter
from copy import deepcopy

from native_reporting_projection import project_native_reporting
from runtime_full_projection import FullProjectionError,entries,fn,aset
from runtime_reporting_actions import canonical,equal,reporting_actions,facts

FUNCTION_FIELDS=('catalogComplete','runs','sources','handles','admitted','natives','tasks','docTasks')


def fail(index,action,field):
    raise FullProjectionError(index,action,'reporting_projection.'+field)


def initial_chat(parent,context):
    """Represent no observed source history with an unknown inventory flag.

    An unobserved owner partition makes no claim about its provider processes.
    The reporting product admits no agent work until its catalog is complete.
    """
    no_runtime=context['reporting_constants']['NoRuntime']
    return dict(catalogComplete=fn([(parent,False)]),
        runs=fn([(parent,dict(run=0,attempt=0,active=False,endBy='none'))]),
        sources=fn([]),natives=fn([]),tasks=fn([]),docTasks=fn([]),
        handles=fn([(parent,no_runtime)]),admitted=fn([(parent,dict(present=False))]),
        pending=[],lastCause=dict(kind='none',source=no_runtime,changed=aset([])))


def join_pending(previous,chat_states,index,action):
    """Keep observed global order; refuse ambiguous multi-event source rows."""
    available=Counter(canonical(event) for state in chat_states.values() for event in state['pending'])
    old_counts=Counter(canonical(event) for event in previous)
    if any(count>1 and available[identity]<count for identity,count in old_counts.items()):
        fail(index,action,'ambiguous_pending_removal')
    result=[]
    for event in previous:
        identity=canonical(event)
        if available[identity]:
            result.append(deepcopy(event));available[identity]-=1
    appended=[]
    for state in chat_states.values():
        for event in state['pending']:
            identity=canonical(event)
            if available[identity]:
                appended.append(deepcopy(event));available[identity]-=1
    if len(appended)>1:fail(index,action,'multiple_pending_append')
    return result+appended


def merge_chat_states(states,previous_states,previous_global,index,action,context):
    """Join disjoint source ownership; never sort pending events by parent."""
    image={}
    for field in FUNCTION_FIELDS:
        pairs=[];seen={}
        for parent,state in states.items():
            for key,value in entries(state[field]):
                identity=canonical(key)
                if identity in seen:fail(index,action,field+'_alias')
                seen[identity]=parent;pairs.append((deepcopy(key),deepcopy(value)))
        image[field]=fn(pairs)
    previous_pending=previous_global['pending'] if previous_global is not None else []
    image['pending']=join_pending(previous_pending,states,index,action)
    changed=[parent for parent,state in states.items()
             if not equal(state,previous_states.get(parent,initial_chat(parent,context)))]
    if not changed:
        image['lastCause']=deepcopy(previous_global['lastCause'] if previous_global is not None else
                                    dict(kind='none',source=context['reporting_constants']['NoRuntime'],changed=aset([])))
    else:
        causes=[states[parent]['lastCause'] for parent in changed]
        if any(not equal(cause,causes[0]) for cause in causes[1:]):
            fail(index,action,'multiple_last_cause')
        image['lastCause']=deepcopy(causes[0])
    return image


def prefix_routes(row,index):
    """Use only positive placement evidence present at this raw row."""
    routes={}
    def register(chat,host):
        if not isinstance(host,str) or not host or host=='none':
            fail(index,row.get('action','Unknown'),'owner_identity')
        if chat in routes and routes[chat]!=host:
            fail(index,row.get('action','Unknown'),'owner_conflict')
        routes[chat]=host
    for device,part in entries(row['partitions']):
        for chat,binding in entries(part['mechanism']['bindings']):
            if binding['host_known'] is True:register(chat,binding['host'])
    for scoped,fact in facts(row).items():
        if fact['kind']=='registry_host':
            data=fact['data']
            if data.get('validated') is not True:fail(index,row.get('action','Unknown'),'owner_validated')
            register(data['chat'],data['host'])
    return routes


def require_source_authority(row,index,routes,new_facts,parents):
    """Refuse physical evidence that has no current captured owner."""
    parts=dict(entries(row['partitions']))
    def require(chat):
        if chat not in parents:return
        owner=routes.get(chat)
        if owner not in parts or chat not in dict(entries(parts[owner]['core']['threads'])):
            fail(index,row.get('action','Unknown'),'physical_owner_unknown')
    accepted=row.get('action')=='Apply' and row.get('args',{}).get('accepted') is True and row.get('args',{}).get('duplicate') is False
    if accepted and row.get('event',{}).get('act') in {
        'StartFromQueue','StartFromQueueWake','StartQuestion','UsageLimitRecovery',
        'RetryAttempt','RestartActive','ProviderTerminal','IngestEnd'}:
        require(row['event']['args']['t'])
    for fact in new_facts:
        kind=fact['kind'];data=fact['data']
        if kind in {'runtime_binding','provider_event','native_binding','native_doc_commit'}:
            require(data['chat'])
        elif kind=='agent_admission' and data.get('disposition')=='accepted':require(data['chat'])
        elif kind=='provider_start' and data.get('outcome')=='engine_start_ok':require(data['chat'])
        elif kind=='provider_stream_catalog':
            for stream in data.get('streams',[]):require(stream['chat'])


def eligible_row(row,owner,created_at,first_facts):
    """Do not reuse an old device catalog as a future chat inventory."""
    result=deepcopy(row);parts=dict(entries(result['partitions']))
    mechanism=parts[owner]['mechanism'];excluded=[]
    for identity,fact in entries(mechanism['facts']):
        if fact['kind']=='provider_stream_catalog' and first_facts[(owner,identity)]<created_at:
            excluded.append(identity)
    mechanism['facts']=fn((identity,fact) for identity,fact in entries(mechanism['facts']) if identity not in excluded)
    mechanism['fact_order']=[identity for identity in mechanism['fact_order'] if identity not in excluded]
    return result,excluded


def project_operational_reporting(rows,context):
    """Return exact global reporting images, named actions and source rows.

    ``reporting_source_map`` has one row for every image. Normal source rows
    add one image. The declared final reporting-loss macro adds two images.
    The caller must expand its core forest images at the same raw row boundary.
    """
    if not rows:fail(0,'Context','rows_empty')
    constants=context.get('reporting_constants',{})
    if constants.get('UseReporting') is not True:fail(0,'Context','layer_disabled')
    parents=constants['Parents']['$set']
    starts={};owners={};prefixes={parent:[] for parent in parents};first_facts={}
    states={parent:initial_chat(parent,context) for parent in parents}
    images=[];actions=[];source_map=[];provenance=[]
    for index,row in enumerate(rows):
        action=row.get('action','Unknown');current_parts=dict(entries(row['partitions']))
        routes=prefix_routes(row,index)
        observed_facts=facts(row)
        new_facts=[fact for scoped,fact in observed_facts.items() if scoped not in first_facts]
        require_source_authority(row,index,routes,new_facts,parents)
        for scoped in observed_facts:first_facts.setdefault(scoped,index)
        old_states=states;states={}
        prefix_provenance=[]
        for parent in parents:
            owner=routes.get(parent)
            created=owner in current_parts and parent in dict(entries(current_parts[owner]['core']['threads']))
            if not created:
                if parent in starts:fail(index,action,'owner_or_chat_disappeared')
                states[parent]=initial_chat(parent,context);continue
            if parent in owners and owners[parent]!=owner:fail(index,action,'owner_changed')
            owners[parent]=owner;starts.setdefault(parent,index)
            focused,excluded=eligible_row(row,owner,starts[parent],first_facts)
            prefixes[parent].append(focused)
            def stop(field):raise FullProjectionError(index,action,field)
            observed=project_native_reporting(prefixes[parent],context,parent,owner,stop)
            states[parent]=observed['reporting_state']
            prefix_provenance.append(dict(parent=parent,owner=owner,reporting_complete=observed['reporting_complete'],
                                          source=[dict(item,row=item.get('row',0)+starts[parent]) for item in observed['provenance']],
                                          excluded_catalog_before_creation=excluded))
        image=merge_chat_states(states,old_states,images[-1] if images else None,index,action,context)
        raw_seq=context.get('source_map',[{}]*len(rows))[index].get('raw_seq',index)
        if index==0:
            images.append(image);source_map.append(dict(raw_row=0,raw_seq=raw_seq,macro_substep=0,substep=0))
        else:
            expanded=reporting_actions(rows[index-1],dict(row,raw_row=index,raw_seq=raw_seq),context,images[-1],image)
            images.extend(expanded['images']);actions.extend(expanded['actions']);source_map.extend(expanded['source_map'])
        provenance.append(dict(raw_row=index,raw_seq=raw_seq,parents=prefix_provenance))
    return dict(reporting_images=images,reporting_actions=actions,reporting_source_map=source_map,
                reporting_provenance=provenance,
                reporting_scope='captured global prefixes; source result streams; provider process life remains unknown')


project_reporting=project_operational_reporting
