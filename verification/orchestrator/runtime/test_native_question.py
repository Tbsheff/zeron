#!/usr/bin/env python3
"""Small hand-built state/effect witnesses. No provider or full runtime claim."""
import argparse
from copy import deepcopy
from pathlib import Path
import shutil
import subprocess
from check_trace import tla
from remote_projection import fn, aset
HERE=Path(__file__).resolve().parent

def thread(t,parent='none',live=True):
    return dict(id=t,parent=parent,depth=0 if parent=='none' else 1,access=2,reconfigure=False,
        queue=[],held='None',run_base=0,runs=[dict(start=dict(kind='Questions'),status='Live',
        cohort={'disp':'Open','st':'None','del':aset([]),'generation':0})] if live else [],int_req=False,
        attempt=dict(n=1 if live else 0,reason='Initial',emitted=False,kid_open=False,retries=0,steers=0),
        inbox=aset([]),ul=dict(armed=0,due=False),wait_on='none',late_wait='none',task=dict(present=False))
def base():
    return dict(config=dict(max_depth=2,max_fanout=2,max_queue=4,max_retry=2,run_window=8),
        threads=fn([('root',thread('root')),('child',thread('child','root'))]),thread_order=['root','child'],
        outbox=aset([]),stop_receipts=aset([]),stops=0,questions=fn([]),question_order=[],question_receipts=aset([]),natives=fn([]),native_order=[])
def event(act,args,before):
    return dict(act=act,args=args,question_order=deepcopy(before['question_order']),native_order=deepcopy(before['native_order']))
