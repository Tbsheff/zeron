---------------- MODULE RuntimePlanOperationalTransferKernel ----------------
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS Threads, Main, NoThread, Providers, NoProv, MaxMsg, MaxX,
          MaxCrash, Limits, Mutant
VARIABLE ct
vars == <<ct>>
L == INSTANCE RuntimePlanTransferKernel
Idle == L!Idle
EmptyX == L!EmptyX
EmptyPT == L!EmptyPT
NoPacket == [present |-> FALSE,ids |-> {},om |-> {},input |-> 0,provider |-> NoProv]
Members(s) == {s[i]:i \in 1..Len(s)}
Max(S) == IF S={} THEN 0 ELSE CHOOSE i \in S: \A j \in S:j<=i
Own(t,w) == {i \in 1..Len(ct.journal):ct.journal[i].th=t /\ i<=w}
Hist(t,w) == Own(t,w) \cup IF ct.lineage[t].src=NoThread THEN {} ELSE Own(ct.lineage[t].src,IF w<ct.lineage[t].w THEN w ELSE ct.lineage[t].w)
Expand(S) == {i \in S:ct.journal[i].role#"ctx"} \cup UNION {ct.journal[i].cx:i \in {j \in S:ct.journal[j].role="ctx"}}
Stable(t) == Max({i \in Own(t,Len(ct.journal)):ct.journal[i].role="asst"})
Pending(t) == {x \in 1..ct.next:ct.ledger[x].tgt=t /\ ct.ledger[x].st="pending"}
Resolved(t) == {x \in 1..ct.next:ct.ledger[x].tgt=t /\ ct.ledger[x].st="resolved"}
Failed(t) == {x \in 1..ct.next:ct.ledger[x].tgt=t /\ ct.transferInput[x]=ct.runs[t].u /\ ct.ledger[x].st="failed"}
Init == ct=[journal |-> <<>>,created |-> [t \in Threads|->t=Main],
 lineage |-> [t \in Threads|->[src |-> NoThread,w |-> 0]],kinds |-> [t \in Threads|->"none"],
 runs |-> [t \in Threads|->Idle],providers |-> [t \in Threads|->[p \in Providers|->EmptyPT]],
 ledger |-> [x \in 1..MaxX|->EmptyX],next |-> 0,volatile |-> [t \in Threads|->"none"],
 dup |-> FALSE,abort |-> [t \in Threads|->"none"],crashes |-> MaxCrash,
 packets |-> [t \in Threads|->NoPacket],accepted |-> [t \in Threads|->FALSE],
 transferInput |-> [x \in 1..MaxX|->0],anchors |-> [x \in 1..MaxX|->{}],
 bindings |-> [t \in Threads|->[run |-> 0,attempt |-> 0]],
 intent |-> [t \in Threads|->FALSE]]
LogicalPending(t,ord,attempt) ==
 /\ ord>0 /\ attempt>0
 /\ ct.created[t] /\ ct.runs[t].st="idle" /\ ~ct.intent[t]
 /\ ct'=[ct EXCEPT !.intent[t]=TRUE,!.runs[t]=[st |-> "pending",p |-> NoProv,u |-> 0,w |-> 0],
     !.packets[t]=NoPacket,!.accepted[t]=FALSE,!.abort[t]="none",
     !.bindings[t]=[run |-> ord,attempt |-> attempt]]
PlanStart(t,ord,attempt) == LogicalPending(t,ord,attempt)
ObserveUser(t) ==
 /\ ct.intent[t] /\ ct.runs[t].st="pending" /\ ct.runs[t].u=0 /\ Len(ct.journal)<MaxMsg
 /\ ct'=[ct EXCEPT !.journal=Append(@,[th |-> t,role |-> "user",cx |-> {}]),
     !.runs[t].u=Len(ct.journal)+1,!.runs[t].w=Len(ct.journal)]
PrepareObserved(t,p,u) ==
 /\ p \in Providers /\ ct.intent[t] /\ ct.runs[t].st="pending" /\ ct.runs[t].u=u /\ u>0
 /\ ct.journal[u].th=t /\ ct.journal[u].role="user"
 /\ ~ct.providers[t][p].dpend
 /\ \A x \in Resolved(t):ct.ledger[x].tp=p /\ ct.transferInput[x]=u
 /\ ct'=[ct EXCEPT !.runs[t].st="starting",!.runs[t].p=p]
