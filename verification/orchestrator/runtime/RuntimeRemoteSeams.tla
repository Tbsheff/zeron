------------------------ MODULE RuntimeRemoteSeams ------------------------
EXTENDS RuntimeState, RuntimeNativeQuestionSeams
RemoteTask(st,delivery,cohort,run) == [present |-> TRUE,st |-> st,wake |-> "Always",
 delivery |-> delivery,cohort |-> cohort,run |-> run,pushes |-> 0]
RemoteStub(id,parent,depth,cohort) ==
 [NewThread(id,parent,depth,2) EXCEPT !.run_base=cohort-1,
 !.runs= <<NewRun(TaskStart,"Completed")>>,!.attempt.n=1,
 !.task=IF parent="none" THEN [present |-> FALSE]
         ELSE RemoteTask("Settled","Disposed",1,cohort)]
RemoteExtend(s,p,cohort) == LET th == s.threads[p] IN
 [s EXCEPT !.threads[p].runs = @ \o
 [i \in 1..(IF cohort>RunOrd(s,p) THEN cohort-RunOrd(s,p) ELSE 0) |-> NewRun(TaskStart,"Completed")]]
RemoteImportBase(s,a) ==
 LET s1 == IF a.depth=2 /\ a.ancestor \notin DOMAIN s.threads
           THEN PutThread(s,a.ancestor,RemoteStub(a.ancestor,"none",0,1)) ELSE s
     s2 == IF a.parent \notin DOMAIN s1.threads
           THEN PutThread(s1,a.parent,RemoteStub(a.parent,a.ancestor,a.depth-1,a.cohort)) ELSE s1
 IN s2
RemoteImportGuard(s,a) ==
 /\ a.child \notin DOMAIN s.threads
 /\ a.depth \in 1..s.config.max_depth /\ a.depth<=2 /\ a.cohort>0 /\ a.access<=2
 /\ IF a.depth=2 THEN a.ancestor#"none" /\ a.ancestor#a.parent /\ a.ancestor#a.child /\ a.parent#a.child
                    ELSE a.ancestor="none" /\ a.parent#a.child
 /\ LET ss == RemoteImportBase(s,a) IN
    ss.threads[a.parent].depth+1=a.depth /\ a.cohort>ss.threads[a.parent].run_base
RemoteImportRaw(s,a) ==
 LET base == RemoteExtend(RemoteImportBase(s,a),a.parent,a.cohort)
     child == [NewThread(a.child,a.parent,a.depth,a.access) EXCEPT
       !.queue= <<[kind |-> "Task"]>>,!.task=RemoteTask("Running","Pending",a.cohort,1)]
 IN [PutThread(base,a.child,child) EXCEPT !.thread_order=a.thread_order]
RemoteFollowupGuard(s,a) == LET th == s.threads[a.child] IN
 /\ ~IsLive(s,a.child) /\ th.queue= <<>> /\ th.held="None" /\ ~th.int_req
 /\ ~NQBlocked(s,a.child) /\ RunOrd(s,a.child)+1=a.run /\ a.access<=th.access
 /\ th.task.present /\ th.task.st="Settled" /\ Handled(th.task.delivery)
 /\ th.parent#"none" /\ th.parent \in DOMAIN s.threads
 /\ a.cohort>0 /\ a.cohort>s.threads[th.parent].run_base
RemoteFollowupRaw(s,a) ==
 LET base == RemoteExtend(s,s.threads[a.child].parent,a.cohort) IN
 [base EXCEPT !.threads[a.child].access=a.access,
 !.threads[a.child].queue=Append(@,[kind |-> "Task"]),
 !.threads[a.child].task=RemoteTask("Running","Pending",a.cohort,a.run)]
