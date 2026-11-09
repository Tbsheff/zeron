# Check physical commits

Keep `origin.commit` as the unique row ID. Check `origin.transaction` as the ID of the first local row in the physical SQLite transaction. The source writes both fields in the same transaction as the captured state. A complete, source-bound export is required; these fields are not independent evidence against a forged export.

Reject missing IDs in strict mode. Reject mixed old/new rows, a different device, a noncanonical sequence, a group that starts before or after its first row, and a closed group that appears again. Historical all-missing captures remain component input only and report `physical_transactions=false`.

Sort the DAG by physical groups. Attach each external edge to the entire group, then expand all internal rows in local order with no external node between them. Reject a cycle after collapse: an external write cannot depend on an uncommitted intent and then be observed inside that same commit.

Keep every raw row for exact Core/Tx/effect checks. Set `row.transaction` to `{id,index,size,visible,known}`. Only the last row has `visible=true`. Keep `step.parts` as the internal proof state; update `step.committed_parts` only at a group end. Readers must not use an internal proof state as an externally visible pending window. Returned current rows also expose `committed_partitions`; retained partition records do not embed that history. For an internal implementation image, use its own raw partition plus the committed partitions of other devices. This permits checking the real internal reducer steps without inventing a separate visible commit. Source maps must retain the physical ID and internal index for every target macro-step.

The strict full entry point requires transaction IDs. This change alone does not enable the full gate. Main reader/source-map integration, target review, exact raw prerequisites and current source/model provenance remain required. Old receipts and the first source package stay unchanged.
