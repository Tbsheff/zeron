---------------- MODULE RuntimeNativeQuestionSeams ----------------
(* Exact state relations for the native and question reducers. All map keys
   use stable chat IDs and opaque decimal-string u64 IDs. Unknown events fail.
   Finalization (native reconciliation and run retirement) belongs to caller. *)
EXTENDS RuntimeState

NQCap == 2147483647
NQInc(n) == n + 1
NQLive(th) == \E i \in 1..Len(th.runs) : th.runs[i].status = "Live"
NQOrd(th) == th.run_base + Len(th.runs)
NQAttempt(th) == [run |-> [thread |-> th.id, ord |-> NQOrd(th)], n |-> th.attempt.n]
NQBlocked(s, t) == \E k \in DOMAIN s.questions : s.questions[k].child = t /\ s.questions[k].pending
NQPut(s, k, q) == [s EXCEPT !.questions = (k :> q) @@ @]
NQSend(s, m) == [s EXCEPT !.outbox = @ \cup {m}]
NQUp(c, q, e, t) == [kind |-> "QuestionUp", child |-> c, qid |-> q, epoch |-> e, to |-> t]
NQRelease(c, q, e, t) == [kind |-> "QuestionRelease", child |-> c, qid |-> q, epoch |-> e, via |-> t]
NQClosed(c, q, e, t) == [kind |-> "QuestionClosed", child |-> c, qid |-> q, epoch |-> e, to |-> t]
NQAnswer(c, q, e, t, a) == [kind |-> "AnswerDown", child |-> c, qid |-> q, epoch |-> e, via |-> t, answer |-> a]
NQEmpty(c, q, e) == [child |-> c, qid |-> q, epoch |-> e, pending |-> TRUE,
  holder |-> "none", at_person |-> FALSE, views |-> {}, waiting |-> {}, seen |-> {}, future_answers |-> [x \in {} |-> x]]

RECURSIVE NQAncestors(_, _), NQHop(_, _, _)
NQAncestors(s, t) == IF s.threads[t].parent = "none" THEN {}
  ELSE {s.threads[t].parent} \cup NQAncestors(s, s.threads[t].parent)
NQHop(s, t, via) == IF s.threads[t].parent = "none" \/ s.threads[t].parent = via
  THEN t ELSE NQHop(s, s.threads[t].parent, via)

(* A close changes ownership first. View retraction is a separate durable row. *)
NQClose(s, c, qid, epoch, answer) ==
  LET k == <<c, qid>> IN
  IF k \notin DOMAIN s.questions THEN s
  ELSE LET q == s.questions[k] IN
    IF ~q.pending \/ q.epoch # epoch THEN s
    ELSE LET viewers == q.views \cup NQAncestors(s, c)
             closed == [q EXCEPT !.pending = FALSE, !.holder = "none", !.at_person = FALSE]
         IN [NQPut(s, k, closed) EXCEPT !.outbox = @ \cup {NQClosed(c, qid, epoch, t) : t \in viewers}]

QuestionReroute(s, t, all) ==
  LET keys == {k \in DOMAIN s.questions : LET q == s.questions[k] IN
                  q.pending /\ q.holder = t /\ (~(t \in q.waiting) \/ all) /\ (all \/ t \in q.seen)}
      qs == [k \in DOMAIN s.questions |-> IF k \notin keys THEN s.questions[k]
             ELSE [s.questions[k] EXCEPT !.views = @ \ {t}, !.seen = @ \ {t},
                   !.waiting = @ \ {t}, !.holder = "none", !.epoch = NQInc(@)]]
      msgs == UNION {{NQRelease(qs[k].child, qs[k].qid, qs[k].epoch, t),
                     NQUp(qs[k].child, qs[k].qid, qs[k].epoch, s.threads[t].parent)} : k \in keys}
  IN [s EXCEPT !.questions = qs, !.outbox = @ \cup msgs]
QuestionRerouteSafe(s, t, all) == \A k \in DOMAIN s.questions :
  (s.questions[k].pending /\ s.questions[k].holder = t /\
   (~(t \in s.questions[k].waiting) \/ all) /\ (all \/ t \in s.questions[k].seen))
    => s.questions[k].epoch < NQCap