RemoteNoticeGuard(s,a) ==
 /\ a.child \in DOMAIN s.threads /\ s.threads[a.child].task.present
 /\ a.status#"Live" /\ s.threads[a.child].task.run=a.run
 /\ IF s.threads[a.child].task.st="Settled" THEN TRUE ELSE
    ~IsLive(s,a.child) /\ a.run>0 /\
    s.threads[a.child].parent \in DOMAIN s.threads /\
    HasRun(s,s.threads[a.child].parent,s.threads[a.child].task.cohort)
RemoteNoticeResult(s,a) ==
 IF s.threads[a.child].task.st="Settled" THEN [state |-> s,fx |-> <<>>] ELSE
 LET c == a.child p == s.threads[c].parent r == s.threads[c].task.cohort
     task == s.threads[c].task co == s.threads[p].runs[RunIndex(s,p,r)].cohort
     claim == task.delivery="Pending" /\ ~(task.wake="SettledOnly" /\ IsLive(s,p) /\ RunOrd(s,p)=r)
     disposed == task.delivery="Pending" /\ co.disp="Stopped"
     offered == claim /\ co.disp#"Stopped"
     gen == co.generation + (IF co.st="None" THEN 1 ELSE 0)
     msg == [kind |-> "Offer",parent |-> p,run |-> r,generation |-> gen]
     base == [s EXCEPT !.threads[c].queue= <<>>,!.threads[c].run_base=a.run-1,
       !.threads[c].runs= <<NewRun(TaskStart,a.status)>>,!.threads[c].attempt.n=1,
       !.threads[c].task.st="Settled",
       !.threads[c].task.delivery=IF disposed THEN "Disposed" ELSE IF offered THEN "Claimed" ELSE @,
       !.outbox=@ \ {[kind |-> "Notice",child |-> c,run |-> a.run]}]
     planned == IF offered THEN [base EXCEPT
       !.threads[p].runs[RunIndex(base,p,r)].cohort.del=@ \cup {c},
       !.threads[p].runs[RunIndex(base,p,r)].cohort.generation=gen,
       !.threads[p].runs[RunIndex(base,p,r)].cohort.st=IF co.st="None" THEN "Offered" ELSE @]
       ELSE base
     send == offered /\ co.st#"Queued"
 IN [state |-> IF send THEN [planned EXCEPT !.outbox=@ \cup {msg}] ELSE planned,
     fx |-> IF send THEN <<[kind |-> "Send",message |-> msg]>> ELSE <<>>]
