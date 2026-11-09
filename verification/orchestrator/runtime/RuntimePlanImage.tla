------------------------ MODULE RuntimePlanImage ------------------------
(* Structural image of independently stepped operational state. This module
   has no INSTANCE of the target, Next search, successor choice, or target
   fairness. Ghost updates inspect only accepted raw actions, raw effects,
   before/after core, and independent provider/CT observations. *)
EXTENDS RuntimeState
CONSTANTS Threads, Root, NoThread, MaxRuns, MaxX,
 RLMaxPerson,RLMaxServer,RLMaxStop,RLMaxUL,RLMaxCrash,
 DMaxPerson,MaxFail,DMaxCrash,MaxAbandon,DMaxStop,MaxNarrow,MaxInterrupt,
 Devices,RootDev,MaxOffline,InPlace

PIChildren == Threads \ {Root}
PIOrds == 1..MaxRuns
PILower(x) == CASE x="None" -> "none" [] x="Initial" -> "initial"
 [] x="Stop" -> "stop" [] x="Fail" -> "fail" [] x="Limit" -> "limit"
 [] x="Live" -> "live" [] x="Completed" -> "completed" [] x="Failed" -> "failed"
 [] x="Interrupted" -> "interrupted" [] x="UsageLimit" -> "usage_limit"
 [] x="Transient" -> "transient" [] x="Crash" -> "crash"
 [] x="SteeringRestart" -> "steering_restart" [] x="Retry" -> "retry"
 [] x="ProviderRecovery" -> "provider_recovery" [] x="Running" -> "running"
 [] x="Settled" -> "settled" [] x="Always" -> "always" [] x="SettledOnly" -> "settled_only"
 [] x="Pending" -> "pending" [] x="Claimed" -> "claimed" [] x="Acknowledged" -> "acknowledged"
 [] x="Delivered" -> "delivered" [] x="Disposed" -> "disposed"
 [] x="Open" -> "open" [] x="Stopped" -> "stopped" [] x="Offered" -> "offered" [] x="Queued" -> "queued"
 [] OTHER -> x
PIThread(x) == IF x="none" THEN NoThread ELSE x
RECURSIVE PIMessage(_)
PIMessage(m) ==
 CASE m.kind="Queued" -> PIMessage(m.message)
 [] m.kind="Person" -> [k |-> "person",r |-> 0,g |-> 0]
 [] m.kind="Server" -> [k |-> "server",r |-> 0,g |-> 0]
 [] m.kind="Task" -> [k |-> "task",r |-> 0,g |-> 0]
 [] m.kind="Resume" -> [k |-> "resume",r |-> 0,g |-> 0]
 [] m.kind="Questions" -> [k |-> "wake",r |-> 0,g |-> 0]
 [] m.kind="Wake" -> [k |-> "wake",r |-> m.run,g |-> m.generation]
