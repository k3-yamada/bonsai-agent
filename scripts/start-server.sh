#!/bin/sh
# bonsai-agent用llama-serverを起動する
# Usage: ./scripts/start-server.sh
set -e

BONSAI_DEMO="${BONSAI_DEMO:-$HOME/Bonsai-demo}"
BIN="$BONSAI_DEMO/bin/mac/llama-server"
MODEL="$BONSAI_DEMO/models/gguf/8B/Bonsai-8B.gguf"
HOST="127.0.0.1"
PORT="${PORT:-8080}"

if [ ! -f "$BIN" ]; then
    echo "エラー: llama-serverが見つかりません: $BIN"
    echo "  cd ~/Bonsai-demo && sh scripts/download_binaries.sh"
    exit 1
fi

if [ ! -f "$MODEL" ]; then
    echo "エラー: モデルが見つかりません: $MODEL"
    echo "  モデルをダウンロードしてください"
    exit 1
fi

echo "=== bonsai-agent llama-server ==="
echo "  モデル:  $(basename "$MODEL")"
echo "  URL:     http://$HOST:$PORT"
echo "  KV:      q8_0 (FP16比50%削減)"
echo "  Flash:   有効"
echo "  コンテキスト: 16384"
echo ""
echo "  Ctrl+C で停止"
echo ""

exec "$BIN" -m "$MODEL" \
    --host "$HOST" --port "$PORT" \
    -ngl 99 \
    -c 16384 \
    --cache-type-k q8_0 --cache-type-v q8_0 \
    --flash-attn on \
    --temp 0.5 --top-p 0.85 --top-k 20 --min-p 0.05 \
    --repeat-penalty 1.15 --repeat-last-n 128 \
    --no-penalize-nl \
    --reasoning-budget 0 --reasoning-format none \
    --chat-template-kwargs '{"enable_thinking": false}' \
    "$@"
