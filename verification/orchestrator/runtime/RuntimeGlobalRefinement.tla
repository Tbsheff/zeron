------------------- MODULE RuntimeGlobalRefinement -------------------
(* Strict global authority map, DESIGN 9.2. Foreign shadow rows cannot
   supply authoritative run state. No host move is supported. A capture
   prefix without the declared chat host is incomplete, not a model state.
   Pending CreateChild rows require captured registry and exact durable
   CreateChild effect evidence; they never supply executing run state. *)
EXTENDS RuntimeRefinement

Chats(parts) == UNION {DOMAIN parts[d].core.threads : d \in DOMAIN parts}
PlacedNames(parts) == UNION {KnownHosts(parts[d].mechanism):d \in DOMAIN parts}
Holders(parts,t) == {d \in DOMAIN parts : t \in DOMAIN parts[d].core.threads}
RouteHolders(parts,t) == {d \in DOMAIN parts : t \in KnownHosts(parts[d].mechanism)}
Hosts(parts,t) == {KnownHost(parts[d].mechanism,t) : d \in RouteHolders(parts,t)}
Host(parts,t) == CHOOSE d \in Hosts(parts,t) : TRUE
HasAuthority(parts,t) ==
 /\ Host(parts,t) \in DOMAIN parts
 /\ t \in DOMAIN parts[Host(parts,t)].core.threads
 /\ t \in KnownHosts(parts[Host(parts,t)].mechanism)
 /\ KnownHost(parts[Host(parts,t)].mechanism,t)=Host(parts,t)
PendingAt(parts,t,d) ==
 /\ t \in RegistryChats(parts[d].mechanism)
 /\ t \in DOMAIN parts[d].core.threads
 /\ LET th == parts[d].core.threads[t] route == Registry(parts[d].mechanism,t) IN
      /\ th.parent=route.parent /\ th.parent \in Chats(parts)
      /\ Host(parts,th.parent)=d
      /\ Ord(th)=0 /\ ~Live(th) /\ th.task.present
      /\ th.task.run=1 /\ th.task.st="Running"
      /\ \E id \in DOMAIN parts[d].mechanism.effects :
          LET e == parts[d].mechanism.effects[id].effect IN
          e.kind="CreateChild" /\ e.parent=th.parent /\ e.child=t /\
          e.depth=th.depth /\ e.access=th.access
