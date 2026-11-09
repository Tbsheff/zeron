---------------- MODULE RuntimeTransferReportingSpec ----------------
EXTENDS Naturals,Sequences,FiniteSets,TLC
CONSTANTS Chats,Harnesses,Inputs,Requests,Pins,PacketIds,SessionIds,ReceiptIds,
 DocumentIds,MessageIds,NoMessage,NoRequest,NoPin,NoPacket,MaxCTRun,MaxCTAttempt
VARIABLES logicalCreated,loaded,durable,requests,current,logical,ranges,
 bindings,outcomes,receipts,uncertain,uses,catalogKnown,savedDocuments,packets,logicalUses
ctVars == <<logicalCreated,loaded,durable,requests,current,logical,ranges,
 bindings,outcomes,receipts,uncertain,uses,catalogKnown,savedDocuments,packets,logicalUses>>
Empty == [k \in {} |-> k]
Use(q,r,a) == <<q,r,a>>
Members(xs) == {xs[i]:i \in 1..Len(xs)}
OrderedSubset(xs,ys) ==
 /\ Cardinality(Members(xs))=Len(xs)
 /\ Members(xs) \subseteq Members(ys)
 /\ \A i,j \in 1..Len(xs):i<j =>
      \A x,y \in 1..Len(ys):ys[x]=xs[i] /\ ys[y]=xs[j] => x<y
AllUses == UNION {{Use(q,u[1],u[2]):u \in uses[q]}:q \in DOMAIN uses}
BoundedSequences == UNION {[1..n -> MessageIds]:n \in 0..Cardinality(MessageIds)}
CurrentUse(q,r,a) ==
 \E c \in Chats:current[c]=q /\ logical[c].live /\ logical[c].run=r /\ logical[c].attempt=a
Init ==
 /\ logicalCreated={} /\ loaded={} /\ durable=Empty /\ requests=Empty
 /\ current=[c \in Chats |-> NoRequest]
 /\ logical=[c \in Chats |-> [run |-> 0,attempt |-> 0,live |-> FALSE]]
 /\ ranges=Empty /\ bindings=Empty /\ outcomes=Empty /\ receipts=Empty
 /\ uncertain={} /\ uses=Empty /\ catalogKnown=FALSE /\ savedDocuments=Empty /\ packets=Empty /\ logicalUses={}
OpenLogical(c) ==
 /\ c \in Chats\logicalCreated
 /\ logicalCreated'=logicalCreated\cup{c}
 /\ UNCHANGED <<loaded,durable,requests,current,logical,ranges,bindings,outcomes,receipts,uncertain,uses,catalogKnown,savedDocuments,packets,logicalUses>>
LoadDocument(c) ==
 /\ c \in Chats\loaded /\ loaded'=loaded\cup{c}
 /\ UNCHANGED <<logicalCreated,durable,requests,current,logical,ranges,bindings,outcomes,receipts,uncertain,uses,catalogKnown,savedDocuments,packets,logicalUses>>
ObserveSavedDocuments(catalog) ==
 /\ DOMAIN catalog \subseteq Chats
 /\ \A c \in DOMAIN catalog:catalog[c] \in DocumentIds
 /\ Cardinality({catalog[c]:c \in DOMAIN catalog})=Cardinality(DOMAIN catalog)
 /\ DOMAIN durable \subseteq DOMAIN catalog
 /\ \A c \in DOMAIN catalog:catalog[c] \in DOMAIN savedDocuments => savedDocuments[catalog[c]]=c
 /\ durable'=catalog /\ loaded'=loaded\cup DOMAIN catalog /\ catalogKnown'=TRUE
 /\ savedDocuments'=[d \in DOMAIN savedDocuments\cup{catalog[c]:c \in DOMAIN catalog} |->
      IF d \in DOMAIN savedDocuments THEN savedDocuments[d] ELSE CHOOSE c \in DOMAIN catalog:catalog[c]=d]
 /\ UNCHANGED <<logicalCreated,requests,current,logical,ranges,bindings,outcomes,receipts,uncertain,uses,packets,logicalUses>>
