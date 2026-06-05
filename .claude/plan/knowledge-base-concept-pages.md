# 知識基盤強化: 概念ページ (横断的知見の合成) 計画

**起票** 2026-06-05 / **起源** Karpathy LLM Wiki (zenn tsurubee / qiita 2do BRAIN) の最大 insight =「概念ページ＝合成成果物」
**制約** クリーンアーキテクチャ厳守 ([[feedback_clean_architecture]]) / Scaffolding > Model / 証拠ゲート (ADR-003 paired)
**production touch** あり (新規 `knowledge/concept.rs` + agent 合成経路、全 env-gated)

> 本計画は **fresh session で pickup 可能**な自己完結 doc。前提知識は memory `zenn_tsurubee_llm_wiki_learnings` / `qiita_2do_brain_learnings`、本 doc、現コードで足りる。

## 0. 実装ステータス (2026-06-05)

- ✅ **Phase 1** 完遂 — `src/knowledge/concept.rs`: `detect_concept_candidates` (純粋クラスタリング)、`ConceptCandidate`/`ConceptConfig`。TDD strict (Red `336f58d` / Green `72cc79c`)。
- ✅ **Phase 2a** 完遂 — `concept.rs`: `ConceptPage`/`theme_slug`/`render_concept_markdown`/`member_entries`、`vault.rs`: `write_concept_page`/`record_concept_to_graph`。commit `8ddcfd7`。
- ✅ **Phase 2b** 完遂 — `src/agent/concept_synthesis.rs`: `synthesize_concepts` (env-gated, raw 再読込, MockLlm 検証) + `config.rs::is_concept_synthesis_enabled` (`BONSAI_CONCEPT_SYNTHESIS`, default OFF)。TDD strict (Red `e2e0503` / Green `b88ea52`)。
- ✅ **Phase 4a** 完遂 — `concept.rs::knowledge_gap_sources` (未統合 source = 知識ギャップ、純粋)。commit `7f52225`。VaultLintReport 6 軸への field 追加は assertion 結合が重く、独立純粋関数として decouple (lint pass への wiring は ACCEPT 後)。
- 🔧 **Phase 4b 実装完遂 (起動待ち)** — §9.4 eval scaffold を TDD strict 実装 (Red `bb6bf13` / Green `97894f1`)。`config.rs::is_concept_eval_enabled` (`BONSAI_CONCEPT_EVAL`, default OFF) + `BenchConfig.concept_eval` + `BenchReport.concept_memories_indexed` + `run_benchmark(.., backend: Option<&dyn LlmBackend>)` ON 分岐 (pseudo entry 写像→`detect_concept_candidates`→`build_synthesis_messages`→実 backend `generate`→concept memory を同一 store に index) + `retrieved_ids` の category 分岐 (§9.2: concept は全 tags 寄与) + `longmemeval-bench --concept-eval` flag (実 MLX backend wiring + health check)。1482 passed / clippy clean / structural clean。**残 = §9.5 起動 + §9.3 paired 判定 (実 MLX、別 session 数時間)**。ACCEPT (R@5 改善) が出るまで `BONSAI_CONCEPT_SYNTHESIS` default OFF 維持。
- ⏸ **Phase 3 (recall premium) = Phase 4b ACCEPT 後** — `RecallTool` は現状 `db_path` のみ保持で vault root 非参照のため新規 wiring 要。概念ページは 1bit モデルで recall 劣化リスクあり (本 doc §6)、ACCEPT 前の wiring は計画違反のため未着手。

全 commit local のみ (未 push)。1480 passed / clippy clean / structural (DEP-001 層) clean / fmt clean、退行ゼロ。

## 1. なぜ概念ページか (gap 分析、コード接地済 2026-06-05)

