# Plan: gbrain (Knowledge Graph + RAG) 知見の bonsai-agent port

> **由来**: Zenn 記事 https://zenn.dev/headwaters/articles/8bc4e8c3119fa3 「gbrain に学ぶ、Agent の記憶基盤と Knowledge Graph の作り方」 (koki takeishi、ヘッドウォータース、2026-05-10、~8,500 字) の deep dive 結果。Y Combinator CEO 開発の `gbrain` (TypeScript + PostgreSQL/PGLite + MCP server) の Knowledge Graph 設計 7 観点を bonsai-agent (1bit Bonsai-8B / M2 16GB / Rust 製自律エージェント) に **段階的 port** する戦略。
>
> **由来 research**: 本 session の Zenn 記事 deep dive (agent ID `a3dfe2428...`、bonsai 応用候補 5 個を優先度付きで判定済)
>
> **関連 plan**: `ds4-insights-port-impl.md` (外部 OSS port 構造手本) / `cerememory-decay-port-impl.md` (env opt-in pattern) / `cerememory-review-state-v12-impl.md` (Freshness 軸思想、Stage 2 と思想整合)

## Task Type
- [ ] Frontend
- [x] Backend (`memory/search.rs` ranking 拡張、`memory/graph.rs` provenance 列追加、`knowledge/extractor.rs` wikilink parser、`memory/store.rs` schema migration)
- [ ] Fullstack
- [x] Docs (CLAUDE.md 項目 224 / `memory/gbrain_alignment.md` 新規)

## 1. 背景

### 1.1 gbrain の設計核心 (Zenn 記事要点)
| 観点 | gbrain の選択 | 設計 trade-off |
|---|---|---|
| 知識抽出 | regex + parser (zero LLM calls) | 再現性 / コスト / backfill 容易性で LLM 抽出に勝る |
| データモデル | pages + content_chunks + **links** | links の `link_source` + `origin_page_id` + `origin_field` で provenance 保持 |
| 記憶層分離 | facts / pages / timeline / takes | 「全 embedding」を否定、種類ごとに保存先と検索方法を変える |
| Stale edge 削除 | extraction + reconciliation 二段階 | provenance 確認で「page が責任を持つ edge だけ更新」(全削除→再 INSERT 禁止) |
| Graph 用途 | (a) ranking signal `score *= 1 + 0.05 * log(1 + backlink_count)` (b) traversal (recursive CTE + visited array + depth ≤ 10) | PageRank ほど複雑でなく実用的 |
| コード/知識 Graph 分離 | `code_edges_chunk` (resolved) と `code_edges_symbol` (unresolved) | UX は `--near-symbol` で統合 |
| Multi-source / federation | `source_id + slug` で一意 + `federated=true/false` | default 検索参加可否制御 |
| セキュリティ | remote MCP write は auto-link skip | trust boundary 設計、retrieval ranking 操作防止 |

### 1.2 bonsai 「Scaffolding > Model」原則との整合度
**高**。記事の核心 = 「**LLM 抽出に頼らず deterministic な regex/parser/SQL CTE で大部分を実装**」「edge provenance / backlink boost / 記憶種別分離」は **すべて scaffolding 側の改善**。1bit モデル (推論コスト・誤抽出リスク高) で特に効く。
- 記事の "zero LLM calls" 思想 → 項目 47 (think 強制) / 項目 50 (フォールバック) / 項目 136 (ファイル内容確認) の延長線上
- 記憶層分離 → bonsai は既に Cerememory 三本柱 (項目 217-219) で完遂済 = **external validation** 候補

