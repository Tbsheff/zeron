------------------------- MODULE PlacementS10Proof -------------------------
(***************************************************************************)
(* S10, DESIGN's step form, for Placement (same scope as PlacementProof:   *)
(* v2, no host move, no rollback, no mutant; any devices, any budgets).    *)
(*   Spec => ReplayStepIsStutter   (Placement.tla)                          *)
(* A step that applies or replays an attempt whose receipt exists (outcome *)
(* row, or the message already ran here) changes no domain variable.       *)
(* Proof: IndInv (S9 proof) plus two receipt facts, JInv:                  *)
(*   JournalMid  : the run journal is inside the message-id ledger.        *)
(*   ExecNoJrnl  : a claimed, not yet dispatched attempt has not run here. *)
(***************************************************************************)
EXTENDS PlacementProof

USE ModeV2, NoMove, NoMutant, NoRollbck, DevTypes

JournalMid == \A d \in Dev : journal[d] \subseteq procMid[d]
ExecNoJrnl == \A d \in Dev, m \in Mids : \A id \in executing[d] :
                Cmd(id, m) \in rep[d][Target(m)] => m \notin journal[d]
JInv == JournalMid /\ ExecNoJrnl

THEOREM JInitThm == Init => JInv
  BY DEF Init, JInv, JournalMid, ExecNoJrnl

\* Steps that keep journal and executing (or shrink executing), grow procMid, and add no
\* command row for an executing id keep JInv.
LEMMA JFrame ==
  ASSUME JInv, journal' = journal,
         \A d \in Dev : procMid[d] \subseteq procMid'[d],
         \A d \in Dev : executing'[d] \subseteq executing[d],
         \A d \in Dev : \A id \in executing[d], m \in Mids :
            Cmd(id, m) \in rep'[d][Target(m)] => Cmd(id, m) \in rep[d][Target(m)]
  PROVE  JInv'
  BY DEF JInv, JournalMid, ExecNoJrnl

THEOREM JStep == IndInv /\ JInv /\ [Next]_vars => JInv'
<1> SUFFICES ASSUME IndInv, JInv, [Next]_vars PROVE JInv' OBVIOUS
<1> USE DEF IndInv
<1>a. \A m \in Mids : Target(m) \in Chats BY DEF Target, Chats, Mids
<1>b. \A d \in Dev : executing[d] \subseteq processed[d] BY DEF LiveIsOpen
\* Unchanged rep, journal, procMid; executing kept or cleared.
<1>0. CASE UNCHANGED vars BY <1>0 DEF vars, JInv, JournalMid, ExecNoJrnl
<1>1. CASE IssueRun
  <2>1. /\ rep' = [rep EXCEPT ![UserDev]["P"] = @ \cup {Cmd(<<UserDev, seq[UserDev] + 1>>, "run")}]
        /\ edge' = edge /\ UNCHANGED <<journal, procMid, executing>> /\ UserDev \in Dev /\ "P" \in Chats
    BY <1>1 DEF IssueRun, Dev, Chats
  <2>2. Cmd(<<UserDev, seq[UserDev] + 1>>, "run") \in RowT
    <3>1. <<UserDev, seq[UserDev] + 1>> \in IdT BY <2>1 DEF TypeOKP, IdT
    <3>. QED BY <3>1, RowTypes DEF Mids
  <2>3. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = UserDev /\ c2 = "P" THEN rep[d2][c2] \cup {Cmd(<<UserDev, seq[UserDev] + 1>>, "run")} ELSE rep[d2][c2]
    BY <2>1, <2>2, CRStep
  <2>4. \A d2 \in Dev : <<UserDev, seq[UserDev] + 1>> \notin executing[d2] BY <2>1, Fresh
  <2>. QED BY <2>1, <2>3, <2>4, <1>a, JFrame DEF Cmd
<1>2. ASSUME NEW d \in Dev, IssueKick(d) PROVE JInv'
  <2>1. /\ rep' = [rep EXCEPT ![d]["K"] = @ \cup {Cmd(<<d, seq[d] + 1>>, "kick")}]
        /\ edge' = edge /\ UNCHANGED <<journal, procMid, executing>> /\ "K" \in Chats
    BY <1>2 DEF IssueKick, Chats
  <2>2. Cmd(<<d, seq[d] + 1>>, "kick") \in RowT
    <3>1. <<d, seq[d] + 1>> \in IdT BY DEF TypeOKP, IdT
    <3>. QED BY <3>1, RowTypes DEF Mids
  <2>3. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = d /\ c2 = "K" THEN rep[d2][c2] \cup {Cmd(<<d, seq[d] + 1>>, "kick")} ELSE rep[d2][c2]
    BY <2>1, <2>2, CRStep
  <2>4. \A d2 \in Dev : <<d, seq[d] + 1>> \notin executing[d2] BY Fresh
  <2>. QED BY <2>1, <2>3, <2>4, <1>a, JFrame DEF Cmd
