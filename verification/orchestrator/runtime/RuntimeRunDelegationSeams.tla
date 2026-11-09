------------------- MODULE RuntimeRunDelegationSeams -------------------
(* Exact finite-capture relations for RunEvent, DelEvent and OutboxEvent.
   State is the complete raw projection. No observed successor is consulted. *)
EXTENDS RuntimeState, RuntimeNativeQuestionSeams
RDResult(s,fx) == [state |-> s, fx |-> fx]
RDNoFx(s) == RDResult(s,<<>>)
RDInc8(n) == IF n<255 THEN n+1 ELSE 255
RDRun(s,t) == [thread |-> t,ord |-> RunOrd(s,t)]
RDStartFx(s,t,start,delivered) == [kind |-> "StartRun",run |-> RDRun(s,t),start |-> start,access |-> s.threads[t].access,delivered |-> delivered]
RDBegin(s,t,start) == [s EXCEPT !.threads[t].runs=Append(@,NewRun(start,"Live")),
 !.threads[t].int_req=FALSE,!.threads[t].attempt=[DefaultAttempt EXCEPT !.n=1],
 !.threads[t].ul=DefaultUL,!.threads[t].wait_on="none"]
RDNewAttempt(s,t,reason) == [s EXCEPT !.threads[t].attempt.n=@+1,
 !.threads[t].attempt.reason=reason,!.threads[t].attempt.emitted=FALSE,
 !.threads[t].attempt.kid_open=FALSE,!.threads[t].wait_on="none"]
RDStartAttempt(s,t,reason,msg) == [kind |-> "StartAttempt",attempt |-> CurrentAttempt(s,t),reason |-> reason,access |-> s.threads[t].access,msg |-> msg]
RDRestart(s,t,reason,msg) ==
 LET old == CurrentAttempt(s,t)
     root == [node |-> "Root",run |-> RunOrd(s,t),attempt |-> s.threads[t].attempt.n,outcome |-> "Interrupted"]
     staged == IF s.threads[t].attempt.emitted THEN s ELSE [s EXCEPT !.threads[t].inbox=@ \cup {root}]
     next == RDNewAttempt(staged,t,reason)
 IN RDResult(next,<<[kind |-> "InterruptAttempt",attempt |-> old],RDStartAttempt(next,t,reason,msg)>>)