PinObserved(kind,s,t,sp,bp) ==
 /\ kind \in {"fork","subagent_spawn","provider_switch","merge_back","subagent_result"}
 /\ ct.created[s] /\ ct.created[t] /\ ct.next<MaxX /\ bp<=sp /\ sp<=Len(ct.journal)
 /\ ~ct.packets[t].present /\ ~ct.accepted[t]
 /\ (ct.runs[t].u>0 => sp<ct.runs[t].u)
 /\ ct'=[ct EXCEPT !.next=@+1,!.ledger[ct.next+1]=[EmptyX EXCEPT
       !.ty=kind,!.src=s,!.tgt=t,!.sp=sp,!.bp=bp,!.st="pending"]]
PinSwitch(t) ==
 /\ ct.runs[t].st="starting" /\ ct.runs[t].w>0 /\ Pending(t)={}
 /\ PinObserved("provider_switch",t,t,ct.runs[t].w,0)
CreateLineage(s,t,kind) ==
 /\ ct.created[s] /\ ~ct.created[t] /\ Stable(s)>0 /\ ct.next<MaxX
 /\ kind \in {"fork","subagent_spawn"}
 /\ ct'=[ct EXCEPT !.created[t]=TRUE,!.lineage[t]=[src |-> s,w |-> Stable(s)],!.kinds[t]=kind,
     !.next=@+1,!.ledger[ct.next+1]=[EmptyX EXCEPT !.ty=kind,!.src=s,!.tgt=t,!.sp=Stable(s),!.st="pending"]]
ResolveObserved(t,x,limit) ==
 /\ ct.runs[t].st="starting" /\ ~ct.packets[t].present /\ ~ct.accepted[t]
 /\ x \in Pending(t) /\ limit \in Limits
 /\ LET p==ct.runs[t].p r==ct.ledger[x]
        strategy==CASE r.ty="provider_switch" -> IF ct.providers[t][p].alive THEN "delta" ELSE "full_summary"
          [] r.ty="merge_back" -> "merge_delta" [] r.ty="subagent_result" -> "result" [] OTHER -> "portable_replay"
        base==IF strategy="delta" THEN ct.providers[t][p].seen ELSE r.bp
        range==CASE r.ty="subagent_result" -> {r.sp}
          [] r.ty="merge_back" -> {i \in Own(r.src,r.sp):i>base}
          [] strategy="delta" -> {i \in Hist(r.src,r.sp):i>base}
          [] OTHER -> Hist(r.src,r.sp)
        ids==SelectSeq([i \in 1..Len(ct.journal)|->i],LAMBDA i:i \in Expand(range))
        items==[i \in 1..Len(ids)|->[id |-> ids[i],role |-> IF ct.journal[ids[i]].role="asst" THEN "assistant" ELSE ct.journal[ids[i]].role,render_bytes |-> 4]]
        choice==L!Selection([items |-> items,header_base |-> 4],ct.providers[t][p].ecov,limit)
        selected==Members(choice.selected)
        candidates==L!Candidates([items |-> items],ct.providers[t][p].ecov)
        anchors=={candidates[i].id:i \in Members(L!AnchorOrder(candidates))}
    IN ct'=[ct EXCEPT !.ledger[x]=[r EXCEPT !.tp=p,!.bp=base,!.hd=Len(ct.journal),!.strat=strategy,
        !.st=IF choice.failed THEN "failed" ELSE "resolved",!.why=IF choice.failed THEN "budget" ELSE "none",
        !.sel=IF Mutant="drop_anchors" THEN selected\anchors ELSE selected,
        !.om={item.id:item \in Members(candidates)}\selected,!.hdr=~choice.failed],
       !.transferInput[x]=ct.runs[t].u,!.anchors[x]=anchors,
       !.abort[t]=IF choice.failed THEN "budget" ELSE @]
CommitPrepared(t) ==
 /\ ct.runs[t].st="starting" /\ Pending(t)={} /\ ~ct.packets[t].present
 /\ (Mutant="skip_failed" \/ Failed(t)={})
 /\ LET p==ct.runs[t].p ids==UNION {ct.ledger[x].sel:x \in Resolved(t)}
        omitted==UNION {ct.ledger[x].om:x \in Resolved(t)}
    IN ct'=[ct EXCEPT !.packets[t]=[present |-> TRUE,ids |-> ids,om |-> omitted,input |-> ct.runs[t].u,provider |-> p],
       !.providers[t][p].dpend=TRUE,!.volatile[t]="batched",
       !.ledger=[x \in 1..MaxX|->IF x \in Resolved(t) THEN [ct.ledger[x] EXCEPT !.st="consumed",!.dl="pending"] ELSE ct.ledger[x]]]