### 1.3 bonsai 既存実装範囲 (照合結果)
- `src/memory/graph.rs` `KnowledgeGraph` + `add_edge()` (現状 `source_id/target_id/relation/weight` のみ、**provenance 列なし**)
- `src/memory/search.rs` HybridSearch (FTS5 + ベクトル + RRF 融合のみ、**graph signal 未使用**)
- `src/memory/store.rs` SCHEMA_VERSION = 14 (本 session AgentFloor で V13→V14 bump)
- `src/knowledge/extractor.rs` `extract_stock()` (StockCategory 6 種、Vault md 入力対応、**wikilink/markdown link parsing 未実装**)
- `src/knowledge/vault.rs:73` `add_edge(source_id, entry_id, "extracted_from", 1.0)` (extracted_from 概念は既に存在、provenance の半分は揃っている)
- `src/tools/mcp_client.rs` MCP クライアント (現状 MCP 経由 memory ingestion 未実装、tool call 主体)
- Cerememory 三本柱 (項目 217-219、env opt-in pattern 確立)

## 2. 目的
1. **gbrain 哲学の体系的取込** — 5 候補 (Backlink Boost / Edge Provenance / 記憶層分離 validation / wikilink 抽出 / MCP trust boundary) を優先度判定し段階 port
2. **HybridSearch ranking の精度向上** — Stage 1 backlink boost で死蔵 graph signal を活性化、Lab paired t-test で 1bit モデル下の効果検証
3. **Stale edge 問題の根本解決** — Stage 2 edge provenance で Vault/Graph 同期時の古い edge 残存を解消、Cerememory ReviewState (項目 218) Freshness 軸と思想整合
4. **天井 7 連続打開仮説** — Stage 1/2 で context-level の構造変異を追加 (Lab v22 paired t-test 候補)、項目 215 Lab v17 REJECT 後の打開経路

### 非目標
- gbrain 自体の Rust port (TypeScript + PostgreSQL から発想抽出のみ、code 直接依存なし)
- gbrain の `pages`/`content_chunks` モデルへの全面移行 (bonsai は既に SQLite A-MEM + KnowledgeGraph で同等機能を実装済)
- PageRank 採用 (記事 9 章で gbrain も非採用、bonsai も同様)
- LLM ベース entity 抽出 (zero LLM calls 原則踏襲)
- Stage 1 完遂前の Stage 2 着手 (依存関係: Backlink Boost → Edge Provenance / 残 Stage は独立)
- 日本語 wikilink 抽出強化 (Stage 4、記事 15 章注意点 = regex 英語特化、defer)

## 3. 既存項目との関係
| 項目 | 関係 |
|---|---|
| 13 (KnowledgeGraph) | Stage 1 backlink boost で graph signal を ranking に活用 |
| 162 (HybridSearch RRF k=60) | Stage 1 で post-RRF 段階に backlink coefficient 追加 |
| 199 (A-RAG validation) | Stage 3 = gbrain external validation、A-RAG と同 pattern |
| 217-219 (Cerememory 三本柱) | 同 env opt-in pattern (`BONSAI_*_ENABLED`) で Stage 1/2 設計統一 |
| 218 (ReviewState V12 Freshness) | Stage 2 edge provenance は Freshness 軸の edge 適用 |
| 220-222 (sqlite-vec wiring 削除) | 外部 OSS 採否判定の前例、本 plan も同等の Lab paired smoke 必須 |
| 215 (Lab v17 天井 7 連続) | Stage 1/2 が context-level 構造変異候補、Lab v22 で paired t-test 検証 |
| AgentFloor V14 (本 session) | Stage 2 で V14 → V15 ALTER TABLE knowledge_edges 追加可能 |

## 4. 設計

