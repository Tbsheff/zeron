# Try the isolated build

Build this checkout after its checks pass. Keep the installed app on its current
engine. The commands below use staging, a separate profile and port 27872.

```sh
ZERON_ORCH_V2=1 ORCH_CAPTURE_MECHANISM=1 \
ZERON_DEV_DATA_DIR="$HOME/.zeron-orch-v2" \
ZERON_DEV_IPC_PORT=27872 ZERON_DEV_BUNDLE_ID=sh.zeron.app.orch-v2 \
ZERON_DEV_BUILD_ONLY=1 scripts/staging-app.sh
```

Remove `ZERON_DEV_BUILD_ONLY=1` to open the bundle. Sign in to staging in that
window. The staging sign-in callback uses port 27642. Close another staging dev
app before using that callback. The build script checks for its own running app
before it replaces the bundle.

Use a test project with no required work in it. Start with these cases:

- **Delegate work:** create one wait task and one async task. Check each result
  in the parent chat and through `task_status`.
- **Route questions:** ask a child to delegate once more, then ask a question
  from the grandchild. Answer it in the parent chat and the child chat.
- **Stop work:** leave a native background task running after the root turn
  ends. Press Stop. Check that the task and its saved transcript both end.
- **Move context:** fork a chat, continue it, then merge its result back. Switch
  agents and resume. Check that earlier user inputs remain present once.
- **Recover work:** restart this dev app during a task. Check that a saved
  result is delivered once and the same input does not start twice.

Native tasks need a provider that reports its own subagent events. Cross-device
use needs a second device running the same v2 build and its own isolated
profile. Local tests with provider stand-ins do not prove either connection.

The local profile stores its database under
`~/.zeron-orch-v2/profiles/local/docs.sqlite3`. A signed-in profile stores it
under `~/.zeron-orch-v2/orgs/<org>/<user>/docs.sqlite3`. Use the profile that owns
the test chats. Do not copy only a live SQLite main file: its WAL may contain
the latest steps. Use SQLite backup, or the capture command's read transaction.

Keep raw captures private. Mark automated trials as `trial`. Mark `dogfood`
only after actual use. Repeated copies of one trace do not count as new use.
The release check requires seven elapsed days with fresh steps, current proofs,
and full model conformance. Until those checks pass, keep v2 off by default.

The build must supply a source-bound provenance receipt and the mapping context
for the actual profile. Set `ORCH_USE_PROVENANCE`, `ORCH_USE_CONTEXT` and
`ORCH_USE_DB` to those files. Capture actual use with:

```sh
python3 scripts/orch-dogfood.py capture --usage dogfood \
  --db "${ORCH_USE_DB:?set the isolated profile database}" \
  --provenance "${ORCH_USE_PROVENANCE:?set the build receipt}" \
  --context "${ORCH_USE_CONTEXT:?set the mapping context}" \
  --out "$HOME/.local/share/zeron-orch-evidence/dogfood"
```

Missing or changed proof inputs fail the check. The capture command does not
accept a stored success flag as proof of conformance.