SaveDocument(c,d) ==
 /\ c \in loaded /\ d \in DocumentIds
 /\ d \in DOMAIN savedDocuments => savedDocuments[d]=c
 /\ durable'=(c:>d)@@durable /\ savedDocuments'=(d:>c)@@savedDocuments
 /\ UNCHANGED <<logicalCreated,loaded,requests,current,logical,ranges,bindings,outcomes,receipts,uncertain,uses,catalogKnown,packets,logicalUses>>
LogicalStart(c,r,a) ==
 /\ c \in logicalCreated /\ ~logical[c].live
 /\ r=logical[c].run+1 /\ r \in 1..MaxCTRun /\ a=1
 /\ logical'=[logical EXCEPT ![c]=[run |-> r,attempt |-> a,live |-> TRUE]]
 /\ logicalUses'=logicalUses\cup{<<c,r,a>>}
 /\ UNCHANGED <<logicalCreated,loaded,durable,requests,current,ranges,bindings,outcomes,receipts,uncertain,uses,catalogKnown,savedDocuments,packets>>
LogicalAttempt(c,r,a) ==
 /\ c \in logicalCreated /\ logical[c].live
 /\ r=logical[c].run /\ a=logical[c].attempt+1 /\ a \in 1..MaxCTAttempt
 /\ logical'=[logical EXCEPT ![c]=[run |-> r,attempt |-> a,live |-> TRUE]]
 /\ logicalUses'=logicalUses\cup{<<c,r,a>>}
 /\ UNCHANGED <<logicalCreated,loaded,durable,requests,current,ranges,bindings,outcomes,receipts,uncertain,uses,catalogKnown,savedDocuments,packets>>
LogicalFinish(c,r,a) ==
 /\ c \in logicalCreated /\ logical[c].live /\ logical[c].run=r /\ logical[c].attempt=a
 /\ logical'=[logical EXCEPT ![c].live=FALSE]
 /\ UNCHANGED <<logicalCreated,loaded,durable,requests,current,ranges,bindings,outcomes,receipts,uncertain,uses,catalogKnown,savedDocuments,packets,logicalUses>>
PinRange(p,range) ==
 /\ p \in Pins /\ range.source \in Chats /\ range.target \in Chats
 /\ range.base \in MessageIds\cup{NoMessage} /\ range.point \in MessageIds
 /\ Members(range.items) \subseteq MessageIds
 /\ Cardinality(Members(range.items))=Len(range.items)
 /\ p \in DOMAIN ranges => ranges[p]=range
 /\ ranges'=ranges@@(p:>range)
 /\ UNCHANGED <<logicalCreated,loaded,durable,requests,current,logical,bindings,outcomes,receipts,uncertain,uses,catalogKnown,savedDocuments,packets,logicalUses>>
PrepareRequest(q,c,h,i,p,packet,selected,r,a) ==
 /\ q \in Requests /\ c \in loaded /\ h \in Harnesses /\ i \in Inputs /\ <<c,r,a>> \in logicalUses
 /\ IF packet=NoPacket THEN p=NoPin /\ selected= <<>>
    ELSE p \in DOMAIN ranges /\ packet \in PacketIds /\ OrderedSubset(selected,ranges[p].items) /\ ranges[p].target=c
 /\ \A other \in Chats\{c}:current[other]#q
 /\ LET u==Use(q,r,a)
        req==[chat |-> c,harness |-> h,input |-> i,run |-> r,attempt |-> a,pin |-> p,packet |-> packet]
    IN /\ q \in DOMAIN requests =>
              requests[q].chat=c /\ requests[q].harness=h /\ requests[q].input=i /\ requests[q].pin=p /\ requests[q].packet=packet
       /\ packet \in DOMAIN packets => packets[packet]=[pin |-> p,selected |-> selected]
       /\ requests'=IF q \in DOMAIN requests THEN requests ELSE (q:>req)@@requests
       /\ packets'=IF packet=NoPacket THEN packets ELSE (packet:>[pin |-> p,selected |-> selected])@@packets
       /\ uses'=(q:>((IF q \in DOMAIN uses THEN uses[q] ELSE {})\cup{<<r,a>>}))@@uses
       /\ uncertain'=IF u \in AllUses \/ packet=NoPacket THEN uncertain ELSE uncertain\cup{u}
 /\ current'=IF logical[c].run=r /\ logical[c].attempt=a THEN [current EXCEPT ![c]=q] ELSE current
 /\ UNCHANGED <<logicalCreated,loaded,durable,logical,ranges,bindings,outcomes,receipts,catalogKnown,savedDocuments,logicalUses>>

