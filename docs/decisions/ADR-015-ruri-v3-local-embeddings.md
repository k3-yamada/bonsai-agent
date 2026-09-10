# ADR-015: ローカル日本語埋め込みに Ruri v3 を採用（内部完結・prefix 規約）

## Status: Proposed (2026-09-10)

## Context

個人アシスタント向け記憶（コア属性 / 意味記憶 / エピソード）では、日本語クエリと日本語文書の意味検索品質が recall を左右する。現行の既定ローカル埋め込みは:

- Rust `embeddings` feature → fastembed AllMiniLML6V2（英語寄り・ONNX/ort の build-time DL）
- または MLX sidecar → `mlx-community/all-MiniLM-L6-v2-4bit`

いずれも **日本語生活文脈の RAG** には次善。外部マネージドベクトル DB（Pinecone 等）は、オフライン・プライバシー・コストの点で「内部完結」方針と衝突する。

一方、既存の `HttpEmbedder`（`BONSAI_EMBED_URL` → OpenAI 互換 `/v1/embeddings`）は **クラウド非依存のローカル sidecar を差し込める**設計になっている（ADR-005 の「embedder 外部プロセス化」再検討条件とも整合）。

Ruri v3（`cl-nagoya/ruri-v3-*`、Apache-2.0、ModernBERT-Ja、最大 8192 tok）は日本語特化の一般埋め込みで、サイズ階層が明確:

| モデル ID | 次元 | 備考 |
|---|---|---|
| `cl-nagoya/ruri-v3-30m` | **256** | 既存 `DEFAULT_EMBEDDING_DIM=256` と一致 |
| `cl-nagoya/ruri-v3-70m` | 384 | dim 移行または切捨てが必要 |
| `cl-nagoya/ruri-v3-130m` | 512 | 同上 |
| `cl-nagoya/ruri-v3-310m` | 768 | 精度寄り・RSS 大。Phase 2 候補 |

公式利用では **入力種別ごとの日本語 prefix** が必須（付けないと retrieval 品質が崩れる）:

| 役割 | prefix |
|---|---|
| 意味そのもの | （空） |
| トピック / 分類 | `トピック: ` |
| 検索クエリ | `検索クエリ: ` |
| 検索文書 | `検索文書: ` |

## Decision

1. **埋め込みの一次経路は内部完結のローカル sidecar** とする。外部ベクトル DB（Pinecone 等）は採用しない。検索ストアは既存の SQLite（A-MEM / experiences）+ KnowledgeGraph + HybridSearch（FTS + 任意ベクトル + RRF）を維持する（ADR-005: sqlite-vec 本番常駐は再評価までしない）。

2. **第一採用モデルは `cl-nagoya/ruri-v3-30m`（256d）**。理由: 次元が現状契約と一致し、切捨てによる情報損失を避けられる。`ruri-v3-310m` 等への昇格は、日本語エピソード recall の paired / 手元ベンチで不足が実証されてから（ADR-003）。

3. **Ruri prefix 規約を SSOT 化する**（本 ADR + `docs/execution/ruri-embed-sidecar.md`）:
   - 呼び出し側は OpenAI 互換 body に任意拡張フィールド `input_type` を付けてよい:
     - `"semantic"` → prefix なし
     - `"topic"` → `トピック: `
     - `"query"` → `検索クエリ: `
     - `"document"` → `検索文書: `
   - 未指定時の sidecar 既定は `"semantic"`（ツール選択など非 retrieval 用途の破壊を避ける）。
   - recall / HybridSearch の **クエリ側は `query`、インデックス側は `document`** を必須とする（実装 Phase で `HttpEmbedder` または呼び出し元が付与）。

4. **sidecar 契約**は `docs/execution/ruri-embed-sidecar.md` を正とする。参照実装は `scripts/ruri_embed_server/`。ポート既定は **8787**（MLX chat sidecar 8888 と分離）。

5. **運用 env**:
   - `BONSAI_EMBED_URL=http://127.0.0.1:8787`
   - `BONSAI_EMBED_MODEL=cl-nagoya/ruri-v3-30m`（sidecar が解釈する論理名。実体はローカルキャッシュ）
   - 初回のみ Hugging Face から取得可。以降はキャッシュのみでエアギャップ運用可能にする。

6. **非ゴール（本 ADR）**:
   - Rust への ONNX/Ruri 直埋め（メンテコスト高。HTTP 境界を維持）
   - sqlite-vec の再配線（ADR-005 を覆さない）
   - 意味記憶の黙った自動マージや感情推定の本番化（別計画・確認ゲート必須）

## Consequences

**Positive**:
- 日本語エピソード recall の品質向上余地を、既存 `HttpEmbedder` を壊さず得られる。
- クラウド依存ゼロ。チャット用 MLX sidecar とポート分離でき、embed 単独の起動・停止が可能。
- 30m / 256d 採用で既存 dim 契約・hash fallback・テスト前提と整合。

**Negative / Trade-off**:
- sentence-transformers / PyTorch（または将来の MLX 変換）のプロセス常駐コスト。チャットモデルと同時起動時は RSS に注意（M2 16GB）。
- `input_type` は OpenAI 標準外。他社互換サーバでは無視されるため、**prefix 適用の責任境界を sidecar/仕様で固定**する必要がある。
- 70m 以上へ上げる場合は dim マイグレーション（再 index）が必須。

## Implementation Phases

| Phase | 内容 |
|---|---|
| **0（本 PR）** | ADR + sidecar 仕様 + 参照実装（prefix ヘルパ含む）+ runbook / INDEX 更新 |
| **1** | 手元で sidecar 起動 → `HttpEmbedder` 疎通。ツール選択は `semantic` のまま |
| **2** | recall / HybridSearch / remember 経路に `input_type=query|document` を配線 |
| **3** | 日本語エピソード fixture で MiniLM 対照。必要なら 130m/310m を **別 dim プロファイル**として評価（default 切替は paired） |

**Progress (2026-09-10)**:
- Phase 0: 完了（本ドキュメント + `scripts/ruri_embed_server/`）。
- Phase 1: 参照実装で `cl-nagoya/ruri-v3-30m` preload → `/health` dim=256、query/document embed 疎通、関連文書の cosine が非関連より高いことを確認。
- Phase 2: `EmbedInputType` + `Embedder::embed_typed` を追加。`HttpEmbedder` が `input_type` を送信。`HybridSearch::search` は Query、文書 index / linear scan / `ensure_vec_table` は Document。ツール選択セマンティックは従来どおり `embed()` = Semantic。

## Related

- ADR-002（Scaffolding > Model）
- ADR-003（Paired Evidence）
- ADR-005（sqlite-vec REJECT / embedder 外部化の再検討条件）
- ADR-009（recall/ingest 品質はハーネス側）
- ADR-013（ModelProfile — 推論モデル。埋め込みプロファイルは本 ADR が担う）
- `src/runtime/http_embedder.rs`（`BONSAI_EMBED_URL` / `BONSAI_EMBED_MODEL`）
- `docs/execution/ruri-embed-sidecar.md`（sidecar 契約の詳細 SSOT）
- `scripts/ruri_embed_server/`（参照実装）
