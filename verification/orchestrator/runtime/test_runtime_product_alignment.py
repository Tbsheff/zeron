import unittest
from runtime_full_projection import align_operational_paths,fn,transaction_image_rows,annotate_transactions


def action(label):return dict(label=label,args={})
def product(name,images,rows,labels):
    return {name+'_images':images,name+'_actions':[action(label) for label in labels],name+'_source_map':[dict(raw_row=row,macro_substep=i) for i,row in enumerate(rows)]}

class AlignmentTests(unittest.TestCase):
    def setUp(self):
        self.context=dict(reporting_constants={'UseReporting':True})
        self.core=dict(constants={},images=[fn([]),fn([(['r','r'],{'state':1})])],actions=[fn([(['r','r'],action('RegisterRoot'))])])

    def test_initial_catalog_macro_keeps_core_and_reporting_fixed(self):
        rp=product('reporting',[{'r':0},{'r':1}],[0,1],['LogicalStart'])
        tr=product('transfer',[{'t':0},{'t':1},{'t':2}],[0,0,1],['ObserveSavedDocuments','OpenLogical']);tr['transfer_constants']={}
        out=align_operational_paths(self.core,rp,tr,self.context)
        self.assertEqual(out['images'][:2],[fn([]),fn([])])
        self.assertEqual(out['reporting_actions'][0],action('Stutter'))
        self.assertEqual(out['transfer_actions'][0],action('ObserveSavedDocuments'))
        self.assertEqual(len(out['images']),len(out['transfer_images']))

    def test_loss_closure_commits_core_on_first_declared_substep(self):
        rp=product('reporting',[{'r':0},{'r':1},{'r':2}],[0,1,1],['HandleLossNative','Handle'])
        tr=product('transfer',[{'t':0},{'t':0}],[0,1],['Stutter']);tr['transfer_constants']={}
        out=align_operational_paths(self.core,rp,tr,self.context)
        self.assertEqual(out['images'][1:],self.core['images'][1:]*2)
        self.assertEqual(dict(out['actions'][1]['$fn'][0][1])['label'],'Stutter')

    def test_two_unordered_macros_refuse(self):
        rp=product('reporting',[{}, {}, {}],[0,1,1],['a','b'])
        tr=product('transfer',[{}, {}, {}],[0,1,1],['c','d']);tr['transfer_constants']={}
        with self.assertRaisesRegex(ValueError,'concurrent_macros'):align_operational_paths(self.core,rp,tr,self.context)

    def test_physical_group_metadata_survives_every_product_macro(self):
        tx=dict(id='sqlite:d:1',index=1,size=2,visible=True,known=True)
        out={name:[dict(raw_row=0,macro_substep=0),dict(raw_row=0,macro_substep=1)]
             for name in ['source_map','reporting_source_map','transfer_source_map']}
        annotate_transactions(out,[dict(transaction=tx)])
        for name,sources in out.items():
            self.assertEqual([s['physical_visible'] for s in sources],[False,True])
            self.assertEqual([s['transaction'] for s in sources],[tx,tx])

    def test_internal_group_row_is_never_visible(self):
        out=dict(source_map=[dict(raw_row=0,macro_substep=0)])
        annotate_transactions(out,[dict(transaction=dict(known=True,visible=False))])
        self.assertFalse(out['source_map'][0]['physical_visible'])

    def test_own_raw_microstep_uses_committed_foreign_state(self):
        row=dict(dev='a',action='Apply',transaction=dict(known=True),
                 partitions=fn([('a',{'step':2}),('b',{'step':1})]),
                 committed_partitions=fn([('a',{'step':0}),('b',{'step':1})]))
        self.assertEqual(transaction_image_rows([row])[0]['partitions'],row['partitions'])
        row['committed_partitions']=fn([('a',{'step':0}),('b',{'step':0})])
        with self.assertRaisesRegex(ValueError,'foreign_uncommitted'):transaction_image_rows([row])

if __name__=='__main__':unittest.main()
