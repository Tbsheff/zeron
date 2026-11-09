---------------- MODULE RuntimePlanRetainedTransferKernel ----------------
EXTENDS Naturals,Sequences,FiniteSets,TLC
CONSTANTS Chats,Harnesses,Inputs,Requests,Pins,PacketIds,SessionIds,ReceiptIds,
 DocumentIds,MessageIds,NoMessage,NoRequest,NoPin,NoPacket,MaxCTRun,MaxCTAttempt,Limits
VARIABLE ct
vars == <<ct>>
Empty == [k \in {} |-> k]
Members(s) == {s[i]:i \in 1..Len(s)}
Use(q,r,a) == <<q,r,a>>
RequestKey(c,h,i) == [chat |-> c,harness |-> h,input |-> i]
AllUses == UNION {{Use(q,u[1],u[2]):u \in ct.uses[q]}:q \in DOMAIN ct.uses}
Ordered(xs,ys) ==
 /\ Cardinality(Members(xs))=Len(xs) /\ Members(xs) \subseteq Members(ys)
 /\ \A i,j \in 1..Len(xs):i<j => \A x,y \in 1..Len(ys):ys[x]=xs[i] /\ ys[y]=xs[j] => x<y
Position(c,m) == IF m=NoMessage THEN 0 ELSE CHOOSE i \in 1..Len(ct.journal[c]):ct.journal[c][i].id=m
JournalIds(c) == [i \in 1..Len(ct.journal[c])|->ct.journal[c][i].id]
ReadRange(c,base,point) == SelectSeq(JournalIds(c),LAMBDA m:Position(c,m)>Position(c,base) /\ Position(c,m)<=Position(c,point))
RoleIndices(items,role) == {i \in 1..Len(items):items[i].role=role}
First(S) == IF S={} THEN 0 ELSE CHOOSE i \in S:\A j \in S:i<=j
Last(S) == IF S={} THEN 0 ELSE CHOOSE i \in S:\A j \in S:i>=j
AnchorOrder(items) ==
 LET raw==<<Last(RoleIndices(items,"user")),Last(RoleIndices(items,"assistant")),First(RoleIndices(items,"user"))>>
     keep==SelectSeq(<<1,2,3>>,LAMBDA i:raw[i]#0 /\ \A j \in 1..(i-1):raw[j]#raw[i])
 IN [i \in 1..Len(keep)|->raw[keep[i]]]
RECURSIVE Greedy(_,_,_,_)
Greedy(items,order,left,n) ==
 IF n>Len(order) THEN {} ELSE LET i==order[n] IN
 IF items[i].bytes<=left THEN {i}\cup Greedy(items,order,left-items[i].bytes,n+1)
 ELSE Greedy(items,order,left,n+1)
PacketChoice(pin,limit) ==
 LET r==ct.pinned[pin]
     items==[i \in 1..Len(r.items)|->ct.journal[r.source][Position(r.source,r.items[i])]]
     anchors==AnchorOrder(items)
     priority==anchors\o SelectSeq([i \in 1..Len(items)|->Len(items)+1-i],LAMBDA i:i \notin Members(anchors))
     reserve==4+Len(ToString(Len(items)))+Len(ToString(Len(items)))
     left==IF limit>=reserve THEN limit-reserve ELSE 0
     chosen==Greedy(items,priority,left,1)
     selected==SelectSeq(r.items,LAMBDA id:Position(r.source,id) \in {Position(r.source,items[i].id):i \in chosen})
 IN [ok |-> limit>=reserve /\ Members(anchors) \subseteq chosen,selected |-> selected]
Init == ct=[open |-> {},loaded |-> {},disk |-> Empty,prepared |-> Empty,
 latest |-> [c \in Chats|->NoRequest],turn |-> [c \in Chats|->[run |-> 0,attempt |-> 0,live |-> FALSE]],
 pinned |-> Empty,sessions |-> Empty,done |-> Empty,acks |-> Empty,uncertain |-> {},uses |-> Empty,
 catalogKnown |-> FALSE,documents |-> Empty,packets |-> Empty,logicalUses |-> {},
 journal |-> [c \in Chats|-> <<>>],inputMessages |-> Empty]
OpenLogical(c) ==
 /\ c \in Chats\ct.open /\ ct'=[ct EXCEPT !.open=@\cup{c}]
LoadDocument(c) ==
 /\ c \in Chats\ct.loaded /\ ct'=[ct EXCEPT !.loaded=@\cup{c}]
SaveDocument(c,d) ==
 /\ c \in ct.loaded /\ d \in DocumentIds
 /\ d \in DOMAIN ct.documents => ct.documents[d]=c
 /\ ct'=[ct EXCEPT !.disk=(c:>d)@@ @,!.documents=(d:>c)@@ @]
ObserveSavedDocuments(catalog) ==
 /\ DOMAIN catalog \subseteq Chats /\ \A c \in DOMAIN catalog:catalog[c] \in DocumentIds
 /\ Cardinality({catalog[c]:c \in DOMAIN catalog})=Cardinality(DOMAIN catalog)
 /\ DOMAIN ct.disk \subseteq DOMAIN catalog
 /\ \A c \in DOMAIN catalog:catalog[c] \in DOMAIN ct.documents => ct.documents[catalog[c]]=c
 /\ ct'=[ct EXCEPT !.disk=catalog,!.loaded=@\cup DOMAIN catalog,!.catalogKnown=TRUE,
     !.documents=[d \in DOMAIN ct.documents\cup{catalog[c]:c \in DOMAIN catalog}|->
       IF d \in DOMAIN ct.documents THEN ct.documents[d] ELSE CHOOSE c \in DOMAIN catalog:catalog[c]=d]]
