# Zeron MCP server

`zeron mcp` serves the Model Context Protocol on stdin/stdout and proxies every
tool into the running engine's localhost IPC (`ws://127.0.0.1:$ZERON_IPC_PORT`,
default 27654) — the same `zeron_rpc` surface the headed app and `zeron sync`
dial. It is a subcommand of the one `zeron` binary: no Node runtime, no extra
install, a few MB resident.

Crate: `crates/mcp` (`zeron-mcp`). The protocol layer is hand-rolled
(`initialize`, `ping`, `tools/list`, `tools/call`; newline-delimited JSON-RPC
2.0) — the repo already owns JSON-RPC framing for the Codex and ACP drivers and
the stdio tool-server subset is tiny, so no SDK dependency was taken. It
accepts protocol `2025-11-25` and `2025-06-18` (older revisions are echoed
too) and answers `server/discover` and any other unknown method with `-32601`
at once, so a client that probes the 2026 stateless draft first falls back
without a wait.

## Identity

The engine can inject this server into a harness's MCP config with the
originating chat in the environment:

| Variable                | Meaning                                                  |
| ----------------------- | -------------------------------------------------------- |
| `ZERON_IPC_PORT`        | Engine to proxy (default 27654).                         |
| `ZERON_IPC_TOKEN_FILE`  | The engine's IPC bearer file (see below).                |
| `ZERON_CHAT_ID`         | The chat whose agent spawned this server.                |
| `ZERON_DEVICE_ID`       | That chat's host device.                                 |
| `ZERON_SIDE_CHATS_ONLY` | `1`: the agent has no subagent tools of its own (below). |
| `ZERON_WAITS_ON_BACKGROUND` | `1`: the runtime wakes the agent as background work ends; `wait_on_background` is offered. |
| `ZERON_PLUGIN_INSTRUCTIONS` | Path of the run's plugin instructions (step 4). |
| `ZERON_MCP_GUIDANCE`    | `prompt`: the harness put Zeron's guidance in the system prompt (below). |

`ZERON_SIDE_CHATS_ONLY=1`: the host device turned the provider's native
subagents off (Settings → Providers), so the guidance tells the agent to
delegate with `create_chats`. Never set for a chat that has a parent, nor
for one whose host doesn't know its parent yet.

### IPC authentication