BindSession(q,r,a,session) ==
 /\ Use(q,r,a) \in AllUses /\ session \in SessionIds
 /\ Use(q,r,a) \in DOMAIN bindings => bindings[Use(q,r,a)]=session
 /\ bindings'=bindings@@(Use(q,r,a):>session)
 /\ UNCHANGED <<logicalCreated,loaded,durable,requests,current,logical,ranges,outcomes,receipts,uncertain,uses,catalogKnown,savedDocuments,packets,logicalUses>>
RecordOutcome(q,r,a,outcome) ==
 /\ q \in Requests /\ r \in 1..MaxCTRun /\ a \in 1..MaxCTAttempt
 /\ outcome \in {"completed","failed","interrupted","transient","usage_limit"}
 /\ Use(q,r,a) \in DOMAIN outcomes => outcomes[Use(q,r,a)]=outcome
 /\ Use(q,r,a) \in AllUses
 /\ outcomes'=outcomes@@(Use(q,r,a):>outcome)
 /\ UNCHANGED logical
 /\ UNCHANGED <<logicalCreated,loaded,durable,requests,current,ranges,bindings,receipts,uncertain,uses,catalogKnown,savedDocuments,packets,logicalUses>>
SaveConfirmation(id,q,packet,session,input,r,a,document) ==
 /\ id \in ReceiptIds /\ q \in DOMAIN requests /\ packet=requests[q].packet /\ input=requests[q].input
 /\ Use(q,r,a) \in AllUses /\ Use(q,r,a) \in DOMAIN bindings /\ bindings[Use(q,r,a)]=session
 /\ Use(q,r,a) \in DOMAIN outcomes /\ outcomes[Use(q,r,a)]="completed"
 /\ document \in DOMAIN savedDocuments /\ savedDocuments[document]=requests[q].chat
 /\ LET receipt==[request |-> q,chat |-> requests[q].chat,harness |-> requests[q].harness,
       input |-> input,run |-> r,attempt |-> a,packet |-> packet,session |-> session,document |-> document]
    IN /\ id \in DOMAIN receipts => receipts[id]=receipt
       /\ receipts'=receipts@@(id:>receipt)
 /\ uncertain'=uncertain\{Use(q,r,a)}
 /\ UNCHANGED <<logicalCreated,loaded,durable,requests,current,logical,ranges,bindings,outcomes,uses,catalogKnown,savedDocuments,packets,logicalUses>>
TypeOK ==
 /\ logicalCreated \subseteq Chats /\ loaded \subseteq Chats /\ DOMAIN durable \subseteq Chats
 /\ catalogKnown \in BOOLEAN /\ DOMAIN savedDocuments \subseteq DocumentIds
 /\ \A d \in DOMAIN savedDocuments:savedDocuments[d] \in Chats
 /\ \A c \in DOMAIN durable:durable[c] \in DOMAIN savedDocuments /\ savedDocuments[durable[c]]=c
 /\ logicalUses \subseteq Chats\X(1..MaxCTRun)\X(1..MaxCTAttempt)
 /\ DOMAIN current=Chats /\ DOMAIN logical=Chats
 /\ \A c \in Chats:current[c] \in Requests\cup{NoRequest} /\ logical[c].run \in 0..MaxCTRun /\ logical[c].attempt \in 0..MaxCTAttempt /\ logical[c].live \in BOOLEAN
 /\ DOMAIN requests \subseteq Requests /\ DOMAIN uses=DOMAIN requests
 /\ DOMAIN ranges \subseteq Pins /\ DOMAIN packets \subseteq PacketIds
 /\ \A q \in DOMAIN requests:
      /\ requests[q].chat \in loaded /\ requests[q].harness \in Harnesses /\ requests[q].input \in Inputs
      /\ IF requests[q].packet=NoPacket THEN requests[q].pin=NoPin
         ELSE requests[q].pin \in DOMAIN ranges /\ requests[q].packet \in DOMAIN packets /\ packets[requests[q].packet].pin=requests[q].pin
      /\ uses[q] \subseteq (1..MaxCTRun)\X(1..MaxCTAttempt)
      /\ \A use \in uses[q]:<<requests[q].chat,use[1],use[2]>> \in logicalUses
 /\ DOMAIN bindings \subseteq AllUses /\ uncertain \subseteq AllUses
 /\ \A u \in DOMAIN bindings:bindings[u] \in SessionIds
 /\ DOMAIN receipts \subseteq ReceiptIds