<1>3. ASSUME NEW d \in Dev, ChildFinish(d) PROVE JInv'
  <2>0. d = ChildHost
    <3>1. view[d]["K"] = d BY <1>3 DEF ChildFinish
    <3>2. view[d] = InitHost BY DEF TypeOKP
    <3>. QED BY <3>1, <3>2 DEF InitHost, Chats
  <2>1. ResId \notin processed[d]
    <3> SUFFICES ASSUME ResId \in processed[d] PROVE FALSE OBVIOUS
    <3>1. PICK c \in Chats, m \in Mids : InitHost[c] = d /\ Target(m) = c /\ Cmd(ResId, m) \in rep[d][c]
      BY ProcChat
    <3>2. Cmd(ResId, m) \in ChatRows(c) BY <3>1, RowsIn
    <3>3. m = "res" BY <3>2 DEF CmdShape, Cmd, ResId
    <3>. QED BY <3>1, <3>3, <2>0, TwoHosts DEF Target, InitHost
  <2>2. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = rep[d2][c2] \cup
            (IF d2 = d /\ c2 = "K" THEN {DoneRow}
             ELSE IF d2 = d /\ c2 = "P" /\ PushRoad THEN {Cmd(ResId, "res")} ELSE {})
    <3>1. rep \in RepT BY DEF TypeOKP, RepT
    <3>2. CASE PushRoad
      <4>1. rep' = [rep EXCEPT ![d]["K"] = @ \cup {DoneRow}, ![d]["P"] = @ \cup {Cmd(ResId, "res")}]
        BY <3>2, <1>3 DEF ChildFinish
      <4>. QED BY <4>1, <3>1, <3>2 DEF RepT, Chats
    <3>3. CASE ~PushRoad
      <4>1. rep' = [rep EXCEPT ![d]["K"] = @ \cup {DoneRow}] BY <3>3, <1>3 DEF ChildFinish
      <4>. QED BY <4>1, <3>1, <3>3 DEF RepT, Chats
    <3>. QED BY <3>2, <3>3
  <2>3. UNCHANGED <<journal, procMid, executing>> BY <1>3 DEF ChildFinish
  <2>. QED BY <2>1, <2>2, <2>3, <1>a, <1>b, JFrame DEF Cmd, DoneRow
<1>4. ASSUME NEW d \in Dev, EmitResult(d) PROVE JInv'
  BY <1>4 DEF EmitResult, TypeOKP
<1>5. ASSUME NEW d \in Dev, RemoteChildResult(d) PROVE JInv'
  <2>0. d = ParentHost /\ Cmd(ResId, "res") \notin rep[d]["P"]
    <3>1. view[d]["P"] = d /\ Cmd(ResId, "res") \notin rep[d]["P"] BY <1>5 DEF RemoteChildResult
    <3>2. view[d] = InitHost BY DEF TypeOKP
    <3>. QED BY <3>1, <3>2 DEF InitHost, Chats
  <2>1. ResId \notin processed[d]
    <3> SUFFICES ASSUME ResId \in processed[d] PROVE FALSE OBVIOUS
    <3>1. PICK c \in Chats, m \in Mids : InitHost[c] = d /\ Target(m) = c /\ Cmd(ResId, m) \in rep[d][c]
      BY ProcChat
    <3>2. Cmd(ResId, m) \in ChatRows(c) BY <3>1, RowsIn
    <3>3. m = "res" BY <3>2 DEF CmdShape, Cmd, ResId
    <3>. QED BY <3>1, <3>3, <2>0 DEF Target
  <2>2. /\ rep' = [rep EXCEPT ![d]["P"] = @ \cup {Cmd(ResId, "res")}] /\ edge' = edge
        /\ UNCHANGED <<journal, procMid, executing>>
    BY <1>5 DEF RemoteChildResult
  <2>3. Cmd(ResId, "res") \in RowT /\ "P" \in Chats BY RowTypes DEF Mids, Chats
  <2>4. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = d /\ c2 = "P" THEN rep[d2][c2] \cup {Cmd(ResId, "res")} ELSE rep[d2][c2]
    BY <2>2, <2>3, CRStep
  <2>. QED BY <2>1, <2>2, <2>4, <1>a, <1>b, JFrame DEF Cmd
<1>6. ASSUME NEW d \in Dev, GoOffline(d) PROVE JInv'
  BY <1>6 DEF GoOffline, JInv, JournalMid, ExecNoJrnl
<1>7. ASSUME NEW d \in Dev, GoOnline(d) PROVE JInv'
  BY <1>7 DEF GoOnline, JInv, JournalMid, ExecNoJrnl