ConfirmAccepted(t,p,ids,omitted) ==
 /\ ct.runs[t].st="starting" /\ p=ct.runs[t].p /\ ct.packets[t].present /\ ~ct.accepted[t]
 /\ ids=ct.packets[t].ids \cup {ct.runs[t].u} /\ omitted=ct.packets[t].om
 /\ (Mutant="skip_failed" \/ Failed(t)={})
 /\ ct'=[ct EXCEPT !.accepted[t]=TRUE,!.volatile[t]="none",
     !.dup=@ \/ ids \cap ct.providers[t][p].pcov#{},
     !.providers[t][p]=[@ EXCEPT !.alive=TRUE,!.pcov=@ \cup ids,!.pst=@ \cup omitted,
        !.ecov=@ \cup ids,!.est=@ \cup omitted,!.dpend=FALSE],
     !.ledger=[x \in 1..MaxX|->IF ct.ledger[x].tgt=t /\ ct.transferInput[x]=ct.runs[t].u /\ ct.ledger[x].st="consumed"
        THEN [ct.ledger[x] EXCEPT !.dl="inline"] ELSE ct.ledger[x]]]
CompleteObserved(t) ==
 /\ ct.runs[t].st="starting" /\ ct.accepted[t] /\ Len(ct.journal)<MaxMsg
 /\ LET i==Len(ct.journal)+1 p==ct.runs[t].p IN
    ct'=[ct EXCEPT !.journal=Append(@,[th |-> t,role |-> "asst",cx |-> {}]),!.runs[t].st="complete",
       !.providers[t][p].pcov=@ \cup {i},!.providers[t][p].ecov=@ \cup {i},!.providers[t][p].seen=i]
FailObserved(t) ==
 /\ ct.intent[t] /\ ct.runs[t].st \in {"pending","starting"}
 /\ ~ct.accepted[t]
 /\ ct'= [ct EXCEPT !.runs[t].st="failed"]
IngestEnd(t) ==
 /\ ct.runs[t].st \in {"complete","failed"}
 /\ ct'=[ct EXCEPT !.runs[t]=Idle,!.intent[t]=FALSE]
AbortObserved(t) ==
 /\ ct.intent[t] /\ ct.abort[t]="budget" /\ Failed(t)#{}
 /\ ct'=[ct EXCEPT !.runs[t]=Idle,!.intent[t]=FALSE]
ReadyRequest(t) == ct.runs[t].st="starting" /\ ct.packets[t].present /\ Pending(t)={} /\ Failed(t)={}
CTImage == [xMsgs |-> ct.journal,xCreated |-> ct.created,xForkOf |-> ct.lineage,xKindOf |-> ct.kinds,
 xRun |-> ct.runs,xPt |-> ct.providers,xXf |-> ct.ledger,xNx |-> ct.next,xVol |-> ct.volatile,
 xDup |-> ct.dup,xAbort |-> ct.abort,xCrashLeft |-> ct.crashes,xStaleLeft |-> 0]
Next ==
 \/ \E t \in Threads: LogicalPending(t,1,1) \/ ObserveUser(t) \/ CompleteObserved(t) \/ FailObserved(t) \/ IngestEnd(t) \/ AbortObserved(t) \/ CommitPrepared(t)
 \/ \E t \in Threads,p \in Providers,u \in 1..MaxMsg: PrepareObserved(t,p,u)
 \/ \E t \in Threads: ConfirmAccepted(t,ct.runs[t].p,ct.packets[t].ids \cup {ct.runs[t].u},ct.packets[t].om)
 \/ \E t \in Threads: PinSwitch(t)
 \/ \E t \in Threads,x \in 1..MaxX,l \in Limits: ResolveObserved(t,x,l)
Spec == Init /\ [][Next]_vars
NoEarlyProvider == \A t \in Threads:ct.runs[t].st="pending" => ct.runs[t].p=NoProv
NoFailedStart == \A t \in Threads:ct.accepted[t] => Failed(t)={}
CoverageReceipt == \A t \in Threads,p \in Providers:ct.providers[t][p].pcov \subseteq ct.providers[t][p].ecov
AnchorsKept == \A x \in 1..ct.next:ct.ledger[x].st \in {"resolved","consumed"} => ct.anchors[x] \subseteq ct.ledger[x].sel
NoDuplicate == ~ct.dup
====