LLM Wiki の 3 オペ (Ingest / Query / Lint) に対し bonsai の現状:
- **Lint 軸 = 4 重発展済** (`knowledge/vault_lint.rs` 32KB / KG lint 項目244 / drift 257)。
- **Query 軸 = 実装済** (`src/tools/memory.rs` recall/remember)。
- **Ingest 軸 = 品質ゲートとして再設計 plan 済** (`.claude/plan/vault-ingest-compile-separation.md`、deferred)。
- **graph 軸 = 実装済** (`src/memory/graph.rs` KnowledgeGraph、`Vault::record_to_graph`)。
- **❌ 概念ページ (横断的知見の合成) = 最大 gap**。記事曰く「サマリーだけなら NotebookLM で代替可、**概念ページの存在が肝**」。bonsai の Vault は category-stock 型 (`decisions/facts/preferences/patterns/insights/todos` の .md 追記、`knowledge/vault.rs`)、graph は DB triple での fact-check 中心で、**複数ソースを横断して「共通構造・分類・反例」を統合した人間可読な合成成果物が不在**。

**概念ページ = 「複数の vault entry / source を横断し、テーマ単位で『概要 + 横断的知見 + 未解決の問い』を LLM が合成した markdown ページ + graph ノード」。**

## 2. 現コード接地 (grounded)

- `knowledge/vault.rs`: `Vault` = category-stock。`append`/`append_all`/`read_category`/`record_to_graph(entry, graph)`。entry = `StockEntry{category, content, source}`。
- `memory/graph.rs`: `KnowledgeGraph` (add_node/add_edge/neighbors)。vault entry は `vault_entry`/`vault_category`/`source` ノードで既に graph に載る。
- `tools/memory.rs`: recall/remember (Query 軸)。
- layer 順: `... memory < knowledge < runtime < tools < agent < main` (`docs/architecture/module-layer-rules.md`)。

## 3. クリーンアーキテクチャ層マッピング (必須・厳守)

| 責務 | 層 | 内容 |
|---|---|---|
| 概念候補検出 (純粋クラスタリング) | **knowledge** (`concept.rs` 新規) | graph 隣接 / source 共起 / category 重なりで entry を概念クラスタにまとめる**決定的純粋関数**。LLM 非依存・TDD 容易。`ConceptCandidate` / `ConceptPage` 型定義 |
| LLM 合成 (横断的知見の生成) | **agent** | 候補のメンバ **raw entry を再読込** (要約の要約を防ぐ=案 I-5/再帰的要約劣化防止)、LLM backend で「概要 + 横断的知見 + 未解決の問い + `[[wikilink]]`」を合成 |
| 永続化 | knowledge 経由 (agent が呼ぶ) | 概念ページ markdown + `KnowledgeGraph` に `concept` ノード + member source への `synthesizes` エッジ |
| recall 統合 | **tools** (`tools/memory.rs`) | 概念ページを recall の優先結果に |
| lint 統合 | knowledge (`vault_lint.rs`) | 「どの concept にも属さない source = 知識ギャップ」検出 |

- **境界厳守**: LLM backend は `domain::llm::LlmBackend` port を agent 層で consume。knowledge 層には LLM を漏らさない (純粋ロジックのみ)。`concept.rs` は memory/knowledge より上に依存しない。
- 各 Phase 後 `cargo test --test structural` で層違反 0 確認。`WHITELIST_DEP` 安易追加禁止。

## 4. Phase 分割 (TDD strict、全 env-gated)

### Phase 1 — 概念候補検出 (knowledge 層、純粋、LLM 非依存)
- `knowledge/concept.rs`: `ConceptCandidate{theme_key, member_entry_keys, member_sources, score}`。
- 純粋関数 `detect_concept_candidates(entries, graph_adjacency) -> Vec<ConceptCandidate>`: 2+ source を横断する entry 群をクラスタ化 (共有 graph 隣接 / source 共起 / 高頻度 term)。閾値 (最小 source 数 2、最小クラスタサイズ) で足切り。
- TDD: 合成シナリオ (3 source が 1 テーマを共有 → 1 候補)、ノイズ (孤立 entry → 候補なし)。
- 完了: 決定的検出 + unit test 緑 + structural 緑。