<1>8. ASSUME NEW d \in Dev, Crash(d) PROVE JInv'
  <2>1. UNCHANGED <<journal, procMid, rep>> /\ executing' = [executing EXCEPT ![d] = {}] BY <1>8 DEF Crash
  <2>2. \A d2 \in Dev : executing'[d2] \subseteq executing[d2] BY <2>1 DEF TypeOKP
  <2>. QED BY <2>1, <2>2, JFrame
<1>9. ASSUME NEW d \in Dev, Rollback(d) PROVE JInv'
  BY <1>9 DEF Rollback, TypeOKP
<1>10. ASSUME NEW d \in Dev, NEW c \in Chats, Push(d, c) PROVE JInv'
  BY <1>10 DEF Push, JInv, JournalMid, ExecNoJrnl
<1>11. ASSUME NEW d \in Dev, NEW c \in Chats, Pull(d, c) PROVE JInv'
  <2>1. /\ rep' = [rep EXCEPT ![d][c] = @ \cup edge[c]] /\ edge' = edge
        /\ UNCHANGED <<journal, procMid, executing>>
    BY <1>11 DEF Pull
  <2>2. edge[c] \in SUBSET RowT BY DEF TypeOKP
  <2>3. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = d /\ c2 = c THEN rep[d2][c2] \cup edge[c] ELSE rep[d2][c2]
    BY <2>1, <2>2, CRStep
  <2>4. \A i, m : Cmd(i, m) \in edge[c] /\ i \in processed[d] => Cmd(i, m) \in rep[d][c]
    <3> SUFFICES ASSUME NEW i, NEW m, Cmd(i, m) \in edge[c], i \in processed[d] PROVE Cmd(i, m) \in rep[d][c]
      OBVIOUS
    <3>1. Cmd(i, m) \in ChatRows(c) BY RowsIn
    <3>. QED BY <3>1, ProcOwn
  <2>. QED BY <2>1, <2>3, <2>4, <1>a, <1>b, JFrame
<1>12. ASSUME NEW d \in Dev, NEW c \in Chats, RegistrySync(d, c) PROVE JInv'
  BY <1>12 DEF RegistrySync
<1>13. ASSUME NEW d \in Dev, NEW m \in Mids, RetryUser(d, m) PROVE JInv'
  <2>1. /\ rep' = [rep EXCEPT ![d][Target(m)] = @ \cup {Cmd(<<d, seq[d] + 1>>, m)}]
        /\ edge' = edge /\ UNCHANGED <<journal, procMid, executing>>
    BY <1>13 DEF RetryUser
  <2>2. <<d, seq[d] + 1>> \in IdT /\ Target(m) \in Chats BY <1>a DEF TypeOKP, IdT
  <2>3. Cmd(<<d, seq[d] + 1>>, m) \in RowT BY <2>2, RowTypes
  <2>4. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = d /\ c2 = Target(m) THEN rep[d2][c2] \cup {Cmd(<<d, seq[d] + 1>>, m)} ELSE rep[d2][c2]
    BY <2>1, <2>2, <2>3, CRStep
  <2>5. \A d2 \in Dev : <<d, seq[d] + 1>> \notin executing[d2] BY Fresh
  <2>. QED BY <2>1, <2>4, <2>5, <1>a, JFrame DEF Cmd
<1>14. CASE RelaySendA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ RelaySend(hh, r.id, r.m) BY <1>14 DEF RelaySendA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ RelaySend(hh, rr.id, rr.m) BY <2>1
  <2>. QED BY <2>2 DEF RelaySend, JInv, JournalMid, ExecNoJrnl
