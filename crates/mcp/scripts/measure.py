#!/usr/bin/env python3
import json, os, subprocess, sys

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/debug/zeron-mcp"
env = dict(os.environ)
env.setdefault("ZERON_IPC_PORT", "1")
env.setdefault("ZERON_CHAT_ID", "measure-chat")
env.setdefault("ZERON_WAITS_ON_BACKGROUND", "1")
frames = [
    {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "measure", "version": "0"}}},
    {"jsonrpc": "2.0", "method": "notifications/initialized"},
    {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
]
stdin = "".join(json.dumps(f) + "\n" for f in frames)
proc = subprocess.Popen([BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env, text=True)
proc.stdin.write(stdin)
proc.stdin.flush()
replies = {}
while len(replies) < 2:
    line = proc.stdout.readline()
    if not line:
        break
    msg = json.loads(line)
    if "id" in msg:
        replies[msg["id"]] = msg
proc.stdin.close()
proc.wait(timeout=10)

def size(value):
    if value is None:
        return 0
    if isinstance(value, str):
        return len(value.encode())
    return len(json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode())

instructions = replies[1]["result"].get("instructions", "")
tools = replies[2]["result"]["tools"]
rows = []
for t in tools:
    rows.append({
        "name": t["name"],
        "title": size(t.get("title")),
        "description": size(t.get("description")),
        "inputSchema": size(t.get("inputSchema")),
        "outputSchema": size(t.get("outputSchema")),
        "annotations": size(t.get("annotations")),
        "meta": size(t.get("_meta")),
        "total": size(t),
    })
summary = {
    "protocolVersion": replies[1]["result"]["protocolVersion"],
    "instructions": size(instructions),
    "tools": len(tools),
    "description": sum(r["description"] for r in rows),
    "inputSchema": sum(r["inputSchema"] for r in rows),
    "outputSchema": sum(r["outputSchema"] for r in rows),
    "toolsListBytes": size(tools),
    "alwaysLoad": [t["name"] for t in tools if (t.get("_meta") or {}).get("anthropic/alwaysLoad")],
}
if "--json" in sys.argv:
    print(json.dumps({"summary": summary, "tools": rows, "instructions": instructions}, indent=1))
else:
    print(f"{'tool':28} {'desc':>6} {'input':>6} {'output':>6} {'total':>6}")
    for r in sorted(rows, key=lambda r: -r["total"]):
        print(f"{r['name']:28} {r['description']:>6} {r['inputSchema']:>6} {r['outputSchema']:>6} {r['total']:>6}")
    print(json.dumps(summary, indent=1))
