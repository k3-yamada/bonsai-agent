"""Ruri v3 OpenAI-compatible /v1/embeddings sidecar (ADR-015).

Contract: docs/execution/ruri-embed-sidecar.md

依存: sentence-transformers（requirements.txt）。モデル未導入でも
prefix ヘルパは単体テスト可能（本ファイル末尾）。
"""

from __future__ import annotations

import json
import os
import threading
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import urlparse

# ─── prefix helpers（モデル非依存・単体テスト対象）──────────────────────────

PREFIX_BY_INPUT_TYPE: dict[str, str] = {
    "semantic": "",
    "topic": "トピック: ",
    "query": "検索クエリ: ",
    "document": "検索文書: ",
}

VALID_INPUT_TYPES = frozenset(PREFIX_BY_INPUT_TYPE)


def normalize_input_type(raw: Any) -> str:
    if raw is None or raw == "":
        return "semantic"
    if not isinstance(raw, str):
        raise ValueError("input_type must be a string")
    key = raw.strip().lower()
    if key not in VALID_INPUT_TYPES:
        raise ValueError(
            f"unknown input_type {raw!r}; expected one of {sorted(VALID_INPUT_TYPES)}"
        )
    return key


def apply_ruri_prefix(text: str, input_type: str) -> str:
    """Ruri v3 公式 prefix を付与。既に付いていれば二重付与しない。"""
    if not isinstance(text, str):
        raise ValueError("input texts must be strings")
    key = normalize_input_type(input_type)
    prefix = PREFIX_BY_INPUT_TYPE[key]
    if not prefix:
        return text
    if text.startswith(prefix):
        return text
    return prefix + text


def coerce_input_list(raw: Any) -> list[str]:
    if isinstance(raw, str):
        if raw == "":
            raise ValueError("input must not be empty")
        return [raw]
    if isinstance(raw, list):
        if not raw:
            raise ValueError("input list must not be empty")
        out: list[str] = []
        for i, item in enumerate(raw):
            if not isinstance(item, str):
                raise ValueError(f"input[{i}] must be a string")
            out.append(item)
        return out
    raise ValueError("input must be a string or array of strings")


def build_embeddings_payload(
    model: str, vectors: list[list[float]], prompt_tokens: int = 0
) -> dict[str, Any]:
    return {
        "object": "list",
        "data": [
            {"object": "embedding", "index": i, "embedding": vec}
            for i, vec in enumerate(vectors)
        ],
        "model": model,
        "usage": {"prompt_tokens": prompt_tokens, "total_tokens": prompt_tokens},
    }


# ─── model wrapper ───────────────────────────────────────────────────────────

class RuriEmbedder:
    def __init__(self, model_id: str, device: str, local_files_only: bool) -> None:
        self.model_id = model_id
        self.device = device
        self.local_files_only = local_files_only
        self._model = None
        self._dim: int | None = None
        self._lock = threading.Lock()

    @property
    def loaded(self) -> bool:
        return self._model is not None

    @property
    def dim(self) -> int | None:
        return self._dim

    def ensure_loaded(self) -> None:
        with self._lock:
            if self._model is not None:
                return
            from sentence_transformers import SentenceTransformer

            kwargs: dict[str, Any] = {"device": self.device}
            # transformers / sentence-transformers は版により local_files_only の
            # 渡し方が違うため、環境変数でも制御する。
            if self.local_files_only:
                os.environ.setdefault("HF_HUB_OFFLINE", "1")
                os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
            self._model = SentenceTransformer(self.model_id, **kwargs)
            probe = self._model.encode(["ping"], normalize_embeddings=True)
            self._dim = int(probe.shape[-1])

    def embed(self, texts: list[str]) -> list[list[float]]:
        self.ensure_loaded()
        assert self._model is not None
        arr = self._model.encode(texts, normalize_embeddings=True)
        return [row.tolist() for row in arr]


# ─── HTTP server ─────────────────────────────────────────────────────────────

def _env_bool(name: str, default: bool = False) -> bool:
    raw = os.environ.get(name)
    if raw is None or raw.strip() == "":
        return default
    return raw.strip().lower() in ("1", "true", "yes", "on")


