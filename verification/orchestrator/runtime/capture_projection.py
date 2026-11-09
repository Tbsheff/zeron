"""Project observed transaction frames. This does not infer I/O fact truth."""
import json
from check_seams import fn


def canon(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True)


def unique(pairs, name):
    pairs = list(pairs)
    if len({str(k) for k, _ in pairs}) != len(pairs):
        raise ValueError('duplicate ' + name)
    return fn(pairs)


def project_capture(row, command_receipts):
    snap = row['snapshot']
    rc = []
    for r in snap['receipts']:
        result = r['result']
        if r['seq'] != result['seq']:
            raise ValueError('receipt sequence differs from result sequence')
        rc.append((r['key'], dict(seq=r['seq'], accepted=result['ok'] == 'accepted',
            effects=result.get('effects', []), reason=canon(result.get('reason')))))
    outbox = [(r['effect_id'], dict(seq=r['seq'], idx=r['idx'], effect=canon(r['effect']),
                                  done=r['done'])) for r in snap['outbox']]
    facts = [(r['id'], dict(kind=r['kind'], data=canon(r['data']))) for r in snap['facts']]
    return dict(raw_core=canon(snap['state']), receipts=unique(rc, 'receipt'),
        outbox=unique(outbox, 'outbox id'), processed={'$set': snap['processed']},
        facts=unique(facts, 'fact id'), command_receipts=fn(sorted(command_receipts.items())),
        command_decisions=unique((([x['command_id'],x['phase']],canon(x['receipt']))
             for x in snap.get('command_receipts',[])), 'command decision'))


def project_frames(rows):
    commands = {}
    result = []
    dev = rows[0]['dev']
    seq = rows[0]['seq']
    for i, row in enumerate(rows):
        if row['v'] != 2 or row['dev'] != dev or row['seq'] != seq + i:
            raise ValueError('requires complete contiguous per-device v2 capture')
        args = {k:('none' if v is None else v) for k,v in row['args'].items() if k != 'event'}
        if row['action'] == 'CommandDecision':
            args['observed']=canon(row['args']['observed'])
            args['receipt']=canon(row['args']['receipt'])
        if row['action'] == 'Apply':
            command = row['args'].get('command_id')
            args['command_id'] = command if command is not None else 'none'
            if not args['duplicate'] and command is not None:
                commands[command] = args['key']
        result.append(dict(action=row['action'], args=args or {'baseline': True},
                           state=project_capture(row, commands)))
    return result
