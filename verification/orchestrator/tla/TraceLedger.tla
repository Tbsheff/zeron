---- MODULE TraceLedger ----
EXTENDS Naturals, Sequences, FiniteSets
CONSTANT Rows
VARIABLES cursor, receipts, posts, valid
vars == <<cursor, receipts, posts, valid>>
Devices == {Rows[n].dev : n \in 1..Len(Rows)}
Init == /\ cursor = 1
        /\ receipts = {}
        /\ posts = [d \in Devices |-> ""]
        /\ valid = TRUE
Next == /\ cursor <= Len(Rows)
        /\ LET row == Rows[cursor]
               key == <<row.dev, row.key>>
               initial == row.act = "Init"
               permitted == IF initial THEN posts[row.dev] = ""
                            ELSE IF row.dup THEN key \in receipts /\ row.post = posts[row.dev] /\ row.fx = <<>>
                            ELSE key \notin receipts
           IN /\ valid' = (valid /\ permitted)
              /\ receipts' = IF initial \/ row.dup THEN receipts ELSE receipts \cup {key}
              /\ posts' = [posts EXCEPT ![row.dev] = row.post]
        /\ cursor' = cursor + 1
Spec == Init /\ [][Next]_vars
LedgerStepOK == valid
====
