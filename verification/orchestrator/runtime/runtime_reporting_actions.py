"""Name R44 steps from captured facts and accepted engine inputs.

This adapter never calls a target relation or searches for a successor. Each
macro has fixed source evidence and explicit intermediate records. The target
checker must still validate each named step and the independent relation.
"""
from copy import deepcopy
import json

from native_reporting_projection import decoded, known, project_native_reporting
from runtime_full_projection import FullProjectionError, entries, fn, aset


def canonical(value):
    """Compare TLA functions and sets without their serialization order."""
    if isinstance(value,dict):
        if set(value)=={'$fn'}:
            return ('function',frozenset((canonical(k),canonical(v)) for k,v in entries(value)))
        if set(value)=={'$set'}:return ('set',frozenset(canonical(x) for x in value['$set']))
        return ('record',tuple(sorted((k,canonical(v)) for k,v in value.items())))
    if isinstance(value,list):return ('sequence',tuple(canonical(x) for x in value))
    return (type(value).__name__,value)


def equal(a,b):return canonical(a)==canonical(b)

def lookup(mapping,key):
    matches=[v for k,v in entries(mapping) if equal(k,key)]
    if len(matches)!=1:raise KeyError('source_key')
    return matches[0]


def facts(row):
    if row is None:return {}
    parts=entries(row['partitions']) if 'partitions' in row else [(row['dev'],row)]
    result={}
    for device,part in parts:
        mechanism=part['mechanism'];values=dict(entries(mechanism['facts']))
        for identity in mechanism['fact_order']:
            result[(device,identity)]=decoded(values[identity])
    return result


def added_facts(before,row):
    old=facts(before);new=facts(row)
    if any(key not in new or not equal(value,new[key]) for key,value in old.items()):
        raise ValueError('immutable_fact')
    return [fact for key,fact in new.items() if key not in old]


