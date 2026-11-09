----------------------- MODULE RuntimeRefinement -----------------------
(* Read-only refinement map from observed runtime core plus typed mechanism
   facts into RuntimeComposition. It never runs a reducer or reads posthash.
   fact_order is the first-observed commit order, not wall-clock order.
   Unknown optional access/host values are absent, never guessed. *)
EXTENDS RuntimeComposition

Ord(th) == th.run_base+Len(th.runs)
Live(th) == \E i \in 1..Len(th.runs) : th.runs[i].status="Live"
Members(xs) == {xs[i]:i \in 1..Len(xs)}
KindFacts(m,kind) == {id \in DOMAIN m.facts : m.facts[id].kind=kind}
LastFact(m,kind,chat) ==
 LET indexes == {i \in 1..Len(m.fact_order) : LET f == m.facts[m.fact_order[i]] IN
                   f.kind=kind /\ f.data.chat=chat}
 IN m.facts[m.fact_order[CHOOSE i \in indexes : \A j \in indexes : j<=i]].data
FactChats(m,kind) == {m.facts[id].data.chat:id \in KindFacts(m,kind)}
ConfigKnown(d) ==
 IF "configured_access" \in DOMAIN d
 THEN d.configured_access.present \/
      ("submitted_access" \in DOMAIN d /\ "policy_source" \in DOMAIN d /\
       d.policy_source="request_default" /\ d.submitted_access.present)
 ELSE "access" \in DOMAIN d
ConfigValue(d) ==
 IF "configured_access" \in DOMAIN d
 THEN IF d.configured_access.present THEN d.configured_access.value ELSE d.submitted_access.value
 ELSE d.access
ConfigFacts(m) == {id \in KindFacts(m,"access_config") : ConfigKnown(m.facts[id].data)}
LastConfig(m,t) ==
 LET indexes == {i \in 1..Len(m.fact_order) : m.fact_order[i] \in ConfigFacts(m) /\
                   m.facts[m.fact_order[i]].data.chat=t}
 IN m.facts[m.fact_order[CHOOSE i \in indexes : \A j \in indexes : j<=i]].data
LatestAccess(m,kind) ==
 IF kind="access_config" THEN
   [t \in {m.facts[id].data.chat:id \in ConfigFacts(m)} |-> ConfigValue(LastConfig(m,t))]
 ELSE [t \in FactChats(m,kind) |->
   LET levels == {m.facts[id].data.access : id \in {id \in KindFacts(m,kind):m.facts[id].data.chat=t}}
   IN CHOOSE a \in levels : \A b \in levels : a<=b]
ReplyKey(d) == <<d.parent,d.parent_run,d.parent_attempt,d.child,d.task_run,d.reply_id>>
AttemptKey(d) == <<d.chat,d.run,d.attempt>>
StartFacts(m) == {id \in KindFacts(m,"provider_start") :
                   m.facts[id].data.outcome="engine_start_ok" /\ m.facts[id].data.effective_access.present}
LastStart(m,key) ==
 LET indexes == {i \in 1..Len(m.fact_order) : LET f == m.facts[m.fact_order[i]] IN
                  f.kind="provider_start" /\ f.data.outcome="engine_start_ok" /\ AttemptKey(f.data)=key}
 IN m.facts[m.fact_order[CHOOSE i \in indexes : \A j \in indexes : j<=i]].data
RegistryChats(m) == FactChats(m,"registry_host")
Registry(m,t) == LastFact(m,"registry_host",t)
KnownHosts(m) == {t \in DOMAIN m.bindings : m.bindings[t].host_known} \cup RegistryChats(m)
KnownHost(m,t) == IF t \in RegistryChats(m) THEN Registry(m,t).host ELSE m.bindings[t].host
LifecycleDevices(m) == {m.facts[id].data.device : id \in
 {id \in KindFacts(m,"engine_lifecycle"):m.facts[id].data.state="started"}}
DeviceFacts(m) == KindFacts(m,"device_reachability") \cup
 {id \in KindFacts(m,"engine_lifecycle"):m.facts[id].data.device \in LifecycleDevices(m)}
