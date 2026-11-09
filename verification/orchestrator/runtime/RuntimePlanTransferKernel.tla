---------------- MODULE RuntimePlanTransferKernel ----------------
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS Threads, Main, NoThread, Providers, NoProv, MaxMsg, MaxX,
          MaxCrash, Limits, Mutant
VARIABLE ct
vars == <<ct>>
Members(seq) == {seq[i]:i \in 1..Len(seq)}
Ids(items) == [i \in 1..Len(items)|->items[i].id]
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

EmptyPT == [alive |-> FALSE,pcov |-> {},pst |-> {},ecov |-> {},est |-> {},seen |-> 0,dpend |-> FALSE]
Idle == [st |-> "idle",p |-> NoProv,u |-> 0,w |-> 0]
EmptyX == [ty |-> "none",src |-> NoThread,tgt |-> NoThread,sp |-> 0,bp |-> 0,hd |-> 0,tp |-> NoProv,
 st |-> "none",strat |-> "none",why |-> "none",sel |-> {},om |-> {},hdr |-> FALSE,dl |-> "none",cr |-> FALSE]
NoPacket == [present |-> FALSE,ids |-> {},om |-> {},point |-> 0,input |-> 0,provider |-> NoProv,attempt |-> 0,run |-> 0]
Max(S) == IF S={} THEN 0 ELSE CHOOSE i \in S: \A j \in S:j<=i
Own(t,w) == {i \in 1..Len(ct.journal):ct.journal[i].th=t /\ i<=w}
Hist(t,w) == Own(t,w) \cup IF ct.lineage[t].src=NoThread THEN {} ELSE Own(ct.lineage[t].src,IF w<ct.lineage[t].w THEN w ELSE ct.lineage[t].w)
Expand(S) == {i \in S:ct.journal[i].role#"ctx"} \cup UNION {ct.journal[i].cx:i \in {j \in S:ct.journal[j].role="ctx"}}
Stable(t) == Max({i \in Own(t,Len(ct.journal)):ct.journal[i].role="asst"})
Pending(t) == {x \in 1..ct.next:ct.ledger[x].tgt=t /\ ct.ledger[x].st="pending"}
Resolved(t) == {x \in 1..ct.next:ct.ledger[x].tgt=t /\ ct.ledger[x].st="resolved"}
Range(x) == LET r==ct.ledger[x] IN
 CASE r.ty="subagent_result" -> {r.sp}
 [] r.ty="merge_back" -> {i \in Own(r.src,r.sp):i>r.bp}
 [] r.ty="provider_switch" /\ r.strat="delta" -> {i \in Hist(r.src,r.sp):i>r.bp}
 [] OTHER -> Hist(r.src,r.sp)
NewX(ty,s,t,sp,bp) == [EmptyX EXCEPT !.ty=ty,!.src=s,!.tgt=t,!.sp=sp,!.bp=bp,!.st="pending"]
Init == ct=[journal |-> <<>>,created |-> [t \in Threads|->t=Main],
 lineage |-> [t \in Threads|->[src |-> NoThread,w |-> 0]],kinds |-> [t \in Threads|->"none"],
 runs |-> [t \in Threads|->Idle],providers |-> [t \in Threads|->[p \in Providers|->EmptyPT]],
 transferInput |-> [x \in 1..MaxX|->0],acknowledged |-> [x \in 1..MaxX|->{}],anchors |-> [x \in 1..MaxX|->{}],ledger |-> [x \in 1..MaxX|->EmptyX],next |-> 0,volatile |-> [t \in Threads|->"none"],
 dup |-> FALSE,abort |-> [t \in Threads|->"none"],crashes |-> MaxCrash,
 originalPackets |-> [t \in Threads|->NoPacket],packets |-> [t \in Threads|->NoPacket],receipts |-> [t \in Threads|->{}],
 accepted |-> [t \in Threads|->{}],bindings |-> [t \in Threads|->[run |-> 0,attempt |-> 0]]]
PlanStart(t,p,input,ord,attempt) ==
 /\ ct.created[t] /\ ct.runs[t].st="idle" /\ ~ct.providers[t][p].dpend /\ Len(ct.journal)<MaxMsg /\ input=Len(ct.journal)+1
 /\ ord>0 /\ attempt>0
 /\ LET needs==Pending(t)={} /\ \E i \in Hist(t,Len(ct.journal)):i>ct.providers[t][p].seen
        r==[st |-> "starting",p |-> p,u |-> input,w |-> Len(ct.journal)]
    IN /\ (needs => ct.next<MaxX)
       /\ ct'=[ct EXCEPT !.journal=Append(@,[th |-> t,role |-> "user",cx |-> {}]),!.runs[t]=r,
          !.bindings[t]=[run |-> ord,attempt |-> attempt],!.packets[t]=NoPacket,!.originalPackets[t]=NoPacket,!.accepted[t]={},!.receipts[t]={},
          !.next=IF needs THEN @+1 ELSE @,
          !.ledger=IF needs THEN [@ EXCEPT ![ct.next+1]=NewX("provider_switch",t,t,Len(ct.journal),0)] ELSE @]