def _reporting_actions(before_row,row,context,before_reporting,after_reporting):
    """Return successor images, their exact source labels, and source rows.

    The result excludes the caller's initial image. A normal row returns one
    image/action. A final source-loss closure returns two explicit steps.
    """
    raw_row=row.get('raw_row',row.get('raw_seq',row.get('seq')))
    def stop(field):raise FullProjectionError(raw_row if type(raw_row) is int else 0,
                                             row.get('action','Unknown'),'reporting_action.'+field)
    def result(images,actions):
        source=[dict(raw_row=raw_row,macro_substep=i+1,substep=i+1,
                     **({'raw_seq':row['raw_seq']} if 'raw_seq' in row else {})) for i in range(len(images))]
        return dict(images=images,actions=actions,source_map=source)
    def one(label,**args):return result([deepcopy(after_reporting)],[dict(label=label,args=args)])
    if raw_row is None:stop('raw_row')
    if equal(before_reporting,after_reporting):return one('Stutter')
    try:added=added_facts(before_row,row)
    except (ValueError,KeyError):stop('immutable_fact')
    relevant=[f for f in added if f['kind'] in {'runtime_binding','provider_event',
                 'provider_stream_catalog','agent_admission','native_doc_commit'}]
    if len(relevant)>1:stop('multiple_facts')
    if relevant:
        fact=relevant[0];data=fact['data'];kind=fact['kind']
        if kind=='runtime_binding':
            if data.get('validated') is not True:stop('binding_validated')
            sid=data['runtime_id'];op=data['operation']
            args=dict(sid=sid,access=data['effective_access'],revision=data['source_revision'],
                      origin=data.get('origin_id') or context['reporting_constants']['NoOrigin'])
            if op=='install':return one('InstallHandle',p=data['chat'],**args)
            if op in {'rebind','config','issue'}:
                return one({'rebind':'RebindHandle','config':'ConfigHandle','issue':'IssueRequest'}[op],**args)
            if op in {'retire','remove'}:
                return one('RetireHandle',sid=sid,revision=data['source_revision'],remove=op=='remove')
            stop('binding_operation')
        if kind=='agent_admission':
            if data.get('validated') is not True:stop('admission_validated')
            if data['disposition']=='accepted':return one('Admit',sid=data['runtime_id'])
            if data['disposition']=='refused':return one('Refuse',p=data['chat'])
            stop('admission_disposition')
        if kind=='provider_stream_catalog':
            if data.get('complete') is not True:stop('catalog_incomplete')
            changed=[p for p,value in entries(after_reporting['catalogComplete'])
                     if value and not lookup(before_reporting['catalogComplete'],p)]
            if not changed:stop('catalog_delta')
            return one('ObserveCatalog',ps=aset(changed))
        if kind=='native_doc_commit':
            return one('PersistTask',k=[data['chat'],str(data['task_key'])],status=data['chip_status'])
        sid=data['provider_run_id'];event=data['event_kind']
        if event=='stream_opened':return one('ObserveStreamOpen',sid=sid)
        if event in {'session_started','root_started'}:return one('ObserveRootStart',sid=sid)
        if event in {'stream_closed','stream_crashed','session_closed','session_crashed'}:
            return one('ObserveStreamClose',sid=sid)
        if event=='root_terminal':return one('ObserveRootEnd',sid=sid,out=data['status'])
        if event in {'native_spawn','native_started','native_progress','native_terminal'}:
            path=data.get('native_path');chat=data['chat']
            path_key=json.dumps(path,separators=(',',':'))
            key=context.get('native_slots',{}).get(chat,{}).get(path_key)
            if key is None:stop('native_path_binding')
            nid=[chat,str(key)]
            if event=='native_spawn':
                native=lookup(after_reporting['natives'],nid)
                if native['activation']!=context['reporting_constants']['NoActivation']:
                    stop('spawn_activation_macro')
                return one('ObserveSpawn',sid=sid,k=nid,ancestor=native['ancestor'],
                           ownBackground=native['ownBackground'])
            if event=='native_terminal':return one('ObserveNativeEnd',k=nid,out=data['status'])
            old=lookup(before_reporting['natives'],nid);new=lookup(after_reporting['natives'],nid)
            activation=new['activation']
            if old['activation']==context['reporting_constants']['NoActivation'] and activation!=old['activation']:
                observer=data['observer'];natives=observer.get('natives',[])
                if isinstance(natives,dict):natives=[value for key,value in entries(natives)]
                observed=[n for n in natives if n['native_path']==path and n.get('activation_id')==activation]
                if data.get('activation_id')!=activation and not observed:stop('activation_evidence')
                return one('BindActivation',k=nid,activation=activation)
        stop('provider_event_macro')
    accepted=row.get('action')=='Apply' and row.get('args',{}).get('accepted') is True \
             and row.get('args',{}).get('duplicate') is False
    if not accepted:stop('missing_source')
    event=row['event'];act=event['act'];args=event['args']
    def pending(kind,key=None,run=None,attempt=None,source=None):
        found=[(i+1,e) for i,e in enumerate(before_reporting['pending']) if e['kind']==kind
               and (key is None or equal(e['key'],key)) and (run is None or e['run']==run)
               and (attempt is None or e['attempt']==attempt) and (source is None or e['source']==source)]
        if len(found)!=1:stop('pending_'+kind)
        return found[0]
    if act in {'StartFromQueue','StartFromQueueWake','StartQuestion','UsageLimitRecovery'}:
        p=args['t'];old=lookup(before_reporting['runs'],p);new=lookup(after_reporting['runs'],p)
        if not old['active'] and new['active'] and new['run']==old['run']+1:return one('LogicalStart',p=p)
        stop('logical_start_delta')
    if act in {'RetryAttempt','RestartActive','Steer','NarrowAccess'}:
        p=args['t']
        if act=='NarrowAccess':
            changed=[parent for parent,value in entries(after_reporting['runs'])
                     if not equal(value,lookup(before_reporting['runs'],parent))]
            if len(changed)!=1:stop('narrow_attempt_macro')
            p=changed[0]
        old=lookup(before_reporting['runs'],p);new=lookup(after_reporting['runs'],p)
        if old['active'] and new['active'] and new['run']==old['run'] and new['attempt']==old['attempt']+1:
            return one('LogicalAttempt',p=p)
        stop('logical_attempt_delta')
    if act in {'NativeStart','NativeStartNested'}:
        key=[args['parent'],str(args['key'])];i,spawn=pending('spawn',key)
        tasks=after_reporting['tasks']
        inserted=any(equal(k,key) for k,v in entries(tasks))
        return one('Handle' if inserted else 'RejectSpawn',i=i)
    if act=='NativeEnd':
        key=[args['parent'],str(args['key'])]
        terminal=[(i+1,e) for i,e in enumerate(before_reporting['pending']) if e['kind']=='nativeEnd' and equal(e['key'],key)]
        if terminal:
            if len(terminal)!=1:stop('pending_nativeEnd')
            return one('Handle',i=terminal[0][0])
        if not args['failed']:stop('loss_outcome')
        task=lookup(before_reporting['tasks'],key);i,loss=pending('sourceLoss',source=task['source'])
        retained=any(equal(e,loss) for e in after_reporting['pending'])
        if retained:return one('HandleLossNative',i=i,k=key)
        expected=deepcopy(before_reporting['pending']);expected.pop(i-1)
        if not equal(expected,after_reporting['pending']):stop('loss_pending_delta')
                                                                              
                                                                                
        middle=deepcopy(after_reporting);middle['pending']=deepcopy(before_reporting['pending'])
        middle['lastCause']=dict(kind='sourceLoss',source=loss['source'],changed=aset([key]))
        return result([middle,deepcopy(after_reporting)],
                      [dict(label='HandleLossNative',args=dict(i=i,k=key)),
                       dict(label='Handle',args=dict(i=i))])
    if act=='IngestEnd':
        terminal=args['ev'];i,e=pending('rootEnd',run=terminal['run'],attempt=terminal['attempt'])
        remaining=deepcopy(before_reporting['pending']);remaining.pop(i-1)
        if equal(remaining,after_reporting['pending']):return one('Handle',i=i)
                                                                          
                                                                                
        losses=[(j+1,loss) for j,loss in enumerate(remaining) if loss['kind']=='sourceLoss'
                and not any(equal(loss,kept) for kept in after_reporting['pending'])]
        if len(losses)!=1:stop('root_loss_pending_delta')
        j,loss=losses[0]
        if loss['source']!=e['source']:stop('root_loss_source')
        if any(old['source']==loss['source'] for old in remaining[:j-1]):stop('root_loss_lane')
        if any(task['source']==loss['source'] and task['status']=='running'
               for key,task in entries(after_reporting['tasks'])):stop('root_loss_running_task')
        final_pending=deepcopy(remaining);final_pending.pop(j-1)
        if not equal(final_pending,after_reporting['pending']):stop('root_loss_pending_delta')
        final_cause=dict(kind='sourceLoss',source=loss['source'],changed=aset([]))
        if not equal(final_cause,after_reporting['lastCause']):stop('root_loss_cause')
        orphaned=[key for key,task in entries(after_reporting['tasks'])
                  if task['source']==e['source'] and task['cause']=='orphan'
                  and lookup(before_reporting['tasks'],key)['status']=='running']
        middle=deepcopy(after_reporting);middle['pending']=remaining
        middle['lastCause']=dict(kind='rootEnd',source=e['source'],changed=aset(orphaned))
        return result([middle,deepcopy(after_reporting)],
                      [dict(label='Handle',args=dict(i=i)),dict(label='Handle',args=dict(i=j))])
    if act in {'NativeRecover','NativeDelivered'}:
        key=[args['parent'],str(args['key'])];task=lookup(after_reporting['tasks'],key)
        return one('RecordDeliveryGeneration',k=key,generation=task['generation'])
    if act=='ProviderTerminal':
        fresh=[e for e in after_reporting['pending'] if not any(equal(e,old) for old in before_reporting['pending'])]
        if len(fresh)!=1 or fresh[0]['kind']!='rootEnd':stop('engine_eof_terminal')
        e=fresh[0];source=lookup(before_reporting['sources'],e['source'])
        if source['reporting']!='disconnected' or source['parent']!=args['t'] \
           or (e['run'],e['attempt'])!=(args['run'],args['attempt']):stop('engine_eof_evidence')
        return one('EngineEOFRootTerminal',sid=e['source'],out=e['out'])
    stop('unsupported_'+act)