### 4.0 Stage 構成と依存関係
| Stage | 候補 | 優先度 | 依存 | 工数 | 別 plan 化 |
|---|---|---|---|---|---|
| **Stage 1** | Backlink Boost ranking signal | ★★★ | なし | ~0.5 day | 本 plan で完結 (Phase 1-5 詳細) |
| **Stage 2** | Edge Provenance (`link_source`/`origin_node_id`/`origin_field`) | ★★★ | なし (独立、SCHEMA_V14 との依存のみ) | ~1 day | `gbrain-edge-provenance-impl.md` 派生起票 |
| **Stage 3** | 記憶層分離 validation (external validation only) | ★★ | なし | 30 min | 本 plan §4.4 で完結 (`memory/gbrain_alignment.md` 拡張) |
| **Stage 4** | wikilink/markdown link 抽出 | ★★ | なし | ~1 day | 派生 plan、ただし日本語制約あり defer 推奨 |
| **Stage 5** | MCP trust boundary (auto-link skip) | ★ | 将来 MCP memory ingestion 実装後 | — | 起票しない (MEMORY.md `mcp_integrity_design_reference.md` 化候補) |

### 4.1 Stage 1: Backlink Boost ranking signal (本 plan 主体)

#### 4.1.1 数式 (gbrain 9 章踏襲)
```
final_score = rrf_score * (1 + coef * ln_1p(backlink_count))
```
- `coef` default = 0.05 (gbrain と同値)
- `backlink_count` = `KnowledgeGraph::incoming` の edge 件数
- `ln_1p` は `ln(1 + x)` (Rust `f32::ln_1p()` 直接使用、`x=0` で 0、numerically stable)

#### 4.1.2 配線
**A. HybridSearch 拡張** (`src/memory/search.rs`):
```rust
pub struct HybridSearch {
    // 既存 fields
    backlink_boost_enabled: bool,
    backlink_boost_coef: f32,
}

impl HybridSearch {
    pub fn with_backlink_boost(mut self, coef: f32) -> Self {
        self.backlink_boost_enabled = true;
        self.backlink_boost_coef = coef.clamp(0.0, 1.0);
        self
    }

    fn apply_backlink_boost(&self, results: &mut [SearchResult], graph: &KnowledgeGraph) {
        if !self.backlink_boost_enabled { return; }
        for r in results.iter_mut() {
            let count = graph.incoming_count(&r.node_name).unwrap_or(0);
            let boost = 1.0 + self.backlink_boost_coef * (count as f32).ln_1p();
            r.score *= boost;
        }
    }
}
```

**B. KnowledgeGraph に `incoming_count(node_name)` 追加**:
- 既存 `incoming` query は edge 列挙、count のみ返す軽量 method 追加
- SQL: `SELECT COUNT(*) FROM knowledge_edges WHERE target_id = (SELECT id FROM knowledge_nodes WHERE name = ?)`

**C. env opt-in** (`BONSAI_BACKLINK_BOOST_ENABLED=1`):
- `HybridSearch::new()` で env 確認、未設定で従来挙動
- coef は env で override 可 (`BONSAI_BACKLINK_BOOST_COEF=0.05` default)

#### 4.1.3 Lab paired t-test 設計
- core 22 paired 5 cycle、項目 215 Lab v17 と同方式
- ON: `BONSAI_BACKLINK_BOOST_ENABLED=1`
- OFF: env 未設定
- ACCEPT: mean Δ ≥ +0.015 AND p < 0.1
- 別 plan `lab-v22-backlink-boost-effectiveness.md` 起票方針 (Stage 1 ACCEPT 後)

### 4.2 Stage 2: Edge Provenance (派生 plan で起票)

**派生 plan**: `.claude/plan/gbrain-edge-provenance-impl.md` (~600 行、Stage 1 ACCEPT 後または独立着手)

要点:
- SCHEMA_V14 → V15 で `knowledge_edges` に 3 列追加:
  - `link_source TEXT` (markdown / frontmatter / extracted_from / manual / mcp)
  - `origin_node_id INTEGER` (どの node の責任で created)
  - `origin_field TEXT` (どの field 由来 — `body` / `title` / `tags` / etc.)
- `add_edge()` signature 拡張 (Builder pattern または additive params)
- `prune_stale_edges_for_origin(origin_node_id, origin_field)` 新 API — 「page が責任を持つ edge だけ削除/更新」
- env opt-in: `BONSAI_EDGE_PROVENANCE_ENABLED=1`
- Cerememory ReviewState (項目 218) Freshness 軸と思想整合 = 古い関係の retire

