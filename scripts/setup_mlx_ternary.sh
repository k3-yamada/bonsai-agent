#!/bin/bash
# bonsai-agent MLX (Apple Silicon) セットアップスクリプト
# Usage: ./scripts/setup_mlx_ternary.sh
# Mac M2/M3/M4 Apple Silicon向け
#
# 既定モデル (MiniCPM5-2B 等の標準 LlamaForCausalLM アーキ) は素の mlx-lm で動く。
# 本スクリプトが追加install する PrismML MLX fork (手順3) は legacy ternary-bonsai-8b
# 専用 (ternary カーネル対応) で、ternary-bonsai-8b 指定時のみインストールする
# (upstream mlx を置き換えるため既定では入れない)。

set -e

SCRIPT_DIR="$(dirname "$0")"
# shellcheck source=./model.env
. "$SCRIPT_DIR/model.env"

VENV_DIR="${HOME}/.venvs/bonsai-mlx"
MLX_MODEL="${MLX_MODEL:-$BONSAI_MODEL_MLX_REPO}"
PORT=8000

if [ -z "$MLX_MODEL" ]; then
    echo "エラー: BONSAI_MODEL_ID=$BONSAI_MODEL_ID には MLX ビルドがありません。BONSAI_MODEL_MLX_REPO または MLX_MODEL を指定してください" >&2
    exit 1
fi

echo "=== bonsai-agent MLX セットアップ ==="
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

# PrismML fork の MLX（legacy ternary-bonsai-8b の ternary カーネル対応。既定モデルでは未使用）
case "$BONSAI_MODEL_ID" in
    ternary-bonsai-8b)
        echo "3. PrismML MLX fork インストール..."
        pip install --quiet "mlx @ git+https://github.com/PrismML-Eng/mlx.git@prism"
        ;;
    *)
        echo "  (PrismML MLX fork は ternary-bonsai-8b 専用のため skip)"
        ;;
esac

# mlx-openai-server インストール
echo "4. mlx-openai-server インストール..."
pip install --quiet mlx-openai-server

# mlx-embeddings インストール（sidecar の /v1/embeddings 用、ローカル埋め込み）
echo "5. mlx-embeddings インストール..."
pip install --quiet mlx-embeddings

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
echo "  model_id = \"${BONSAI_MODEL_ID}\""
echo "  context_length = ${BONSAI_MODEL_CTX}"
echo ""
echo "動作確認:"
echo "  curl http://localhost:${PORT}/v1/models"
