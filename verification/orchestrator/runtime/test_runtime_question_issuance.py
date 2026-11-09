from copy import deepcopy
import unittest

from runtime_question_issuance import QuestionIssuanceError, question_issuance


def path():
    issue = dict(id='issued', kind='question_input_issued', data=dict(
        chat='child', runtime_id='runtime', request_id='request', source_run=1,
        source_attempt=1, source_message_id='message'))
    route = dict(id='route', kind='question_input_route', data=dict(
        child=7, qid=9, request_id='request', receipt='receipt', source_run=1,
        source_attempt=1))
    snapshot = dict(facts=[], receipts=[], threads=[dict(local_thread=7, chat='child')],
                    state=dict(threads=[[7, dict(run_base=0, runs=[dict(status='Live')],
                                                attempt=dict(n=1))]]))
    rows = [dict(dev='device', action='Init', args={}, snapshot=deepcopy(snapshot))]
    for fact in [issue, route]:
        snapshot['facts'].append(fact)
        rows.append(dict(dev='device', action='Observe', args={}, snapshot=deepcopy(snapshot)))
    snapshot['receipts'] = [dict(key='receipt', result=dict(ok='accepted', effects=[]))]
    rows.append(dict(dev='device', action='Apply', args=dict(key='receipt', accepted=True,
        duplicate=False, event=dict(act='Raise', args=dict(child=7, qid=9))), snapshot=deepcopy(snapshot)))
    return rows


class QuestionIssuanceTests(unittest.TestCase):
    def test_exact_accepted_route(self):
        got = question_issuance(path())
        self.assertEqual([(x['child'], x['run'], x['attempt'], x['raw_row']) for x in got], [('child', 1, 1, 3)])
        self.assertNotIn('provider_turn', got[0])

    def test_route_alone_does_not_prove_acceptance(self):
        self.assertEqual(question_issuance(path()[:-1]), [])

    def test_rejected_and_duplicate_add_no_claim(self):
        for key in ['accepted', 'duplicate']:
            rows = path()
            rows[-1]['args'][key] = key == 'duplicate'
            self.assertEqual(question_issuance(rows), [])

    def test_future_issuance_does_not_repair_missing_issue(self):
        rows = path()
        for row in rows[:4]:
            row['snapshot']['facts'] = [f for f in row['snapshot']['facts'] if f['id'] != 'issued']
        future = deepcopy(path()[1])
        rows.append(future)
        with self.assertRaisesRegex(QuestionIssuanceError, 'issued_identity'):
            question_issuance(rows)

    def test_stale_attempt_fails(self):
        rows = path()
        for row in rows:
            row['snapshot']['state']['threads'][0][1]['attempt']['n'] = 2
        with self.assertRaisesRegex(QuestionIssuanceError, 'current_attempt'):
            question_issuance(rows)

    def test_null_attempt_and_origin_fail(self):
        for kind, field, error in [('question_input_route', 'source_attempt', 'attempt_unknown'),
                                   ('question_input_issued', 'source_message_id', 'origin_unknown')]:
            rows = path()
            for row in rows:
                for fact in row['snapshot']['facts']:
                    if fact['kind'] == kind:
                        fact['data'][field] = None
            with self.assertRaisesRegex(QuestionIssuanceError, error):
                question_issuance(rows)

    def test_device_scoped_fact_ids_do_not_collide(self):
        rows = path()
        other = deepcopy(rows)
        for row in other:
            row['dev'] = 'other'
            for fact in row['snapshot']['facts']:
                if fact['kind'] == 'question_input_issued':
                    fact['data']['runtime_id'] = 'other-runtime'
        self.assertEqual(len(question_issuance(rows + other)), 2)

    def test_actual_accepted_receipt_required(self):
        rows = path()
        rows[-1]['snapshot']['receipts'][0]['result']['ok'] = 'rejected'
        with self.assertRaisesRegex(QuestionIssuanceError, 'accepted_receipt'):
            question_issuance(rows)

    def test_issue_and_route_need_prior_observed_order(self):
        rows = path()
        rows[1], rows[2] = rows[2], rows[1]
        rows[1]['snapshot']['facts'] = [f for f in rows[1]['snapshot']['facts'] if f['id'] == 'route']
        rows[2]['snapshot']['facts'] = deepcopy(path()[2]['snapshot']['facts'])
        with self.assertRaisesRegex(QuestionIssuanceError, 'source_order'):
            question_issuance(rows)


if __name__ == '__main__':
    unittest.main()