### 4.3 Stage 3: 記憶層分離 validation (本 plan §4.4 で完結)

**docs 追記** (`memory/gbrain_alignment.md` 新規 ~150 行):
- gbrain 5 章「全 embedding を否定、種類ごとに保存先と検索方法を変える」を bonsai 既存実装と照合
- bonsai 該当: `MemoryStore` (一般記憶) / `experience.rs` (経験) / `skill.rs` (スキル) / `dreams.rs` (Dreaming) + Cerememory 三本柱 (decay / ReviewState / Working Memory Cap = 7±2)
- 結論: bonsai はすでに gbrain の主張する「分離」を **Cerememory 三本柱で完遂済**、本 candidate は **external validation** のみ (port 不要)
- 項目 199 (A-RAG validation) と同 pattern で記録、`memory/external_memory_oss_comparison_2026_05_09.md` に gbrain 行追加 (deterministic 抽出 + edge provenance + backlink boost 軸)

### 4.4 Stage 4: wikilink/markdown link 抽出 (派生 plan で起票、日本語制約あり)

**派生 plan**: `.claude/plan/gbrain-wikilink-extractor-impl.md` (~500 行、defer 推奨)

要点:
- `[Alice](people/alice)` / `[[people/alice]]` / `[[source-id:people/alice|Alice]]` / bare slug `people/alice` の 3 parser
- code block 内除外 (` ``` ... ``` ` / inline `code`)
- ホワイトリスト (env で制御): `people/companies/meetings/concepts/...`
- bonsai 適用: Vault md 保存時に `KnowledgeGraph::add_edge` 自動配線
- 日本語制約: 記事 15 章「regex は英語特化、日本語『A 社に所属』『B に出資』は拾えない」= bonsai は CLAUDE.md / handoff が日本語比率高、効果限定的
- defer 判断 (★★)、wait until 多言語対応の必要性が明確化

### 4.5 Stage 5: MCP trust boundary (起票しない、設計 reference)

**docs 追記** (`memory/mcp_integrity_design_reference.md` 新規 ~80 行、将来用):
- gbrain 13 章「remote MCP write は auto-link skip / local CLI write は enabled / trusted subagent は allow-list」
- bonsai 適用先 = 将来 MCP 経由 memory ingestion (項目 124 系拡張) 実装時
- 現時点は MCP は tool call 主体、memory ingestion 経路なし → port 不要
- `memory/external_memory_oss_comparison_2026_05_09.md` に注釈追加: 「MCP trust boundary 設計を gbrain から学習、将来 MCP memory ingestion plan 起票時に再評価」

### 4.6 SQLite / TSV / config への影響 (Stage 1 のみ)
- **SQLite**: 変更なし (Stage 1 は ranking 計算式のみ、既存 `knowledge_edges` table 利用)
- **TSV**: 変更なし
- **Config**: `~/.config/bonsai-agent/config.toml` に `[hybrid_search]` section 追加 (default 全 false で後方互換)
- **env**: `BONSAI_BACKLINK_BOOST_ENABLED=1` / `BONSAI_BACKLINK_BOOST_COEF=0.05`

## 5. TDD strict 5 phase (Stage 1 主体)

### Phase 1 — Red
新規 test 6 件 (`src/memory/search.rs` / `src/memory/graph.rs`):
1. `test_hybrid_search_default_no_backlink_boost` — env 未設定で `apply_backlink_boost` no-op
2. `test_with_backlink_boost_modifies_score` — `coef=0.05`、incoming=10 で score *= 1 + 0.05 * ln(11) ≈ 1.12
3. `test_backlink_boost_zero_count_no_change` — incoming=0 で score 不変 (`ln_1p(0)=0`)
4. `test_knowledge_graph_incoming_count_basic` — 3 edge 追加後 count=3
5. `test_knowledge_graph_incoming_count_zero_for_missing` — 存在しない node で 0 返す
6. `test_env_override_backlink_boost_enabled` — `BONSAI_BACKLINK_BOOST_ENABLED=1` で `is_backlink_boost_enabled()` true

