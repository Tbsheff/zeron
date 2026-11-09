--------------------------- MODULE CompositionStub ---------------------------
(***************************************************************************)
(* Proves the two modules compose: both are INSTANCEd over ONE copy of the *)
(* RunBase variables (queue, held, liveSet, rord, intReq). Name clashes    *)
(* (Stop, MaxPerson, personLeft, ...) stay behind the RL! and D! prefixes. *)
(* Composition (AC-M1) will replace Next: each RL action that ends a run   *)
(* conjoins D's run-end hook (sweep, notice), and each D action leaves the *)
(* RL variables unchanged. This stub only parses and evaluates Init.       *)
(***************************************************************************)
EXTENDS RunBase

CONSTANTS
  RLMaxPerson, RLMaxServer, RLMaxRetry, RLMaxStop, RLMaxUL, RLMaxCrash, RLMaxAtt, FixLimitRelease,
  MaxDepth, MaxFanout, MaxGen, DMaxPerson, MaxFail, DMaxCrash, MaxAbandon, DMaxStop, MaxNarrow, MaxInterrupt,
  DupDelivery, Crash, LateAck, Questions, FixWakeSkipsFailHold, StopCheckAnc, PartialCrash, QSplit, MaxFollowUp

VARIABLES
  att, why, emitted, kidOpen, pev, retries, steers, lastOut, rkind, ulArmed, ulDue,
  rPersonLeft, serverLeft, rStopLeft, ulLeft, rCrashLeft,
  created, parent, depth, access, rtask, waitOn, zWait,
  tSt, tWake, tDel, tCoh, pushes, kidIn,
  qPend, qAt, qVis, qWake, qUsed, answers, qEp,
  cDisp, cSt, cDel, cGen, outbox, replayable, rcpt, replayBad, receiptBad, doomed,
  dPersonLeft, failLeft, dCrashLeft, abandonLeft, dStopLeft, narrowLeft, interruptLeft

RL == INSTANCE RunLifecycle WITH
        MaxPerson <- RLMaxPerson, MaxServer <- RLMaxServer, MaxRetry <- RLMaxRetry,
        MaxStop <- RLMaxStop, MaxUL <- RLMaxUL, MaxCrash <- RLMaxCrash, MaxAtt <- RLMaxAtt,
        personLeft <- rPersonLeft, stopLeft <- rStopLeft, crashLeft <- rCrashLeft

D == INSTANCE Delegation WITH
        MaxPerson <- DMaxPerson, MaxCrash <- DMaxCrash, MaxStop <- DMaxStop,
        personLeft <- dPersonLeft, crashLeft <- dCrashLeft, stopLeft <- dStopLeft

allVars == <<base, att, why, emitted, kidOpen, pev, retries, steers, lastOut, rkind, ulArmed, ulDue,
             rPersonLeft, serverLeft, rStopLeft, ulLeft, rCrashLeft,
             created, parent, depth, access, rtask, waitOn, zWait, tSt, tWake, tDel, tCoh, pushes, kidIn,
             qPend, qAt, qVis, qWake, qUsed, answers, qEp, cDisp, cSt, cDel, cGen,
             outbox, replayable, rcpt, replayBad, receiptBad, doomed,
             dPersonLeft, failLeft, dCrashLeft, abandonLeft, dStopLeft, narrowLeft, interruptLeft>>

Init == RL!RL_Init /\ D!D_Init
Next == UNCHANGED allVars
Spec == Init /\ [][Next]_allVars

SharedBaseOK == RL!RL_TypeOK /\ D!D_TypeOK /\ S1_AtMostOneActiveRun

=============================================================================