def reporting_actions(before_row,row,context,before_reporting,after_reporting):
    """Name one source row; malformed or unsupported inputs fail safely."""
    try:return _reporting_actions(before_row,row,context,before_reporting,after_reporting)
    except FullProjectionError:raise
    except (KeyError,TypeError,ValueError,IndexError):
        index=row.get('raw_row',row.get('raw_seq',row.get('seq',0)))
        raise FullProjectionError(index,row.get('action','Unknown'),'reporting_action.source_schema') from None


def expand_reporting(rows,context):
    """Project and expand a single-parent reporting path.

    The full mapper owns multi-parent merging and must call reporting_actions
    with its independently merged images. This helper refuses that scope.
    """
    parents=context['reporting_constants']['Parents']['$set']
    if len(parents)!=1:raise FullProjectionError(0,'Context','reporting_action.multi_parent_projection')
    focus=parents[0]
    parts=dict(entries(rows[-1]['partitions']))
    owners={binding['host'] for part in parts.values() for chat,binding in entries(part['mechanism']['bindings'])
            if chat==focus and binding['host_known']}
    if len(owners)!=1:raise FullProjectionError(0,'Context','reporting_action.owner')
    owner=next(iter(owners));images=[];actions=[];source_map=[]
    for index,row in enumerate(rows):
        def fail(field):raise FullProjectionError(index,row.get('action','Unknown'),field)
        projected=project_native_reporting(rows[:index+1],context,focus,owner,fail)['reporting_state']
        source=dict(raw_row=index,raw_seq=context.get('source_map',[{}]*len(rows))[index].get('raw_seq',index))
        if index==0:
            images.append(projected);source_map.append(dict(source,macro_substep=0,substep=0));continue
        expanded=reporting_actions(rows[index-1],dict(row,**source),context,images[-1],projected)
        images.extend(expanded['images']);actions.extend(expanded['actions']);source_map.extend(expanded['source_map'])
    return dict(images=images,actions=actions,source_map=source_map)
