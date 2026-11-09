"""Map physical CT facts. Missing evidence is an error, never a target Init value."""
from copy import deepcopy
import json
import hashlib

from runtime_full_projection import FullProjectionError, aset, fn

NO_BUDGET = hashlib.sha256(b'context transfer failed: NoBudget').hexdigest()

FIELDS = ('xMsgs', 'xCreated', 'xForkOf', 'xKindOf', 'xRun', 'xPt', 'xXf',
          'xNx', 'xVol', 'xDup', 'xAbort', 'xCrashLeft', 'xStaleLeft')


def known(value):
    if isinstance(value, dict) and 'present' in value:
        return value.get('value') if value['present'] else None
    return value


def mapping(value):
    if not isinstance(value, dict):
        raise ValueError('expected captured map')
    return dict(value['$fn']) if set(value) == {'$fn'} else value


def members(value):
    return value['$set'] if isinstance(value, dict) and set(value) == {'$set'} else value


def frozen(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'))


class TransferProjection:
    def __init__(self, prefix, context, root):
        self.prefix, self.context, self.root = prefix, context, root
        self.constants = context['constants']
        self.tree = context['tree_threads'][root]
        self.row, self.action = 0, 'Init'
        self.docs, self.facts, self.operations = {}, {}, []
        self.aliases, self.message_ids, self.journal = {}, {}, []
        self.prepares, self.binds, self.accepts, self.streams = [], [], [], {}
        self.transfer_prepares = {}
        self.catalog = False
        self.runtime = {}
        self.crashes = self.stales = 0
        self.lineage = {}
        self.resolve_heads, self.supersede_reasons = {}, {}
        self.crashed = set()
        self.transfer_order = []
        self.seen_transfers = set()

    def fail(self, field):
        raise FullProjectionError(self.row, self.action, field)

    def provider(self, harness):
        providers = members(self.constants.get('Providers', []))
        result = self.context.get('ct_provider_map', {}).get(harness, harness)
        if result not in providers:
            self.fail('xPt.provider_identity')
        return result

    def key(self, chat, identity):
        key = (chat, identity)
        visited = set()
        while key in self.aliases:
            if key in visited:
                self.fail('xMsgs.lineage_cycle')
            visited.add(key)
            key = self.aliases[key]
        return key

    def ordinal(self, chat, identity):
        identity = known(identity)
        if identity is None or identity == 'none':
            return 0
        key = self.key(chat, identity)
        if key not in self.message_ids:
            self.fail('xMsgs.missing_item')
        return self.message_ids[key]

    def accepted_ordinal(self, identity):
        found = {ordinal for (chat, item), ordinal in self.message_ids.items() if item == identity}
        if len(found) != 1:
            self.fail('xPt.accepted_item_identity')
        return next(iter(found))

    def add_lineage(self, doc):
        for identity, receipt in mapping(doc.get('lineage', {})).items():
            if receipt.get('validated') is not True:
                self.fail('xForkOf.unvalidated_lineage')
            if receipt['target_chat'] != doc['chat_id']:
                self.fail('xForkOf.target')
            old = self.lineage.get(identity)
            if old is not None and old != receipt:
                self.fail('xForkOf.immutable_receipt')
            self.lineage[identity] = deepcopy(receipt)
            for item in receipt['copied_items']:
                target = known(item['target_item_id'])
                if target is None:
                    continue
                if item.get('validated') is not True:
                    self.fail('xMsgs.unvalidated_copy')
                alias = (receipt['target_chat'], target)
                source = (item['source_chat'], item['source_item_id'])
                if alias == source:
                    self.fail('xMsgs.self_copy')
                if alias in self.message_ids:
                    self.fail('xMsgs.late_copy_identity')
                if alias in self.aliases and self.aliases[alias] != source:
                    self.fail('xMsgs.changed_copy_identity')
                self.aliases[alias] = source

    def add_doc(self, doc):
        chat = doc['chat_id']
        if chat not in self.tree:
            return
        if doc.get('durable') is not True:
            self.fail('xCreated.document_not_durable')
        self.add_lineage(doc)
        old = self.docs.get(chat)
        if old:
            if doc['revision'] < old['revision']:
                return
            if doc['revision'] == old['revision'] and doc != old:
                self.fail('xMsgs.conflicting_revision')
            for key, packet in mapping(old['runs']).items():
                if mapping(doc['runs']).get(key) != packet:
                    self.fail('xPt.immutable_packet')
            current_entries = {entry['id']:entry for entry in doc['entries']}
            for entry in old['entries']:
                if entry.get('complete') is True and current_entries.get(entry['id']) != entry:
                    self.fail('xMsgs.completed_entry_rewrite')
            previous = [entry['id'] for entry in old['entries']]
            if [entry['id'] for entry in doc['entries']][:len(previous)] != previous:
                self.fail('xMsgs.journal_rewrite')
        for entry in doc['entries']:
            key = self.key(chat, entry['id'])
            if key in self.message_ids:
                continue
            if key[0] != chat:
                self.fail('xMsgs.copy_source_unobserved')
            role = entry['role']
            if role == 'system':
                identity = entry['id']
                if identity.startswith('handoff:'):
                    transfer = mapping(doc['ledger']).get(identity[len('handoff:'):])
                    if transfer is None or transfer['status'] != 'Consumed':
                        self.fail('xMsgs.context_receipt')
                    receipt = self.lineage.get(transfer['id'])
                    if receipt is None: self.fail('xMsgs.context_kind')
                    if receipt['kind'] not in {'merge_back','subagent_result'}: continue
                    context = {self.ordinal(transfer['source'], item) for item in transfer['selected']}
                    self.message_ids[key] = len(self.journal)+1
                    self.journal.append(dict(th=chat, role='ctx', cx=aset(context)))
                    continue
                if identity.startswith('handoff-error:'):
                    transfer = mapping(doc['ledger']).get(identity[len('handoff-error:'):])
                    if transfer is not None and transfer['status'] == 'Failed': continue
                self.fail('xMsgs.system_context_expansion')
            if role not in {'user', 'assistant'}:
                self.fail('xMsgs.role')
            self.message_ids[key] = len(self.journal) + 1
            self.journal.append(dict(th=chat, role='asst' if role == 'assistant' else 'user', cx=aset([])))
        self.docs[chat] = deepcopy(doc)

    def pin_order(self, data):
        previous = data['before']
        if not set(mapping(previous['ledger'])) <= self.seen_transfers:
            self.fail('xNx.unordered_before')
        for change in data['journal']:
            if change['before'] != previous:
                self.fail('xNx.journal_chain')
            current = change['after']
            new = set(mapping(current['ledger'])) - self.seen_transfers
            if len(new) > 1:
                self.fail('xNx.ambiguous_pin_order')
            if new:
                identity = next(iter(new))
                self.seen_transfers.add(identity)
                self.transfer_order.append(identity)
            for identity, record in mapping(current['ledger']).items():
                before_record = mapping(previous['ledger']).get(identity, {})
                if record['status'] in {'ResolvedPortable', 'ResolvedNative', 'Consumed', 'Failed'} and before_record.get('status') == 'Pending':
                    self.resolve_heads.setdefault(identity, len(self.journal))
                if record['status'] == 'Superseded' and before_record.get('status') != 'Superseded':
                    if data['operation'] == 'RejectResume': self.supersede_reasons[identity] = 'uncertain'
                    elif data['operation'] in {'TransferPin','ForkPin','MergeBackPinAndQueue'}: self.supersede_reasons[identity] = 'replaced'
                    else: self.fail('xXf.supersede_reason')
            previous = current
        if previous != data['after']:
            self.fail('xNx.journal_after')

    def scan(self):
        for self.row, row in enumerate(self.prefix):
            self.action = row.get('action', 'Unknown')
            for dev, part in mapping(row['partitions']).items():
                mechanism = part['mechanism']
                facts = mapping(mechanism['facts'])
                for identity in mechanism['fact_order']:
                    fact = facts[identity]
                    scope = (dev, identity)
                    if scope in self.facts:
                        if self.facts[scope] != fact:
                            self.fail('xMsgs.immutable_fact')
                        continue
                    self.facts[scope] = deepcopy(fact)
                    kind, data = fact['kind'], fact['data']
                    if kind == 'transfer_catalog':
                        if data.get('scope') != 'durable_snapshots' or data.get('unknown'):
                            self.fail('xCreated.incomplete_catalog')
                        self.catalog = True
                        new_chats = [doc for doc in data['chats'] if doc['chat_id'] in self.tree
                                     and doc['entries'] and doc['chat_id'] not in self.docs]
                        if len(new_chats) > 1:
                            self.fail('xMsgs.unordered_catalog_journals')
                        for doc in data['chats']:
                            self.add_doc(doc)
                    elif kind == 'transfer_state':
                        self.add_doc(data)
                    elif kind == 'transfer_operation':
                        operation = data['operation']
                        if operation in {'Crash', 'Stale'}:
                            if data.get('succeeded') is not True:
                                self.fail('xCrashLeft.unvalidated_event')
                            if operation == 'Crash':
                                self.crashes += 1
                                targets = data.get('args', {}).get('targets')
                                if not isinstance(targets, list): self.fail('xXf.crash_targets')
                                for doc in self.docs.values():
                                    self.crashed.update(identity for identity, item in mapping(doc['ledger']).items()
                                                        if item['target'] in targets and item['status'] in {'Pending','ResolvedPortable'})
                            else: self.stales += 1
                            continue
                        self.add_doc(data['before'])
                        self.pin_order(data)
                        self.operations.append(deepcopy(data))
                        if operation == 'PrepareRun':
                            before_records = mapping(data['before']['ledger'])
                            for transfer_id, record in mapping(data['after']['ledger']).items():
                                if record['status'] in {'Consumed','Failed','ResolvedPortable'} and before_records.get(transfer_id,{}).get('status') in {None,'Pending','ResolvedPortable'}:
                                    self.transfer_prepares.setdefault(transfer_id,deepcopy(data))
                        for change in data['journal']:
                            self.add_doc(change['after'])
                        if operation == 'PrepareRun': self.prepares.append(deepcopy(data))
                        elif operation == 'BindProvider': self.binds.append(deepcopy(data))
                    elif kind == 'runtime_binding':
                        if data.get('validated') is not True: self.fail('xRun.unvalidated_binding')
                        chat = data['chat']
                        if chat in self.tree:
                            if data['operation'] in {'install','issue','rebind'}: self.runtime[chat] = deepcopy(data)
                            elif data['operation'] in {'remove','retire'}: self.runtime.pop(chat,None)
                            elif data['operation'] == 'config': pass
                            else: self.fail('xRun.binding_operation')
                    elif kind == 'transfer_provider':
                        if data.get('stage') != 'accepted_request':
                            self.fail('xPt.acceptance_stage')
                        self.accepts.append(deepcopy(data))
                    elif kind == 'provider_stream_catalog':
                        if data.get('complete') is not True:
                            self.fail('xPt.incomplete_stream_catalog')
                        self.streams[dev] = deepcopy(data['streams'])
        if not self.catalog:
            self.fail('xCreated.missing_catalog')
        if self.root not in self.docs:
            self.fail('xCreated.missing_root_document')

    def result(self):
        self.scan()
        maximum = self.constants['MaxX']
        if len(self.transfer_order) > maximum:
            self.fail('xNx.exhausted')
        no_thread, no_provider = self.constants['NoThread'], self.constants['NoProv']
        created = {chat: chat in self.docs for chat in self.tree}
        lineage = {chat: dict(src=no_thread, w=0) for chat in self.tree}
        kinds = {chat: 'none' for chat in self.tree}
        for chat in self.docs:
            if chat == self.root:
                continue
            receipts = [receipt for receipt in self.lineage.values()
                        if receipt['target_chat'] == chat and receipt['kind'] in {'fork', 'subagent_spawn'}]
            if len(receipts) != 1:
                self.fail('xForkOf.creation_receipt')
            receipt = receipts[0]
            lineage[chat] = dict(src=receipt['source_chat'],
                w=self.ordinal(receipt['source_chat'], receipt['source_point']['last_item_id']))
            kinds[chat] = receipt['kind']
        providers = members(self.constants['Providers'])
        pt = {chat: {p: dict(alive=False, pcov=set(), pst=set(), ecov=set(), est=set(), seen=0, dpend=False)
                     for p in providers} for chat in self.tree}
        runs = {chat: dict(st='idle', p=no_provider, u=0, w=0) for chat in self.tree}
        vol = {chat: 'none' for chat in self.tree}
        abort = {chat: 'none' for chat in self.tree}
        prep_for = {}
        for operation in self.prepares:
            args = operation['args']; chat = args['chat']
            if chat not in self.tree: continue
            p = self.provider(args['harness'])
            u = self.ordinal(chat, args['input'])
            prep_for[chat] = operation
            runs[chat] = dict(st='starting', p=p, u=u, w=u-1)
            if not operation['succeeded']:
                failures = [reason for key,reason in mapping(operation['after']['failures']).items()
                            if key.startswith(args['harness']+':') and key.endswith(':'+args['input'])]
                if not failures or any(reason != NO_BUDGET for reason in failures):
                    self.fail('xAbort.failure_reason')
                abort[chat] = 'budget'; runs[chat] = dict(st='idle', p=no_provider, u=0, w=0)
        for chat, binding in self.runtime.items():
            if chat not in prep_for or prep_for[chat]['args']['input'] != binding['origin_id']:
                self.fail('xRun.missing_prepare')
        done = {}
        coverage, stated, duplicate = {}, {}, False
        for operation in self.binds:
            args = operation['args']; chat = operation['after']['chat_id']
            if chat not in self.tree: continue
            if args.get('validated_local') is not True:
                self.fail('xPt.unvalidated_binding')
            if args.get('provider_event') != 'Done' or args.get('provider_success') is not True:
                continue
            if (chat, args['input']) in done: continue
            matches = [accepted for accepted in self.accepts
                       if (accepted['chat'], accepted['input'], accepted['session']) == (chat, args['input'], args['session'])]
            if len(matches) != 1:
                self.fail('xPt.done_without_accepted_packet')
            accepted = matches[0]; p = self.provider(args['harness'])
            records = mapping(operation['after']['runs'])
            key = f'{args["harness"]}:{args["session"]}:{args["input"]}'
            if records.get(key) != accepted['context_packet_digest']:
                self.fail('xPt.accepted_packet_identity')
            ids = {self.accepted_ordinal(identity) for identity in accepted['accepted_ids']}
            ids.add(self.ordinal(chat, args['input']))
            seen = self.ordinal(chat, args['seen'])
            if seen:
                ids.add(seen)
            physical = (chat,p,args['session'])
            if coverage.get(physical,set()) & ids: duplicate = True
            coverage.setdefault(physical,set()).update(ids)
            packet_records = [record for record in mapping(operation['after']['ledger']).values()
                              if known(record['packet']) == accepted['context_packet_digest']]
            omitted = set()
            for record in packet_records:
                header = known(known(record['observed_packet'])['header'])
                if header is None: self.fail('xPt.accepted_packet_header')
                selected = {self.ordinal(record['source'], identity) for identity in record['selected']}
                candidates = {self.ordinal(record['source'], item['id']) for item in record['items']}
                if header['omitted'] != len(candidates-selected): self.fail('xPt.omitted_identity')
                omitted.update(candidates-selected)
            if accepted['accepted_ids'] and not packet_records: self.fail('xPt.accepted_packet_range')
            stated.setdefault(physical,set()).update(omitted)
            done[(chat, args['input'])] = operation
            runs[chat] = dict(st='idle', p=no_provider, u=0, w=0)
        if not self.streams:
            self.fail('xPt.missing_stream_catalog')
        for streams in self.streams.values():
            for stream in streams:
                chat = stream['chat']
                if chat not in self.docs: continue
                session = known(stream.get('provider_session_id'))
                marks = mapping(self.docs[chat]['providers'])
                matches = [h for h, mark in marks.items() if known(mark['session']) == session]
                if session is None or len(matches) != 1:
                    self.fail('xPt.stream_session_identity')
                p = self.provider(matches[0]); alive = known(stream.get('provider_alive'))
                if type(alive) is not bool:
                    self.fail('xPt.provider_alive')
                pt[chat][p]['alive'] = alive
                physical = (chat,p,session)
                if alive and physical not in coverage:
                    self.fail('xPt.context_acceptance_unconfirmed')
                pt[chat][p]['pcov'] = set(coverage.get(physical,set()))
                pt[chat][p]['pst'] = set(stated.get(physical,set()))
                active = known(stream.get('root_turn_active'))
                if type(active) is not bool:
                    self.fail('xRun.root_turn_active')
                if active:
                    if chat not in prep_for: self.fail('xRun.unprepared_provider_turn')
                    runs[chat]['st'] = 'live'
        for chat, doc in self.docs.items():
            marks = mapping(doc['providers'])
            for harness, mark in marks.items():
                p = self.provider(harness); delivery = f'{harness}:{known(mark["session"])}'
                receipt = mapping(doc['receipts']).get(delivery, [])
                pt[chat][p]['ecov'].update(self.accepted_ordinal(identity) for identity in members(receipt))
                for (done_chat, input_id), operation in done.items():
                    args = operation['args']
                    if done_chat == chat and args['harness'] == harness and args['session'] == known(mark['session']):
                        pt[chat][p]['ecov'].update({self.ordinal(chat,input_id),self.ordinal(chat,args['seen'])})
                pt[chat][p]['seen'] = self.ordinal(chat, mark['seen'])
                physical = (chat,p,known(mark['session']))
                pt[chat][p]['pcov'] = set(coverage.get(physical,set()))
                pt[chat][p]['pst'] = set(stated.get(physical,set()))
                pt[chat][p]['est'] = set(stated.get(physical,set()))
                uncertain = mapping(doc.get('uncertain', {}))
                pt[chat][p]['dpend'] = delivery in uncertain
                if delivery in uncertain: vol[chat] = 'batched'
            for delivery, marker in mapping(doc.get('uncertain', {})).items():
                matches = [operation for operation in self.prepares
                           if operation['args']['chat'] == chat
                           and operation['args']['input'] == marker['input']
                           and delivery.startswith(operation['args']['harness']+':')]
                if not matches: self.fail('xPt.uncertain_delivery_identity')
                harnesses = {operation['args']['harness'] for operation in matches}
                if len(harnesses) != 1: self.fail('xPt.uncertain_delivery_identity')
                p = self.provider(next(iter(harnesses)))
                pt[chat][p]['dpend'] = True
                vol[chat] = 'batched'
        empty = dict(ty='none', src=no_thread, tgt=no_thread, sp=0, bp=0, hd=0, tp=no_provider,
                     st='none', strat='none', why='none', sel=aset([]), om=aset([]), hdr=False, dl='none', cr=False)
        xf = {i: deepcopy(empty) for i in range(1, maximum+1)}
        records = {}
        for doc in self.docs.values():
            for identity, record in mapping(doc['ledger']).items():
                if identity in records and records[identity] != record:
                    self.fail('xXf.replica_record_conflict')
                records[identity] = record
        if set(records) != self.seen_transfers:
            self.fail('xNx.unordered_final_records')
        for ordinal, identity in enumerate(self.transfer_order, 1):
            if identity not in records: self.fail('xXf.missing_record')
            record = records[identity]; receipt = self.lineage.get(identity)
            if receipt is None: self.fail('xXf.missing_kind_receipt')
            source, target = record['source'], record['target']
            selected = {self.ordinal(source, item) for item in record['selected']}
            covered = {self.ordinal(source, item['id']) for item in record['items']}
            observed = known(record.get('observed_packet'))
            header = known(observed.get('header')) if observed else None
            if record['status'] == 'Consumed' and (header is None or header['selected'] != len(selected)):
                self.fail('xXf.packet_header')
            if header is not None and header['omitted'] != len(covered-selected):
                self.fail('xXf.omitted_identity')
            prepare = self.transfer_prepares.get(identity)
            p = self.provider(prepare['args']['harness']) if prepare else no_provider
            status = {'Pending':'pending', 'ResolvedPortable':'resolved', 'ResolvedNative':'consumed',
                      'Consumed':'consumed', 'Failed':'failed', 'Superseded':'superseded'}.get(record['status'])
            if status is None: self.fail('xXf.status')
            strategy = 'none'
            if status in {'resolved', 'consumed'}:
                if prepare is None: self.fail('xXf.missing_prepare')
                strategy = {'fork':'portable_replay', 'subagent_spawn':'portable_replay',
                            'merge_back':'merge_delta', 'subagent_result':'result',
                            'provider_switch':'delta' if known(prepare['args']['provider_session']) not in {None,'none'} else 'full_summary'}[receipt['kind']]
            if record['status'] == 'ResolvedNative': self.fail('xXf.native_fork_receipt')
            if status not in {'pending', 'superseded'} and identity not in self.resolve_heads:
                self.fail('xXf.resolution_head')
            if status == 'superseded' and identity not in self.supersede_reasons:
                self.fail('xXf.supersede_reason')
            xf[ordinal] = dict(ty=receipt['kind'], src=source, tgt=target,
                sp=self.ordinal(source, record['point']), bp=self.ordinal(source, record['base']),
                hd=self.resolve_heads.get(identity,0), tp=p, st=status, strat=strategy, why='budget' if status=='failed' else self.supersede_reasons.get(identity,'none'),
                sel=aset(selected), om=aset(covered-selected), hdr=header is not None,
                dl=('inline' if (target,prepare['args']['input']) in done else 'pending') if status=='consumed' else 'none', cr=identity in self.crashed)
        for name, spent in [('CTMaxCrash',self.crashes), ('MaxStale',self.stales)]:
            if type(self.constants.get(name)) is not int or self.constants[name] < spent:
                self.fail('xCrashLeft.bound' if name=='CTMaxCrash' else 'xStaleLeft.bound')
        return dict(xMsgs=self.journal, xCreated=fn(created.items()), xForkOf=fn(lineage.items()),
            xKindOf=fn(kinds.items()), xRun=fn(runs.items()), xPt=fn((chat,fn(states.items()))
                for chat, states in {chat:{p:{**state, **{key:aset(state[key]) for key in ('pcov','pst','ecov','est')}}
                    for p,state in states.items()} for chat,states in pt.items()}.items()),
            xXf=fn(xf.items()), xNx=len(self.transfer_order), xVol=fn(vol.items()), xDup=duplicate,
            xAbort=fn(abort.items()), xCrashLeft=self.constants['CTMaxCrash']-self.crashes,
            xStaleLeft=self.constants['MaxStale']-self.stales)


def transfer_fields(prefix, context, root, *, image=None):
    if context.get('provenance', {}).get('kind') not in {'capture', 'verified_test'}:
        raise FullProjectionError(0, 'Context', 'constants.provenance')
    try:
        return TransferProjection(prefix, context, root).result()
    except (KeyError, TypeError, ValueError) as error:
        if isinstance(error, FullProjectionError): raise
        index = len(prefix)-1
        raise FullProjectionError(index, prefix[index].get('action','Unknown') if prefix else 'Init',
                                  'transfer.malformed_evidence') from error
