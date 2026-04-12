#!/bin/sh
set -e
SRC="${HOME}/.local/share/bonsai-agent/vault"
DST="${1:-${HOME}/Documents/Obsidian/bonsai-agent}"
[ ! -d "$SRC" ] && echo "エラー: $SRC なし" && exit 1
[ -e "$DST" ] && echo "既存: $DST" && exit 0
mkdir -p "$(dirname "$DST")"
ln -s "$SRC" "$DST"
echo "リンク: $DST -> $SRC"