期待: compile error or 全 fail で Red 確認。

### Phase 2 — Green
1. `KnowledgeGraph::incoming_count(node_name)` SQL 1 行実装 → test 4, 5 pass
2. `HybridSearch::with_backlink_boost(coef)` builder method → test 2 pass
3. `HybridSearch::apply_backlink_boost(results, graph)` → test 3 pass
4. `is_backlink_boost_enabled()` env reader (Cerememory 三本柱と同 pattern) → test 6 pass
5. `HybridSearch::search()` の post-RRF 段階に `apply_backlink_boost` 呼び出し追加 → test 1 pass

期待: 既存 1162 + 新規 6 = **1168 passed** / clippy 0 / fmt 0

### Phase 3 — Refactor
- `apply_backlink_boost` の coef 上限・下限 sanity check (`coef.clamp(0.0, 1.0)`)
- `is_backlink_boost_enabled()` を `crate::env` module に集約 (Cerememory 三本柱と同)
- docstring 整備 (項目 224 参照、gbrain 由来明記、`memory/gbrain_alignment.md` cross-link)

### Phase 4 — Smoke 検証 (3 段)
```bash
# G-4a: 既存経路 (env 未設定、後方互換)
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 期待: 1168 pass 維持、HybridSearch 既存挙動互換

# G-4b: backlink boost 有効化、smoke
BONSAI_BACKLINK_BOOST_ENABLED=1 BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 期待: smoke 完走、log で「backlink boost: enabled, coef=0.05」確認

# G-4c: paired smoke (1 cycle、core 22)
BONSAI_BACKLINK_BOOST_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 1
# 期待: ON run で stale signal が活性化、score 観測 (ACCEPT は別 plan Lab v22 で paired 5 cycle)
```

判定基準:
- ✅ G-4a: 既存経路 1168 passed 維持、duration ±5%
- ✅ G-4b: env 反映確認 (log 出力で coef=0.05 確認)
- ✅ G-4c: smoke 完走、score variance 範囲内 (Lab v22 で paired ACCEPT 判定)

### Phase 5 — Commit + handoff + CLAUDE.md 項目 224
5 commits:
1. `test(gbrain-backlink): Phase 1 Red — HybridSearch backlink boost + incoming_count test`
2. `feat(gbrain-backlink): Phase 2 Green — HybridSearch backlink boost ranking signal`
3. `refactor(gbrain-backlink): Phase 3 — env reader 集約 + coef clamp + docstring`
4. `docs(gbrain): memory/gbrain_alignment.md + Stage 3 validation`
5. `docs(claude.md): 項目 224 — gbrain 知見 Stage 1 backlink boost wiring 完遂`

## 6. API 影響
| API | 変更 | 後方互換 |
|---|---|---|
| `HybridSearch::new()` | 内部 backlink boost field 追加 (default false) | ✅ env 未設定で従来挙動 |
| `HybridSearch::with_backlink_boost(coef)` | 新 builder method | ✅ additive |
| `HybridSearch::search()` | 内部 post-RRF 拡張 | ✅ env 未設定で挙動同 |
| `KnowledgeGraph::incoming_count(node_name)` | 新 method | ✅ additive |
| env `BONSAI_BACKLINK_BOOST_ENABLED` / `BONSAI_BACKLINK_BOOST_COEF` | 新規 | ✅ default 未設定で既存挙動 |
| SQLite | 変更なし (Stage 1) | — |
| TSV | 変更なし | — |

**signature 変更ゼロ** — 全 additive、項目 205 のような必須化はなし。Cerememory 三本柱と同 pattern。

