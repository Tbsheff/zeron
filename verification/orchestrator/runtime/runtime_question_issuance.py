"""Check raw R50 question issuance before any full-image map.

Use raw device-local thread bindings. The generic typed fact map does not turn
question_input_route.child into a stable chat ID. This check reads only the
current raw prefix and returns evidence, never a model successor or hidden
provider turn identity. It does not replace the exact core/transaction check.
"""
from copy import deepcopy


class QuestionIssuanceError(ValueError):
    def __init__(self, row, field):
        self.row, self.action, self.field = row, 'QuestionIssuance', field
        super().__init__(f'row={row} action=QuestionIssuance field={field}')


def question_issuance(rows):
    """Join accepted nonduplicate raw Raise to prior route and issued facts.

    rows must retain the causal input order and each full raw snapshot. Facts
    are scoped by device. Missing capture writes stay unknown. A duplicate or
    rejected input contributes no new accepted question or issuance claim.
    """
    previous, seen, first_seen, result = {}, {}, {}, []
    for index, row in enumerate(rows):
        def fail(field):
            raise QuestionIssuanceError(index, 'question_issuance.' + field)

        dev = row['dev']
        snapshot = row['snapshot']
        facts = {}
        for fact in snapshot['facts']:
            identity = fact['id']
            if identity in facts:
                fail('fact_duplicate')
            facts[identity] = fact
            scoped = (dev, identity)
            if scoped in seen and seen[scoped] != fact:
                fail('fact_changed')
            first_seen.setdefault(scoped, index)
            seen[scoped] = deepcopy(fact)
        if any(device == dev and identity not in facts for device, identity in seen):
            fail('fact_disappeared')

        args = row.get('args', {})
        event = args.get('event', {})
        is_raise = event.get('act') in {'Raise', 'RaiseQuestion'}
        if row['action'] != 'Apply' or not is_raise or args.get('accepted') is not True or args.get('duplicate') is not False:
            previous[dev] = deepcopy(snapshot)
            continue
        if dev not in previous:
            fail('before_missing')

        child, qid = event['args']['child'], event['args']['qid']
        receipts = [item for item in snapshot['receipts'] if item['key'] == args['key']]
        if len(receipts) != 1 or receipts[0]['result'].get('ok') != 'accepted':
            fail('accepted_receipt')
        routes = [(identity, fact['data']) for identity, fact in facts.items()
                  if fact['kind'] == 'question_input_route'
                  and fact['data'].get('receipt') == args['key']
                  and fact['data'].get('child') == child
                  and fact['data'].get('qid') == qid]
        if len(routes) != 1:
            fail('route_identity')
        route_id, route = routes[0]
        request = route.get('request_id')
        run, attempt = route.get('source_run'), route.get('source_attempt')
        if not isinstance(request, str) or not request:
            fail('request_unknown')
        if type(run) is not int or run <= 0 or type(attempt) is not int or attempt <= 0:
            fail('attempt_unknown')

        bindings = [binding['chat'] for binding in snapshot['threads'] if binding['local_thread'] == child]
        if len(bindings) != 1:
            fail('child_binding')
        chat = bindings[0]
        issued = [(identity, fact['data']) for identity, fact in facts.items()
                  if fact['kind'] == 'question_input_issued'
                  and fact['data'].get('request_id') == request
                  and fact['data'].get('chat') == chat
                  and fact['data'].get('source_run') == run
                  and fact['data'].get('source_attempt') == attempt]
        if len(issued) != 1:
            fail('issued_identity')
        issue_id, issue = issued[0]
        if not first_seen[(dev, issue_id)] < first_seen[(dev, route_id)] < index:
            fail('source_order')
        if not isinstance(issue.get('runtime_id'), str) or not issue['runtime_id']:
            fail('runtime_unknown')
        if not isinstance(issue.get('source_message_id'), str) or not issue['source_message_id']:
            fail('origin_unknown')

        threads = [thread for key, thread in previous[dev]['state']['threads'] if key == child]
        if len(threads) != 1:
            fail('before_child')
        thread = threads[0]
        runs = thread['runs']
        if not runs or runs[-1]['status'] != 'Live':
            fail('before_live')
        if thread['run_base'] + len(runs) != run or thread['attempt']['n'] != attempt:
            fail('current_attempt')

        result.append(dict(device=dev, child=chat, qid=str(qid), request_id=request,
                           runtime_id=issue['runtime_id'], run=run, attempt=attempt,
                           origin_id=issue['source_message_id'], raw_row=index,
                           route_fact=route_id, issued_fact=issue_id,
                           receipt=args['key']))
        previous[dev] = deepcopy(snapshot)
    return result
