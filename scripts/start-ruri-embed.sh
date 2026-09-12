#!/usr/bin/env bash
# Start Ruri v3 /v1/embeddings sidecar (ADR-015).
# Spec: docs/execution/ruri-embed-sidecar.md
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export BONSAI_RURI_HOST="${BONSAI_RURI_HOST:-127.0.0.1}"
export BONSAI_RURI_PORT="${BONSAI_RURI_PORT:-8787}"
export BONSAI_RURI_MODEL="${BONSAI_RURI_MODEL:-cl-nagoya/ruri-v3-30m}"
export BONSAI_RURI_DEVICE="${BONSAI_RURI_DEVICE:-cpu}"

exec python3 "$ROOT/scripts/ruri_embed_server/server.py"