def up(c,q,e,t): return dict(kind='QuestionUp',child=c,qid=q,epoch=e,to=t)
def release(c,q,e,t): return dict(kind='QuestionRelease',child=c,qid=q,epoch=e,via=t)
def answer(c,q,e,t,a): return dict(kind='AnswerDown',child=c,qid=q,epoch=e,via=t,answer=a)
def closed(c,q,e,t): return dict(kind='QuestionClosed',child=c,qid=q,epoch=e,to=t)
def send(m): return dict(kind='Send',message=m)
def view(c,q,e,t,r): return dict(kind='QuestionView',viewer=t,child=c,qid=q,epoch=e,retract=r)
def cases():
    rows=[]
    def add(name,b,a,e,fx): rows.append(dict(name=name,before=b,after=a,event=e,effects=fx))
    b=base(); a=deepcopy(b); k=['root','9007199254740995'];n=dict(attempt=dict(run=dict(thread='root',ord=1),n=1),background=True,ancestor='none',status='Running',delivery='Pending',generation=0)
    a['natives']=fn([(k,n)]);a['native_order']=[k];e=event('NativeStart',dict(parent='root',key=k[1],background=True),b);e['native_order']=[k]
    add('start',b,a,e,[dict(kind='NativeChanged',parent='root',key=k[1])])
    b=deepcopy(a);a=deepcopy(b);a['natives']['$fn'][0][1].update(status='Completed',delivery='Delivered')
    add('inline',b,a,event('NativeEnd',dict(parent='root',key=k[1],failed=False),b),[dict(kind='NativeChanged',parent='root',key=k[1])])
    b=deepcopy(b);b['threads']['$fn'][0][1]['attempt']['n']=2;a=deepcopy(b);a['natives']['$fn'][0][1].update(status='Completed',delivery='Claimed')
    add('late',b,a,event('NativeEnd',dict(parent='root',key=k[1],failed=False),b),[dict(kind='DeliverNative',parent='root',key=k[1],generation=0),dict(kind='NativeChanged',parent='root',key=k[1])])
    b=deepcopy(a);a=deepcopy(b);a['natives']['$fn'][0][1]['generation']=1
    add('recover',b,a,event('NativeRecover',dict(parent='root',key=k[1]),b),[dict(kind='DeliverNative',parent='root',key=k[1],generation=1)])
    b=deepcopy(a);add('stale-native-ack',b,deepcopy(b),event('NativeDelivered',dict(parent='root',key=k[1],generation=0),b),[])
    a=deepcopy(b);a['natives']['$fn'][0][1]['delivery']='Delivered'
    add('native-ack',b,a,event('NativeDelivered',dict(parent='root',key=k[1],generation=1),b),[])
    b=base();a=deepcopy(b);k=['child','18446744073709551615'];q=dict(child='child',qid=k[1],epoch=1,pending=True,holder='none',at_person=False,views=aset([]),waiting=aset([]),seen=aset([]),future_answers=fn([]))
    m=up('child',k[1],1,'root');a.update(questions=fn([(k,q)]),question_order=[k],outbox=aset([m]));e=event('RaiseQuestion',dict(child='child',qid=k[1]),b);e['question_order']=[k]
    add('raise',b,a,e,[send(m)])
    b=deepcopy(a);a=deepcopy(b);a['outbox']=aset([]);a['question_receipts']=aset([m]);a['questions']['$fn'][0][1].update(holder='root',views=aset(['root']))
    add('holder',b,a,event('DeliverQuestion',dict(message=m,how='Ack'),b),[view('child',k[1],1,'root',False)])
    b=deepcopy(a);m=answer('child',k[1],2,'root','8');a=deepcopy(b);a['question_receipts']['$set'].append(m);a['questions']['$fn'][0][1]['future_answers']=fn([(2,'8')])
    add('future-answer',b,a,event('ImportAnswer',dict(child='child',qid=k[1],epoch=2,via='root',answer='8'),b),[])
    b=deepcopy(a);m=release('child',k[1],2,'root');a=deepcopy(b);a['question_receipts']['$set'].append(m);a['questions']['$fn'][0][1].update(epoch=2,pending=False,holder='none',at_person=False,future_answers=fn([]));c=closed('child',k[1],2,'root');a['outbox']=aset([c])
    add('release-closes-buffered',b,a,event('ImportRelease',dict(child='child',qid=k[1],epoch=2,via='root'),b),[dict(kind='RespondInput',child='child',qid=k[1],epoch=2,answer='8'),send(c)])
    b=deepcopy(a);add('stale-person-answer',b,deepcopy(b),event('PersonAnswer',dict(child='child',qid=k[1],epoch=1,answer='9'),b),[])
    b=deepcopy(a);m=c;a=deepcopy(b);a['question_receipts']['$set'].append(m);a['outbox']=aset([]);a['questions']['$fn'][0][1].update(views=aset([]),waiting=aset([]),seen=aset([]))
    add('close-view',b,a,event('DeliverQuestion',dict(message=m,how='Ack'),b),[view('child',k[1],2,'root',True)])
    b=base();a=deepcopy(b);k=['root','7'];q=dict(child='root',qid='7',epoch=1,pending=True,holder='none',at_person=False,views=aset([]),waiting=aset([]),seen=aset([]),future_answers=fn([]));m=up('root','7',1,'none');b.update(questions=fn([(k,q)]),question_order=[k],outbox=aset([m]));a=deepcopy(b);a['outbox']=aset([]);a['question_receipts']=aset([m]);a['questions']['$fn'][0][1]['at_person']=True
    add('person-escape',b,a,event('DeliverQuestion',dict(message=m,how='Ack'),b),[dict(kind='NotifyQuestion',child='root',qid='7',epoch=1)])
                                                                               
    b=deepcopy(rows[0]['after']);k2=['child','4'];n2=dict(attempt=dict(run=dict(thread='child',ord=1),n=1),background=False,ancestor='none',status='Running',delivery='Pending',generation=0)
    b['natives']['$fn'].append((k2,n2));b['native_order'].append(k2);a=deepcopy(b);a['natives']['$fn'][0][1].update(status='Failed',delivery='Delivered')
    add('all-parent-native',b,a,event('NativeEnd',dict(parent='root',key='9007199254740995',failed=True),b),[dict(kind='NativeChanged',parent='root',key='9007199254740995')])
                                                                   
    b=deepcopy(rows[7]['after']);q=b['questions']['$fn'][0][1];q['epoch']=3;q['seen']=aset(['root']);a=deepcopy(b);q2=a['questions']['$fn'][0][1];q2.update(pending=False,holder='none',at_person=False,views=aset([]),waiting=aset([]),seen=aset([]));m=closed('child',q['qid'],3,'root');a['question_receipts']['$set'].append(m)
    add('authoritative-closure',b,a,event('ImportClosed',dict(child='child',qid=q['qid'],epoch=2,to='root'),b),[view('child',q['qid'],3,'root',True)])
                                                                           
    b=deepcopy(rows[7]['after']);m=up('child',b['question_order'][0][1],1,'root');b['outbox']=aset([m]);a=deepcopy(b);a['outbox']=aset([])
    add('duplicate-question-delivery',b,a,event('DeliverQuestion',dict(message=m,how='Ack'),b),[])
                                                        
    b=deepcopy(rows[6]['after']);b['threads']['$fn'][0][1]['held']='Stop';m=b['outbox']['$set'][0];a=deepcopy(b);a['outbox']=aset([up(m['child'],m['qid'],m['epoch'],'none')]);a['question_receipts']=aset([m])
    add('held-holder-skipped',b,a,event('DeliverQuestion',dict(message=m,how='Ack'),b),[send(up(m['child'],m['qid'],m['epoch'],'none'))])
                                                                 
    b=base();m=answer('child','77',9,'root','1');a=deepcopy(b);a['question_receipts']=aset([m])
    add('unknown-question-import',b,a,event('ImportAnswer',dict(child='child',qid='77',epoch=9,via='root',answer='1'),b),[])
    b=deepcopy(rows[6]['after']);b['threads']['$fn'][0][1]['wait_on']='child';m=b['outbox']['$set'][0];a=deepcopy(b);a['outbox']=aset([]);a['question_receipts']=aset([m]);a['questions']['$fn'][0][1].update(holder='root',views=aset(['root']),waiting=aset(['root']))
    add('wait-only-view',b,a,event('DeliverQuestion',dict(message=m,how='Ack'),b),[])
    b=deepcopy(rows[7]['after']);b['threads']['$fn'][0][1]['runs'][0]['status']='Completed';a=deepcopy(b);a['threads']['$fn'][0][1]['runs'].append(dict(start=dict(kind='Questions'),status='Live',cohort={'disp':'Open','st':'None','del':aset([]),'generation':0}));q=a['questions']['$fn'][0][1];q['seen']=aset(['root'])
    add('start-holder-run',b,a,event('StartQuestion',dict(t='root'),b),[view('child',q['qid'],1,'root',False),dict(kind='StartRun',run=dict(thread='root',ord=2),start=dict(kind='Questions'),access=2,delivered=[])])
    b=deepcopy(a);m=answer('child',q['qid'],1,'root','42');a=deepcopy(b);a['outbox']=aset([m])
    add('holder-agent-answer',b,a,event('AgentAnswer',dict(agent='root',child='child',qid=q['qid'],epoch=1,answer='42'),b),[send(m)])
    b=deepcopy(b);add('stale-agent-answer',b,deepcopy(b),event('AgentAnswer',dict(agent='root',child='child',qid=q['qid'],epoch=0,answer='43'),b),[])
    b=deepcopy(rows[13]['before']);b['natives']['$fn'][0][1]['background']=False;b['threads']['$fn'][0][1]['attempt']['n']=2;b['natives']['$fn'][1][1]['background']=True;b['threads']['$fn'][1][1]['attempt']['n']=2;a=deepcopy(b);a['natives']['$fn'][0][1].update(status='Orphaned',delivery='Disposed')
    add('foreground-closes-background-lives',b,a,event('NativeDelivered',dict(parent='child',key='4',generation=0),b),[dict(kind='NativeChanged',parent='root',key='9007199254740995')])
    b=deepcopy(rows[0]['after']);b['threads']['$fn'][0][1]['runs'][0]['status']='Completed';b['threads']['$fn'][0][1]['attempt']['n']=2;a=deepcopy(b);owner=a['natives']['$fn'][0][1];key=['root','9007199254740996'];nested=dict(attempt=deepcopy(owner['attempt']),background=True,ancestor='9007199254740995',status='Running',delivery='Pending',generation=0);a['natives']['$fn'].append((key,nested));a['native_order'].append(key);ev=event('NativeStartNested',dict(parent='root',key=key[1],ancestor='9007199254740995',background=False),b);ev['native_order']=deepcopy(a['native_order'])
    add('nested-background-after-root-end',b,a,ev,[dict(kind='NativeChanged',parent='root',key=key[1])])
    b=deepcopy(a);add('duplicate-nested-before-guards',b,deepcopy(b),event('NativeStartNested',dict(parent='root',key=key[1],ancestor='missing',background=False),b),[])
    b=deepcopy(rows[0]['after']);b['natives']['$fn'][0][1]['background']=False;a=deepcopy(b);nested=dict(attempt=deepcopy(b['natives']['$fn'][0][1]['attempt']),background=False,ancestor='9007199254740995',status='Running',delivery='Pending',generation=0);a['natives']['$fn'].append((key,nested));a['native_order'].append(key);ev=event('NativeStartNested',dict(parent='root',key=key[1],ancestor='9007199254740995',background=False),b);ev['native_order']=deepcopy(a['native_order'])
    add('nested-foreground',b,a,ev,[dict(kind='NativeChanged',parent='root',key=key[1])])
    b=deepcopy(a);a=deepcopy(b);a['natives']['$fn'][0][1].update(status='Completed',delivery='Delivered');a['natives']['$fn'][1][1].update(status='Orphaned',delivery='Disposed')
    add('ancestor-end-closes-foreground',b,a,event('NativeEnd',dict(parent='root',key='9007199254740995',failed=False),b),[dict(kind='NativeChanged',parent='root',key='9007199254740995'),dict(kind='NativeChanged',parent='root',key=key[1])])
    b=deepcopy(rows[-1]['before']);grandkey=['root','9007199254740997'];grand=deepcopy(b['natives']['$fn'][1][1]);grand['ancestor']=key[1];b['natives']['$fn'].append((grandkey,grand));b['native_order'].append(grandkey);a=deepcopy(b)
    a['natives']['$fn'][0][1].update(status='Completed',delivery='Delivered')
    for _,n in a['natives']['$fn'][1:]: n.update(status='Orphaned',delivery='Disposed')
    add('ancestor-end-closes-whole-chain',b,a,event('NativeEnd',dict(parent='root',key='9007199254740995',failed=False),b),[dict(kind='NativeChanged',parent='root',key=k[1]) for k in b['native_order']])
    b=deepcopy(rows[-1]['before']);b['natives']['$fn'][2][1]['background']=True;a=deepcopy(b);a['natives']['$fn'][0][1].update(status='Completed',delivery='Delivered');a['natives']['$fn'][1][1].update(status='Orphaned',delivery='Disposed')
    add('ancestor-end-keeps-background-descendant',b,a,event('NativeEnd',dict(parent='root',key='9007199254740995',failed=False),b),[dict(kind='NativeChanged',parent='root',key=k[1]) for k in b['native_order'][:2]])
    b=deepcopy(rows[-2]['before']);b['natives']['$fn'][0][1]['ancestor']=grandkey[1];a=deepcopy(b)
    for _,n in a['natives']['$fn']: n.update(status='Orphaned',delivery='Disposed')
    add('cycle-closes-foreground',b,a,event('NativeDelivered',dict(parent='root',key=grandkey[1],generation=0),b),[dict(kind='NativeChanged',parent='root',key=k[1]) for k in b['native_order']])
    b=deepcopy(rows[-3]['before']);b['natives']['$fn'][0][1]['ancestor']='404';a=deepcopy(b)
    for _,n in a['natives']['$fn']: n.update(status='Orphaned',delivery='Disposed')
    add('missing-ancestor-closes-whole-chain',b,a,event('NativeDelivered',dict(parent='root',key=grandkey[1],generation=0),b),[dict(kind='NativeChanged',parent='root',key=k[1]) for k in b['native_order']])
    return rows

