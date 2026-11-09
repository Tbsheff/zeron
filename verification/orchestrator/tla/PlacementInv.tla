---------------------------- MODULE PlacementInv ----------------------------
(* Inductive invariant for S9 (used by PlacementProof.tla). No TLAPS import, *)
(* so tla-ts can model-check IndInv (MC_PL_IndInv.cfg).                      *)
EXTENDS Placement

IdT  == (Dev \cup {"K", "none"}) \X Nat
RowT == [t : {"cmd", "out", "msg", "done"}, id : IdT, m : Mids \cup {"applied", "dup", "expired", "rejected", "K"}]
MsgT == [to : Dev, id : IdT, m : Mids]
H(m) == InitHost[Target(m)]
ChatRows(c) == edge[c] \cup UNION {rep[d][c] : d \in Dev}
Open(d, id, m) == id \in processed[d] /\ Cmd(id, m) \in rep[d][Target(m)]
                  /\ ~HasOut(rep[d][Target(m)], id)

TypeOKP ==
  /\ edge \in [Chats -> SUBSET RowT]
  /\ rep \in [Dev -> [Chats -> SUBSET RowT]]
  /\ online \in [Dev -> BOOLEAN]
  /\ host = InitHost
  /\ view = [d \in Dev |-> InitHost]
  /\ processed \in [Dev -> SUBSET IdT]
  /\ procMid \in [Dev -> SUBSET Mids]
  /\ journal \in [Dev -> SUBSET Mids]
  /\ executing \in [Dev -> SUBSET IdT]
  /\ inflight \in [Dev -> SUBSET IdT]
  /\ net \in SUBSET MsgT
  /\ relayLog \in SUBSET (IdT \X Dev)
  /\ wake \in SUBSET (Dev \X Chats)
  /\ issued \in SUBSET Mids
  /\ child \in {"idle", "running", "done"}
  /\ emitDue = FALSE
  /\ handoff = NoHandoff
  /\ dispatched \in [Mids -> 0..2]
  /\ execBy \in SUBSET (IdT \X Dev)
  /\ badHost = FALSE
  /\ seq \in [Dev -> Nat]
  /\ crashes \in Nat /\ offs \in [Dev -> Nat] /\ expires \in Nat
  /\ retries \in Nat /\ dups \in Nat /\ moves = 0 /\ rollbacks = 0

\* Command rows: ids are fresh per device sequence number, or the fixed result id.
CmdShape == \A c \in Chats : \A r \in ChatRows(c) : r.t = "cmd" =>
              /\ r.m \in Mids /\ Target(r.m) = c
              /\ IF r.id[1] \in Dev THEN r.id[2] \in 1..seq[r.id[1]]
                 ELSE r.id = ResId /\ r.m = "res"
IdUnique == \A c1, c2 \in Chats : \A r1 \in ChatRows(c1), r2 \in ChatRows(c2) :
              (r1.t = "cmd" /\ r2.t = "cmd" /\ r1.id = r2.id) => r1.m = r2.m
NetOK == \A msg \in net : Cmd(msg.id, msg.m) \in ChatRows(Target(msg.m))
\* Only the host writes outcomes and landed messages.
OutAtHost == \A c \in Chats : \A r \in ChatRows(c) :
               r.t \in {"out", "msg"} => r \in rep[InitHost[c]][c]
MsgAtTarget == \A c \in Chats : \A m \in Mids : MsgRow(m) \in ChatRows(c) => Target(m) = c
DispatchDoc == \A m \in Mids : dispatched[m] \in 0..1 /\
                 (dispatched[m] = 1 <=> MsgRow(m) \in rep[H(m)][Target(m)])
ProcAtHost == \A d \in Dev : \A id \in processed[d] :
                \E c \in Chats : \E r \in rep[d][c] : r.t = "cmd" /\ r.id = id /\ InitHost[c] = d
LiveIsOpen == \A d \in Dev : \A id \in executing[d] \cup inflight[d] :
                /\ id \in processed[d]
                /\ \A c \in Chats : ~HasOut(rep[d][c], id)
                /\ id \notin executing[d] \cap inflight[d]
OpenUnique == \A d \in Dev, m \in Mids : \A i1, i2 \in processed[d] :
                (Open(d, i1, m) /\ Open(d, i2, m)) => i1 = i2
OpenMid == \A d \in Dev, m \in Mids : \A id \in processed[d] : Open(d, id, m) => m \in procMid[d]
ExecNotRun == \A d \in Dev, m \in Mids : \A id \in executing[d] :
                Cmd(id, m) \in rep[d][Target(m)] => MsgRow(m) \notin rep[d][Target(m)]
ExecByHost == \A p \in execBy : \E m \in Mids : Cmd(p[1], m) \in rep[p[2]][Target(m)] /\ p[2] = H(m)
\* An out row sits in the same row set as its command row, in the command's own chat
\* (strengthening found by Apalache, apalache/PlacementInvA.tla: CTI on Pull, IngestClaim, Claim).
OutWithCmdIn(S, c) == \A r \in S : r.t = "out" => \E m \in Mids : Target(m) = c /\ Cmd(r.id, m) \in S
OutWithCmd == \A c \in Chats : OutWithCmdIn(edge[c], c) /\ \A d \in Dev : OutWithCmdIn(rep[d][c], c)

IndInv == /\ TypeOKP /\ CmdShape /\ IdUnique /\ NetOK /\ OutAtHost /\ MsgAtTarget
          /\ DispatchDoc /\ ProcAtHost /\ LiveIsOpen /\ OpenUnique /\ OpenMid
          /\ ExecNotRun /\ ExecByHost /\ OutWithCmd

\* Candidate extra conjunct for an S3Safe proof (checked by MC_PL_IndInv, not yet in IndInv).
S3Conj == /\ \A c \in Chats : \A r \in ChatRows(c) :
               (r = DoneRow \/ r = MsgRow("res") \/ r = Cmd(ResId, "res")) => child = "done"
          /\ (dispatched["res"] >= 1 => child = "done")
=============================================================================
