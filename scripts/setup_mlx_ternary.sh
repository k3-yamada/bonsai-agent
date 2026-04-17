#!/bin/bash
# Ternary Bonsai 8B MLX版セットアップスクリプト
# Usage: ./scripts/setup_mlx_ternary.sh
# Mac M2/M3/M4 Apple Silicon向け

set -e

MLX_MODEL="prism-ml/Ternary-Bonsai-8B-mlx-2bit"
PORT=8000

echo "=== Ternary Bonsai 8B MLX セットアップ ==="
echo ""

# Python/pip チェック
if ! command -v python3 &> /dev/null; then
    echo "python3 が必要です"
    exit 1
fi

# mlx-lm インストール
echo "1. mlx-lm インストール..."
pip3 install --quiet mlx-lm

# PrismML fork の MLX（ternaryカーネル対応）
echo "2. PrismML MLX fork インストール..."
pip3 install --quiet "mlx @ git+https://github.com/PrismML-Eng/mlx.git@prism"

# mlx-openai-server インストール
echo "3. mlx-openai-server インストール..."
pip3 install --quiet mlx-openai-server

echo ""
echo "=== セットアップ完了 ==="
echo ""
echo "サーバー起動:"
echo "  mlx-openai-server launch --model-path ${MLX_MODEL} --model-type lm --port ${PORT}"
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
