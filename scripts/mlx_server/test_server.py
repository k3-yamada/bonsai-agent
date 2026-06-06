#!/usr/bin/env python3
"""server.py の pure helper 単体テスト (mlx / モデルロード不要・高速)。

実行: python scripts/mlx_server/test_server.py  (assert ベース、pytest 不要)
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
import server  # noqa: E402


def _clear_env():
    for k in list(os.environ):
        if k.startswith("BONSAI_MLX_"):
            del os.environ[k]


def test_kv_kwargs_default_off():
    """env 未設定 = 量子化なし = cubist 等価 (空 dict)。"""
    _clear_env()
    assert server.kv_kwargs_from_env() == {}, "既定は KV機能 OFF"


def test_kv_kwargs_kv_bits():
    _clear_env()
    os.environ["BONSAI_MLX_KV_BITS"] = "4"
    os.environ["BONSAI_MLX_QUANTIZED_KV_START"] = "256"
    kw = server.kv_kwargs_from_env()
    assert kw["kv_bits"] == 4
    assert kw["kv_group_size"] == 64  # 既定
    assert kw["quantized_kv_start"] == 256
    _clear_env()


def test_kv_kwargs_invalid_bits_ignored():
    """4/8 以外の kv_bits は無視 (安全側で OFF)。"""
    _clear_env()
    os.environ["BONSAI_MLX_KV_BITS"] = "3"
    assert "kv_bits" not in server.kv_kwargs_from_env()
    _clear_env()


def test_kv_kwargs_max_kv_size():
    _clear_env()
    os.environ["BONSAI_MLX_MAX_KV_SIZE"] = "8192"
    assert server.kv_kwargs_from_env()["max_kv_size"] == 8192
    _clear_env()


def test_env_int_parse():
    _clear_env()
    os.environ["BONSAI_MLX_PORT"] = "8888"
    assert server.env_int("BONSAI_MLX_PORT") == 8888
    assert server.env_int("BONSAI_MLX_NONEXISTENT") is None
    os.environ["BONSAI_MLX_PORT"] = "notnum"
    assert server.env_int("BONSAI_MLX_PORT") is None
    _clear_env()


def test_models_response_shape():
    r = server.models_response("m/x")
    assert r["object"] == "list"
    assert r["data"][0]["id"] == "m/x"
    assert r["data"][0]["owned_by"] == "local"


def test_delta_chunk_openai_shape():
    c = server.delta_chunk("m", "hello")
    assert c["object"] == "chat.completion.chunk"
    assert c["choices"][0]["delta"]["content"] == "hello"
    assert c["choices"][0]["finish_reason"] is None


def test_final_chunk_usage():
    c = server.final_chunk("m", 10, 5)
    assert c["choices"][0]["finish_reason"] == "stop"
    assert c["usage"]["prompt_tokens"] == 10
    assert c["usage"]["completion_tokens"] == 5
    assert c["usage"]["total_tokens"] == 15


def test_completion_response_nonstream():
    r = server.completion_response("m", "ok", 3, 2)
    assert r["object"] == "chat.completion"
    assert r["choices"][0]["message"]["content"] == "ok"
    assert r["choices"][0]["message"]["role"] == "assistant"
    assert r["usage"]["total_tokens"] == 5


def test_sse_line_format():
    line = server.sse_line({"a": 1})
    assert line.startswith("data: ")
    assert line.endswith("\n\n")
    assert server.SSE_DONE == "data: [DONE]\n\n"


def test_sse_line_unicode_preserved():
    """日本語 (ensure_ascii=False) がエスケープされず保持される。"""
    line = server.sse_line({"content": "日本語"})
    assert "日本語" in line


def test_sampler_params_forwards_penalties():
    """bonsai が送る repeat_penalty/top_k/min_p が転送される (反復崩壊対策)。
    旧 _gen_args は temperature/top_p しか拾わず repeat_penalty を破棄していた。"""
    body = {
        "temperature": 0.5,
        "top_p": 0.85,
        "top_k": 20,
        "min_p": 0.05,
        "max_tokens": 1024,
        "repeat_penalty": 1.15,
    }
    p = server.sampler_params_from_body(body)
    assert p["temp"] == 0.5
    assert p["top_p"] == 0.85
    assert p["top_k"] == 20
    assert p["min_p"] == 0.05
    assert p["max_tokens"] == 1024
    assert p["repetition_penalty"] == 1.15


def test_sampler_params_defaults_and_alias():
    """欠損時は安全な既定。repetition_penalty alias も拾い、空 body は penalty=1.0。"""
    p = server.sampler_params_from_body({"repetition_penalty": 1.2})
    assert p["temp"] == 0.0
    assert p["top_k"] == 0
    assert p["min_p"] == 0.0
    assert p["repetition_penalty"] == 1.2  # alias 経由
    assert server.sampler_params_from_body({})["repetition_penalty"] == 1.0


if __name__ == "__main__":
    passed = 0
    for name in sorted(dir()):
        if name.startswith("test_"):
            globals()[name]()
            passed += 1
            print(f"  PASS {name}")
    print(f"\n{passed} passed")