<1>15. CASE IngestClaimA
  <2>1. PICK msg \in net : IngestClaim(msg) BY <1>15 DEF IngestClaimA
  <2>2. msg \in MsgT /\ Cmd(msg.id, msg.m) \in ChatRows(Target(msg.m)) BY <2>1 DEF TypeOKP, NetOK
  <2>3. msg.to \in Dev /\ msg.m \in Mids /\ msg.id \in IdT /\ Target(msg.m) \in Chats
    BY <2>2 DEF MsgT, Target, Chats
  <2>4. /\ rep' = [rep EXCEPT ![msg.to][Target(msg.m)] = @ \cup {Cmd(msg.id, msg.m)}]
        /\ edge' = edge /\ journal' = journal
        /\ procMid' = [procMid EXCEPT ![msg.to] = @ \cup {msg.m}]
        /\ executing' = [executing EXCEPT ![msg.to] = @ \cup {msg.id}]
        /\ msg.id \notin executing[msg.to] /\ msg.m \notin procMid[msg.to]
    BY <2>1, Gates DEF IngestClaim, ClaimEff, ClaimGuard, LedgerFree
  <2>5. Cmd(msg.id, msg.m) \in RowT BY <2>3, RowTypes
  <2>6. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] =
           IF d2 = msg.to /\ c2 = Target(msg.m) THEN rep[d2][c2] \cup {Cmd(msg.id, msg.m)} ELSE rep[d2][c2]
    BY <2>3, <2>4, <2>5, CRStep
  <2>7. \A m2 \in Mids : Cmd(msg.id, m2) \in ChatRows(Target(m2)) => m2 = msg.m
    <3> SUFFICES ASSUME NEW m2 \in Mids, Cmd(msg.id, m2) \in ChatRows(Target(m2)) PROVE m2 = msg.m OBVIOUS
    <3>1. Cmd(msg.id, m2).t = "cmd" /\ Cmd(msg.id, msg.m).t = "cmd" /\ Cmd(msg.id, m2).id = Cmd(msg.id, msg.m).id BY DEF Cmd
    <3>2. Cmd(msg.id, m2) = Cmd(msg.id, msg.m) BY <3>1, <2>2, <2>3, <1>a, IdChat
    <3>. QED BY <3>2 DEF Cmd
  <2>8. JournalMid' BY <2>3, <2>4 DEF JInv, JournalMid, TypeOKP
  <2>9. ExecNoJrnl'
    <3> SUFFICES ASSUME NEW d2 \in Dev, NEW m2 \in Mids, NEW id \in executing'[d2], Cmd(id, m2) \in rep'[d2][Target(m2)]
                 PROVE m2 \notin journal[d2]
      BY <2>4 DEF ExecNoJrnl
    <3>1. CASE d2 = msg.to /\ id = msg.id
      <4>1. Cmd(id, m2) \in ChatRows(Target(m2)) BY <3>1, <2>6, <2>2, <1>a, RowsIn
      <4>2. m2 = msg.m BY <3>1, <4>1, <2>7
      <4>. QED BY <3>1, <4>2, <2>4 DEF JInv, JournalMid
    <3>2. CASE ~(d2 = msg.to /\ id = msg.id)
      <4>1. id \in executing[d2] BY <3>2, <2>3, <2>4 DEF TypeOKP
      <4>2. Cmd(id, m2) \in rep[d2][Target(m2)] BY <4>1, <3>2, <2>6, <1>a DEF Cmd
      <4>. QED BY <4>1, <4>2 DEF JInv, ExecNoJrnl
    <3>. QED BY <3>1, <3>2
  <2>. QED BY <2>8, <2>9 DEF JInv
<1>16. CASE IngestDupA
  <2>1. PICK msg \in net : IngestDup(msg) BY <1>16 DEF IngestDupA
  <2>2. msg \in MsgT BY <2>1 DEF TypeOKP
  <2>3. msg.to \in Dev /\ msg.m \in Mids /\ msg.id \in IdT /\ Target(msg.m) \in Chats
    BY <2>2 DEF MsgT, Target, Chats
  <2>4. /\ rep' = [rep EXCEPT ![msg.to][Target(msg.m)] = @ \cup {Cmd(msg.id, msg.m), Out(msg.id, "dup")}]
        /\ edge' = edge /\ UNCHANGED <<journal, procMid, executing>>
        /\ msg.id \notin processed[msg.to]
    BY <2>1 DEF IngestDup, DupGuard
  <2>5. {Cmd(msg.id, msg.m), Out(msg.id, "dup")} \subseteq RowT BY <2>3, RowTypes
  <2>6. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] =
           IF d2 = msg.to /\ c2 = Target(msg.m) THEN rep[d2][c2] \cup {Cmd(msg.id, msg.m), Out(msg.id, "dup")} ELSE rep[d2][c2]
    BY <2>3, <2>4, <2>5, CRStep
  <2>. QED BY <2>4, <2>6, <1>a, <1>b, JFrame DEF Cmd, Out
<1>17. CASE IngestDropA
  <2>1. PICK msg \in net : IngestDrop(msg) BY <1>17 DEF IngestDropA
  <2>. QED BY <2>1 DEF IngestDrop, JInv, JournalMid, ExecNoJrnl
<1>18. CASE RelayLoseA
  <2>1. PICK msg \in net : RelayLose(msg) BY <1>18 DEF RelayLoseA
  <2>. QED BY <2>1 DEF RelayLose, JInv, JournalMid, ExecNoJrnl