CreateLineage(s,t,kind) ==
 /\ ct.created[s] /\ ~ct.created[t] /\ ct.lineage[s].src=NoThread /\ Stable(s)>0 /\ ct.next<MaxX
 /\ kind \in {"fork","subagent_spawn"} /\ (kind="subagent_spawn" => ct.runs[s].st="live")
 /\ ct'=[ct EXCEPT !.created[t]=TRUE,!.lineage[t]=[src |-> s,w |-> Stable(s)],!.kinds[t]=kind,
     !.next=@+1,!.ledger[ct.next+1]=NewX(kind,s,t,Stable(s),0)]
PinResult(t,kind) ==
 /\ ct.created[t] /\ ct.lineage[t].src#NoThread /\ ct.runs[t].st="idle" /\ Stable(t)>0 /\ ct.next<MaxX
 /\ kind=IF ct.kinds[t]="fork" THEN "merge_back" ELSE "subagent_result"
 /\ (kind="merge_back" => ct.runs[ct.lineage[t].src].st="idle")
 /\ ~\E x \in 1..ct.next:ct.ledger[x].ty=kind /\ ct.ledger[x].src=t /\ ct.ledger[x].sp=Stable(t)
 /\ ct'=[ct EXCEPT !.next=@+1,!.ledger[ct.next+1]=NewX(kind,t,ct.lineage[t].src,Stable(t),0)]
Resolve(t,x,limit) ==
 /\ ct.runs[t].st="starting" /\ x \in Pending(t) /\ limit \in Limits
 /\ ~(ct.ledger[x].ty="provider_switch" /\ ct.providers[t][ct.runs[t].p].alive /\ ct.providers[t][ct.runs[t].p].seen>=ct.ledger[x].sp)
 /\ LET p==ct.runs[t].p
        old==ct.ledger[x]
        strategy==CASE old.ty="provider_switch" -> IF ct.providers[t][p].alive THEN "delta" ELSE "full_summary"
          [] old.ty="merge_back" -> "merge_delta" [] old.ty="subagent_result" -> "result" [] OTHER -> "portable_replay"
        r==[old EXCEPT !.strat=strategy,!.bp=IF strategy="delta" THEN ct.providers[t][p].seen ELSE old.bp]
        range==CASE r.ty="subagent_result" -> {r.sp}
          [] r.ty="merge_back" -> {i \in Own(r.src,r.sp):i>r.bp}
          [] strategy="delta" -> {i \in Hist(r.src,r.sp):i>r.bp}
          [] OTHER -> Hist(r.src,r.sp)
        ids==SelectSeq([i \in 1..Len(ct.journal)|->i],LAMBDA i:i \in Expand(range))
        items==[i \in 1..Len(ids)|->[id |-> ids[i],role |-> IF ct.journal[ids[i]].role="asst" THEN "assistant" ELSE ct.journal[ids[i]].role,render_bytes |-> 4]]
        choice==Selection([items |-> items,header_base |-> 4],ct.providers[t][p].ecov,limit)
        selected=={choice.selected[i]:i \in 1..Len(choice.selected)}
        candidates==Candidates([items |-> items],ct.providers[t][p].ecov)
        required=={candidates[i].id:i \in Members(AnchorOrder(candidates))}
        nr==[r EXCEPT !.tp=p,!.hd=Len(ct.journal),!.st=IF choice.failed THEN "failed" ELSE "resolved",
          !.why=IF choice.failed THEN "budget" ELSE "none",!.sel=IF Mutant="drop_anchors" THEN selected\required ELSE selected,!.om=Expand(range)\selected,!.hdr=~choice.failed]
    IN ct'=[ct EXCEPT !.ledger[x]=nr,!.anchors[x]=required,!.transferInput[x]=ct.runs[t].u,
       !.journal=IF ~choice.failed /\ old.ty \in {"merge_back","subagent_result"} THEN Append(@,[th |-> t,role |-> "ctx",cx |-> selected]) ELSE @,
       !.runs[t]=IF choice.failed /\ Mutant#"skip_failed" THEN Idle ELSE @,
       !.abort[t]=IF choice.failed THEN "budget" ELSE @]
