"""Lossless core projection for runtime seams; no reducer calls or posthash lookup.

All raw OrchState fields are retained. Local ThreadId values map only through
captured thread bindings. Opaque u64 IDs use decimal strings; ordinals/epochs
must fit the exact signed-TLC tranche. Placement evidence stays separate.
"""
from copy import deepcopy

CAP = 2147483647
REMOTE_ACTIONS = {'RemoteImport', 'RemoteFollowup', 'RemoteNotice', 'RemoteSent',
                  'RemoteStop', 'RemoteFence', 'RemoteClamp', 'RemoteCancel'}

def fn(pairs):
    return {'$fn': list(pairs)}

def aset(values):
    return {'$set': list(values)}

def counter(value):
    if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value < CAP:
        raise ValueError(f'counter outside exact TLC tranche: {value}')
    return value

def map_keys(pairs, name):
    keys = [x[0] for x in pairs]
    if keys != sorted(keys) or any(a == b for a,b in zip(keys,keys[1:])):
        raise ValueError(f'noncanonical {name} map')

def message(raw, tid):
    if isinstance(raw, str):
        return {'kind': raw}
    if not isinstance(raw, dict) or len(raw) != 1:
        raise ValueError('invalid tagged message')
    kind, data = next(iter(raw.items()))
    if kind in ('Person', 'Server'):
        return {'kind': kind, 'value': str(data)}
    if kind == 'Queued':
        return {'kind': kind, 'message': message(data, tid)}
    out = {'kind': kind, **deepcopy(data)}
    for key in ('child', 'parent', 'via', 'to', 'viewer', 't'):
        if key in out:
            out[key] = 'none' if out[key] is None else tid(out[key])
    for key in ('qid', 'answer', 'key'):
        if key in out:
            out[key] = str(out[key])
    for key in ('run', 'stop', 'epoch', 'generation'):
        if key in out:
            counter(out[key])
    return out

def run_id(raw, tid):
    return {'thread': tid(raw['thread']), 'ord': counter(raw['ord'])}

def effect(raw, tid):
    kind, data = next(iter(raw.items()))
    if kind == 'Send':
        return {'kind': kind, 'message': message(data, tid)}
    out = {'kind': kind, **deepcopy(data)}
    for key in ('t', 'parent', 'child', 'viewer'):
        if key in out:
            out[key] = tid(out[key])
    if 'run' in out and isinstance(out['run'], dict):
        out['run'] = run_id(out['run'], tid)
    if 'attempt' in out:
        out['attempt']['run'] = run_id(out['attempt']['run'], tid)
    for key in ('qid', 'answer', 'key', 'msg'):
        if key in out:
            out[key] = 'none' if out[key] is None else str(out[key])
    if 'start' in out:
        out['start'] = message(out['start'], tid)
    if 'task' in out:
        out['task']['child'] = tid(out['task']['child'])
    if 'delivered' in out:
        out['delivered'] = [dict(child=tid(x['child']), run=counter(x['run'])) for x in out['delivered']]
    return out

def project_remote_event(raw, tid, effects=None, before_snapshot=None):
    out = deepcopy(raw)
    args = out.get('args', {})
    for key in ('t', 'parent', 'child', 'ancestor'):
        if key in args:
            args[key] = 'none' if args[key] is None else tid(args[key])
    if 'message' in args:
        args['message'] = message(args['message'], tid)
    if out.get('act') == 'RemoteImport':
        if before_snapshot is None:
            raise ValueError('RemoteImport requires before snapshot for sorted local order')
        ids = {key for key, _ in before_snapshot['state']['threads']}
        ids.update(raw['args'][k] for k in ('parent','child','ancestor') if raw['args'][k] is not None)
        args['thread_order'] = [tid(key) for key in sorted(ids)]
    for key in ('run','cohort','stop','depth','access'):
        if key in args:
            counter(args[key])
    out['args'] = args
    if effects is not None:
        out['effects'] = [effect(x.get('effect', x), tid) for x in effects]
    return out

def project_remote(snapshot, tid):
    raw = snapshot['state']
    expected = {'config','threads','outbox','stop_receipts','stops','questions','question_receipts','natives'}
    if set(raw) != expected:
        raise ValueError(f'unsupported OrchState fields: {set(raw) ^ expected}')
    for name in ('threads','questions','natives'):
        map_keys(raw[name],name)
    threads = []
    for key, source in raw['threads']:
        if source['id'] != key:
            raise ValueError('thread map key mismatch')
        th = deepcopy(source)
        for name in ('id', 'parent', 'wait_on', 'late_wait'):
            th[name] = 'none' if th[name] is None else tid(th[name])
        th['queue'] = [message(x, tid) for x in th['queue']]
        th['inbox'] = aset(th['inbox'])
        th['task'] = {'present': False} if th['task'] is None else {'present': True, **th['task']}
        th['ul']['armed'] = 0 if th['ul']['armed'] is None else counter(th['ul']['armed'])
        counter(th['run_base']); counter(th['run_base'] + len(th['runs']))
        counter(th['depth']); counter(th['access'])
        if th['task']['present']:
            for name in ('cohort','run','pushes'):
                counter(th['task'][name])
        for name in ('n', 'retries', 'steers'):
            counter(th['attempt'][name])
        for run in th['runs']:
            run['start'] = message(run['start'], tid)
            run['cohort']['del'] = aset(tid(x) for x in run['cohort']['del'])
            counter(run['cohort']['generation'])
        threads.append((tid(key), th))
    questions = []
    for (child, qid), source in raw['questions']:
        q = deepcopy(source)
        if source['child'] != child or source['qid'] != qid:
            raise ValueError('question key mismatch')
        q['child'], q['qid'] = tid(child), str(qid)
        q['holder'] = 'none' if q['holder'] is None else tid(q['holder'])
        for name in ('views', 'waiting', 'seen'):
            q[name] = aset(tid(x) for x in q[name])
        map_keys(q['future_answers'],'future_answers')
        q['future_answers'] = fn((counter(k), str(v)) for k,v in q['future_answers'])
        counter(q['epoch'])
        questions.append(([tid(child), str(qid)], q))
    natives = []
    for (parent, key), source in raw['natives']:
        n = deepcopy(source)
        ancestor=n.get('ancestor')
        if ancestor is not None and (type(ancestor) is not int or not 0<=ancestor<2**64):
            raise ValueError('invalid opaque native ancestor key')
        n['ancestor']='none' if ancestor is None else str(ancestor)
        if n['attempt']['run']['thread'] != parent:
            raise ValueError('native owner mismatch')
        n['attempt']['run'] = run_id(n['attempt']['run'], tid)
        counter(n['attempt']['n']); counter(n['generation'])
        natives.append(([tid(parent), str(key)], n))
    for value in raw['config'].values():
        counter(value)
    return dict(config=deepcopy(raw['config']), threads=fn(threads),
        thread_order=[tid(key) for key, _ in raw['threads']],
        outbox=aset(message(x,tid) for x in raw['outbox']),
        stop_receipts=aset([tid(c),counter(e)] for c,e in raw['stop_receipts']),
        stops=counter(raw['stops']), questions=fn(questions),
        question_receipts=aset(message(x,tid) for x in raw['question_receipts']), natives=fn(natives),
        native_order=[[tid(parent),str(key)] for (parent,key),_ in raw['natives']],
        question_order=[[tid(child),str(qid)] for (child,qid),_ in raw['questions']])
