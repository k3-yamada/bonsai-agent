#!/bin/sh
# bonsai-agent用llama-serverを起動する (既定モデル: MiniCPM5-2B)
# Usage: ./scripts/start-server.sh
# モデルを差し替える場合は BONSAI_MODEL_ID (または個別の scripts/model.env の
# BONSAI_MODEL_* 変数) を上書きする。
set -e

SCRIPT_DIR="$(dirname "$0")"
# shellcheck source=./model.env
. "$SCRIPT_DIR/model.env"

case "$BONSAI_ENABLE_THINKING" in
    true | false) ;;
    *)
        echo "エラー: BONSAI_ENABLE_THINKING は true か false のみ指定できます (現在値: $BONSAI_ENABLE_THINKING)" >&2
        exit 1
        ;;
esac

BIN="$BONSAI_LLAMA_SERVER_BIN"
MODEL="${BONSAI_GGUF_PATH:-$BONSAI_MODEL_DIR/$BONSAI_MODEL_GGUF_FILE}"
HOST="127.0.0.1"
PORT="${PORT:-8080}"

if [ ! -x "$BIN" ] && ! command -v "$BIN" >/dev/null 2>&1; then
    echo "エラー: llama-serverが見つかりません: $BIN"
    echo "  brew install llama.cpp"
    echo "  (または ~/Bonsai-demo/scripts/download_binaries.sh で legacy バイナリを取得)"
    exit 1
fi

if [ ! -f "$MODEL" ]; then
    echo "エラー: モデルが見つかりません: $MODEL"
    echo "  ./scripts/download_model.sh でダウンロードしてください"
    exit 1
fi

echo "=== bonsai-agent llama-server ==="
echo "  モデル:  $(basename "$MODEL")"
echo "  URL:     http://$HOST:$PORT"
echo "  KV:      q8_0 (FP16比50%削減)"
echo "  Flash:   有効"
echo "  コンテキスト: $BONSAI_MODEL_CTX"
echo "  thinking: $BONSAI_ENABLE_THINKING"
echo ""
echo "  Ctrl+C で停止"
echo ""

if [ "$BONSAI_ENABLE_THINKING" = "true" ]; then
    # <think> をインラインで返す。src/agent/parse.rs 側でパースする。
    exec "$BIN" -m "$MODEL" \
        --host "$HOST" --port "$PORT" \
        -ngl 99 \
        -c "$BONSAI_MODEL_CTX" \
        --cache-type-k q8_0 --cache-type-v q8_0 \
        --flash-attn on \
        --temp "$BONSAI_MODEL_TEMP" --top-p "$BONSAI_MODEL_TOP_P" --top-k 20 --min-p 0.05 \
        --repeat-penalty "$BONSAI_MODEL_REPEAT_PENALTY" --repeat-last-n 128 \
        --reasoning-format none \
        --chat-template-kwargs '{"enable_thinking": true}' \
        "$@"
else
    exec "$BIN" -m "$MODEL" \
        --host "$HOST" --port "$PORT" \
        -ngl 99 \
        -c "$BONSAI_MODEL_CTX" \
        --cache-type-k q8_0 --cache-type-v q8_0 \
        --flash-attn on \
        --temp "$BONSAI_MODEL_TEMP" --top-p "$BONSAI_MODEL_TOP_P" --top-k 20 --min-p 0.05 \
        --repeat-penalty "$BONSAI_MODEL_REPEAT_PENALTY" --repeat-last-n 128 \
        --reasoning-budget 0 --reasoning-format none \
        --chat-template-kwargs '{"enable_thinking": false}' \
        "$@"
fi
