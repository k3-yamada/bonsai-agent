#!/bin/sh
# Ternary Bonsai 8B (MLX 2-bit) を mlx-openai-server で起動する
# Usage: ./scripts/start-mlx-server.sh
#
# 役割:
#   bonsai-agent の fallback_chain primary entry (port 8000) を起こす。
#   このサーバーが落ちていると毎 LLM call で MLX retry overhead が発生し、
#   Lab paired runs が極端に slow down する (項目 244 後の Lab v21 cycle 1 = 5h+ 観測)。
#
# 前提:
#   - scripts/setup_mlx_ternary.sh を 1 回実行済 (venv + mlx-openai-server + PrismML MLX fork install)
#   - HF cache に prism-ml/Ternary-Bonsai-8B-mlx-2bit 取得済 (~2.2GB)
#   - 別 terminal で llama-server (Bonsai-8B fallback、port 8080) を起動推奨 (scripts/start-server.sh)
#
# 検証:
#   curl http://localhost:8000/v1/models | jq .
#   → "prism-ml/Ternary-Bonsai-8B-mlx-2bit" が data[].id に出れば成功
#
# 停止: Ctrl+C
set -e

VENV_DIR="${HOME}/.venvs/bonsai-mlx"
MLX_BIN="${VENV_DIR}/bin/mlx-openai-server"
MLX_MODEL="${MLX_MODEL:-prism-ml/Ternary-Bonsai-8B-mlx-2bit}"
PORT="${MLX_PORT:-8000}"

if [ ! -x "$MLX_BIN" ]; then
    echo "エラー: mlx-openai-server が見つかりません: $MLX_BIN"
    echo "  先に scripts/setup_mlx_ternary.sh を実行してください"
    exit 1
fi

# 既に起動済かチェック (port 8000 に応答があれば skip)
if curl -fsS "http://localhost:${PORT}/v1/models" >/dev/null 2>&1; then
    echo "MLX server は既に port ${PORT} で稼働中です"
    curl -fsS "http://localhost:${PORT}/v1/models" | head -c 200
    echo ""
    exit 0
fi

echo "=== Ternary Bonsai MLX server ==="
echo "  モデル: ${MLX_MODEL}"
echo "  URL:    http://localhost:${PORT}"
echo "  venv:   ${VENV_DIR}"
echo ""
echo "  Ctrl+C で停止"
echo ""

exec "$MLX_BIN" launch \
    --model-path "${MLX_MODEL}" \
    --model-type lm \
    --port "${PORT}" \
    "$@"
