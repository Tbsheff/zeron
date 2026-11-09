--------------------------- MODULE RuntimePlanJoint ---------------------------
(* Independent free-running bounded raw mechanism product. Atomic normalized
   core commits are an explicit abstraction boundary, not Rust equivalence.
   Provider steps are separate from logical start effects. CT provider packet
   acceptance is separate from durable delivery receipts. Fixed registered
   root/tree and hosts only. No target Init/Next participates in successors. *)
EXTENDS RuntimePlanNativeKernel, RuntimePlanImage
CONSTANTS Subs, Providers, NoProv, PlanMaxMsg, PlanMaxX,
 PlanMaxAttempt, PlanMaxPerson, PlanMaxServer, PlanMaxStop, PlanMaxNarrow,
 PlanLimits, PlanMutant
VARIABLES core, physical, ct, effects, personLeft, serverLeft, stopLeft, narrowLeft, ctTracked, ghost
jointVars == <<core,physical,ct,effects,personLeft,serverLeft,stopLeft,narrowLeft,ctTracked,ghost>>
CT == INSTANCE RuntimePlanTransferKernel WITH
 Threads<-Threads,Main<-Root,NoThread<-"none",Providers<-Providers,NoProv<-NoProv,
 MaxMsg<-PlanMaxMsg,MaxX<-PlanMaxX,MaxCrash<-PlanMaxCrash,Limits<-PlanLimits,
 Mutant<-PlanMutant,ct<-ct

EmptyFn == [x \in {} |-> x]
JointCoreInit ==
 [config |-> [max_depth |-> Cardinality(Threads)-1,max_fanout |-> Cardinality(Threads)-1,
              max_queue |-> 3,max_retry |-> 1,run_window |-> PlanMaxRuns+1],
  threads |-> [t \in {Root} |-> NewThread(t,"none",0,2)],thread_order |-> <<Root>>,
  outbox |-> {},stop_receipts |-> {},stops |-> 0,
  questions |-> EmptyFn,question_receipts |-> {},natives |-> EmptyFn,
  native_order |-> <<>>,question_order |-> <<>>]
JointInit ==
 /\ core=JointCoreInit /\ physical=PlanNativeInit(Threads,Subs) /\ CT!Init
 /\ ctTracked=[t \in Threads |-> FALSE] /\ ghost=PlanGhostInit
 /\ effects= <<>> /\ personLeft=PlanMaxPerson /\ serverLeft=PlanMaxServer
 /\ stopLeft=PlanMaxStop /\ narrowLeft=PlanMaxNarrow
Ev(act,args) == [act |-> act,args |-> args]
CoreRoom(t) == RunOrd(core,t)<PlanMaxRuns
AttemptRoom(t) == core.threads[t].attempt.n<PlanMaxAttempt
(* prepare_run must finish before the actual harness request is issued.
   This is packet preparation, not provider acceptance or session life. *)
JointCTReadyRequest(t) == ~ctTracked[t] \/
 /\ ct.runs[t].st="starting" /\ CT!Pending(t)={}
 /\ (CT!Resolved(t)={} \/ ct.packets[t].present)
 /\ ~\E x \in 1..ct.next:ct.ledger[x].tgt=t /\ ct.ledger[x].st="failed" /\ ct.transferInput[x]=ct.runs[t].u
JointRequestIssued(t) == physical.issued_handle[t].present /\
 physical.callable_serial[t]=physical.issued_handle[t].serial /\
 physical.issued_handle[t].run=ct.bindings[t].run /\ physical.issued_handle[t].attempt=ct.bindings[t].attempt
(* Effects are committed with the core, then dispatched by physical steps.
   CT starts track the same core run/attempt only when its own operational
   start can proceed. A full map must record whether that turn is tracked. *)