NativeResume(t,x) ==
 /\ ct.runs[t].st="starting" /\ x \in Pending(t) /\ ct.ledger[x].ty="provider_switch"
 /\ LET p==ct.runs[t].p IN
    /\ ct.providers[t][p].alive /\ ct.providers[t][p].seen>=ct.ledger[x].sp
    /\ ct'=[ct EXCEPT !.ledger[x]=[@ EXCEPT !.st="consumed",!.strat="native_resume",!.tp=p,!.hd=Len(ct.journal)]]
ProviderBare(t) ==
 /\ ct.runs[t].st="starting" /\ Pending(t)={} /\ Resolved(t)={} /\ ~ct.packets[t].present
 /\ (Mutant="skip_failed" \/ ~\E x \in 1..ct.next:ct.ledger[x].tgt=t /\ ct.ledger[x].st="failed" /\ ct.transferInput[x]=ct.runs[t].u)
 /\ LET p==ct.runs[t].p u==ct.runs[t].u IN
    ct'=[ct EXCEPT !.runs[t].st="live",!.accepted[t]={u},!.providers[t][p].alive=TRUE,
      !.dup=@ \/ u \in ct.providers[t][p].pcov,!.providers[t][p].pcov=@ \cup {u},!.providers[t][p].ecov=@ \cup {u}]
NativeFork(t,x) ==
 /\ ct.runs[t].st="starting" /\ x \in Pending(t)
 /\ ct.ledger[x].ty \in {"fork","subagent_spawn"}
 /\ LET p==ct.runs[t].p s==ct.ledger[x].src sp==ct.ledger[x].sp
        ids==Expand(Hist(s,sp)) old==ct.providers[s][p]
    IN /\ old.alive /\ ids \subseteq old.ecov
       /\ ct'=[ct EXCEPT !.providers[t][p]=[old EXCEPT !.pcov={i \in @:i<=sp},!.ecov={i \in @:i<=sp},
             !.pst={i \in @:i<=sp},!.est={i \in @:i<=sp},!.seen=sp,!.dpend=FALSE],
          !.acknowledged[x]=ids,!.ledger[x]=[@ EXCEPT !.st="consumed",!.strat="native_fork",!.tp=p,!.hd=Len(ct.journal),!.sel=ids]]
SavePacket(t) ==
 /\ ct.runs[t].st="starting" /\ Pending(t)={} /\ Resolved(t)#{} /\ ~ct.packets[t].present
 /\ LET p==ct.runs[t].p ids==UNION {ct.ledger[x].sel:x \in Resolved(t)}
        omitted==UNION {ct.ledger[x].om:x \in Resolved(t)}
        packet==[present |-> TRUE,ids |-> ids\ct.providers[t][p].ecov,om |-> omitted,point |-> ct.runs[t].w,input |-> ct.runs[t].u,
          provider |-> p,attempt |-> ct.bindings[t].attempt,run |-> ct.bindings[t].run]
    IN ct'=[ct EXCEPT !.packets[t]=packet,!.originalPackets[t]=packet,!.volatile[t]="batched",!.providers[t][p].dpend=TRUE,
      !.ledger=[x \in 1..MaxX|->IF x \in Resolved(t) THEN [ct.ledger[x] EXCEPT !.dl="pending"] ELSE ct.ledger[x]]]