### Phase 2 — 概念ページ合成 (agent 層、LLM、env-gated)
- `BONSAI_CONCEPT_SYNTHESIS=1` で有効、既定 OFF (後方互換)。
- 候補 → member **raw entry を再読込** → LLM backend で合成 (概要 / 横断的知見 / 未解決の問い / `[[source]]` 出典)。
- 出力: vault に `concepts/<theme>.md` (frontmatter: sources, updated_at, status=draft) + graph に concept ノード + `synthesizes` エッジ。
- **inline 出典必須** (全主張に `[[source]]`、推測禁止)。**raw 再読込で再帰的要約劣化を防ぐ** (案 I-5 を本経路で実現)。
- TDD: MockLlmBackend で合成テキスト固定 → ページ生成 + graph 記録を検証 (実 LLM 不要)。
- 完了: env OFF で no-op (後方互換) + mock 経路で合成・永続化 test 緑。

### Phase 3 — recall/Query 統合 (tools 層)
- `tools/memory.rs` recall が concept ページを**優先結果**として返す (横断的知見は単一 entry より高価値)。
- TDD: concept ありで recall 上位に出る。

### Phase 4 — Lint 統合 + 証拠ゲート
- `vault_lint.rs` に「concept 未カバー source = 知識ギャップ」軸追加 (記事の Lint「知識ギャップ提案」相当)。
- **証拠ゲート (ADR-003)**: `src/eval/longmemeval/` (LongMemEval-S 500Q baseline R@5=0.91 既存) で concept ページ ON/OFF の recall 品質を paired 比較。**R@5 改善が ACCEPT 条件**。改善無ければ env default OFF 維持 (Lab 変異と同じ規律)。

## 5. 統合される既存 gap (一石三鳥)
- **概念ページ** = 本計画の核 (最大 ROI gap)。
- **raw 再読込 (案 I-5 / 再帰的要約劣化防止)** = Phase 2 合成で raw entry を必ず再読込する設計に内包。
- **Query 軸強化** = Phase 3 で concept を premium recall 化。
- Ingest 品質ゲート (`vault-ingest-compile-separation.md`) は別 plan のまま (合成の入力品質を上げる前段、独立進行可)。

## 6. リスク / 落とし穴
- **1bit モデルの合成品質**: Bonsai-8B で横断的知見の合成が hallucination しうる → inline 出典必須 + vault_lint で concept の矛盾検出 (既存 lint 資産を再利用) + 証拠ゲートで recall 改善を定量確認。改善無ければ採用しない。
- **計算コスト**: 合成は LLM call → daemon/手動トリガ (常時実行しない)、env-gated。
- **概念の乱立**: Phase 1 の閾値 (最小 2 source) で足切り。多すぎる候補は score 上位 N に制限。
- **層侵犯**: LLM を knowledge 層に漏らさない (合成は agent 層)。structural test で担保。

## 7. 推奨着手順序
Phase 1 (純粋検出、低リスク・LLM 不要) → Phase 2 (mock で合成経路) → Phase 4 証拠ゲート (LongMemEval-S paired) で ACCEPT 確認 → Phase 3 recall 統合。**ACCEPT 出るまで env default OFF**。

## 9. Phase 4b Run-Spec (証拠ゲート、未実装 — 別 session/MLX で launch)

> 目的: 概念ページが LongMemEval-S retrieval の **R@5 を改善するか** を paired で定量し、ACCEPT 判定する。
> production/eval コードは現状未変更。本 spec は「実装 → 起動 → 判定」を決定的に再現するための完全手順。

### 9.1 データモデル橋渡し (実装時の決定事項、固定)
現 `run_benchmark` は 1 haystack session = 1 memory (`category="session"`, `tags=[session_id]`) を index し、`recall_any_at_k(retrieved_ids, answer_session_ids, k)` で評価 (`retrieved_ids` = 各 hit の `tags[0]`)。橋渡しは以下で固定:
- 各 entry の haystack session narrative を擬似 `StockEntry{content=narrative, source=session_id}` に写像。
- `detect_concept_candidates(pseudo_entries, ConceptConfig{min_sources:2,..})` で候補抽出 (entry スコープ ephemeral、cross-entry 汚染なし)。
- 各候補を **実 backend** で合成 (`build_synthesis_messages` → `generate`)、概念 memory を **同一 in-memory store** に index: `category="concept"`, `content=合成body`, `tags=member_session_ids` (JSON 配列)。

