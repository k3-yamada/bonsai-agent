#!/bin/sh
# bonsai-agent用モデルをダウンロードする (既定: MiniCPM5-2B)
# Usage: ./scripts/download_model.sh
# モデルを差し替える場合は BONSAI_MODEL_* env を上書きするか scripts/model.env を編集する。
set -e

SCRIPT_DIR="$(dirname "$0")"
# shellcheck source=./model.env
. "$SCRIPT_DIR/model.env"

if [ -z "$BONSAI_MODEL_GGUF_REPO" ] || [ -z "$BONSAI_MODEL_GGUF_FILE" ]; then
    echo "エラー: 未知の BONSAI_MODEL_ID=$BONSAI_MODEL_ID。BONSAI_MODEL_GGUF_REPO / BONSAI_MODEL_GGUF_FILE を指定してください" >&2
    exit 1
fi

echo "=== ${BONSAI_MODEL_ID} ダウンロード ==="
echo "リポジトリ: ${BONSAI_MODEL_GGUF_REPO}"
echo "保存先:     ${BONSAI_MODEL_DIR}/${BONSAI_MODEL_GGUF_FILE}"
echo ""

mkdir -p "${BONSAI_MODEL_DIR}"

if command -v huggingface-cli >/dev/null 2>&1; then
    huggingface-cli download "${BONSAI_MODEL_GGUF_REPO}" "${BONSAI_MODEL_GGUF_FILE}" \
        --local-dir "${BONSAI_MODEL_DIR}"
else
    echo "huggingface-cli が見つかりません。pip install huggingface_hub でのインストールを推奨します。"
    echo "curl での代替取得にフォールバックします。"
    DEST="${BONSAI_MODEL_DIR}/${BONSAI_MODEL_GGUF_FILE}"
    if ! curl -fL -o "${DEST}" \
        "https://huggingface.co/${BONSAI_MODEL_GGUF_REPO}/resolve/main/${BONSAI_MODEL_GGUF_FILE}"; then
        rm -f -- "${DEST}" 2>/dev/null || true
        echo "エラー: モデルのダウンロードに失敗しました: ${DEST}" >&2
        exit 1
    fi
fi

echo ""
echo "=== ダウンロード完了 ==="
echo "起動:"
echo "  ./scripts/start-server.sh"
echo ""
echo "config.toml 設定 (省略時は上記既定値と一致):"
echo "  [model]"
echo "  model_id = \"${BONSAI_MODEL_ID}\""
echo "  context_length = ${BONSAI_MODEL_CTX}"
echo "  gguf_path = \"${BONSAI_MODEL_DIR}/${BONSAI_MODEL_GGUF_FILE}\""
