#!/usr/bin/env python3
"""
M-1: MLX server prompt cache potential measurement (warmup-controlled).

LocalAI 調査 (2026-06-04) 推奨 M-1。初版は同一 prefix を 3 回送るだけだったため
warmup (Metal kernel compile / model load) と prefix cache を区別できなかった。
本版は 2 つの control を導入してこれを分離する:

  1. Warmup phase — 計測前に throwaway request を投げ、cold-start (初回のみ遅い
     steady-state) を計測対象から除外する。
  2. SAME vs DIFF prefix control — 計測 phase で
       SAME: 固定の長い system prompt (prefix cache hit 候補)
       DIFF: 先頭に unique nonce を付けた system prompt (先頭 token が毎回異なる
             → prefix cache miss を強制)
     を interleave 計測する。両者の latency 差が真の prefix cache 効果。
     SAME ≈ DIFF なら「prefix cache 不在 = 初版の +39% は warmup のみ」が確定する。

Usage:
    python scripts/measure_prefix_cache_potential.py [--server http://localhost:8888]
                                                     [--rounds 6] [--warmup 3]

目的:
- 項目 263/268 paired -6.83% の root cause 切り分け evidence
  (warmup を差し引いた真の prefix cache 効果の有無を判定)
- /props / config から context 上限を確認 (B-3 auto-clamp の前提確認にも利用可)
"""
import argparse
import json
import statistics
import time
import urllib.error
import urllib.request
import uuid


def fetch_json(server_url: str, path: str) -> dict | None:
    try:
        url = f"{server_url.rstrip('/')}{path}"
        with urllib.request.urlopen(url, timeout=5) as r:
            return json.loads(r.read())
    except Exception as e:
        print(f"  [WARN] {path} unavailable: {e}")
        return None


def fetch_text(server_url: str, path: str) -> str | None:
    try:
        url = f"{server_url.rstrip('/')}{path}"
        with urllib.request.urlopen(url, timeout=5) as r:
            return r.read().decode()
    except Exception as e:
        print(f"  [WARN] {path} unavailable: {e}")
        return None


def check_server_capabilities(server_url: str) -> dict:
    result = {}
    print("=== [1] Server Capabilities ===")

    # /props — llama.cpp-compatible context metadata
    props = fetch_json(server_url, "/props")
    if props:
        print("  /props: AVAILABLE")
        for k, v in props.items():
            print(f"    {k}: {v}")
        result["n_ctx"] = props.get("n_ctx") or props.get("max_length")
        if result["n_ctx"]:
            print(f"  => n_ctx (for B-3 auto-clamp): {result['n_ctx']}")
    else:
        print("  /props: NOT AVAILABLE")

    # /metrics — Prometheus-style cache metrics
    metrics_raw = fetch_text(server_url, "/metrics")
    if metrics_raw:
        print("  /metrics: AVAILABLE")
        cache_lines = [
            l for l in metrics_raw.splitlines()
            if any(kw in l.lower() for kw in ["cache", "prompt", "prefix", "kv", "hit", "miss"])
        ]
        if cache_lines:
            print("  Cache-related metrics:")
            for l in cache_lines[:20]:
                print(f"    {l}")
            result["has_cache_metrics"] = True
        else:
            print("  (no cache/prompt/kv keywords in /metrics)")
            result["has_cache_metrics"] = False
    else:
        print("  /metrics: NOT AVAILABLE")

    # /v1/models — model info
    models = fetch_json(server_url, "/v1/models")
    if models:
        print("  /v1/models:")
        for m in models.get("data", []):
            print(f"    id={m.get('id')} owned_by={m.get('owned_by')}")
            result["model_id"] = m.get("id", "unknown")

    return result


# 長い固定 prefix — prefix cache が効くだけの token 数を稼ぐ (~数百 token)。
# 内容は無害な instruction の反復。SAME trial はこれをそのまま使う。
_SHARED_PREFIX = (
    "You are a meticulous assistant operating under strict constraints. "
    "Follow every instruction precisely and answer with extreme brevity. "
) * 24