ExactReceipts ==
 \A id \in DOMAIN receipts: LET x==receipts[id] q==x.request u==Use(q,x.run,x.attempt) IN
 /\ q \in DOMAIN requests /\ x.chat=requests[q].chat /\ x.harness=requests[q].harness
 /\ x.packet=requests[q].packet /\ x.input=requests[q].input
 /\ u \in AllUses /\ u \in DOMAIN bindings /\ bindings[u]=x.session
 /\ u \in DOMAIN outcomes /\ outcomes[u]="completed"
 /\ x.document \in DOMAIN savedDocuments /\ savedDocuments[x.document]=x.chat
 /\ u \notin uncertain
OrderedPackets == \A packet \in DOMAIN packets:
 packets[packet].pin \in DOMAIN ranges /\ OrderedSubset(packets[packet].selected,ranges[packets[packet].pin].items)
UncertaintyExact ==
 \A u \in AllUses:
   (u \in uncertain) <=> (requests[u[1]].packet#NoPacket /\
     ~\E id \in DOMAIN receipts:receipts[id].request=u[1] /\ receipts[id].run=u[2] /\ receipts[id].attempt=u[3])
RetainedStep ==
 /\ logicalUses \subseteq logicalUses'
 /\ DOMAIN requests \subseteq DOMAIN requests' /\ \A q \in DOMAIN requests:requests'[q]=requests[q]
 /\ DOMAIN receipts \subseteq DOMAIN receipts' /\ \A id \in DOMAIN receipts:receipts'[id]=receipts[id]
 /\ DOMAIN savedDocuments \subseteq DOMAIN savedDocuments'
Safety == TypeOK /\ ExactReceipts /\ OrderedPackets /\ UncertaintyExact
Next ==
 \/ \E c \in Chats:OpenLogical(c) \/ LoadDocument(c)
 \/ \E domain \in SUBSET Chats:\E catalog \in [domain -> DocumentIds]:ObserveSavedDocuments(catalog)
 \/ \E c \in Chats,d \in DocumentIds:SaveDocument(c,d)
 \/ \E c \in Chats,r \in 1..MaxCTRun,a \in 1..MaxCTAttempt:LogicalStart(c,r,a)
 \/ \E c \in Chats,r \in 1..MaxCTRun,a \in 1..MaxCTAttempt:LogicalAttempt(c,r,a)
 \/ \E c \in Chats,r \in 1..MaxCTRun,a \in 1..MaxCTAttempt:LogicalFinish(c,r,a)
 \/ \E p \in Pins,source,target \in Chats,base \in MessageIds\cup{NoMessage},point \in MessageIds,items \in BoundedSequences:
      PinRange(p,[source |-> source,target |-> target,base |-> base,point |-> point,items |-> items])
 \/ \E q \in Requests,c \in Chats,h \in Harnesses,i \in Inputs,p \in Pins\cup{NoPin},packet \in PacketIds\cup{NoPacket},selected \in BoundedSequences,r \in 1..MaxCTRun,a \in 1..MaxCTAttempt:PrepareRequest(q,c,h,i,p,packet,selected,r,a)
 \/ \E q \in Requests,r \in 1..MaxCTRun,a \in 1..MaxCTAttempt,session \in SessionIds:BindSession(q,r,a,session)
 \/ \E q \in Requests,r \in 1..MaxCTRun,a \in 1..MaxCTAttempt,out \in {"completed","failed","interrupted","transient","usage_limit"}:RecordOutcome(q,r,a,out)
 \/ \E id \in ReceiptIds,q \in Requests,packet \in PacketIds\cup{NoPacket},session \in SessionIds,input \in Inputs,r \in 1..MaxCTRun,a \in 1..MaxCTAttempt,d \in DocumentIds:
      SaveConfirmation(id,q,packet,session,input,r,a,d)
Spec == Init /\ [][Next]_ctVars
=============================================================================
