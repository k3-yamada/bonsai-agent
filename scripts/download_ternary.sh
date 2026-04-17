#!/bin/bash
# Ternary Bonsai 8B ダウンロードスクリプト
# Usage: ./scripts/download_ternary.sh

MODEL_DIR="${HOME}/Bonsai-demo/models/gguf/8B"
HF_REPO="prism-ml/Ternary-Bonsai-8B-GGUF"
MODEL_FILE="Ternary-Bonsai-8B.gguf"

echo "=== Ternary Bonsai 8B ダウンロード ==="
echo "保存先: ${MODEL_DIR}/${MODEL_FILE}"
echo ""

# huggingface-cli チェック
if ! command -v huggingface-cli &> /dev/null; then
    echo "huggingface-cli が見つかりません。pip install huggingface_hub でインストールしてください。"
    echo "代替: curl -L https://huggingface.co/${HF_REPO}/resolve/main/${MODEL_FILE} -o ${MODEL_DIR}/${MODEL_FILE}"
    exit 1
fi

mkdir -p "${MODEL_DIR}"
huggingface-cli download "${HF_REPO}" "${MODEL_FILE}" --local-dir "${MODEL_DIR}"

echo ""
echo "=== ダウンロード完了 ==="
echo "llama-server で使用:"
echo "  llama-server -m ${MODEL_DIR}/${MODEL_FILE} --host 127.0.0.1 --port 8080 -ngl 99 -c 65536 --cache-type-k q8_0 --cache-type-v q8_0 --flash-attn on"
echo ""
echo "config.toml 設定:"
echo "  [model]"
echo "  model_id = \"ternary-bonsai-8b\""
echo "  context_length = 65536"
echo "  gguf_path = \"${MODEL_DIR}/${MODEL_FILE}\""
