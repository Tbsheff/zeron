import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'orch-trace.py'
spec = importlib.util.spec_from_file_location('orch_trace', SCRIPT)
trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace)


class TraceMerge(unittest.TestCase):
    def merge(self, rows):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'trace.jsonl'
            path.write_text(''.join(json.dumps(r) + '\n' for r in rows))
            return trace.merge([path])

    def row(self, dev, seq, msg=None, fx=None):
        return dict(v=1, dev=dev, seq=seq, act='Init' if seq == 0 else 'DoNotice',
                    msg=msg, fx=fx or [])

    def test_message_edges_override_device_sort(self):
        rows = [self.row('a', 0), self.row('a', 1, dict(id='effect:1', dir='recv')),
                self.row('z', 0), self.row('z', 1, fx=[dict(id='effect:1')])]
        ordered = [(r['dev'], r['seq']) for r in self.merge(rows[::-1])]
        self.assertLess(ordered.index(('z', 1)), ordered.index(('a', 1)))

    def test_missing_sequence_fails(self):
        with self.assertRaisesRegex(ValueError, 'missing sequence'):
            self.merge([self.row('a', 0), self.row('a', 2)])

    def test_duplicate_sequence_fails(self):
        with self.assertRaisesRegex(ValueError, 'duplicate sequence'):
            self.merge([self.row('a', 0), self.row('a', 0)])

    def test_message_cycle_fails(self):
        rows = [self.row('a', 0), self.row('z', 0),
                self.row('a', 1, dict(id='z', dir='recv')),
                self.row('a', 2, fx=[dict(id='a')]),
                self.row('z', 1, dict(id='a', dir='recv')),
                self.row('z', 2, fx=[dict(id='z')])]
        with self.assertRaisesRegex(ValueError, 'cycle'):
            self.merge(rows)

    def test_missing_remote_producer_fails(self):
        received = self.row('a', 1, dict(id='wire:1', dir='recv'))
        received['cmd'] = 'remote-recv:wire:1'
        with self.assertRaisesRegex(ValueError, 'missing remote send'):
            self.merge([self.row('a', 0), received])

    def test_retried_send_uses_first_causal_edge(self):
        rows = [self.row('a', 0), self.row('z', 0),
                self.row('z', 1, dict(id='wire:1', dir='send')),
                self.row('a', 1, dict(id='wire:1', dir='recv')),
                self.row('z', 2, dict(id='wire:1', dir='send'))]
        self.assertEqual(len(self.merge(rows)), 5)


if __name__ == '__main__':
    unittest.main()
