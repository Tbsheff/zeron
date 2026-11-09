-------------------- MODULE RuntimeTransferSeams --------------------
(* Captured exact durable operations and attributed source change journals. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC

Ids(items) == [i \in 1..Len(items) |-> items[i].id]
Members(seq) == {seq[i] : i \in 1..Len(seq)}
OrderedSubset(xs, ys) ==
  /\ Cardinality(Members(xs)) = Len(xs)
  /\ Members(xs) \subseteq Members(ys)
  /\ \A i,j \in 1..Len(xs) : i < j =>
       \A a,b \in 1..Len(ys) : ys[a] = xs[i] /\ ys[b] = xs[j] => a < b
Pinned(t) == [id |-> t.id, source |-> t.source, target |-> t.target,
              base |-> t.base, point |-> t.point, items |-> t.items]
WellFormedChat(s) ==
  /\ \A k \in DOMAIN s.ledger :
      LET t == s.ledger[k] IN
      /\ t.id = k
      /\ t.status \in {"Pending","ResolvedPortable","ResolvedNative","Consumed","Superseded","Failed"}
      /\ OrderedSubset(t.selected, Ids(t.items))
      /\ (t.status = "Consumed" => t.packet # "none")
      /\ (t.status = "Consumed" => \E d \in DOMAIN s.receipts : Members(t.selected) \subseteq s.receipts[d])
  /\ \A k \in DOMAIN s.committed :
       s.committed[k].status = "Consumed" =>
         k \in DOMAIN s.ledger /\ s.committed[k] = s.ledger[k]
WellFormedTransfer(s) == \A chat \in DOMAIN s : WellFormedChat(s[chat])

TransferInitial(s) ==
  /\ WellFormedTransfer(s)
  /\ \A chat \in DOMAIN s :
       /\ DOMAIN s[chat].ledger = {} /\ DOMAIN s[chat].committed = {}
       /\ DOMAIN s[chat].receipts = {} /\ DOMAIN s[chat].runs = {}
       /\ DOMAIN s[chat].uncertain = {}
       /\ DOMAIN s[chat].failures = {} /\ s[chat].rejected = {}
       /\ DOMAIN s[chat].providers = {} /\ s[chat].queue = <<>> /\ s[chat].entries = <<>>

ChatAdvances(old, new) ==
  /\ old.revision <= new.revision
  /\ DOMAIN old.ledger \subseteq DOMAIN new.ledger
  /\ DOMAIN old.receipts \subseteq DOMAIN new.receipts
  /\ DOMAIN old.runs \subseteq DOMAIN new.runs
  /\ DOMAIN old.failures \subseteq DOMAIN new.failures
  /\ old.rejected \subseteq new.rejected
  /\ \A k \in DOMAIN old.ledger :
       /\ Pinned(old.ledger[k]) = Pinned(new.ledger[k])
       /\ (old.ledger[k].status = "Consumed" => old.ledger[k] = new.ledger[k])
  /\ \A d \in DOMAIN old.receipts : old.receipts[d] \subseteq new.receipts[d]
  /\ \A k \in DOMAIN old.runs : old.runs[k] = new.runs[k]
  /\ \A k \in DOMAIN old.failures : old.failures[k] = new.failures[k]

TransferCheckpoint(before, next, chat) ==
  /\ WellFormedTransfer(next)
  /\ chat \in DOMAIN next
  /\ DOMAIN before \subseteq DOMAIN next
  /\ \A c \in DOMAIN before :
       IF c = chat THEN ChatAdvances(before[c], next[c]) ELSE before[c] = next[c]
  /\ \A c \in DOMAIN next \ DOMAIN before : c = chat

(* Named exact public operations. Initial tranche: explicit pin and binding.
   PrepareRun/RejectResume/merge queue completion remain fail-closed. *)
IndexOf(items, id) == CHOOSE i \in 1..Len(items) : items[i].id = id
PinRange(a) == SubSeq(a.source,
  IF a.base = "none" THEN 1 ELSE IndexOf(a.source,a.base)+1,
  IndexOf(a.source,a.point))
NoHeader == [present |-> FALSE, selected |-> 0, omitted |-> 0, bytes |-> 0,digest |-> "none"]
NoPacket == [present |-> FALSE,header |-> NoHeader,chunks |-> <<>>,remaining |-> 0]
PinRecord(a) == [id |-> a.id, source |-> a.source_chat, target |-> a.target_chat,
  base |-> a.base, point |-> a.point, items |-> PinRange(a), status |-> "Pending",
  packet |-> "none", packet_bytes |-> 0, header_base |-> a.header_base, selected |-> <<>>,
  observed_packet |-> NoPacket, expected_header_digest |-> "none"]
StableDelivery(old,new) ==
  /\ old.receipts = new.receipts /\ old.runs = new.runs
  /\ old.rejected = new.rejected /\ old.failures = new.failures
  /\ old.uncertain = new.uncertain
PinOperation(old,new,a) ==
  /\ a.target_local /\ a.source_stable
  /\ old.revision <= new.revision
  /\ StableDelivery(old,new) /\ old.committed = new.committed
  /\ old.providers = new.providers /\ old.queue = new.queue /\ old.entries = new.entries
  /\ a.point \in Members(Ids(a.source))
  /\ (a.base = "none" \/ a.base \in Members(Ids(a.source)))
  /\ (a.base # "none" => IndexOf(a.source,a.base) <= IndexOf(a.source,a.point))
  /\ IF a.id \in DOMAIN old.ledger THEN new.ledger = old.ledger
     ELSE new.ledger = [k \in DOMAIN old.ledger \cup {a.id} |->
       IF k = a.id THEN PinRecord(a)
       ELSE IF old.ledger[k].target = a.target_chat /\ old.ledger[k].status = "Pending"
            THEN [old.ledger[k] EXCEPT !.status = "Superseded"]
            ELSE old.ledger[k]]

GetSet(f,k) == IF k \in DOMAIN f THEN f[k] ELSE {}
EmptyPacketDigest == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
Without(f,k) == [x \in DOMAIN f \ {k} |-> f[x]]
Unconfirmed(s,delivery,input,strict) ==
  IF strict THEN s.uncertain @@ (delivery :> [input |-> input,retry_same_input |-> FALSE])
  ELSE IF delivery \in DOMAIN s.uncertain THEN s.uncertain
  ELSE s.uncertain @@ (delivery :> [input |-> input,retry_same_input |-> FALSE])
BindOperation(old,new,a) ==
  LET fresh == a.harness \o ":fresh:" \o a.input
      replacement == a.harness \o ":replacement:" \o a.input
      actual == a.harness \o ":" \o a.session
      rkey == replacement \o ":" \o a.input
      fkey == fresh \o ":" \o a.input
      akey == actual \o ":" \o a.input
      addReceipt == fresh \in DOMAIN old.receipts \/ replacement \in DOMAIN old.receipts
      addRun == fkey \in DOMAIN old.runs \/ rkey \in DOMAIN old.runs
  IN
  /\ a.validated_local /\ a.session # ""
  /\ old.revision <= new.revision
  /\ old.ledger = new.ledger /\ old.committed = new.committed
  /\ old.rejected = new.rejected /\ old.failures = new.failures
  /\ old.queue = new.queue /\ old.entries = new.entries
  /\ new.providers = [h \in DOMAIN old.providers \cup {a.harness} |->
       IF h = a.harness THEN [session |-> a.session, cwd_digest |-> a.cwd_digest, seen |-> a.seen]
       ELSE old.providers[h]]
  /\ new.receipts = IF ~addReceipt THEN old.receipts ELSE
       [k \in DOMAIN old.receipts \cup {actual} |-> IF k = actual
         THEN GetSet(old.receipts,actual) \cup GetSet(old.receipts,fresh) \cup GetSet(old.receipts,replacement)
         ELSE old.receipts[k]]
  /\ new.runs = IF ~addRun \/ akey \in DOMAIN old.runs THEN old.runs ELSE
       old.runs @@ (akey :> (IF rkey \in DOMAIN old.runs THEN old.runs[rkey] ELSE old.runs[fkey]))
  /\ a.provider_event \in {"SessionStarted","Done"}
  /\ new.uncertain = IF a.provider_event = "Done" /\ a.provider_success
       THEN [delivery \in {d \in DOMAIN old.uncertain : d # actual /\ d # fresh /\ d # replacement /\
          ~(d \in DOMAIN new.receipts /\ new.receipts[d] # {} /\ new.receipts[d] \subseteq GetSet(new.receipts,actual))} |-> old.uncertain[delivery]]
       ELSE IF akey \in DOMAIN new.runs /\ new.runs[akey] # EmptyPacketDigest
            THEN Unconfirmed(old,actual,a.input,a.provider_event="SessionStarted")
            ELSE old.uncertain


(* Selection is computed from captured source lengths and delivered IDs.
   header_base and render_bytes are independently derived by the adapter from
   the documented header and UTF-8 ID/role lengths, never from a post packet. *)
NatSub(a,b) == IF a >= b THEN a-b ELSE 0
Min2(a,b) == IF a < b THEN a ELSE b
Max2(a,b) == IF a > b THEN a ELSE b
ContextLimit(a) == Min2(16000,
  NatSub(NatSub(NatSub(a.window,a.used_tokens),a.prompt_bytes + a.images*8192 + a.attachments*4096),
         Max2(16000,(a.window+3) \div 4)))
HeaderBytes(t,selected,omitted) == t.header_base + Len(ToString(selected)) + Len(ToString(omitted))
Candidates(t,seen) == SelectSeq(t.items,LAMBDA item: item.id \notin seen)
RoleIndices(items,role) == {i \in 1..Len(items): items[i].role = role}
FirstIx(S) == IF S = {} THEN 0 ELSE CHOOSE n \in S: \A k \in S: n <= k
LastIx(S) == IF S = {} THEN 0 ELSE CHOOSE n \in S: \A k \in S: n >= k
AnchorRaw(items) == <<LastIx(RoleIndices(items,"user")),LastIx(RoleIndices(items,"assistant")),FirstIx(RoleIndices(items,"user"))>>
AnchorOrder(items) == LET raw == AnchorRaw(items)
  keep == SelectSeq(<<1,2,3>>,LAMBDA i: raw[i] # 0 /\ \A j \in 1..(i-1):raw[j] # raw[i])
  IN [i \in 1..Len(keep) |-> raw[keep[i]]]
Priority(items) == AnchorOrder(items) \o
  SelectSeq([i \in 1..Len(items) |-> Len(items)+1-i],
            LAMBDA i:i \notin Members(AnchorOrder(items)))
RECURSIVE ItemBytes(_)
ItemBytes(items) == IF Len(items)=0 THEN 0 ELSE Head(items).render_bytes + ItemBytes(Tail(items))
RECURSIVE Greedy(_,_,_,_)
Greedy(items,priority,left,index) ==
  IF index > Len(priority) THEN {}
  ELSE LET n == priority[index] cost == items[n].render_bytes IN
    IF cost <= left THEN {n} \cup Greedy(items,priority,left-cost,index+1)
    ELSE Greedy(items,priority,left,index+1)
Selection(t,seen,limit) ==
  LET candidates == Candidates(t,seen)
      anchors == AnchorOrder(candidates)
      required == ItemBytes([i \in 1..Len(anchors) |-> candidates[anchors[i]]])
      reserve == HeaderBytes(t,Len(candidates),Len(candidates))
      left == NatSub(limit,reserve)
      chosen == Greedy(candidates,Priority(candidates),left,1)
      selected == SelectSeq(candidates,LAMBDA item:item.id \in {candidates[i].id:i \in chosen})
  IN [failed |-> limit=0 \/ reserve>limit \/ required>left,
      selected |-> Ids(selected),
      bytes |-> IF Len(candidates)=0 THEN 0 ELSE HeaderBytes(t,Len(selected),Len(candidates)-Len(selected)) + ItemBytes(selected)]

Delivery(old,a) == a.harness \o ":" \o
  (IF a.provider_session # "none" THEN a.provider_session
   ELSE (IF a.harness \o ":" \o a.input \in old.rejected THEN "replacement:" ELSE "fresh:") \o a.input)
RunKey(old,a) == Delivery(old,a) \o ":" \o a.input
Pending(s) == {k \in DOMAIN s.ledger:s.ledger[k].status \in {"Pending","ResolvedPortable"}}
WithPin(s,a) == IF a.id \in DOMAIN s.ledger THEN s ELSE
  [s EXCEPT !.ledger = [k \in DOMAIN s.ledger \cup {a.id} |->
    IF k = a.id THEN PinRecord(a)
    ELSE IF s.ledger[k].target = a.target_chat /\ s.ledger[k].status = "Pending"
         THEN [s.ledger[k] EXCEPT !.status = "Superseded"] ELSE s.ledger[k]]]
PreparedPin(old,a) ==
  IF a.needs_history /\ RunKey(old,a) \notin DOMAIN old.runs /\ Pending(old) = {} /\ a.switch.present
  THEN WithPin(old,a.switch.pin) ELSE old
SameMetadata(left,right) == [left EXCEPT !.revision = 0] = [right EXCEPT !.revision = 0]
HasEntry(s,id) == \E i \in 1..Len(s.entries):s.entries[i].id = id
WithMarker(s,id) == IF HasEntry(s,id) THEN s ELSE
  [s EXCEPT !.entries = Append(@,[id |-> id,role |-> "system",complete |-> TRUE])]

IntactPacket(t,omitted) ==
  LET p == t.observed_packet
      chosen == SelectSeq(t.items,LAMBDA item:item.id \in Members(t.selected))
  IN /\ p.present /\ p.remaining = 0
     /\ IF t.packet_bytes = 0 THEN ~p.header.present /\ p.chunks = <<>> /\ t.selected = <<>>
        ELSE /\ p.header.present
             /\ p.header.selected = Len(t.selected) /\ p.header.omitted = omitted
             /\ p.header.bytes = HeaderBytes(t,Len(t.selected),omitted)
             /\ p.header.digest = t.expected_header_digest
             /\ Len(p.chunks) = Len(chosen)
             /\ \A i \in 1..Len(chosen):
                  /\ p.chunks[i].id = chosen[i].id
                  /\ p.chunks[i].bytes = chosen[i].render_bytes
                  /\ p.chunks[i].digest = chosen[i].rendered_digest

ConsumeOperation(old,new,a) ==
  LET s == PreparedPin(old,a) key == RunKey(old,a) delivery == Delivery(old,a)
  IN
  /\ old.revision <= new.revision
  /\ CASE key \in DOMAIN s.failures -> ~a.succeeded /\ SameMetadata(s,new)
    [] key \in DOMAIN s.runs -> a.succeeded /\ SameMetadata(s,new)
    [] Pending(s) = {} -> a.succeeded /\ SameMetadata(s,new)
    [] OTHER ->
      /\ Cardinality(Pending(s)) = 1
      /\ LET id == CHOOSE k \in Pending(s):TRUE
             t == s.ledger[id]
             sel == Selection(t,GetSet(s.receipts,delivery),ContextLimit(a))
         IN
         IF t.status = "Pending" /\ sel.failed THEN
           /\ ~a.succeeded
           /\ SameMetadata(new,WithMarker([s EXCEPT !.ledger[id].status = "Failed",
                !.failures = @ @@ (key :> a.budget_failure_hash)],"handoff-error:" \o id))
         ELSE
           LET ids == IF t.status = "Pending" THEN sel.selected ELSE t.selected
               bytes == IF t.status = "Pending" THEN sel.bytes ELSE t.packet_bytes
               packet == new.ledger[id].packet
               nt == [t EXCEPT !.status = "Consumed", !.selected = ids,
                              !.packet = packet, !.packet_bytes = bytes,
                              !.observed_packet = new.ledger[id].observed_packet,
                              !.expected_header_digest = new.ledger[id].expected_header_digest]
               ledger == [s.ledger EXCEPT ![id] = nt]
               receipts == [d \in DOMAIN s.receipts \cup {delivery} |->
                   IF d = delivery THEN GetSet(s.receipts,d) \cup Members(ids) ELSE s.receipts[d]]
               result == [s EXCEPT !.ledger = ledger, !.committed = ledger,
                       !.receipts = receipts, !.runs = @ @@ (key :> packet)]
           IN /\ a.succeeded /\ packet # "none"
              /\ IntactPacket(nt,Len(Candidates(t,GetSet(s.receipts,delivery)))-Len(ids))
              /\ SameMetadata(new,WithMarker(result,"handoff:" \o id))

PrepareOperation(old,new,a,succeeded) ==
  LET plain == [new EXCEPT !.uncertain = old.uncertain]
      delivery == Delivery(old,a)
      key == RunKey(old,a)
      marked == key \in DOMAIN new.runs /\ new.runs[key] # EmptyPacketDigest
  IN
  /\ a.succeeded = succeeded
  /\ IF a.skip_consume THEN succeeded /\ SameMetadata(old,new)
     ELSE /\ ConsumeOperation(old,plain,a)
          /\ new.uncertain = IF marked THEN Unconfirmed(old,delivery,a.input,FALSE) ELSE old.uncertain

RejectOperation(old,new,a) ==
  /\ a.explicit_rejection /\ a.rejected_session # "none"
  /\ old.revision <= new.revision
  /\ LET pinned == IF a.replacement.present THEN WithPin(old,a.replacement.pin) ELSE old
     IN SameMetadata(new,[pinned EXCEPT !.rejected = @ \cup {a.harness \o ":" \o a.input},
       !.uncertain = Unconfirmed(old,a.harness \o ":" \o a.rejected_session,a.input,TRUE)])
MergeOperation(old,new,a) ==
  LET pinned == WithPin(old,a)
      exists == HasEntry(old,a.queue_id) \/ \E i \in 1..Len(old.queue):old.queue[i].id = a.queue_id
      queued == IF exists THEN pinned ELSE [pinned EXCEPT !.queue = Append(@,
        [id |-> a.queue_id,hold_for_turn_end |-> TRUE,gated |-> FALSE,issued_by |-> a.issuer])]
      expected == WithMarker(queued,"merge-marker:" \o a.id)
      pinOnly == [new EXCEPT !.queue = old.queue, !.entries = old.entries]
  IN /\ a.target_local /\ a.hold_for_turn_end
     /\ PinOperation(old,pinOnly,a)
     /\ SameMetadata(new,expected)

JournalAppend(old,new) ==
  /\ Len(old.entries) <= Len(new.entries)
  /\ SubSeq(new.entries,1,Len(old.entries)) = old.entries
QueueAdvance(old,new) ==
  \/ new.queue = old.queue
  \/ Len(new.queue) = Len(old.queue)+1 /\ SubSeq(new.queue,1,Len(old.queue))=old.queue
  \/ Len(old.queue)>0 /\ new.queue = Tail(old.queue)
AttributedBind(old,new,a,journal) ==
  /\ (Len(journal)=0 => SameMetadata(old,new))
  /\ \A i \in 1..Len(journal):
       LET step == journal[i] IN
       /\ step.kind \in {"NoTransferChange","JournalChange","QueueChange","ProviderChange","MixedChange"}
       /\ (i=1 => step.before=old)
       /\ (i>1 => step.before=journal[i-1].after)
       /\ (i=Len(journal) => step.after=new)
       /\ JournalAppend(step.before,step.after)
       /\ QueueAdvance(step.before,step.after)
  /\ BindOperation(old,[new EXCEPT !.entries=old.entries,!.queue=old.queue],a)

TransferOperation(old,new,operation,args,succeeded,journal) ==
  /\ WellFormedChat(old) /\ WellFormedChat(new)
  /\ CASE operation \in {"ForkPin","TransferPin"} -> succeeded /\ PinOperation(old,new,args)
       [] operation = "BindProvider" -> succeeded /\ AttributedBind(old,new,args,journal)
       [] operation = "PrepareRun" -> PrepareOperation(old,new,args,succeeded)
       [] operation = "RejectResume" -> succeeded /\ RejectOperation(old,new,args)
       [] operation = "MergeBackPinAndQueue" -> succeeded /\ MergeOperation(old,new,args)
       [] OTHER -> FALSE

TransferApply(before, next, event) ==
  CASE event.act = "TransferCheckpoint" -> TransferCheckpoint(before, next, event.args.chat)
    [] event.act \in {"TransferPreserve","TransferCatalog"} -> before = next
    [] event.act = "TransferOperation" ->
         /\ TransferCheckpoint(before, next, event.args.chat)
         /\ next[event.args.chat] = event.args.after
         /\ TransferOperation(event.args.before, event.args.after, event.args.operation, event.args.args, event.args.succeeded,event.args.journal)
    [] OTHER -> FALSE
====