<1>19. CASE ClaimA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ Claim(hh, r.id, r.m) BY <1>19 DEF ClaimA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ Claim(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>5. /\ rep' = rep /\ journal' = journal
        /\ procMid' = [procMid EXCEPT ![hh] = @ \cup {rr.m}]
        /\ executing' = [executing EXCEPT ![hh] = @ \cup {rr.id}]
        /\ rr.m \notin procMid[hh]
    BY <2>2, <2>4, Gates DEF Claim, ClaimEff, ClaimGuard, LedgerFree
  <2>6. Cmd(rr.id, rr.m) \in ChatRows(cc) BY <2>2, <2>3, <2>4, CmdRec
  <2>7. \A m2 \in Mids : Cmd(rr.id, m2) \in ChatRows(Target(m2)) => m2 = rr.m
    <3> SUFFICES ASSUME NEW m2 \in Mids, Cmd(rr.id, m2) \in ChatRows(Target(m2)) PROVE m2 = rr.m OBVIOUS
    <3>1. Cmd(rr.id, m2).t = "cmd" /\ Cmd(rr.id, rr.m).t = "cmd" /\ Cmd(rr.id, m2).id = Cmd(rr.id, rr.m).id BY DEF Cmd
    <3>2. Cmd(rr.id, m2) = Cmd(rr.id, rr.m) BY <3>1, <2>6, <1>a, IdChat
    <3>. QED BY <3>2 DEF Cmd
  <2>8. JournalMid' BY <2>5 DEF JInv, JournalMid, TypeOKP
  <2>9. ExecNoJrnl'
    <3> SUFFICES ASSUME NEW d2 \in Dev, NEW m2 \in Mids, NEW id \in executing'[d2], Cmd(id, m2) \in rep'[d2][Target(m2)]
                 PROVE m2 \notin journal[d2]
      BY <2>5 DEF ExecNoJrnl
    <3>1. CASE d2 = hh /\ id = rr.id
      <4>1. Cmd(id, m2) \in ChatRows(Target(m2)) BY <3>1, <2>5, <1>a, RowsIn
      <4>2. m2 = rr.m BY <3>1, <4>1, <2>7
      <4>. QED BY <3>1, <4>2, <2>5 DEF JInv, JournalMid
    <3>2. CASE ~(d2 = hh /\ id = rr.id)
      <4>1. id \in executing[d2] BY <3>2, <2>5 DEF TypeOKP
      <4>. QED BY <4>1, <2>5 DEF JInv, ExecNoJrnl
    <3>. QED BY <3>1, <3>2
  <2>. QED BY <2>8, <2>9 DEF JInv
<1>20. CASE ClaimDupA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ ClaimDup(hh, r.id, r.m) BY <1>20 DEF ClaimDupA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ ClaimDup(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>5. rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "dup")}] /\ edge' = edge /\ UNCHANGED <<journal, procMid, executing>>
    BY <2>2, <2>4 DEF ClaimDup
  <2>6. Out(rr.id, "dup") \in RowT BY <2>4, RowTypes DEF RowT
  <2>7. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = hh /\ c2 = cc THEN rep[d2][c2] \cup {Out(rr.id, "dup")} ELSE rep[d2][c2]
    BY <2>5, <2>6, CRStep
  <2>. QED BY <2>5, <2>7, <1>a, JFrame DEF Cmd, Out
<1>21. CASE ClaimLandedA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ ClaimLanded(hh, r.id, r.m) BY <1>21 DEF ClaimLandedA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ ClaimLanded(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>5. rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "applied")}] /\ edge' = edge /\ UNCHANGED <<journal, procMid, executing>>
    BY <2>2, <2>4 DEF ClaimLanded
  <2>6. Out(rr.id, "applied") \in RowT BY <2>4, RowTypes DEF RowT
  <2>7. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = hh /\ c2 = cc THEN rep[d2][c2] \cup {Out(rr.id, "applied")} ELSE rep[d2][c2]
    BY <2>5, <2>6, CRStep
  <2>. QED BY <2>5, <2>7, <1>a, JFrame DEF Cmd, Out
