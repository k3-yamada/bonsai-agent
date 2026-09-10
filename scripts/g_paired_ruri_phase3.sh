#!/usr/bin/env bash
# ADR-015 Phase 3 wrapper — MiniLM vs Ruri offline paired bench
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENV="${ROOT}/.venv-ruri"
PY="${VENV}/bin/python"
if [[ ! -x "${PY}" ]]; then
  echo "missing ${PY}; create with: virtualenv ${VENV} && ${VENV}/bin/pip install -r scripts/ruri_embed_server/requirements.txt sentence-transformers" >&2
  exit 1
fi

# Ensure MiniLM dependency present (Ruri requirements may already include ST)
"${PY}" -c "import sentence_transformers" 2>/dev/null || \
  "${PY}" -m pip install -q "sentence-transformers>=3,<7"

RURI_URL="${BONSAI_EMBED_URL:-http://127.0.0.1:8787}"
if ! curl -sf "${RURI_URL}/health" >/dev/null; then
  echo "Ruri sidecar not healthy at ${RURI_URL}. Start: scripts/start-ruri-embed.sh" >&2
  exit 1
fi

exec "${PY}" "${ROOT}/scripts/g_paired_ruri_phase3.py" \
  --ruri-url "${RURI_URL}" \
  --mode "${PHASE3_MODE:-smoke}" \
  "$@"