def rejections():
    rs=[]
    def add(b,act,args,reason): rs.append(dict(state=b,event=event(act,args,b),reason=reason))
    b=base();add(b,'NativeEnd',dict(parent='root',key='404',failed=False),'NotKid')
    add(b,'NativeStart',dict(parent='absent',key='404',background=False),'UnknownThread')
    b=deepcopy(b);b['threads']['$fn'][0][1]['runs'][0]['status']='Completed'
    add(b,'NativeStart',dict(parent='root',key='404',background=False),'NotLive')
    b=deepcopy(cases()[0]['after']);add(b,'NativeClaim',dict(parent='root',key='9007199254740995'),'NotSettled')
    b=base();b['threads']['$fn'][1][1].update(wait_on='root',held='Stop');b['threads']['$fn'][1][1]['runs'][0]['status']='Completed'
    add(b,'RaiseQuestion',dict(child='child',qid='8'),'NotLive')
    b=deepcopy(b);b['threads']['$fn'][1][1]['runs'][0]['status']='Live';add(b,'RaiseQuestion',dict(child='child',qid='8'),'Waiting')
    b=deepcopy(b);b['threads']['$fn'][1][1]['wait_on']='none';add(b,'RaiseQuestion',dict(child='child',qid='8'),'InterruptRequested')
    b=base();b['threads']['$fn'][0][1]['held']='Stop';add(b,'StartQuestion',dict(t='root'),'RunLive')
    b=deepcopy(b);b['threads']['$fn'][0][1]['runs'][0]['status']='Completed';add(b,'StartQuestion',dict(t='root'),'Held')
    b=deepcopy(b);b['threads']['$fn'][0][1]['held']='None';add(b,'StartQuestion',dict(t='root'),'QueueEmpty')
    b=deepcopy(cases()[7]['after']);q=b['questions']['$fn'][0][1];add(b,'AgentAnswer',dict(agent='root',child='child',qid=q['qid'],epoch=1,answer='5'),'NotCurrent')
    b=deepcopy(b);b['threads']['$fn'][0][1]['runs'][0]['status']='Completed';add(b,'AgentAnswer',dict(agent='root',child='child',qid=q['qid'],epoch=1,answer='5'),'NotLive')
    b=base();add(b,'DeliverQuestion',dict(message=up('child','1',1,'root'),how='Ack'),'NotInOutbox')
    add(b,'ImportClosed',dict(child='child',qid='1',epoch=1,to='absent'),'UnknownThread')
    add(b,'ImportQuestion',dict(child='absent',qid='1',epoch=1,to='absent'),'UnknownThread')
    b=base();m=dict(kind='Notice',child='child',run=1);b['outbox']=aset([m]);add(b,'DeliverQuestion',dict(message=m,how='Ack'),'Corrupt')
    b=base();add(b,'NativeStartNested',dict(parent='root',key='9',ancestor='9',background=False),'NotKid')
    add(b,'NativeStartNested',dict(parent='root',key='9',ancestor='8',background=False),'NotKid')
    b=deepcopy(cases()[0]['after']);b['natives']['$fn'][0][1]['status']='Completed';add(b,'NativeStartNested',dict(parent='root',key='9',ancestor='9007199254740995',background=False),'NotLive')
    b=deepcopy(cases()[0]['after']);b['natives']['$fn'][0][1]['background']=False;b['threads']['$fn'][0][1]['attempt']['emitted']=True;add(b,'NativeStartNested',dict(parent='root',key='9',ancestor='9007199254740995',background=False),'NotLive')
    return rs

