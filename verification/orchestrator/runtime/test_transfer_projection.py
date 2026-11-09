"""Synthetic adapter checks. These are not captured engine witnesses."""
import copy
import unittest
from transfer_projection import project_transfer, transfer_event, project_journal

class TransferProjectionTests(unittest.TestCase):
    def fact(self):
        return {'id': 'transfer:test', 'kind': 'transfer_state', 'data': {
            'chat_id': 'fixture', 'revision': 1, 'durable': True, 'version': {'1':1}, 'lineage': {},
            'ledger': {}, 'committed': {}, 'receipts': {}, 'runs': {}, 'rejected': [], 'failures': {}, 'uncertain': {}, 'providers': {}, 'queue': [], 'entries': []}}

    def test_missing_state_fails(self):
        with self.assertRaises(ValueError):
            project_transfer({'facts': []})

    def test_observed_empty_catalog_projects_empty(self):
        self.assertEqual(project_transfer({'facts': [{'id': 'catalog', 'kind': 'transfer_catalog', 'data': {'scope': 'durable_snapshots', 'chats': [], 'unknown': []}}]}), {'$fn': []})

    def test_conflicting_revision_fails(self):
        a = self.fact()
        b = copy.deepcopy(a)
        b['data']['rejected'] = ['mock:input']
        with self.assertRaises(ValueError):
            project_transfer({'facts': [a, b]})

    def test_later_revision_selected(self):
        a = self.fact()
        b = copy.deepcopy(a)
        b['data']['revision'] = 2
        b['data']['version'] = {'1':2}
        b['data']['rejected'] = ['mock:input']
        result = project_transfer({'facts': [b, a]})
        self.assertEqual(result['$fn'][0][1]['revision'], 2)
        self.assertEqual(result['$fn'][0][1]['rejected'], {'$set': ['mock:input']})

    def test_observe_requires_exact_fact(self):
        row = {'action': 'Observe', 'args': {'kind': 'transfer_state', 'fact_id': 'missing'},
               'snapshot': {'facts': [self.fact()]}}
        with self.assertRaises(ValueError):
            transfer_event(row)

    def test_journal_accepts_exact_source_change(self):
        before = self.fact()['data']
        after = copy.deepcopy(before)
        after['version'] = {'1':2}
        after['revision'] = 2
        change = dict(id='1@1',deps=['0@1'],from_version=before['version'],to_version=after['version'],kind='NoTransferChange',before=before,after=after)
        result = project_journal({'before':before,'after':after,'journal':[change]})
        self.assertEqual(result[0]['id'],'1@1')

    def test_journal_rejects_missing_source_change(self):
        before = self.fact()['data']
        after = copy.deepcopy(before)
        after['version'] = {'1':2}
        after['revision'] = 2
        with self.assertRaisesRegex(ValueError,'does not reach'):
            project_journal({'before':before,'after':after,'journal':[]})

    def test_journal_rejects_wrong_mutation_label(self):
        before = self.fact()['data']
        after = copy.deepcopy(before)
        after['version'] = {'1':2}
        after['revision'] = 2
        change = dict(id='1@1',deps=['0@1'],from_version=before['version'],to_version=after['version'],kind='JournalChange',before=before,after=after)
        with self.assertRaisesRegex(ValueError,'mutation label'):
            project_journal({'before':before,'after':after,'journal':[change]})

    def test_journal_rejects_wrong_causal_dependency(self):
        before = self.fact()['data']
        after = copy.deepcopy(before)
        after['version'] = {'1':2}
        after['revision'] = 2
        change = dict(id='1@1',deps=['5@9'],from_version=before['version'],to_version=after['version'],kind='NoTransferChange',before=before,after=after)
        with self.assertRaisesRegex(ValueError,'causal dependency'):
            project_journal({'before':before,'after':after,'journal':[change]})

if __name__ == '__main__':
    unittest.main()