LastDevice(m,dev) ==
 LET starts == {i \in 1..Len(m.fact_order): LET f==m.facts[m.fact_order[i]] IN
                 f.kind="engine_lifecycle" /\ f.data.device=dev /\ f.data.state="started"}
 IN IF starts#{} THEN
   LET start == CHOOSE i \in starts : \A j \in starts:j<=i
       instance == m.facts[m.fact_order[start]].data.engine_instance_id
       indexes == {i \in 1..Len(m.fact_order): LET f==m.facts[m.fact_order[i]] IN
        f.kind="engine_lifecycle" /\ f.data.device=dev /\ f.data.engine_instance_id=instance}
       last == CHOOSE i \in indexes : \A j \in indexes:j<=i
   IN m.facts[m.fact_order[last]].data.state#"stopped"
 ELSE LET indexes == {i \in 1..Len(m.fact_order): LET f==m.facts[m.fact_order[i]] IN
                    f.kind="device_reachability" /\ f.data.device=dev}
      IN m.facts[m.fact_order[CHOOSE i \in indexes : \A j \in indexes:j<=i]].data.online
PassiveFact(f) == LET d==f.data IN
 CASE f.kind \in {"journal_observed","doc_commit"} -> TRUE
 [] f.kind="remote_receive" -> d.validated /\ d.wire_id#""
 [] f.kind="admission_state" -> d.state \in {"open","quiesced","held"}
 [] f.kind="transport_connection" -> d.scope \in {"edge","chat_room"} /\
         d.state \in {"connected","disconnected"}
 [] f.kind="provider_event" -> d.stream_seq \in Nat /\ d.provider_run_id=d.observer.provider_run_id
 [] f.kind="provider_stream_catalog" -> d.complete \in BOOLEAN /\ Len(d.streams)>=0
 [] f.kind="native_binding" -> Len(d.native_path)>0 /\ d.tool_id=d.native_path[Len(d.native_path)] /\
       d.own_background \in BOOLEAN /\ d.effective_background \in BOOLEAN /\
       (d.own_background => d.effective_background) /\ d.doc_id#"" /\ d.chip_doc_id#""
 [] f.kind="native_doc_commit" -> Len(d.native_path)>0 /\ d.tool_id=d.native_path[Len(d.native_path)] /\
       d.chip_status \in {"running","done","failed"} /\ Len(d.doc_hash)=64 /\ Len(d.chip_doc_hash)=64
 [] f.kind="runtime_binding" -> d.validated /\ d.operation \in {"install","config","rebind","retire","remove","issue"} /\
       d.source_revision \in Nat /\ d.source_id#"" /\ d.runtime_id#""
 [] f.kind="agent_admission" -> d.validated /\ d.disposition \in {"accepted","refused"} /\
       d.reason \in {"current_runtime_attempt","runtime_attempt_mismatch"}
 [] f.kind="question_input_issued" ->
       {"chat","runtime_id","request_id","source_run","source_attempt","source_message_id"} \subseteq DOMAIN d
 [] f.kind="question_input_route" ->
       {"child","qid","request_id","receipt","source_run","source_attempt"} \subseteq DOMAIN d
 [] f.kind="capability_catalog" -> d.validated /\ d.complete \in BOOLEAN /\
       {"scope","providers","policy","policy_sources","core_config"} \subseteq DOMAIN d
 [] f.kind="transfer_provider" -> d.stage="accepted_request" /\ d.adapter="sqlite_transfer_fixture" /\
       d.session#"" /\ Len(d.packet_digest)>0 /\ Len(d.context_packet_digest)>0

 [] OTHER -> FALSE
PassiveKinds == {"journal_observed","doc_commit","remote_receive","admission_state",
 "transport_connection","provider_event","provider_stream_catalog","native_binding","native_doc_commit","transfer_provider","runtime_binding","agent_admission","capability_catalog","question_input_issued","question_input_route"}

