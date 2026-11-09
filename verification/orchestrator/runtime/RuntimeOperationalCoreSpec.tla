---------------- MODULE RuntimeOperationalCoreSpec ----------------
(* This is a target contract. The implementation enumerates its own steps.
   The exact source Step is retained, with admitted caller guards on agent
   actions and issued-source guards on provider input. R44/R48 use independent
   reporting and retained-request products; old native and CT slots stay Init. *)
EXTENDS RuntimeCompositionSpec
VARIABLES currentAdmission, currentIssued
GuardedStep ==

  \/ \E t \in Threads :
       \/ \E m \in RL!Modes : C_PersonSend(t, m) \/ C_Restart(t, m)
       \/ C_ObserveProviderStart(t)
       \/ (\E a \in D!Levels : C_ObserveAccess(t,a))
       \/ (\E o \in RL!Outcomes : C_ObserveProviderTerminal(t,o))
       \/ C_Steer(t) \/ C_Stop(t) \/ C_InterruptChat(t) \/ C_ServerEnqueue(t) \/ C_Narrow(t) \/ C_ClampRestart(t)
       \/ \E o \in RL!Outcomes : C_ProviderTerminal(t, o)
       \/ C_ChildStart(t) \/ C_ChildEnd(t) \/ C_ResetPasses(t)
       \/ C_StartQueue(t) \/ C_StartWake(t) \/ C_ULRecovery(t)
       \/ \E q \in Children : C_StartQuestion(t, q)
       \/ \E e \in pev[t] : C_IngestEnd(t, e) \/ C_RetryAttempt(t, e) \/ C_IngestChild(t, e) \/ C_IngestStale(t, e)
       \/ C_WaitReturn(t) \/ C_WaitReturnQuestion(t) \/ C_WaitTimeout(t) \/ C_WaitAbandon(t) \/ C_LateWaitAck(t)
  \/ \E t \in Threads, c \in Children, m \in {"wait", "async"}, a \in D!Levels : (currentAdmission[t] /\ C_Delegate(t, c, m, a))
  \/ \E p \in Threads, c \in Children : (currentAdmission[p] /\ C_ReadResult(p, c))
  \/ \E t \in Threads, c \in Children, m \in {"wait", "async"} : (currentAdmission[t] /\ C_FollowUp(t, c, m))
  \/ \E q \in Children : (currentIssued[q] /\ C_RaiseQuestion(q)) \/ C_PersonAnswer(q) \/ \E x \in Threads : (currentAdmission[x] /\ C_AgentAnswer(q, x))
  \/ \E q \in Children : (currentIssued[q] /\ C_RaiseQuestionS(q)) \/ C_PersonAnswerS(q) \/ \E x \in Threads : (currentAdmission[x] /\ C_AgentAnswerS(q, x))
  \/ \E e \in outbox : C_DoQHop(e)
  \/ \E e \in outbox, how \in D!Hows : C_DoNotice(e, how) \/ C_DoOffer(e, how) \/ C_DoStop(e, how)
  \/ C_CrashEngine
  \/ \E d \in Devices : C_GoOffline(d) \/ C_GoOnline(d)
  \/ \E s \in Subs : \/ \E bg \in BOOLEAN : C_NsSpawn(s, bg)
                     \/ C_NsResume(s) \/ C_NsProgress(s)
                     \/ \E out \in NS!Outs : C_NsSubEnd(s, out)
  \/ C_NsSessionEnd \/ C_NsHandle \/ C_NsCrash \/ C_NsRecover
  \/ \E e \in nReplay : C_NsReplay(e)
  \/ \E s, f \in Threads : C_CtFork(s, f)
  \/ \E f \in Threads : C_CtMergeBack(f) \/ C_CtSubagentResult(f)
  \/ \E t \in Threads : \/ \E x \in CT!XIds : C_CtResolve(t, x)
                        \/ C_CtDeliver(t)
                        \/ \E p \in Providers : C_CtRecoverDelivery(t, p) \/ C_CtFreeSend(t, p)
  \/ C_CtCrash
GuardedNext == GuardedStep \/ C_Done
====