JointRD(ev) ==
 /\ RDGuard(core,ev)
 /\ (ev.act="Delegate" => PlanMutant="call_before_issue" \/ PlanNativeCallerCurrent(core,physical,ev.args.t))
 /\ LET r == RDRawResult(core,ev) next == Finalize(r.state)
        fx == r.fx \o ReconcileEffects(r.state)
    IN /\ core'=next /\ effects'=fx
       /\ physical'=PlanNativeCoreBoundary(core,next,physical,fx)
 /\ personLeft'=IF ev.act="PersonSend" THEN personLeft-1 ELSE personLeft
 /\ serverLeft'=IF ev.act="ServerEnqueue" THEN serverLeft-1 ELSE serverLeft
 /\ stopLeft'=IF ev.act="Stop" THEN stopLeft-1 ELSE stopLeft
 /\ narrowLeft'=IF ev.act="NarrowAccess" THEN narrowLeft-1 ELSE narrowLeft
 /\ IF ev.act \in {"StartFromQueue","StartFromQueueWake","UsageLimitRecovery"}
    THEN LET t == ev.args.t nextCore == Finalize(RDRawResult(core,ev).state)
             ord == RunOrd(nextCore,t) attempt == nextCore.threads[t].attempt.n
         IN IF ENABLED (\E p \in Providers : CT!PlanStart(t,p,Len(ct.journal)+1,ord,attempt))
            THEN /\ \E p \in Providers : CT!PlanStart(t,p,Len(ct.journal)+1,ord,attempt)
                 /\ ctTracked'=[ctTracked EXCEPT ![t]=TRUE]
            ELSE /\ UNCHANGED ct /\ ctTracked'=[ctTracked EXCEPT ![t]=FALSE]
    ELSE IF ev.act="Delegate"
    THEN /\ IF ENABLED CT!CreateLineage(ev.args.t,ev.args.child,"subagent_spawn")
            THEN CT!CreateLineage(ev.args.t,ev.args.child,"subagent_spawn") ELSE UNCHANGED ct
         /\ UNCHANGED ctTracked
    ELSE IF ev.act="IngestEnd" /\ ctTracked[ev.args.t]
    THEN /\ ct'=[ct EXCEPT !.runs[ev.args.t]=CT!Idle]
         /\ ctTracked'=[ctTracked EXCEPT ![ev.args.t]=FALSE]
    ELSE UNCHANGED <<ct,ctTracked>>
 /\ ghost'=[PlanGhostCT(PlanGhostNext(core,core',ev,effects',ghost),ct,ct',core') EXCEPT
      !.ct_tracked={t \in Threads:ctTracked'[t]}]
(* wire.rs auto_step first abandons an open wait when a current Completed
   terminal is buffered. It does not require ~attempt.emitted. *)
JointAutoAbandon(t) ==
 /\ core.threads[t].wait_on#"none"
 /\ \E e \in core.threads[t].inbox:
      e.node="Root" /\ RDCurrent(core,t,e) /\ e.outcome="Completed"
 /\ JointRD(Ev("WaitAbandon",[t |-> t]))
JointNative(ev) ==
 /\ PlanNativeSteps(core,physical,core',physical',ev,effects')
 /\ IF ev.act="EngineRootObserve" THEN
      IF physical.inbox#<<>> /\ Head(physical.inbox).kind="rootEnd" THEN
        IF ctTracked[Head(physical.inbox).parent] /\ Head(physical.inbox).status="completed"
        THEN CT!Complete(Head(physical.inbox).parent) ELSE UNCHANGED ct
      ELSE UNCHANGED ct
    ELSE IF ev.act="QuestionApply" THEN
      IF ev.args.event.act="StartQuestion" THEN
        LET t == ev.args.event.args.t
            nextCore == Finalize(NQOrderedResult(core,ev.args.event))
            ord == RunOrd(nextCore,t) attempt == nextCore.threads[t].attempt.n
        IN IF ENABLED (\E p \in Providers: CT!PlanStart(t,p,Len(ct.journal)+1,ord,attempt))
           THEN \E p \in Providers: CT!PlanStart(t,p,Len(ct.journal)+1,ord,attempt)
           ELSE UNCHANGED ct
      ELSE UNCHANGED ct
    ELSE UNCHANGED ct
 /\ ctTracked'=IF ev.act="QuestionApply" THEN
      IF ev.args.event.act="StartQuestion" THEN
        [ctTracked EXCEPT ![ev.args.event.args.t]=ct.runs[ev.args.event.args.t]#ct'.runs[ev.args.event.args.t]]
      ELSE ctTracked ELSE ctTracked
 /\ UNCHANGED <<personLeft,serverLeft,stopLeft,narrowLeft>>
 /\ LET g == PlanGhostCT(PlanGhostNext(core,core',ev,effects',ghost),ct,ct',core')
        ga == IF ev.act \in {"ProviderHandleInstall","ProviderSteerAccepted","ProviderSteered"} THEN
          LET h == physical'.issued_handle[ev.args.parent] IN
          PlanGhostProviderAccess(g,ev.args.parent,h.run,h.attempt,h.access)
          ELSE g
    IN ghost'=[ga EXCEPT !.ct_tracked={t \in Threads:ctTracked'[t]}]

JointCoreNext ==
 \/ \E t \in DOMAIN core.threads :
    \/ personLeft>0 /\ JointRD(Ev("PersonSend",[t |-> t,msg |-> "1",mode |-> "Queue"]))
    \/ serverLeft>0 /\ JointRD(Ev("ServerEnqueue",[t |-> t,msg |-> "2"]))
    \/ CoreRoom(t) /\ JointRD(Ev("StartFromQueue",[t |-> t]))
    \/ CoreRoom(t) /\ JointRD(Ev("StartFromQueueWake",[t |-> t]))
    \/ JointRD(Ev("WaitTimeout",[t |-> t]))
    \/ JointRD(Ev("WaitReturn",[t |-> t]))
    \/ JointAutoAbandon(t)
    \/ stopLeft>0 /\ JointRD(Ev("Stop",[t |-> t]))
    \/ narrowLeft>0 /\ AttemptRoom(t) /\ \E a \in 0..1:
          JointRD(Ev("NarrowAccess",[t |-> t,access |-> a]))
    \/ \E e \in core.threads[t].inbox :
       \/ JointRD(Ev("IngestEnd",[t |-> t,ev |-> e]))
       \/ AttemptRoom(t) /\ JointRD(Ev("RetryAttempt",[t |-> t,ev |-> e]))
       \/ JointRD(Ev("IngestChild",[t |-> t,ev |-> e]))
       \/ JointRD(Ev("IngestStale",[t |-> t,ev |-> e]))
    \/ \E c \in Threads \ DOMAIN core.threads,m \in {"Wait","Async"},a \in 0..2:
       JointRD(Ev("Delegate",[t |-> t,child |-> c,mode |-> m,access |-> a,reconfigure |-> FALSE]))
 \/ \E m \in core.outbox :
    CASE m.kind="Notice" -> JointRD(Ev("DoNotice",[child |-> m.child,run |-> m.run,how |-> "Ack"]))
    [] m.kind="Offer" -> JointRD(Ev("DoOffer",[parent |-> m.parent,run |-> m.run,generation |-> m.generation,how |-> "Ack"]))
    [] m.kind="Stop" -> JointRD(Ev("DoStop",[child |-> m.child,stop |-> m.stop,how |-> "Ack"]))
    [] OTHER -> FALSE

ProviderArgs(t,k,bg,out) ==
 [parent |-> t,key |-> k,run |-> IF t \in DOMAIN core.threads THEN RunOrd(core,t) ELSE 0,
  attempt |-> IF t \in DOMAIN core.threads THEN core.threads[t].attempt.n ELSE 0,
  epoch |-> physical.natives[<<t,k>>].epoch+1,background |-> bg,status |-> out,
  access |-> IF t \in DOMAIN core.threads THEN core.threads[t].access ELSE 0,reconfigure |-> TRUE,
  native_order |-> IF <<t,k>> \in {core.native_order[i]:i \in 1..Len(core.native_order)}
                  THEN core.native_order ELSE Append(core.native_order,<<t,k>>),
  generation |-> IF <<t,k>> \in DOMAIN core.natives THEN core.natives[<<t,k>>].generation ELSE 0,
  msg |-> "1"]
JointProviderNext ==
 (* Only Spawn reads background, and only End reads status. Canonicalize
    unused arguments to avoid generating each identical successor four
    times. The event consumers read their actual inbox Head, not args. *)
 \/ \E t \in Threads,k \in Subs:
    LET a == ProviderArgs(t,k,FALSE,"completed") IN
      \/ JointNative(Ev("ProviderHandleInstall",a))
      \/ JointCTReadyRequest(t) /\ JointNative(Ev("ProviderHarnessEnter",a))
      \/ JointNative(Ev("ProviderSteerAccepted",a))
      \/ JointNative(Ev("ProviderSteered",a))
      \/ JointNative(Ev("ProviderStart",a))
      \/ JointNative(Ev("ProviderProgress",a))
      \/ JointNative(Ev("ProviderSessionEnd",a))
      \/ JointNative(Ev("ProviderCrash",a))
      \/ JointNative(Ev("ProviderInterruptedTerminal",a))
      \/ JointNative(Ev("ProviderStopConfirmed",a))
      \/ JointNative(Ev("EngineRecover",a))
      \/ JointNative(Ev("NativeEnqueue",a))
 \/ \E t \in Threads,k \in Subs,bg \in BOOLEAN:
      JointNative(Ev("ProviderSpawn",ProviderArgs(t,k,bg,"completed")))
 \/ \E t \in Threads,k \in Subs,out \in {"completed","failed"}:
    LET a == ProviderArgs(t,k,FALSE,out) IN
      JointNative(Ev("ProviderNativeEnd",a)) \/ JointNative(Ev("ProviderRootEnd",a))
 \/ JointNative(Ev("EngineNativeObserve",EmptyFn))
 \/ JointNative(Ev("EngineRootObserve",EmptyFn))
 \/ JointNative(Ev("EngineSessionObserve",EmptyFn))

(* CT work can stutter the core; a tracked start hook is supplied by the
   join that maps each StartRun effect to CT PlanStart. This stage exposes
   full packet/receipt interleavings but does not claim their target map. *)
JointCTNext ==
 (/\ (\/ \E t \in Threads,x \in 1..PlanMaxX,l \in PlanLimits:
          CT!ResolveSwitch(t,x,l) \/ CT!ResolveFork(t,x,l) \/ CT!ResolveSpawn(t,x,l) \/
          CT!ResolveMerge(t,x,l) \/ CT!ResolveResult(t,x,l) \/ CT!NativeFork(t,x) \/ CT!NativeResume(t,x)
      \/ \E t \in Threads: CT!SavePacket(t) \/ (JointRequestIssued(t) /\ CT!ProviderAccept(t)) \/ CT!SaveReceipt(t) \/
          CT!Inject(t) \/ CT!PersistInjected(t) \/ (JointRequestIssued(t) /\ CT!StartAfterInjection(t)) \/
          (JointRequestIssued(t) /\ CT!ProviderBare(t)) \/
          CT!Recover(t) \/ CT!Retry(t)
      \/ CT!Crash)
  /\ UNCHANGED <<core,physical,effects,personLeft,serverLeft,stopLeft,narrowLeft,ctTracked>>
  /\ ghost'=[PlanGhostCT(ghost,ct,ct',core) EXCEPT !.ct_tracked={t \in Threads:ctTracked[t]}])