### 9.2 指標定義 (採用 1 案・固定)
- **採用**: 概念 memory が retrieve されたら、その `tags` の **全 member session_ids** を `retrieved_ids` に寄与させる (session memory は従来通り `tags[0]` のみ)。⇒「gold session を含む概念が surface したら hit」。`recall_any_at_k` 本体は不変、`retrieved_ids` 構築のみ category 分岐で拡張。
- **不採用**: 概念を half-credit (0.5) 計上案は、既存 metric を汚し解釈を難しくするため却下 (単純性優先)。
- k_values=[5,10,20] 据え置き、主判定は **R@5**。

### 9.3 paired プロトコル (ADR-003 準拠、項目 266/268 と同方法論)
- 同一 dataset・同一順序・同一 store seed で **OFF arm** (`BONSAI_CONCEPT_EVAL` 未設定) と **ON arm** (`=1`) を per-entry で走らせ、**per-entry R@5 の paired Δ** を収集。
- 集計: mean Δ / Cohen's dz / Wilcoxon signed-rank (既存 `scripts/lab_v22_metric.py` 系の判定器に倣う)。
- **ACCEPT 条件**: mean Δ(R@5) > 0 かつ dz が改善方向・noise floor (~5%) 超、Wilcoxon が destructive 方向でないこと。FAIL なら `BONSAI_CONCEPT_SYNTHESIS` default OFF 恒久維持 (unpaired の見かけ改善は採用しない)。

### 9.4 実装タスク (未着手、eval-only・production recall 不変)
1. `BenchConfig` に `concept_eval: bool` 追加 (env `BONSAI_CONCEPT_EVAL=1` から populate、default false)。
2. `run_benchmark` 内 ON 分岐で 9.1 の概念 memory を追加 index + 9.2 の `retrieved_ids` 拡張。real backend は CLI/env で注入 (eval は `agent::concept_synthesis::{build_synthesis_messages}` + `knowledge::concept::detect_concept_candidates` を consume。`eval` は LAYER_ORDER 外で層制約 OK)。
3. TDD: MockLlm で「ON arm が concept memory を corpus に追加する」plumbing を検証 (実数値は assert しない、`graph_fusion smoke` と同型)。
4. CLI: `--concept-eval` フラグ + 出力に arm 表示。

### 9.5 起動コマンド (実装後)
```bash
# dataset (one-time、既存 bin docstring 参照)
#   ~/.cache/bonsai-agent/longmemeval/longmemeval_s_cleaned.json

# OFF arm (baseline、MLX 不要・既存経路)
cargo run --release --bin longmemeval-bench -- --limit 100 > /tmp/concept_off.txt

# ON arm (要 MLX 起動: port 8888、prism-ml Ternary 2bit。real 合成のため数時間)
BONSAI_CONCEPT_EVAL=1 BONSAI_CONCEPT_SYNTHESIS=1 \
  cargo run --release --bin longmemeval-bench -- --limit 100 > /tmp/concept_on.txt
# per-entry paired Δ(R@5) を 9.3 の判定器に通す
```
- **計算量**: 500Q × 候補 1-3 × 1 合成 call。MLX 2-bit latency で **full は数時間**。まず `--limit 100` smoke → ACCEPT 兆候があれば full。
- **MLX 必須**: ON arm は real 合成必須 (MockLlm body では retrieval 信号が無意味)。OFF arm は MLX 不要。

### 9.6 完了基準
ON/OFF paired の R@5 比較で ACCEPT/REJECT を `docs/quality/lab-history.md` に追記 + 本 §0 を更新。ACCEPT 時のみ Phase 3 (production recall premium) 着手 + `BONSAI_CONCEPT_SYNTHESIS` default 化検討。

## 8. 関連
- memory: [[zenn_tsurubee_llm_wiki_learnings]] / [[qiita_2do_brain_learnings]] / [[knowledge_synthesis_2026_04_20]]
- plan: `.claude/plan/vault-ingest-compile-separation.md` (Ingest 品質ゲート) / `vault-status-state-machine.md` (項目254 完遂、status 軸)
- code: `knowledge/vault.rs` / `memory/graph.rs` / `tools/memory.rs` / `src/eval/longmemeval/` (証拠ゲート harness)