## 7. Risks / Mitigations
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| R1 | coef=0.05 が bonsai で過大/過小、score 大幅変動 | ranking 質低下 | (i) coef は env override 可 (ii) Phase 4 G-4c で coef 候補 (0.01 / 0.05 / 0.10) sweep 推奨 |
| R2 | 1bit Bonsai-8B variance >> Δ で statistical power 不足 | Lab v22 ACCEPT 判定不能 | (i) Lab v22 paired 5 cycle、Lab v17 と同 sample size (ii) secondary metric (stability_delta) 併記 (iii) REJECT 時は項目 222 と同経路で wiring 削除 |
| R3 | KnowledgeGraph::incoming_count が SQL N+1 で performance 劣化 | search latency 増 | (i) SQL は 1 query / result item、items 通常 ≤ 20 で無視可 (ii) Phase 4 で wall time 計測、+10% 以上なら batch query 化検討 |
| R4 | ln_1p(backlink_count) が高頻度 node で過剰 boost | popular node 偏在 | (i) coef 上限 1.0 clamp (ii) gbrain も同 formula 採用、実用例存在 |
| R5 | env race condition (concurrent test で env 不一致) | test 不安定 | (i) Cerememory 三本柱と同様 std::sync::OnceLock 使用回避、test ごとに env serial 設定 (ii) 既存 `BONSAI_*_ENABLED` 系の test pattern 踏襲 |
| R6 | 派生 Stage 2/4 の plan 起票忘れで本 plan が "isolated" 化 | 知見継続性低下 | (i) §10 完了条件 #9 で Stage 2 派生 plan 起票必須 (ii) INDEX.md 「📊 Lab effectiveness」/「🆕 外部 OSS 取込み」 section に Stage 1 ACCEPT 後の派生 plan trigger 明記 |
| R7 | gbrain 自体が gbrain 内のスキーマ進化を続ける可能性 | 設計 reference 陳腐化 | (i) bonsai は gbrain *発想* の port、code 直接依存なし (ii) Zenn 記事は public、follow up 容易 |
| R8 | 日本語比率高い bonsai handoff/CLAUDE.md で wikilink 抽出効果限定 | Stage 4 効果薄 | (i) Stage 4 は defer 判断 (本 plan §4.4 で明記) (ii) 多言語対応は別 plan (Stage 4 派生) |
| R9 | edge provenance 列追加で V14 → V15 migration が本番 DB 互換性破壊 | 既存 .bonsai/ で ALTER TABLE 失敗 | (i) Stage 2 派生 plan で migrate.rs 既存 pattern (V13→V14) 同形 (ii) Phase 2 で migration test 必須 (iii) PRAGMA user_version チェック必須 |

## 8. Quality Gates
- **G-1 Phase 1 Red**: 6 新規 test compile error or 全 fail
- **G-2 Phase 2 Green**: 6 新規 test PASS + 1162 維持 = **1168 passed** + clippy 0 + fmt 0
- **G-3 Phase 3 Refactor**: docstring 完備 + helper 集約 + 既存 test 退行ゼロ + coef clamp test 1 件追加
- **G-4 Phase 4 Smoke 3 段**:
  - G-4a: 既存経路 1168 pass 維持、duration ±5%
  - G-4b: env 反映確認 (log 出力)
  - G-4c: paired 1 cycle smoke 完走 + score variance 範囲内
- **G-5 Final**: handoff 起票 + CLAUDE.md 項目 224 + `memory/gbrain_alignment.md` + Stage 2 派生 plan 起票 trigger 明記 + Lab v22 paired t-test plan 起票方針