RemoteStopKids(s,t) == SelectSeq(s.thread_order,LAMBDA c: s.threads[c].parent=t)
RemoteStopBase(s,t) ==
 [s EXCEPT !.threads=[c \in DOMAIN @ |->
 IF c=t THEN [@[c] EXCEPT !.int_req=@ \/ IsLive(s,t),!.held="Stop",!.ul=DefaultUL,
 !.runs=[i \in 1..Len(@) |-> [@[i] EXCEPT !.cohort.disp="Stopped",!.cohort.st="None",!.cohort.del={}]],
 !.queue=SelectSeq(@,LAMBDA m: m.kind#"Wake")]
 ELSE IF @[c].parent=t /\ @[c].task.present /\ ~Handled(@[c].task.delivery)
      THEN [@[c] EXCEPT !.task.delivery="Disposed"] ELSE @[c]]]
RemoteStopResult(s,t,stop) ==
 LET kids == RemoteStopKids(s,t)
     base == RemoteStopBase(s,t)
     q == Reroute(base,t,TRUE)
     msgs == [i \in 1..Len(kids) |-> [kind |-> "Stop",child |-> kids[i],stop |-> stop]]
     sendfx == [i \in 1..Len(msgs) |-> [kind |-> "Send",message |-> msgs[i]]]
     intfx == IF IsLive(s,t) THEN <<[kind |-> "InterruptRun",run |-> [thread |-> t,ord |-> RunOrd(s,t)]]>> ELSE <<>>
 IN [state |-> [q.state EXCEPT !.outbox=@ \cup {msgs[i]: i \in 1..Len(msgs)}],
     fx |-> intfx \o q.fx \o sendfx]
RemoteSentGuard(s,a) == IF a.message.kind="Notice" THEN
 a.message.child \in DOMAIN s.threads /\ s.threads[a.message.child].task.present ELSE TRUE
RemoteSentRaw(s,a) == LET base == [s EXCEPT !.outbox=@ \ {a.message}] IN
 IF a.message.kind="Notice" /\ s.threads[a.message.child].task.run=a.message.run
 THEN [base EXCEPT !.threads[a.message.child].task.st="Settled",!.threads[a.message.child].task.delivery="Disposed"] ELSE base
RemoteGuard(s,ev) == LET a == ev.args IN
 CASE ev.act="RemoteImport" -> RemoteImportGuard(s,a)
 [] ev.act="RemoteFollowup" -> a.child \in DOMAIN s.threads /\ RemoteFollowupGuard(s,a)
 [] ev.act="RemoteNotice" -> RemoteNoticeGuard(s,a)
 [] ev.act="RemoteSent" -> RemoteSentGuard(s,a)
 [] ev.act="RemoteStop" -> a.child \in DOMAIN s.threads
 [] ev.act="RemoteCancel" -> a.child \in DOMAIN s.threads
 [] ev.act="RemoteFence" -> a.t \in DOMAIN s.threads
 [] ev.act="RemoteClamp" -> a.child \in DOMAIN s.threads /\ s.threads[a.child].access=a.access
 [] OTHER -> FALSE
RemoteResult(s,ev) == LET a == ev.args IN
 CASE ev.act="RemoteImport" -> [state |-> RemoteImportRaw(s,a),fx |-> <<>>]
 [] ev.act="RemoteFollowup" -> [state |-> RemoteFollowupRaw(s,a),fx |-> <<>>]
 [] ev.act="RemoteNotice" -> RemoteNoticeResult(s,a)
 [] ev.act="RemoteSent" -> [state |-> RemoteSentRaw(s,a),fx |-> <<>>]
 [] ev.act="RemoteFence" -> [state |-> s,fx |-> <<>>]
 [] ev.act="RemoteClamp" -> [state |-> s,fx |-> <<[kind |-> "Reconfigure",t |-> a.child,access |-> a.access]>>]
 [] ev.act="RemoteStop" -> IF <<a.child,a.stop>> \in s.stop_receipts THEN [state |-> s,fx |-> <<>>] ELSE
     LET stop == (IF s.stops>a.stop THEN s.stops ELSE a.stop)+1
         base == [s EXCEPT !.stops=stop,!.stop_receipts=@ \cup {<<a.child,a.stop>>}]
     IN RemoteStopResult(base,a.child,stop)
 [] ev.act="RemoteCancel" ->
     LET stop == s.stops+1
         stopped == RemoteStopResult([s EXCEPT !.stops=stop],a.child,stop)
         msg == [kind |-> "Stop",child |-> a.child,stop |-> stop]
     IN [state |-> [stopped.state EXCEPT !.outbox=@ \cup {msg}],fx |-> Append(stopped.fx,[kind |-> "Send",message |-> msg])]
 [] OTHER -> [state |-> s,fx |-> <<>>]
(* Ordered Rust rejection paths. "none" denotes success, not a rejection. *)
RemoteTaskError(s,c) == IF c \notin DOMAIN s.threads THEN "UnknownThread"
 ELSE IF ~s.threads[c].task.present THEN "NotKid" ELSE "none"
RemoteImportError(s,a) ==
 IF a.child \in DOMAIN s.threads THEN "ThreadExists"
 ELSE IF ~(a.depth \in 1..s.config.max_depth /\ a.depth<=2 /\ a.cohort>0) THEN "DepthCap"
 ELSE IF a.access>2 THEN "AccessWider"
 ELSE IF a.depth=2 /\ (a.ancestor="none" \/ a.ancestor=a.parent \/ a.ancestor=a.child \/ a.parent=a.child)
      THEN "Corrupt"
 ELSE IF a.depth#2 /\ (a.ancestor#"none" \/ a.parent=a.child) THEN "Corrupt"
 ELSE LET base == RemoteImportBase(s,a) IN
      IF base.threads[a.parent].depth+1#a.depth THEN "Corrupt"
      ELSE IF a.cohort<=base.threads[a.parent].run_base THEN "StaleRun" ELSE "none"
RemoteFollowupError(s,a) ==
 IF a.child \notin DOMAIN s.threads THEN "UnknownThread" ELSE
 LET th == s.threads[a.child] IN
 IF IsLive(s,a.child) \/ th.queue#<<>> THEN "RunLive"
 ELSE IF th.held#"None" \/ th.int_req THEN "Held"
 ELSE IF NQBlocked(s,a.child) THEN "Held"
 ELSE IF RunOrd(s,a.child)+1#a.run THEN "StaleRun"
 ELSE IF a.access>th.access THEN "AccessWider"
 ELSE IF ~th.task.present THEN "NotKid"
 ELSE IF th.task.st#"Settled" \/ ~Handled(th.task.delivery) THEN "NotSettled"
 ELSE IF th.parent="none" THEN "Corrupt"
 ELSE IF th.parent \notin DOMAIN s.threads THEN "UnknownThread"
 ELSE IF a.cohort=0 \/ a.cohort<=s.threads[th.parent].run_base THEN "StaleRun" ELSE "none"
RemoteNoticeError(s,a) ==
 IF a.status="Live" THEN "NotSettled"
 ELSE IF RemoteTaskError(s,a.child)#"none" THEN RemoteTaskError(s,a.child)
 ELSE LET th == s.threads[a.child] IN
 IF th.task.run#a.run THEN "StaleRun"
 ELSE IF th.task.st="Settled" THEN "none"
 ELSE IF IsLive(s,a.child) THEN "RunLive"
 ELSE IF th.parent="none" THEN "Corrupt"
 ELSE IF th.parent \notin DOMAIN s.threads THEN "UnknownThread"
 ELSE IF ~HasRun(s,th.parent,th.task.cohort) THEN "Corrupt" ELSE "none"
RemoteStopError(s,t) ==
 IF \E c \in DOMAIN s.threads: s.threads[c].parent=t /\ ~s.threads[c].task.present
 THEN "NotKid" ELSE IF t \notin DOMAIN s.threads THEN "UnknownThread" ELSE "none"
RemoteError(s,ev) == LET a == ev.args IN
 CASE ev.act="RemoteImport" -> RemoteImportError(s,a)
 [] ev.act="RemoteFollowup" -> RemoteFollowupError(s,a)
 [] ev.act="RemoteNotice" -> RemoteNoticeError(s,a)
 [] ev.act="RemoteSent" -> IF a.message.kind="Notice" THEN RemoteTaskError(s,a.message.child) ELSE "none"
 [] ev.act="RemoteFence" -> IF a.t \notin DOMAIN s.threads THEN "UnknownThread" ELSE "none"
 [] ev.act="RemoteClamp" -> IF a.child \notin DOMAIN s.threads THEN "UnknownThread"
    ELSE IF s.threads[a.child].access#a.access THEN "NotNarrower" ELSE "none"
 [] ev.act="RemoteCancel" -> RemoteStopError(s,a.child)
 [] ev.act="RemoteStop" -> IF a.child \notin DOMAIN s.threads THEN "UnknownThread"
    ELSE IF <<a.child,a.stop>> \in s.stop_receipts THEN "none" ELSE RemoteStopError(s,a.child)
 [] OTHER -> "unsupported"
RemoteReject(s,ev,reason) == LET err == RemoteError(s,ev) IN
 err#"none" /\ err#"unsupported" /\ reason=err
RemoteApply(s,next,ev,fx) ==
 /\ RemoteGuard(s,ev)
 /\ LET result == RemoteResult(s,ev) IN
    next=Finalize(result.state) /\ fx=result.fx \o ReconcileEffects(result.state)
=============================================================================
