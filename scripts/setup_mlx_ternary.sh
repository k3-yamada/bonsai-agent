#!/bin/bash
# bonsai-agent MLX (Apple Silicon) セットアップスクリプト
# Usage: ./scripts/setup_mlx_ternary.sh
# Mac M2/M3/M4 Apple Silicon向け
#
# 既定モデル (MiniCPM5-2B 等の標準 LlamaForCausalLM アーキ) は素の mlx-lm で動く。
# 本スクリプトが追加install する PrismML MLX fork (手順3) は legacy Ternary-Bonsai 系列
# 専用 (ternary カーネル対応) で、解決後の MLX repo (MLX_MODEL) が Ternary-Bonsai の
# ときのみインストールする (upstream mlx を置き換えるため既定では入れない)。

set -e

SCRIPT_DIR="$(dirname "$0")"
# shellcheck source=./model.env
. "$SCRIPT_DIR/model.env"

VENV_DIR="${HOME}/.venvs/bonsai-mlx"
MLX_MODEL="${MLX_MODEL:-$BONSAI_MODEL_MLX_REPO}"
PORT=8000

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

# PrismML fork の MLX（legacy Ternary-Bonsai 系列専用の ternary カーネル対応。既定モデルでは未使用）
# 判定は解決後の MLX repo (MLX_MODEL) 基準。BONSAI_MODEL_ID は見ない
# (未知の BONSAI_MODEL_ID でも MLX_MODEL を直接 Ternary-Bonsai 系列に指定すれば fork が入る)。
_mlx_lower=$(printf '%s' "$MLX_MODEL" | tr '[:upper:]' '[:lower:]')
case "$_mlx_lower" in
    *ternary-bonsai-*)
        echo "3. PrismML MLX fork インストール..."
        pip install --quiet "mlx @ git+https://github.com/PrismML-Eng/mlx.git@prism"
        ;;
    *)
        echo "  (MLX_MODEL=$MLX_MODEL は ternary ではないため PrismML fork を skip)"
        ;;
esac
unset _mlx_lower

# mlx-openai-server インストール
echo "4. mlx-openai-server インストール..."
pip install --quiet mlx-openai-server

# mlx-embeddings インストール（sidecar の /v1/embeddings 用、ローカル埋め込み）
echo "5. mlx-embeddings インストール..."
pip install --quiet mlx-embeddings

# MLX_MODEL 未指定なら venv・mlx-lm・mlx-openai-server・mlx-embeddings までは準備済みの
# 状態で打ち切る (起動コマンド案内にはモデル repo が要るため exit 1 する。呼び出し側の
# set -e 連鎖を維持する)
if [ -z "$MLX_MODEL" ]; then
    echo "エラー: venv と mlx-lm、mlx-openai-server、mlx-embeddings は準備済みです。BONSAI_MODEL_ID=$BONSAI_MODEL_ID には MLX ビルドがありません。BONSAI_MODEL_MLX_REPO または MLX_MODEL を指定してください。モデル指定がないため起動案内を中断します" >&2
    exit 1
fi

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
