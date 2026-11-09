"""Normalize native/question events and derive ordered keys from prior state.

Order comes from prior raw keys plus the command input, never from post-state.
IDs stay opaque decimal strings; only Python compares raw u64 IDs for order.
"""
from copy import deepcopy
from remote_projection import message

ALIASES = {'Raise': 'RaiseQuestion', 'ImportUp': 'ImportQuestion'}
NATIVE = {'Start': 'NativeStart', 'StartNested': 'NativeStartNested', 'End': 'NativeEnd', 'Claim': 'NativeClaim',
          'Delivered': 'NativeDelivered', 'Recover': 'NativeRecover'}

def project_native_question_event(raw, tid, before_snapshot):
    out = deepcopy(raw)
    a = out.get('args', {})
    kind = out['act']
    if kind in NATIVE and 'key' in a and 'parent' in a:
        out['act'] = NATIVE[kind]
    elif kind == 'Start' and set(a) == {'t'}:
        out['act'] = 'StartQuestion'
    elif kind == 'Deliver' and 'message' in a and next(iter(a['message'])) in {
            'QuestionUp', 'AnswerDown', 'QuestionRelease', 'QuestionClosed'}:
        out['act'] = 'DeliverQuestion'
    else:
        out['act'] = ALIASES.get(kind, kind)
    question_keys = [tuple(k) for k, _ in before_snapshot['state']['questions']]
    native_keys = [tuple(k) for k, _ in before_snapshot['state']['natives']]
    for keys in (question_keys, native_keys):
        if keys != sorted(set(keys)):
            raise ValueError('noncanonical native/question key order')
    if out['act'] in {'RaiseQuestion', 'ImportQuestion'}:
        k = (a['child'], a['qid'])
        if k not in question_keys:
            question_keys.append(k)
    if out['act'] in {'NativeStart', 'NativeStartNested'}:
        k = (a['parent'], a['key'])
        if k not in native_keys:
            native_keys.append(k)
    out['question_order'] = [[tid(c), str(q)] for c,q in sorted(question_keys)]
    out['native_order'] = [[tid(c), str(q)] for c,q in sorted(native_keys)]
    for name in ('t', 'parent', 'child', 'to', 'via', 'agent'):
        if name in a:
            a[name] = 'none' if a[name] is None else tid(a[name])
    for name in ('key', 'qid', 'answer') + (('ancestor',) if out['act']=='NativeStartNested' else ()):
        if name in a:
            a[name] = str(a[name])
    if 'message' in a:
        a['message'] = message(a['message'], tid)
    out['args'] = a
    return out
