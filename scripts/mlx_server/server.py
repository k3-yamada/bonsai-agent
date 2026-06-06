#!/usr/bin/env python3
"""bonsai-agent 用 MLX sidecar server (Phase 2 案B)。

cubist mlx-openai-server の drop-in 代替。OpenAI 互換 `/v1/chat/completions`
(SSE stream) + `/v1/models` を提供しつつ、cubist が配線していない **メモリ最適化**
を env で解禁する:
  - `mx.set_cache_limit` / `set_wired_limit` (MLX バッファ上限 → swap 阻止)
  - KV cache 量子化 (`kv_bits` / `quantized_kv_start`)
  - `max_kv_size` (回転 KV → 長文でのメモリ暴走防止)

**クリーンアーキテクチャ境界**: 本 sidecar は HTTP 境界の外。bonsai (Rust) 側は
`domain::llm::LlmBackend` port 経由の純粋 consumer のままで、推論実行 + メモリ制御
のみを担う。prompt 整形や tool policy は持たない (chat template の source-of-truth を
backend tokenizer に委ねる = ADR-011)。

既定値は全メモリ機能 OFF = 現状 (cubist) と等価動作。env で段階的に有効化する。

env:
  BONSAI_MLX_MODEL              既定 prism-ml/Ternary-Bonsai-8B-mlx-2bit
  BONSAI_MLX_PORT               既定 8888
  BONSAI_MLX_CACHE_LIMIT_GB     設定時 mx.set_cache_limit(GB)
  BONSAI_MLX_WIRED_LIMIT_GB     設定時 mx.set_wired_limit(GB)
  BONSAI_MLX_KV_BITS            設定時 KV 量子化 (4 or 8、0.25.3 は K/V 共通)
  BONSAI_MLX_KV_GROUP_SIZE      既定 64
  BONSAI_MLX_QUANTIZED_KV_START 既定 0 (>0 で先頭 N tok を fp16 保持し精度劣化緩和)
  BONSAI_MLX_MAX_KV_SIZE        設定時 回転 KV 上限

Usage: scripts/start-mlx-sidecar.sh  (uvicorn 起動)
"""
import json
import os
import time
import uuid
from typing import Any, Optional

# ───────────────────────── pure helpers (mlx 非依存・テスト可能) ─────────────────────────


def sampler_params_from_body(body: dict[str, Any]) -> dict[str, Any]:
    """request body から sampler/penalty の数値を抽出 (mlx 非依存・テスト可能)。

    bonsai は repeat_penalty/repetition_penalty/top_k/min_p も送るが、旧実装は
    temperature/top_p しか拾わず反復ペナルティが効かなかった (1bit モデルの
    token 反復崩壊「おっ！おっ！…」の原因)。repeat_penalty を優先し、
    OpenAI 互換 alias repetition_penalty も拾う。欠損 / 不正値は安全な既定へ。"""

    def _f(key: str, default: float) -> float:
        v = body.get(key, default)
        try:
            return float(v) if v is not None else default
        except (TypeError, ValueError):
            return default

    def _i(key: str, default: int) -> int:
        v = body.get(key, default)
        try:
            return int(v) if v is not None else default
        except (TypeError, ValueError):
            return default

    rep_raw = body.get("repeat_penalty", body.get("repetition_penalty"))
    try:
        rep = float(rep_raw) if rep_raw is not None else 1.0
    except (TypeError, ValueError):
        rep = 1.0

    return {
        "temp": _f("temperature", 0.0),
        "top_p": _f("top_p", 1.0) or 1.0,
        "top_k": _i("top_k", 0),
        "min_p": _f("min_p", 0.0),
        "max_tokens": _i("max_tokens", 512),
        "repetition_penalty": rep,
    }


def env_int(name: str) -> Optional[int]:
    """env を Optional[int] で読む。未設定 / 空 / parse 不能で None。"""
    v = os.environ.get(name, "").strip()
    if not v:
        return None
    try:
        return int(v)
    except ValueError:
        return None


