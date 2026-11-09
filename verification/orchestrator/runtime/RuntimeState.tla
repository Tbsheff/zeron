--------------------------- MODULE RuntimeState ---------------------------
EXTENDS Naturals, Sequences, FiniteSets, TLC
DefaultCohort == [disp |-> "Open", st |-> "None", del |-> {}, generation |-> 0]
DefaultAttempt == [n |-> 0, reason |-> "Initial", emitted |-> FALSE,
                   kid_open |-> FALSE, retries |-> 0, steers |-> 0]
DefaultUL == [armed |-> 0, due |-> FALSE]
NewRun(start,status) == [start |-> start, status |-> status, cohort |-> DefaultCohort]
TaskStart == [kind |-> "Queued", message |-> [kind |-> "Task"]]
NewThread(id,parent,depth,access) ==
 [id |-> id, parent |-> parent, depth |-> depth, access |-> access,
  reconfigure |-> FALSE, queue |-> <<>>, held |-> "None", run_base |-> 0,
  runs |-> <<>>, int_req |-> FALSE, attempt |-> DefaultAttempt,
  inbox |-> {}, ul |-> DefaultUL, wait_on |-> "none", late_wait |-> "none",
  task |-> [present |-> FALSE]]
RunOrd(s,t) == s.threads[t].run_base + Len(s.threads[t].runs)
IsLive(s,t) == \E i \in 1..Len(s.threads[t].runs): s.threads[t].runs[i].status = "Live"
Handled(d) == d \in {"Delivered","Acknowledged","Disposed"}
RunIndex(s,t,r) == r - s.threads[t].run_base
HasRun(s,t,r) == r > s.threads[t].run_base /\ r <= RunOrd(s,t)
PutThread(s,t,v) == [s EXCEPT !.threads = [x \in DOMAIN @ \cup {t} |-> IF x=t THEN v ELSE @[x]],
 !.thread_order = IF t \in {s.thread_order[i]: i \in 1..Len(s.thread_order)} THEN @ ELSE Append(@,t)]
PutRun(s,t,r,v) == [s EXCEPT !.threads[t].runs[RunIndex(s,t,r)] = v]
CurrentAttempt(s,t) == [run |-> [thread |-> t, ord |-> RunOrd(s,t)], n |-> s.threads[t].attempt.n]
RECURSIVE NativeAncestorInvalid(_,_,_,_)
NativeAncestorInvalid(s,parent,ancestor,depth) ==
 IF ancestor="none" THEN FALSE
 ELSE IF depth>=Cardinality(DOMAIN s.natives) THEN TRUE
 ELSE LET owner == <<parent,ancestor>> IN
      IF owner \notin DOMAIN s.natives THEN TRUE
      ELSE IF s.natives[owner].status#"Running" THEN TRUE
      ELSE NativeAncestorInvalid(s,parent,s.natives[owner].ancestor,depth+1)
NativeAncestorStale(s,k,n) == NativeAncestorInvalid(s,k[1],n.ancestor,0)
StaleNative(s,k) == LET n == s.natives[k] IN
 n.status = "Running" /\ ~n.background /\
 ((IF k[1] \notin DOMAIN s.threads THEN TRUE
   ELSE ~IsLive(s,k[1]) \/ CurrentAttempt(s,k[1]) # n.attempt)
  \/ NativeAncestorStale(s,k,n))
Reconcile(s) == [s EXCEPT !.natives = [k \in DOMAIN @ |->
 IF StaleNative(s,k) THEN [@[k] EXCEPT !.status="Orphaned", !.delivery="Disposed"] ELSE @[k]]]
ReconcileEffects(s) == LET keys == SelectSeq(s.native_order,LAMBDA k: StaleNative(s,k)) IN
 [i \in 1..Len(keys) |-> [kind |-> "NativeChanged",parent |-> keys[i][1],key |-> keys[i][2]]]
OldestQuiet(s,t) == LET th == s.threads[t] o == th.run_base+1 r == th.runs[1] IN
 /\ r.status # "Live" /\ r.cohort.st = "None" /\ r.cohort.del = {}
 /\ th.ul.armed # o
 /\ ~\E e \in th.inbox: e.run=o
 /\ ~\E i \in 1..Len(th.queue): th.queue[i].kind="Wake" /\ th.queue[i].run=o
 /\ ~\E m \in s.outbox: m.kind="Offer" /\ m.parent=t /\ m.run=o
 /\ \A c \in DOMAIN s.threads: LET ct == s.threads[c] IN
    ct.parent # t \/ ~ct.task.present \/ ct.task.cohort # o \/
    (ct.task.st="Settled" /\ Handled(ct.task.delivery))
RECURSIVE RetireOne(_, _)
RetireOne(s,t) == IF Len(s.threads[t].runs) > (IF s.config.run_window>1 THEN s.config.run_window ELSE 1) /\ OldestQuiet(s,t)
 THEN RetireOne([s EXCEPT !.threads[t].runs=Tail(@),!.threads[t].run_base=@+1],t) ELSE s
RECURSIVE RetireList(_, _)
RetireList(s,ts) == IF Len(ts)=0 THEN s ELSE RetireList(RetireOne(s,Head(ts)),Tail(ts))
RetireRuns(s) == RetireList(s,s.thread_order)
Finalize(s) == RetireRuns(Reconcile(s))
=============================================================================