<1>22. CASE ExecA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ Exec(hh, r.id, r.m) BY <1>22 DEF ExecA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ Exec(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>5. /\ rr.id \in executing[hh] /\ Cmd(rr.id, rr.m) \in rep[hh][cc]
        /\ rep' = [rep EXCEPT ![hh][cc] = @ \cup {MsgRow(rr.m)}] /\ edge' = edge
        /\ executing' = [executing EXCEPT ![hh] = @ \ {rr.id}]
        /\ journal' = [journal EXCEPT ![hh] = @ \cup {rr.m}]
        /\ procMid' = procMid
    BY <2>2, <2>4 DEF Exec, DispatchEff
  <2>6. rr.id \in processed[hh] /\ \A c2 \in Chats : ~HasOut(rep[hh][c2], rr.id) BY <2>5 DEF LiveIsOpen
  <2>7. Open(hh, rr.id, rr.m) BY <2>5, <2>6, <2>4 DEF Open
  <2>8. MsgRow(rr.m) \in RowT BY <2>4, RowTypes
  <2>9. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = hh /\ c2 = cc THEN rep[d2][c2] \cup {MsgRow(rr.m)} ELSE rep[d2][c2]
    BY <2>5, <2>8, CRStep
  <2>10. JournalMid'
    <3>1. rr.m \in procMid[hh] BY <2>4, <2>6, <2>7 DEF OpenMid
    <3>. QED BY <3>1, <2>5 DEF JInv, JournalMid, TypeOKP
  <2>11. ExecNoJrnl'
    <3> SUFFICES ASSUME NEW d2 \in Dev, NEW m2 \in Mids, NEW id \in executing'[d2], Cmd(id, m2) \in rep'[d2][Target(m2)]
                 PROVE m2 \notin journal'[d2]
      BY DEF ExecNoJrnl
    <3>1. id \in executing[d2] /\ (d2 = hh => id # rr.id) BY <2>5 DEF TypeOKP
    <3>2. Cmd(id, m2) \in rep[d2][Target(m2)] BY <2>9, <1>a DEF Cmd, MsgRow
    <3>3. m2 \notin journal[d2] BY <3>1, <3>2 DEF JInv, ExecNoJrnl
    <3>4. CASE d2 = hh /\ m2 = rr.m
      <4>1. id \in processed[hh] /\ \A c2 \in Chats : ~HasOut(rep[hh][c2], id) BY <3>1, <3>4 DEF LiveIsOpen
      <4>2. Open(hh, id, rr.m) BY <4>1, <3>2, <3>4, <2>4 DEF Open
      <4>3. id = rr.id BY <4>1, <4>2, <2>6, <2>7, <2>4 DEF OpenUnique
      <4>. QED BY <4>3, <3>1, <3>4
    <3>5. CASE ~(d2 = hh /\ m2 = rr.m) BY <3>3, <3>5, <2>5 DEF TypeOKP
    <3>. QED BY <3>4, <3>5
  <2>. QED BY <2>10, <2>11 DEF JInv
<1>23. CASE ResolveA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ Resolve(hh, r.id, r.m) BY <1>23 DEF ResolveA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ Resolve(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>5. rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "applied")}] /\ edge' = edge /\ UNCHANGED <<journal, procMid, executing>>
    BY <2>2, <2>4 DEF Resolve
  <2>6. Out(rr.id, "applied") \in RowT BY <2>4, RowTypes DEF RowT
  <2>7. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = hh /\ c2 = cc THEN rep[d2][c2] \cup {Out(rr.id, "applied")} ELSE rep[d2][c2]
    BY <2>5, <2>6, CRStep
  <2>. QED BY <2>5, <2>7, <1>a, JFrame DEF Cmd, Out
<1>24. CASE ExpireA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ Expire(hh, r.id, r.m) BY <1>24 DEF ExpireA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ Expire(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>5. rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "expired")}] /\ edge' = edge /\ UNCHANGED <<journal, procMid, executing>>
    BY <2>2, <2>4 DEF Expire
  <2>6. Out(rr.id, "expired") \in RowT BY <2>4, RowTypes DEF RowT
  <2>7. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = hh /\ c2 = cc THEN rep[d2][c2] \cup {Out(rr.id, "expired")} ELSE rep[d2][c2]
    BY <2>5, <2>6, CRStep
  <2>. QED BY <2>5, <2>7, <1>a, JFrame DEF Cmd, Out
<1>25. CASE SweepA
  BY <1>25 DEF SweepA, Sweep
<1>26. CASE RecoverDoneA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ RecoverDone(hh, r.id, r.m) BY <1>26 DEF RecoverDoneA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ RecoverDone(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>5. rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "applied")}] /\ edge' = edge /\ UNCHANGED <<journal, procMid, executing>>
    BY <2>2, <2>4 DEF RecoverDone
  <2>6. Out(rr.id, "applied") \in RowT BY <2>4, RowTypes DEF RowT
  <2>7. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = hh /\ c2 = cc THEN rep[d2][c2] \cup {Out(rr.id, "applied")} ELSE rep[d2][c2]
    BY <2>5, <2>6, CRStep
  <2>. QED BY <2>5, <2>7, <1>a, JFrame DEF Cmd, Out