ProviderAccept(t) ==
 /\ ct.runs[t].st="starting" /\ ct.packets[t].present /\ ct.volatile[t]="batched"
 /\ (Mutant="skip_failed" \/ ~\E x \in 1..ct.next:ct.ledger[x].tgt=t /\ ct.ledger[x].st="failed" /\ ct.transferInput[x]=ct.runs[t].u)
 /\ LET p==ct.runs[t].p pay==ct.packets[t].ids \cup {ct.packets[t].input} IN
     ct'=[ct EXCEPT !.accepted[t]=pay,!.volatile[t]="sent",!.runs[t].st="live",
   !.dup=@ \/ pay \cap ct.providers[t][p].pcov#{},!.providers[t][p].alive=TRUE,
   !.providers[t][p].pcov=@ \cup pay,!.providers[t][p].pst=@ \cup ct.packets[t].om]
Inject(t) ==
 /\ ct.volatile[t]="batched" /\ ct.runs[t].st="starting"
 /\ LET p==ct.runs[t].p ids==ct.packets[t].ids IN
     ct'=[ct EXCEPT !.accepted[t]=ids,!.volatile[t]="injected",!.providers[t][p].alive=TRUE,
   !.dup=@ \/ ids \cap ct.providers[t][p].pcov#{},!.providers[t][p].pcov=@ \cup ids,
   !.providers[t][p].pst=@ \cup ct.packets[t].om]
PersistInjected(t) ==
 /\ ct.volatile[t]="injected"
 /\ LET p==ct.runs[t].p xs==Resolved(t) IN
     ct'=[ct EXCEPT !.receipts[t]=ct.accepted[t],!.acknowledged=[x \in 1..MaxX|->IF x \in xs THEN ct.ledger[x].sel ELSE ct.acknowledged[x]],!.volatile[t]="none",
   !.providers[t][p].ecov=@ \cup ct.accepted[t],!.providers[t][p].est=@ \cup ct.packets[t].om,
   !.providers[t][p].dpend=FALSE,!.providers[t][p].seen=ct.runs[t].w,
   !.ledger=[x \in 1..MaxX|->IF x \in xs THEN [ct.ledger[x] EXCEPT !.st="consumed",!.dl="injected"] ELSE ct.ledger[x]]]
StartAfterInjection(t) ==
 /\ ct.runs[t].st="starting" /\ ct.volatile[t]="none" /\ ct.packets[t].present
 /\ Pending(t)={} /\ Resolved(t)={} /\ ~ct.providers[t][ct.runs[t].p].dpend
 /\ LET p==ct.runs[t].p u==ct.runs[t].u IN
     ct'=[ct EXCEPT !.runs[t].st="live",!.providers[t][p].alive=TRUE,
   !.dup=@ \/ u \in ct.providers[t][p].pcov,!.providers[t][p].pcov=@ \cup {u},!.providers[t][p].ecov=@ \cup {u}]
SaveReceipt(t) ==
 /\ ct.volatile[t]="sent"
 /\ LET p==ct.runs[t].p xs==Resolved(t) IN
     ct'=[ct EXCEPT !.receipts[t]=ct.accepted[t],!.acknowledged=[x \in 1..MaxX|->IF x \in xs THEN ct.ledger[x].sel ELSE ct.acknowledged[x]],!.volatile[t]="none",
   !.providers[t][p].ecov=@ \cup ct.accepted[t],!.providers[t][p].est=@ \cup ct.packets[t].om,
   !.providers[t][p].dpend=FALSE,!.providers[t][p].seen=Max({ct.runs[t].w} \cup {ct.providers[t][p].seen}),
   !.ledger=[x \in 1..MaxX|->IF x \in xs THEN [ct.ledger[x] EXCEPT !.st="consumed",!.dl="inline"] ELSE ct.ledger[x]]]
Complete(t) ==
 /\ ct.runs[t].st="live" /\ ct.volatile[t]="none" /\ Len(ct.journal)<MaxMsg
 /\ LET a==Len(ct.journal)+1 p==ct.runs[t].p IN
     ct'=[ct EXCEPT !.journal=Append(@,[th |-> t,role |-> "asst",cx |-> {}]),!.runs[t]=Idle,
  !.providers[t][p].pcov=@ \cup {a},!.providers[t][p].ecov=@ \cup {a},!.providers[t][p].seen=a]
