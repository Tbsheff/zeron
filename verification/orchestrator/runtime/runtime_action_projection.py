"""Name source actions from captured causes; never search a target successor."""
from copy import deepcopy
from runtime_full_projection import FullProjectionError, entries, fn, enum, event


SIMPLE = {
    'Steer':'C_Steer', 'Stop':'C_Stop', 'InterruptChat':'C_InterruptChat',
    'ServerEnqueue':'C_ServerEnqueue', 'ChildStart':'C_ChildStart',
    'ChildEnd':'C_ChildEnd', 'ResetPasses':'C_ResetPasses',
    'StartFromQueue':'C_StartQueue', 'StartFromQueueWake':'C_StartWake',
    'UsageLimitRecovery':'C_ULRecovery', 'WaitTimeout':'C_WaitTimeout',
    'WaitAbandon':'C_WaitAbandon', 'LateWaitAck':'C_LateWaitAck',
}
INGEST = {'IngestEnd':'C_IngestEnd','RetryAttempt':'C_RetryAttempt',
          'IngestChild':'C_IngestChild','IngestStale':'C_IngestStale'}


def known(value):
    if isinstance(value,dict) and 'present' in value:
        return value.get('value') if value['present'] else None
    return value


def field(image,name,key):
    return dict((tuple(k) if isinstance(k,list) else k,v) for k,v in entries(image[name]))[key]


def source_actions(before_row,row,context,before_image,after_image):
    """Return one exact named action per active forest coordinate.

    Equality permits only Stutter. Changed images require an observed cause.
    A returned label still MUST pass the independent target action relation.
    Macro steps without an observed decomposition fail rather than invent images.
    """
    def fail(name):
        raise FullProjectionError(row.get('seq',0),row.get('action','Unknown'),'named_action.'+name)
    old={tuple(k):v for k,v in entries(before_image)}
    new={tuple(k):v for k,v in entries(after_image)}
    if not old.keys()<=new.keys():fail('root_removed')
    output=[]
    for index,image in new.items():
        root,focus=index
        if index not in old:
            if any(k[0]==root for k in old):fail('focus_added')
            ev=row.get('event',{})
            if row.get('action')!='Apply' or not row.get('args',{}).get('accepted') or row['args'].get('duplicate') or ev.get('act')!='OpenRoot' or ev.get('args',{}).get('t')!=root:
                fail('registration_cause')
            output.append((list(index),dict(label='RegisterRoot',args={})));continue
        previous=old[index]
        if previous==image:
            output.append((list(index),dict(label='Stutter',args={})));continue
        label=None;args={}
        if row['action']=='Apply':
            flags=row['args']
            if flags.get('duplicate') or not flags.get('accepted'):fail('rejected_or_duplicate_changed')
            ev=row.get('event',{});act=ev.get('act');a=ev.get('args',{})
            t=a.get('t',a.get('parent',a.get('p',a.get('child',a.get('c')))))
            if act!='CrashEngine' and t not in context['tree_threads'][root]:fail('unrelated_tree_changed')
            if act in SIMPLE:label=SIMPLE[act];args=dict(t=t)
            elif act in {'PersonSend','RestartActive'}:
                mode=enum(a['mode'])
                if mode not in {'now','queue','steer','restart'}:fail('mode')
                label='C_PersonSend' if act=='PersonSend' else 'C_Restart';args=dict(t=t,m=mode)
            elif act=='ProviderTerminal':label='C_ProviderTerminal';args=dict(t=t,o=enum(a['outcome']))
            elif act in INGEST:label=INGEST[act];args=dict(t=t,e=event(a['ev']))
            elif act=='Delegate':label='C_Delegate';args=dict(t=t,c=a['child'],m=enum(a['mode']),a=a['access'])
            elif act=='Followup':
                if a['access']!=field(previous,'access',a['child']):fail('macro.followup_access')
                label='C_FollowUp';args=dict(t=t,c=a['child'],m='async')
            elif act in {'ReadResult','ReadTaskResult'}:label='C_ReadResult';args=dict(p=a['p'],c=a['c'])
            elif act=='WaitReturn':
                child=field(previous,'waitOn',t)
                pending=field(previous,'qPend',child) if child in dict(entries(previous['qPend'])) else False
                holder=field(previous,'qAt',child) if pending else None
                label='C_WaitReturnQuestion' if pending and holder==t else 'C_WaitReturn';args=dict(t=t)
            elif act=='NarrowAccess':
                if a['access']!=field(previous,'access',t)-1:fail('macro.narrow_levels')
                if previous['att']!=image['att']:fail('macro.narrow_restart')
                label='C_Narrow';args=dict(t=t)
            elif act in {'DoNotice','DoOffer','DoStop'}:
                kind={'DoNotice':'notice','DoOffer':'offer','DoStop':'stop'}[act]
                message=dict(k=kind,t=t,r=a.get('stop',a.get('run')),g=a.get('generation',0))
                if a['how']=='Keep':fail('macro.delivery_without_ack')
                how='ack' if a['how']=='Ack' else None
                if how is None:fail('delivery_mode')
                label='C_'+act;args=dict(e=message,how=how)
            elif act=='CrashEngine':label='C_CrashEngine'
            else:fail('unmapped_event')
        elif row['action']=='Observe':
            facts=dict(entries(row['mechanism']['facts']))
            fact=facts.get(row['args'].get('fact_id'))
            if fact is None:fail('observation_missing')
            data=fact['data'];chat=data.get('chat')
            if fact['kind']=='runtime_binding' and data.get('operation') in {'install','config','rebind','issue'}:
                if data.get('validated') is not True or chat not in context['tree_threads'][root]:fail('access_identity')
                access=known(data.get('effective_access'))
                if type(access) is not int or access not in range(3):fail('access_value')
                label='C_ObserveAccess';args=dict(t=chat,a=access)
            else:fail('macro.observation')
        else:fail('macro.mechanism')
        output.append((list(index),dict(label=label,args=deepcopy(args))))
    return fn(output)