JointQuestionNext == \E ev \in PlanQuestionEvents(core,physical,Threads\{Root},{1}) \cup PlanNativeDeliveryEvents(core,physical):
 /\ (IF ev.act="QuestionApply" THEN ev.args.event.act="StartQuestion" => CoreRoom(ev.args.event.args.t) ELSE TRUE)
 /\ JointNative(ev)
JointNext == JointCoreNext \/ JointProviderNext \/ JointQuestionNext \/ JointCTNext
JointSpec == JointInit /\ [][JointNext]_jointVars
JointNativeImage(focus) ==
 LET ni == PlanNativeImage(core,physical,focus)
     pend == IF focus \in DOMAIN core.threads THEN
       {e \in core.threads[focus].inbox:e.node="Root"} ELSE {}
     prefix == IF pend={} THEN <<>> ELSE
       LET e == CHOOSE v \in pend:TRUE IN
       <<[k |-> "rootEnd",s |-> "none",ep |-> 0,r |-> e.run,bg |-> FALSE,out |-> PILower(e.outcome)]>>
 IN [ni EXCEPT !.nInbox=prefix \o @,!.nReplay={}]
JointImage(focus) == PlanImage(core,JointNativeImage(focus),CT!CTImage,ghost,focus)
JointMapDomain == PIQuestionSingle(core) /\ PIHistoryComplete(core) /\ \A k \in DOMAIN ghost.budgets:ghost.budgets[k]>=0