Crash ==
 /\ ct.crashes>0
 /\ ct'=[ct EXCEPT !.crashes=@-1,!.volatile=[t \in Threads|->"none"],
  !.runs=[t \in Threads|->IF ct.runs[t].st="live" THEN Idle ELSE ct.runs[t]],
  !.ledger=[x \in 1..MaxX|->IF ct.ledger[x].st="resolved" THEN [ct.ledger[x] EXCEPT !.cr=TRUE] ELSE ct.ledger[x]]]
Recover(t) ==
 /\ ct.volatile[t]="none" /\ ct.packets[t].present /\ ct.providers[t][ct.packets[t].provider].dpend
 /\ (ct.runs[t].st="idle" \/ ct.next<MaxX)
 /\ LET p==ct.packets[t].provider fresh==ct.runs[t].st="starting" IN
    ct'=[ct EXCEPT !.providers[t][p]=EmptyPT,!.packets[t]=NoPacket,!.next=IF fresh THEN @+1 ELSE @,
      !.ledger=[x \in 1..MaxX|->IF fresh /\ x=ct.next+1 THEN NewX("provider_switch",t,t,ct.packets[t].point,0)
        ELSE IF x \in Resolved(t) THEN [ct.ledger[x] EXCEPT !.st="superseded",!.why="uncertain"] ELSE ct.ledger[x]]]
AbortRecovery(t) ==
 /\ ct.volatile[t]="none" /\ ct.packets[t].present /\ ct.providers[t][ct.packets[t].provider].dpend
 /\ ct.next=MaxX
 /\ LET p==ct.packets[t].provider IN
    ct'=[ct EXCEPT !.providers[t][p]=EmptyPT,!.packets[t]=NoPacket,!.runs[t]=Idle,!.abort[t]="replaced",
      !.ledger=[x \in 1..MaxX|->IF x \in Resolved(t) THEN [ct.ledger[x] EXCEPT !.st="superseded",!.why="uncertain"] ELSE ct.ledger[x]]]
Retry(t) ==
 /\ ct.packets[t].present /\ ct.volatile[t]="batched" /\ ct.runs[t].st="starting"
 /\ ct'=[ct EXCEPT !.packets[t].ids=IF Mutant="changed_retry" THEN {} ELSE @]
ResolveSwitch(t,x,l) == ct.ledger[x].ty="provider_switch" /\ Resolve(t,x,l)
ResolveFork(t,x,l) == ct.ledger[x].ty="fork" /\ Resolve(t,x,l)
ResolveSpawn(t,x,l) == ct.ledger[x].ty="subagent_spawn" /\ Resolve(t,x,l)
ResolveMerge(t,x,l) == ct.ledger[x].ty="merge_back" /\ Resolve(t,x,l)
ResolveResult(t,x,l) == ct.ledger[x].ty="subagent_result" /\ Resolve(t,x,l)
ResolveUsing(t,x,a) == Resolve(t,x,ContextLimit(a))
AnchorsKept == \A x \in 1..ct.next:ct.ledger[x].st \in {"resolved","consumed"} => ct.anchors[x] \subseteq ct.ledger[x].sel
Next ==
 \/ \E t \in Threads,p \in Providers:PlanStart(t,p,Len(ct.journal)+1,1,1)
 \/ \E s,t \in Threads,k \in {"fork","subagent_spawn"}:CreateLineage(s,t,k)
 \/ \E t \in Threads,k \in {"merge_back","subagent_result"}:PinResult(t,k)
 \/ \E t \in Threads,x \in 1..MaxX,l \in Limits:ResolveSwitch(t,x,l) \/ ResolveFork(t,x,l) \/ ResolveSpawn(t,x,l) \/ ResolveMerge(t,x,l) \/ ResolveResult(t,x,l) \/ NativeFork(t,x) \/ NativeResume(t,x)
 \/ \E t \in Threads:ProviderBare(t) \/ SavePacket(t) \/ ProviderAccept(t) \/ SaveReceipt(t) \/ Inject(t) \/ PersistInjected(t) \/ StartAfterInjection(t) \/ Complete(t) \/ Recover(t) \/ AbortRecovery(t) \/ Retry(t)
 \/ Crash