AbstractState(row) ==
 LET s == row.core m == row.mechanism
     keys == {AttemptKey(m.facts[id].data):id \in StartFacts(m)}
     hosts == KnownHosts(m)
 IN [threads |-> [t \in DOMAIN s.threads |-> LET th == s.threads[t] IN
        [parent |-> th.parent,depth |-> th.depth,depth_limit |-> s.config.max_depth,
         fanout_limit |-> s.config.max_fanout,run |-> Ord(th),live |-> Live(th),
         attempt |-> th.attempt.n,emitted |-> th.attempt.emitted,access |-> th.access,
         held |-> th.held,wait |-> th.wait_on,interrupt |-> th.int_req]],
     tasks |-> [k \in {<<c,s.threads[c].task.run>> : c \in
                       {t \in DOMAIN s.threads:s.threads[t].task.present}} |->
        LET th == s.threads[k[1]] x == th.task IN
        [parent |-> th.parent,cohort |-> x.cohort,status |-> x.st,delivery |-> x.delivery,
         wake |-> x.wake,pushes |-> x.pushes]],
     natives |-> [k \in DOMAIN s.natives |-> LET n == s.natives[k] IN
        [run |-> n.attempt.run.ord,attempt |-> n.attempt.n,background |-> n.background,ancestor |-> n.ancestor,
         status |-> n.status,delivery |-> n.delivery,generation |-> n.generation]],
     questions |-> [k \in DOMAIN s.questions |-> LET q == s.questions[k] IN
        [pending |-> q.pending,epoch |-> q.epoch,holder |-> q.holder,
         views |-> q.views,waiting |-> q.waiting,seen |-> q.seen]],
     transfers |-> row.transfer,
     configured |-> LatestAccess(m,"access_config"),
     ceilings |-> LatestAccess(m,"received_ceiling"),
     liveAccess |-> [k \in keys |-> LastStart(m,k).effective_access.value],
     dispatches |-> [k \in keys |-> Cardinality({id \in StartFacts(m):AttemptKey(m.facts[id].data)=k})],
     replySent |-> {ReplyKey(m.facts[id].data):id \in KindFacts(m,"tool_reply")},
     replyObserved |-> {ReplyKey(m.facts[id].data):id \in KindFacts(m,"tool_observed")},
     hosts |-> [t \in hosts |-> KnownHost(m,t)],
     online |-> [d \in {m.facts[id].data.device:id \in DeviceFacts(m)} |-> LastDevice(m,d)]]

KnownFactKinds == {"provider_start","journal_observed","doc_commit","access_config",
 "received_ceiling","tool_reply","tool_observed","device_reachability","transfer_checkpoint","transfer_state","transfer_operation","transfer_catalog","registry_host","engine_lifecycle","remote_receive","admission_state",
 "transport_connection","provider_event","provider_stream_catalog","native_binding","native_doc_commit","transfer_provider","runtime_binding","agent_admission","capability_catalog","question_input_issued","question_input_route"}
