#!/bin/sh
# MLX モデル (既定: MiniCPM5-2B) を mlx-openai-server で起動する
# Usage: ./scripts/start-mlx-server.sh
#
# 役割:
#   bonsai-agent の fallback_chain primary entry (port 8000) を起こす。
#   このサーバーが落ちていると毎 LLM call で MLX retry overhead が発生し、
#   Lab paired runs が極端に slow down する (項目 244 後の Lab v21 cycle 1 = 5h+ 観測)。
#
# 前提:
#   - scripts/setup_mlx_ternary.sh を 1 回実行済 (venv + mlx-openai-server)。
#     MiniCPM5 等の標準 Llama 系アーキは素の mlx-lm で動く (PrismML fork 不要、fork は legacy
#     ternary-bonsai-8b 専用)。
#   - HF cache に MLX モデル取得済 (既定 openbmb/MiniCPM5-2B-MLX、~1.42GB)
#   - 別 terminal で llama-server (fallback、port 8080) を起動推奨 (scripts/start-server.sh)
#
# 検証:
#   curl http://localhost:8000/v1/models | jq .
#   → 起動した MLX モデルの repo id が data[].id に出れば成功
#
# 停止: Ctrl+C
set -e

SCRIPT_DIR="$(dirname "$0")"
# shellcheck source=./model.env
. "$SCRIPT_DIR/model.env"

VENV_DIR="${HOME}/.venvs/bonsai-mlx"
MLX_BIN="${VENV_DIR}/bin/mlx-openai-server"
MLX_MODEL="${MLX_MODEL:-$BONSAI_MODEL_MLX_REPO}"
PORT="${MLX_PORT:-8000}"

if [ -z "$MLX_MODEL" ]; then
    echo "エラー: BONSAI_MODEL_ID=$BONSAI_MODEL_ID には MLX ビルドがありません。BONSAI_MODEL_MLX_REPO または MLX_MODEL を指定してください" >&2
    exit 1
fi

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

echo "=== bonsai-agent MLX server ==="
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