CTImage == [xMsgs |-> ct.journal,xCreated |-> ct.created,xForkOf |-> ct.lineage,xKindOf |-> ct.kinds,
 xRun |-> ct.runs,xPt |-> ct.providers,xXf |-> ct.ledger,xNx |-> ct.next,xVol |-> ct.volatile,
 xDup |-> ct.dup,xAbort |-> ct.abort,xCrashLeft |-> ct.crashes,xStaleLeft |-> 0]
NoFuture == \A x \in 1..ct.next:ct.ledger[x].sel \subseteq 1..ct.ledger[x].sp
ReceiptSound == \A t \in Threads:ct.receipts[t] \subseteq ct.providers[t][ct.packets[t].provider].pcov
ConsumedSound == \A x \in 1..ct.next:ct.ledger[x].st="consumed" =>
 ct.ledger[x].sel \subseteq ct.acknowledged[x]
NoFailedStart == \A t \in Threads:ct.runs[t].st="live" =>
 ~\E x \in 1..ct.next:ct.ledger[x].tgt=t /\ ct.ledger[x].st="failed" /\ ct.transferInput[x]=ct.runs[t].u
PacketBound == \A t \in Threads:ct.packets[t].present =>
 ct.packets[t].run=ct.bindings[t].run /\ ct.packets[t].attempt=ct.bindings[t].attempt
RetryStable == \A t \in Threads:ct.packets[t].present => ct.packets[t]=ct.originalPackets[t]
NoDuplicate == ~ct.dup
W_ProviderAccepted == ~\E t \in Threads:ct.accepted[t]#{}
W_BudgetFailure == ~\E x \in 1..ct.next:ct.ledger[x].st="failed"
W_Fork == ~\E x \in 1..ct.next:ct.ledger[x].ty="fork" /\ ct.ledger[x].st="consumed"
W_Spawn == ~\E x \in 1..ct.next:ct.ledger[x].ty="subagent_spawn" /\ ct.ledger[x].st="consumed"
W_BudgetCut == ~\E x \in 1..ct.next:ct.ledger[x].st="consumed" /\ ct.ledger[x].om#{} /\ ct.ledger[x].hdr
W_NativeFork == ~\E x \in 1..ct.next:ct.ledger[x].strat="native_fork"
W_Injected == ~\E x \in 1..ct.next:ct.ledger[x].dl="injected"
W_Merge == ~\E x \in 1..ct.next:ct.ledger[x].ty="merge_back" /\ ct.ledger[x].st="consumed"
W_Result == ~\E x \in 1..ct.next:ct.ledger[x].ty="subagent_result" /\ ct.ledger[x].st="consumed"
W_Recovered == ~\E x \in 1..ct.next:ct.ledger[x].why="uncertain"
W_Consumed == ~\E x \in 1..ct.next:ct.ledger[x].st="consumed"
ResolveFor(t) == \E x \in 1..MaxX,l \in Limits:Resolve(t,x,l) \/ NativeResume(t,x) \/ NativeFork(t,x)
TransferResolves == \A x \in 1..MaxX:
 (ct.ledger[x].st \in {"pending","resolved"} /\
  IF ct.ledger[x].tgt \in Threads THEN ct.runs[ct.ledger[x].tgt].st="starting" ELSE FALSE)
 ~> (ct.ledger[x].st \in {"consumed","failed","superseded"} \/
  IF ct.ledger[x].tgt \in Threads THEN ct.abort[ct.ledger[x].tgt]#"none" ELSE FALSE)
Spec == Init /\ [][Next]_vars
FairSpec == Spec /\ \A t \in Threads:
 WF_vars(ResolveFor(t)) /\ WF_vars(SavePacket(t)) /\ WF_vars(ProviderBare(t))
 /\ WF_vars(ProviderAccept(t)) /\ WF_vars(Inject(t)) /\ WF_vars(PersistInjected(t))
 /\ WF_vars(StartAfterInjection(t)) /\ WF_vars(SaveReceipt(t)) /\ WF_vars(Recover(t) \/ AbortRecovery(t))
=================================================================
