#!/usr/bin/env python3
"""ADR-015 Phase 3 — MiniLM vs Ruri v3 offline paired ranking bench.

同一 fixture 項目で A=MiniLM / B=Ruri の margin を取り、Δ = B−A に対し
lab_v22_metric の Cohen's dz / Wilcoxon / ACCEPT 基準（smoke）を適用する。

非ゴール:
  - 本番 default embedder の切替（ACCEPT 後の別 PR）
  - フル Lab smoke / llama-server（本スクリプトは埋め込み品質のみ）

使い方:
  # Ruri sidecar 起動済み前提
  .venv-ruri/bin/python scripts/g_paired_ruri_phase3.py
  # または
  scripts/g_paired_ruri_phase3.sh
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from lab_v22_metric import cohen_dz, judge_accept_v22, wilcoxon_signed_rank_p  # noqa: E402

DEFAULT_FIXTURE = (
    ROOT / "scripts/ruri_embed_server/fixtures/ja_episodes_phase3.json"
)
DEFAULT_RURI_URL = os.environ.get("BONSAI_EMBED_URL", "http://127.0.0.1:8787").rstrip(
    "/"
)
DEFAULT_MINILM = os.environ.get(
    "BONSAI_PHASE3_MINILM_MODEL", "sentence-transformers/all-MiniLM-L6-v2"
)


def cosine(a: list[float], b: list[float]) -> float:
    if len(a) != len(b) or not a:
        raise ValueError("vector length mismatch or empty")
    dot = 0.0
    na = 0.0
    nb = 0.0
    for x, y in zip(a, b):
        dot += x * y
        na += x * x
        nb += y * y
    if na <= 0 or nb <= 0:
        return 0.0
    return dot / math.sqrt(na * nb)


def l2_normalize(v: list[float]) -> list[float]:
    n = math.sqrt(sum(x * x for x in v))
    if n <= 0:
        return v
    return [x / n for x in v]


def http_embed(
    base_url: str,
    texts: list[str],
    *,
    model: str,
    input_type: str | None,
    timeout_s: float = 120.0,
) -> list[list[float]]:
    body: dict = {"model": model, "input": texts}
    if input_type is not None:
        body["input_type"] = input_type
    data = json.dumps(body).encode("utf-8")
    req = urllib.request.Request(
        f"{base_url}/v1/embeddings",
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout_s) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
    except urllib.error.URLError as e:
        raise SystemExit(f"Ruri sidecar unreachable at {base_url}: {e}") from e
    rows = sorted(payload["data"], key=lambda r: r["index"])
    return [l2_normalize([float(x) for x in r["embedding"]]) for r in rows]


class MiniLMLocal:
    """Production 既定と同系の All-MiniLM-L6-v2（sentence-transformers）。"""

    def __init__(self, model_id: str) -> None:
        from sentence_transformers import SentenceTransformer

        print(f"[phase3] loading MiniLM: {model_id}", flush=True)
        self.model = SentenceTransformer(model_id)

    def embed(self, texts: list[str]) -> list[list[float]]:
        vecs = self.model.encode(texts, normalize_embeddings=True)
        return [list(map(float, v)) for v in vecs]


def score_item(
    query_vec: list[float], gold_vec: list[float], distractor_vecs: list[list[float]]
) -> dict[str, float]:
    gold_sim = cosine(query_vec, gold_vec)
    dist_sims = [cosine(query_vec, d) for d in distractor_vecs]
    best_dist = max(dist_sims) if dist_sims else -1.0
    margin = gold_sim - best_dist
    # ranks: gold + distractors by similarity desc
    ranked = [("gold", gold_sim)] + [
        (f"d{i}", s) for i, s in enumerate(dist_sims)
    ]
    ranked.sort(key=lambda t: t[1], reverse=True)
    rank = next(i for i, (name, _) in enumerate(ranked, start=1) if name == "gold")
    hit1 = 1.0 if rank == 1 else 0.0
    mrr = 1.0 / rank
    return {
        "margin": margin,
        "gold_sim": gold_sim,
        "best_distractor": best_dist,
        "hit1": hit1,
        "mrr": mrr,
        "rank": float(rank),
    }


def mean(xs: list[float]) -> float:
    return sum(xs) / len(xs) if xs else 0.0


def run(args: argparse.Namespace) -> int:
    args.ruri_url = args.ruri_url.rstrip("/")
    fixture_path = Path(args.fixture)
    with fixture_path.open(encoding="utf-8") as f:
        fixture = json.load(f)
    items = fixture["items"]
    if args.limit:
        items = items[: args.limit]

    # health
    try:
        with urllib.request.urlopen(f"{args.ruri_url}/health", timeout=10) as resp:
            health = json.loads(resp.read().decode("utf-8"))
    except urllib.error.URLError as e:
        raise SystemExit(f"Ruri /health failed: {e}") from e
    print(f"[phase3] ruri health={health}", flush=True)

    minilm = MiniLMLocal(args.minilm_model)
    ruri_model = health.get("model") or args.ruri_model

    a_margins: list[float] = []
    b_margins: list[float] = []
    a_hit: list[float] = []
    b_hit: list[float] = []
    a_mrr: list[float] = []
    b_mrr: list[float] = []
    rows: list[dict] = []

    for item in items:
        q = item["query"]
        gold = item["gold"]
        distractors = item["distractors"]
        docs = [gold] + distractors

        # A: MiniLM — no Ruri prefixes
        a_q = minilm.embed([q])[0]
        a_docs = minilm.embed(docs)
        a_score = score_item(a_q, a_docs[0], a_docs[1:])

        # B: Ruri — query/document prefixes via sidecar
        b_q = http_embed(
            args.ruri_url, [q], model=ruri_model, input_type="query"
        )[0]
        b_docs = http_embed(
            args.ruri_url, docs, model=ruri_model, input_type="document"
        )
        b_score = score_item(b_q, b_docs[0], b_docs[1:])

        delta = b_score["margin"] - a_score["margin"]
        a_margins.append(a_score["margin"])
        b_margins.append(b_score["margin"])
        a_hit.append(a_score["hit1"])
        b_hit.append(b_score["hit1"])
        a_mrr.append(a_score["mrr"])
        b_mrr.append(b_score["mrr"])
        row = {
            "id": item["id"],
            "a_margin": a_score["margin"],
            "b_margin": b_score["margin"],
            "delta_margin": delta,
            "a_hit1": a_score["hit1"],
            "b_hit1": b_score["hit1"],
            "a_mrr": a_score["mrr"],
            "b_mrr": b_score["mrr"],
        }
        rows.append(row)
        print(
            f"[phase3] {item['id']}: A_margin={a_score['margin']:.4f} "
            f"B_margin={b_score['margin']:.4f} Δ={delta:+.4f} "
            f"hit1 A={int(a_score['hit1'])} B={int(b_score['hit1'])}",
            flush=True,
        )

    deltas = [r["delta_margin"] for r in rows]
    mean_delta = mean(deltas)
    dz = cohen_dz(deltas)
    w_plus, wp = wilcoxon_signed_rank_p(deltas)

    # factcheck は埋め込みベンチ非対象。lab_v22 の (a)(b)(c) 相当を margin 主軸で判定。
    accepted, reasons = accept_margin_only(mean_delta, dz, wp, args.mode)
    # 参考: 同関数のメトリクス形状（gate_d は無視）
    _, _, metrics = judge_accept_v22(
        deltas, on_summaries=[], noise_floor=0.0, mode=args.mode
    )

    summary = {
        "n": len(rows),
        "mean_margin_A": mean(a_margins),
        "mean_margin_B": mean(b_margins),
        "mean_delta_margin": mean_delta,
        "cohen_dz": None if math.isinf(dz) else dz,
        "cohen_dz_raw": str(dz) if math.isinf(dz) else dz,
        "wilcoxon_Wplus": w_plus,
        "wilcoxon_p": wp,
        "hit1_A": mean(a_hit),
        "hit1_B": mean(b_hit),
        "mrr_A": mean(a_mrr),
        "mrr_B": mean(b_mrr),
        "verdict": "ACCEPT" if accepted else "REJECT",
        "reasons": reasons,
        "mode": args.mode,
        "note": "default embedder は切替しない（ADR-015 Phase 3 / ADR-003）",
        "lab_v22_gate_ref": {
            "gate_a": metrics.get("gate_a"),
            "gate_b": metrics.get("gate_b"),
            "gate_c": metrics.get("gate_c"),
        },
    }

    print("\n=== Phase 3 paired summary (B=Ruri − A=MiniLM) ===", flush=True)
    print(json.dumps(summary, ensure_ascii=False, indent=2), flush=True)

    out_path = Path(args.out) if args.out else None
    if out_path:
        out_path.parent.mkdir(parents=True, exist_ok=True)
        payload = {"summary": summary, "rows": rows}
        out_path.write_text(
            json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        print(f"[phase3] wrote {out_path}", flush=True)

    # REJECT でもハーネス成功（exit 0）。ACCEPT のみ 0 / 統計失敗は 0。
    # 実行エラーだけ非ゼロ。判定は summary.verdict を見る。
    return 0


def accept_margin_only(
    mean_delta: float, dz: float, wp: float, mode: str
) -> tuple[bool, list[str]]:
    if mode == "full":
        dz_th, p_th = 0.40, 0.05
    else:
        dz_th, p_th = 0.30, 0.10
    reasons: list[str] = []
    ok_mean = mean_delta >= 0.010
    ok_dz = (dz >= dz_th) if not math.isnan(dz) else False
    ok_p = wp <= p_th
    if not ok_mean:
        reasons.append(f"mean_delta {mean_delta:.4f} < 0.010")
    if not ok_dz:
        reasons.append(f"cohen_dz {dz} < {dz_th}")
    if not ok_p:
        reasons.append(f"wilcoxon_p {wp:.4f} > {p_th}")
    if ok_mean and ok_dz and ok_p:
        return True, ["margin primary gates passed (factcheck N/A)"]
    return False, reasons


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--fixture", default=str(DEFAULT_FIXTURE))
    p.add_argument("--ruri-url", default=DEFAULT_RURI_URL)
    p.add_argument("--ruri-model", default="cl-nagoya/ruri-v3-30m")
    p.add_argument("--minilm-model", default=DEFAULT_MINILM)
    p.add_argument("--mode", choices=("smoke", "full"), default="smoke")
    p.add_argument("--limit", type=int, default=0, help="先頭 N 件のみ（0=全件）")
    p.add_argument(
        "--out",
        default=str(ROOT / "scripts/ruri_embed_server/fixtures/phase3_last_result.json"),
    )
    args = p.parse_args()
    raise SystemExit(run(args))


if __name__ == "__main__":
    main()