PendingHolders(parts,t) == {d \in DOMAIN parts : PendingAt(parts,t,d)}
AuthorityReady(parts) ==
 /\ \A t \in PlacedNames(parts):Cardinality(Hosts(parts,t))=1
 /\ \A d \in DOMAIN parts :
      /\ FactSchema(parts[d].mechanism)
      /\ DOMAIN parts[d].core.threads \subseteq DOMAIN parts[d].mechanism.bindings
      /\ \A t \in DOMAIN parts[d].core.threads :
           /\ Cardinality(Hosts(parts,t))=1
           /\ (t \in KnownHosts(parts[d].mechanism) => KnownHost(parts[d].mechanism,t)=Host(parts,t))
           /\ (parts[d].mechanism.bindings[t].host_known =>
                parts[d].mechanism.bindings[t].shadow=(Host(parts,t)#d))
 /\ \A t \in Chats(parts) :
      /\ Cardinality(Hosts(parts,t))=1
      /\ HasAuthority(parts,t) \/ Cardinality(PendingHolders(parts,t))=1

Source(parts,t) == IF HasAuthority(parts,t) THEN parts[Host(parts,t)]
                  ELSE parts[CHOOSE d \in PendingHolders(parts,t):TRUE]
SourceState(parts,t) == AbstractState(Source(parts,t))
Parent(parts,t) == Source(parts,t).core.threads[t].parent
Root(parts,t) == Parent(parts,t)="none"
TaskOwner(parts,t) == Host(parts,Parent(parts,t))
TaskSource(parts,t) == AbstractState(parts[TaskOwner(parts,t)])
TaskKeys(parts) == UNION {{k \in DOMAIN AbstractState(parts[d]).tasks :
      k[1] \in Chats(parts) /\ ~Root(parts,k[1]) /\ TaskOwner(parts,k[1])=d} : d \in DOMAIN parts}
NativeKeys(parts) == UNION {{k \in DOMAIN AbstractState(parts[d]).natives :
      k[1] \in Chats(parts) /\ Host(parts,k[1])=d} : d \in DOMAIN parts}
QuestionKeys(parts) == UNION {{k \in DOMAIN AbstractState(parts[d]).questions :
      k[1] \in Chats(parts) /\ Host(parts,k[1])=d} : d \in DOMAIN parts}
RunKeys(parts) == UNION {{k \in DOMAIN AbstractState(parts[d]).liveAccess :
      k[1] \in Chats(parts) /\ Host(parts,k[1])=d} : d \in DOMAIN parts}
GlobalReady(parts) ==
 /\ AuthorityReady(parts)
 /\ \A t \in Chats(parts) :
      /\ Root(parts,t) \/ Parent(parts,t) \in Chats(parts)
      /\ IF Root(parts,t) THEN TRUE
         ELSE /\ t \in DOMAIN parts[TaskOwner(parts,t)].core.threads
              /\ parts[TaskOwner(parts,t)].core.threads[t].parent=Parent(parts,t)
              /\ parts[TaskOwner(parts,t)].core.threads[t].depth=Source(parts,t).core.threads[t].depth

GlobalState(parts) ==
 LET ts == Chats(parts)
     configs == {t \in ts : t \in DOMAIN SourceState(parts,t).configured}
     ceilings == {t \in ts : t \in DOMAIN SourceState(parts,t).ceilings}
     xfers == {t \in ts : t \in DOMAIN SourceState(parts,t).transfers}
     online == {d \in DOMAIN parts : d \in DOMAIN AbstractState(parts[d]).online}
 IN [threads |-> [t \in ts |-> SourceState(parts,t).threads[t]],
     tasks |-> [k \in TaskKeys(parts) |-> TaskSource(parts,k[1]).tasks[k]],
     natives |-> [k \in NativeKeys(parts) |-> SourceState(parts,k[1]).natives[k]],
     questions |-> [k \in QuestionKeys(parts) |-> SourceState(parts,k[1]).questions[k]],
     transfers |-> [t \in xfers |-> SourceState(parts,t).transfers[t]],
     configured |-> [t \in configs |-> SourceState(parts,t).configured[t]],
     ceilings |-> [t \in ceilings |-> SourceState(parts,t).ceilings[t]],
     liveAccess |-> [k \in RunKeys(parts) |-> SourceState(parts,k[1]).liveAccess[k]],
     dispatches |-> [k \in RunKeys(parts) |-> SourceState(parts,k[1]).dispatches[k]],
     replySent |-> UNION {{r \in AbstractState(parts[d]).replySent :
                          r[1] \in ts /\ Host(parts,r[1])=d} : d \in DOMAIN parts},
     replyObserved |-> UNION {{r \in AbstractState(parts[d]).replyObserved :
                              r[1] \in ts /\ Host(parts,r[1])=d} : d \in DOMAIN parts},
     hosts |-> [t \in PlacedNames(parts) |-> Host(parts,t)],
     online |-> [d \in online |-> AbstractState(parts[d]).online[d]]]

GlobalInitial(parts) == GlobalReady(parts) /\ Initial(GlobalState(parts))
GlobalBindingsStable(old,parts) ==
 /\ Chats(old) \subseteq Chats(parts)
 /\ \A t \in Chats(old) : Host(old,t)=Host(parts,t)
GlobalStep(old,parts,row) ==
 LET a == GlobalState(old) b == GlobalState(parts) IN
 /\ GlobalReady(old) /\ GlobalReady(parts) /\ GlobalBindingsStable(old,parts)
 /\ row.dev \in DOMAIN parts
 /\ DOMAIN old \subseteq DOMAIN parts
 /\ \A d \in DOMAIN old \ {row.dev} : old[d]=parts[d]
 /\ (DOMAIN parts \ DOMAIN old \subseteq {row.dev})
 /\ (row.action#"Init" => row.dev \in DOMAIN old)
 /\ CASE row.action="Init" -> a=b
    [] row.action="Apply" ->
         IF row.args.duplicate \/ ~row.args.accepted THEN a=b
         ELSE CoreStep(a,b,row.event.act,row.event.args)
    [] row.action \in {"Observe","ObserveCatalog"} ->
         LET before == old[row.dev].mechanism after == parts[row.dev].mechanism
             f == after.facts[row.args.fact_id]
         IN /\ FactsRetained(before,after)
            /\ (row.action="ObserveCatalog" => f.kind="transfer_catalog")
            /\ IF row.args.fact_id \in DOMAIN before.facts THEN a=b
               ELSE IF f.kind="provider_start" THEN ProviderFactStep(a,b,before,f,row.dev)
               ELSE IF f.kind="access_config" /\ ~ConfigKnown(f.data) THEN a=b
               ELSE IF f.kind="engine_lifecycle" THEN
                 /\ f.data.device \in LifecycleDevices(after)
                 /\ EvidenceStep(a,b,"device_reachability",[device |-> f.data.device,online |-> LastDevice(after,f.data.device)])
               ELSE IF f.kind \in PassiveKinds THEN PassiveFact(f) /\ a=b
               ELSE EvidenceStep(a,b,f.kind,EvidenceArgs(f,row.dev))
    [] row.action="CommandDecision" ->
         /\ old[row.dev].core=parts[row.dev].core
         /\ old[row.dev].mechanism.effects=parts[row.dev].mechanism.effects
         /\ old[row.dev].mechanism.facts=parts[row.dev].mechanism.facts /\ a=b
    [] row.action \in {"CompleteEffect","MarkCommand","RecoveryOpen"} -> a=b
    [] OTHER -> FALSE
=============================================================================