## 9. 完了条件 (Stage 1 のみ)
1. ✅ `KnowledgeGraph::incoming_count(node_name)` 実装
2. ✅ `HybridSearch::with_backlink_boost(coef)` + `apply_backlink_boost()` 実装
3. ✅ `BONSAI_BACKLINK_BOOST_ENABLED=1` env reader 実装
4. ✅ `BONSAI_BACKLINK_BOOST_COEF=0.05` env override 実装
5. ✅ 6 新規 test PASS、1168 passed 維持
6. ✅ smoke G-4a/b/c 全 PASS
7. ✅ `memory/gbrain_alignment.md` 新規 (Stage 3 同梱)
8. ✅ CLAUDE.md 項目 224
9. ✅ Stage 2 派生 plan (`gbrain-edge-provenance-impl.md`) 起票 trigger 文書化
10. ✅ Lab v22 paired t-test plan (`lab-v22-backlink-boost-effectiveness.md`) 起票方針

## 10. 見積もり
| Phase | 内容 | 時間 |
|-------|------|------|
| Phase 1 | Red — 6 test 追加 | 0.5h |
| Phase 2 | Green — incoming_count + builder + apply_boost + env reader | 1.5h |
| Phase 3 | Refactor — env module 集約 + coef clamp + docstring | 0.5h |
| Phase 4 | Smoke 3 段 (うち G-4c は smoke 1 cycle 実機 60-90 min) | 2.5h (実機 wall 1.5h) |
| Phase 5 | Commit + handoff + CLAUDE.md 項目 + memory/gbrain_alignment.md | 1.0h |
| Buffer | KnowledgeGraph SQL 最適化 + coef sweep | 1.0h |
| **合計** | | **~7h ≈ 0.5-1 day** |

派生 plan (Stage 2/4) は別 session、合計工数 +1 day + 1 day。

## 11. Quick Start
```bash
# 0. 既存 caller 全網羅
rtk grep -rn "HybridSearch" src/
rtk grep -rn "KnowledgeGraph::incoming" src/
rtk grep -rn "BONSAI_.*_ENABLED" src/  # Cerememory 三本柱の env pattern 確認

# 1. Phase 1 Red
$EDITOR src/memory/search.rs       # backlink boost test 追加
$EDITOR src/memory/graph.rs        # incoming_count test 追加
rtk cargo test --lib backlink_boost  # compile error or fail

# 2. Phase 2 Green
$EDITOR src/memory/graph.rs        # incoming_count 実装
$EDITOR src/memory/search.rs       # with_backlink_boost + apply_backlink_boost + env reader
rtk cargo test --lib  # 1168 passed

# 3. Phase 3 Refactor
$EDITOR src/env.rs (or src/memory/search.rs)  # is_backlink_boost_enabled() 集約
$EDITOR src/memory/search.rs       # coef clamp + docstring

# 4. Phase 4 Smoke
rtk cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0  # G-4a
BONSAI_BACKLINK_BOOST_ENABLED=1 BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0  # G-4b
BONSAI_BACKLINK_BOOST_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 1  # G-4c (60-90 min)

# 5. Commit + handoff + CLAUDE.md 項目 224 + Stage 2/4 派生 plan 起票方針
$EDITOR /Users/keizo/.claude/projects/-Users-keizo-bonsai-agent/memory/gbrain_alignment.md
$EDITOR /Users/keizo/bonsai-agent/CLAUDE.md  # 項目 224
```

