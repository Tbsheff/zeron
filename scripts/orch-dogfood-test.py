#!/usr/bin/env python3
import contextlib
from datetime import datetime, timedelta, timezone
import io
import json
import importlib.util
from pathlib import Path
import sqlite3
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('dogfood', Path(__file__).with_name('orch-dogfood.py'))
dogfood = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dogfood)

class SnapshotTests(unittest.TestCase):
    def test_keeps_both_projections_at_one_sqlite_version(self):
        with tempfile.TemporaryDirectory() as root:
            db = Path(root) / 'snapshot.db'
            with sqlite3.connect(db) as writer:
                writer.execute('PRAGMA journal_mode=WAL')
                for table in ('orch_trace', 'orch_mechanism'):
                    writer.execute(f'CREATE TABLE {table}(seq INTEGER,line TEXT)')
                    writer.execute(f'INSERT INTO {table} VALUES(1,?)', ('old',))
                writer.commit()
                def advance():
                    for table in ('orch_trace', 'orch_mechanism'):
                        writer.execute(f'INSERT INTO {table} VALUES(2,?)', ('new',))
                    writer.commit()
                self.assertEqual(dogfood.read_snapshot(db, advance), (['old'], ['old']))
                self.assertEqual(dogfood.read_snapshot(db), (['old', 'new'], ['old', 'new']))

    def test_rejects_repeated_trace_with_changed_metadata(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            for day in range(8):
                capture = root / str(day)
                capture.mkdir()
                trace = capture / 'trace.jsonl'
                trace.write_text(json.dumps({'v':1,'dev':'test','seq':1}) + '\n')
                record = {'captured_at':(datetime.now(timezone.utc)-timedelta(days=8-day)).isoformat(),
                          'usage':'dogfood','traces':{'trace.jsonl':dogfood.digest(trace)},
                          'watermarks':{'test':day+1}}
                (capture/'capture.json').write_text(json.dumps(record))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(dogfood.status(root), 1)
            self.assertIn('Captured days: 1;', output.getvalue())
            record['captured_at'] = (datetime.now(timezone.utc)+timedelta(days=1)).isoformat()
            (capture/'capture.json').write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, 'future'):
                dogfood.status(root)

    def test_requires_current_full_checker_instead_of_stored_boolean(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            start = datetime.now(timezone.utc)-timedelta(days=9)
            for day in range(8):
                capture = root/str(day)
                capture.mkdir()
                trace = capture/'trace.jsonl'
                trace.write_text(json.dumps({'v':1,'dev':'test','seq':day+1})+'\n')
                record = {'captured_at':(start+timedelta(days=day)).isoformat(), 'usage':'dogfood',
                          'traces':{'trace.jsonl':dogfood.digest(trace)},
                          'full_zeronimpl_conformance':True}
                (capture/'capture.json').write_text(json.dumps(record))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(dogfood.status(root), 1)
            self.assertIn('seven-day window: True; full ZeronImpl: False', output.getvalue())

if __name__ == '__main__':
    unittest.main()