JointRunRoom == \A t \in DOMAIN core.threads : RunOrd(core,t)<PlanMaxRuns
JointAttemptRoom == \A t \in DOMAIN core.threads : core.threads[t].attempt.n<PlanMaxAttempt
JointJournalRoom == Len(ct.journal)<PlanMaxMsg
JointTransferRoom == ct.next<PlanMaxX
JointForegroundCore == \A k \in DOMAIN core.natives :
 StaleNative(core,k) => core.natives[k].status="Orphaned"
JointPacketBound == CT!PacketBound
JointReceiptSound == CT!ReceiptSound
JointNoDuplicate == CT!NoDuplicate
JointNativeObserved == ~\E k \in DOMAIN core.natives : core.natives[k].status="Running"
JointBackgroundSurvives == ~\E k \in DOMAIN core.natives :
 core.natives[k].status="Running" /\ core.natives[k].background /\ ~IsLive(core,k[1])
JointBufferedEndWait == ~\E t \in DOMAIN core.threads:
 core.threads[t].wait_on#"none" /\
 (\E e \in core.threads[t].inbox:e.node="Root" /\ RDCurrent(core,t,e) /\ e.outcome="Completed")
JointEarlyIssuedCall == ~\E c \in DOMAIN core.threads\{Root}:
 LET p == core.threads[c].parent IN
 PlanNativeCallerCurrent(core,physical,p) /\ ~physical.parents[p].session
JointIssuedBeforeDelegate ==
 [][\A c \in DOMAIN core'.threads\DOMAIN core.threads:
     PlanNativeCallerCurrent(core,physical,core'.threads[c].parent)]_jointVars
=============================================================================