<1>27. CASE RecoverRedoA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ RecoverRedo(hh, r.id, r.m) BY <1>27 DEF RecoverRedoA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ RecoverRedo(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>5. /\ Cmd(rr.id, rr.m) \in rep[hh][cc] /\ rr.id \in processed[hh] /\ ~HasOut(rep[hh][cc], rr.id)
        /\ rr.id \notin executing[hh]
        /\ rep' = [rep EXCEPT ![hh][cc] = @ \cup {MsgRow(rr.m), Out(rr.id, "applied")}] /\ edge' = edge
        /\ journal' = [journal EXCEPT ![hh] = @ \cup {rr.m}]
        /\ UNCHANGED <<procMid, executing>>
    BY <2>2, <2>4 DEF RecoverRedo, ReplayDue, DispatchEff
  <2>6. Open(hh, rr.id, rr.m) BY <2>5, <2>4 DEF Open
  <2>7. {MsgRow(rr.m), Out(rr.id, "applied")} \subseteq RowT BY <2>4, RowTypes DEF RowT
  <2>8. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = hh /\ c2 = cc THEN rep[d2][c2] \cup {MsgRow(rr.m), Out(rr.id, "applied")} ELSE rep[d2][c2]
    BY <2>5, <2>7, CRStep
  <2>9. JournalMid'
    <3>1. rr.m \in procMid[hh] BY <2>5, <2>6, <2>4 DEF OpenMid
    <3>. QED BY <3>1, <2>5 DEF JInv, JournalMid, TypeOKP
  <2>10. ExecNoJrnl'
    <3> SUFFICES ASSUME NEW d2 \in Dev, NEW m2 \in Mids, NEW id \in executing'[d2], Cmd(id, m2) \in rep'[d2][Target(m2)]
                 PROVE m2 \notin journal'[d2]
      BY DEF ExecNoJrnl
    <3>1. id \in executing[d2] BY <2>5
    <3>2. Cmd(id, m2) \in rep[d2][Target(m2)] BY <2>8, <1>a DEF Cmd, MsgRow, Out
    <3>3. m2 \notin journal[d2] BY <3>1, <3>2 DEF JInv, ExecNoJrnl
    <3>4. CASE d2 = hh /\ m2 = rr.m
      <4>1. id \in processed[hh] /\ \A c2 \in Chats : ~HasOut(rep[hh][c2], id) BY <3>1, <3>4 DEF LiveIsOpen
      <4>2. Open(hh, id, rr.m) BY <4>1, <3>2, <3>4, <2>4 DEF Open
      <4>3. id = rr.id BY <4>1, <4>2, <2>5, <2>6, <2>4 DEF OpenUnique
      <4>. QED BY <4>3, <3>1, <3>4, <2>5
    <3>5. CASE ~(d2 = hh /\ m2 = rr.m) BY <3>3, <3>5, <2>5 DEF TypeOKP
    <3>. QED BY <3>4, <3>5
  <2>. QED BY <2>9, <2>10 DEF JInv
<1>28. CASE RequeueA
  BY <1>28 DEF RequeueA, RecoverRequeue
<1>29. CASE MoveNaive
  BY <1>29 DEF MoveNaive
<1>30. CASE Release
  BY <1>30 DEF Release
<1>31. CASE Accept
  BY <1>31 DEF Accept
<1>32. CASE Terminal
  BY <1>32, <1>0 DEF Terminal
<1>. QED
  BY <1>0, <1>1, <1>2, <1>3, <1>4, <1>5, <1>6, <1>7, <1>8, <1>9, <1>10, <1>11, <1>12, <1>13, <1>14, <1>15, <1>16, <1>17, <1>18, <1>19, <1>20, <1>21, <1>22, <1>23, <1>24, <1>25, <1>26, <1>27, <1>28, <1>29, <1>30, <1>31, <1>32 DEF Next

----------------------------------------------------------------------------
(* The step property: a receipted attempt's apply step is a stutter on domain vars. *)

\* A step that adds only the rows S to rep[h][c] (edge kept) adds only closed rows when
\* every new row is an outcome or a command closed by an outcome in S.
LEMMA RowsClosedAdd ==
  ASSUME IndInv, NEW h \in Dev, NEW c \in Chats, NEW S \in SUBSET RowT,
         rep' = [rep EXCEPT ![h][c] = @ \cup S], edge' = edge,
         \A r \in S : r.t = "out" \/ (r.t = "cmd" /\ \E o \in S : o.t = "out" /\ o.id = r.id)
  PROVE  S10RowsClosed
  <1>1. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = h /\ c2 = c THEN rep[d2][c2] \cup S ELSE rep[d2][c2]
    BY CRStep
  <1>. QED BY <1>1 DEF S10RowsClosed, HasOut

THEOREM StepIsStutter == IndInv /\ JInv => ReplayStep
<1> SUFFICES ASSUME IndInv, JInv, NEW h \in Dev, NEW m \in Mids, NEW id \in S10Ids,
                    S10Receipt(h, id, m), S10Apply(h, id, m)
             PROVE  S10Stutter
  BY DEF ReplayStep
<1> USE DEF IndInv
<1> DEFINE tc == Target(m)
<1>0. tc \in Chats BY DEF Target, Chats
<1>1. CASE Claim(h, id, m)
  <2>1. ~S10Receipt(h, id, m) BY <1>1, Gates DEF Claim, ClaimGuard, LedgerFree, S10Receipt, RanBefore, JInv, JournalMid
  <2>. QED BY <2>1
<1>2. CASE ClaimDup(h, id, m)
  <2>1. rep' = [rep EXCEPT ![h][tc] = @ \cup {Out(id, "dup")}] /\ edge' = edge BY <1>2 DEF ClaimDup
  <2>2. id \in IdT
    <3>1. Cmd(id, m) \in ChatRows(tc) BY <1>2, <1>0, RowsIn DEF ClaimDup
    <3>. QED BY <3>1, <1>0, RowsIn DEF RowT, Cmd
  <2>3. Out(id, "dup") \in RowT BY <2>2, RowTypes
  <2>4. S10RowsClosed BY <2>1, <2>3, <1>0, RowsClosedAdd DEF Out
  <2>. QED BY <1>2, <2>4 DEF ClaimDup, S10Stutter, S10DomainVars
<1>3. CASE ClaimLanded(h, id, m)
  <2>1. rep' = [rep EXCEPT ![h][tc] = @ \cup {Out(id, "applied")}] /\ edge' = edge BY <1>3 DEF ClaimLanded
  <2>2. id \in IdT
    <3>1. Cmd(id, m) \in ChatRows(tc) BY <1>3, <1>0, RowsIn DEF ClaimLanded
    <3>. QED BY <3>1, <1>0, RowsIn DEF RowT, Cmd
  <2>3. Out(id, "applied") \in RowT BY <2>2, RowTypes
  <2>4. S10RowsClosed BY <2>1, <2>3, <1>0, RowsClosedAdd DEF Out
  <2>. QED BY <1>3, <2>4 DEF ClaimLanded, S10Stutter, S10DomainVars
<1>4. CASE Exec(h, id, m)
  <2>1. id \in executing[h] /\ Cmd(id, m) \in rep[h][tc] BY <1>4 DEF Exec
  <2>2. \A c2 \in Chats : ~HasOut(rep[h][c2], id) BY <2>1 DEF LiveIsOpen
  <2>3. MsgRow(m) \notin rep[h][tc] BY <2>1 DEF ExecNotRun
  <2>4. m \notin journal[h] BY <2>1 DEF JInv, ExecNoJrnl
  <2>. QED BY <2>2, <2>3, <2>4, <1>0 DEF S10Receipt, RanBefore
<1>5. CASE RecoverDone(h, id, m)
  <2>1. rep' = [rep EXCEPT ![h][tc] = @ \cup {Out(id, "applied")}] /\ edge' = edge BY <1>5 DEF RecoverDone
  <2>2. id \in IdT
    <3>1. Cmd(id, m) \in ChatRows(tc) BY <1>5, <1>0, RowsIn DEF RecoverDone
    <3>. QED BY <3>1, <1>0, RowsIn DEF RowT, Cmd
  <2>3. Out(id, "applied") \in RowT BY <2>2, RowTypes
  <2>4. S10RowsClosed BY <2>1, <2>3, <1>0, RowsClosedAdd DEF Out
  <2>. QED BY <1>5, <2>4 DEF RecoverDone, S10Stutter, S10DomainVars
<1>6. CASE RecoverRedo(h, id, m)
  BY <1>6 DEF RecoverRedo, ReplayDue, S10Receipt
<1>7. CASE RecoverRequeue(h, id, m)
  BY <1>7 DEF RecoverRequeue
<1>8. CASE Sweep(h, id, m)
  BY <1>8 DEF Sweep
<1>9. CASE \E msg \in net : msg.to = h /\ msg.id = id /\ msg.m = m /\ IngestClaim(msg)
  <2>1. PICK msg \in net : msg.to = h /\ msg.id = id /\ msg.m = m /\ IngestClaim(msg) BY <1>9
  <2>. QED BY <2>1, Gates DEF IngestClaim, ClaimGuard, LedgerFree, S10Receipt, RanBefore, JInv, JournalMid
<1>10. CASE \E msg \in net : msg.to = h /\ msg.id = id /\ msg.m = m /\ IngestDup(msg)
  <2>1. PICK msg \in net : msg.to = h /\ msg.id = id /\ msg.m = m /\ IngestDup(msg) BY <1>10
  <2>2. rep' = [rep EXCEPT ![h][tc] = @ \cup {Cmd(id, m), Out(id, "dup")}] /\ edge' = edge BY <2>1 DEF IngestDup
  <2>3. id \in IdT BY <2>1 DEF TypeOKP, MsgT
  <2>4. {Cmd(id, m), Out(id, "dup")} \subseteq RowT BY <2>3, RowTypes
  <2>5. S10RowsClosed BY <2>2, <2>4, <1>0, RowsClosedAdd DEF Out, Cmd
  <2>. QED BY <2>1, <2>5 DEF IngestDup, S10Stutter, S10DomainVars
<1>. QED BY <1>1, <1>2, <1>3, <1>4, <1>5, <1>6, <1>7, <1>8, <1>9, <1>10 DEF S10Apply

THEOREM S10StepSafe == Spec => ReplayStepIsStutter
<1>1. Init => IndInv /\ JInv BY InitInv, JInitThm
<1>2. IndInv /\ JInv /\ [Next]_vars => (IndInv /\ JInv)' BY StepInv, JStep
<1>3. IndInv /\ JInv => ReplayStep BY StepIsStutter
<1>. QED BY <1>1, <1>2, <1>3, PTL DEF Spec, ReplayStepIsStutter
=============================================================================