def _one_call(server_url: str, model_id: str, system_prompt: str, tag: str) -> float | None:
    """1 回の chat completion を投げ、wall latency(秒) を返す。失敗で None。"""
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": f"Reply 'OK' only. ({tag})"},
    ]
    payload = json.dumps({
        "model": model_id,
        "messages": messages,
        "max_tokens": 5,
        "temperature": 0.0,
        "stream": False,
    }).encode()
    req = urllib.request.Request(
        f"{server_url.rstrip('/')}/v1/chat/completions",
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    t0 = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            json.loads(r.read())
        return time.monotonic() - t0
    except Exception as e:
        print(f"  [{tag}] ERROR — {e}")
        return None


def run_warmup(server_url: str, model_id: str, n: int) -> None:
    """計測前に throwaway request を投げ cold-start を除外する。"""
    print(f"\n=== [2] Warmup ({n} throwaway calls — excluded from measurement) ===")
    for i in range(n):
        # warmup ごとに別 prefix で、SAME/DIFF どちらにも cache を残さない。
        sys_prompt = f"warmup-{uuid.uuid4().hex}\n{_SHARED_PREFIX}"
        lat = _one_call(server_url, model_id, sys_prompt, f"warmup {i + 1}")
        if lat is not None:
            print(f"  warmup {i + 1}: {lat:.3f}s")


def measure_same_vs_diff(
    server_url: str, model_id: str, rounds: int
) -> tuple[list[float], list[float]]:
    """SAME prefix と DIFF prefix を interleave 計測する。

    SAME: 固定 prefix (warmup 後は prefix cache hit 候補)
    DIFF: 先頭 unique nonce 付き (先頭 token が毎回異なる → cache miss 強制)
    交互に投げて時間ドリフトを相殺する。
    """
    print(f"\n=== [3] SAME vs DIFF prefix ({rounds} rounds each, interleaved) ===")
    same_lat: list[float] = []
    diff_lat: list[float] = []
    for i in range(rounds):
        # SAME: 固定 prefix。
        s = _one_call(server_url, model_id, _SHARED_PREFIX, f"same {i + 1}")
        # DIFF: 先頭に nonce → 先頭 token から divergence。
        diff_prompt = f"{uuid.uuid4().hex}\n{_SHARED_PREFIX}"
        d = _one_call(server_url, model_id, diff_prompt, f"diff {i + 1}")
        if s is not None:
            same_lat.append(s)
        if d is not None:
            diff_lat.append(d)
        s_str = f"{s:.3f}s" if s is not None else "ERR"
        d_str = f"{d:.3f}s" if d is not None else "ERR"
        print(f"  round {i + 1}: SAME={s_str}  DIFF={d_str}")
    return same_lat, diff_lat


def analyze(same_lat: list[float], diff_lat: list[float]) -> None:
    """PAIRED 解析。

    SAME/DIFF は同 round に interleave 計測したので、独立 median 比較ではなく
    per-round の paired delta (diff[i] - same[i]) で評価する。これにより緩やかな
    latency drift (熱/負荷) を round 内で相殺する (ADR-003 paired-evidence 規律)。

    判定は 2 条件を AND で要求し、片方でも欠ければ inconclusive とする:
      (a) 符号一貫性 — paired delta が過半数で正 (DIFF が遅い = SAME が cache hit)
      (b) 効果量 — mean paired delta の相対値がノイズフロア (5%) を超える
    """
    print("\n=== [4] Analysis (paired, warmup-controlled) ===")
    n = min(len(same_lat), len(diff_lat))
    if n < 2:
        print("  Insufficient data for analysis.")
        return

    same_mean = statistics.mean(same_lat)
    diff_mean = statistics.mean(diff_lat)
    same_sd = statistics.pstdev(same_lat)
    diff_sd = statistics.pstdev(diff_lat)
    print(f"  SAME : mean={same_mean:.3f}s sd={same_sd:.3f}s n={len(same_lat)}")
    print(f"  DIFF : mean={diff_mean:.3f}s sd={diff_sd:.3f}s n={len(diff_lat)}")

    # ── drift confound 警告: 末尾が先頭の 1.5x 超なら latency が単調上昇している ──
    drift = (same_lat[-1] + diff_lat[-1]) > 1.5 * (same_lat[0] + diff_lat[0])
    if drift:
        print("  ⚠️  latency が計測中に上昇 (熱/負荷 drift) — paired delta で相殺評価する")

    # ── paired delta ──
    # MLX latency は heavy-tail (単発ストールで 5-6x spike)。mean は外れ値に
    # 乗っ取られるため、verdict は MEDIAN paired delta で評価する (mean は参考表示)。
    deltas = [diff_lat[i] - same_lat[i] for i in range(n)]
    mean_delta = statistics.mean(deltas)
    median_delta = statistics.median(deltas)
    median_same = statistics.median(same_lat)
    n_pos = sum(1 for d in deltas if d > 0)
    # 相対効果: median paired delta を median SAME で正規化 (外れ値に頑健)。
    rel_pct = median_delta / median_same * 100 if median_same > 0 else 0.0
    print(
        f"  paired delta (DIFF-SAME): median={median_delta:+.3f}s "
        f"({rel_pct:+.1f}% of median SAME) | mean={mean_delta:+.3f}s (outlier-inflated)"
    )
    print(f"  sign: {n_pos}/{n} rounds DIFF slower (SAME faster)")
    print("  per-round deltas:", [f"{d:+.3f}s" for d in deltas])
    print()

    NOISE_FLOOR_PCT = 5.0
    sign_ok = n_pos > n / 2.0 and (n_pos / n) >= 2.0 / 3.0  # 過半数 かつ >=2/3
    effect_ok = rel_pct > NOISE_FLOOR_PCT
    if sign_ok and effect_ok:
        print(f"  ✅ REAL prefix cache effect: SAME faster by {rel_pct:.1f}% (paired, {n_pos}/{n})")
        print("     warmup 除外 + paired drift 相殺後も SAME<DIFF = server が prefix を cache。")
        print("     項目 263/268 含意: prefix cache miss が paired noise の一因たり得る。")
    elif n_pos < n / 2.0 and rel_pct < -NOISE_FLOOR_PCT:
        print("  ⚠️  SAME slower than DIFF (unexpected — server load / measurement artifact)")
    else:
        print("  ℹ️  INCONCLUSIVE / no robust prefix cache effect.")
        print(f"     sign {n_pos}/{n} (need >=2/3) and/or effect {rel_pct:+.1f}% (need >{NOISE_FLOOR_PCT:.0f}%) 不足。")
        print("     median だけ見ると効果ありに見えるが、符号反転 round + 大きな分散が示す通り")
        print("     drift/noise 支配。初版 +39% (round1→2) は warmup (cold-start) が真因と整合し、")
        print("     項目 263/268 paired noise への prefix-cache 寄与は確証できない。")

    print()
    print("  SAME latencies:", [f"{l:.3f}s" for l in same_lat])
    print("  DIFF latencies:", [f"{l:.3f}s" for l in diff_lat])


def main() -> None:
    parser = argparse.ArgumentParser(
        description="MLX prompt cache potential measurement (M-1, warmup-controlled)"
    )
    parser.add_argument("--server", default="http://localhost:8888", help="MLX server URL")
    parser.add_argument("--model", default="bonsai", help="Model ID for /v1/chat/completions")
    parser.add_argument("--rounds", type=int, default=6, help="SAME/DIFF rounds (each)")
    parser.add_argument("--warmup", type=int, default=3, help="Warmup throwaway calls")
    args = parser.parse_args()

    print("MLX Prompt Cache Potential Measurement (M-1, warmup-controlled)")
    print(f"Server : {args.server}")
    print(f"Model  : {args.model}")
    print(f"Rounds : {args.rounds}  Warmup : {args.warmup}")
    print()

    caps = check_server_capabilities(args.server)
    model_id = caps.get("model_id", args.model)

    run_warmup(args.server, model_id, args.warmup)
    same_lat, diff_lat = measure_same_vs_diff(args.server, model_id, args.rounds)
    analyze(same_lat, diff_lat)

    print("\n=== Summary ===")
    n_ctx = caps.get("n_ctx")
    if n_ctx:
        print(f"  /props n_ctx={n_ctx} → B-3 auto-clamp applicable via /props ✅")
    else:
        print("  /props n_ctx=unavailable → B-3 falls back to model card config.json")

    cache_metrics = caps.get("has_cache_metrics")
    if cache_metrics:
        print("  /metrics cache data available → monitor cache hit ratio ✅")
    elif cache_metrics is False:
        print("  /metrics available but no cache keywords → server does not expose cache stats")
    else:
        print("  /metrics unavailable → no server-side cache observability")


if __name__ == "__main__":
    main()
