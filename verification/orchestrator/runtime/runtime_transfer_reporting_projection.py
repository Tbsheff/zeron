"""Read retained CT request receipts from causal source prefixes.

Logical core existence does not prove a loaded or saved document. Saved source
snapshots prove document identity. Prepared packets and provider session starts
never prove a saved successful delivery. This reader does not call target Next.
"""
from copy import deepcopy
import hashlib
import json

from runtime_full_projection import Projection,FullProjectionError,fn,aset,entries,enum
from native_reporting_projection import decoded,known
from runtime_reporting_actions import facts,equal

FIELDS=('logicalCreated','loaded','durable','requests','current','logical','ranges',
        'bindings','outcomes','receipts','uncertain','uses','catalogKnown','savedDocuments','packets','logicalUses')
MAPS=set(FIELDS)-{'logicalCreated','loaded','uncertain','catalogKnown','logicalUses'}


def key(kind,*values):
    return kind+':'+json.dumps(values,separators=(',',':'),sort_keys=True)


def document_id(doc):
    return 'document:'+hashlib.sha256(json.dumps(doc,sort_keys=True,separators=(',',':')).encode()).hexdigest()


def unpack(value):return dict(entries(value))


class TransferReportingProjection:
    def __init__(self,rows,context):
        self.rows,self.context=rows,context
        self.chats=sorted({t for tree in context['tree_threads'].values() for t in tree})
        self.state={f:{} for f in MAPS}
        self.state.update(logicalCreated=set(),loaded=set(),uncertain=set(),logicalUses=set(),catalogKnown=False,
            current={c:'none' for c in self.chats},
            logical={c:dict(run=0,attempt=0,live=False) for c in self.chats})
        self.seen={};self.runtime={};self.docs={};self.row=0;self.action='Init'
        self.images=[];self.actions=[];self.source_map=[]
        self.scope={n:set() for n in ('Harnesses','Inputs','Requests','Pins','PacketIds','SessionIds','ReceiptIds','DocumentIds','MessageIds')}

    def fail(self,field):raise FullProjectionError(self.row,self.action,'transfer_reporting.'+field)

    def image(self):
        out={f:fn(deepcopy(list(self.state[f].items()))) for f in MAPS}
        for f in ('logicalCreated','loaded'):out[f]=aset(sorted(self.state[f]))
        out['uncertain']=aset([list(u) for u in sorted(self.state['uncertain'])])
        out['catalogKnown']=self.state['catalogKnown']
        out['logicalUses']=aset([list(u) for u in sorted(self.state['logicalUses'])])
                                                                             
        for f in ('bindings','outcomes'):
            out[f]=fn((list(k),deepcopy(v)) for k,v in self.state[f].items())
        out['uses']=fn((q,aset([list(u) for u in sorted(uses)])) for q,uses in self.state['uses'].items())
        return out

    def emit(self,label,args):
        self.images.append(self.image());self.actions.append(dict(label=label,args=deepcopy(args)))
        self.source_map.append(dict(raw_row=self.row,macro_substep=self.substep))
        self.substep+=1

    def saved(self,doc):
        if doc.get('durable') is not True:self.fail('document.not_saved')
        c=doc['chat_id']
        if c not in self.chats:return
        d=document_id(doc);self.scope['DocumentIds'].add(d)
        if d in self.state['savedDocuments']:
            if self.state['savedDocuments'][d]!=c:self.fail('document.identity_collision')
            return
        if c not in self.state['loaded']:
            self.state['loaded'].add(c);self.emit('LoadDocument',dict(c=c))
        self.state['durable'][c]=d;self.state['savedDocuments'][d]=c;self.docs[c]=deepcopy(doc)
        self.scope['MessageIds'].update(e['id'] for e in doc['entries'])
        self.emit('SaveDocument',dict(c=c,d=d))

    def catalog(self,data):
        if data.get('scope')!='durable_snapshots' or data.get('unknown'):self.fail('catalog.incomplete')
        catalog=dict(self.state['durable'])
        for doc in data['chats']:
            if doc['chat_id'] not in self.chats:continue
            if doc.get('durable') is not True:self.fail('document.not_saved')
            c=doc['chat_id'];d=document_id(doc)
            if d in self.state['savedDocuments'] and self.state['savedDocuments'][d]!=c:self.fail('document.identity_collision')
            catalog[c]=d;self.state['savedDocuments'][d]=c;self.state['loaded'].add(c)
            self.docs[c]=deepcopy(doc);self.scope['DocumentIds'].add(d)
            self.scope['MessageIds'].update(e['id'] for e in doc['entries'])
        self.state['durable']=catalog;self.state['catalogKnown']=True
        self.emit('ObserveSavedDocuments',dict(catalog=fn(catalog.items())))

    def logical(self,row):
        p=Projection(self.context);p.row=self.row;p.action=self.action
        _,_,threads=p.authority(row)
        for c in self.chats:
            th=threads.get(c)
            if th is None:continue
            if c not in self.state['logicalCreated']:
                self.state['logicalCreated'].add(c);self.emit('OpenLogical',dict(c=c))
            runs=th['runs'];n=th['run_base']+len(runs)
            target=dict(run=n,attempt=th['attempt']['n'],live=bool(runs and runs[-1]['status']=='Live'))
            old=self.state['logical'][c]
            if target==old:continue
            if target['run']==old['run']+1 and target['attempt']==1 and target['live'] and not old['live']:
                label='LogicalStart'
            elif target['run']==old['run'] and target['attempt']==old['attempt']+1 and target['live'] and old['live']:
                label='LogicalAttempt'
            elif target['run']==old['run'] and target['attempt']==old['attempt'] and not target['live'] and old['live']:
                label='LogicalFinish'
            else:self.fail('logical.macro_unsupported')
            self.state['logical'][c]=target
            if label in {'LogicalStart','LogicalAttempt'}:self.state['logicalUses'].add((c,n,target['attempt']))
            self.emit(label,dict(c=c,r=n,a=target['attempt']))

    def pin(self,c,identity,record):
        p=key('pin',c,identity)
        items=[item['id'] for item in record['items']]
        if len(items)!=len(set(items)):self.fail('pin.duplicate_items')
        point=known(record['point']);base=known(record['base'])
        if point is None:self.fail('pin.point_unknown')
        value=dict(source=record['source'],target=record['target'],base=base or 'none',point=point,items=items)
        if p in self.state['ranges']:
            if self.state['ranges'][p]!=value:self.fail('pin.changed')
        else:
            self.scope['Pins'].add(p);self.scope['MessageIds'].update(items+[point]+([base] if base else []))
            self.state['ranges'][p]=value;self.emit('PinRange',dict(p=p,range=value))
        return p

    def prepare(self,data):
        if not data['succeeded']:self.fail('prepare.failed_requires_outcome_contract')
        a=data['args'];c=a['chat'];h=a['harness'];i=a['input'];q=key('request',c,h,i)
        if c not in self.chats:return
        identities={(d['run'],d['attempt']) for d in self.runtime.values()
                    if d['chat']==c and known(d.get('origin_id'))==i
                    and isinstance(d.get('run'),int) and isinstance(d.get('attempt'),int)}
        if len(identities)!=1:self.fail('prepare.binding_identity')
        run,attempt=next(iter(identities))
        if (c,run,attempt) not in self.state['logicalUses']:self.fail('prepare.logical_identity')
        logical=dict(run=run,attempt=attempt)
        after=data['after'];runs=unpack(after['runs'])
        packets={v for k,v in runs.items() if k.startswith(h+':') and k.endswith(':'+i)}
        if len(packets)>1:self.fail('packet.ambiguous_request')
        packet='none';pin='none';selected=[]
        if packets:
            digest=next(iter(packets));packet='packet:'+digest
            records=[(x,r) for x,r in unpack(after['committed']).items() if known(r['packet'])==digest]
            if len(records)!=1:self.fail('packet.pin_identity')
            identity,record=records[0];pin=self.pin(c,identity,record);selected=record['selected']
            items=self.state['ranges'][pin]['items']
            if len(selected)!=len(set(selected)) or [x for x in items if x in selected]!=selected:self.fail('packet.order')
            value=dict(pin=pin,selected=selected)
            if packet in self.state['packets'] and self.state['packets'][packet]!=value:self.fail('packet.changed')
            self.state['packets'][packet]=value;self.scope['PacketIds'].add(packet)
        req=dict(chat=c,harness=h,input=i,run=logical['run'],attempt=logical['attempt'],pin=pin,packet=packet)
        if q in self.state['requests']:
            if any(self.state['requests'][q][f]!=req[f] for f in ('chat','harness','input','pin','packet')):self.fail('request.changed')
        else:self.state['requests'][q]=req
        self.scope['Requests'].add(q);self.scope['Harnesses'].add(h);self.scope['Inputs'].add(i)
        use=(q,logical['run'],logical['attempt']);self.state['uses'].setdefault(q,set()).add(use[1:])
        if packet!='none' and use not in self.state['outcomes']:self.state['uncertain'].add(use)
        if all(self.state['logical'][c][f]==logical[f] for f in ('run','attempt')):self.state['current'][c]=q
        self.emit('PrepareRequest',dict(q=q,c=c,h=h,i=i,p=pin,packet=packet,selected=selected,r=run,a=attempt))

    def outcome(self,c,r,a,outcome):
        candidates=[q for q,req in self.state['requests'].items() if req['chat']==c and (r,a) in self.state['uses'][q]]
        if len(candidates)!=1:self.fail('outcome.request_identity')
        q=candidates[0];use=(q,r,a);value={'Completed':'completed','Errored':'failed','Interrupted':'interrupted','Cancelled':'interrupted','Failed':'failed','Transient':'transient','UsageLimit':'usage_limit'}.get(outcome)
        if value is None:self.fail('outcome.unsupported')
        if use in self.state['outcomes']:
            if self.state['outcomes'][use]!=value:self.fail('outcome.changed')
            return
        self.state['outcomes'][use]=value;self.emit('RecordOutcome',dict(q=q,r=r,a=a,outcome=value))

    def operation(self,data):
        c=data['after']['chat_id']
        if c not in self.chats:return
        self.saved(data['before'])
        operation=data['operation']
        if operation=='PrepareRun':
                                                                          
            self.saved(data['after']);self.prepare(data)
        elif operation=='BindProvider':
            self.saved(data['after']);a=data['args']
            if not data['succeeded'] or a.get('validated_local') is not True:self.fail('binding.unvalidated')
            q=key('request',c,a['harness'],a['input'])
            if q not in self.state['requests']:self.fail('binding.request_unknown')
            uses=self.state['uses'][q]
            if len(uses)!=1:self.fail('binding.attempt_identity')
            r,at=next(iter(uses));u=(q,r,at);session=known(a['session'])
            if session is not None:
                if u in self.state['bindings'] and self.state['bindings'][u]!=session:self.fail('binding.session_changed')
                self.state['bindings'][u]=session;self.scope['SessionIds'].add(session)
                self.emit('BindSession',dict(q=q,r=r,a=at,session=session))
            if a.get('provider_event')=='Done' and a.get('provider_success') is True:
                if u not in self.state['outcomes']:
                    self.state['outcomes'][u]='completed';self.emit('RecordOutcome',dict(q=q,r=r,a=at,outcome='completed'))
                if self.state['requests'][q]['packet']!='none':
                    if session is None:self.fail('confirmation.session_unknown')
                    receipt=key('receipt',c,a['harness'],a['input'],document_id(data['after']))
                    packet=self.state['requests'][q]['packet'];d=document_id(data['after'])
                    delivery=f'{a["harness"]}:{session}'
                    selected=unpack(data['after']['receipts']).get(delivery)
                    if selected is None or not set(self.state['packets'][packet]['selected'])<=set(selected):self.fail('confirmation.saved_receipt')
                    run_key=f'{delivery}:{a["input"]}'
                    if unpack(data['after']['runs']).get(run_key)!=packet[len('packet:'):]:self.fail('confirmation.packet_identity')
                    if any(marker['input']==a['input'] for marker in unpack(data['after'].get('uncertain',fn([]))).values()):
                        self.fail('confirmation.uncertainty_retained')
                    self.scope['ReceiptIds'].add(receipt)
                    self.state['receipts'][receipt]=dict(request=q,chat=c,harness=a['harness'],input=a['input'],run=r,attempt=at,packet=packet,session=session,document=d)
                    self.state['uncertain'].discard(u)
                    self.emit('SaveConfirmation',dict(id=receipt,q=q,packet=packet,session=session,input=a['input'],r=r,a=at,document=d))
        else:self.fail('operation.unsupported')

    def run(self):
        for self.row,row in enumerate(self.rows):
            self.action=row.get('action','Unknown');self.substep=0
            if self.row==0:
                self.images.append(self.image());self.source_map.append(dict(raw_row=0,macro_substep=0,initial_unknown=True));self.substep=1
            start=len(self.images)
            self.logical(row)
            if self.row==0 and self.state['logicalCreated']:self.fail('init.preexisting_core')
            for identity,fact in facts(row).items():
                if identity in self.seen:
                    if self.seen[identity]!=fact:self.fail('fact.changed')
                    continue
                self.seen[identity]=deepcopy(fact);kind=fact['kind'];data=fact['data']
                if kind=='transfer_catalog':self.catalog(data)
                elif kind=='transfer_state':self.saved(data)
                elif kind=='transfer_operation':self.operation(data)
                elif kind=='runtime_binding':
                    if data.get('validated') is not True:self.fail('binding.unvalidated')
                    self.runtime[identity]=deepcopy(data)
            event=row.get('event',{})
            if self.action=='Apply' and row.get('args',{}).get('accepted') and not row['args'].get('duplicate') and event.get('act')=='ProviderTerminal':
                a=event['args'];self.outcome(a['t'],self.state['logical'][a['t']]['run'],self.state['logical'][a['t']]['attempt'],a['outcome'])
            if self.row>0 and len(self.images)==start:self.emit('Stutter',{})
        constants={n:aset(sorted(v)) for n,v in self.scope.items()}
        constants.update(Chats=aset(self.chats),NoMessage='none',NoRequest='none',NoPin='none',NoPacket='none',
            MaxCTRun=self.context['constants']['MaxRuns'],MaxCTAttempt=self.context['constants']['RLMaxAtt'],UseTransferReporting=True)
        return dict(transfer_constants=constants,transfer_images=self.images,transfer_actions=self.actions,transfer_source_map=self.source_map,
                    transfer_scope='saved source snapshots and exact request receipts; no provider process-liveness claim')


def project_transfer_reporting(rows,context):return TransferReportingProjection(rows,context).run()