def main():
    p=argparse.ArgumentParser();p.add_argument('--jar',type=Path,required=True);p.add_argument('--mutant',default='none',choices=['none','stale-ack','lost-epoch','lost-effect','wrong-order','unsupported','wrong-reject','lost-ancestor','lost-background','kept-grandchild']);p.add_argument('--out',type=Path,required=True);args=p.parse_args()
    rows=cases()
    rejected=rejections()
    if args.mutant=='wrong-reject': rejected[4]['reason']='Waiting'
    if args.mutant=='stale-ack': rows[4]['after']['natives']['$fn'][0][1]['delivery']='Delivered'
    if args.mutant=='lost-epoch': rows[9]['after']['questions']['$fn'][0][1]['epoch']=1
    if args.mutant=='lost-effect': rows[2]['effects']=rows[2]['effects'][1:]
    if args.mutant=='wrong-order': rows[2]['effects'].reverse()
    if args.mutant=='unsupported': rows[0]['event']['act']='UnmappedNativeStart'
    if args.mutant=='lost-ancestor': rows[23]['after']['natives']['$fn'][1][1]['ancestor']='none'
    if args.mutant=='lost-background': rows[23]['after']['natives']['$fn'][1][1]['background']=False
    if args.mutant=='kept-grandchild': rows[27]['after']['natives']['$fn'][2][1].update(status='Running',delivery='Pending')
    args.out.mkdir(parents=True,exist_ok=True)
    for name in ('RuntimeState.tla','RuntimeNativeQuestionSeams.tla'): shutil.copy(HERE/name,args.out/name)
    (args.out/'NativeQuestionFixture.tla').write_text('---- MODULE NativeQuestionFixture ----\nEXTENDS RuntimeNativeQuestionSeams\nCases == '+tla(rows)+'\nRejected == '+tla(rejected)+'\nVARIABLE i\nInit == i=1\nNext == /\\ i<Len(Cases) /\\ i\'=i+1\nSpec == Init /\\ [][Next]_i\nGood == /\\ NativeQuestionApply(Cases[i].before,Cases[i].after,Cases[i].event,Cases[i].effects) /\\ \\A j \\in 1..Len(Rejected): NativeQuestionReject(Rejected[j].state,Rejected[j].event,Rejected[j].reason)\n====\n')
    (args.out/'NativeQuestionFixture.cfg').write_text('SPECIFICATION Spec\nINVARIANT Good\nCHECK_DEADLOCK FALSE\n')
    r=subprocess.run(['java','-XX:ActiveProcessorCount=2','-cp',str(args.jar.resolve()),'tlc2.TLC','-workers','1','-config','NativeQuestionFixture.cfg','NativeQuestionFixture'],cwd=args.out,text=True,capture_output=True)
    (args.out/'check.log').write_text(r.stdout+r.stderr)
    print(r.stdout+r.stderr)
    return r.returncode
if __name__=='__main__': raise SystemExit(main())