FactSchema(m) ==
 /\ Members(m.fact_order)=DOMAIN m.facts
 /\ Cardinality(Members(m.fact_order))=Len(m.fact_order)
 /\ \A id \in DOMAIN m.facts : m.facts[id].kind \in KnownFactKinds
 /\ \A id \in {id \in DOMAIN m.facts:m.facts[id].kind \in PassiveKinds} : PassiveFact(m.facts[id])
 /\ \A id \in KindFacts(m,"engine_lifecycle") :
      /\ m.facts[id].data.state \in {"started","stopping","stopped"}
      /\ m.facts[id].data.generation=m.facts[id].data.engine_instance_id
 /\ \A id \in KindFacts(m,"transfer_catalog") :
      /\ m.facts[id].data.scope="durable_snapshots"
      /\ m.facts[id].data.unknown= <<>>
      /\ \A i \in 1..Len(m.facts[id].data.chats) : m.facts[id].data.chats[i].durable
 /\ \A t \in DOMAIN m.bindings :
      /\ (m.bindings[t].host_known => m.bindings[t].host#"none")
      /\ (m.bindings[t].host_known /\ t \in RegistryChats(m) => m.bindings[t].host=Registry(m,t).host)
 /\ \A id \in KindFacts(m,"registry_host") :
      /\ m.facts[id].data.validated /\ m.facts[id].data.host#"none"
      /\ m.facts[id].data.host=Registry(m,m.facts[id].data.chat).host
FactsRetained(a,b) ==
 /\ DOMAIN a.facts \subseteq DOMAIN b.facts
 /\ \A id \in DOMAIN a.facts : a.facts[id]=b.facts[id]
 /\ SubSeq(b.fact_order,1,Len(a.fact_order))=a.fact_order
NewFacts(a,b) == DOMAIN b.facts \ DOMAIN a.facts

EvidenceArgs(f,device) == LET d == f.data IN
 CASE f.kind="provider_start" ->
      [key |-> AttemptKey(d),access |-> d.effective_access.value,ceiling |-> d.access_ceiling.value,device |-> device]
 [] f.kind \in {"tool_reply","tool_observed"} -> [key |-> ReplyKey(d)]
 [] f.kind="access_config" -> [chat |-> d.chat,access |-> ConfigValue(d)]
 [] f.kind="received_ceiling" -> [chat |-> d.chat,access |-> d.access,
      baseline |-> "source_kind" \in DOMAIN d /\ d.source_kind="root_initial"]
 [] f.kind="registry_host" -> [chat |-> d.chat,host |-> d.host,validated |-> d.validated]
 [] f.kind="device_reachability" -> [device |-> d.device,online |-> d.online]
 [] OTHER -> [unused |-> TRUE]
FactGuard(f) == IF f.kind="provider_start"
               THEN f.data.effective_access.present /\ f.data.access_ceiling.present
               ELSE TRUE

SameDispatch(d,e) == <<d.chat,d.run,d.attempt,d.effect_id,d.message_id>> =
                     <<e.chat,e.run,e.attempt,e.effect_id,e.message_id>>
ProviderFactStep(a,b,before,f,device) == LET d == f.data IN
 CASE d.outcome="engine_start_ok" ->
       /\ FactGuard(f) /\ d.provider_return_id.present
       /\ d.journal_seq_after>=d.journal_seq_before
       /\ EvidenceStep(a,b,f.kind,EvidenceArgs(f,device))
 [] d.outcome \in {"failed","uncertain"} ->
       /\ ~d.effective_access.present /\ ~d.provider_return_id.present
       /\ d.journal_seq_after=d.journal_seq_before /\ d.journal_change="unchanged"
       /\ a=b
 [] d.outcome="already_done" ->
       /\ ~d.effective_access.present /\ ~d.provider_return_id.present
       /\ d.journal_seq_after=d.journal_seq_before /\ d.journal_change="unchanged"
       /\ \E id \in KindFacts(before,"journal_observed") :
           LET prior == before.facts[id].data IN
           SameDispatch(d,prior) /\ prior.journal_seq_after<=d.journal_seq_after
       /\ a=b
 [] OTHER -> FALSE

CompositionInitial(row) == FactSchema(row.mechanism) /\ Initial(AbstractState(row))
CompositionStep(old,row) ==
 LET a == AbstractState(old) b == AbstractState(row)
     before == old.mechanism after == row.mechanism
     added == NewFacts(before,after)
 IN /\ FactSchema(after) /\ FactsRetained(before,after)
    /\ CASE row.action="Apply" ->
           /\ added={}
           /\ IF row.args.duplicate \/ ~row.args.accepted THEN a=b
              ELSE CoreStep(a,b,row.event.act,row.event.args)
       [] row.action \in {"Observe","ObserveCatalog"} ->
           /\ row.args.fact_id \in DOMAIN after.facts
           /\ (row.action="ObserveCatalog" => after.facts[row.args.fact_id].kind="transfer_catalog")
           /\ IF row.args.fact_id \in DOMAIN before.facts
              THEN added={} /\ a=b
              ELSE /\ added={row.args.fact_id}
                   /\ LET f == after.facts[row.args.fact_id] IN
                        IF f.kind="provider_start"
                        THEN ProviderFactStep(a,b,before,f,row.dev)
                        ELSE IF f.kind="access_config" /\ ~ConfigKnown(f.data) THEN a=b
                        ELSE IF f.kind="engine_lifecycle" THEN
                          /\ f.data.device \in LifecycleDevices(after)
                          /\ EvidenceStep(a,b,"device_reachability",[device |-> f.data.device,online |-> LastDevice(after,f.data.device)])
                        ELSE IF f.kind \in PassiveKinds THEN PassiveFact(f) /\ a=b
                             ELSE EvidenceStep(a,b,f.kind,EvidenceArgs(f,row.dev))
       [] row.action="CommandDecision" ->
           /\ added={} /\ old.core=row.core
           /\ old.mechanism.effects=row.mechanism.effects /\ a=b
       [] row.action \in {"CompleteEffect","MarkCommand","RecoveryOpen"} ->
           added={} /\ a=b
       [] OTHER -> FALSE
=============================================================================
