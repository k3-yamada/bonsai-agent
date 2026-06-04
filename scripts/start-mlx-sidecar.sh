#!/bin/sh
# Phase 2 案B: bonsai 自前 MLX sidecar を起動する (cubist mlx-openai-server の代替)。
# Usage: ./scripts/start-mlx-sidecar.sh
#
# 役割:
#   scripts/mlx_server/server.py を uvicorn で起動。OpenAI 互換 /v1/chat/completions
#   + /v1/models を提供しつつ、env で MLX メモリ最適化 (set_cache_limit / KV量子化 /
#   max_kv_size) を解禁する。既定値は全 OFF = cubist 等価。
#
# 前提: scripts/setup_mlx_ternary.sh 実行済 (venv + mlx-lm + PrismML fork)。fastapi/uvicorn は
#   mlx-openai-server の依存で同 venv に既に存在。
#
# メモリ最適化 env (任意、段階導入):
#   BONSAI_MLX_CACHE_LIMIT_GB=10   # MLX バッファ上限 (swap 阻止)
#   BONSAI_MLX_KV_BITS=8           # KV 量子化 (8=保守 / 4=積極)
#   BONSAI_MLX_QUANTIZED_KV_START=256  # 先頭 256 tok は fp16 保持
#   BONSAI_MLX_MAX_KV_SIZE=16384   # 回転 KV 上限
#
# 停止: Ctrl+C
set -e

VENV_DIR="${HOME}/.venvs/bonsai-mlx"
PY="${VENV_DIR}/bin/python"
SERVER="$(dirname "$0")/mlx_server/server.py"

export BONSAI_MLX_MODEL="${BONSAI_MLX_MODEL:-prism-ml/Ternary-Bonsai-8B-mlx-2bit}"
export BONSAI_MLX_PORT="${BONSAI_MLX_PORT:-8888}"

if [ ! -x "$PY" ]; then
    echo "エラー: venv python が見つかりません: $PY"
    echo "  先に scripts/setup_mlx_ternary.sh を実行してください"
    exit 1
fi

# 既に起動済かチェック
if curl -fsS "http://localhost:${BONSAI_MLX_PORT}/v1/models" >/dev/null 2>&1; then
    echo "port ${BONSAI_MLX_PORT} で既にサーバ稼働中です"
    exit 0
fi

echo "=== bonsai MLX sidecar ==="
echo "  モデル: ${BONSAI_MLX_MODEL}"
echo "  URL:    http://localhost:${BONSAI_MLX_PORT}"
echo "  cache_limit=${BONSAI_MLX_CACHE_LIMIT_GB:-(unset)} kv_bits=${BONSAI_MLX_KV_BITS:-(unset)} max_kv_size=${BONSAI_MLX_MAX_KV_SIZE:-(unset)}"
echo "  Ctrl+C で停止"
echo ""

exec "$PY" "$SERVER"
