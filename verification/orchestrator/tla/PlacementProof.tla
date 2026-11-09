--------------------------- MODULE PlacementProof ---------------------------
(***************************************************************************)
(* AC-M4, first cut: TLAPS inductive-invariant proof of S9 (OneHost) for   *)
(* Placement under the v2 rules, no host move, no rollback, no mutant.     *)
(* Unbounded: any number of laptops and Cloud devices, any MaxSeq and any  *)
(* crash / offline / expire / retry / duplicate budget.                    *)
(*   IndInv /\ Init => IndInv      (InitInv)                               *)
(*   IndInv /\ [Next]_vars => IndInv'   (StepInv, one step per action)     *)
(*   IndInv => S9                  (InvS9)                                 *)
(***************************************************************************)
EXTENDS PlacementInv, TLAPS

ASSUME ModeV2    == Mode = "v2"
ASSUME NoMove    == HostMove = "off"
ASSUME NoMutant  == Mutant = "none"
ASSUME NoRollbck == RollbackBudget = 0
ASSUME DevTypes  ==
  /\ IsFiniteSet(Laptops) /\ IsFiniteSet(Cloud)
  /\ "K" \notin Dev /\ "none" \notin Dev
  /\ MaxSeq \in Nat /\ CrashBudget \in Nat /\ OfflineBudget \in Nat
  /\ ExpireBudget \in Nat /\ RetryBudget \in Nat /\ DupBudget \in Nat
  /\ OffDevs \subseteq Dev
  /\ ParentHost \in Laptops /\ ChildHost \in Dev /\ UserDev \in Laptops
  /\ MoveTarget \in Laptops /\ PHolders \subseteq Laptops
ASSUME TwoHosts  == ChildHost # ParentHost

----------------------------------------------------------------------------

LEMMA InitHostType == InitHost \in [Chats -> Dev]
  BY DevTypes DEF InitHost, Dev

THEOREM InitInv == Init => IndInv
<1> SUFFICES ASSUME Init PROVE IndInv OBVIOUS
<1>1. TypeOKP BY InitHostType DEF Init, TypeOKP, NoHandoff
<1>2. \A c \in Chats : ChatRows(c) = {} BY DEF Init, ChatRows
<1>3. CmdShape /\ IdUnique /\ OutAtHost /\ MsgAtTarget BY <1>2 DEF CmdShape, IdUnique, OutAtHost, MsgAtTarget
<1>4. NetOK /\ ProcAtHost /\ LiveIsOpen /\ ExecByHost BY DEF Init, NetOK, ProcAtHost, LiveIsOpen, ExecByHost
<1>5. DispatchDoc
  <2>1. \A m \in Mids : H(m) \in Dev /\ Target(m) \in Chats BY InitHostType DEF H, Target, Chats, Mids
  <2>2. \A m \in Mids : rep[H(m)][Target(m)] = {} /\ dispatched[m] = 0 BY <2>1 DEF Init
  <2>. QED BY <2>2 DEF DispatchDoc
<1>6. OpenUnique /\ OpenMid
  <2>1. \A d \in Dev : processed[d] = {} BY DEF Init
  <2>. QED BY <2>1 DEF OpenUnique, OpenMid, Open
<1>7. ExecNotRun BY DEF Init, ExecNotRun
<1>8. OutWithCmd BY DEF Init, OutWithCmd, OutWithCmdIn
<1>. QED BY <1>1, <1>3, <1>4, <1>5, <1>6, <1>7, <1>8 DEF IndInv

THEOREM InvS9 == IndInv => S9
<1> SUFFICES ASSUME IndInv PROVE S9 OBVIOUS
<1>1. NoDoubleDispatch BY DEF IndInv, DispatchDoc, NoDoubleDispatch
<1>2. HostOnly BY DEF IndInv, TypeOKP, HostOnly
<1>3. OneExecutor
  <2> SUFFICES ASSUME NEW p \in execBy, NEW q \in execBy, p[1] = q[1] PROVE p[2] = q[2]
      BY DEF OneExecutor
  <2>1. PICK mp \in Mids : Cmd(p[1], mp) \in rep[p[2]][Target(mp)] /\ p[2] = H(mp)
        BY DEF IndInv, ExecByHost
  <2>2. PICK mq \in Mids : Cmd(q[1], mq) \in rep[q[2]][Target(mq)] /\ q[2] = H(mq)
        BY DEF IndInv, ExecByHost
  <2>3. p[2] \in Dev /\ q[2] \in Dev BY DEF IndInv, TypeOKP
  <2>4. Cmd(p[1], mp) \in ChatRows(Target(mp)) /\ Cmd(q[1], mq) \in ChatRows(Target(mq))
        BY <2>1, <2>2, <2>3 DEF ChatRows
  <2>5. Target(mp) \in Chats /\ Target(mq) \in Chats BY DEF Target, Chats
  <2>6. Cmd(p[1], mp).t = "cmd" /\ Cmd(q[1], mq).t = "cmd" /\ Cmd(p[1], mp).id = Cmd(q[1], mq).id
        BY DEF Cmd
  <2>7. Cmd(p[1], mp).m = Cmd(q[1], mq).m BY <2>4, <2>5, <2>6 DEF IndInv, IdUnique
  <2>8. mp = mq BY <2>7 DEF Cmd
  <2>. QED BY <2>1, <2>2, <2>8
<1>. QED BY <1>1, <1>2, <1>3 DEF S9

----------------------------------------------------------------------------
(* Inductive step: one case per action of Next. *)

\* Facts the cases share.
LEMMA Gates == MidGate /\ DocGate
  BY ModeV2, NoMutant DEF MidGate, DocGate

USE ModeV2, NoMove, NoMutant, NoRollbck, DevTypes

(* ---- shared lemmas ---- *)
CR(E, R, c) == E[c] \cup UNION {R[dd][c] : dd \in Dev}
RepT == [Dev -> [Chats -> SUBSET RowT]]

LEMMA RowTypes ==
  /\ \A i \in IdT, m \in Mids : Cmd(i, m) \in RowT
  /\ \A i \in IdT, o \in {"applied", "dup", "expired", "rejected"} : Out(i, o) \in RowT
  /\ \A m \in Mids : MsgRow(m) \in RowT
  /\ DoneRow \in RowT
  /\ ResId \in IdT
  BY DEF RowT, Cmd, Out, MsgRow, DoneRow, IdT, NoId, ResId, Mids

LEMMA RepExcept ==
  ASSUME NEW R \in RepT, NEW h \in Dev, NEW c \in Chats, NEW S \in SUBSET RowT
  PROVE  /\ [R EXCEPT ![h][c] = @ \cup S] \in RepT
         /\ \A d2 \in Dev, c2 \in Chats : [R EXCEPT ![h][c] = @ \cup S][d2][c2] =
              IF d2 = h /\ c2 = c THEN R[d2][c2] \cup S ELSE R[d2][c2]
  BY DEF RepT

LEMMA CRAdd ==
  ASSUME NEW E, NEW R \in RepT, NEW h \in Dev, NEW c \in Chats, NEW S \in SUBSET RowT, NEW c2 \in Chats
  PROVE  CR(E, [R EXCEPT ![h][c] = @ \cup S], c2) = IF c2 = c THEN CR(E, R, c2) \cup S ELSE CR(E, R, c2)
  <1>1. \A d2 \in Dev : [R EXCEPT ![h][c] = @ \cup S][d2][c2] =
              IF d2 = h /\ c2 = c THEN R[d2][c2] \cup S ELSE R[d2][c2]
    BY RepExcept
  <1>. QED BY <1>1 DEF CR

LEMMA ChatRowsCR == \A c \in Chats : ChatRows(c) = CR(edge, rep, c)
  BY DEF ChatRows, CR

LEMMA RowsIn == ASSUME IndInv PROVE
  \A c \in Chats : /\ edge[c] \subseteq ChatRows(c)
                   /\ \A d \in Dev : rep[d][c] \subseteq ChatRows(c)
                   /\ ChatRows(c) \subseteq RowT
  BY DEF IndInv, TypeOKP, ChatRows

LEMMA CmdRec == ASSUME NEW r \in RowT, r.t = "cmd" PROVE r = Cmd(r.id, r.m)
  BY DEF RowT, Cmd

\* An id names one command row, in one chat.
LEMMA IdChat ==
  ASSUME IndInv, NEW c1 \in Chats, NEW c2 \in Chats, NEW r1 \in ChatRows(c1), NEW r2 \in ChatRows(c2),
         r1.t = "cmd", r2.t = "cmd", r1.id = r2.id
  PROVE  c1 = c2 /\ r1 = r2 /\ r1.m \in Mids /\ Target(r1.m) = c1
  <1>1. r1.m = r2.m BY DEF IndInv, IdUnique
  <1>2. r1.m \in Mids /\ Target(r1.m) = c1 /\ Target(r2.m) = c2 BY DEF IndInv, CmdShape
  <1>3. r1 \in RowT /\ r2 \in RowT BY RowsIn
  <1>4. r1 = Cmd(r1.id, r1.m) /\ r2 = Cmd(r2.id, r2.m) BY <1>3, CmdRec
  <1>. QED BY <1>1, <1>2, <1>4

LEMMA ProcChat ==
  ASSUME IndInv, NEW d \in Dev, NEW i \in processed[d]
  PROVE  \E c \in Chats, m \in Mids : InitHost[c] = d /\ Target(m) = c /\ Cmd(i, m) \in rep[d][c]
  <1>1. PICK c \in Chats : \E r \in rep[d][c] : r.t = "cmd" /\ r.id = i /\ InitHost[c] = d
    BY DEF IndInv, ProcAtHost
  <1>2. PICK r \in rep[d][c] : r.t = "cmd" /\ r.id = i /\ InitHost[c] = d BY <1>1
  <1>3. r \in ChatRows(c) /\ r \in RowT BY <1>2, RowsIn
  <1>4. r.m \in Mids /\ Target(r.m) = c BY <1>2, <1>3 DEF IndInv, CmdShape
  <1>5. r = Cmd(i, r.m) BY <1>2, <1>3, CmdRec
  <1>. QED BY <1>2, <1>4, <1>5

\* A processed id's command row already sits in its host's replica of its own chat.
LEMMA ProcOwn ==
  ASSUME IndInv, NEW d \in Dev, NEW i \in processed[d], NEW c \in Chats, NEW m, Cmd(i, m) \in ChatRows(c)
  PROVE  m \in Mids /\ Target(m) = c /\ InitHost[c] = d /\ Cmd(i, m) \in rep[d][c]
  <1>1. PICK c0 \in Chats, m0 \in Mids : InitHost[c0] = d /\ Target(m0) = c0 /\ Cmd(i, m0) \in rep[d][c0]
    BY ProcChat
  <1>2. Cmd(i, m0) \in ChatRows(c0) BY <1>1, RowsIn
  <1>3. Cmd(i, m).t = "cmd" /\ Cmd(i, m0).t = "cmd" /\ Cmd(i, m).id = Cmd(i, m0).id BY DEF Cmd
  <1>4. c = c0 /\ Cmd(i, m) = Cmd(i, m0) BY <1>2, <1>3, IdChat
  <1>5. m = m0 BY <1>4 DEF Cmd
  <1>. QED BY <1>1, <1>4, <1>5

LEMMA OutOwn ==
  ASSUME IndInv, NEW d \in Dev, NEW c \in Chats, NEW i, HasOut(rep[d][c], i)
  PROVE  \E m \in Mids : Target(m) = c /\ Cmd(i, m) \in rep[d][c]
  BY DEF IndInv, OutWithCmd, OutWithCmdIn, HasOut

\* An outcome for id sits only in id's own chat.
LEMMA OutChat ==
  ASSUME IndInv, NEW d \in Dev, NEW c \in Chats, NEW c2 \in Chats, NEW i, NEW m,
         Cmd(i, m) \in ChatRows(c), HasOut(rep[d][c2], i)
  PROVE  c2 = c
  <1>1. PICK m2 \in Mids : Target(m2) = c2 /\ Cmd(i, m2) \in rep[d][c2] BY OutOwn
  <1>2. Cmd(i, m2) \in ChatRows(c2) BY <1>1, RowsIn
  <1>3. Cmd(i, m).t = "cmd" /\ Cmd(i, m2).t = "cmd" /\ Cmd(i, m).id = Cmd(i, m2).id BY DEF Cmd
  <1>. QED BY <1>2, <1>3, IdChat

\* A fresh id <<d, seq[d] + 1>> is in no row, ledger or live set.
LEMMA Fresh ==
  ASSUME IndInv, NEW d \in Dev
  PROVE  /\ \A c \in Chats : \A r \in ChatRows(c) : r.t = "cmd" => r.id # <<d, seq[d] + 1>>
         /\ \A d2 \in Dev : /\ <<d, seq[d] + 1>> \notin processed[d2]
                            /\ <<d, seq[d] + 1>> \notin executing[d2]
                            /\ <<d, seq[d] + 1>> \notin inflight[d2]
         /\ \A d2 \in Dev, c \in Chats : ~HasOut(rep[d2][c], <<d, seq[d] + 1>>)
  <1> DEFINE i == <<d, seq[d] + 1>>
  <1>1. \A c \in Chats : \A r \in ChatRows(c) : r.t = "cmd" => r.id # i
    <2> SUFFICES ASSUME NEW c \in Chats, NEW r \in ChatRows(c), r.t = "cmd", r.id = i PROVE FALSE
      OBVIOUS
    <2>1. seq[d] \in Nat BY DEF IndInv, TypeOKP
    <2>2. r.id[1] = d /\ r.id[2] = seq[d] + 1 OBVIOUS
    <2>3. r.id[2] \in 1..seq[d] BY <2>2 DEF IndInv, CmdShape
    <2>. QED BY <2>1, <2>2, <2>3
  <1>2. \A d2 \in Dev : i \notin processed[d2]
    <2> SUFFICES ASSUME NEW d2 \in Dev, i \in processed[d2] PROVE FALSE OBVIOUS
    <2>1. PICK c \in Chats, m \in Mids : Cmd(i, m) \in rep[d2][c] BY ProcChat
    <2>2. Cmd(i, m) \in ChatRows(c) BY <2>1, RowsIn
    <2>. QED BY <2>2, <1>1 DEF Cmd
  <1>3. \A d2 \in Dev : i \notin executing[d2] /\ i \notin inflight[d2]
    BY <1>2 DEF IndInv, LiveIsOpen
  <1>4. \A d2 \in Dev, c \in Chats : ~HasOut(rep[d2][c], i)
    <2> SUFFICES ASSUME NEW d2 \in Dev, NEW c \in Chats, HasOut(rep[d2][c], i) PROVE FALSE OBVIOUS
    <2>1. PICK m \in Mids : Target(m) = c /\ Cmd(i, m) \in rep[d2][c] BY OutOwn
    <2>2. Cmd(i, m) \in ChatRows(c) BY <2>1, RowsIn
    <2>. QED BY <2>2, <1>1 DEF Cmd
  <1>. QED BY <1>1, <1>2, <1>3, <1>4

LEMMA OWU ==
  ASSUME NEW S, NEW T, NEW c, OutWithCmdIn(S, c),
         \A r \in T : r.t = "out" => \E m \in Mids : Target(m) = c /\ Cmd(r.id, m) \in S \cup T
  PROVE  OutWithCmdIn(S \cup T, c)
  BY DEF OutWithCmdIn

\* The step adds the rows S to rep[h][c] only.
LEMMA CRStep ==
  ASSUME IndInv, NEW h \in Dev, NEW c \in Chats, NEW S \in SUBSET RowT,
         rep' = [rep EXCEPT ![h][c] = @ \cup S], edge' = edge
  PROVE  /\ rep' \in RepT
         /\ \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = h /\ c2 = c THEN rep[d2][c2] \cup S ELSE rep[d2][c2]
         /\ \A d2 \in Dev, c2 \in Chats : rep[d2][c2] \subseteq rep'[d2][c2]
         /\ \A c2 \in Chats : ChatRows(c2)' = IF c2 = c THEN ChatRows(c2) \cup S ELSE ChatRows(c2)
  <1>1. rep \in RepT BY DEF IndInv, TypeOKP, RepT
  <1>2. /\ rep' \in RepT
        /\ \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = h /\ c2 = c THEN rep[d2][c2] \cup S ELSE rep[d2][c2]
    BY <1>1, RepExcept
  <1>3. \A c2 \in Chats : ChatRows(c2)' = IF c2 = c THEN ChatRows(c2) \cup S ELSE ChatRows(c2)
    <2> SUFFICES ASSUME NEW c2 \in Chats PROVE ChatRows(c2)' = IF c2 = c THEN ChatRows(c2) \cup S ELSE ChatRows(c2)
      OBVIOUS
    <2>1. ChatRows(c2)' = CR(edge', rep', c2) BY DEF ChatRows, CR
    <2>2. CR(edge, [rep EXCEPT ![h][c] = @ \cup S], c2) = IF c2 = c THEN CR(edge, rep, c2) \cup S ELSE CR(edge, rep, c2)
      BY <1>1, CRAdd
    <2>. QED BY <2>1, <2>2, ChatRowsCR
  <1>. QED BY <1>2, <1>3

\* OutWithCmd after rep[h][c] grows by S (edge unchanged).
LEMMA OWRep ==
  ASSUME IndInv, NEW h \in Dev, NEW c \in Chats, NEW S \in SUBSET RowT,
         rep' = [rep EXCEPT ![h][c] = @ \cup S], edge' = edge,
         \A r \in S : r.t = "out" => \E m \in Mids : Target(m) = c /\ Cmd(r.id, m) \in rep[h][c] \cup S
  PROVE  OutWithCmd'
  <1>1. rep \in RepT BY DEF IndInv, TypeOKP, RepT
  <1>2. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = h /\ c2 = c THEN rep[d2][c2] \cup S ELSE rep[d2][c2]
    BY <1>1, RepExcept
  <1>3. OutWithCmdIn(rep[h][c] \cup S, c) BY OWU DEF IndInv, OutWithCmd
  <1>4. \A c2 \in Chats : OutWithCmdIn(edge[c2], c2) /\ \A d2 \in Dev : OutWithCmdIn(rep[d2][c2], c2)
    BY DEF IndInv, OutWithCmd
  <1>. QED BY <1>2, <1>3, <1>4 DEF OutWithCmd

\* OutWithCmd after edge[c] grows by rep[d][c] (rep unchanged).
LEMMA OWEdge ==
  ASSUME IndInv, NEW d \in Dev, NEW c \in Chats,
         edge' = [edge EXCEPT ![c] = @ \cup rep[d][c]], rep' = rep
  PROVE  OutWithCmd'
  <1>1. \A c2 \in Chats : edge'[c2] = IF c2 = c THEN edge[c2] \cup rep[d][c] ELSE edge[c2]
    BY DEF IndInv, TypeOKP
  <1>2. OutWithCmdIn(edge[c] \cup rep[d][c], c) BY DEF IndInv, OutWithCmd, OutWithCmdIn
  <1>. QED BY <1>1, <1>2 DEF IndInv, OutWithCmd

\* Steps that keep the ledger and the live sets and add no command row for a processed id
\* and no landed message next to an executing one keep OpenUnique, OpenMid, ExecNotRun.
LEMMA FrameOpen ==
  ASSUME IndInv,
         processed' = processed, procMid' = procMid, executing' = executing,
         \A d2 \in Dev, c2 \in Chats : rep[d2][c2] \subseteq rep'[d2][c2],
         \A d2 \in Dev : \A i \in processed[d2], m \in Mids :
            Cmd(i, m) \in rep'[d2][Target(m)] => Cmd(i, m) \in rep[d2][Target(m)],
         \A d2 \in Dev : \A i \in executing[d2], m \in Mids :
            Cmd(i, m) \in rep[d2][Target(m)] /\ MsgRow(m) \in rep'[d2][Target(m)] => MsgRow(m) \in rep[d2][Target(m)]
  PROVE  OpenUnique' /\ OpenMid' /\ ExecNotRun'
  <1>0. \A m \in Mids : Target(m) \in Chats BY DEF Target, Chats, Mids
  <1>1. \A d2 \in Dev, m \in Mids : \A i \in processed[d2] : Open(d2, i, m)' => Open(d2, i, m)
    BY <1>0 DEF Open, HasOut
  <1>2. OpenUnique' BY <1>1 DEF IndInv, OpenUnique
  <1>3. OpenMid' BY <1>1 DEF IndInv, OpenMid
  <1>4. ExecNotRun'
    <2> SUFFICES ASSUME NEW d2 \in Dev, NEW m \in Mids, NEW i \in executing[d2],
                        Cmd(i, m) \in rep'[d2][Target(m)], MsgRow(m) \in rep'[d2][Target(m)]
                 PROVE FALSE
      BY DEF ExecNotRun
    <2>1. i \in processed[d2] BY DEF IndInv, LiveIsOpen
    <2>. QED BY <2>1 DEF IndInv, ExecNotRun
  <1>. QED BY <1>2, <1>3, <1>4


THEOREM StepInv == IndInv /\ [Next]_vars => IndInv'
<1> SUFFICES ASSUME IndInv, [Next]_vars PROVE IndInv' OBVIOUS
<1> USE DEF IndInv
<1>0. CASE UNCHANGED vars
  BY <1>0 DEF vars, TypeOKP, CmdShape, IdUnique, NetOK, OutAtHost, MsgAtTarget, DispatchDoc,
     ProcAtHost, LiveIsOpen, OpenUnique, OpenMid, ExecNotRun, ExecByHost, ChatRows, Open, HasOut, H,
     OutWithCmd, OutWithCmdIn
<1>1. CASE IssueRun
  <2>1. \A d2 \in Dev : <<UserDev, seq[UserDev] + 1>> \notin processed[d2] /\ <<UserDev, seq[UserDev] + 1>> \notin executing[d2] \cup inflight[d2]
    <3>1. UserDev \in Dev BY DEF Dev
    <3>. QED BY <3>1, Fresh
  <2>e1. /\ UserDev \in Dev /\ "P" \in Chats
         /\ rep' = [rep EXCEPT ![UserDev]["P"] = @ \cup {Cmd(<<UserDev, seq[UserDev] + 1>>, "run")}]
         /\ edge' = edge /\ net' = net
    BY <1>1 DEF IssueRun, Dev, Chats
  <2>e2. Cmd(<<UserDev, seq[UserDev] + 1>>, "run") \in RowT
    <3>1. <<UserDev, seq[UserDev] + 1>> \in IdT BY <2>e1 DEF TypeOKP, IdT
    <3>. QED BY <3>1, RowTypes DEF Mids
  <2>e3. \A c2 \in Chats : ChatRows(c2)' = IF c2 = "P" THEN ChatRows(c2) \cup {Cmd(<<UserDev, seq[UserDev] + 1>>, "run")} ELSE ChatRows(c2)
    BY <2>e1, <2>e2, CRStep
  <2>t. TypeOKP' BY <1>1, <2>1, Gates, InitHostType, SMTT(20) DEF IssueRun, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK'
    <3> SUFFICES ASSUME NEW msg \in net PROVE Cmd(msg.id, msg.m) \in ChatRows(Target(msg.m))'
      BY <2>e1 DEF NetOK
    <3>1. Target(msg.m) \in Chats BY DEF TypeOKP, MsgT, Target, Chats
    <3>. QED BY <2>e3, <3>1 DEF NetOK
  <2>c3. OutAtHost' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>1, <2>1, Gates, InitHostType, SMTT(40) DEF IssueRun, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd' BY <2>e1, <2>e2, OWRep DEF Cmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>2. ASSUME NEW d \in Dev, IssueKick(d) PROVE IndInv'
  <2>1. \A d2 \in Dev : <<d, seq[d] + 1>> \notin processed[d2] /\ <<d, seq[d] + 1>> \notin executing[d2] \cup inflight[d2] BY <1>2 DEF ProcAtHost, LiveIsOpen, CmdShape, ChatRows, TypeOKP
  <2>t. TypeOKP' BY <1>2, <2>1, Gates, InitHostType, SMTT(20) DEF IssueKick, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>2, <2>1, Gates, InitHostType, SMTT(40) DEF IssueKick, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. rep' = [rep EXCEPT ![d]["K"] = @ \cup {Cmd(<<d, seq[d] + 1>>, "kick")}] /\ edge' = edge
      BY <1>2 DEF IssueKick
    <3>2. <<d, seq[d] + 1>> \in IdT /\ "K" \in Chats BY DEF TypeOKP, IdT, Chats
    <3>3. Cmd(<<d, seq[d] + 1>>, "kick") \in RowT BY <3>2, RowTypes DEF Mids
    <3>. QED BY <3>1, <3>2, <3>3, OWRep DEF Cmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>3. ASSUME NEW d \in Dev, ChildFinish(d) PROVE IndInv'
  <2>e0. d = ChildHost
    <3>1. view[d]["K"] = d BY <1>3 DEF ChildFinish
    <3>2. view[d] = InitHost BY DEF TypeOKP
    <3>. QED BY <3>1, <3>2 DEF InitHost, Chats
  <2>e1. ResId \notin processed[d]
    <3> SUFFICES ASSUME ResId \in processed[d] PROVE FALSE OBVIOUS
    <3>1. PICK c \in Chats, m \in Mids : InitHost[c] = d /\ Target(m) = c /\ Cmd(ResId, m) \in rep[d][c]
      BY ProcChat
    <3>2. Cmd(ResId, m) \in ChatRows(c) BY <3>1, RowsIn
    <3>3. m = "res" BY <3>2 DEF CmdShape, Cmd, ResId
    <3>. QED BY <3>1, <3>3, <2>e0, TwoHosts DEF Target, InitHost
  <2>e2. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = rep[d2][c2] \cup
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
  <2>e3. processed' = processed /\ procMid' = procMid /\ executing' = executing /\ edge' = edge
    BY <1>3 DEF ChildFinish
  <2>e4. OpenUnique' /\ OpenMid' /\ ExecNotRun'
    <3>1. \A d2 \in Dev, c2 \in Chats : rep[d2][c2] \subseteq rep'[d2][c2] BY <2>e2
    <3>2. \A d2 \in Dev : \A i \in processed[d2], m \in Mids :
            Cmd(i, m) \in rep'[d2][Target(m)] => Cmd(i, m) \in rep[d2][Target(m)]
      BY <2>e1, <2>e2 DEF Target, Chats, Cmd, DoneRow
    <3>3. \A d2 \in Dev : \A i \in executing[d2], m \in Mids :
            Cmd(i, m) \in rep[d2][Target(m)] /\ MsgRow(m) \in rep'[d2][Target(m)] => MsgRow(m) \in rep[d2][Target(m)]
      BY <2>e2 DEF Target, Chats, MsgRow, DoneRow, Cmd
    <3>. QED BY <2>e3, <3>1, <3>2, <3>3, FrameOpen
  <2>t. TypeOKP' BY <1>3, Gates, InitHostType, SMTT(20) DEF ChildFinish, PushRoad, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>3, Gates, InitHostType, SMTT(40) DEF ChildFinish, PushRoad, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>3, Gates, InitHostType, SMTT(40) DEF ChildFinish, PushRoad, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>3, Gates, InitHostType, SMTT(40) DEF ChildFinish, PushRoad, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>3, Gates, InitHostType, SMTT(40) DEF ChildFinish, PushRoad, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>3, Gates, InitHostType, SMTT(40) DEF ChildFinish, PushRoad, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>3, Gates, InitHostType, SMTT(40) DEF ChildFinish, PushRoad, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>3, Gates, InitHostType, SMTT(40) DEF ChildFinish, PushRoad, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>3, Gates, InitHostType, SMTT(40) DEF ChildFinish, PushRoad, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <2>e4
  <2>c9. OpenMid' BY <2>e4
  <2>c10. ExecNotRun' BY <2>e4
  <2>c11. ExecByHost' BY <1>3, Gates, InitHostType, SMTT(40) DEF ChildFinish, PushRoad, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. \A d2 \in Dev, c2 \in Chats : OutWithCmdIn(rep'[d2][c2], c2)
      <4> SUFFICES ASSUME NEW d2 \in Dev, NEW c2 \in Chats PROVE OutWithCmdIn(rep'[d2][c2], c2) OBVIOUS
      <4> DEFINE TT == IF d2 = d /\ c2 = "K" THEN {DoneRow}
                       ELSE IF d2 = d /\ c2 = "P" /\ PushRoad THEN {Cmd(ResId, "res")} ELSE {}
      <4>1. rep'[d2][c2] = rep[d2][c2] \cup TT BY <2>e2
      <4>2. \A r \in TT : r.t # "out" BY DEF DoneRow, Cmd
      <4>3. OutWithCmdIn(rep[d2][c2], c2) BY DEF OutWithCmd
      <4>. QED BY <4>1, <4>2, <4>3, OWU
    <3>. QED BY <3>1, <2>e3 DEF OutWithCmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>4. ASSUME NEW d \in Dev, EmitResult(d) PROVE IndInv'
  BY <1>4 DEF EmitResult, TypeOKP
<1>5. ASSUME NEW d \in Dev, RemoteChildResult(d) PROVE IndInv'
  <2>e0. d = ParentHost /\ Cmd(ResId, "res") \notin rep[d]["P"]
    <3>1. view[d]["P"] = d /\ Cmd(ResId, "res") \notin rep[d]["P"] BY <1>5 DEF RemoteChildResult
    <3>2. view[d] = InitHost BY DEF TypeOKP
    <3>. QED BY <3>1, <3>2 DEF InitHost, Chats
  <2>e1. ResId \notin processed[d]
    <3> SUFFICES ASSUME ResId \in processed[d] PROVE FALSE OBVIOUS
    <3>1. PICK c \in Chats, m \in Mids : InitHost[c] = d /\ Target(m) = c /\ Cmd(ResId, m) \in rep[d][c]
      BY ProcChat
    <3>2. Cmd(ResId, m) \in ChatRows(c) BY <3>1, RowsIn
    <3>3. m = "res" BY <3>2 DEF CmdShape, Cmd, ResId
    <3>. QED BY <3>1, <3>3, <2>e0 DEF Target
  <2>e2. /\ rep' = [rep EXCEPT ![d]["P"] = @ \cup {Cmd(ResId, "res")}] /\ edge' = edge
         /\ processed' = processed /\ procMid' = procMid /\ executing' = executing
    BY <1>5 DEF RemoteChildResult
  <2>e3. Cmd(ResId, "res") \in RowT /\ "P" \in Chats BY RowTypes DEF Mids, Chats
  <2>e4. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = d /\ c2 = "P" THEN rep[d2][c2] \cup {Cmd(ResId, "res")} ELSE rep[d2][c2]
    BY <2>e2, <2>e3, CRStep
  <2>e5. OpenUnique' /\ OpenMid' /\ ExecNotRun'
    <3>1. \A d2 \in Dev, c2 \in Chats : rep[d2][c2] \subseteq rep'[d2][c2] BY <2>e4
    <3>2. \A d2 \in Dev : \A i \in processed[d2], m \in Mids :
            Cmd(i, m) \in rep'[d2][Target(m)] => Cmd(i, m) \in rep[d2][Target(m)]
      BY <2>e1, <2>e4 DEF Target, Chats, Cmd
    <3>3. \A d2 \in Dev : \A i \in executing[d2], m \in Mids :
            Cmd(i, m) \in rep[d2][Target(m)] /\ MsgRow(m) \in rep'[d2][Target(m)] => MsgRow(m) \in rep[d2][Target(m)]
      BY <2>e4 DEF Target, Chats, MsgRow, Cmd
    <3>. QED BY <2>e2, <3>1, <3>2, <3>3, FrameOpen
  <2>t. TypeOKP' BY <1>5, Gates, InitHostType, SMTT(20) DEF RemoteChildResult, PullRoad, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>5, Gates, InitHostType, SMTT(40) DEF RemoteChildResult, PullRoad, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>5, Gates, InitHostType, SMTT(40) DEF RemoteChildResult, PullRoad, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>5, Gates, InitHostType, SMTT(40) DEF RemoteChildResult, PullRoad, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>5, Gates, InitHostType, SMTT(40) DEF RemoteChildResult, PullRoad, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>5, Gates, InitHostType, SMTT(40) DEF RemoteChildResult, PullRoad, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>5, Gates, InitHostType, SMTT(40) DEF RemoteChildResult, PullRoad, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>5, Gates, InitHostType, SMTT(40) DEF RemoteChildResult, PullRoad, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>5, Gates, InitHostType, SMTT(40) DEF RemoteChildResult, PullRoad, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <2>e5
  <2>c9. OpenMid' BY <2>e5
  <2>c10. ExecNotRun' BY <2>e5
  <2>c11. ExecByHost' BY <1>5, Gates, InitHostType, SMTT(40) DEF RemoteChildResult, PullRoad, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd' BY <2>e2, <2>e3, OWRep DEF Cmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>6. ASSUME NEW d \in Dev, GoOffline(d) PROVE IndInv'
  <2>1. UNCHANGED <<edge, rep, host, view, processed, procMid, journal, executing, inflight, dispatched, execBy, badHost, seq>> BY <1>6 DEF GoOffline
  <2>2. TypeOKP' BY <1>6, <2>1 DEF GoOffline, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>3. CmdShape' /\ IdUnique' /\ OutAtHost' /\ MsgAtTarget' BY <2>1 DEF CmdShape, IdUnique, OutAtHost, MsgAtTarget, ChatRows
  <2>4. DispatchDoc' /\ ProcAtHost' /\ LiveIsOpen' BY <2>1 DEF DispatchDoc, ProcAtHost, LiveIsOpen, H, HasOut
  <2>5. OpenUnique' /\ OpenMid' /\ ExecNotRun' /\ ExecByHost' BY <2>1 DEF OpenUnique, OpenMid, ExecNotRun, ExecByHost, Open, HasOut, H
  <2>6. NetOK' BY <1>6, <2>1 DEF GoOffline, NetOK, ChatRows
  <2>7. OutWithCmd' BY <2>1 DEF OutWithCmd
  <2>. QED BY <2>2, <2>3, <2>4, <2>5, <2>6, <2>7 DEF IndInv
<1>7. ASSUME NEW d \in Dev, GoOnline(d) PROVE IndInv'
  <2>1. UNCHANGED <<edge, rep, host, view, processed, procMid, journal, executing, inflight, dispatched, execBy, badHost, seq>> BY <1>7 DEF GoOnline
  <2>2. TypeOKP' BY <1>7, <2>1 DEF GoOnline, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>3. CmdShape' /\ IdUnique' /\ OutAtHost' /\ MsgAtTarget' BY <2>1 DEF CmdShape, IdUnique, OutAtHost, MsgAtTarget, ChatRows
  <2>4. DispatchDoc' /\ ProcAtHost' /\ LiveIsOpen' BY <2>1 DEF DispatchDoc, ProcAtHost, LiveIsOpen, H, HasOut
  <2>5. OpenUnique' /\ OpenMid' /\ ExecNotRun' /\ ExecByHost' BY <2>1 DEF OpenUnique, OpenMid, ExecNotRun, ExecByHost, Open, HasOut, H
  <2>6. NetOK' BY <1>7, <2>1 DEF GoOnline, NetOK, ChatRows
  <2>7. OutWithCmd' BY <2>1 DEF OutWithCmd
  <2>. QED BY <2>2, <2>3, <2>4, <2>5, <2>6, <2>7 DEF IndInv
<1>8. ASSUME NEW d \in Dev, Crash(d) PROVE IndInv'
  <2>t. TypeOKP' BY <1>8, Gates, InitHostType, SMTT(20) DEF Crash, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>8, Gates, InitHostType, SMTT(40) DEF Crash, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. edge' = edge /\ rep' = rep BY <1>8 DEF Crash
    <3>. QED BY <3>1 DEF OutWithCmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>9. ASSUME NEW d \in Dev, Rollback(d) PROVE IndInv'
  BY <1>9 DEF Rollback, TypeOKP
<1>10. ASSUME NEW d \in Dev, NEW c \in Chats, Push(d, c) PROVE IndInv'
  <2>t. TypeOKP' BY <1>10, Gates, InitHostType, SMTT(20) DEF Push, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>10, Gates, InitHostType, SMTT(40) DEF Push, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. edge' = [edge EXCEPT ![c] = @ \cup rep[d][c]] /\ rep' = rep BY <1>10 DEF Push
    <3>. QED BY <3>1, OWEdge
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>11. ASSUME NEW d \in Dev, NEW c \in Chats, Pull(d, c) PROVE IndInv'
  <2>e0. /\ rep' = [rep EXCEPT ![d][c] = @ \cup edge[c]] /\ edge' = edge
         /\ processed' = processed /\ procMid' = procMid
         /\ executing' = executing /\ inflight' = inflight
    BY <1>11 DEF Pull
  <2>e1. edge[c] \in SUBSET RowT BY DEF TypeOKP
  <2>e2. /\ \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = d /\ c2 = c THEN rep[d2][c2] \cup edge[c] ELSE rep[d2][c2]
         /\ \A d2 \in Dev, c2 \in Chats : rep[d2][c2] \subseteq rep'[d2][c2]
    BY <2>e0, <2>e1, CRStep
  <2>e3. \A i, m : Cmd(i, m) \in edge[c] /\ i \in processed[d] => Cmd(i, m) \in rep[d][c]
    <3> SUFFICES ASSUME NEW i, NEW m, Cmd(i, m) \in edge[c], i \in processed[d] PROVE Cmd(i, m) \in rep[d][c]
      OBVIOUS
    <3>1. Cmd(i, m) \in ChatRows(c) BY RowsIn
    <3>. QED BY <3>1, ProcOwn
  <2>e4. \A r \in edge[c] : r.t \in {"out", "msg"} => r \in rep[InitHost[c]][c] BY RowsIn DEF OutAtHost
  <2>e5. OpenUnique' /\ OpenMid' /\ ExecNotRun'
    <3>2. \A d2 \in Dev : \A i \in processed[d2], m \in Mids :
            Cmd(i, m) \in rep'[d2][Target(m)] => Cmd(i, m) \in rep[d2][Target(m)]
      BY <2>e2, <2>e3 DEF Target, Chats
    <3>3. \A d2 \in Dev : \A i \in executing[d2], m \in Mids :
            Cmd(i, m) \in rep[d2][Target(m)] /\ MsgRow(m) \in rep'[d2][Target(m)] => MsgRow(m) \in rep[d2][Target(m)]
      <4> SUFFICES ASSUME NEW d2 \in Dev, NEW i \in executing[d2], NEW m \in Mids,
                          Cmd(i, m) \in rep[d2][Target(m)], MsgRow(m) \in rep'[d2][Target(m)],
                          MsgRow(m) \notin rep[d2][Target(m)]
                   PROVE FALSE
        OBVIOUS
      <4>0. Target(m) \in Chats BY DEF Target, Chats
      <4>1. d2 = d /\ Target(m) = c /\ MsgRow(m) \in edge[c] BY <4>0, <2>e2
      <4>2. i \in processed[d2] BY DEF LiveIsOpen
      <4>3. Cmd(i, m) \in ChatRows(Target(m)) BY <4>0, RowsIn
      <4>4. InitHost[Target(m)] = d2 BY <4>0, <4>2, <4>3, ProcOwn
      <4>. QED BY <4>1, <4>4, <2>e4 DEF MsgRow
    <3>. QED BY <2>e0, <2>e2, <3>2, <3>3, FrameOpen
  <2>t. TypeOKP' BY <1>11, Gates, InitHostType, SMTT(20) DEF Pull, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>11, Gates, InitHostType, SMTT(40) DEF Pull, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>11, Gates, InitHostType, SMTT(40) DEF Pull, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>11, Gates, InitHostType, SMTT(40) DEF Pull, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>11, Gates, InitHostType, SMTT(40) DEF Pull, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>11, Gates, InitHostType, SMTT(40) DEF Pull, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>11, Gates, InitHostType, SMTT(40) DEF Pull, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>11, Gates, InitHostType, SMTT(40) DEF Pull, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen'
    <3> SUFFICES ASSUME NEW d2 \in Dev, NEW i \in executing[d2] \cup inflight[d2]
                 PROVE /\ i \in processed[d2]
                       /\ \A c2 \in Chats : ~HasOut(rep'[d2][c2], i)
                       /\ i \notin executing[d2] \cap inflight[d2]
      BY <2>e0 DEF LiveIsOpen
    <3>1. /\ i \in processed[d2] /\ i \notin executing[d2] \cap inflight[d2]
          /\ \A c2 \in Chats : ~HasOut(rep[d2][c2], i)
      BY DEF LiveIsOpen
    <3>2. ASSUME NEW c2 \in Chats, HasOut(rep'[d2][c2], i) PROVE FALSE
      <4>1. d2 = d /\ c2 = c /\ HasOut(edge[c], i) BY <3>1, <3>2, <2>e2 DEF HasOut
      <4>2. PICK r \in edge[c] : r.t = "out" /\ r.id = i BY <4>1 DEF HasOut
      <4>3. PICK m \in Mids : Target(m) = c /\ Cmd(i, m) \in edge[c] BY <4>2 DEF OutWithCmd, OutWithCmdIn
      <4>4. Cmd(i, m) \in ChatRows(c) BY <4>3, RowsIn
      <4>5. InitHost[c] = d BY <4>1, <4>4, <3>1, ProcOwn
      <4>6. r \in rep[d][c] BY <4>2, <4>5, <2>e4
      <4>. QED BY <4>1, <4>2, <4>6, <3>1 DEF HasOut
    <3>. QED BY <3>1, <3>2
  <2>c8. OpenUnique' BY <2>e5
  <2>c9. OpenMid' BY <2>e5
  <2>c10. ExecNotRun' BY <2>e5
  <2>c11. ExecByHost' BY <1>11, Gates, InitHostType, SMTT(40) DEF Pull, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. \A r \in edge[c] : r.t = "out" => \E m \in Mids : Target(m) = c /\ Cmd(r.id, m) \in rep[d][c] \cup edge[c]
      BY DEF OutWithCmd, OutWithCmdIn
    <3>. QED BY <2>e0, <2>e1, <3>1, OWRep
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>12. ASSUME NEW d \in Dev, NEW c \in Chats, RegistrySync(d, c) PROVE IndInv'
  BY <1>12 DEF RegistrySync
<1>13. ASSUME NEW d \in Dev, NEW m \in Mids, RetryUser(d, m) PROVE IndInv'
  <2>1. \A d2 \in Dev : <<d, seq[d] + 1>> \notin processed[d2] /\ <<d, seq[d] + 1>> \notin executing[d2] \cup inflight[d2] BY <1>13 DEF ProcAtHost, LiveIsOpen, CmdShape, ChatRows, TypeOKP
  <2>t. TypeOKP' BY <1>13, <2>1, Gates, InitHostType, SMTT(20) DEF RetryUser, AttIds, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>13, <2>1, Gates, InitHostType, SMTT(40) DEF RetryUser, AttIds, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. rep' = [rep EXCEPT ![d][Target(m)] = @ \cup {Cmd(<<d, seq[d] + 1>>, m)}] /\ edge' = edge
      BY <1>13 DEF RetryUser
    <3>2. <<d, seq[d] + 1>> \in IdT /\ Target(m) \in Chats BY DEF TypeOKP, IdT, Target, Chats
    <3>3. Cmd(<<d, seq[d] + 1>>, m) \in RowT BY <3>2, RowTypes
    <3>. QED BY <3>1, <3>2, <3>3, OWRep DEF Cmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>14. CASE RelaySendA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ RelaySend(hh, r.id, r.m) BY <1>14 DEF RelaySendA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ RelaySend(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>t. TypeOKP' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(20) DEF RelaySend, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>14, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RelaySend, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. edge' = edge /\ rep' = rep BY <2>2 DEF RelaySend
    <3>. QED BY <3>1 DEF OutWithCmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>15. CASE IngestClaimA
  <2>1. PICK msg \in net : IngestClaim(msg) BY <1>15 DEF IngestClaimA
  <2>2. msg \in MsgT /\ Cmd(msg.id, msg.m) \in ChatRows(Target(msg.m)) BY <2>1 DEF TypeOKP, NetOK
  <2>e0. msg.to \in Dev /\ msg.m \in Mids /\ msg.id \in IdT /\ Target(msg.m) \in Chats
    BY <2>2 DEF MsgT, Target, Chats
  <2>e1. /\ rep' = [rep EXCEPT ![msg.to][Target(msg.m)] = @ \cup {Cmd(msg.id, msg.m)}]
         /\ edge' = edge /\ seq' = seq /\ net' \subseteq net /\ inflight' = inflight
         /\ processed' = [processed EXCEPT ![msg.to] = @ \cup {msg.id}]
         /\ executing' = [executing EXCEPT ![msg.to] = @ \cup {msg.id}]
         /\ ClaimGuard(msg.to, Target(msg.m), msg.id, msg.m)
    BY <2>1 DEF IngestClaim, ClaimEff
  <2>e2. Cmd(msg.id, msg.m) \in RowT BY <2>e0, RowTypes
  <2>e3. /\ \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] =
              IF d2 = msg.to /\ c2 = Target(msg.m) THEN rep[d2][c2] \cup {Cmd(msg.id, msg.m)} ELSE rep[d2][c2]
         /\ \A c2 \in Chats : ChatRows(c2)' =
              IF c2 = Target(msg.m) THEN ChatRows(c2) \cup {Cmd(msg.id, msg.m)} ELSE ChatRows(c2)
    BY <2>e0, <2>e1, <2>e2, CRStep
  <2>e4. \A c2 \in Chats : ChatRows(c2)' = ChatRows(c2) BY <2>e3, <2>2
  <2>t. TypeOKP' BY <1>15, <2>1, <2>2, Gates, InitHostType, SMTT(20) DEF IngestClaim, ClaimGuard, ClaimEff, LedgerFree, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <2>e1, <2>e4 DEF CmdShape
  <2>c1. IdUnique' BY <2>e4 DEF IdUnique
  <2>c2. NetOK'
    <3> SUFFICES ASSUME NEW m2 \in net' PROVE Cmd(m2.id, m2.m) \in ChatRows(Target(m2.m))'
      BY DEF NetOK
    <3>1. m2 \in net /\ Target(m2.m) \in Chats BY <2>e1 DEF TypeOKP, MsgT, Target, Chats
    <3>. QED BY <3>1, <2>e4 DEF NetOK
  <2>c3. OutAtHost' BY <1>15, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestClaim, ClaimGuard, ClaimEff, LedgerFree, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>15, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestClaim, ClaimGuard, ClaimEff, LedgerFree, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc'
    <3>1. dispatched' = dispatched BY <2>1 DEF IngestClaim
    <3>2. \A m2 \in Mids : MsgRow(m2) \in rep'[H(m2)][Target(m2)] <=> MsgRow(m2) \in rep[H(m2)][Target(m2)]
      BY <2>e3, InitHostType DEF H, Target, Chats, MsgRow, Cmd
    <3>. QED BY <3>1, <3>2 DEF DispatchDoc
  <2>c6. ProcAtHost' BY <1>15, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestClaim, ClaimGuard, ClaimEff, LedgerFree, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen'
    <3>1. /\ msg.id \notin processed[msg.to] /\ msg.id \notin executing[msg.to] \cup inflight[msg.to]
          /\ ~HasOut(rep[msg.to][Target(msg.m)], msg.id)
      BY <2>e1 DEF ClaimGuard, LedgerFree
    <3>2. \A d2 \in Dev, c2 \in Chats : \A j : HasOut(rep'[d2][c2], j) <=> HasOut(rep[d2][c2], j)
      BY <2>e3 DEF HasOut, Cmd
    <3>3. \A c2 \in Chats : ~HasOut(rep[msg.to][c2], msg.id)
      <4> SUFFICES ASSUME NEW c2 \in Chats, HasOut(rep[msg.to][c2], msg.id) PROVE FALSE OBVIOUS
      <4>1. c2 = Target(msg.m) BY <2>2, <2>e0, OutChat
      <4>. QED BY <4>1, <3>1
    <3>4. \A d2 \in Dev : /\ processed'[d2] = IF d2 = msg.to THEN processed[d2] \cup {msg.id} ELSE processed[d2]
                          /\ executing'[d2] = IF d2 = msg.to THEN executing[d2] \cup {msg.id} ELSE executing[d2]
      BY <2>e0, <2>e1 DEF TypeOKP
    <3> SUFFICES ASSUME NEW d2 \in Dev, NEW j \in executing'[d2] \cup inflight'[d2]
                 PROVE /\ j \in processed'[d2]
                       /\ \A c2 \in Chats : ~HasOut(rep'[d2][c2], j)
                       /\ j \notin executing'[d2] \cap inflight'[d2]
      BY DEF LiveIsOpen
    <3>5. CASE d2 = msg.to /\ j = msg.id BY <3>1, <3>2, <3>3, <3>4, <3>5, <2>e1
    <3>6. CASE ~(d2 = msg.to /\ j = msg.id)
      <4>1. j \in executing[d2] \cup inflight[d2] BY <3>4, <3>6, <2>e1
      <4>2. /\ j \in processed[d2] /\ \A c2 \in Chats : ~HasOut(rep[d2][c2], j)
            /\ j \notin executing[d2] \cap inflight[d2]
        BY <4>1 DEF LiveIsOpen
      <4>. QED BY <4>2, <3>1, <3>2, <3>4, <3>6, <2>e1
    <3>. QED BY <3>5, <3>6
  <2>c8. OpenUnique' BY <1>15, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestClaim, ClaimGuard, ClaimEff, LedgerFree, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>15, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestClaim, ClaimGuard, ClaimEff, LedgerFree, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>15, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestClaim, ClaimGuard, ClaimEff, LedgerFree, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>15, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestClaim, ClaimGuard, ClaimEff, LedgerFree, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd' BY <2>e0, <2>e1, <2>e2, OWRep DEF Cmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>16. CASE IngestDupA
  <2>1. PICK msg \in net : IngestDup(msg) BY <1>16 DEF IngestDupA
  <2>2. msg \in MsgT /\ Cmd(msg.id, msg.m) \in ChatRows(Target(msg.m)) BY <2>1 DEF TypeOKP, NetOK
  <2>e0. msg.to \in Dev /\ msg.m \in Mids /\ msg.id \in IdT /\ Target(msg.m) \in Chats
    BY <2>2 DEF MsgT, Target, Chats
  <2>e1. /\ rep' = [rep EXCEPT ![msg.to][Target(msg.m)] = @ \cup {Cmd(msg.id, msg.m), Out(msg.id, "dup")}]
         /\ edge' = edge /\ seq' = seq /\ net' \subseteq net /\ execBy' = execBy
    BY <2>1 DEF IngestDup
  <2>e2. {Cmd(msg.id, msg.m), Out(msg.id, "dup")} \subseteq RowT BY <2>e0, RowTypes
  <2>e3. /\ \A d2 \in Dev, c2 \in Chats : rep[d2][c2] \subseteq rep'[d2][c2]
         /\ \A c2 \in Chats : ChatRows(c2)' =
              IF c2 = Target(msg.m) THEN ChatRows(c2) \cup {Cmd(msg.id, msg.m), Out(msg.id, "dup")} ELSE ChatRows(c2)
    BY <2>e0, <2>e1, <2>e2, CRStep
  <2>e4. \A c2 \in Chats : \A r \in ChatRows(c2)' : r.t # "out" => r \in ChatRows(c2)
    BY <2>e3, <2>2 DEF Out
  <2>e5. \A c2 \in Chats : ChatRows(c2) \subseteq ChatRows(c2)' BY <2>e3
  <2>t. TypeOKP' BY <1>16, <2>1, <2>2, Gates, InitHostType, SMTT(20) DEF IngestDup, DupGuard, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <2>e1, <2>e4 DEF CmdShape
  <2>c1. IdUnique' BY <2>e4 DEF IdUnique
  <2>c2. NetOK'
    <3> SUFFICES ASSUME NEW m2 \in net' PROVE Cmd(m2.id, m2.m) \in ChatRows(Target(m2.m))'
      BY DEF NetOK
    <3>1. m2 \in net /\ Target(m2.m) \in Chats BY <2>e1 DEF TypeOKP, MsgT, Target, Chats
    <3>. QED BY <3>1, <2>e5 DEF NetOK
  <2>c3. OutAtHost' BY <1>16, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDup, DupGuard, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <2>e4 DEF MsgAtTarget, MsgRow
  <2>c5. DispatchDoc'
    <3>1. dispatched' = dispatched BY <2>1 DEF IngestDup
    <3>2. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] =
              IF d2 = msg.to /\ c2 = Target(msg.m)
              THEN rep[d2][c2] \cup {Cmd(msg.id, msg.m), Out(msg.id, "dup")}
              ELSE rep[d2][c2]
      BY <2>e0, <2>e1, <2>e2, CRStep
    <3>3. \A m2 \in Mids : MsgRow(m2) \in rep'[H(m2)][Target(m2)] <=> MsgRow(m2) \in rep[H(m2)][Target(m2)]
      BY <3>2, InitHostType DEF H, Target, Chats, MsgRow, Cmd, Out
    <3>. QED BY <3>1, <3>3 DEF DispatchDoc
  <2>c6. ProcAtHost' BY <1>16, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDup, DupGuard, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>16, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDup, DupGuard, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>16, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDup, DupGuard, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>16, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDup, DupGuard, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>16, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDup, DupGuard, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost'
    <3> SUFFICES ASSUME NEW p \in execBy' PROVE \E m2 \in Mids : Cmd(p[1], m2) \in rep'[p[2]][Target(m2)] /\ p[2] = H(m2)
      BY DEF ExecByHost
    <3>1. PICK m2 \in Mids : Cmd(p[1], m2) \in rep[p[2]][Target(m2)] /\ p[2] = H(m2) BY <2>e1 DEF ExecByHost
    <3>2. p[2] \in Dev /\ Target(m2) \in Chats BY <3>1, InitHostType DEF H, Target, Chats
    <3>. QED BY <3>1, <3>2, <2>e3
  <2>c12. OutWithCmd'
    <3>1. \A r \in {Cmd(msg.id, msg.m), Out(msg.id, "dup")} : r.t = "out" =>
             \E mm \in Mids : Target(mm) = Target(msg.m) /\ Cmd(r.id, mm) \in rep[msg.to][Target(msg.m)] \cup {Cmd(msg.id, msg.m), Out(msg.id, "dup")}
      BY <2>e0 DEF Out, Cmd
    <3>. QED BY <2>e0, <2>e1, <2>e2, <3>1, OWRep
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>17. CASE IngestDropA
  <2>1. PICK msg \in net : IngestDrop(msg) BY <1>17 DEF IngestDropA
  <2>2. msg \in MsgT /\ Cmd(msg.id, msg.m) \in ChatRows(Target(msg.m)) BY <2>1 DEF TypeOKP, NetOK
  <2>t. TypeOKP' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(20) DEF IngestDrop, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>17, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF IngestDrop, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. edge' = edge /\ rep' = rep BY <2>1 DEF IngestDrop
    <3>. QED BY <3>1 DEF OutWithCmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>18. CASE RelayLoseA
  <2>1. PICK msg \in net : RelayLose(msg) BY <1>18 DEF RelayLoseA
  <2>2. msg \in MsgT /\ Cmd(msg.id, msg.m) \in ChatRows(Target(msg.m)) BY <2>1 DEF TypeOKP, NetOK
  <2>t. TypeOKP' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(20) DEF RelayLose, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>18, <2>1, <2>2, Gates, InitHostType, SMTT(40) DEF RelayLose, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. edge' = edge /\ rep' = rep BY <2>1 DEF RelayLose
    <3>. QED BY <3>1 DEF OutWithCmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>19. CASE ClaimA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ Claim(hh, r.id, r.m) BY <1>19 DEF ClaimA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ Claim(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>t. TypeOKP' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(20) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen'
    <3>1. /\ rep' = rep /\ inflight' = inflight
          /\ processed' = [processed EXCEPT ![hh] = @ \cup {rr.id}]
          /\ executing' = [executing EXCEPT ![hh] = @ \cup {rr.id}]
          /\ rr.id \notin processed[hh] /\ rr.id \notin executing[hh] \cup inflight[hh]
          /\ ~HasOut(rep[hh][cc], rr.id)
      BY <2>2, <2>4 DEF Claim, ClaimEff, ClaimGuard, LedgerFree
    <3>2. Cmd(rr.id, rr.m) \in ChatRows(cc) BY <2>2, <2>3, <2>4, CmdRec
    <3>3. \A c2 \in Chats : ~HasOut(rep[hh][c2], rr.id)
      <4> SUFFICES ASSUME NEW c2 \in Chats, HasOut(rep[hh][c2], rr.id) PROVE FALSE OBVIOUS
      <4>1. c2 = cc BY <3>2, OutChat
      <4>. QED BY <4>1, <3>1
    <3>4. \A d2 \in Dev : /\ processed'[d2] = IF d2 = hh THEN processed[d2] \cup {rr.id} ELSE processed[d2]
                          /\ executing'[d2] = IF d2 = hh THEN executing[d2] \cup {rr.id} ELSE executing[d2]
      BY <3>1 DEF TypeOKP
    <3> SUFFICES ASSUME NEW d2 \in Dev, NEW j \in executing'[d2] \cup inflight'[d2]
                 PROVE /\ j \in processed'[d2]
                       /\ \A c2 \in Chats : ~HasOut(rep'[d2][c2], j)
                       /\ j \notin executing'[d2] \cap inflight'[d2]
      BY DEF LiveIsOpen
    <3>5. CASE d2 = hh /\ j = rr.id BY <3>1, <3>3, <3>4, <3>5
    <3>6. CASE ~(d2 = hh /\ j = rr.id)
      <4>1. j \in executing[d2] \cup inflight[d2] BY <3>4, <3>6, <3>1
      <4>2. /\ j \in processed[d2] /\ \A c2 \in Chats : ~HasOut(rep[d2][c2], j)
            /\ j \notin executing[d2] \cap inflight[d2]
        BY <4>1 DEF LiveIsOpen
      <4>. QED BY <4>2, <3>1, <3>4, <3>6
    <3>. QED BY <3>5, <3>6
  <2>c8. OpenUnique' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>19, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Claim, ClaimGuard, ClaimEff, LedgerFree, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. edge' = edge /\ rep' = rep BY <2>2 DEF Claim
    <3>. QED BY <3>1 DEF OutWithCmd
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>20. CASE ClaimDupA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ ClaimDup(hh, r.id, r.m) BY <1>20 DEF ClaimDupA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ ClaimDup(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>t. TypeOKP' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(20) DEF ClaimDup, DupGuard, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK'
    <3>1. net' = net /\ edge' = edge /\ rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "dup")}]
      BY <2>2, <2>4 DEF ClaimDup
    <3>2. Out(rr.id, "dup") \in RowT BY <2>4, RowTypes DEF RowT
    <3>3. \A c2 \in Chats : ChatRows(c2) \subseteq ChatRows(c2)'
      BY <2>1, <3>1, <3>2, CRStep
    <3> SUFFICES ASSUME NEW m2 \in net' PROVE Cmd(m2.id, m2.m) \in ChatRows(Target(m2.m))'
      BY DEF NetOK
    <3>4. m2 \in net /\ Target(m2.m) \in Chats BY <3>1 DEF TypeOKP, MsgT, Target, Chats
    <3>. QED BY <3>3, <3>4 DEF NetOK
  <2>c3. OutAtHost' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>20, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimDup, DupGuard, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "dup")}] /\ edge' = edge
          /\ Cmd(rr.id, rr.m) \in rep[hh][cc]
      BY <2>2, <2>4 DEF ClaimDup
    <3>2. Out(rr.id, "dup") \in RowT BY <2>4, RowTypes DEF RowT
    <3>. QED BY <3>1, <3>2, <2>4, OWRep DEF Out
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>21. CASE ClaimLandedA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ ClaimLanded(hh, r.id, r.m) BY <1>21 DEF ClaimLandedA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ ClaimLanded(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>t. TypeOKP' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(20) DEF ClaimLanded, DupGuard, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>21, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF ClaimLanded, DupGuard, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "applied")}] /\ edge' = edge
          /\ Cmd(rr.id, rr.m) \in rep[hh][cc]
      BY <2>2, <2>4 DEF ClaimLanded
    <3>2. Out(rr.id, "applied") \in RowT BY <2>4, RowTypes DEF RowT
    <3>. QED BY <3>1, <3>2, <2>4, OWRep DEF Out
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>22. CASE ExecA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ Exec(hh, r.id, r.m) BY <1>22 DEF ExecA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ Exec(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>e0. /\ rr.id \in executing[hh] /\ Cmd(rr.id, rr.m) \in rep[hh][cc]
         /\ rep' = [rep EXCEPT ![hh][cc] = @ \cup {MsgRow(rr.m)}] /\ edge' = edge
         /\ executing' = [executing EXCEPT ![hh] = @ \ {rr.id}]
         /\ inflight' = [inflight EXCEPT ![hh] = @ \cup {rr.id}]
         /\ dispatched' = [dispatched EXCEPT ![rr.m] = Cap2(@)]
         /\ journal' = [journal EXCEPT ![hh] = @ \cup {rr.m}]
         /\ execBy' = execBy \cup {<<rr.id, hh>>}
         /\ badHost' = (badHost \/ host[Target(rr.m)] # hh)
         /\ child' = IF rr.m = "kick" /\ child = "idle" THEN "running" ELSE child
         /\ UNCHANGED <<online, host, view, processed, procMid, net, relayLog, wake, issued, emitDue,
                        handoff, seq, crashes, offs, expires, retries, dups, moves, rollbacks>>
    BY <2>2, <2>4 DEF Exec, DispatchEff
  <2>e1. rr.id \in processed[hh] /\ rr.id \notin inflight[hh] /\ \A c2 \in Chats : ~HasOut(rep[hh][c2], rr.id)
    BY <2>e0 DEF LiveIsOpen
  <2>e2. InitHost[cc] = hh
    <3>1. Cmd(rr.id, rr.m) \in ChatRows(cc) BY <2>e0, RowsIn
    <3>. QED BY <3>1, <2>e1, ProcOwn
  <2>e3. dispatched[rr.m] = 0
    <3>1. MsgRow(rr.m) \notin rep[hh][cc] BY <2>e0, <2>4 DEF ExecNotRun
    <3>. QED BY <3>1, <2>e2, <2>4 DEF DispatchDoc, H
  <2>e4. MsgRow(rr.m) \in RowT BY <2>4, RowTypes
  <2>e5. /\ rep' \in RepT
         /\ \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = hh /\ c2 = cc THEN rep[d2][c2] \cup {MsgRow(rr.m)} ELSE rep[d2][c2]
         /\ \A d2 \in Dev, c2 \in Chats : rep[d2][c2] \subseteq rep'[d2][c2]
         /\ \A c2 \in Chats : ChatRows(c2)' = IF c2 = cc THEN ChatRows(c2) \cup {MsgRow(rr.m)} ELSE ChatRows(c2)
    BY <2>e0, <2>e4, CRStep
  <2>e6. dispatched'[rr.m] = 1 /\ \A m2 \in Mids : m2 # rr.m => dispatched'[m2] = dispatched[m2]
    BY <2>e0, <2>e3, <2>4 DEF TypeOKP, Cap2
  <2>e7. \A d2 \in Dev : /\ executing'[d2] = IF d2 = hh THEN executing[d2] \ {rr.id} ELSE executing[d2]
                         /\ inflight'[d2] = IF d2 = hh THEN inflight[d2] \cup {rr.id} ELSE inflight[d2]
    BY <2>e0 DEF TypeOKP
  <2>t. TypeOKP'
    <3>1. badHost' = FALSE BY <2>e0, <2>e2, <2>4 DEF TypeOKP
    <3>2. dispatched' \in [Mids -> 0..2] BY <2>e0, <2>e3, <2>4 DEF TypeOKP, Cap2
    <3>3. journal' \in [Dev -> SUBSET Mids] BY <2>e0, <2>4 DEF TypeOKP
    <3>4. rr.id \in IdT BY <2>4 DEF RowT
    <3>5. execBy' \in SUBSET (IdT \X Dev) BY <2>e0, <3>4 DEF TypeOKP
    <3>6. executing' \in [Dev -> SUBSET IdT] /\ inflight' \in [Dev -> SUBSET IdT] BY <2>e0, <3>4 DEF TypeOKP
    <3>7. child' \in {"idle", "running", "done"} BY <2>e0 DEF TypeOKP
    <3>. QED BY <2>e0, <2>e5, <3>1, <3>2, <3>3, <3>5, <3>6, <3>7 DEF TypeOKP, RepT
  <2>c0. CmdShape' BY <1>22, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Exec, DispatchEff, Cap2, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>22, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Exec, DispatchEff, Cap2, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>22, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Exec, DispatchEff, Cap2, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost'
    <3> SUFFICES ASSUME NEW c2 \in Chats, NEW r \in ChatRows(c2)', r.t \in {"out", "msg"}
                 PROVE r \in rep'[InitHost[c2]][c2]
      BY DEF OutAtHost
    <3>1. InitHost[c2] \in Dev BY InitHostType
    <3>2. CASE r \in ChatRows(c2) BY <3>1, <3>2, <2>e5 DEF OutAtHost
    <3>3. CASE r \notin ChatRows(c2) BY <3>3, <2>e5, <2>e2
    <3>. QED BY <3>2, <3>3
  <2>c4. MsgAtTarget' BY <1>22, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Exec, DispatchEff, Cap2, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc'
    <3> SUFFICES ASSUME NEW m2 \in Mids
                 PROVE dispatched'[m2] \in 0..1 /\ (dispatched'[m2] = 1 <=> MsgRow(m2) \in rep'[H(m2)][Target(m2)])
      BY DEF DispatchDoc
    <3>1. H(m2) \in Dev /\ Target(m2) \in Chats BY InitHostType DEF H, Target, Chats
    <3>2. CASE m2 = rr.m BY <3>2, <2>e6, <2>e5, <2>e2, <2>4 DEF H
    <3>3. CASE m2 # rr.m
      <4>1. MsgRow(m2) \in rep'[H(m2)][Target(m2)] <=> MsgRow(m2) \in rep[H(m2)][Target(m2)]
        BY <3>1, <3>3, <2>e5 DEF MsgRow
      <4>. QED BY <4>1, <3>3, <2>e6 DEF DispatchDoc
    <3>. QED BY <3>2, <3>3
  <2>c6. ProcAtHost' BY <1>22, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Exec, DispatchEff, Cap2, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen'
    <3>1. \A d2 \in Dev, c2 \in Chats : \A j : HasOut(rep'[d2][c2], j) <=> HasOut(rep[d2][c2], j)
      BY <2>e5 DEF HasOut, MsgRow
    <3> SUFFICES ASSUME NEW d2 \in Dev, NEW j \in executing'[d2] \cup inflight'[d2]
                 PROVE /\ j \in processed'[d2]
                       /\ \A c2 \in Chats : ~HasOut(rep'[d2][c2], j)
                       /\ j \notin executing'[d2] \cap inflight'[d2]
      BY DEF LiveIsOpen
    <3>2. j \in executing[d2] \cup inflight[d2] BY <2>e7, <2>e0
    <3>3. /\ j \in processed[d2] /\ \A c2 \in Chats : ~HasOut(rep[d2][c2], j)
          /\ j \notin executing[d2] \cap inflight[d2]
      BY <3>2 DEF LiveIsOpen
    <3>4. j \notin executing'[d2] \cap inflight'[d2] BY <3>3, <2>e7, <2>e1
    <3>. QED BY <3>1, <3>3, <3>4, <2>e0
  <2>c8. OpenUnique' BY <1>22, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Exec, DispatchEff, Cap2, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>22, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Exec, DispatchEff, Cap2, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun'
    <3> SUFFICES ASSUME NEW d2 \in Dev, NEW m2 \in Mids, NEW j \in executing'[d2],
                        Cmd(j, m2) \in rep'[d2][Target(m2)], MsgRow(m2) \in rep'[d2][Target(m2)]
                 PROVE FALSE
      BY DEF ExecNotRun
    <3>1. Target(m2) \in Chats BY DEF Target, Chats
    <3>2. j \in executing[d2] /\ (d2 = hh => j # rr.id) BY <2>e7
    <3>3. Cmd(j, m2) \in rep[d2][Target(m2)] BY <3>1, <2>e5 DEF Cmd, MsgRow
    <3>4. CASE MsgRow(m2) \in rep[d2][Target(m2)] BY <3>2, <3>3, <3>4 DEF ExecNotRun
    <3>5. CASE MsgRow(m2) \notin rep[d2][Target(m2)]
      <4>1. d2 = hh /\ Target(m2) = cc /\ m2 = rr.m BY <3>1, <3>5, <2>e5 DEF MsgRow
      <4>2. j \in processed[hh] /\ \A c2 \in Chats : ~HasOut(rep[hh][c2], j) BY <3>2, <4>1 DEF LiveIsOpen
      <4>3. Open(hh, j, rr.m) /\ Open(hh, rr.id, rr.m) BY <4>1, <4>2, <3>3, <2>e0, <2>e1, <2>4 DEF Open
      <4>4. j = rr.id BY <4>3, <4>2, <2>e1, <2>4 DEF OpenUnique
      <4>. QED BY <4>4, <4>1, <3>2
    <3>. QED BY <3>4, <3>5
  <2>c11. ExecByHost'
    <3> SUFFICES ASSUME NEW p \in execBy' PROVE \E m2 \in Mids : Cmd(p[1], m2) \in rep'[p[2]][Target(m2)] /\ p[2] = H(m2)
      BY DEF ExecByHost
    <3>2. CASE p \in execBy
      <4>1. PICK m2 \in Mids : Cmd(p[1], m2) \in rep[p[2]][Target(m2)] /\ p[2] = H(m2) BY <3>2 DEF ExecByHost
      <4>2. p[2] \in Dev /\ Target(m2) \in Chats BY <4>1, InitHostType DEF H, Target, Chats
      <4>. QED BY <4>1, <4>2, <2>e5
    <3>3. CASE p = <<rr.id, hh>> BY <3>3, <2>e5, <2>e0, <2>e2, <2>4 DEF H
    <3>. QED BY <3>2, <3>3, <2>e0
  <2>c12. OutWithCmd' BY <2>e0, <2>e4, OWRep DEF MsgRow
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>23. CASE ResolveA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ Resolve(hh, r.id, r.m) BY <1>23 DEF ResolveA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ Resolve(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>e0. /\ rr.id \in inflight[hh] /\ Cmd(rr.id, rr.m) \in rep[hh][cc]
         /\ rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "applied")}] /\ edge' = edge
    BY <2>2, <2>4 DEF Resolve
  <2>e1. InitHost[cc] = hh
    <3>1. rr.id \in processed[hh] BY <2>e0 DEF LiveIsOpen
    <3>2. Cmd(rr.id, rr.m) \in ChatRows(cc) BY <2>e0, RowsIn
    <3>. QED BY <3>1, <3>2, ProcOwn
  <2>e2. Out(rr.id, "applied") \in RowT BY <2>4, RowTypes DEF RowT
  <2>e3. /\ \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = hh /\ c2 = cc THEN rep[d2][c2] \cup {Out(rr.id, "applied")} ELSE rep[d2][c2]
         /\ \A d2 \in Dev, c2 \in Chats : rep[d2][c2] \subseteq rep'[d2][c2]
         /\ \A c2 \in Chats : ChatRows(c2)' = IF c2 = cc THEN ChatRows(c2) \cup {Out(rr.id, "applied")} ELSE ChatRows(c2)
    BY <2>e0, <2>e2, CRStep
  <2>t. TypeOKP' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(20) DEF Resolve, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost'
    <3> SUFFICES ASSUME NEW c2 \in Chats, NEW r \in ChatRows(c2)', r.t \in {"out", "msg"}
                 PROVE r \in rep'[InitHost[c2]][c2]
      BY DEF OutAtHost
    <3>1. InitHost[c2] \in Dev BY InitHostType
    <3>2. CASE r \in ChatRows(c2) BY <3>1, <3>2, <2>e3 DEF OutAtHost
    <3>3. CASE r \notin ChatRows(c2) BY <3>3, <2>e3, <2>e1
    <3>. QED BY <3>2, <3>3
  <2>c4. MsgAtTarget' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>23, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Resolve, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd' BY <2>e0, <2>e2, <2>4, OWRep DEF Out
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>24. CASE ExpireA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ Expire(hh, r.id, r.m) BY <1>24 DEF ExpireA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ Expire(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>t. TypeOKP' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(20) DEF Expire, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>24, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF Expire, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "expired")}] /\ edge' = edge
          /\ Cmd(rr.id, rr.m) \in rep[hh][cc]
      BY <2>2, <2>4 DEF Expire
    <3>2. Out(rr.id, "expired") \in RowT BY <2>4, RowTypes DEF RowT
    <3>. QED BY <3>1, <3>2, <2>4, OWRep DEF Out
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>25. CASE SweepA
  BY <1>25 DEF SweepA, Sweep
<1>26. CASE RecoverDoneA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ RecoverDone(hh, r.id, r.m) BY <1>26 DEF RecoverDoneA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ RecoverDone(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>e0. /\ Cmd(rr.id, rr.m) \in rep[hh][cc]
         /\ rep' = [rep EXCEPT ![hh][cc] = @ \cup {Out(rr.id, "applied")}] /\ edge' = edge
    BY <2>2, <2>4 DEF RecoverDone
  <2>e2. Out(rr.id, "applied") \in RowT BY <2>4, RowTypes DEF RowT
  <2>e3. \A c2 \in Chats : ChatRows(c2)' = IF c2 = cc THEN ChatRows(c2) \cup {Out(rr.id, "applied")} ELSE ChatRows(c2)
    BY <2>e0, <2>e2, CRStep
  <2>e4. \A c2 \in Chats : \A r \in ChatRows(c2)' : r.t # "out" => r \in ChatRows(c2)
    BY <2>e3 DEF Out
  <2>t. TypeOKP' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(20) DEF RecoverDone, ReplayDue, RanBefore, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <2>e4 DEF IdUnique
  <2>c2. NetOK' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, ExecNotRun, OpenUnique, OpenMid, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, OutAtHost, DispatchDoc, MsgAtTarget, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c11. ExecByHost' BY <1>26, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverDone, ReplayDue, RanBefore, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd' BY <2>e0, <2>e2, <2>4, OWRep DEF Out
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
<1>27. CASE RecoverRedoA
  <2>1. PICK hh \in Dev, cc \in Chats : \E r \in rep[hh][cc] : r.t = "cmd" /\ RecoverRedo(hh, r.id, r.m) BY <1>27 DEF RecoverRedoA
  <2>2. PICK rr \in rep[hh][cc] : rr.t = "cmd" /\ RecoverRedo(hh, rr.id, rr.m) BY <2>1
  <2>3. rr \in ChatRows(cc) BY <2>1, <2>2 DEF ChatRows
  <2>4. rr \in RowT /\ rr.m \in Mids /\ Target(rr.m) = cc BY <2>1, <2>2, <2>3 DEF TypeOKP, CmdShape
  <2>t. TypeOKP' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(20) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, TypeOKP, RowT, IdT, MsgT, Cmd, Out, MsgRow, DoneRow, NoId, ResId, Mids, Chats, Target
  <2>c0. CmdShape' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c1. IdUnique' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, IdUnique, CmdShape, ProcAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c2. NetOK' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, NetOK, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c3. OutAtHost' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, OutAtHost, LiveIsOpen, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c4. MsgAtTarget' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, MsgAtTarget, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c5. DispatchDoc' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, DispatchDoc, ExecNotRun, LiveIsOpen, OpenUnique, MsgAtTarget, OutAtHost, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c6. ProcAtHost' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, ProcAtHost, CmdShape, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c7. LiveIsOpen' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, LiveIsOpen, ProcAtHost, OutAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c8. OpenUnique' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, OpenUnique, OpenMid, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c9. OpenMid' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, OpenMid, OpenUnique, ProcAtHost, LiveIsOpen, CmdShape, IdUnique, OutAtHost, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c10. ExecNotRun'
    <3>1. /\ Cmd(rr.id, rr.m) \in rep[hh][cc] /\ rr.id \in processed[hh] /\ ~HasOut(rep[hh][cc], rr.id)
          /\ rr.id \notin executing[hh]
          /\ rep' = [rep EXCEPT ![hh][cc] = @ \cup {MsgRow(rr.m), Out(rr.id, "applied")}] /\ edge' = edge
          /\ executing' = executing
      BY <2>2, <2>4 DEF RecoverRedo, ReplayDue
    <3>2. {MsgRow(rr.m), Out(rr.id, "applied")} \subseteq RowT BY <2>4, RowTypes DEF RowT
    <3>3. \A d2 \in Dev, c2 \in Chats : rep'[d2][c2] = IF d2 = hh /\ c2 = cc THEN rep[d2][c2] \cup {MsgRow(rr.m), Out(rr.id, "applied")} ELSE rep[d2][c2]
      BY <3>1, <3>2, CRStep
    <3> SUFFICES ASSUME NEW d2 \in Dev, NEW m2 \in Mids, NEW j \in executing'[d2],
                        Cmd(j, m2) \in rep'[d2][Target(m2)], MsgRow(m2) \in rep'[d2][Target(m2)]
                 PROVE FALSE
      BY DEF ExecNotRun
    <3>4. Target(m2) \in Chats /\ j \in executing[d2] BY <3>1 DEF Target, Chats
    <3>5. Cmd(j, m2) \in rep[d2][Target(m2)] BY <3>3, <3>4 DEF Cmd, MsgRow, Out
    <3>6. CASE MsgRow(m2) \in rep[d2][Target(m2)] BY <3>4, <3>5, <3>6 DEF ExecNotRun
    <3>7. CASE MsgRow(m2) \notin rep[d2][Target(m2)]
      <4>1. d2 = hh /\ Target(m2) = cc /\ m2 = rr.m BY <3>3, <3>4, <3>7 DEF MsgRow, Out
      <4>2. j \in processed[hh] /\ \A c2 \in Chats : ~HasOut(rep[hh][c2], j) BY <3>4, <4>1 DEF LiveIsOpen
      <4>3. Open(hh, j, rr.m) /\ Open(hh, rr.id, rr.m) BY <4>1, <4>2, <3>5, <3>1, <2>4 DEF Open
      <4>4. j = rr.id BY <4>3, <4>2, <3>1, <2>4 DEF OpenUnique
      <4>. QED BY <4>4, <4>1, <3>4, <3>1
    <3>. QED BY <3>6, <3>7
  <2>c11. ExecByHost' BY <1>27, <2>1, <2>2, <2>3, <2>4, Gates, InitHostType, SMTT(40) DEF RecoverRedo, ReplayDue, RanBefore, DispatchEff, Cap2, ExecByHost, LiveIsOpen, ProcAtHost, CmdShape, IdUnique, TypeOKP, ChatRows, Open, HasOut, H, Cmd, Out, MsgRow, DoneRow, Target, Chats, Mids, NoId, ResId
  <2>c12. OutWithCmd'
    <3>1. rep' = [rep EXCEPT ![hh][cc] = @ \cup {MsgRow(rr.m), Out(rr.id, "applied")}] /\ edge' = edge
          /\ Cmd(rr.id, rr.m) \in rep[hh][cc]
      BY <2>2, <2>4 DEF RecoverRedo
    <3>2. {MsgRow(rr.m), Out(rr.id, "applied")} \subseteq RowT BY <2>4, RowTypes DEF RowT
    <3>3. \A r \in {MsgRow(rr.m), Out(rr.id, "applied")} : r.t = "out" =>
             \E mm \in Mids : Target(mm) = cc /\ Cmd(r.id, mm) \in rep[hh][cc] \cup {MsgRow(rr.m), Out(rr.id, "applied")}
      BY <3>1, <2>4 DEF MsgRow, Out
    <3>. QED BY <3>1, <3>2, <3>3, OWRep
  <2>. QED BY <2>t, <2>c0, <2>c1, <2>c2, <2>c3, <2>c4, <2>c5, <2>c6, <2>c7, <2>c8, <2>c9, <2>c10, <2>c11, <2>c12 DEF IndInv
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

THEOREM S9Safe == Spec => []S9
<1>1. Init => IndInv BY InitInv
<1>2. IndInv /\ [Next]_vars => IndInv' BY StepInv
<1>3. IndInv => S9 BY InvS9
<1>. QED BY <1>1, <1>2, <1>3, PTL DEF Spec
=============================================================================
