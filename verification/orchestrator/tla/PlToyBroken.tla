---- MODULE PlToyBroken ----
\* Toy TLAPS check for the tlapm install: two counters, y never passes x.
EXTENDS Naturals, TLAPS
VARIABLES x, y
vars == <<x, y>>

Init == x = 0 /\ y = 0
IncX == x' = x + 1 /\ y' = y
IncY == y' = y + 1 /\ x' = x
Next == IncX \/ IncY
Spec == Init /\ [][Next]_vars

TypeOK == x \in Nat /\ y \in Nat
Safe == y <= x
IndInv == TypeOK /\ Safe

THEOREM InitInv == Init => IndInv
  BY DEF Init, IndInv, TypeOK, Safe

THEOREM StepInv == IndInv /\ [Next]_vars => IndInv'
  BY DEF IndInv, TypeOK, Safe, Next, IncX, IncY, vars

THEOREM SpecSafe == Spec => []Safe
<1>1. Init => IndInv BY InitInv
<1>2. IndInv /\ [Next]_vars => IndInv' BY StepInv
<1>3. IndInv => Safe BY DEF IndInv
<1>. QED BY <1>1, <1>2, <1>3, PTL DEF Spec
====