def env_float(name: str) -> Optional[float]:
    v = os.environ.get(name, "").strip()
    if not v:
        return None
    try:
        return float(v)
    except ValueError:
        return None


def kv_kwargs_from_env() -> dict[str, Any]:
    """generate kwargs (KV量子化 / max_kv_size) を env から組む。未設定キーは含めない
    = generate_step の既定 (量子化なし) を使う = cubist 等価。"""
    kw: dict[str, Any] = {}
    kv_bits = env_int("BONSAI_MLX_KV_BITS")
    if kv_bits in (4, 8):
        kw["kv_bits"] = kv_bits
        kw["kv_group_size"] = env_int("BONSAI_MLX_KV_GROUP_SIZE") or 64
        kw["quantized_kv_start"] = env_int("BONSAI_MLX_QUANTIZED_KV_START") or 0
    max_kv = env_int("BONSAI_MLX_MAX_KV_SIZE")
    if max_kv and max_kv > 0:
        kw["max_kv_size"] = max_kv
    return kw


def models_response(model_id: str) -> dict[str, Any]:
    return {
        "object": "list",
        "data": [{"id": model_id, "object": "model", "owned_by": "local"}],
    }


def _chunk(model_id: str, delta: dict[str, Any], finish_reason: Optional[str]) -> dict[str, Any]:
    return {
        "id": f"chatcmpl-{uuid.uuid4().hex[:24]}",
        "object": "chat.completion.chunk",
        "created": int(time.time()),
        "model": model_id,
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}],
    }


def delta_chunk(model_id: str, text: str) -> dict[str, Any]:
    """ストリーム途中の content delta チャンク。"""
    return _chunk(model_id, {"content": text}, None)


def final_chunk(model_id: str, prompt_tokens: int, completion_tokens: int) -> dict[str, Any]:
    """終端チャンク (finish_reason=stop + usage)。"""
    c = _chunk(model_id, {}, "stop")
    c["usage"] = {
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": prompt_tokens + completion_tokens,
    }
    return c


def completion_response(
    model_id: str, text: str, prompt_tokens: int, completion_tokens: int
) -> dict[str, Any]:
    """非ストリーム応答 (stream=false)。"""
    return {
        "id": f"chatcmpl-{uuid.uuid4().hex[:24]}",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": model_id,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop",
            }
        ],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        },
    }


def sse_line(obj: dict[str, Any]) -> str:
    return f"data: {json.dumps(obj, ensure_ascii=False)}\n\n"


SSE_DONE = "data: [DONE]\n\n"


# ───────────────────────── server (mlx 依存・起動時に import) ─────────────────────────


