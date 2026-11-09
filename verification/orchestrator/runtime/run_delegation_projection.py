"""Project run, delegation and outbox events without successor-state input."""
from copy import deepcopy
from remote_projection import counter

RUN_DELEGATION_ACTIONS = {
    'PersonSend','Steer','RestartActive','RequestInterrupt','ServerEnqueue',
    'ProviderTerminal','ChildStart','ChildEnd','ResetPasses','StartFromQueue',
    'UsageLimitRecovery','IngestEnd','RetryAttempt','IngestChild','IngestStale',
    'OpenRoot','Stop','NarrowAccess','InterruptChat','Delegate','Followup',
    'ReadTaskResult','ReadResult','WaitReturn','WaitTimeout','WaitAbandon',
    'LateWaitAck','StartFromQueueWake','DoNotice','DoOffer','DoStop','CrashEngine',
}

def project_run_delegation_event(raw, tid, before_snapshot):
    if raw['act'] not in RUN_DELEGATION_ACTIONS:
        raise ValueError(f'unsupported run/delegation action {raw["act"]}')
    out = deepcopy(raw)
    args = out.setdefault('args', {})
    for key in ('t','parent','child','p','c'):
        if key in args:
            args[key] = tid(args[key])
    if 'msg' in args:
        args['msg'] = str(args['msg'])
    for key in ('run','attempt','task_run','generation','stop','access'):
        if key in args:
            counter(args[key])
    if 'ev' in args:
        counter(args['ev']['run']); counter(args['ev']['attempt'])
    keys = [key for key, _ in before_snapshot['state']['threads']]
    if keys != sorted(set(keys)):
        raise ValueError('thread keys lack unique numeric source order')
    new = raw.get('args', {})
    if raw['act'] == 'OpenRoot':
        keys = sorted(set(keys) | {new['t']})
    elif raw['act'] == 'Delegate':
        keys = sorted(set(keys) | {new['child']})
    out['thread_order'] = [tid(key) for key in keys]
    return out
