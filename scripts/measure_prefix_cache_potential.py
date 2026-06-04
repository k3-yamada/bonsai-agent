#!/usr/bin/env python3
"""
M-1: MLX server prompt cache potential measurement.

LocalAI 調査 (2026-06-04) 推奨 M-1 — MLX-LM server-side prompt cache の有無を確認し
prefix reuse 率を計測する。

Usage:
    python scripts/measure_prefix_cache_potential.py [--server http://localhost:8888]

目的:
- 項目 263/268 paired -6.83% の root cause 切り分け evidence
  (prefix cache miss が MLX latency noise の一因かを確認)
- /props から n_ctx を取得 (B-3 auto-clamp 機能の動作確認にも利用可)
"""
import argparse
import json
import sys
import time
import urllib.request
import urllib.error


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


def measure_prefix_reuse(server_url: str, model_id: str = "bonsai") -> list[float]:
    """同一 system prompt を繰り返し送り latency 変化を観測する"""
    print("\n=== [2] Prefix Reuse Latency Measurement ===")
    print("  (3 rounds with identical system prompt — cache hit should reduce latency)")

    system_prompt = "You are a helpful assistant. Reply very briefly."
    latencies = []

    for i in range(3):
        messages = [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": f"Reply 'OK' only. (round {i + 1})"},
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
            with urllib.request.urlopen(req, timeout=60) as r:
                resp = json.loads(r.read())
            elapsed = time.monotonic() - t0
            latencies.append(elapsed)
            content = (
                resp.get("choices", [{}])[0]
                .get("message", {})
                .get("content", "?")
            )
            usage = resp.get("usage", {})
            print(
                f"  Round {i + 1}: {elapsed:.3f}s | "
                f"prompt={usage.get('prompt_tokens','?')}tok "
                f"completion={usage.get('completion_tokens','?')}tok | "
                f"reply={content!r}"
            )
        except Exception as e:
            print(f"  Round {i + 1}: ERROR — {e}")

    return latencies


def analyze_results(latencies: list[float]) -> None:
    print("\n=== [3] Analysis ===")
    if len(latencies) < 2:
        print("  Insufficient data for analysis.")
        return

    first = latencies[0]
    last = latencies[-1]
    improvement_pct = (first - last) / first * 100 if first > 0 else 0

    print(f"  First round latency : {first:.3f}s")
    print(f"  Last round latency  : {last:.3f}s")
    print(f"  Δ (first - last)    : {first - last:+.3f}s ({improvement_pct:+.1f}%)")
    print()

    # MLX latency noise floor from item 268: ~5%
    NOISE_FLOOR_PCT = 5.0
    if improvement_pct > NOISE_FLOOR_PCT:
        print(f"  ✅ Possible prefix cache effect (>{NOISE_FLOOR_PCT:.0f}% speedup exceeds noise floor)")
        print("     Recommendation: investigate MLX-LM --cache-limit-gb flag")
    elif improvement_pct < -NOISE_FLOOR_PCT:
        print("  ⚠️  Latency INCREASED on repeated calls (unexpected — check server load)")
    else:
        print(f"  ℹ️  No significant speedup within noise floor (±{NOISE_FLOOR_PCT:.0f}%)")
        print("     Conclusion: MLX server likely does NOT cache prompt prefixes client-side")
        print("     item 268 paired -6.83% root cause remains: MLX latency measurement noise")

    print()
    print("  All latency values:", [f"{l:.3f}s" for l in latencies])


def main() -> None:
    parser = argparse.ArgumentParser(description="MLX prompt cache potential measurement (M-1)")
    parser.add_argument("--server", default="http://localhost:8888", help="MLX server URL")
    parser.add_argument("--model", default="bonsai", help="Model ID for /v1/chat/completions")
    args = parser.parse_args()

    print(f"MLX Prompt Cache Potential Measurement (M-1)")
    print(f"Server : {args.server}")
    print(f"Model  : {args.model}")
    print()

    caps = check_server_capabilities(args.server)

    model_id = caps.get("model_id", args.model)
    latencies = measure_prefix_reuse(args.server, model_id)

    analyze_results(latencies)

    print("\n=== Summary ===")
    n_ctx = caps.get("n_ctx")
    if n_ctx:
        print(f"  /props n_ctx={n_ctx} → B-3 auto-clamp applicable ✅")
    else:
        print("  /props n_ctx=unavailable → B-3 auto-clamp will use configured value")

    cache_metrics = caps.get("has_cache_metrics")
    if cache_metrics:
        print("  /metrics cache data available → monitor cache hit ratio ✅")
    elif cache_metrics is False:
        print("  /metrics available but no cache keywords → server does not expose cache stats")
    else:
        print("  /metrics unavailable → no server-side cache observability")


if __name__ == "__main__":
    main()