def build_app():
    """FastAPI app を生成。重い import (mlx) は本関数内に閉じる (helper のテストを軽くする)。"""
    import mlx.core as mx
    from fastapi import FastAPI, Request
    from fastapi.responses import JSONResponse, StreamingResponse
    from starlette.concurrency import run_in_threadpool
    from mlx_lm import load, stream_generate
    from mlx_lm.sample_utils import make_logits_processors, make_sampler

    model_id = os.environ.get("BONSAI_MLX_MODEL", "prism-ml/Ternary-Bonsai-8B-mlx-2bit")

    # ── メモリ上限を load 前に固定 (swap 阻止、99% ディスク環境で致命的な swap を回避) ──
    cache_gb = env_float("BONSAI_MLX_CACHE_LIMIT_GB")
    if cache_gb is not None:
        mx.set_cache_limit(int(cache_gb * (1024**3)))
        print(f"[sidecar] set_cache_limit={cache_gb}GB")
    wired_gb = env_float("BONSAI_MLX_WIRED_LIMIT_GB")
    if wired_gb is not None:
        mx.set_wired_limit(int(wired_gb * (1024**3)))
        print(f"[sidecar] set_wired_limit={wired_gb}GB")

    print(f"[sidecar] loading {model_id} ...")
    model, tokenizer = load(model_id)
    gen_kw = kv_kwargs_from_env()
    print(f"[sidecar] ready. KV/memory kwargs = {gen_kw or '(none = cubist等価)'}")

    app = FastAPI()

    @app.get("/v1/models")
    def list_models():
        return models_response(model_id)

    @app.get("/health")
    def health():
        return {"status": "ok"}

    @app.get("/mem")
    def mem():
        """MLX allocator のメモリ計測 (Phase 2b RSS 計測用)。
        peak は直近 chat request 開始時に reset される。"""
        gb = 1024.0**3
        return {
            "active_gb": round(mx.get_active_memory() / gb, 3),
            "peak_gb": round(mx.get_peak_memory() / gb, 3),
            "cache_gb": round(mx.get_cache_memory() / gb, 3),
            "kv_kwargs": gen_kw,
        }

    def _prompt(messages: list[dict[str, Any]]) -> Any:
        # chat template は backend tokenizer に委譲 (ADR-011)。
        return tokenizer.apply_chat_template(
            messages, add_generation_prompt=True, tokenize=False
        )

    def _gen_args(body: dict[str, Any]) -> dict[str, Any]:
        p = sampler_params_from_body(body)
        sampler = make_sampler(
            temp=p["temp"],
            top_p=p["top_p"],
            min_p=p["min_p"],
            top_k=p["top_k"],
        )
        args: dict[str, Any] = {
            "max_tokens": p["max_tokens"],
            "sampler": sampler,
        }
        # 反復ペナルティ (>1.0 のときのみ): 1bit モデルの token 反復崩壊を抑止。
        if p["repetition_penalty"] and p["repetition_penalty"] != 1.0:
            args["logits_processors"] = make_logits_processors(
                repetition_penalty=p["repetition_penalty"]
            )
        args.update(gen_kw)  # KV量子化 / max_kv_size (env 由来)
        return args

    @app.post("/v1/chat/completions")
    async def chat(request: Request):
        body = await request.json()
        messages = body.get("messages", [])
        prompt = _prompt(messages)
        args = _gen_args(body)
        stream = bool(body.get("stream", False))
        mx.reset_peak_memory()  # この request の peak を /mem で読めるように

        if stream:
            def event_stream():
                last = None
                try:
                    for resp in stream_generate(model, tokenizer, prompt, **args):
                        last = resp
                        if resp.text:
                            yield sse_line(delta_chunk(model_id, resp.text))
                    pt = getattr(last, "prompt_tokens", 0) if last else 0
                    ct = getattr(last, "generation_tokens", 0) if last else 0
                    yield sse_line(final_chunk(model_id, pt, ct))
                    yield SSE_DONE
                except GeneratorExit:
                    # クライアント切断 (bonsai の Ctrl+C / キャンセル)。同期せず
                    # generator が破棄されると進行中の Metal command buffer が
                    # 途中解放され segfault する → mx.synchronize() で GPU を
                    # 一貫状態にしてから unwind する。
                    try:
                        mx.synchronize()
                    except Exception:
                        pass
                    raise

            return StreamingResponse(event_stream(), media_type="text/event-stream")

        # 非ストリーム: 同期生成は threadpool で実行しイベントループをブロックしない
        # (/health /mem を生成中も応答可能に保つ。bonsai supervisor の health poll 対策)。
        def _blocking_generate():
            text, last = "", None
            for resp in stream_generate(model, tokenizer, prompt, **args):
                last = resp
                text += resp.text
            pt = getattr(last, "prompt_tokens", 0) if last else 0
            ct = getattr(last, "generation_tokens", 0) if last else 0
            return text, pt, ct

        text, pt, ct = await run_in_threadpool(_blocking_generate)
        return JSONResponse(completion_response(model_id, text, pt, ct))

    return app


def main() -> None:
    import uvicorn

    port = env_int("BONSAI_MLX_PORT") or 8888
    uvicorn.run(build_app(), host="127.0.0.1", port=port, log_level="warning")


if __name__ == "__main__":
    main()