LogicalStart(c,r,a) ==
 /\ c \in ct.open /\ ~ct.turn[c].live /\ r=ct.turn[c].run+1 /\ r \in 1..MaxCTRun /\ a=1
 /\ ct'=[ct EXCEPT !.turn[c]=[run |-> r,attempt |-> a,live |-> TRUE],!.logicalUses=@\cup{<<c,r,a>>}]
LogicalAttempt(c,r,a) ==
 /\ c \in ct.open /\ ct.turn[c].live /\ r=ct.turn[c].run /\ a=ct.turn[c].attempt+1 /\ a \in 1..MaxCTAttempt
 /\ ct'=[ct EXCEPT !.turn[c].attempt=a,!.logicalUses=@\cup{<<c,r,a>>}]
LogicalFinish(c,r,a) ==
 /\ c \in ct.open /\ ct.turn[c].live /\ ct.turn[c].run=r /\ ct.turn[c].attempt=a
 /\ ct'=[ct EXCEPT !.turn[c].live=FALSE]
AppendUser(c,i,m) ==
 /\ c \in ct.loaded /\ i \in Inputs /\ m \in MessageIds /\ i=m
 /\ i \notin DOMAIN ct.inputMessages
 /\ \A other \in Chats:m \notin Members(JournalIds(other))
 /\ ct'=[ct EXCEPT !.journal[c]=Append(@,[id |-> m,role |-> "user",bytes |-> 4]),
     !.inputMessages=(i:>[chat |-> c,message |-> m])@@ @]
AppendAssistant(c,m) ==
 /\ c \in ct.loaded /\ m \in MessageIds
 /\ \A other \in Chats:m \notin Members(JournalIds(other))
 /\ ct'=[ct EXCEPT !.journal[c]=Append(@,[id |-> m,role |-> "assistant",bytes |-> 4])]
PinJournal(p,source,target,base,point) ==
 /\ p \in Pins\DOMAIN ct.pinned /\ source \in ct.loaded /\ target \in ct.loaded
 /\ point \in Members(JournalIds(source)) /\ base \in Members(JournalIds(source))\cup{NoMessage}
 /\ Position(source,base)<=Position(source,point)
 /\ ct'=[ct EXCEPT !.pinned=(p:>[source |-> source,target |-> target,base |-> base,point |-> point,
     items |-> ReadRange(source,base,point)])@@ @]
BuildPacket(packet,pin,limit) ==
 /\ packet \in PacketIds /\ pin \in DOMAIN ct.pinned /\ limit \in Limits
 /\ PacketChoice(pin,limit).ok
 /\ LET body==[pin |-> pin,selected |-> PacketChoice(pin,limit).selected] IN
    /\ packet \in DOMAIN ct.packets => ct.packets[packet]=body
    /\ ct'=[ct EXCEPT !.packets=(packet:>body)@@ @]
PrepareRequest(q,c,h,i,p,packet,selected,r,a) ==
 /\ q \in Requests /\ q=RequestKey(c,h,i)
 /\ c \in ct.loaded /\ h \in Harnesses /\ i \in DOMAIN ct.inputMessages
 /\ ct.inputMessages[i].chat=c /\ <<c,r,a>> \in ct.logicalUses
 /\ IF packet=NoPacket THEN p=NoPin /\ selected= <<>>
    ELSE packet \in DOMAIN ct.packets /\ ct.packets[packet]=[pin |-> p,selected |-> selected] /\ ct.pinned[p].target=c
 /\ \A other \in Chats\{c}:ct.latest[other]#q
 /\ LET u==Use(q,r,a)
        call==[chat |-> c,harness |-> h,input |-> i,run |-> r,attempt |-> a,pin |-> p,packet |-> packet]
    IN /\ (q \in DOMAIN ct.prepared => ct.prepared[q].chat=c /\ ct.prepared[q].harness=h /\ ct.prepared[q].input=i /\ ct.prepared[q].pin=p /\ ct.prepared[q].packet=packet)
       /\ ct'=[ct EXCEPT !.prepared=IF q \in DOMAIN @ THEN @ ELSE (q:>call)@@ @,
          !.latest[c]=IF ct.turn[c].run=r /\ ct.turn[c].attempt=a THEN q ELSE @,!.uses=(q:>((IF q \in DOMAIN @ THEN @[q] ELSE {})\cup{<<r,a>>}))@@ @,
          !.uncertain=IF u \in AllUses \/ packet=NoPacket THEN @ ELSE @\cup{u}]