class EmbedHandler(BaseHTTPRequestHandler):
    embedder: RuriEmbedder
    strict_model: bool

    def log_message(self, fmt: str, *args: Any) -> None:
        # 既定で入力本文は出さない
        sys_stderr = __import__("sys").stderr
        print(f"[ruri-embed] {self.address_string()} {fmt % args}", file=sys_stderr)

    def _send_json(self, code: int, body: dict[str, Any]) -> None:
        data = json.dumps(body, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _read_json(self) -> dict[str, Any]:
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b"{}"
        try:
            obj = json.loads(raw.decode("utf-8"))
        except json.JSONDecodeError as e:
            raise ValueError(f"invalid JSON: {e}") from e
        if not isinstance(obj, dict):
            raise ValueError("JSON body must be an object")
        return obj

    def do_GET(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path in ("/health", "/v1/health"):
            self._send_json(
                200,
                {
                    "ok": True,
                    "model": self.embedder.model_id,
                    "dim": self.embedder.dim,
                    "loaded": self.embedder.loaded,
                },
            )
            return
        if path == "/v1/models":
            self._send_json(
                200,
                {
                    "object": "list",
                    "data": [
                        {
                            "id": self.embedder.model_id,
                            "object": "model",
                            "owned_by": "local-ruri",
                        }
                    ],
                },
            )
            return
        self._send_json(404, {"error": {"message": f"not found: {path}", "type": "not_found"}})

    def do_POST(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path != "/v1/embeddings":
            self._send_json(404, {"error": {"message": f"not found: {path}", "type": "not_found"}})
            return
        try:
            body = self._read_json()
            texts = coerce_input_list(body.get("input"))
            input_type = normalize_input_type(body.get("input_type"))
            model = body.get("model") or self.embedder.model_id
            if not isinstance(model, str) or not model.strip():
                raise ValueError("model must be a non-empty string")
            model = model.strip()
            if self.strict_model and model != self.embedder.model_id:
                raise ValueError(
                    f"model {model!r} != configured {self.embedder.model_id!r}"
                )
            prefixed = [apply_ruri_prefix(t, input_type) for t in texts]
            vectors = self.embedder.embed(prefixed)
            self._send_json(200, build_embeddings_payload(model, vectors))
        except ValueError as e:
            self._send_json(400, {"error": {"message": str(e), "type": "invalid_request"}})
        except Exception as e:  # noqa: BLE001 — sidecar 境界で 503 に正規化
            traceback.print_exc()
            self._send_json(
                503,
                {"error": {"message": f"embed failed: {e}", "type": "unavailable"}},
            )


def main() -> None:
    host = os.environ.get("BONSAI_RURI_HOST", "127.0.0.1").strip() or "127.0.0.1"
    port = int(os.environ.get("BONSAI_RURI_PORT", "8787"))
    model_id = (
        os.environ.get("BONSAI_RURI_MODEL", "cl-nagoya/ruri-v3-30m").strip()
        or "cl-nagoya/ruri-v3-30m"
    )
    device = os.environ.get("BONSAI_RURI_DEVICE", "cpu").strip() or "cpu"
    local_only = _env_bool("BONSAI_RURI_LOCAL_FILES_ONLY", False)
    strict = _env_bool("BONSAI_RURI_STRICT_MODEL", False)
    preload = _env_bool("BONSAI_RURI_PRELOAD", False)

    embedder = RuriEmbedder(model_id, device, local_only)
    if preload:
        print(f"[ruri-embed] preloading {model_id} on {device}...", flush=True)
        embedder.ensure_loaded()
        print(f"[ruri-embed] ready dim={embedder.dim}", flush=True)

    handler = type(
        "BoundEmbedHandler",
        (EmbedHandler,),
        {"embedder": embedder, "strict_model": strict},
    )
    server = ThreadingHTTPServer((host, port), handler)
    print(
        f"[ruri-embed] listening on http://{host}:{port} model={model_id}",
        flush=True,
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n[ruri-embed] shutdown", flush=True)


if __name__ == "__main__":
    main()
