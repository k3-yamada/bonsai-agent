#!/bin/bash
# Ternary Bonsai 8B MLX版セットアップスクリプト
# Usage: ./scripts/setup_mlx_ternary.sh
# Mac M2/M3/M4 Apple Silicon向け

set -e

VENV_DIR="${HOME}/.venvs/bonsai-mlx"
MLX_MODEL="prism-ml/Ternary-Bonsai-8B-mlx-2bit"
PORT=8000

echo "=== Ternary Bonsai 8B MLX セットアップ ==="
echo ""

# Python チェック
if ! command -v python3 &> /dev/null; then
    echo "python3 が必要です"
    exit 1
fi

# venv 作成
if [ ! -d "${VENV_DIR}" ]; then
    echo "1. venv 作成: ${VENV_DIR}"
    python3 -m venv "${VENV_DIR}"
else
    echo "1. venv 既存: ${VENV_DIR}"
fi

# venv 有効化
source "${VENV_DIR}/bin/activate"
echo "   Python: $(which python3)"

# pip アップグレード
pip install --quiet --upgrade pip

# mlx-lm インストール
echo "2. mlx-lm インストール..."
pip install --quiet mlx-lm

# PrismML fork の MLX（ternaryカーネル対応）
echo "3. PrismML MLX fork インストール..."
pip install --quiet "mlx @ git+https://github.com/PrismML-Eng/mlx.git@prism"

# mlx-openai-server インストール
echo "4. mlx-openai-server インストール..."
pip install --quiet mlx-openai-server

echo ""
echo "=== セットアップ完了 ==="
echo ""
echo "サーバー起動（毎回 venv を有効化してから）:"
echo "  source ${VENV_DIR}/bin/activate"
echo "  mlx-openai-server launch --model-path ${MLX_MODEL} --model-type lm --port ${PORT}"
echo ""
echo "ワンライナー起動:"
echo "  ${VENV_DIR}/bin/mlx-openai-server launch --model-path ${MLX_MODEL} --model-type lm --port ${PORT}"
echo ""
echo "config.toml 設定:"
echo "  [model]"
echo "  backend = \"mlx-lm\""
echo "  server_url = \"http://localhost:${PORT}\""
echo "  model_id = \"ternary-bonsai-8b\""
echo "  context_length = 65536"
echo ""
echo "動作確認:"
echo "  curl http://localhost:${PORT}/v1/models"
