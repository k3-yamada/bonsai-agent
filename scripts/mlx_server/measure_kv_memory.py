#!/usr/bin/env python3
"""Phase 2b: KV量子化の peak メモリ効果を実測する。

長い prompt を投げて KV cache を膨らませ、sidecar の `/mem` (mx.get_peak_memory)
を読む。baseline (kv量子化OFF) と kv_bits=4/8 を別 server 起動で比較する。

Usage:
    python scripts/mlx_server/measure_kv_memory.py [--server http://localhost:8888] [--ctx-words 6000]
"""
import argparse
import json
import urllib.request


def get_mem(server: str) -> dict:
    with urllib.request.urlopen(f"{server}/mem", timeout=5) as r:
        return json.loads(r.read())


def chat(server: str, prompt: str, max_tokens: int) -> dict:
    body = {
        "model": "bonsai",
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens,
        "temperature": 0.0,
        "stream": False,
    }
    req = urllib.request.Request(
        f"{server}/v1/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read())


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://localhost:8888")
    ap.add_argument("--ctx-words", type=int, default=6000, help="prompt 語数 (KV を膨らませる)")
    ap.add_argument("--max-tokens", type=int, default=32)
    args = ap.parse_args()

    # 長い prompt = KV cache を大きくする (語ごとに別 token になりやすい連番)
    filler = " ".join(f"item{i}" for i in range(args.ctx_words))
    prompt = f"Below is a list. Reply only 'DONE'.\n{filler}\nReply 'DONE'."

    cfg = get_mem(args.server)
    print(f"config kv_kwargs = {cfg.get('kv_kwargs')}")
    print(f"before: active={cfg['active_gb']}GB peak={cfg['peak_gb']}GB cache={cfg['cache_gb']}GB")

    resp = chat(args.server, prompt, args.max_tokens)
    usage = resp.get("usage", {})
    after = get_mem(args.server)
    print(f"prompt_tokens={usage.get('prompt_tokens')} completion={usage.get('completion_tokens')}")
    print(f"AFTER: active={after['active_gb']}GB **peak={after['peak_gb']}GB** cache={after['cache_gb']}GB")
    print(f"=> peak_gb={after['peak_gb']} (kv={cfg.get('kv_kwargs')})")


if __name__ == "__main__":
    main()