The IPC port is loopback-only, but loopback is not an identity: any local
process could otherwise drive the engine (start agents, broker Cloud vault
grants). Every engine start that binds the port mints a fresh 32-byte token
and writes it to `{data_dir}/ipc-token` (owner-only `0600`, atomic replace;
on Windows the file inherits `%LOCALAPPDATA%\Zeron`'s per-user ACL). The
WebSocket handshake must carry `Authorization: Bearer <token>` (compared in
constant time) and is refused with `401` otherwise; the browser `Origin`
rejection (`403`) still applies on top. In-process (in-memory) transports are
unaffected.

Clients resolve the token at every dial — `$ZERON_IPC_TOKEN`, then the file
named by `$ZERON_IPC_TOKEN_FILE`, then `{ZERON_DATA_DIR or ~/.zeron}/ipc-token`
(`zeron_rpc::connect_ipc`) — so they follow the rotation across engine
restarts. The engine hands injected servers the file PATH
(`ZERON_IPC_TOKEN_FILE`), never the token itself: Claude receives this env on
its command line (`--mcp-config <inline json>`), which other local users can
read with `ps`. A client run against a different data dir (e.g. a viewport on
a daemon's port) sets `ZERON_IPC_TOKEN_FILE` to the daemon's file.

When `ZERON_CHAT_ID` is set, every message `message_chats` sends is prefixed with a
`[Message from Zeron chat <title> (<id8>) …]` line so the receiving agent and the
human reading that transcript can tell an agent-to-agent message from a typed
one, and the server refuses to message its own chat. The transcript renders
this routing header as “Message from **chat name**”, keeping the full routing
instructions in the stored prompt for agents.

### Parent links

Each request in `create_chats` accepts `kind`:

| Input | Placement |
| --- | --- |
| `kind: "chat"` | Standalone session, visible in the left **Sessions** sidebar; no `parentChatId` is written. |
| `kind: "side"` | Child of the explicit `parent` or origin chat. Requires one of those. |
| Kind omitted, origin or parent supplied | Child, preserving the previous default. |
| Kind omitted, no origin or parent | Standalone, preserving terminal usage. |

`kind: "chat"` with a nonempty `parent`, `kind: "side"` without an origin/parent,
and unknown kinds fail before any writes. Empty `parent` uses the default.
The creation result includes the effective `kind`, `chatId`, `deviceId`,
`project` and `parentChatId`. Kind is derived from the parent; it is not persisted.

Children record `Chat.parent_chat_id` (proto) ⇄ `parentChatId` on the registry
row (`Mutate createChat { parentChatId? }`). Any chat may delegate to any provider.
Delegation is one level deep: a chat that has a parent creates no chats at all,
and a new chat's parent (the origin, or an explicit `parent`) must be top-level.
Standalone sessions have no parent.

`cloud: true` runs the child on a Cloud machine of its own (Codex or Claude
Code), cloning the project's GitHub repository; from a Cloud chat, the parent's
repository. Without it, a child of a Cloud chat shares the parent's machine. A
Cloud machine starts children only for the chat it runs, and sees and manages
only those children (docs/design/cloud-device.md, "Delegation").

For children, `parent` (id, prefix, or title) overrides the origin (`ZERON_CHAT_ID`).
`list_chats { parent }` returns children, and chat summaries carry `parentChatId`.
Rows from older engines read as parentless; a dangling parent id is tolerated.

The left sidebar hides chats that have a parent: `AppState::visible_chats`
(the Sessions list, project tabs, jump slots) and the Archived section both
require `parent_chat_id == None`. Children remain addressable by id, deep
link, and every MCP tool; `list_chats { parent }` is how an orchestrator
finds them.

### Injection

The host engine stamps this server onto every run it drives
(`RunRequest.mcp`, additive): the same `zeron` binary with `args: ["mcp"]`
and the variables above, pointed at the port the engine itself serves
(never a port it lost the bind race for). Each driver spells it in its own
dialect and leaves the user's configured servers alone:

| Harness | Where |
| ------- | ----- |
| Claude  | `--mcp-config <inline json>` (no `--strict-mcp-config`)             |
| ACP (Devin, Grok, Hermes, Antigravity) | `session/new` and `session/load` → `mcpServers: [{name, command, args, env}]` |
| Pi | Per-run `--extension` bridges stdio MCP into Pi tools in the native RPC process |
| OpenCode | Child-only `OPENCODE_CONFIG_CONTENT`: `mcp.zeron` on 1.x, `mcp.servers.zeron` on 2.x |
| Codex   | `thread/start` config overrides `mcp_servers.zeron.{command,args,env}` |
| Cursor  | SDK `Agent.create` / `Agent.resume` → inline `mcpServers.zeron`, plus `local.settingSources: ["user", "team", "mdm", "plugins"]` so `~/.cursor/mcp.json` and plugin servers load (not `project`: the SDK skips MCP approvals, so repo-defined servers would run unprompted) |

OpenCode preserves inherited inline configuration and other servers. Its config
shape follows the installed binary's major version. Cursor uses the SDK's
[inline MCP configuration](https://cursor.com/docs/sdk/typescript); OpenCode's
[1.x config layer](https://opencode.ai/docs/config/) and
[2.x MCP format](https://opencode.ai/v2/docs/mcp-servers) differ.
Pi's temporary extension lives only for the native RPC process lifetime;
user settings, extensions, and session arguments remain intact. The bridge
registers `zeron_<tool>` tools, propagates cancellation and errors, and closes
the MCP child when the Pi session shuts down.

Title runs never carry it. A run with no served port (embedded engine that
lost the bind) gets no Zeron tools rather than a dead server.

### Guidance

What tells an agent when to use these tools lives in the agent's system
prompt, not in the MCP `instructions` (Claude Code cuts those at 4 KB, Codex
shows only the first ~233 chars). `zeron_proto::agent_guidance` builds it from
sections that match the run: core (always; opens "Ordinary coding tasks need
no Zeron tools.", then a task → tool map and five rules), waits
(`wait_on_background` offered), delegated tasks (`ZERON_ORCH_V2` on, in a
chat), side chats (own subagents off), visuals (in a chat). Plugin instructions
follow it.

| Harness | Where the guidance goes |
| ------- | ----------------------- |
| Claude | `--append-system-prompt` (sections); plugin text via `--append-system-prompt-file` |
| Codex | `turn/start` `additionalContext`: one `application` entry per section, plugin text split, each < 4,000 B, keys `zeron_01`… |
| OpenCode 1.x | the prompt body's `system` field on every prompt |
| ACP, OpenCode 2.x, Pi, Cursor, a person's own client | the server's `instructions` |

A harness that delivers it sets `ZERON_MCP_GUIDANCE=prompt` on the server, and
`initialize` answers with `MCP_INSTRUCTIONS` only (under 230 chars, so Codex's
one-line summary is complete). Otherwise `instructions` is that line plus the
sections and plugin text. Claude chats also get the `zeron:visuals` skill (the
full `show_visual` reference) through `--plugin-dir`.

Known gap: Codex drops client context on compaction and resends
`additionalContext` only when it changes, so a long Codex thread can lose the
guidance after compaction until the next run starts.

### Forks and the session card

`ForkSideChat { chatId, sourceChatId, parentChatId? }` copies a chat's
history through its latest completed response into a new chat and appends
a `fork` part (a system entry: `sourceChatId`, `sourceTitle`) as the seam;
the transcript draws it as "This chat was forked from <title>". The copy
settles what only the source's runtime could finish: open questions copy
answered, and a spawn chip still `running` in the source copies `failed`
(its `subagentRef` intact). `parentChatId` defaults to the source; a side
chat's own fork button passes its parent so the copy lists as a sibling. The lanes card, under the session card, draws
a chat as a main line with its **side chats** (its children: forks and
`kind: "side"` spawns) and **subagents** (its spawn chips) branching off it,
and opens either in the right pane.

## Tools

Chat status uses the same shared derivation as the sidebar, including OSC 7501
reports from chat-owned terminals. `status` is `working`, `awaitingInput`,
`completed` (an unseen result), `errored`, or `idle`; `sessionStatus` retains the
native execution state. `statusStale` distinguishes expired live reports, and
`terminalActivity` identifies a terminal's input reason or result. Native waits
can return `interrupted`; a lost host is never interpreted as successful
completion. See [agent and terminal status](program-status.md).

Chats are referenced by full id, a unique id prefix, or an exact title.
Projects by id, path, display name, or unique path suffix. Devices by id or
name (default: the local engine's device).

| Tool | Engine calls |
| ---- | ------------ |
| `catalog {kind}` | `self`: `LocalDevice`, `EngineInfo`, origin chat; `devices`: `WatchDevices`; `projects`: `WatchSpaces`; `harnesses`: `ListHarnesses` + `ListAgentAccounts`; `models`: `ListModels` (`targetDeviceId?` from `device`) |
| `list_chats` | `WatchChats` + `WatchSessions` snapshots (status merged) |
| `read_chat` | above + `WatchDocMessages` opening frame: chat summary, `pendingInput`, newest messages (`limit: 0` is status only) |
| `create_chats` | 1–32 × `Mutate createChat` (+ `renameChat`; optional first send), concurrent |
| `message_chats` | 1–32 × `QueueCommand` Run / Steer, or `QueueMessage`, concurrent |
| `wait_for_turn` | `WatchSessions` until the chat settles |
| `interrupt_chat` | `QueueCommand` Interrupt |
| `respond_to_input` | `QueueCommand` RespondInput |
| `archive_chat` | `Mutate setChatArchived` |
| `show_visual` | `PublishVisual {chatId, title, script, assets}` (origin chat only; assets base64 by name, read by the MCP server from the agent's cwd) |
| `show_image` | `PublishImage {chatId, name, data}` (origin chat only; the file base64, read by the MCP server from the agent's cwd) |
| `visual_reference {topic}` | none: the full `show_visual` API by topic (origin chat only) |
| `wait_on_background` | `AwaitBackground` (origin chat on a runtime that wakes it) |
| `plugins {action}` | list, status, elements, updates, create, install → `ListPlugins`, `PluginStatus`, `DescribeElements`, `CheckPluginUpdates`, `NewPlugin`, `InstallPlugin` |
| `templates {action}` | list, status, app, create, use, delete, export, import → the engine's template methods |
| `themes {action}` | list, get, create, update (`remove: ["light"]` drops a scheme), delete, use → the engine's theme methods |

Every tool has a `title` and the four annotations (`readOnlyHint`,
`destructiveHint`, `idempotentHint`, `openWorldHint`). Only
`wait_on_background` carries `_meta["anthropic/alwaysLoad"]`: the guidance
requires it at turn end, and Claude Code defers every other tool. Input
schemas are compacted when listed (no `additionalProperties: true`,
`$schema`, null type branches; const-tagged unions flattened). A call to a
removed name (`create_chat`, `send_messages`, `list_devices`, `create_theme`,
…) fails with a message naming its replacement. `message_chats` is not called
`send_messages` because Claude Code has a built-in `SendMessage` tool for its
own teammates, and Sonnet picked it over the Zeron tool in evals.

Results are `structuredContent` with a compact JSON text copy. `catalog`,
`list_chats`, `read_chat`, `create_chats`, `message_chats` and `wait_for_turn`
declare an `outputSchema`. Where agents tend to poll, a result carries `next`
(after an async create or send: collect with `wait_for_turn`, do not poll
`read_chat`). Errors are `isError: true` with `{code, message}` in both
`structuredContent` and text; codes are `invalid_arguments` (with a valid
example call), `not_found` (with "Did you mean …" for chats), `ambiguous`,
`rejected`, `engine_unavailable`, `unknown_tool`. `"me"` names the origin chat
(`list_chats {parent: "me"}`). A `clientRequestId` on a create or send request
returns the first result again on retry (per server connection) instead of a
second write.

### Delegated tasks (`ZERON_ORCH_V2`)

With `ZERON_ORCH_V2=1` (`true`, `on`) in the server's env and an origin chat,
three more tools follow T3 Code's orchestrator shapes. With the flag off the
list is the tools above, byte for byte.

| Tool | Input | Engine call |
| ---- | ----- | ----------- |
| `delegate_task` | `task` (≤ 120,000 B), `target? {harness, model, project, device}`, `title?` (≤ 512), `role?` (implementation, research, review, design, test, general), `isolation?` (shared, worktree), `mode?` (async, wait), `timeoutMs?` (wait only; default 600,000, max 3,600,000), `clientRequestId?` (≤ 256) | `OrchDelegate {originChatId, task, title?, role?, mode, timeoutMs?, clientRequestId?, child: {chatId, deviceId, spaceId?, config, cwd?, worktree?}}` |
| `task_status` | `taskId` | `OrchTaskStatus {originChatId, taskId}` |
| `task_cancel` | `taskId`, `reason?` (≤ 2,000), `clientRequestId?` | `OrchTaskCancel {originChatId, taskId, reason?, clientRequestId?}` |

`delegate_task` resolves `target` like `create_chats` (project and device,
default and available harness, listed model, access and sandbox no more than
the origin chat's, a scratch cwd for a projectless child) but writes no chat:
the engine creates the child row (parent = origin, kind subagent). The
one-level rule of `create_chats` does not apply; the engine caps depth at 2 and
unsettled children at 8. `isolation: "worktree"` sends `child.worktree
{repoPath, base: "HEAD", spaceId}` and needs a git project; the child's first
run creates the checkout.

`delegate_task` and `task_status` return the engine's task object unchanged:
`{taskId, childThreadId, status, summary, hasPendingQuestion, waitTimedOut,
next?}`. `next`, on a running task whose completion wakes the parent, says to
end the turn. `task_cancel` returns `{taskId, status}` (`cancel_requested` or
the settled status). An engine failure `"<code>: <message>"` becomes the tool
error `{code, message}` with T3's code (`capability_denied`,
`parent_not_active`, `task_not_found`, `task_not_cancellable`,
`invalid_request`, …; anything else `orchestration_error`) and a one-line fix.

Watch streams are the engine's only read surface (there is no one-shot "get
transcript" RPC); a snapshot is "subscribe, take the first item, drop" — drop
cancels server-side, exactly what the sidebar does on attach.

`message_chats` mode `auto` starts idle chats and steers working chats through
their live mailbox without interrupting tools or child processes. Providers
consume it at their next supported input boundary. Only explicit `queue` mode
creates a held queue row. `awaitingInput` refuses and points at
`respond_to_input`. Quiet, long-running turns still receive steering; the host
falls back to starting a turn if the live runtime has already exited.

Claude uses `priority: "next"` and confirms consumption through replayed user
messages. Cursor uses the SDK's native `Run.steer`, waits for active tools to
finish, and retains input until its delivery acknowledgment. New injections do
not wait for earlier delivery acknowledgments; that wait serializes responses
even when the SDK reports a single active turn. These are mid-turn
paths; they do not terminate the harness process. Adapters that only accept input
between turns are labeled **Send next** in the composer.

The opt-in `steering_live` engine test checks a rapid burst, foreground and
background process survival, retained context, exactly-once effects, and ordered
normal queue delivery. For Claude, Cursor and Codex it additionally requires the
burst to finish within the original turn. Set `ZERON_TEST_BURST=6` to reproduce a
six-message burst; select an inexpensive model with `ZERON_TEST_MODEL` and the
harness with `ZERON_TEST_HARNESS`. Codex is checked for child survival during
the active tool: its runtime cleans up background jobs on normal tool completion
even without steering. Other providers also check a job that outlives that tool.

For conversational redirection, run `cargo run -p zeron-harness --example
cursor_steering_probe -- gemini-3-flash`. It sends six bare digits during streamed
prose and requires only the latest requested answer. Counting turn completions
alone cannot distinguish real steering from serial responses inside one run.

`wait_for_turn` after a send is edge-triggered on the `Session` row captured
before the send: it returns on a new `last_completed_turn`, an
`awaitingInput`/`errored` stamp newer than the baseline, or a working→idle
edge. A brand-new chat has no session row until the host picks the run up, so
the wait keeps waiting in that case rather than reporting the unstarted run as
done (this was the one bug the first live run found).

### Visual replies

`show_visual` is listed only when `ZERON_CHAT_ID` is set: it shows an
interactive visual (chart, table, dashboard, diagram, simulation, 3D view)
inline in the calling chat, above the agent's final reply. A visual is a Luau
script, not a web page. The script builds its data and state once and returns a
view function; the view returns a tree of `ui.*` nodes (rows, columns, grids,
text, stats, tables, charts, tabs, toggles, sliders, buttons, links, 2D
canvases, 3D scenes, and GPU shaders). Controls call the script's handlers, and Zeron renders
the view again.

`crates/visual` (`zeron-visual`) is the runtime. Scripts run in a Luau sandbox
with only the `string`, `table`, `math`, `utf8` and `bit32` libraries (no I/O,
network, or `require`), a 64 MB memory cap, and a time budget per call (250 ms
for setup, renders and handlers, 60 ms per canvas or scene frame), so a runaway
script fails with an error instead of freezing the app. `math.random` is
reseeded from the visual's id before every call, so random data is identical on
every render and device. The style rules are the API rather than advice: colors
are theme tokens (`text`, `muted`, `faint`, `accent`, `on-accent`, `success`,
`warning`, `danger`, `border`, `chart1`…`chart6`), text sizes are steps on the
transcript's type scale, the font is always the interface font, and layout nodes
have no fill, border, shadow, padding, or font props; using one is an error that
names the rule. 3D scenes render on the GPU (`zeron-visual-gpu`: physically
based materials, shadows, glass with dispersion, glTF models, textures,
environment maps, sprites and bloom), with the runtime's software rasterizer
(`raster.rs`) as the fallback where there's no GPU: Zeron frames an orbit
camera on the geometry and draws it into one transparent image. A visual's
files (`assets`: at most 16, 8 MB each, 10 MB together) are stored beside its
script as `{id}-asset-{name}` with an `{id}-assets.json` list, and travel to
other devices like attachments. `ui.shader` takes Shadertoy-style GLSL
(`mainImage`, `iResolution`, `iTime`, `iMouse`, plus `iCamera` for an orbit
camera, named float inputs bound to the script's state, `iNoise`, and up to four
Shadertoy-style buffer passes read as `iChannel0`-`3`, with feedback from their
own previous frame); `zeron_visual::shader` wraps each pass in a GLSL 4.50
program and validates it with naga, so a compile error reaches the agent with
its pass and line. A shader composites onto the chat by `blend` (`alpha`,
`light` for emitted light where black is clear, or `opaque`) with optional
bloom, so it never sits in a black box.

The engine runs every script before showing it (`zeron_visual::check`: setup,
the first render, every canvas and scene draw, then each tab, toggle, slider
end, and button once) and returns the first error with its line, so the agent
fixes it before the reader sees anything. A passing script is stored in uploads
(`{id}-visual.luau`) and a `visual` doc part is folded into the chat's live turn
through the run's engine event channel; with no live turn on this device the
call fails and nothing is stored. The doc carries only the path and title, so
clients read the script with `ReadAttachmentChunk` like any attachment; old
clients see an empty part.

The desktop (`crates/ui/src/visuals.rs`) renders the tree with the transcript's
own GPUI elements: markdown's table treatment, the settings tab, switch, and
button idioms, canvases as GPUI paths and shaped text in theme colors, and
scenes and shaders as images. Shaders run offscreen on the GPU through wgpu
(`crates/visual-gpu`, one shared device, pipelines cached by source) and are
read back like a scene's image. Nothing foreign is hosted, so a visual scrolls, clips, blurs under
the glass composer, and takes input like any other row, identically on Linux,
macOS, and Windows, and it follows theme, appearance, and text-size changes
without rerunning the script.

### Image replies

`show_image {path}` is listed under the same condition: it shows an image file
(a screenshot, a rendered PNG) inline in the calling chat as the same `image`
doc part Codex's generated images use, so every client already renders it
(the desktop's image row with its lightbox, iOS's image row). The MCP server
reads the file from the agent's working directory with the agent's own access
(at most 10 MB) and sends it base64 in `PublishImage {chatId, name, data}`. The
engine refuses anything but a PNG, JPEG, WebP, or GIF the desktop can preview
(at most 4096 pixels on a side, read from the header), stores it in uploads as
`{id}-image.{ext}`, and folds the part into the live turn through the run's
engine event channel, past the generated-image intake (whose root is Codex's
own folder). The doc carries the stored path and the file's name, never the
original path, and the image stays viewable after the source file changes or
is deleted. Codex rarely finds `show_image` (it defers MCP tools behind its
tool search) and answers "show me" with `![alt](/abs/path.png)`, the way its
own app displays local files; the transcript draws such an image in place,
from the chat's own folder only and never copied (see
[image-preview.md](image-preview.md#images-in-agent-replies)). Nothing tells
an agent about that: the tool description and the guidance name only
`show_image`.

## Selecting a host and project

`catalog {kind: "projects", device?}` filters by device id or exact name;
without `device` it lists all projects. `catalog {kind: "harnesses", device?}`
and `catalog {kind: "models", harness, device?}` query the selected host with
`targetDeviceId`; without `device` they read the local engine's catalogs.

For creation, project alone determines the host. Device alone creates a session
without a project on that device. With both, the project is resolved **within**
the selected device, and a project id belonging to another device is rejected.
Neither argument means a projectless session on the local engine. Repeated names
or paths are errors with candidate ids and devices; use ids from discovery.

Before writing, creation checks the chosen engine's harness catalog, chooses
its connected default (Claude Code when connected, otherwise the first connected
non-mock harness, falling back to an available provider), and validates any
explicit model against that host's catalog.
An explicit harness must be offered, installed and enabled. Catalog failures,
including model lookup failures, are returned with the device id; there is no
fallback to the local catalog. A successful remote query checks reachability;
`lastSeenAt` alone does not establish that an engine is online.

Discover and launch a standalone session:

```text
catalog { kind: "devices" }
catalog { kind: "projects", device: "<device-id>" }
catalog { kind: "harnesses", device: "<device-id>" }
catalog { kind: "models", device: "<device-id>", harness: "codex" }
create_chats { requests: [{
  kind: "chat",
  device: "<device-id>",
  project: "<project-id>",
  harness: "codex",
  model: "<model-id from that host's catalog>",
  title: "Implement feature",
  prompt: "Implement the feature..."
}] }
```

One request without a project (the host uses its home directory unless `cwd` is given):

```json
{"kind":"chat", "device":"<device-id>", "harness":"codex", "prompt":"Reply with pong", "wait":true}
```

A mixed `create_chats` batch, with an origin chat or explicit top-level parent:

```json
{
  "requests": [
    {"kind":"chat", "device":"<device-id>", "project":"<project-id>", "title":"Feature", "prompt":"Implement the feature"},
    {"kind":"side", "parent":"<parent-chat-id>", "project":"<project-id>", "prompt":"Review the API"}
  ]
}
```

The first prompt and later sends use durable chat commands drained by the host.
Transcript reads target the chat's host; session status comes from the shared
registry. Sends remember the session and existing message ids on the MCP connection before
sending, so a subsequent `wait_for_turn` after `wait:false` waits for that send,
even before a session row appears. A separate MCP server has no send baseline
and reports the chat's current posture. Creation records the send time before dispatching its first
prompt. If completion arrives before the transcript, the wait allows the new
assistant response to arrive within the **same timeout**. A previous response
is never substituted for a missing response to a new send. An unfinished
transcript or missing response at the deadline returns `timedOut`.

## Delegating across providers

The `zeron` server is injected into every real harness (table above), and
`create_chats` takes any `harness`, so every provider agent can hand work to any
other: Claude Code → Codex and the reverse, or to Cursor, OpenCode, and so on.
`catalog {kind: "harnesses"}` marks each provider:

- `available` — installed and enabled on this device.
- `signedIn` — whether a login for it is present on the selected host
  (`ListAgentAccounts`), or `null` when that could not be determined.
- `connected` — available and not known to be signed out. This is what a
  delegating agent should choose from; a request without a `harness` also
  prefers a connected provider (claude-code first).

`signedIn` is advisory: credential detection does not see every source (an API
key in the environment, say), so only an explicit `false` counts against a
provider. `create_chats` still creates the chat but returns a `warning` when the
chosen provider looks signed out, rather than refusing.

## Switching a chat's provider

A chat is not tied to the provider it started on. Picking another provider in
the composer (or `Mutate setChatConfig` with a different `harness`) moves the
conversation between turns; it is refused while a turn is running.

- A harness-native resume id only means something to the harness that minted
  it, so the chat row stores which one that was
  (`Chat.harness_session_harness`). `resume_for` turns away an id owned by a
  different harness; rows from before the tag are attributed from the run
  journal on first use.
- On the first run after a switch the host writes a `switch` part (a system
  entry: `from`, `to` display names) into the transcript, sets the session doc's
  `providerSwitched` flag, and hands the new provider's fresh session the prior
  conversation once (`Inner::fork_history_prompt`, the side-chat bootstrap,
  generalised). That replay is capped (about 80k characters, newest messages
  win; long messages and tool output are clipped), so a long chat continues
  with its recent context and a note that earlier messages were omitted.
- Later turns on the new provider resume its own session natively; switching
  back starts a fresh session there and replays again.

## Parallel side chats

Use `create_chats` with a `requests` array to launch independent workers in
one tool call, including when the harness executes its tool calls sequentially:

```json
{
  "requests": [
    {"project": "/repo", "title": "Review tests", "prompt": "Review test coverage"},
    {"project": "/repo", "title": "Review API", "prompt": "Review API compatibility"}
  ]
}
```

Use `message_chats` with the same envelope for existing chats (`chat`, `text`,
`mode?`, `wait?`). Both accept 1–32 requests, run them concurrently, and
return `results` in input order, each `{index, ok: true, chatId, …}` or
`{index, ok: false, error: {code, message}}`. A failed request does not cancel
or roll back successful requests. Each request defaults to `wait: false`;
explicit waits also run concurrently. Only batch independent work, not ordered
messages to the same chat. Launch **all** work first, then collect replies
with `wait_for_turn`.

## Smoke recipe

```sh
BIN=target/debug/zeron
{ echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}'
  echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_chats","arguments":{"limit":5}}}'
  sleep 5; } | ZERON_IPC_PORT=27655 ZERON_DATA_DIR=/path/to/that/engine/data $BIN mcp
```

`create_chats` with one request `{"prompt": "Reply with exactly the word pong", "wait": true}`
against a live daemon returns the assistant's `pong` in a few seconds; archive
the chat afterwards with `archive_chat`. Unit tests (`cargo test -p zeron-mcp`)
drive the whole tool set against an in-memory stub `RpcService`.

An isolated stdio instance is available with
`cargo run -p zeron-engine --example mcp_standalone_smoke`. It provides a temporary
project, an origin coordinator and a scripted `codex` adapter (`smoke-1`) returning
`pong`. Discover its ids with the list tools, create `kind: "chat"` with a prompt
and `wait: true`, then check `read_chat`, `message_chats` and a mixed batch. The
profile is removed on exit. This checks MCP dispatch and engine execution without
changing your running app's sessions or requiring provider credentials.

The `mcp_standalone_session_executes_on_the_selected_device` test in
`device_routing` additionally runs two engines with a shared registry and device
relay, verifies execution on the selected host (and absence of execution on the
caller), and retrieves responses through MCP.

## Snapshot for evals

The server answers `initialize` and `tools/list` with no engine running (the
plugin-tool lookup fails fast and the static catalog goes out), so an eval
harness can snapshot what agents see over stdio. Build the standalone binary
once (it is the same server as `zeron mcp`):

```sh
cargo build -p zeron-mcp --bin zeron-mcp
```

Then, from the repo root:

```sh
{ echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"eval","version":"0"}}}'
  echo '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
  sleep 1; } | ZERON_IPC_PORT=1 ZERON_CHAT_ID=eval-chat ZERON_WAITS_ON_BACKGROUND=1 target/debug/zeron-mcp
```

- `ZERON_IPC_PORT=1` points at a closed port, so nothing reaches a running Zeron.
- `ZERON_CHAT_ID` adds the in-chat tools (`show_visual`, `show_image`,
  `visual_reference`); `ZERON_WAITS_ON_BACKGROUND=1` adds `wait_on_background`.
- `ZERON_MCP_GUIDANCE=prompt` gives the one-line instructions a Claude, Codex
  or OpenCode 1.x run sees; without it, `instructions` holds the full
  guidance (what ACP agents see). `ZERON_SIDE_CHATS_ONLY=1` adds that section.

`crates/mcp/scripts/measure.py [binary] [--json]` runs the same exchange and
prints bytes per tool and totals. `crates/mcp/budget.json` is the checked-in
budget the `budget` unit test holds the catalog and guidance to.

