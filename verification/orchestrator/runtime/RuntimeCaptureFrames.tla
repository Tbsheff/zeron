---------------------- MODULE RuntimeCaptureFrames ----------------------
(* SQLite receipt/state/outbox commit frames. Effect payload semantics and
   the core action relation must ALSO hold. This module alone is not full
   conformance. Observe frames do not prove the I/O fact's semantic truth. *)
EXTENDS Naturals, Sequences, FiniteSets, TLC
TxSameExcept(f, g, keys) ==
  /\ DOMAIN f \ keys = DOMAIN g \ keys
  /\ \A k \in DOMAIN f \ keys : f[k] = g[k]
TxReceiptFrame(s, n, key, accepted) ==
  LET r == n.receipts[key]
      added == DOMAIN n.outbox \ DOMAIN s.outbox
  IN
  /\ key \notin DOMAIN s.receipts
  /\ DOMAIN n.receipts = DOMAIN s.receipts \cup {key}
  /\ TxSameExcept(s.receipts, n.receipts, {key})
  /\ r.accepted = accepted
  /\ IF accepted
     THEN /\ added = {r.effects[i] : i \in 1..Len(r.effects)}
          /\ Cardinality(added) = Len(r.effects)
          /\ \A i \in 1..Len(r.effects) :
               LET fx == n.outbox[r.effects[i]] IN
               fx.seq = r.seq /\ fx.idx = i-1 /\ ~fx.done
     ELSE /\ added = {} /\ r.effects = <<>> /\ n.raw_core = s.raw_core
  /\ DOMAIN s.outbox \subseteq DOMAIN n.outbox
  /\ TxSameExcept(s.outbox, n.outbox, added)
  /\ n.processed = s.processed /\ n.facts = s.facts
  /\ n.command_decisions=s.command_decisions

TxApply(s, n, args) ==
  IF args.duplicate
  THEN /\ args.key \in DOMAIN s.receipts
       /\ args.accepted = s.receipts[args.key].accepted
       /\ n = s
  ELSE /\ TxReceiptFrame(s, n, args.key, args.accepted)
       /\ n.command_receipts = IF args.command_id = "none" THEN s.command_receipts
            ELSE s.command_receipts @@ (args.command_id :> args.key)

TxComplete(s, n, id) ==
  /\ id \in DOMAIN s.outbox
  /\ n = [s EXCEPT !.outbox[id].done = TRUE]
TxMark(s, n, id) ==
  /\ ((id \in DOMAIN s.command_receipts /\ s.command_receipts[id] \in DOMAIN s.receipts)
      \/ (\E phase \in {"decision","outcome"} : <<id,phase>> \in DOMAIN s.command_decisions))
  /\ n = [s EXCEPT !.processed = @ \cup {id}]
TxObserve(s, n, id, kind) ==
  /\ id \in DOMAIN n.facts /\ n.facts[id].kind = kind
  /\ DOMAIN n.facts = DOMAIN s.facts \cup {id}
  /\ TxSameExcept(s.facts, n.facts, {id})
  /\ (id \in DOMAIN s.facts => n.facts[id] = s.facts[id])
  /\ n = [s EXCEPT !.facts = n.facts]


TxCommandDecision(s,n,args) ==
 LET key == <<args.command_id,args.phase>> IN
 /\ args.phase \in {"decision","outcome"}
 /\ IF args.duplicate
      THEN /\ key \in DOMAIN s.command_decisions
           /\ args.receipt=s.command_decisions[key] /\ n=s
      ELSE /\ key \notin DOMAIN s.command_decisions
           /\ args.receipt=args.observed
           /\ n=[s EXCEPT !.command_decisions=(key :> args.receipt) @@ @]

TxStep(s, n, action, args) ==
  CASE action = "Apply" -> TxApply(s, n, args)
    [] action = "CompleteEffect" -> TxComplete(s, n, args.effect_id)
    [] action = "MarkCommand" -> TxMark(s, n, args.command_id)
    [] action = "CommandDecision" -> TxCommandDecision(s,n,args)
    [] action = "Observe" -> TxObserve(s, n, args.fact_id, args.kind)
    [] action = "ObserveCatalog" -> args.fact_id \notin DOMAIN s.facts
         /\ TxObserve(s,n,args.fact_id,"transfer_catalog")
    [] action = "RecoveryOpen" -> n = s
    [] OTHER -> FALSE
====
