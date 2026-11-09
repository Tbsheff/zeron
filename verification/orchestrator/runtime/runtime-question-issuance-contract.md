# Check R50 question issuance

`runtime_question_issuance.question_issuance(raw_rows)` checks raw source evidence before full projection. It returns private receipt links. It raises `QuestionIssuanceError(row, field)` with no private values when a link is missing or false. It does not read future rows or search target successors.

- Join an accepted nonduplicate raw `Raise` to its exact stored accepted receipt.
- Join that receipt and raw child/qid to one prior `question_input_route` fact.
- Join the route request/run/attempt to one prior `question_input_issued` fact for the same chat.
- Resolve the raw route child with the same device's captured thread binding.
- Check the source run/attempt against the prior live core row.
- Keep the issued runtime and message IDs exact. Reject null IDs. Do not infer a hidden provider turn ID.
- Skip rejected and duplicate input claims. They do not create a new question.

The source records issuance synchronously before it inserts the resolver and sends `InputRequested` (`sessions.rs:94`). The source records route before `apply_if`; `apply_if` checks the captured attempt against the current thread (`orchestrator/questions.rs:34`). The route fact proves an attempted route only. A capture-write error can leave a real product question without the fact, so missing facts block this check.

The generic typed fact map retains the route's numeric local child. This helper needs raw snapshots and device-local thread bindings. It must not treat that number as a stable chat name.

Nine focused tests pass. The older `capture-final` set has 58 completed groups: 57 have no accepted question raises; one blocks on missing route identity. `question-issuance-old-capture-stage.json` records this scope and exact helper hashes. This helper is not part of the frozen common-target receipt. Fresh R50 data and main-entrypoint integration remain required.
