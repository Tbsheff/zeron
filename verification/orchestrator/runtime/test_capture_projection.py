import unittest
from capture_projection import project_frames
from check_trace import tla


def row(seq,action,args):
    return dict(v=2,dev='A',seq=seq,action=action,args=args,
                snapshot=dict(state={},receipts=[],outbox=[],processed=[],facts=[]))

class CaptureProjectionTests(unittest.TestCase):
    def test_null_command_stays_absent(self):
        rows=[row(0,'Init',{}),row(1,'Apply',dict(key='k',command_id=None,effect_id=None,duplicate=False,accepted=True,event={'act':'CrashEngine'}))]
        result=project_frames(rows)
        self.assertEqual(result[1]['state']['command_receipts'],{'$fn':[]})
        self.assertEqual(result[1]['args']['command_id'],'none')
        self.assertEqual(result[1]['args']['effect_id'],'none')

    def test_real_command_keeps_exact_receipt_link(self):
        rows=[row(0,'Init',{}),row(1,'Apply',dict(key='k',command_id='command-1',duplicate=False,accepted=True,event={'act':'CrashEngine'}))]
        self.assertEqual(project_frames(rows)[1]['state']['command_receipts'],{'$fn':[('command-1','k')]})

    def test_command_phase_key_encodes_as_sequence(self):
        item=row(0,'Init',{})
        item['snapshot']['command_receipts']=[dict(command_id='c',phase=p,receipt={'source':'wire_send'}) for p in ('decision','outcome')]
        result=project_frames([item])[0]['state']
        self.assertEqual(len(result['command_decisions']['$fn']),2)
        self.assertIn('<<"c", "decision">>',tla(result))

if __name__=='__main__':unittest.main()
