"""Join complete raw core, transaction frames and typed I/O observations.

This projection never calls step(), never reads posthash, and never uses a
successor to choose an expected state. Actual successor bindings only resolve
stable IDs; derived ordering uses prior local keys plus explicit event inputs.
"""
from copy import deepcopy
from capture_projection import project_frames
from check_remote_seams import bindings
from remote_projection import REMOTE_ACTIONS, project_remote, project_remote_event, effect, fn, counter
from native_question_projection import project_native_question_event
from run_delegation_projection import RUN_DELEGATION_ACTIONS, project_run_delegation_event
from transfer_projection import project_transfer, transfer_event

NQ_ACTIONS = {'NativeStart','NativeStartNested','NativeEnd','NativeClaim','NativeDelivered','NativeRecover',
    'RaiseQuestion','ImportQuestion','ImportAnswer','ImportClosed','ImportRelease',
    'AgentAnswer','PersonAnswer','StartQuestion','DeliverQuestion'}
MAP_FACT_FIELDS={'natives','ledger','committed','receipts','runs','failures','providers','version'}
OPTIONAL_FACT_FIELDS = {'effective_access','access_ceiling','configured_access','submitted_access','provider_return_id'}


def typed(value, key=None):
    if key in OPTIONAL_FACT_FIELDS:
        return dict(present=False) if value is None else dict(present=True, value=typed(value))
    if value is None:
        return dict(present=False)
    if isinstance(value, bool) or isinstance(value, str):
        return value
    if isinstance(value, int):
        if key in {'qid','answer','key','task_key','msg'}:
            return str(value)
        return counter(value)
    if isinstance(value, list):
        return [typed(x) for x in value]
    if isinstance(value, dict):
        if key in MAP_FACT_FIELDS:
            return fn((k,typed(v)) for k,v in value.items())
        if all(k.isidentifier() for k in value):
            return {k:typed(v,k) for k,v in value.items()}
        return fn((k,typed(v)) for k,v in value.items())
    raise ValueError('unsupported typed fact value')


def mechanism(snapshot, fact_order, tid):
    facts = snapshot['facts']
    found = {x['id'] for x in facts}
    if len(found) != len(facts):
        raise ValueError('duplicate immutable fact id')
    if not set(fact_order) <= found:
        raise ValueError('captured facts disappeared')
    fact_order.extend(x['id'] for x in facts if x['id'] not in fact_order)
    bs = []
    for b in snapshot['threads']:
        known = b.get('host_known', False)
        if known and (not b.get('host') or b.get('shadow') is None):
            raise ValueError('incomplete known host binding')
        bs.append((b['chat'],dict(host=b.get('host') or 'none', host_known=known,
                                   shadow=b['shadow'] if b.get('shadow') is not None else 'none')))
    return dict(facts=fn((x['id'],dict(kind=x['kind'],data=typed(x['data']))) for x in facts),
                fact_order=list(fact_order), bindings=fn(bs),
                effects=fn((x['effect_id'],dict(effect=effect(x['effect'],tid),done=x['done']))
                           for x in snapshot['outbox']))


def project(rows):
    frames = project_frames(rows)
    out = []
    fact_order = []
    previous_bindings = {}
    for i,row in enumerate(rows):
        snapshot = row['snapshot']
        tid = bindings(snapshot)
        for binding in snapshot['threads']:
            old = previous_bindings.setdefault(binding['local_thread'], binding['chat'])
            if old != binding['chat']:
                raise ValueError('local thread rebound to another chat')
        core = project_remote(snapshot,tid)
        family = 'none'
        ev = dict(act='NoCoreAction',args={'unused':True})
        fx = []
        reject_reason='none'
        if row['action'] == 'Apply':
            if not i:
                raise ValueError('Apply lacks observed previous state')
            raw = row['args']['event']
            before = rows[i-1]['snapshot']
            if raw['act'] in REMOTE_ACTIONS:
                family='remote';ev=project_remote_event(raw,tid,before_snapshot=before)
            elif raw['act'] in RUN_DELEGATION_ACTIONS:
                family='run';ev=project_run_delegation_event(raw,tid,before)
            else:
                ev=project_native_question_event(raw,tid,before)
                if ev['act'] not in NQ_ACTIONS:
                    raise ValueError(f'unsupported core event {ev["act"]}')
                family='native_question'
            rc = [r for r in snapshot['receipts'] if r['key']==row['args']['key']]
            if len(rc) != 1:
                raise ValueError('Apply has no unique actual receipt')
            if not row['args']['accepted']:
                reject_reason=rc[0]['result']['reason']
            if row['args']['accepted'] and not row['args']['duplicate']:
                ids=rc[0]['result']['effects']
                byid={x['effect_id']:x for x in snapshot['outbox']}
                effects=[byid[key] for key in ids]
                if [x['idx'] for x in effects] != list(range(len(effects))):
                    raise ValueError('effect sequence differs from receipt order')
                fx=[effect(x['effect'],tid) for x in effects]
        transfer_known=any(x['kind'] in {'transfer_state','transfer_operation','transfer_catalog'} for x in snapshot['facts'])
        transfer=project_transfer(snapshot) if transfer_known else fn([])
        out.append(dict(dev=row['dev'],action=row['action'],args=frames[i]['args'],
            tx=frames[i]['state'],core=core,family=family,event=ev,fx=fx,reject_reason=reject_reason,
            mechanism=mechanism(snapshot,fact_order,tid),transfer=transfer,
            transfer_known=transfer_known,transfer_event=transfer_event(row)))
    return out
