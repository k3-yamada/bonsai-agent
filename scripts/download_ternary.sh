#!/bin/sh
# Ternary Bonsai 8B ダウンロードスクリプト (legacy モデル用ラッパー)
# Usage: ./scripts/download_ternary.sh
#
# 現在の既定モデルは MiniCPM5-2B (scripts/download_model.sh)。
# 本スクリプトは後方互換のため残しており、BONSAI_MODEL_ID を ternary-bonsai-8b に
# 固定した上で download_model.sh に処理を委譲する。repo/file/dir 等の値は
# scripts/model.env の preset (BONSAI_MODEL_ID=ternary-bonsai-8b) からそのまま解決される。
set -e

SCRIPT_DIR="$(dirname "$0")"

BONSAI_MODEL_ID="ternary-bonsai-8b"
export BONSAI_MODEL_ID

exec "$SCRIPT_DIR/download_model.sh" "$@"