## 12. 参考
- Zenn 記事: https://zenn.dev/headwaters/articles/8bc4e8c3119fa3 「gbrain に学ぶ、Agent の記憶基盤と Knowledge Graph の作り方」 (koki takeishi、ヘッドウォータース、2026-05-10)
- gbrain リポ (記事内 reference、TypeScript + PostgreSQL/PGLite + MCP server)
- bonsai 既存 plan: `cerememory-decay-port-impl.md` (Plan A、外部 OSS port pattern 確立)
- bonsai 既存 plan: `cerememory-review-state-v12-impl.md` (Plan B、env opt-in pattern + Freshness 軸)
- bonsai 既存 plan: `ds4-insights-port-impl.md` (本 session 起票、外部 OSS port 構造手本)
- bonsai CLAUDE.md 項目 13/162 (KnowledgeGraph + HybridSearch RRF)
- bonsai CLAUDE.md 項目 217-219 (Cerememory 三本柱、本 plan の port pattern 手本)
- bonsai CLAUDE.md 項目 220-222 (sqlite-vec wiring 採否経緯、Lab paired smoke の前例)
- 派生 plan (本 plan ACCEPT 後起票):
  - `gbrain-edge-provenance-impl.md` (Stage 2、独立着手可)
  - `gbrain-wikilink-extractor-impl.md` (Stage 4、defer 推奨)
  - `lab-v22-backlink-boost-effectiveness.md` (Stage 1 ACCEPT 後の Lab paired t-test plan)
- 既存 OSS 比較: `memory/external_memory_oss_comparison_2026_05_09.md` (gbrain 行追加候補)

---

## 13. ★★★ DRAFT WARNING (session 05-11b gap analysis、実装着手前必読)

> **status**: 本 plan は **draft / 設計再考必要**。INDEX.md status も同訂正済。
>
> **由来**: handoff `session_2026_05_11b_handoff.md` 後の gbrain plan deep-read で **major blocking gap 3 件発見**、即実装着手は危険。

### Major blocking gap (実装前に解決必須)
- **G-1 (★★★)**: plan §4.1.2 で `r.node_name` 参照、実 `SearchResult` field = `memory: MemoryRecord, score: f32, source: SearchSource`、`MemoryRecord` field = `id/content/category/tags/access_count/created_at` のみ、**`node_name` も `name` も両 struct に存在しない**
- **G-2 (★★★)**: HybridSearch (`MemoryStore` 系) と KnowledgeGraph (`knowledge_nodes/edges` 系) の **bridging 経路が design レベルで未定義**。`MemoryRecord.id` → `knowledge_nodes.name` の mapping 規則なし、両 store は独立で同期保証なし、HybridSearch 結果が graph に登録されている保証なし
- **G-3 (★★ minor)**: `HybridSearch<'a>` / `KnowledgeGraph<'a>` の lifetime parameter 整合 (compile 時 fix 可能、minor)
- **G-4 (★★ medium)**: `KnowledgeGraph::incoming_count(name)` の SQL は plan §4.1.2 part B に提示済だが、「name はどこから来るか」(memory.content から抽出? title? hash?) 未定義

### 設計再考の方向性候補
- **Option A**: `MemoryRecord` に `node_name: Option<String>` field 追加 (extractor で graph add_edge 時に同期)
- **Option B**: `memory.id` を `knowledge_nodes.name` として登録 (`name = format!("memory:{}", id)` 規約)、graph と memory を id 経由で 1:1 mapping
- **Option C**: HybridSearch の post-RRF で `memory.content` から `extract_top_term()` で代表 keyword 抽出 → graph lookup (heuristic、精度依存)
- **Option D**: gbrain Stage 1 を **Stage 1.5「bridging design」と Stage 1.6「backlink boost」に分割** し、bridging design plan 別途起票

### 推奨次 action (次 session で gbrain 着手前)
1. 上記 Option A-D を `/ccg:plan` agent で 30 min discussion + design 確定
2. plan §4.1 全面書き直し (bridging logic 明示、Phase 1-5 再設計)
3. version 2 として `gbrain-insights-port-v2-impl.md` 別 file 起票推奨 (現 v1 は思想 capture として保持)

### 優先度 downgrade
- handoff session 05-11b §6 task #7 (gbrain Stage 1) priority: ★★ → ★ (設計再考必要)
- 次 session 推奨着手は **G1 Critic (★★★ 優先順 1 位)** を先行、gbrain は設計再考後

**本 plan は思想 (gbrain Knowledge Graph 設計の bonsai 取込み候補) として保持、実装 plan としては未完**。