BindSession(q,r,a,session) ==
 /\ Use(q,r,a) \in AllUses /\ session \in SessionIds
 /\ Use(q,r,a) \in DOMAIN ct.sessions => ct.sessions[Use(q,r,a)]=session
 /\ ct'=[ct EXCEPT !.sessions=(Use(q,r,a):>session)@@ @]
RecordOutcome(q,r,a,outcome) ==
 /\ Use(q,r,a) \in AllUses /\ outcome \in {"completed","failed","interrupted","transient","usage_limit"}
 /\ Use(q,r,a) \in DOMAIN ct.done => ct.done[Use(q,r,a)]=outcome
 /\ ct'=[ct EXCEPT !.done=(Use(q,r,a):>outcome)@@ @]
SaveConfirmation(id,q,packet,session,input,r,a,document) ==
 /\ id \in ReceiptIds /\ q \in DOMAIN ct.prepared /\ packet=ct.prepared[q].packet /\ input=ct.prepared[q].input
 /\ Use(q,r,a) \in AllUses /\ Use(q,r,a) \in DOMAIN ct.sessions /\ ct.sessions[Use(q,r,a)]=session
 /\ Use(q,r,a) \in DOMAIN ct.done /\ ct.done[Use(q,r,a)]="completed"
 /\ document \in DOMAIN ct.documents /\ ct.documents[document]=ct.prepared[q].chat
 /\ LET saved==[request |-> q,chat |-> ct.prepared[q].chat,harness |-> ct.prepared[q].harness,
     input |-> input,run |-> r,attempt |-> a,packet |-> packet,session |-> session,document |-> document]
    IN /\ id \in DOMAIN ct.acks => ct.acks[id]=saved
       /\ ct'=[ct EXCEPT !.acks=(id:>saved)@@ @,!.uncertain=@\{Use(q,r,a)}]
CTImage == [logicalCreated |-> ct.open,loaded |-> ct.loaded,durable |-> ct.disk,requests |-> ct.prepared,
 current |-> ct.latest,logical |-> ct.turn,ranges |-> ct.pinned,bindings |-> ct.sessions,outcomes |-> ct.done,
 receipts |-> ct.acks,uncertain |-> ct.uncertain,uses |-> ct.uses,catalogKnown |-> ct.catalogKnown,
 savedDocuments |-> ct.documents,packets |-> ct.packets,logicalUses |-> ct.logicalUses]
PreparedFor(c,r,a,origin) ==
 \E q \in DOMAIN ct.prepared:ct.prepared[q].chat=c /\ ct.prepared[q].input=origin /\ <<r,a>> \in ct.uses[q]
RetainedImage == CTImage
Next ==
 \/ \E c \in Chats:OpenLogical(c) \/ LoadDocument(c)
 \/ \E domain \in SUBSET Chats:\E catalog \in [domain -> DocumentIds]:ObserveSavedDocuments(catalog)
 \/ \E c \in Chats,d \in DocumentIds:SaveDocument(c,d)
 \/ \E c \in Chats,r \in 1..MaxCTRun,a \in 1..MaxCTAttempt:LogicalStart(c,r,a) \/ LogicalAttempt(c,r,a) \/ LogicalFinish(c,r,a)
 \/ \E c \in Chats,i \in Inputs,m \in MessageIds:AppendUser(c,i,m)
 \/ \E c \in Chats,m \in MessageIds:AppendAssistant(c,m)
 \/ \E p \in Pins,s,t \in Chats,b \in MessageIds\cup{NoMessage},w \in MessageIds:PinJournal(p,s,t,b,w)
 \/ \E packet \in PacketIds,p \in Pins,l \in Limits:BuildPacket(packet,p,l)
 \/ \E q \in Requests,c \in Chats,h \in Harnesses,i \in Inputs,p \in Pins\cup{NoPin},packet \in PacketIds\cup{NoPacket},r \in 1..MaxCTRun,a \in 1..MaxCTAttempt:
      PrepareRequest(q,c,h,i,p,packet,IF packet=NoPacket THEN <<>> ELSE IF packet \in DOMAIN ct.packets THEN ct.packets[packet].selected ELSE <<>>,r,a)
 \/ \E q \in Requests,r \in 1..MaxCTRun,a \in 1..MaxCTAttempt,s \in SessionIds:BindSession(q,r,a,s)
 \/ \E q \in Requests,r \in 1..MaxCTRun,a \in 1..MaxCTAttempt,o \in {"completed","failed","interrupted","transient","usage_limit"}:RecordOutcome(q,r,a,o)
 \/ \E id \in ReceiptIds,q \in Requests,packet \in PacketIds\cup{NoPacket},s \in SessionIds,i \in Inputs,
       r \in 1..MaxCTRun,a \in 1..MaxCTAttempt,d \in DocumentIds:SaveConfirmation(id,q,packet,s,i,r,a,d)
Spec == Init /\ [][Next]_vars
====