RDCurrent(s,t,e) == IsLive(s,t) /\ e.run=RunOrd(s,t) /\ e.attempt=s.threads[t].attempt.n
RDLastOut(s,t) == LET rs == SelectSeq(s.threads[t].runs,LAMBDA r: r.status#"Live") IN IF Len(rs)=0 THEN "None" ELSE rs[Len(rs)].status
RDRetryable(o) == o \in {"Transient","Crash"}
RDMayRetry(s,t) == s.threads[t].attempt.retries<s.config.max_retry /\ ~s.threads[t].int_req
RDEndStatus(o) == IF o \in {"Transient","Crash"} THEN "Failed" ELSE o
RDPending(s,p,r) == {c \in DOMAIN s.threads: s.threads[c].parent=p /\ s.threads[c].task.present /\ s.threads[c].task.cohort=r /\ s.threads[c].task.st="Settled" /\ s.threads[c].task.delivery="Pending"}
RDUnsettled(s,p) == Cardinality({c \in DOMAIN s.threads: s.threads[c].parent=p /\ s.threads[c].task.present /\ s.threads[c].task.st#"Settled"})
RDTask(wake,cohort,run) == [present |-> TRUE,st |-> "Running",wake |-> wake,delivery |-> "Pending",cohort |-> cohort,run |-> run,pushes |-> 0]
RDPlan(s,p,r,cs,live) ==
 LET co == s.threads[p].runs[RunIndex(s,p,r)].cohort
     claim == {c \in cs: ~(s.threads[c].task.wake="SettledOnly" /\ live)}
     base == [s EXCEPT !.threads=[c \in DOMAIN @ |-> IF c \in cs /\ co.disp="Stopped" THEN [@[c] EXCEPT !.task.delivery="Disposed"]
                ELSE IF c \in claim THEN [@[c] EXCEPT !.task.delivery="Claimed"] ELSE @[c]]]
     gen == co.generation+(IF co.st="None" THEN 1 ELSE 0)
     msg == [kind |-> "Offer",parent |-> p,run |-> r,generation |-> gen]
     next == [base EXCEPT !.threads[p].runs[RunIndex(base,p,r)].cohort.del=@ \cup claim,
       !.threads[p].runs[RunIndex(base,p,r)].cohort.generation=gen,
       !.threads[p].runs[RunIndex(base,p,r)].cohort.st=IF co.st="None" THEN "Offered" ELSE @]
 IN IF co.disp="Stopped" \/ claim={} THEN RDNoFx(base)
    ELSE IF co.st="Queued" THEN RDNoFx(next)
    ELSE RDResult([next EXCEPT !.outbox=@ \cup {msg}],<<NQFxSend(msg)>>)
RDAck(s,c) ==
 LET p == s.threads[c].parent r == s.threads[c].task.cohort
     co == s.threads[p].runs[RunIndex(s,p,r)].cohort
     clear == c \in co.del /\ co.del \ {c}={} /\ co.st#"None"
 IN [s EXCEPT !.threads[c].task.delivery="Acknowledged",!.threads[c].task.pushes=RDInc8(@),
 !.threads[p].runs[RunIndex(s,p,r)].cohort.del=@ \ {c},
 !.threads[p].runs[RunIndex(s,p,r)].cohort.st=IF clear THEN "None" ELSE @,
 !.threads[p].queue=IF clear THEN SelectSeq(@,LAMBDA m: m.kind#"Wake" \/ m.run#r) ELSE @]
RDEnd(s,t,e) ==
 LET o == RDEndStatus(e.outcome) th == s.threads[t]
     base == [s EXCEPT !.threads[t].inbox=@ \ {e},!.threads[t].runs[RunIndex(s,t,RunOrd(s,t))].status=o,
       !.threads[t].int_req=FALSE,!.threads[t].attempt.kid_open=FALSE,
       !.threads[t].held=IF o="Failed" /\ th.held#"Stop" THEN "Fail" ELSE IF o="UsageLimit" /\ th.held#"Stop" THEN "Limit" ELSE @,
       !.threads[t].ul=IF o="UsageLimit" THEN [armed |-> IF th.held#"Stop" THEN RunOrd(s,t) ELSE @.armed,due |-> FALSE] ELSE @]
     timer == IF o="UsageLimit" /\ th.held#"Stop" THEN <<[kind |-> "ArmTimer",run |-> RDRun(s,t)]>> ELSE <<>>
     q == EndRunQuestions(base,t)
     wait == [q.state EXCEPT !.threads[t].late_wait=IF th.wait_on#"none" THEN th.wait_on ELSE @,!.threads[t].wait_on="none"]
     taskrun == th.task.present /\ th.runs[Len(th.runs)].start=TaskStart
     notice == [kind |-> "Notice",child |-> t,run |-> RunOrd(s,t)]
     notified == IF taskrun THEN [wait EXCEPT !.outbox=@ \cup {notice}] ELSE wait
     planned == RDPlan(notified,t,RunOrd(s,t),RDPending(notified,t,RunOrd(s,t)),FALSE)
 IN RDResult(planned.state,timer \o q.fx \o (IF taskrun THEN <<NQFxSend(notice)>> ELSE <<>>) \o planned.fx)
RDWaitKeys(s,t) == {k \in DOMAIN s.questions: s.questions[k].child=s.threads[t].wait_on /\ s.questions[k].pending /\ s.questions[k].holder=t}
RDWaitFx(s,t,c,out) == [kind |-> "CompleteWait",t |-> t,task |-> [child |-> c,run |-> s.threads[c].task.run],outcome |-> out]
RDWaitReturn(s,t) ==
 LET c == s.threads[t].wait_on keys == RDWaitKeys(s,t)
     base == IF keys#{} THEN [s EXCEPT !.threads[c].task.wake="Always",!.questions=[k \in DOMAIN @ |-> IF k \in keys THEN [@[k] EXCEPT !.waiting=@ \ {t},!.seen=@ \cup {t}] ELSE @[k]]]
             ELSE IF ~Handled(s.threads[c].task.delivery) THEN RDAck(s,c) ELSE s
     next == [base EXCEPT !.threads[t].wait_on="none"]
     views == NQConcat([i \in 1..Len(s.question_order) |-> LET k == s.question_order[i] IN IF k \in keys THEN <<NQFxView(s.questions[k],t,FALSE)>> ELSE <<>>])
 IN RDResult(next,views \o <<RDWaitFx(s,t,c,IF keys#{} THEN "Question" ELSE "Result")>>)
RDWakeIndex(s,t) ==
 LET th == s.threads[t] indices == {i \in 1..Len(th.queue):th.queue[i].kind="Wake"}
 IN IF th.held="Fail" /\ indices#{} THEN CHOOSE i \in indices: \A j \in indices:i<=j ELSE 1
RDWakeHead(s,t) == s.threads[t].queue[RDWakeIndex(s,t)]
RDWake(s,t) ==
 LET index == RDWakeIndex(s,t) head == RDWakeHead(s,t) r == head.run co == s.threads[t].runs[RunIndex(s,t,r)].cohort
     claimed == {c \in co.del: s.threads[c].task.delivery="Claimed"}
     pending == RDPending(s,t,r)
     delseq == SelectSeq(s.thread_order,LAMBDA c: c \in claimed)
     delivered == [i \in 1..Len(delseq) |-> [child |-> delseq[i],run |-> s.threads[delseq[i]].task.run]]
     base == [s EXCEPT !.threads=[c \in DOMAIN @ |-> IF c \in claimed THEN [@[c] EXCEPT !.task.delivery="Delivered",!.task.pushes=RDInc8(@)] ELSE @[c]]]
     reoffer == co.disp="Open" /\ pending#{}
     gen == co.generation+1 msg == [kind |-> "Offer",parent |-> t,run |-> r,generation |-> gen]
     staged == IF reoffer THEN [base EXCEPT !.threads=[c \in DOMAIN @ |-> IF c \in pending THEN [@[c] EXCEPT !.task.delivery="Claimed"] ELSE @[c]],
       !.threads[t].runs[RunIndex(s,t,r)].cohort.del=pending,!.threads[t].runs[RunIndex(s,t,r)].cohort.st="Offered",!.threads[t].runs[RunIndex(s,t,r)].cohort.generation=gen,!.outbox=@ \cup {msg}]
       ELSE [base EXCEPT !.threads[t].runs[RunIndex(s,t,r)].cohort.del={},!.threads[t].runs[RunIndex(s,t,r)].cohort.st="None"]
     popped == [staged EXCEPT !.threads[t].queue=SubSeq(@,1,index-1) \o SubSeq(@,index+1,Len(@))]
     start == [kind |-> "Queued",message |-> head] next == RDBegin(popped,t,start)
 IN RDResult(next,(IF reoffer THEN <<NQFxSend(msg)>> ELSE <<>>) \o <<RDStartFx(next,t,start,delivered)>>)
RDKids(s,t) == SelectSeq(s.thread_order,LAMBDA c: s.threads[c].parent=t)
RDStopInterrupt(s,t) == IsLive(s,t) \/ (RunOrd(s,t)>0 /\
 \E k \in DOMAIN s.natives: k[1]=t /\ s.natives[k].background /\ s.natives[k].status="Running")
RDStop(s,t,id) ==
 LET kids == RDKids(s,t)
     base == [s EXCEPT !.threads=[c \in DOMAIN @ |-> IF c=t THEN [@[c] EXCEPT !.int_req=@ \/ IsLive(s,t),!.held="Stop",!.ul=DefaultUL,
       !.runs=[i \in 1..Len(@) |-> [@[i] EXCEPT !.cohort.disp="Stopped",!.cohort.st="None",!.cohort.del={}]],!.queue=SelectSeq(@,LAMBDA m: m.kind#"Wake")]
       ELSE IF c \in {kids[i]: i \in 1..Len(kids)} /\ @[c].task.present /\ ~Handled(@[c].task.delivery) THEN [@[c] EXCEPT !.task.delivery="Disposed"] ELSE @[c]]]
     q == Reroute(base,t,TRUE)
     msgs == [i \in 1..Len(kids) |-> [kind |-> "Stop",child |-> kids[i],stop |-> id]]
 IN RDResult([q.state EXCEPT !.outbox=@ \cup {msgs[i]: i \in 1..Len(msgs)}],
   (IF RDStopInterrupt(s,t) THEN <<[kind |-> "InterruptRun",run |-> RDRun(s,t)]>> ELSE <<>>) \o q.fx \o [i \in 1..Len(msgs) |-> NQFxSend(msgs[i])])
RECURSIVE RDDescWalk(_,_,_), RDNarrowList(_,_,_)
RDDescWalk(s,front,out) == IF Len(front)=0 THEN out ELSE
 LET x == front[Len(front)] kids == SelectSeq(RDKids(s,x),LAMBDA c: c \notin {out[i]:i \in 1..Len(out)}) IN
 RDDescWalk(s,SubSeq(front,1,Len(front)-1) \o kids,out \o kids)
RDNarrowList(s,ts,access) == IF Len(ts)=0 THEN RDNoFx(s) ELSE
 LET t == Head(ts) th == s.threads[t] level == IF th.access<access THEN th.access ELSE access
     base == [s EXCEPT !.threads[t].access=level]
     one == IF level=th.access \/ ~IsLive(s,t) THEN RDNoFx(base)
       ELSE IF th.reconfigure THEN RDResult(base,<<[kind |-> "Reconfigure",t |-> t,access |-> level]>>)
       ELSE IF ~th.int_req /\ ~th.attempt.emitted THEN RDRestart(base,t,"AccessRestart","none") ELSE RDNoFx(base)
     rest == RDNarrowList(one.state,Tail(ts),access)
 IN RDResult(rest.state,one.fx \o rest.fx)
RDThreadGuard(s,ev) == LET a == ev.args th == s.threads[a.t] IN
   CASE ev.act="PersonSend" -> Len(th.queue)<s.config.max_queue /\ (~IsLive(s,a.t) \/ a.mode \in {"Queue","Now"} \/ th.int_req \/ th.attempt.emitted)
   [] ev.act="ServerEnqueue" -> Len(th.queue)<s.config.max_queue
   [] ev.act="Steer" -> IsLive(s,a.t) /\ ~th.int_req /\ ~th.attempt.emitted /\ th.attempt.steers<NQCap
   [] ev.act="RestartActive" -> a.mode \in {"Steer","Restart"} /\ IsLive(s,a.t) /\ ~th.int_req /\ ~th.attempt.emitted /\ th.attempt.n<NQCap
   [] ev.act="RequestInterrupt" -> TRUE
   [] ev.act="ProviderTerminal" -> IsLive(s,a.t) /\ a.run=RunOrd(s,a.t) /\ a.attempt=th.attempt.n /\ ~th.attempt.emitted /\ (a.outcome#"Interrupted" \/ th.int_req)
   [] ev.act="ChildStart" -> IsLive(s,a.t) /\ ~th.attempt.emitted /\ ~th.attempt.kid_open
   [] ev.act="ChildEnd" -> th.attempt.kid_open
   [] ev.act="ResetPasses" -> th.ul.armed=a.run /\ ~th.ul.due
   [] ev.act="StartFromQueue" -> Len(th.queue)>0 /\ Head(th.queue).kind#"Wake" /\ ~IsLive(s,a.t) /\ th.held="None" /\ RunOrd(s,a.t)<NQCap
   [] ev.act="UsageLimitRecovery" -> th.ul.armed=RunOrd(s,a.t) /\ th.ul.armed#0 /\ RDLastOut(s,a.t)="UsageLimit" /\ th.ul.due /\ ~IsLive(s,a.t) /\ th.held \in {"None","Limit"} /\ RunOrd(s,a.t)<NQCap
   [] ev.act="IngestEnd" -> a.ev \in th.inbox /\ a.ev.node="Root" /\ RDCurrent(s,a.t,a.ev) /\ ~(RDRetryable(a.ev.outcome) /\ RDMayRetry(s,a.t)) /\ (a.ev.outcome#"Completed" \/ th.wait_on="none")
   [] ev.act="RetryAttempt" -> a.ev \in th.inbox /\ a.ev.node="Root" /\ RDCurrent(s,a.t,a.ev) /\ RDRetryable(a.ev.outcome) /\ RDMayRetry(s,a.t) /\ th.attempt.n<NQCap
   [] ev.act="IngestChild" -> a.ev \in th.inbox /\ a.ev.node="Child" /\ RDCurrent(s,a.t,a.ev)
   [] ev.act="IngestStale" -> a.ev \in th.inbox /\ ~RDCurrent(s,a.t,a.ev)
   [] ev.act="Stop" -> s.stops<NQCap
   [] ev.act="NarrowAccess" -> a.access<th.access
   [] ev.act="InterruptChat" -> IsLive(s,a.t) /\ ~th.int_req
   [] ev.act="WaitReturn" -> th.wait_on#"none" /\ IsLive(s,a.t) /\ ~th.attempt.emitted /\ (RDWaitKeys(s,a.t)#{} \/ s.threads[th.wait_on].task.st="Settled")
   [] ev.act="WaitTimeout" -> th.wait_on#"none" /\ IsLive(s,a.t) /\ ~th.attempt.emitted /\ s.threads[th.wait_on].task.st#"Settled"
   [] ev.act="WaitAbandon" -> th.wait_on#"none"
   [] ev.act="LateWaitAck" -> th.late_wait#"none"
   [] ev.act="StartFromQueueWake" -> /\ Len(th.queue)>0 /\ RDWakeHead(s,a.t).kind="Wake" /\ ~IsLive(s,a.t) /\ th.held \in {"None","Fail"}
     /\ LET m == RDWakeHead(s,a.t) IN HasRun(s,a.t,m.run) /\ th.runs[RunIndex(s,a.t,m.run)].cohort.st="Queued" /\ th.runs[RunIndex(s,a.t,m.run)].cohort.generation=m.generation
   [] OTHER -> FALSE
RDNames == {"PersonSend","Steer","RestartActive","RequestInterrupt","ServerEnqueue","ProviderTerminal","ChildStart","ChildEnd","ResetPasses","StartFromQueue","UsageLimitRecovery","IngestEnd","RetryAttempt","IngestChild","IngestStale","OpenRoot","Stop","NarrowAccess","InterruptChat","Delegate","Followup","ReadTaskResult","ReadResult","WaitReturn","WaitTimeout","WaitAbandon","LateWaitAck","StartFromQueueWake","DoNotice","DoOffer","DoStop","CrashEngine"}
RDGuardKnown(s,ev) == LET a == ev.args IN
 CASE ev.act="OpenRoot" -> a.t \notin DOMAIN s.threads /\ a.access \in 0..255
 [] ev.act="Followup" -> /\ a.parent \in DOMAIN s.threads /\ a.child \in DOMAIN s.threads
    /\ LET p == s.threads[a.parent] c == s.threads[a.child] IN
       /\ IsLive(s,a.parent) /\ ~p.attempt.emitted /\ ~p.int_req /\ p.held="None" /\ p.wait_on="none" /\ ~NQBlocked(s,a.parent)
       /\ RDUnsettled(s,a.parent)<s.config.max_fanout /\ a.access<=p.access
       /\ c.parent=a.parent /\ c.depth<=s.config.max_depth /\ ~IsLive(s,a.child) /\ c.held="None" /\ ~c.int_req /\ c.queue= <<>> /\ ~NQBlocked(s,a.child)
       /\ c.task.present /\ c.task.st="Settled" /\ Handled(c.task.delivery) /\ a.access<=c.access /\ RunOrd(s,a.child)<NQCap
 [] ev.act="ReadResult" \/ ev.act="ReadTaskResult" ->
    /\ a.p \in DOMAIN s.threads /\ a.c \in DOMAIN s.threads /\ s.threads[a.c].task.present
    /\ s.threads[a.c].task.run=(IF ev.act="ReadResult" THEN 1 ELSE a.task_run)
    /\ IsLive(s,a.p) /\ ~s.threads[a.p].attempt.emitted /\ ~NQBlocked(s,a.p) /\ a.run=RunOrd(s,a.p)
    /\ s.threads[a.c].parent=a.p /\ s.threads[a.c].task.st="Settled" /\ ~Handled(s.threads[a.c].task.delivery)
 [] ev.act="Delegate" -> /\ a.t \in DOMAIN s.threads /\ a.child \notin DOMAIN s.threads
    /\ LET th == s.threads[a.t] IN IsLive(s,a.t) /\ ~th.int_req /\ th.wait_on="none" /\ th.depth<s.config.max_depth /\ RDUnsettled(s,a.t)<s.config.max_fanout /\ a.access<=th.access /\ a.mode \in {"Wait","Async"}
 [] ev.act="DoNotice" -> /\ a.child \in DOMAIN s.threads /\ s.threads[a.child].task.present
    /\ [kind |-> "Notice",child |-> a.child,run |-> a.run] \in s.outbox /\ a.how \in {"Ack","Keep"}
 [] ev.act="DoOffer" -> /\ a.parent \in DOMAIN s.threads /\ HasRun(s,a.parent,a.run)
    /\ [kind |-> "Offer",parent |-> a.parent,run |-> a.run,generation |-> a.generation] \in s.outbox /\ a.how \in {"Ack","Keep"}
 [] ev.act="DoStop" -> /\ a.child \in DOMAIN s.threads /\ [kind |-> "Stop",child |-> a.child,stop |-> a.stop] \in s.outbox /\ a.how \in {"Ack","Keep"}
 [] ev.act="CrashEngine" -> TRUE
 [] OTHER -> /\ a.t \in DOMAIN s.threads /\ RDThreadGuard(s,ev)
RDGuard(s,ev) == ev.act \in RDNames /\ RDGuardKnown(s,ev)
RDRawResult(s,ev) == LET a == ev.args IN
 CASE ev.act="OpenRoot" -> RDNoFx(PutThread(s,a.t,[NewThread(a.t,"none",0,a.access) EXCEPT !.reconfigure=a.reconfigure]))
 [] ev.act="PersonSend" -> RDNoFx([s EXCEPT !.threads[a.t].queue=Append(@,[kind |-> "Person",value |-> a.msg]),!.threads[a.t].held="None"])
 [] ev.act="ServerEnqueue" -> RDNoFx([s EXCEPT !.threads[a.t].queue=Append(@,[kind |-> "Server",value |-> a.msg])])
 [] ev.act="Steer" -> RDResult([s EXCEPT !.threads[a.t].attempt.steers=@+1],<<[kind |-> "SteerRun",attempt |-> CurrentAttempt(s,a.t),msg |-> a.msg]>>)
 [] ev.act="RestartActive" -> LET r == RDRestart(s,a.t,"SteeringRestart",a.msg) IN RDResult([r.state EXCEPT !.threads[a.t].attempt.steers=0],r.fx)
 [] ev.act="RequestInterrupt" -> RDResult([s EXCEPT !.threads[a.t].int_req=@ \/ IsLive(s,a.t),!.threads[a.t].held="Stop",!.threads[a.t].ul=DefaultUL],IF IsLive(s,a.t) THEN <<[kind |-> "InterruptRun",run |-> RDRun(s,a.t)]>> ELSE <<>>)
 [] ev.act="ProviderTerminal" -> RDNoFx([s EXCEPT !.threads[a.t].attempt.emitted=TRUE,!.threads[a.t].inbox=@ \cup {[node |-> "Root",run |-> a.run,attempt |-> a.attempt,outcome |-> a.outcome]}])
 [] ev.act="ChildStart" -> RDNoFx([s EXCEPT !.threads[a.t].attempt.kid_open=TRUE])
 [] ev.act="ChildEnd" -> RDNoFx([s EXCEPT !.threads[a.t].attempt.kid_open=FALSE,!.threads[a.t].inbox=@ \cup {[node |-> "Child",run |-> RunOrd(s,a.t),attempt |-> s.threads[a.t].attempt.n,outcome |-> "Completed"]}])
 [] ev.act="ResetPasses" -> RDNoFx([s EXCEPT !.threads[a.t].ul.due=TRUE])
 [] ev.act="StartFromQueue" -> LET head == Head(s.threads[a.t].queue) start == [kind |-> "Queued",message |-> head]
     next == RDBegin([s EXCEPT !.threads[a.t].queue=Tail(@)],a.t,start) IN RDResult(next,<<RDStartFx(next,a.t,start,<<>>)>>)
 [] ev.act="UsageLimitRecovery" -> LET next == RDBegin([s EXCEPT !.threads[a.t].held="None"],a.t,[kind |-> "Resume"])
   IN RDResult(next,<<RDStartFx(next,a.t,[kind |-> "Resume"],<<>>)>>)
 [] ev.act="IngestEnd" -> RDEnd(s,a.t,a.ev)
 [] ev.act="RetryAttempt" -> LET reason == IF a.ev.outcome="Transient" THEN "Retry" ELSE "ProviderRecovery"
     next == RDNewAttempt([s EXCEPT !.threads[a.t].inbox=@ \ {a.ev},!.threads[a.t].attempt.retries=@+1],a.t,reason)
   IN RDResult(next,<<RDStartAttempt(next,a.t,reason,"none")>>)
 [] ev.act="IngestChild" \/ ev.act="IngestStale" -> RDNoFx([s EXCEPT !.threads[a.t].inbox=@ \ {a.ev}])
 [] ev.act="Delegate" -> LET child == [NewThread(a.child,a.t,s.threads[a.t].depth+1,a.access) EXCEPT !.reconfigure=a.reconfigure,!.queue= <<[kind |-> "Task"]>>,!.task=RDTask(IF a.mode="Wait" THEN "SettledOnly" ELSE "Always",RunOrd(s,a.t),1)]
     next == PutThread(s,a.child,child)
   IN RDResult([next EXCEPT !.threads[a.t].wait_on=IF a.mode="Wait" THEN a.child ELSE @],<<[kind |-> "CreateChild",parent |-> a.t,child |-> a.child,depth |-> child.depth,access |-> a.access,mode |-> a.mode]>>)
 [] ev.act="Followup" -> RDNoFx([s EXCEPT !.threads[a.child].access=a.access,!.threads[a.child].queue=Append(@,[kind |-> "Task"]),!.threads[a.child].task=RDTask("Always",RunOrd(s,a.parent),RunOrd(s,a.child)+1)])
 [] ev.act="ReadResult" \/ ev.act="ReadTaskResult" -> RDNoFx(RDAck(s,a.c))
 [] ev.act="WaitReturn" -> RDWaitReturn(s,a.t)
 [] ev.act="WaitTimeout" -> LET c == s.threads[a.t].wait_on IN RDResult([s EXCEPT !.threads[c].task.wake="Always",!.threads[a.t].wait_on="none"],<<RDWaitFx(s,a.t,c,"TimedOut")>>)
 [] ev.act="WaitAbandon" -> RDNoFx([s EXCEPT !.threads[a.t].wait_on="none"])
 [] ev.act="LateWaitAck" -> RDNoFx([s EXCEPT !.threads[a.t].late_wait="none"])
 [] ev.act="StartFromQueueWake" -> RDWake(s,a.t)
 [] ev.act="Stop" -> RDStop([s EXCEPT !.stops=@+1],a.t,s.stops+1)
 [] ev.act="NarrowAccess" -> RDNarrowList(s,<<a.t>> \o RDDescWalk(s,<<a.t>>,<<>>),a.access)
 [] ev.act="InterruptChat" -> RDResult([s EXCEPT !.threads[a.t].int_req=TRUE],<<[kind |-> "InterruptRun",run |-> RDRun(s,a.t)]>>)
 [] ev.act="DoNotice" -> LET m == [kind |-> "Notice",child |-> a.child,run |-> a.run]
     base == IF a.how="Ack" THEN [s EXCEPT !.outbox=@ \ {m}] ELSE s task == s.threads[a.child].task p == s.threads[a.child].parent
   IN IF task.st="Settled" \/ task.run#a.run THEN RDNoFx(base) ELSE RDPlan([base EXCEPT !.threads[a.child].task.st="Settled"],p,task.cohort,IF task.delivery="Pending" THEN {a.child} ELSE {},IsLive(s,p) /\ RunOrd(s,p)=task.cohort)
 [] ev.act="DoOffer" -> LET m == [kind |-> "Offer",parent |-> a.parent,run |-> a.run,generation |-> a.generation]
     base == IF a.how="Ack" THEN [s EXCEPT !.outbox=@ \ {m}] ELSE s co == s.threads[a.parent].runs[RunIndex(s,a.parent,a.run)].cohort
   IN RDNoFx(IF co.st="Offered" /\ co.generation=a.generation THEN [base EXCEPT !.threads[a.parent].runs[RunIndex(s,a.parent,a.run)].cohort.st="Queued",!.threads[a.parent].queue=Append(@,[kind |-> "Wake",run |-> a.run,generation |-> a.generation])] ELSE base)
 [] ev.act="DoStop" -> LET m == [kind |-> "Stop",child |-> a.child,stop |-> a.stop]
     base == IF a.how="Ack" THEN [s EXCEPT !.outbox=@ \ {m}] ELSE s
   IN IF <<a.child,a.stop>> \in s.stop_receipts THEN RDNoFx(base) ELSE RDStop([base EXCEPT !.stop_receipts=@ \cup {<<a.child,a.stop>>}],a.child,a.stop)
 [] ev.act="CrashEngine" -> RDNoFx([s EXCEPT !.threads=[t \in DOMAIN @ |-> [@[t] EXCEPT !.wait_on="none",!.late_wait="none"]]])
 [] OTHER -> RDNoFx(s)
RunDelegationApply(s,next,ev,fx) ==
 /\ RDGuard(s,ev)
 /\ LET result == RDRawResult(s,ev)
        ordered == [result.state EXCEPT !.thread_order=ev.thread_order]
    IN next=Finalize(ordered) /\ fx=result.fx \o ReconcileEffects(ordered)
(* Ordered source rejection names. Evaluate only captured enum arguments. *)
RDCurrentReject(s,t,e) == CASE e \notin s.threads[t].inbox -> "NotInInbox"
 [] e.node#"Root" -> "NotRoot" [] ~RDCurrent(s,t,e) -> "NotCurrent" [] OTHER -> "accepted"
RDSteerReject(s,t) == CASE ~IsLive(s,t) -> "NotLive" [] s.threads[t].int_req -> "InterruptRequested"
 [] s.threads[t].attempt.emitted -> "TooLate" [] OTHER -> "accepted"
RDReadReject(s,ev) == LET a == ev.args IN
 CASE a.c \notin DOMAIN s.threads -> "UnknownThread" [] ~s.threads[a.c].task.present -> "NotKid"
 [] s.threads[a.c].task.run#(IF ev.act="ReadResult" THEN 1 ELSE a.task_run) -> "StaleRun"
 [] a.p \notin DOMAIN s.threads -> "UnknownThread"
 [] ~IsLive(s,a.p) \/ s.threads[a.p].attempt.emitted -> "NotLive" [] NQBlocked(s,a.p) -> "Held"
 [] a.run#RunOrd(s,a.p) -> "StaleRun" [] s.threads[a.c].parent#a.p -> "NotKid"
 [] s.threads[a.c].task.st#"Settled" -> "NotSettled" [] Handled(s.threads[a.c].task.delivery) -> "AlreadyHandled"
 [] ~HasRun(s,a.p,s.threads[a.c].task.cohort) -> "Corrupt" [] OTHER -> "accepted"
RDFollowupReject(s,a) ==
 CASE a.parent \notin DOMAIN s.threads -> "UnknownThread"
 [] ~IsLive(s,a.parent) \/ s.threads[a.parent].attempt.emitted -> "NotLive"
 [] s.threads[a.parent].int_req \/ s.threads[a.parent].held#"None" -> "InterruptRequested"
 [] s.threads[a.parent].wait_on#"none" -> "Waiting" [] NQBlocked(s,a.parent) -> "Held"
 [] RDUnsettled(s,a.parent)>=s.config.max_fanout -> "FanoutCap" [] a.access>s.threads[a.parent].access -> "AccessWider"
 [] a.child \notin DOMAIN s.threads -> "UnknownThread" [] s.threads[a.child].parent#a.parent -> "NotKid"
 [] s.threads[a.child].depth>s.config.max_depth -> "DepthCap" [] IsLive(s,a.child) -> "RunLive"
 [] s.threads[a.child].held#"None" \/ s.threads[a.child].int_req -> "Held"
 [] s.threads[a.child].queue# <<>> -> "QueueFull" [] NQBlocked(s,a.child) -> "Held"
 [] ~s.threads[a.child].task.present -> "NotKid" [] s.threads[a.child].task.st#"Settled" -> "NotSettled"
 [] ~Handled(s.threads[a.child].task.delivery) -> "AlreadyHandled"
 [] a.access>s.threads[a.child].access -> "AccessWider" [] OTHER -> "accepted"
RDWaitReject(s,t) == CASE s.threads[t].wait_on="none" -> "NotWaiting" [] ~IsLive(s,t) -> "NotLive"
 [] s.threads[t].attempt.emitted -> "TooLate" [] s.threads[t].wait_on \notin DOMAIN s.threads -> "UnknownThread"
 [] ~s.threads[s.threads[t].wait_on].task.present -> "NotKid" [] OTHER -> "accepted"
RDThreadReject(s,ev) == LET a == ev.args th == s.threads[a.t] IN
 CASE ev.act="PersonSend" ->
   CASE Len(th.queue)>=s.config.max_queue -> "QueueFull"
   [] IsLive(s,a.t) /\ a.mode \notin {"Queue","Now"} /\ ~th.int_req /\ ~th.attempt.emitted -> "SteerApplies"
   [] OTHER -> "accepted"
 [] ev.act="ServerEnqueue" -> IF Len(th.queue)>=s.config.max_queue THEN "QueueFull" ELSE "accepted"
 [] ev.act="Steer" -> RDSteerReject(s,a.t)
 [] ev.act="RestartActive" -> IF a.mode \notin {"Steer","Restart"} THEN "NotRestartMode" ELSE RDSteerReject(s,a.t)
 [] ev.act="RequestInterrupt" \/ ev.act="Stop" -> "accepted"
 [] ev.act="ProviderTerminal" ->
   CASE ~IsLive(s,a.t) -> "NotLive" [] a.run#RunOrd(s,a.t) \/ a.attempt#th.attempt.n -> "NotCurrent"
   [] th.attempt.emitted -> "AlreadyEmitted" [] a.outcome="Interrupted" /\ ~th.int_req -> "InterruptNotRequested" [] OTHER -> "accepted"
 [] ev.act="ChildStart" -> CASE ~IsLive(s,a.t) -> "NotLive" [] th.attempt.emitted -> "AlreadyEmitted"
   [] th.attempt.kid_open -> "ChildOpen" [] OTHER -> "accepted"
 [] ev.act="ChildEnd" -> IF th.attempt.kid_open THEN "accepted" ELSE "NoChildOpen"
 [] ev.act="ResetPasses" -> IF th.ul.armed#a.run THEN "TimerNotArmed" ELSE IF th.ul.due THEN "TimerAlreadyFired" ELSE "accepted"
 [] ev.act="StartFromQueue" -> CASE Len(th.queue)=0 -> "QueueEmpty" [] Head(th.queue).kind="Wake" -> "HeadIsWake"
   [] IsLive(s,a.t) -> "RunLive" [] th.held#"None" -> "Held" [] OTHER -> "accepted"
 [] ev.act="UsageLimitRecovery" ->
   CASE th.ul.armed=0 \/ th.ul.armed#RunOrd(s,a.t) -> "UsageLimitNotArmed" [] RDLastOut(s,a.t)#"UsageLimit" -> "LastNotUsageLimit" [] ~th.ul.due -> "ResetNotDue"
   [] IsLive(s,a.t) -> "RunLive" [] th.held \notin {"None","Limit"} -> "Held" [] OTHER -> "accepted"
 [] ev.act="IngestEnd" -> CASE RDCurrentReject(s,a.t,a.ev)#"accepted" -> RDCurrentReject(s,a.t,a.ev)
   [] RDRetryable(a.ev.outcome) /\ RDMayRetry(s,a.t) -> "UseRetry"
   [] a.ev.outcome="Completed" /\ th.wait_on#"none" -> "Waiting" [] OTHER -> "accepted"
 [] ev.act="RetryAttempt" -> CASE RDCurrentReject(s,a.t,a.ev)#"accepted" -> RDCurrentReject(s,a.t,a.ev)
   [] ~RDRetryable(a.ev.outcome) -> "NotRetryable" [] th.attempt.retries>=s.config.max_retry -> "RetryBudget"
   [] th.int_req -> "InterruptRequested" [] OTHER -> "accepted"
 [] ev.act="IngestChild" -> CASE a.ev \notin th.inbox -> "NotInInbox" [] a.ev.node#"Child" -> "NotChildNode"
   [] ~RDCurrent(s,a.t,a.ev) -> "NotCurrent" [] OTHER -> "accepted"
 [] ev.act="IngestStale" -> IF a.ev \notin th.inbox THEN "NotInInbox" ELSE IF RDCurrent(s,a.t,a.ev) THEN "Current" ELSE "accepted"
 [] ev.act="Delegate" -> CASE ~IsLive(s,a.t) -> "NotLive" [] th.int_req -> "InterruptRequested"
   [] th.wait_on#"none" -> "Waiting" [] a.child \in DOMAIN s.threads -> "ThreadExists" [] th.depth>=s.config.max_depth -> "DepthCap"
   [] RDUnsettled(s,a.t)>=s.config.max_fanout -> "FanoutCap" [] a.access>th.access -> "AccessWider" [] OTHER -> "accepted"
 [] ev.act="NarrowAccess" -> IF a.access>=th.access THEN "NotNarrower" ELSE "accepted"
 [] ev.act="InterruptChat" -> IF ~IsLive(s,a.t) THEN "NotLive" ELSE IF th.int_req THEN "InterruptRequested" ELSE "accepted"
 [] ev.act="WaitReturn" -> CASE RDWaitReject(s,a.t)#"accepted" -> RDWaitReject(s,a.t)
   [] RDWaitKeys(s,a.t)#{} -> "accepted" [] s.threads[th.wait_on].task.st#"Settled" -> "NotSettled" [] OTHER -> "accepted"
 [] ev.act="WaitTimeout" -> CASE RDWaitReject(s,a.t)#"accepted" -> RDWaitReject(s,a.t)
   [] s.threads[th.wait_on].task.st="Settled" -> "Settled" [] OTHER -> "accepted"
 [] ev.act="WaitAbandon" -> IF th.wait_on="none" THEN "NotWaiting" ELSE "accepted"
 [] ev.act="LateWaitAck" -> IF th.late_wait="none" THEN "NoLateWait" ELSE "accepted"
 [] ev.act="StartFromQueueWake" -> CASE Len(th.queue)=0 -> "QueueEmpty" [] RDWakeHead(s,a.t).kind#"Wake" -> "HeadNotWake"
   [] IsLive(s,a.t) -> "RunLive" [] th.held \notin {"None","Fail"} -> "Held" [] ~HasRun(s,a.t,RDWakeHead(s,a.t).run) -> "StaleWake"
   [] th.runs[RunIndex(s,a.t,RDWakeHead(s,a.t).run)].cohort.st#"Queued" -> "StaleWake"
   [] th.runs[RunIndex(s,a.t,RDWakeHead(s,a.t).run)].cohort.generation#RDWakeHead(s,a.t).generation -> "StaleWake" [] OTHER -> "accepted"
 [] OTHER -> "unsupported"
RDOutboxReject(s,ev) == LET a == ev.args
 m == CASE ev.act="DoNotice" -> [kind |-> "Notice",child |-> a.child,run |-> a.run]
      [] ev.act="DoOffer" -> [kind |-> "Offer",parent |-> a.parent,run |-> a.run,generation |-> a.generation]
      [] OTHER -> [kind |-> "Stop",child |-> a.child,stop |-> a.stop]
 IN CASE m \notin s.outbox -> "NotInOutbox"
 [] ev.act="DoOffer" -> IF a.parent \notin DOMAIN s.threads THEN "UnknownThread" ELSE IF ~HasRun(s,a.parent,a.run) THEN "Corrupt" ELSE "accepted"
 [] ev.act="DoStop" /\ <<a.child,a.stop>> \in s.stop_receipts -> "accepted"
 [] a.child \notin DOMAIN s.threads -> "UnknownThread"
 [] ev.act="DoNotice" /\ ~s.threads[a.child].task.present -> "NotKid" [] OTHER -> "accepted"
RDRejectName(s,ev) == LET a == ev.args IN
 CASE ev.act="OpenRoot" -> IF a.t \in DOMAIN s.threads THEN "ThreadExists" ELSE "accepted"
 [] ev.act="Followup" -> RDFollowupReject(s,a)
 [] ev.act \in {"ReadResult","ReadTaskResult"} -> RDReadReject(s,ev)
 [] ev.act \in {"DoNotice","DoOffer","DoStop"} -> RDOutboxReject(s,ev)
 [] ev.act="CrashEngine" -> "accepted"
 [] OTHER -> IF a.t \notin DOMAIN s.threads THEN "UnknownThread" ELSE RDThreadReject(s,ev)
RunDelegationReject(s,ev,reason) ==
 /\ ev.act \in {"PersonSend","Steer","RestartActive","RequestInterrupt","ServerEnqueue","ProviderTerminal","ChildStart","ChildEnd","ResetPasses","StartFromQueue","UsageLimitRecovery","IngestEnd","RetryAttempt","IngestChild","IngestStale","OpenRoot","Stop","NarrowAccess","InterruptChat","Delegate","Followup","ReadTaskResult","ReadResult","WaitReturn","WaitTimeout","WaitAbandon","LateWaitAck","StartFromQueueWake","DoNotice","DoOffer","DoStop","CrashEngine"}
 /\ reason#"accepted" /\ RDRejectName(s,ev)=reason
=============================================================================
