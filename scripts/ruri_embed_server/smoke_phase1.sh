#!/usr/bin/env bash
# Phase 1 smoke: Ruri sidecar health + query/document ranking (ADR-015).
# Requires: ./scripts/start-ruri-embed.sh already running on :8787
set -euo pipefail
BASE="${BONSAI_EMBED_URL:-http://127.0.0.1:8787}"
BASE="${BASE%/}"

curl -sf "$BASE/health" | python3 -c 'import sys,json; d=json.load(sys.stdin); assert d.get("ok") and d.get("dim")==256, d; print("health_ok", d)'

python3 - <<PY
import json, urllib.request
base = "${BASE}"

def emb(text, t):
    req = urllib.request.Request(
        f"{base}/v1/embeddings",
        data=json.dumps({
            "model": "cl-nagoya/ruri-v3-30m",
            "input": text,
            "input_type": t,
        }).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req) as r:
        return json.load(r)["data"][0]["embedding"]

def cos(a, b):
    return sum(x * y for x, y in zip(a, b))

q = emb("頭痛の話", "query")
d1 = emb("先週火曜に頭痛で休んだ", "document")
d2 = emb("来月の沖縄旅行を楽しみにしている", "document")
s1, s2 = cos(q, d1), cos(q, d2)
print(f"sim_related={s1:.4f} sim_unrelated={s2:.4f}")
assert s1 > s2, (s1, s2)
print("phase1_ok")
PY
