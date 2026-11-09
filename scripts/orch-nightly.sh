#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
LOCK="$ROOT/target/orch-nightly.lock"
mkdir -p "$ROOT/target"
if ! mkdir "$LOCK" 2>/dev/null; then
  echo 'Failed: nightly already running (remove stale target/orch-nightly.lock after checking the process)' >&2
  exit 1
fi
trap 'rmdir "$LOCK"' EXIT
python3 scripts/orch-verify.py nightly --out "$ROOT/target/orch-nightly/$(date +%Y-%m-%d)"
