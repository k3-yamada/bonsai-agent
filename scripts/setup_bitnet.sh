#!/bin/bash
# BitNet.cpp セットアップスクリプト
# Usage: ./scripts/setup_bitnet.sh
#
# bitnet.cppはllama.cppフォーク上に構築されたllama-server互換HTTP APIサーバー。
# 2026-01最適化でARM M2で1.37-5.07x高速化、エネルギー55-70%削減。

BITNET_DIR="${HOME}/Bonsai-demo/bitnet"
MODEL_REPO="prism-ml/Ternary-Bonsai-8B-GGUF"
PORT=8090

echo "=== BitNet.cpp セットアップ ==="
echo "インストール先: ${BITNET_DIR}"
echo "ポート: ${PORT}"
echo ""

# 1. リポジトリクローン
if [ -d "${BITNET_DIR}" ]; then
    echo "[skip] BitNetリポジトリが既に存在: ${BITNET_DIR}"
    cd "${BITNET_DIR}"
    git pull --rebase 2>/dev/null || true
else
    echo "[1/4] BitNetリポジトリをクローン..."
    git clone --recursive https://github.com/microsoft/BitNet "${BITNET_DIR}"
    cd "${BITNET_DIR}"
fi

# 2. Python依存関係
echo "[2/4] Python依存関係をインストール..."
pip install -r requirements.txt 2>/dev/null || pip3 install -r requirements.txt

# 3. ビルド+モデル準備（i2_s量子化）
echo "[3/4] ビルド+モデル準備（i2_s量子化）..."
python setup_env.py --hf-repo "${MODEL_REPO}" -q i2_s 2>/dev/null || \
python3 setup_env.py --hf-repo "${MODEL_REPO}" -q i2_s

echo ""
echo "=== セットアップ完了 ==="
echo ""
echo "サーバー起動:"
echo "  cd ${BITNET_DIR}"
echo "  python run_inference_server.py --model models/${MODEL_REPO}/ggml-model-i2_s.gguf --port ${PORT}"
echo ""
echo "bonsai-agent config.toml 設定:"
echo "  [model]"
echo "  backend = \"bitnet\""
echo "  server_url = \"http://localhost:${PORT}\""
echo "  model_id = \"ternary-bonsai-8b\""
echo "  context_length = 65536"
echo ""
echo "速度比較:"
echo "  cargo run -- --diagnose  # サーバー接続+応答時間確認"
