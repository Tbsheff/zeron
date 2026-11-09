#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TOOLS="$ROOT/verification/orchestrator/tools"
mkdir -p "$TOOLS"
JAR="$TOOLS/tla2tools.jar"
EXPECTED=e1f11b75c4b5bc4478ba0ac5cb6ae28e40a01b0016bc33e0aa97da22dfdbcd7f
if [ -n "${TLA_JAR:-}" ]; then
  cp "$TLA_JAR" "$JAR"
elif [ ! -f "$JAR" ]; then
  curl --fail --location --retry 2 --output "$JAR.tmp" \
    https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar
  mv "$JAR.tmp" "$JAR"
fi
printf '%s  %s\n' "$EXPECTED" "$JAR" | shasum -a 256 --check
npm ci --prefix "$ROOT/verification/orchestrator" --ignore-scripts
command -v tlapm >/dev/null || { echo 'Missing: tlapm (see docs/orchestrator-v2-verification.md)' >&2; exit 1; }
command -v cargo-kani >/dev/null || { echo 'Missing: cargo-kani' >&2; exit 1; }