PIEvent(e) == [n |-> IF e.node="Root" THEN "root" ELSE "child",r |-> e.run,a |-> e.attempt,o |-> PILower(e.outcome)]
PIStatus(c,t) == c.threads[t]
PIHas(c,t) == t \in DOMAIN c.threads
PIOrd(c,t) == IF PIHas(c,t) THEN RunOrd(c,t) ELSE 0
PILive(c,t) == PIHas(c,t) /\ IsLive(c,t)
PILast(c,t) == LET rs==SelectSeq(c.threads[t].runs,LAMBDA r:r.status#"Live")
 IN IF Len(rs)=0 THEN "none" ELSE PILower(rs[Len(rs)].status)
PIKind(c,t) == IF Len(c.threads[t].runs)=0 THEN "none" ELSE PIMessage(c.threads[t].runs[Len(c.threads[t].runs)].start).k
PIQKeys(c,t) == {k \in DOMAIN c.questions:k[1]=t}
PIQuestion(c,t) == c.questions[CHOOSE k \in PIQKeys(c,t):TRUE]
PIQuestionReady(c,t) == PIQKeys(c,t)#{}
PIQuestionWake(c) == {t \in PIChildren: PIQuestionReady(c,t) /\
 LET q==PIQuestion(c,t) IN q.pending /\ q.holder#"none" /\ q.holder \notin q.seen /\ q.holder \notin q.waiting}
PIQuestionSingle(c) == \A t \in PIChildren:Cardinality(PIQKeys(c,t))<=1
PIHistoryComplete(c) == \A t \in DOMAIN c.threads:c.threads[t].run_base=0

(* Operational counters and finite obligation ledgers. No source image is
   accepted as ghost input. A driver must constrain nonnegative budgets. *)
PlanGhostInit == [
 budgets |-> [rPersonLeft |-> RLMaxPerson,serverLeft |-> RLMaxServer,rStopLeft |-> RLMaxStop,
 ulLeft |-> RLMaxUL,rCrashLeft |-> RLMaxCrash,dPersonLeft |-> DMaxPerson,failLeft |-> MaxFail,
 dCrashLeft |-> DMaxCrash,abandonLeft |-> MaxAbandon,dStopLeft |-> DMaxStop,
 narrowLeft |-> MaxNarrow,interruptLeft |-> MaxInterrupt],
 entered |-> {},answer_count |-> [t \in PIChildren |-> 0],stop_obligations |-> {},
 replayable |-> {},replay_bad |-> FALSE,receipt_bad |-> FALSE,notice_receipts |-> {},
 wait_attempt |-> [t \in Threads |-> 0],resume_wake |-> [t \in Threads |-> 0],
 clamp_pending |-> {},provider_access |-> [t \in Threads |-> [value |-> 0,run |-> 0,attempt |-> 0]],
 ct_tracked |-> {},consumed_kind |-> [x \in 1..MaxX |-> "none"],
 hosts |-> [t \in Threads |-> IF t=Root THEN RootDev ELSE "nodev"],
 online |-> [d \in Devices |-> TRUE],offline_left |-> MaxOffline]
PISpend(act,args,name) ==
 CASE name="rPersonLeft" -> act \in {"PersonSend","Steer","RestartActive"}
 [] name="dPersonLeft" -> act="PersonSend"
 [] name="serverLeft" -> act="ServerEnqueue"
 [] name="rStopLeft" -> act \in {"RequestInterrupt","Stop"}
 [] name="dStopLeft" -> act="Stop"
 [] name="ulLeft" -> act="ProviderTerminal" /\ args.outcome="UsageLimit"
 [] name="rCrashLeft" -> act="ProviderTerminal" /\ args.outcome="Crash"
 [] name="dCrashLeft" -> act="CrashEngine"
 [] name="abandonLeft" -> act="WaitAbandon"
 [] name="narrowLeft" -> act="NarrowAccess"
 [] name="interruptLeft" -> act="InterruptChat"
 [] OTHER -> FALSE
RECURSIVE PIAnc(_,_,_)
PIAnc(c,t,n) == IF n=0 \/ ~PIHas(c,t) \/ c.threads[t].parent="none" THEN {}
 ELSE {c.threads[t].parent} \cup PIAnc(c,c.threads[t].parent,n-1)
PIDesc(c,t) == {u \in DOMAIN c.threads:t \in PIAnc(c,u,Cardinality(Threads))}
PIWakeQueued(c,t) == PIHas(c,t) /\ \E i \in 1..Len(c.threads[t].queue):c.threads[t].queue[i].kind="Wake"
PIClosed(c,c2,t) == PIQuestionReady(c,t) /\ PIQuestion(c,t).pending /\
 PIQuestionReady(c2,t) /\ ~PIQuestion(c2,t).pending
PIEntered(c,fx) == {child \in PIChildren : \E i \in 1..Len(fx):
 fx[i].kind="StartRun" /\ child=fx[i].run.thread /\
 \E k \in DOMAIN c.threads:c.threads[k].parent=child /\ c.threads[k].task.present /\ c.threads[k].task.pushes>=1}
PlanGhostNext(c,c2,ev,fx,g) ==
 LET act==ev.act a==ev.args
     stopped==IF act="Stop" THEN {a.t} \cup PIDesc(c,a.t) ELSE {}
     sent==IF act="PersonSend" THEN {a.t} ELSE {}
     narrowed==IF act="NarrowAccess" THEN {t \in PIDesc(c,a.t):PILive(c,t) /\ t \notin InPlace} ELSE {}
 IN [g EXCEPT
  !.budgets=[k \in DOMAIN @ |-> @[k]-(IF PISpend(act,a,k) THEN 1 ELSE 0)],
  !.entered=(@ \ {t \in PIChildren:PIHas(c,t) /\ PIHas(c2,t) /\ PIOrd(c2,t)>PIOrd(c,t)}) \cup PIEntered(c,fx),
  !.answer_count=[t \in PIChildren |-> @[t]+(IF PIClosed(c,c2,t) THEN 1 ELSE 0)],
  !.stop_obligations=(@ \ sent) \cup stopped,
  !.wait_attempt=[t \in Threads |-> IF PIHas(c2,t) /\ c2.threads[t].wait_on#"none" /\
    (~PIHas(c,t) \/ c.threads[t].wait_on#c2.threads[t].wait_on) THEN c2.threads[t].attempt.n ELSE @[t]],
  !.resume_wake=[t \in Threads |-> IF PIHas(c2,t) /\ PIOrd(c2,t)>PIOrd(c,t) /\ PIKind(c2,t)="resume" /\ PIWakeQueued(c,t) THEN 1
     ELSE IF @[t]=1 /\ PIWakeQueued(c,t) /\ ~PIWakeQueued(c2,t) THEN 2 ELSE @[t]],
  !.clamp_pending=(@ \ {t \in Threads:PIHas(c,t) /\ PIHas(c2,t) /\ c2.threads[t].attempt.n>c.threads[t].attempt.n}) \cup narrowed,
  !.ct_tracked=@ \ {t \in Threads:PIHas(c,t) /\ PIHas(c2,t) /\ PILive(c,t) /\ ~PILive(c2,t)},
  !.notice_receipts=IF act="RemoteNotice" THEN @ \cup {<<a.child,a.run>>} ELSE @,
  !.hosts=[t \in Threads |-> IF ~PIHas(c,t) /\ PIHas(c2,t) /\ t#Root THEN
     IF "host" \in DOMAIN a THEN a.host ELSE RootDev ELSE @[t]]]

(* I/O updates have explicit operational inputs, separate from core. *)
PlanGhostProviderAccess(g,t,r,a,value) == [g EXCEPT
 !.provider_access[t]=[value |-> value,run |-> r,attempt |-> a],
 !.clamp_pending=IF t \in InPlace THEN @ \ {t} ELSE @]
PlanGhostCT(g,ctBefore,ctAfter,c2) == [g EXCEPT
 !.consumed_kind=[x \in 1..MaxX |-> IF ctBefore.ledger[x].st#"consumed" /\ ctAfter.ledger[x].st="consumed"
 THEN PIKind(c2,ctAfter.ledger[x].tgt) ELSE @[x]],
 !.ct_tracked=@ \cup {t \in Threads:ctAfter.runs[t].st#"idle"}]

PIEffect(c,m) ==
 CASE m.kind="Offer" -> [k |-> "offer",t |-> m.parent,r |-> m.run,g |-> m.generation]
 [] m.kind="Notice" -> [k |-> "notice",t |-> m.child,r |-> m.run,g |-> 0]
 [] m.kind="Stop" -> [k |-> "stop",t |-> m.child,r |-> m.stop,g |-> 0]
 [] m.kind="QuestionUp" -> [k |-> "qup",t |-> m.child,r |-> m.epoch,g |-> IF m.to="none" THEN 0 ELSE c.threads[m.to].depth]
 [] m.kind="AnswerDown" -> [k |-> "ans",t |-> m.child,r |-> m.epoch,g |-> 0]
 [] m.kind="QuestionRelease" -> [k |-> "rel",t |-> m.child,r |-> m.epoch,g |-> c.threads[m.via].depth]
 [] m.kind="QuestionClosed" -> [k |-> "qclosed",t |-> m.child,r |-> m.epoch,g |-> IF m.to="none" THEN 0 ELSE c.threads[m.to].depth]
PICohort(c,t,r) == IF PIHas(c,t) /\ HasRun(c,t,r) THEN c.threads[t].runs[RunIndex(c,t,r)].cohort ELSE DefaultCohort
PlanImage(c,ni,xi,g,focus) ==
 LET on==DOMAIN c.threads
     baseImage==[
 queue |-> [t \in Threads |-> IF t \in on THEN LET q==c.threads[t].queue IN [i \in 1..Len(q)|->PIMessage(q[i])] ELSE <<>>],
 held |-> [t \in Threads |-> IF t \in on THEN PILower(c.threads[t].held) ELSE "none"],
 liveSet |-> [t \in Threads |-> IF t \in on THEN {c.threads[t].run_base+i:i \in {j \in 1..Len(c.threads[t].runs):c.threads[t].runs[j].status="Live"}} ELSE {}],
 rord |-> [t \in Threads |-> PIOrd(c,t)],intReq |-> [t \in Threads |-> t \in on /\ c.threads[t].int_req],
 att |-> [t \in Threads |-> IF t \in on THEN c.threads[t].attempt.n ELSE 0],
 why |-> [t \in Threads |-> IF t \in on THEN PILower(c.threads[t].attempt.reason) ELSE "initial"],
 emitted |-> [t \in Threads |-> t \in on /\ c.threads[t].attempt.emitted],
 kidOpen |-> [t \in Threads |-> t \in on /\ c.threads[t].attempt.kid_open],
 pev |-> [t \in Threads |-> IF t \in on THEN {PIEvent(e):e \in c.threads[t].inbox} ELSE {}],
 retries |-> [t \in Threads |-> IF t \in on THEN c.threads[t].attempt.retries ELSE 0],
 steers |-> [t \in Threads |-> IF t \in on THEN c.threads[t].attempt.steers ELSE 0],
 lastOut |-> [t \in Threads |-> IF t \in on THEN PILast(c,t) ELSE "none"],
 rkind |-> [t \in Threads |-> IF t \in on THEN PIKind(c,t) ELSE "none"],
 ulArmed |-> [t \in Threads |-> IF t \in on THEN c.threads[t].ul.armed ELSE 0],
 ulDue |-> [t \in Threads |-> t \in on /\ c.threads[t].ul.due],
 created |-> [t \in Threads |-> t \in on],parent |-> [t \in Threads |-> IF t \in on THEN PIThread(c.threads[t].parent) ELSE NoThread],
 depth |-> [t \in Threads |-> IF t \in on THEN c.threads[t].depth ELSE 0],
 access |-> [t \in Threads |-> IF t \in on THEN c.threads[t].access ELSE 0],
 rtask |-> [t \in Threads |-> t \in on /\ PIKind(c,t)="task"],
 waitOn |-> [t \in Threads |-> IF t \in on THEN PIThread(c.threads[t].wait_on) ELSE NoThread],
 zWait |-> [t \in Threads |-> IF t \in on THEN PIThread(c.threads[t].late_wait) ELSE NoThread],
 tSt |-> [t \in PIChildren |-> IF t \in on /\ c.threads[t].task.present THEN PILower(c.threads[t].task.st) ELSE "none"],
 tWake |-> [t \in PIChildren |-> IF t \in on /\ c.threads[t].task.present THEN PILower(c.threads[t].task.wake) ELSE "always"],
 tDel |-> [t \in PIChildren |-> IF t \in on /\ c.threads[t].task.present THEN PILower(c.threads[t].task.delivery) ELSE "pending"],
 tCoh |-> [t \in PIChildren |-> IF t \in on /\ c.threads[t].task.present THEN c.threads[t].task.cohort ELSE 0],
 pushes |-> [t \in PIChildren |-> IF t \in on /\ c.threads[t].task.present THEN c.threads[t].task.pushes ELSE 0],
 kidIn |-> [t \in PIChildren |-> t \in g.entered],
 qPend |-> [t \in PIChildren |-> PIQuestionReady(c,t) /\ PIQuestion(c,t).pending],
 qAt |-> [t \in PIChildren |-> IF PIQuestionReady(c,t) THEN PIThread(PIQuestion(c,t).holder) ELSE NoThread],
 qVis |-> [t \in PIChildren |-> IF PIQuestionReady(c,t) THEN PIQuestion(c,t).views \cap PIQuestion(c,t).seen ELSE {}],
 qWake |-> PIQuestionWake(c),qUsed |-> [t \in PIChildren |-> PIQuestionReady(c,t)],answers |-> g.answer_count,
 qEp |-> [t \in PIChildren |-> IF PIQuestionReady(c,t) THEN PIQuestion(c,t).epoch ELSE 0],
 cDisp |-> [t \in Threads |-> [r \in PIOrds |-> PILower(PICohort(c,t,r).disp)]],
 cSt |-> [t \in Threads |-> [r \in PIOrds |-> PILower(PICohort(c,t,r).st)]],
 cDel |-> [t \in Threads |-> [r \in PIOrds |-> PICohort(c,t,r).del]],
 cGen |-> [t \in Threads |-> [r \in PIOrds |-> PICohort(c,t,r).generation]],
 outbox |-> {PIEffect(c,m):m \in c.outbox},replayable |-> g.replayable,
 rcpt |-> c.stop_receipts \cup {<<r[1],(IF DMaxStop>1 THEN DMaxStop ELSE 1)+r[2]>>:r \in g.notice_receipts},
 replayBad |-> g.replay_bad,receiptBad |-> g.receipt_bad,doomed |-> g.stop_obligations,
 ctTrk |-> [t \in Threads |-> t \in g.ct_tracked],consumedBy |-> g.consumed_kind,
 attAcc |-> [t \in Threads |-> g.provider_access[t].value],
 attAccAt |-> [t \in Threads |-> [run |-> g.provider_access[t].run,attempt |-> g.provider_access[t].attempt]],
 waitAtt |-> g.wait_attempt,f9w |-> g.resume_wake,owed |-> g.clamp_pending,
 host |-> g.hosts,online |-> g.online,offLeft |-> g.offline_left]
 IN baseImage @@ g.budgets @@ ni @@ xi
=============================================================================
