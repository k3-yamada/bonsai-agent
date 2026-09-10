# Ruri v3 Embed Sidecar 仕様

**SSOT for contract**: 本ファイル  
**Decision**: [ADR-015](../decisions/ADR-015-ruri-v3-local-embeddings.md)  
**参照実装**: `scripts/ruri_embed_server/`

## 目的

bonsai-agent（Rust）の `HttpEmbedder` が叩く **内部完結** の OpenAI 互換 `/v1/embeddings` を、日本語特化モデル **Ruri v3** で提供する。外部マネージドベクトル DB は使わない。

## プロセス境界

```
bonsai (HttpEmbedder)
  BONSAI_EMBED_URL=http://127.0.0.1:8787
  BONSAI_EMBED_MODEL=cl-nagoya/ruri-v3-30m
        │  POST /v1/embeddings
        ▼
ruri_embed_server (本 sidecar)
  sentence-transformers → cl-nagoya/ruri-v3-30m
        │
        ▼
ローカル HF キャッシュ（初回 DL 後はオフライン可）
```

- 既定ポート **8787**（MLX chat sidecar の 8888 と分離）。
- チャット推論プロセスに埋め込みを同居させない（RSS・障害分離）。

## HTTP 契約

### `GET /health`

```json
{ "ok": true, "model": "cl-nagoya/ruri-v3-30m", "dim": 256, "loaded": true }
```

`loaded=false` はモデル未ロード（lazy load 前）でも 200 でよい。

### `GET /v1/models`

OpenAI 互換のモデル一覧。少なくとも設定中の Ruri モデル ID を 1 件返す。

### `POST /v1/embeddings`

**Request（OpenAI 互換 + 拡張）**:

```json
{
  "model": "cl-nagoya/ruri-v3-30m",
  "input": "来月の旅行の話",
  "input_type": "query"
}
```

| フィールド | 必須 | 説明 |
|---|---|---|
| `model` | 推奨 | 論理名。未対応 ID は 400 |
| `input` | 必須 | `string` または `string[]` |
| `input_type` | 任意 | 下記。省略時 `"semantic"` |

**`input_type` → prefix（Ruri v3 公式）**:

| `input_type` | 付与する prefix |
|---|---|
| `semantic` | （なし） |
| `topic` | `トピック: ` |
| `query` | `検索クエリ: ` |
| `document` | `検索文書: ` |

既に文字列が当該 prefix で始まっている場合は **二重付与しない**。

**Response**:

```json
{
  "object": "list",
  "data": [
    { "object": "embedding", "index": 0, "embedding": [0.01, 0.02] }
  ],
  "model": "cl-nagoya/ruri-v3-30m",
  "usage": { "prompt_tokens": 0, "total_tokens": 0 }
}
```

- 各 `embedding` の長さはモデル次元（30m なら **256**）。
- L2 正規化は **sidecar 側で実施**（Rust `HttpEmbedder` も正規化するが冪等でよい）。
- `data[].index` は入力順を維持するために付与する。

### エラー

| 状況 | HTTP |
|---|---|
| `input` 欠落 / 空 | 400 |
| 未知の `input_type` | 400 |
| 未知の `model`（厳密モード時） | 400 |
| モデルロード失敗 | 503 |

## env

| env | default | 説明 |
|---|---|---|
| `BONSAI_RURI_HOST` | `127.0.0.1` | bind |
| `BONSAI_RURI_PORT` | `8787` | port |
| `BONSAI_RURI_MODEL` | `cl-nagoya/ruri-v3-30m` | HF モデル ID |
| `BONSAI_RURI_DEVICE` | `cpu` | `cpu` / `mps` / `cuda` |
| `BONSAI_RURI_STRICT_MODEL` | `0` | `1` で request.model 不一致を 400 |
| `BONSAI_RURI_LOCAL_FILES_ONLY` | `0` | `1` で HF ネット取得禁止（エアギャップ） |

bonsai 側:

| env | 例 |
|---|---|
| `BONSAI_EMBED_URL` | `http://127.0.0.1:8787` |
| `BONSAI_EMBED_MODEL` | `cl-nagoya/ruri-v3-30m` |

## 呼び出し規約（bonsai）

| 用途 | `input_type` |
|---|---|
| ツール選択セマンティック | `semantic` |
| HybridSearch / recall のクエリ | `query` |
| remember / ingest / エピソード index | `document` |
| 嗜好クラスタ等 | `topic` |

**Phase 0（本仕様 + 参照実装）**: sidecar のみ。  
**Phase 1**: sidecar 実モデル疎通（`/health` + query/document embed）。  
**Phase 2**: `HttpEmbedder` / `HybridSearch` / `ensure_vec_table` が `input_type` を付与（ADR-015）。ツール選択は `semantic` のまま。

## 次元・モデル昇格

- Phase 1 既定は **30m / 256d** のみ。`DEFAULT_EMBEDDING_DIM` と一致。
- 70m+ へ上げる場合は **dim 変更 + 全ベクトル再 index** を同一リリースで行う。切捨て運用は非推奨（情報損失）。

## セキュリティ

- **localhost bind 既定**。LAN 公開しない。
- 認証なし（ローカル信頼境界）。必要なら将来 reverse proxy。
- 入力テキストをログに残さない（デバッグ時のみ明示フラグ）。

## 起動

```bash
# 依存導入（venv 推奨）
python3 -m venv .venv-ruri
source .venv-ruri/bin/activate
pip install -r scripts/ruri_embed_server/requirements.txt

# 起動
./scripts/start-ruri-embed.sh

# bonsai
export BONSAI_EMBED_URL=http://127.0.0.1:8787
export BONSAI_EMBED_MODEL=cl-nagoya/ruri-v3-30m
```

疎通:

```bash
curl -s http://127.0.0.1:8787/health
curl -s http://127.0.0.1:8787/v1/embeddings \
  -H 'Content-Type: application/json' \
  -d '{"model":"cl-nagoya/ruri-v3-30m","input":"頭痛で休んだ","input_type":"document"}'
```

## Phase 3 — MiniLM 対照（offline paired）

```bash
# sidecar 起動済み前提
./scripts/g_paired_ruri_phase3.sh
# 先頭 N 件のみ: ./scripts/g_paired_ruri_phase3.sh --limit 5
```

- Fixture: `scripts/ruri_embed_server/fixtures/ja_episodes_phase3.json`
- A: `sentence-transformers/all-MiniLM-L6-v2`（本番 fastembed と同系）
- B: 本 sidecar（`input_type=query|document`）
- 判定: `scripts/lab_v22_metric.py` 相当（mean Δ≥0.010 / Wilcoxon / Cohen's dz）。factcheck ゲートは N/A。
- **default 切替はしない**（ACCEPT 後も opt-in env）。

## 非ゴール

- チャット補完 API（`/v1/chat/completions`）の同居
- Pinecone / クラウド embed API へのフォールバック
- sqlite-vec の再導入（ADR-005）