QuestionEndRun(s, t) ==
  QuestionReroute([s EXCEPT !.questions = [k \in DOMAIN @ |->
     IF s.questions[k].pending THEN [s.questions[k] EXCEPT !.waiting = @ \ {t}]
     ELSE s.questions[k]]], t, s.threads[t].held # "None")

NQDeliverBody(s, m) ==
  LET k == <<m.child, m.qid>> IN
  IF k \notin DOMAIN s.questions THEN s
  ELSE LET q == s.questions[k] IN
  CASE m.kind = "QuestionUp" ->
    IF ~q.pending \/ q.epoch # m.epoch THEN s
    ELSE IF m.to = "none" THEN NQPut(s, k, [q EXCEPT !.at_person = TRUE, !.holder = "none"])
    ELSE LET th == s.threads[m.to] IN
      IF NQBlocked(s, m.to) \/ th.held # "None"
      THEN NQSend(s, NQUp(m.child, m.qid, m.epoch, th.parent))
      ELSE NQPut(s, k, [q EXCEPT !.holder = m.to, !.views = @ \cup {m.to},
                        !.waiting = IF NQLive(th) /\ th.wait_on = m.child THEN @ \cup {m.to} ELSE @])
  [] m.kind = "AnswerDown" ->
    IF ~q.pending \/ q.epoch > m.epoch THEN s
    ELSE IF q.epoch < m.epoch THEN
      IF m.epoch \in DOMAIN q.future_answers THEN s
      ELSE NQPut(s, k, [q EXCEPT !.future_answers = @ @@ (m.epoch :> m.answer)])
    ELSE LET hop == NQHop(s, m.child, m.via) IN
      IF hop = m.child THEN NQClose(s, m.child, m.qid, m.epoch, m.answer)
      ELSE NQSend(s, NQAnswer(m.child, m.qid, m.epoch, hop, m.answer))
  [] m.kind = "QuestionRelease" ->
    IF ~q.pending \/ m.epoch < q.epoch THEN s
    ELSE LET hop == NQHop(s, m.child, m.via)
             buffered == m.epoch \in DOMAIN q.future_answers
             next == NQPut(s, k, [q EXCEPT !.epoch = m.epoch,
                        !.future_answers = [e \in DOMAIN q.future_answers \ {m.epoch} |-> q.future_answers[e]]])
         IN IF hop = m.child THEN
              IF buffered THEN NQClose(next, m.child, m.qid, m.epoch, q.future_answers[m.epoch]) ELSE next
            ELSE LET released == NQSend(next, NQRelease(m.child, m.qid, m.epoch, hop)) IN
              IF buffered THEN NQSend(released, NQAnswer(m.child, m.qid, m.epoch, hop, q.future_answers[m.epoch]))
              ELSE released
  [] m.kind = "QuestionClosed" ->
    IF q.epoch # m.epoch THEN s
    ELSE NQPut(s, k, [q EXCEPT !.views = @ \ {m.to}, !.waiting = @ \ {m.to}, !.seen = @ \ {m.to}])
  [] OTHER -> s

NQDeliver(s, m, how) ==
  LET removed == IF how = "Ack" THEN [s EXCEPT !.outbox = @ \ {m}] ELSE s IN
  IF m \in s.question_receipts THEN removed
  ELSE NQDeliverBody([removed EXCEPT !.question_receipts = @ \cup {m}], m)
NQDeliveryGuard(s, m, how) ==
  /\ how \in {"Ack", "Keep"} /\ m \in s.outbox
  /\ m.kind \in {"QuestionUp", "AnswerDown", "QuestionRelease", "QuestionClosed"}
  /\ LET k == <<m.child,m.qid>> IN
      IF m \in s.question_receipts \/ k \notin DOMAIN s.questions THEN TRUE
      ELSE LET q == s.questions[k] IN
       CASE m.kind = "QuestionUp" -> (~q.pending \/ q.epoch # m.epoch \/ m.to = "none" \/ m.to \in DOMAIN s.threads)
         [] m.kind = "AnswerDown" -> (~q.pending \/ q.epoch # m.epoch \/ m.child \in DOMAIN s.threads)
         [] m.kind = "QuestionRelease" -> (~q.pending \/ m.epoch < q.epoch \/ m.child \in DOMAIN s.threads)
         [] OTHER -> TRUE

NQRaise(s, next, a) ==
  LET th == s.threads[a.child] k == <<a.child, a.qid>> IN
  /\ a.child \in DOMAIN s.threads /\ NQLive(th) /\ th.wait_on = "none"
  /\ th.held # "Stop" /\ ~th.int_req
  /\ IF k \in DOMAIN s.questions THEN next = s
     ELSE /\ ~NQBlocked(s, a.child) /\ QuestionRerouteSafe(s, a.child, TRUE)
          /\ next = QuestionReroute(NQSend(NQPut(s, k, NQEmpty(a.child, a.qid, 1)),
               NQUp(a.child, a.qid, 1, th.parent)), a.child, TRUE)

NQImportUp(s, next, a) ==
  LET k == <<a.child, a.qid>> IN
  /\ a.child \in DOMAIN s.threads /\ a.to \in DOMAIN s.threads
  /\ IF k \in DOMAIN s.questions /\ (~s.questions[k].pending \/ a.epoch < s.questions[k].epoch)
     THEN next = s
     ELSE LET q == IF k \in DOMAIN s.questions THEN [s.questions[k] EXCEPT !.epoch = a.epoch]
                   ELSE NQEmpty(a.child, a.qid, a.epoch)
              m == NQUp(a.child, a.qid, a.epoch, a.to)
              staged == NQSend(NQPut(s, k, q), m)
          IN next = NQDeliver(staged, m, "Ack")
NQImportClosed(s, next, a) ==
  LET k == <<a.child, a.qid>>
      exists == k \in DOMAIN s.questions
      e == IF exists THEN IF s.questions[k].epoch > a.epoch THEN s.questions[k].epoch ELSE a.epoch ELSE a.epoch
      staged == IF exists THEN NQPut(s, k, [s.questions[k] EXCEPT !.pending = FALSE,
                    !.holder = "none", !.at_person = FALSE, !.epoch = e]) ELSE s
      m == NQClosed(a.child, a.qid, e, a.to)
      delivered == NQDeliver(NQSend(staged, m), m, "Ack")
  IN /\ a.to \in DOMAIN s.threads
     /\ next = IF s.threads[a.to].parent = "none" THEN delivered
        ELSE NQSend(delivered, NQClosed(a.child, a.qid, e, s.threads[a.to].parent))
NQImportAnswer(s, next, a) ==
  LET m == NQAnswer(a.child, a.qid, a.epoch, a.via, a.answer) IN
  /\ NQDeliveryGuard(NQSend(s,m),m,"Ack") /\ next = NQDeliver(NQSend(s, m), m, "Ack")
NQImportRelease(s, next, a) ==
  LET m == NQRelease(a.child, a.qid, a.epoch, a.via) IN
  /\ NQDeliveryGuard(NQSend(s,m),m,"Ack") /\ next = NQDeliver(NQSend(s, m), m, "Ack")
NQAgentAnswer(s, next, a) ==
  LET th == s.threads[a.agent] k == <<a.child, a.qid>> IN
  /\ a.agent \in DOMAIN s.threads /\ NQLive(th) /\ ~NQBlocked(s, a.agent)
  /\ IF k \notin DOMAIN s.questions THEN next = s
     ELSE LET q == s.questions[k] IN
       IF ~q.pending \/ q.epoch # a.epoch THEN next = s
       ELSE /\ q.holder = a.agent /\ a.agent \in q.seen
            /\ next = NQSend(s, NQAnswer(a.child, a.qid, a.epoch, a.agent, a.answer))

NQBeginQuestions(th) == [th EXCEPT
  !.runs = Append(@, [start |-> [kind |-> "Questions"], status |-> "Live",
    cohort |-> [disp |-> "Open", st |-> "None", del |-> {}, generation |-> 0]]),
  !.int_req = FALSE, !.attempt = [n |-> 1, reason |-> "Initial", emitted |-> FALSE,
    kid_open |-> FALSE, retries |-> 0, steers |-> 0],
  !.ul = DefaultUL, !.wait_on = "none"]
NQStartQuestion(s, next, a) ==
  LET th == s.threads[a.t]
      keys == {k \in DOMAIN s.questions : LET q == s.questions[k] IN q.pending /\ q.holder = a.t /\ a.t \in q.views}
  IN /\ a.t \in DOMAIN s.threads /\ ~NQLive(th) /\ th.held = "None" /\ ~NQBlocked(s, a.t)
     /\ keys # {} /\ NQOrd(th) < NQCap
     /\ next = [s EXCEPT !.threads[a.t] = NQBeginQuestions(th),
        !.questions = [k \in DOMAIN @ |-> IF k \in keys THEN [s.questions[k] EXCEPT !.seen = @ \cup {a.t}]
                                           ELSE s.questions[k]]]

NQNativeStart(s, next, a) ==
  LET k == <<a.parent, a.key>> IN
  IF k \in DOMAIN s.natives THEN next = s
  ELSE /\ a.parent \in DOMAIN s.threads /\ NQLive(s.threads[a.parent])
       /\ next = [s EXCEPT !.natives = @ @@ (k :> [attempt |-> NQAttempt(s.threads[a.parent]),
          background |-> a.background, ancestor |-> "none", status |-> "Running", delivery |-> "Pending", generation |-> 0])]
NQNativeStartNested(s,next,a) ==
 LET k == <<a.parent,a.key>> ownerKey == <<a.parent,a.ancestor>> IN
 IF k \in DOMAIN s.natives THEN next=s
 ELSE /\ a.key # a.ancestor /\ ownerKey \in DOMAIN s.natives
      /\ LET owner == s.natives[ownerKey] IN
         /\ owner.status="Running" /\ a.parent \in DOMAIN s.threads
         /\ (owner.background \/ (NQLive(s.threads[a.parent]) /\ ~s.threads[a.parent].attempt.emitted /\ NQAttempt(s.threads[a.parent])=owner.attempt))
         /\ next=[s EXCEPT !.natives=@ @@ (k :> [attempt |-> owner.attempt,background |-> a.background \/ owner.background,
              ancestor |-> a.ancestor,status |-> "Running",delivery |-> "Pending",generation |-> 0])]
NQNativeEnd(s, next, a) ==
  LET k == <<a.parent, a.key>> n == s.natives[k]
      inline == IF a.parent \notin DOMAIN s.threads THEN FALSE
                ELSE NQLive(s.threads[a.parent]) /\ NQAttempt(s.threads[a.parent]) = n.attempt
  IN /\ k \in DOMAIN s.natives
     /\ next = IF n.status # "Running" THEN s
        ELSE [s EXCEPT !.natives[k].status = IF a.failed THEN "Failed" ELSE "Completed",
                       !.natives[k].delivery = IF inline THEN "Delivered" ELSE "Claimed"]
NQNativeClaim(s, next, a) ==
  LET k == <<a.parent, a.key>> n == s.natives[k] IN
  /\ k \in DOMAIN s.natives /\ n.status # "Running"
  /\ next = IF n.delivery = "Pending" THEN [s EXCEPT !.natives[k].delivery = "Claimed"] ELSE s
NQNativeDelivered(s, next, a) ==
  LET k == <<a.parent, a.key>> n == s.natives[k] IN
  /\ k \in DOMAIN s.natives
  /\ next = IF n.delivery = "Claimed" /\ n.generation = a.generation
            THEN [s EXCEPT !.natives[k].delivery = "Delivered"] ELSE s
NQNativeRecover(s, next, a) ==
  LET k == <<a.parent, a.key>> n == s.natives[k] IN
  /\ k \in DOMAIN s.natives /\ (n.delivery = "Claimed" => n.generation < NQCap)
  /\ next = IF n.delivery = "Claimed" THEN [s EXCEPT !.natives[k].generation = @ + 1] ELSE s

NativeQuestionRawApply(s, next, ev) ==
  CASE ev.act = "NativeStart" -> NQNativeStart(s, next, ev.args)
    [] ev.act = "NativeStartNested" -> NQNativeStartNested(s,next,ev.args)
    [] ev.act = "NativeEnd" -> NQNativeEnd(s, next, ev.args)
    [] ev.act = "NativeClaim" -> NQNativeClaim(s, next, ev.args)
    [] ev.act = "NativeDelivered" -> NQNativeDelivered(s, next, ev.args)
    [] ev.act = "NativeRecover" -> NQNativeRecover(s, next, ev.args)
    [] ev.act = "RaiseQuestion" -> NQRaise(s, next, ev.args)
    [] ev.act = "ImportQuestion" -> NQImportUp(s, next, ev.args)
    [] ev.act = "ImportClosed" -> NQImportClosed(s, next, ev.args)
    [] ev.act = "ImportAnswer" -> NQImportAnswer(s, next, ev.args)
    [] ev.act = "ImportRelease" -> NQImportRelease(s, next, ev.args)
    [] ev.act = "AgentAnswer" -> NQAgentAnswer(s, next, ev.args)
    [] ev.act = "PersonAnswer" -> next = NQClose(s, ev.args.child, ev.args.qid, ev.args.epoch, ev.args.answer)
    [] ev.act = "StartQuestion" -> NQStartQuestion(s, next, ev.args)
    [] ev.act = "DeliverQuestion" -> /\ NQDeliveryGuard(s, ev.args.message, ev.args.how)
       /\ next = NQDeliver(s, ev.args.message, ev.args.how)
    [] OTHER -> FALSE

(* Preserve the source numeric iteration order across stable ID renaming. *)
RECURSIVE NQConcat(_)
NQConcat(ss) == IF Len(ss) = 0 THEN <<>> ELSE Head(ss) \o NQConcat(Tail(ss))
NQFxSend(m) == [kind |-> "Send", message |-> m]
NQFxView(q, t, retract) == [kind |-> "QuestionView", viewer |-> t,
  child |-> q.child, qid |-> q.qid, epoch |-> q.epoch, retract |-> retract]
NQCloseFx(s, c, qid, epoch, answer) ==
  LET k == <<c, qid>> IN IF k \notin DOMAIN s.questions THEN <<>>
  ELSE LET q == s.questions[k] IN IF ~q.pending \/ q.epoch # epoch THEN <<>>
  ELSE LET viewers == q.views \cup NQAncestors(s,c) IN
    <<[kind |-> "RespondInput", child |-> c, qid |-> qid, epoch |-> epoch, answer |-> answer]>> \o
    NQConcat([i \in 1..Len(s.thread_order) |-> LET t == s.thread_order[i] IN
      IF t \in viewers THEN <<NQFxSend(NQClosed(c,qid,epoch,t))>> ELSE <<>>])
QuestionRerouteFx(s,t,all) ==
  NQConcat([i \in 1..Len(s.question_order) |-> LET q == s.questions[s.question_order[i]] IN
    IF q.pending /\ q.holder = t /\ (~(t \in q.waiting) \/ all) /\ (all \/ t \in q.seen)
    THEN <<NQFxView(q,t,TRUE), NQFxSend(NQRelease(q.child,q.qid,NQInc(q.epoch),t)),
           NQFxSend(NQUp(q.child,q.qid,NQInc(q.epoch),s.threads[t].parent))>> ELSE <<>>])
QuestionCatchup(s,t) == [s EXCEPT !.questions = [k \in DOMAIN @ |->
  IF s.questions[k].pending THEN [s.questions[k] EXCEPT !.waiting = @ \ {t}] ELSE s.questions[k]]]
QuestionEndRunFx(s,t) ==
  NQConcat([i \in 1..Len(s.question_order) |-> LET q == s.questions[s.question_order[i]] IN
    IF q.pending /\ t \in q.waiting THEN <<NQFxView(q,t,FALSE)>> ELSE <<>>]) \o
  QuestionRerouteFx(QuestionCatchup(s,t),t,s.threads[t].held # "None")
Reroute(s,t,all) == [state |-> QuestionReroute(s,t,all), fx |-> QuestionRerouteFx(s,t,all)]
EndRunQuestions(s,t) == [state |-> QuestionEndRun(s,t), fx |-> QuestionEndRunFx(s,t)]

NQDeliverBodyFx(s,m) ==
  LET k == <<m.child,m.qid>> IN IF k \notin DOMAIN s.questions THEN <<>>
  ELSE LET q == s.questions[k] IN
  CASE m.kind = "QuestionUp" ->
    IF ~q.pending \/ q.epoch # m.epoch THEN <<>>
    ELSE IF m.to = "none" THEN <<[kind |-> "NotifyQuestion", child |-> m.child, qid |-> m.qid, epoch |-> m.epoch]>>
    ELSE LET th == s.threads[m.to] IN
      IF NQBlocked(s,m.to) \/ th.held # "None" THEN <<NQFxSend(NQUp(m.child,m.qid,m.epoch,th.parent))>>
      ELSE IF NQLive(th) /\ th.wait_on = m.child THEN <<>> ELSE <<NQFxView(q,m.to,FALSE)>>
  [] m.kind = "AnswerDown" ->
    IF ~q.pending \/ q.epoch # m.epoch THEN <<>>
    ELSE LET hop == NQHop(s,m.child,m.via) IN
      IF hop = m.child THEN NQCloseFx(s,m.child,m.qid,m.epoch,m.answer)
      ELSE <<NQFxSend(NQAnswer(m.child,m.qid,m.epoch,hop,m.answer))>>
  [] m.kind = "QuestionRelease" ->
    IF ~q.pending \/ m.epoch < q.epoch THEN <<>>
    ELSE LET hop == NQHop(s,m.child,m.via)
             buffered == m.epoch \in DOMAIN q.future_answers
             changed == NQPut(s,k,[q EXCEPT !.epoch=m.epoch,
                !.future_answers=[e \in DOMAIN q.future_answers \ {m.epoch} |-> q.future_answers[e]]])
         IN IF hop = m.child THEN
             IF buffered THEN NQCloseFx(changed,m.child,m.qid,m.epoch,q.future_answers[m.epoch]) ELSE <<>>
            ELSE <<NQFxSend(NQRelease(m.child,m.qid,m.epoch,hop))>> \o
             IF buffered THEN <<NQFxSend(NQAnswer(m.child,m.qid,m.epoch,hop,q.future_answers[m.epoch]))>> ELSE <<>>
  [] m.kind = "QuestionClosed" -> IF q.epoch = m.epoch THEN <<NQFxView(q,m.to,TRUE)>> ELSE <<>>
  [] OTHER -> <<>>
NQDeliverFx(s,m,how) == IF m \in s.question_receipts THEN <<>> ELSE NQDeliverBodyFx(s,m)
NQNativeChanged(a) == [kind |-> "NativeChanged", parent |-> a.parent, key |-> a.key]
NQDeliverNative(a,g) == [kind |-> "DeliverNative", parent |-> a.parent, key |-> a.key, generation |-> g]
NQExpectedFx(s,ev) ==
  LET a == ev.args IN
  CASE ev.act \in {"NativeStart","NativeStartNested"} -> IF <<a.parent,a.key>> \in DOMAIN s.natives THEN <<>> ELSE <<NQNativeChanged(a)>>
  [] ev.act = "NativeEnd" -> LET n == s.natives[<<a.parent,a.key>>] IN
      IF n.status # "Running" THEN <<>>
      ELSE (IF a.parent \in DOMAIN s.threads /\ NQLive(s.threads[a.parent]) /\ NQAttempt(s.threads[a.parent])=n.attempt
            THEN <<>> ELSE <<NQDeliverNative(a,n.generation)>>) \o <<NQNativeChanged(a)>>
  [] ev.act = "NativeClaim" -> LET n == s.natives[<<a.parent,a.key>>] IN
      IF n.delivery = "Pending" THEN <<NQDeliverNative(a,n.generation)>> ELSE <<>>
  [] ev.act = "NativeDelivered" -> <<>>
  [] ev.act = "NativeRecover" -> LET n == s.natives[<<a.parent,a.key>>] IN
      IF n.delivery = "Claimed" THEN <<NQDeliverNative(a,NQInc(n.generation))>> ELSE <<>>
  [] ev.act = "RaiseQuestion" -> IF <<a.child,a.qid>> \in DOMAIN s.questions THEN <<>>
      ELSE <<NQFxSend(NQUp(a.child,a.qid,1,s.threads[a.child].parent))>> \o QuestionRerouteFx(s,a.child,TRUE)
  [] ev.act = "ImportQuestion" -> LET k == <<a.child,a.qid>> IN
      IF k \in DOMAIN s.questions /\ (~s.questions[k].pending \/ a.epoch < s.questions[k].epoch) THEN <<>>
      ELSE LET q == IF k \in DOMAIN s.questions THEN [s.questions[k] EXCEPT !.epoch=a.epoch]
                    ELSE NQEmpty(a.child,a.qid,a.epoch)
               m == NQUp(a.child,a.qid,a.epoch,a.to)
           IN NQDeliverFx(NQSend(NQPut(s,k,q),m),m,"Ack")
  [] ev.act = "ImportAnswer" -> LET m == NQAnswer(a.child,a.qid,a.epoch,a.via,a.answer) IN NQDeliverFx(NQSend(s,m),m,"Ack")
  [] ev.act = "ImportRelease" -> LET m == NQRelease(a.child,a.qid,a.epoch,a.via) IN NQDeliverFx(NQSend(s,m),m,"Ack")
  [] ev.act = "ImportClosed" -> LET k == <<a.child,a.qid>> exists == k \in DOMAIN s.questions
       e == IF exists THEN IF s.questions[k].epoch > a.epoch THEN s.questions[k].epoch ELSE a.epoch ELSE a.epoch
       staged == IF exists THEN NQPut(s,k,[s.questions[k] EXCEPT !.pending=FALSE,!.holder="none",!.at_person=FALSE,!.epoch=e]) ELSE s
       m == NQClosed(a.child,a.qid,e,a.to)
     IN NQDeliverFx(NQSend(staged,m),m,"Ack") \o
        IF s.threads[a.to].parent="none" THEN <<>> ELSE <<NQFxSend(NQClosed(a.child,a.qid,e,s.threads[a.to].parent))>>
  [] ev.act = "AgentAnswer" -> LET k == <<a.child,a.qid>> IN
       IF k \notin DOMAIN s.questions THEN <<>>
       ELSE IF ~s.questions[k].pending \/ s.questions[k].epoch # a.epoch THEN <<>>
       ELSE <<NQFxSend(NQAnswer(a.child,a.qid,a.epoch,a.agent,a.answer))>>
  [] ev.act = "PersonAnswer" -> NQCloseFx(s,a.child,a.qid,a.epoch,a.answer)
  [] ev.act = "StartQuestion" ->
     NQConcat([i \in 1..Len(s.question_order) |-> LET q == s.questions[s.question_order[i]] IN
       IF q.pending /\ q.holder=a.t /\ a.t \in q.views THEN <<NQFxView(q,a.t,FALSE)>> ELSE <<>>]) \o
     <<[kind |-> "StartRun", run |-> [thread |-> a.t, ord |-> NQOrd(s.threads[a.t])+1],
        start |-> [kind |-> "Questions"], access |-> s.threads[a.t].access, delivered |-> <<>>]>>
  [] ev.act = "DeliverQuestion" -> NQDeliverFx(s,a.message,a.how)
  [] OTHER -> <<>>

(* Derived key orders use only prior raw keys and command inputs. The adapter
   validates numeric source order before it renames IDs. Raw state relation
   preserves all other fields. Finalize/ReconcileEffects run in the caller. *)
NQResult(s,ev) ==
  LET a == ev.args IN
  CASE ev.act = "NativeStart" -> LET k == <<a.parent,a.key>> IN
    IF k \in DOMAIN s.natives THEN s ELSE [s EXCEPT !.natives=@ @@ (k :>
      [attempt |-> NQAttempt(s.threads[a.parent]), background |-> a.background,
       ancestor |-> "none", status |-> "Running", delivery |-> "Pending", generation |-> 0])]
  [] ev.act = "NativeStartNested" -> LET k == <<a.parent,a.key>> owner == s.natives[<<a.parent,a.ancestor>>] IN
     IF k \in DOMAIN s.natives THEN s ELSE [s EXCEPT !.natives=@ @@ (k :>
       [attempt |-> owner.attempt,background |-> a.background \/ owner.background,ancestor |-> a.ancestor,
        status |-> "Running",delivery |-> "Pending",generation |-> 0])]
  [] ev.act = "NativeEnd" -> LET k == <<a.parent,a.key>> n == s.natives[k]
       inline == IF a.parent \notin DOMAIN s.threads THEN FALSE ELSE NQLive(s.threads[a.parent]) /\ NQAttempt(s.threads[a.parent])=n.attempt
     IN IF n.status # "Running" THEN s ELSE [s EXCEPT !.natives[k].status=IF a.failed THEN "Failed" ELSE "Completed",
        !.natives[k].delivery=IF inline THEN "Delivered" ELSE "Claimed"]
  [] ev.act = "NativeClaim" -> LET k == <<a.parent,a.key>> IN
     IF s.natives[k].delivery="Pending" THEN [s EXCEPT !.natives[k].delivery="Claimed"] ELSE s
  [] ev.act = "NativeDelivered" -> LET k == <<a.parent,a.key>> n == s.natives[k] IN
     IF n.delivery="Claimed" /\ n.generation=a.generation THEN [s EXCEPT !.natives[k].delivery="Delivered"] ELSE s
  [] ev.act = "NativeRecover" -> LET k == <<a.parent,a.key>> IN
     IF s.natives[k].delivery="Claimed" THEN [s EXCEPT !.natives[k].generation=@+1] ELSE s
  [] ev.act = "RaiseQuestion" -> LET k == <<a.child,a.qid>> IN
     IF k \in DOMAIN s.questions THEN s ELSE QuestionReroute(NQSend(NQPut(s,k,NQEmpty(a.child,a.qid,1)),
          NQUp(a.child,a.qid,1,s.threads[a.child].parent)),a.child,TRUE)
  [] ev.act = "ImportQuestion" -> LET k == <<a.child,a.qid>> IN
     IF k \in DOMAIN s.questions /\ (~s.questions[k].pending \/ a.epoch < s.questions[k].epoch) THEN s
     ELSE LET q == IF k \in DOMAIN s.questions THEN [s.questions[k] EXCEPT !.epoch=a.epoch] ELSE NQEmpty(a.child,a.qid,a.epoch)
              m == NQUp(a.child,a.qid,a.epoch,a.to) IN NQDeliver(NQSend(NQPut(s,k,q),m),m,"Ack")
  [] ev.act = "ImportAnswer" -> LET m == NQAnswer(a.child,a.qid,a.epoch,a.via,a.answer) IN NQDeliver(NQSend(s,m),m,"Ack")
  [] ev.act = "ImportRelease" -> LET m == NQRelease(a.child,a.qid,a.epoch,a.via) IN NQDeliver(NQSend(s,m),m,"Ack")
  [] ev.act = "ImportClosed" -> LET k == <<a.child,a.qid>> exists == k \in DOMAIN s.questions
       e == IF exists THEN IF s.questions[k].epoch > a.epoch THEN s.questions[k].epoch ELSE a.epoch ELSE a.epoch
       staged == IF exists THEN NQPut(s,k,[s.questions[k] EXCEPT !.pending=FALSE,!.holder="none",!.at_person=FALSE,!.epoch=e]) ELSE s
       m == NQClosed(a.child,a.qid,e,a.to)
       delivered == NQDeliver(NQSend(staged,m),m,"Ack")
     IN IF s.threads[a.to].parent="none" THEN delivered ELSE NQSend(delivered,NQClosed(a.child,a.qid,e,s.threads[a.to].parent))
  [] ev.act = "AgentAnswer" -> LET k == <<a.child,a.qid>> IN
     IF k \notin DOMAIN s.questions THEN s ELSE IF ~s.questions[k].pending \/ s.questions[k].epoch # a.epoch THEN s
     ELSE NQSend(s,NQAnswer(a.child,a.qid,a.epoch,a.agent,a.answer))
  [] ev.act = "PersonAnswer" -> NQClose(s,a.child,a.qid,a.epoch,a.answer)
  [] ev.act = "StartQuestion" -> [s EXCEPT !.threads[a.t]=NQBeginQuestions(s.threads[a.t]),
       !.questions=[k \in DOMAIN @ |-> LET q == s.questions[k] IN
          IF q.pending /\ q.holder=a.t /\ a.t \in q.views THEN [q EXCEPT !.seen=@ \cup {a.t}] ELSE q]]
  [] ev.act = "DeliverQuestion" -> NQDeliver(s,a.message,a.how)
  [] OTHER -> s
NQOrderedResult(s,ev) == [NQResult(s,ev) EXCEPT !.question_order=ev.question_order,!.native_order=ev.native_order]
(* First-error checks follow the reducer's source order. A rejected step
   never runs reconciliation or retirement. Unknown actions are unsupported. *)
NQActions == {"NativeStart","NativeStartNested","NativeEnd","NativeClaim","NativeDelivered","NativeRecover",
 "RaiseQuestion","ImportQuestion","ImportClosed","ImportAnswer","ImportRelease",
 "AgentAnswer","PersonAnswer","StartQuestion","DeliverQuestion"}
RECURSIVE NQAncestorReject(_, _), NQHopReject(_, _, _)
NQAncestorReject(s,t) == IF t \notin DOMAIN s.threads THEN "UnknownThread"
 ELSE IF s.threads[t].parent="none" THEN "accepted" ELSE NQAncestorReject(s,s.threads[t].parent)
NQHopReject(s,t,via) == IF t \notin DOMAIN s.threads THEN "UnknownThread"
 ELSE IF s.threads[t].parent="none" \/ s.threads[t].parent=via THEN "accepted"
 ELSE NQHopReject(s,s.threads[t].parent,via)
NQCloseReject(s,c,qid,e) == LET k == <<c,qid>> IN
 IF k \notin DOMAIN s.questions THEN "accepted"
 ELSE IF ~s.questions[k].pending \/ s.questions[k].epoch # e THEN "accepted"
 ELSE NQAncestorReject(s,c)
NQBodyReject(s,m) ==
 IF m.kind \notin {"QuestionUp","AnswerDown","QuestionRelease","QuestionClosed"} THEN "Corrupt"
 ELSE LET k == <<m.child,m.qid>> IN
 IF k \notin DOMAIN s.questions THEN "accepted" ELSE LET q == s.questions[k] IN
 CASE m.kind="QuestionUp" ->
   IF ~q.pending \/ q.epoch # m.epoch \/ m.to="none" THEN "accepted"
   ELSE IF m.to \notin DOMAIN s.threads THEN "UnknownThread" ELSE "accepted"
 [] m.kind="AnswerDown" ->
   IF ~q.pending \/ q.epoch # m.epoch THEN "accepted"
   ELSE IF NQHopReject(s,m.child,m.via)#"accepted" THEN NQHopReject(s,m.child,m.via)
   ELSE IF NQHop(s,m.child,m.via)=m.child THEN NQCloseReject(s,m.child,m.qid,m.epoch) ELSE "accepted"
 [] m.kind="QuestionRelease" ->
   IF ~q.pending \/ m.epoch < q.epoch THEN "accepted"
   ELSE IF NQHopReject(s,m.child,m.via)#"accepted" THEN NQHopReject(s,m.child,m.via)
   ELSE IF NQHop(s,m.child,m.via)=m.child /\ m.epoch \in DOMAIN q.future_answers
        THEN NQAncestorReject(s,m.child) ELSE "accepted"
 [] m.kind="QuestionClosed" -> "accepted"
 [] OTHER -> "Corrupt"
NQDeliverReject(s,m) ==
 IF m \notin s.outbox THEN "NotInOutbox"
 ELSE IF m \in s.question_receipts THEN "accepted" ELSE NQBodyReject(s,m)
NQRejectName(s,ev) == LET a == ev.args IN
 CASE ev.act="NativeStart" ->
   IF <<a.parent,a.key>> \in DOMAIN s.natives THEN "accepted"
   ELSE IF a.parent \notin DOMAIN s.threads THEN "UnknownThread"
   ELSE IF ~NQLive(s.threads[a.parent]) THEN "NotLive" ELSE "accepted"
 [] ev.act="NativeStartNested" ->
   IF <<a.parent,a.key>> \in DOMAIN s.natives THEN "accepted"
   ELSE IF a.key=a.ancestor THEN "NotKid"
   ELSE IF <<a.parent,a.ancestor>> \notin DOMAIN s.natives THEN "NotKid"
   ELSE LET owner == s.natives[<<a.parent,a.ancestor>>] IN
     IF owner.status # "Running" THEN "NotLive"
     ELSE IF a.parent \notin DOMAIN s.threads THEN "UnknownThread"
     ELSE IF owner.background \/ (NQLive(s.threads[a.parent]) /\ ~s.threads[a.parent].attempt.emitted /\ NQAttempt(s.threads[a.parent])=owner.attempt)
          THEN "accepted" ELSE "NotLive"
 [] ev.act \in {"NativeEnd","NativeClaim","NativeDelivered","NativeRecover"} ->
   IF <<a.parent,a.key>> \notin DOMAIN s.natives THEN "NotKid"
   ELSE IF ev.act="NativeClaim" /\ s.natives[<<a.parent,a.key>>].status="Running" THEN "NotSettled" ELSE "accepted"
 [] ev.act="RaiseQuestion" ->
   IF a.child \notin DOMAIN s.threads THEN "UnknownThread" ELSE LET th == s.threads[a.child] IN
   CASE ~NQLive(th) -> "NotLive"
    [] th.wait_on # "none" -> "Waiting"
    [] th.held="Stop" \/ th.int_req -> "InterruptRequested"
    [] <<a.child,a.qid>> \in DOMAIN s.questions -> "accepted"
    [] NQBlocked(s,a.child) -> "Waiting"
    [] OTHER -> "accepted"
 [] ev.act="ImportQuestion" ->
   IF a.child \notin DOMAIN s.threads THEN "UnknownThread"
   ELSE IF a.to \notin DOMAIN s.threads THEN "UnknownThread" ELSE "accepted"
 [] ev.act="ImportAnswer" -> LET m == NQAnswer(a.child,a.qid,a.epoch,a.via,a.answer) IN NQDeliverReject(NQSend(s,m),m)
 [] ev.act="ImportRelease" -> LET m == NQRelease(a.child,a.qid,a.epoch,a.via) IN NQDeliverReject(NQSend(s,m),m)
 [] ev.act="ImportClosed" -> IF a.to \notin DOMAIN s.threads THEN "UnknownThread" ELSE "accepted"
 [] ev.act="DeliverQuestion" -> NQDeliverReject(s,a.message)
 [] ev.act="PersonAnswer" -> NQCloseReject(s,a.child,a.qid,a.epoch)
 [] ev.act="AgentAnswer" ->
   IF a.agent \notin DOMAIN s.threads THEN "UnknownThread" ELSE LET th == s.threads[a.agent] k == <<a.child,a.qid>> IN
   CASE ~NQLive(th) -> "NotLive"
    [] NQBlocked(s,a.agent) -> "Waiting"
    [] k \notin DOMAIN s.questions -> "accepted"
    [] ~s.questions[k].pending \/ s.questions[k].epoch # a.epoch -> "accepted"
    [] s.questions[k].holder # a.agent \/ a.agent \notin s.questions[k].seen -> "NotCurrent"
    [] OTHER -> "accepted"
 [] ev.act="StartQuestion" ->
   IF a.t \notin DOMAIN s.threads THEN "UnknownThread" ELSE LET th == s.threads[a.t] IN
   CASE NQLive(th) -> "RunLive"
    [] th.held # "None" -> "Held"
    [] NQBlocked(s,a.t) -> "Waiting"
    [] ~\E k \in DOMAIN s.questions: s.questions[k].pending /\ s.questions[k].holder=a.t /\ a.t \in s.questions[k].views -> "QueueEmpty"
    [] OTHER -> "accepted"
 [] OTHER -> "unsupported"
NativeQuestionReject(s,ev,reason) ==
 /\ ev.act \in NQActions /\ reason # "accepted" /\ reason # "unsupported"
 /\ NQRejectName(s,ev)=reason
NativeQuestionApply(s,next,ev,fx) ==
  LET raw == NQResult(s,ev) ordered == NQOrderedResult(s,ev) IN
  /\ NQRejectName(s,ev)="accepted"
  /\ NativeQuestionRawApply(s,raw,ev)
  /\ next = Finalize(ordered)
  /\ fx = NQExpectedFx(s,ev) \o ReconcileEffects(ordered)
=============================================================================
