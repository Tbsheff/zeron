----------------------- MODULE RuntimeComposition -----------------------
(* Independent finite-path safety abstraction. It has no raw reducer state,
   receipt transaction state, run history, local integer IDs, or source action
   implementation. A separate raw mechanism replay must ALSO hold.
   Multiple roots, native keys on every parent, and access 0..2 are allowed.
   This is a safety product, not the old Composition specification or a
   proof of its liveness/CT anchor-selection obligations. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC

Access == 0..2
Handled == {"Delivered", "Acknowledged", "Disposed"}
SameExcept(a,b,keys) ==
  /\ DOMAIN a \ keys = DOMAIN b \ keys
  /\ \A k \in DOMAIN a \ keys : a[k] = b[k]
Keep(a,b,names) == \A k \in names : a[k] = b[k]
Current(a,t,r,n) == t \in DOMAIN a.threads /\ a.threads[t].live /\
  a.threads[t].run=r /\ a.threads[t].attempt=n
NativeAncestor(n) == IF "ancestor" \in DOMAIN n THEN n.ancestor ELSE "none"

WellFormed(a) ==
 /\ \A t \in DOMAIN a.threads : LET th == a.threads[t] IN
      /\ th.access \in Access /\ th.run \in Nat /\ th.attempt \in Nat
      /\ th.depth<=th.depth_limit
      /\ Cardinality({k \in DOMAIN a.tasks : a.tasks[k].parent=t /\ a.tasks[k].status#"Settled"})<=th.fanout_limit
      /\ IF th.parent="none" THEN th.depth=0
         ELSE th.parent \in DOMAIN a.threads /\ th.depth=a.threads[th.parent].depth+1
 /\ \A k \in DOMAIN a.tasks :
      a.tasks[k].pushes \in 0..1
 /\ \A k \in DOMAIN a.natives : LET n == a.natives[k] IN
      /\ n.status \in {"Running","Completed","Failed","Orphaned"}
      /\ n.delivery \in {"Pending","Claimed","Delivered","Disposed"}
      /\ (n.status="Running" /\ ~n.background => Current(a,k[1],n.run,n.attempt))
      /\ (NativeAncestor(n)#"none" => <<k[1],NativeAncestor(n)>> \in DOMAIN a.natives)
      /\ (n.status="Running" /\ ~n.background /\ NativeAncestor(n)#"none" =>
           a.natives[<<k[1],NativeAncestor(n)>>].status="Running")
 /\ \A t \in DOMAIN a.configured : a.configured[t] \in Access
 /\ \A t \in DOMAIN a.ceilings : a.ceilings[t] \in Access
 /\ \A k \in DOMAIN a.liveAccess : a.liveAccess[k] \in Access
 /\ a.replyObserved \subseteq a.replySent
 /\ \A k \in DOMAIN a.dispatches : a.dispatches[k] <= 1

EmptyTransferCatalog(ts) ==
 \A c \in DOMAIN ts : LET x == ts[c] IN
  /\ DOMAIN x.ledger={} /\ DOMAIN x.committed={}
  /\ DOMAIN x.receipts={} /\ DOMAIN x.runs={} /\ DOMAIN x.failures={}
  /\ x.rejected={} /\ DOMAIN x.providers={} /\ x.queue= <<>> /\ x.entries= <<>>

Initial(a) ==
 /\ WellFormed(a)
 /\ a.threads=[x \in {} |-> x] /\ a.tasks=[x \in {} |-> x]
 /\ a.natives=[x \in {} |-> x] /\ a.questions=[x \in {} |-> x]
 /\ EmptyTransferCatalog(a.transfers)
 /\ a.liveAccess=[x \in {} |-> x] /\ a.dispatches=[x \in {} |-> x]
 /\ a.replySent={} /\ a.replyObserved={}

(* Any foreground native tied to an old attempt must close. Background
   work has no root-attempt lifetime. A terminal native cannot reopen. *)
NativeAdvance(a,b,act,args) ==
 /\ DOMAIN a.natives \subseteq DOMAIN b.natives
 /\ \A k \in DOMAIN a.natives : LET n == a.natives[k] m == b.natives[k] IN
      /\ <<n.run,n.attempt,n.background,NativeAncestor(n)>> = <<m.run,m.attempt,m.background,NativeAncestor(m)>>
      /\ (n.status#"Running" => m.status=n.status)
      /\ (n.status="Running" /\ ~n.background /\ ~Current(b,k[1],n.run,n.attempt)
           => m.status="Orphaned" /\ m.delivery="Disposed")
      /\ (n.status="Running" /\ n.background /\ act#"NativeEnd" => m.status="Running")
      /\ m.generation >= n.generation
 /\ \A k \in DOMAIN b.natives \ DOMAIN a.natives :
      /\ act \in {"NativeStart","NativeStartNested"} /\ k= <<args.parent,args.key>>
      /\ b.natives[k].status="Running" /\ b.natives[k].delivery="Pending"
      /\ IF act="NativeStart" THEN
           /\ NativeAncestor(b.natives[k])="none"
           /\ Current(a,k[1],b.natives[k].run,b.natives[k].attempt)
         ELSE LET ownerKey == <<args.parent,args.ancestor>> IN
           /\ args.key#args.ancestor /\ ownerKey \in DOMAIN a.natives
           /\ LET owner == a.natives[ownerKey] child == b.natives[k] IN
                /\ owner.status="Running" /\ k[1] \in DOMAIN a.threads
                /\ (owner.background \/ (Current(a,k[1],owner.run,owner.attempt) /\ ~a.threads[k[1]].emitted))
                /\ <<child.run,child.attempt,NativeAncestor(child)>>= <<owner.run,owner.attempt,args.ancestor>>
                /\ child.background=(args.background \/ owner.background)

RunAdvance(a,b,act) ==
 /\ \A t \in DOMAIN b.threads \ DOMAIN a.threads :
      /\ act \in {"OpenRoot","Delegate","RemoteImport"}
      /\ (b.threads[t].parent="none" => act \in {"OpenRoot","RemoteImport"})
      /\ (act="Delegate" => b.threads[t].parent \in DOMAIN a.threads /\
            b.threads[t].access<=a.threads[b.threads[t].parent].access)
 /\ DOMAIN a.threads \subseteq DOMAIN b.threads
 /\ \A t \in DOMAIN a.threads : LET x == a.threads[t] y == b.threads[t] IN
      /\ <<x.parent,x.depth,x.depth_limit,x.fanout_limit>> = <<y.parent,y.depth,y.depth_limit,y.fanout_limit>>
      /\ y.run>=x.run /\ y.access<=x.access
      /\ (~x.live /\ y.live => act \in {"StartFromQueue","StartFromQueueWake","StartWake",
           "StartQuestion","UsageLimitRecovery","ResumeLimit"})
      /\ (x.live /\ ~y.live => act \in {"IngestEnd","IngestRoot","RemoteNotice"})
      /\ (y.run=x.run /\ y.attempt#x.attempt =>
          y.attempt=x.attempt+1 /\ act \in {"RetryAttempt","RestartActive","NarrowAccess","RemoteClamp"})
      /\ (y.run>x.run /\ act#"RemoteNotice" /\ act#"RemoteImport" /\ act#"RemoteFollowup"
           => y.run=x.run+1 /\ y.attempt=1)

(* A result has a child plus task-run key. A reply for an older task run
   cannot acknowledge a follow-up. Sent replies are not observations. *)
TaskAdvance(a,b) ==
 /\ \A k \in DOMAIN a.tasks \cap DOMAIN b.tasks :
      /\ b.tasks[k].pushes>=a.tasks[k].pushes
      /\ (a.tasks[k].delivery \in Handled => b.tasks[k].delivery=a.tasks[k].delivery)
 /\ \A k \in DOMAIN b.tasks : LET task == b.tasks[k] IN
      (task.delivery="Acknowledged" /\
       (k \notin DOMAIN a.tasks \/ a.tasks[k].delivery#"Acknowledged")) =>
       \E r \in b.replyObserved :
         r[1]=task.parent /\ r[4]=k[1] /\ r[5]=k[2] /\
         task.parent \in DOMAIN b.threads /\ b.threads[task.parent].live /\
         r[2]=b.threads[task.parent].run /\ r[3]=b.threads[task.parent].attempt /\
         ~b.threads[task.parent].emitted

QuestionAdvance(a,b) ==
 /\ DOMAIN a.questions \subseteq DOMAIN b.questions
 /\ \A k \in DOMAIN a.questions :
      /\ b.questions[k].epoch>=a.questions[k].epoch
      /\ (~a.questions[k].pending => ~b.questions[k].pending)

(* CT abstraction retains pinned identity and exact durable receipt sets.
   Full CT operation guards are a separate required relation. *)
Pinned(x) == <<x.id,x.source,x.target,x.base,x.point,x.items>>
TransferAdvance(a,b) ==
 /\ DOMAIN a.transfers \subseteq DOMAIN b.transfers
 /\ \A c \in DOMAIN a.transfers : LET x == a.transfers[c] y == b.transfers[c] IN
      /\ DOMAIN x.ledger \subseteq DOMAIN y.ledger
      /\ DOMAIN x.receipts \subseteq DOMAIN y.receipts
      /\ \A k \in DOMAIN x.ledger :
           /\ Pinned(x.ledger[k])=Pinned(y.ledger[k])
           /\ (x.ledger[k].status="Consumed" => x.ledger[k]=y.ledger[k])
      /\ \A k \in DOMAIN x.receipts : x.receipts[k] \subseteq y.receipts[k]
 /\ \A c \in DOMAIN b.transfers : LET x == b.transfers[c] IN
      \A k \in DOMAIN x.ledger : LET t == x.ledger[k] IN
       t.status="Consumed" => t.packet#"none" /\
         \E d \in DOMAIN x.receipts : {t.selected[i]:i \in 1..Len(t.selected)} \subseteq x.receipts[d]

CoreActions == {"OpenRoot","PersonSend","ServerEnqueue","StartFromQueue","Steer",
 "RequestInterrupt","ChildStart","ChildEnd","ProviderTerminal","RetryAttempt",
 "RestartActive","IngestChild","IngestStale","ResetPasses","Delegate","Followup",
 "IngestRoot","WaitTask","ReadTaskResult","StartWake","ResumeLimit","NarrowAccess",
 "DeliverOffer","Abandon","InterruptChat","Stop","NativeStart","NativeStartNested","NativeEnd","NativeClaim",
 "NativeDelivered","NativeRecover","RaiseQuestion","ImportQuestion","ImportClosed",
 "ImportAnswer","ImportRelease","AgentAnswer","PersonAnswer","StartQuestion","DeliverQuestion",
 "RemoteImport","RemoteFollowup","RemoteNotice","RemoteSent","RemoteStop","RemoteFence",
 "RemoteClamp","RemoteCancel","UsageLimitRecovery","IngestEnd","WaitReturn","WaitTimeout",
 "WaitAbandon","LateWaitAck","StartFromQueueWake","ReadResult","DoNotice","DoOffer","DoStop","CrashEngine"}
RunActions == CoreActions \ {"NativeStart","NativeStartNested","NativeEnd","NativeClaim","NativeDelivered","NativeRecover",
 "AgentAnswer","PersonAnswer","ImportQuestion","ImportClosed","ImportAnswer","ImportRelease","DeliverQuestion",
 "RemoteFence","RemoteSent","RemoteClamp"}
TaskActions == {"Delegate","Followup","IngestRoot","ReadTaskResult","WaitTask","StartWake",
 "DeliverOffer","Abandon","Stop","RequestInterrupt","RemoteImport","RemoteFollowup",
 "RemoteNotice","RemoteSent","RemoteStop","RemoteCancel","IngestEnd","WaitReturn","WaitTimeout",
 "StartFromQueueWake","ReadResult","DoNotice","DoOffer","DoStop"}
QuestionActions == {"RaiseQuestion","ImportQuestion","ImportClosed","ImportAnswer","ImportRelease",
 "AgentAnswer","PersonAnswer","StartQuestion","DeliverQuestion","IngestRoot","RequestInterrupt",
 "Stop","RemoteStop","RemoteCancel","Abandon","IngestEnd","DoStop"}
EvidenceFields == {"configured","ceilings","liveAccess","replySent","replyObserved","dispatches","hosts","online"}
CoreStep(a,b,act,args) ==
 /\ act \in CoreActions /\ WellFormed(b)
 /\ (act \in {"StartFromQueue","StartFromQueueWake","StartWake","StartQuestion",
       "UsageLimitRecovery","ResumeLimit"} => ~a.threads[args.t].live)
 /\ (act="Delegate" => a.threads[args.t].live /\ ~a.threads[args.t].interrupt /\
       a.threads[args.t].held="None")
 /\ Keep(a,b,(EvidenceFields \ {"hosts"}) \cup {"transfers"})
 /\ DOMAIN a.hosts \subseteq DOMAIN b.hosts
 /\ \A t \in DOMAIN a.hosts : a.hosts[t]=b.hosts[t]
 /\ \A t \in DOMAIN b.hosts \ DOMAIN a.hosts :
      act \in {"OpenRoot","Delegate","RemoteImport"} /\
      t \in DOMAIN b.threads \ DOMAIN a.threads
 /\ RunAdvance(a,b,act) /\ NativeAdvance(a,b,act,args)
 /\ TaskAdvance(a,b) /\ QuestionAdvance(a,b)
 /\ (act \notin RunActions => a.threads=b.threads)
 /\ (act \notin TaskActions => a.tasks=b.tasks)
 /\ (act \notin QuestionActions => a.questions=b.questions)
 /\ (act \notin {"NativeStart","NativeStartNested","NativeEnd","NativeClaim","NativeDelivered","NativeRecover"}
      => \A k \in DOMAIN a.natives :
         IF a.natives[k].status="Running" /\ ~a.natives[k].background /\
            ~Current(b,k[1],a.natives[k].run,a.natives[k].attempt)
         THEN b.natives[k]=[a.natives[k] EXCEPT !.status="Orphaned",!.delivery="Disposed"]
         ELSE b.natives[k]=a.natives[k])

ProviderStart(a,b,key,access,ceiling,device) ==
 /\ Current(a,key[1],key[2],key[3])
 /\ key[1] \in DOMAIN a.hosts /\ a.hosts[key[1]]=device
 /\ device \in DOMAIN a.online /\ a.online[device]
 /\ key[1] \in DOMAIN a.configured /\ key[1] \in DOMAIN a.ceilings
 /\ ceiling=a.ceilings[key[1]] /\ access<=ceiling /\ access<=a.configured[key[1]]
 /\ access<=a.threads[key[1]].access
 /\ key \notin DOMAIN a.dispatches
 /\ b.dispatches=a.dispatches @@ (key :> 1)
 /\ b.liveAccess=a.liveAccess @@ (key :> access)
 /\ Keep(a,b,(DOMAIN a) \ {"dispatches","liveAccess"})

Min2(x,y) == IF x<=y THEN x ELSE y

EvidenceStep(a,b,kind,args) ==
 /\ WellFormed(b)
 /\ CASE kind="provider_start" -> ProviderStart(a,b,args.key,args.access,args.ceiling,args.device)
    [] kind="access_config" ->
         /\ args.access \in Access /\ b.configured=(args.chat :> args.access) @@ a.configured
         /\ Keep(a,b,(DOMAIN a) \ {"configured"})
    [] kind="received_ceiling" ->
         /\ args.access \in Access
         /\ (args.chat \in DOMAIN a.ceilings /\ ~("baseline" \in DOMAIN args /\ args.baseline) => args.access<=a.ceilings[args.chat])
         /\ b.ceilings=(args.chat :>
              IF args.chat \in DOMAIN a.ceilings THEN Min2(args.access,a.ceilings[args.chat]) ELSE args.access) @@ a.ceilings
         /\ Keep(a,b,(DOMAIN a) \ {"ceilings"})
    [] kind="tool_reply" ->
         /\ b.replySent=a.replySent \cup {args.key}
         /\ Keep(a,b,(DOMAIN a) \ {"replySent"})
    [] kind="tool_observed" ->
         /\ args.key \in a.replySent
         /\ b.replyObserved=a.replyObserved \cup {args.key}
         /\ Keep(a,b,(DOMAIN a) \ {"replyObserved"})
    [] kind="registry_host" ->
         /\ args.validated /\ args.host#"none"
         /\ (args.chat \in DOMAIN a.hosts => a.hosts[args.chat]=args.host)
         /\ b.hosts=a.hosts @@ (args.chat :> args.host)
         /\ Keep(a,b,(DOMAIN a) \ {"hosts"})
    [] kind="device_reachability" ->
         /\ b.online=(args.device :> args.online) @@ a.online
         /\ Keep(a,b,(DOMAIN a) \ {"online"})
    [] kind \in {"transfer_checkpoint","transfer_state","transfer_operation","transfer_catalog"} ->
         /\ TransferAdvance(a,b) /\ Keep(a,b,(DOMAIN a) \ {"transfers"})
    [] OTHER -> FALSE
=============================================================================
