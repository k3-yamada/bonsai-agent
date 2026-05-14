# KG-Grounded Hallucination Check (Knowledge Graph 事実検証層)

**状態**: planning-only (未起票)、推奨度 ★★★ (項目 228 3-stream RRF graph fusion 完遂後の自然な拡張)
**推定工数**: ~6-8h (TDD strict 5 phase、SCHEMA migration 不要、純 additive)
**起点**:
- Zenn 記事「LLM の隣にファクトチェック係を置く ナレッジグラフ×LLM の実践ユースケース 7 選」(井本 賢、2026-05-06) usecase #7 ハルシネーション検出
- Zenn 記事「知識グラフを AI エージェントに与えるという挑戦 — EidoGraph で学んだこと」(edom18、2026-05-09) の confidence/weight 二軸分離
- 項目 228 (3-stream RRF graph fusion 完遂、`KnowledgeGraph::bfs_bidirectional` BFS R@10=0.986 達成済)
- 項目 161/201-205 AgentHER hindsight relabel の追補機構

## §1. 背景 — bonsai における事実検証の空白

### 既存の検証経路 (1bit 1B agent の信頼性スキャフォールディング)

| 機構 | 役割 | 検証対象 |
|---|---|---|
| Reflexion (項目 18) | 同一 LLM での self-verification | step outcome |
| G1 Critic 別 LLM (項目 226) | 別 temperature/prompt critic | step outcome |
| AgentHER hindsight relabel (項目 201-205) | failed trajectory の事後学習 | trajectory level |
| 3-stream RRF (項目 228) | FTS5/Hash/Graph 統合検索 | retrieval 段 |

### 未検証の経路 = **生成内容そのものの事実整合性**
- Bonsai-8B (1bit) は **fabricate (捏造) 傾向**が paper モデル比で顕著
- 既存検証は「ステップ妥当性」「軌跡再評価」「検索精度」をカバーするが、**LLM 出力テキスト内の trip-level 事実関係**は未検証
- bonsai は `src/memory/graph.rs` に `KnowledgeGraph` (BFS 双方向探索) を保有しながら、**生成検証への活用がない**

### 記事 #2 usecase #7 のコアアイデア
```
LLM 出力テキスト → トリプル分解 (Subj, Pred, Obj) → KG で path 検証
                                                ↓
                                        矛盾検出 → re-generate
```

これを bonsai の AgentHER hindsight relabel と同列の **hindsight fact-check** として組込む。

## §2. 設計 (3 案、推奨 = 案 B)

### 案 A: 全 LLM 出力に対する pre-emit 検証
agent_loop 内で LLM 出力を emit する前にトリプル分解 + KG 検証 → 矛盾なら retry。
- ✅ ハルシネーション完全防止
- ❌ 出力毎の overhead が線形 (k=3 × 21 task で +63 検証 call)
- ❌ KG 未収載 fact は false positive 多発 (一般知識を KG が網羅できない)
- ❌ 1bit small model は出力多様性が低く retry で改善余地小

### 案 B (推奨): Post-hoc Lab metric として記録、production 既存挙動不変
Lab cycle 末尾の AgentHER hook 直前で **失敗 trajectory のみ** にトリプル分解 + KG 整合性 score を付与。production agent_loop は変更なし。
- ✅ overhead は失敗 trajectory のみ (~3-5/cycle)
- ✅ Lab v20 paired t-test で effectiveness 検証可能
- ✅ KG 未収載 fact は `unknown` 分類 (false positive 回避)
- ❌ production 即時防止はできない (検証は事後)
- 妥当性: bonsai 設計原則「Scaffolding > Model」と整合、Lab で計測 → ACCEPT なら production 移行のフロー

### 案 C: MCP server 化して外部 fact-checker
fact-check を MCP tool として実装、agent が必要時に呼出。
- ✅ tool-use として自然
- ❌ 1bit model はツール選択判断が弱く、必要時にも呼ばない可能性
- ❌ MCP overhead が cycle 時間圧迫
- ❌ 案 B と比較した固有価値が薄い

## §3. TDD strict 5 phase 計画

### Phase 1 (Red) — 失敗 test 6 件
- `t_extract_triples_from_text_basic` (`"A is the parent of B"` → `("A", "parent_of", "B")` 抽出、todo!() panic)
- `t_extract_triples_handles_empty` (空文字列 → 空 Vec)
- `t_verify_triple_in_kg_match` (KG 内に triple 一致 → `FactCheckResult::Match`)
- `t_verify_triple_in_kg_unknown` (KG 内に該当 node なし → `FactCheckResult::Unknown` (NOT Mismatch))
- `t_verify_triple_in_kg_conflict` (KG 内に対立 edge 存在 → `FactCheckResult::Conflict`)
- `t_factcheck_env_opt_in_default_off` (`BONSAI_KG_FACTCHECK_ENABLED` unset → no-op)

### Phase 2 (Green) — 実装
- `src/memory/factcheck.rs` 新規 module (~250 行想定):
  - `pub struct Triple { subject: String, predicate: String, object: String, confidence: f64 }` (EidoGraph 由来 confidence)
  - `pub enum FactCheckResult { Match { path_len: usize }, Unknown, Conflict { conflicting_edge: String } }`
  - `pub fn extract_triples_from_text(text: &str) -> Vec<Triple>` (rule-based regex extraction、LLM 経由は遅いので Phase 5 別案検討)
  - `pub fn verify_triple_in_kg(triple: &Triple, kg: &KnowledgeGraph) -> FactCheckResult`
  - `pub fn is_factcheck_enabled() -> bool` (Cerememory 三本柱 pattern)
- `src/memory/graph.rs` 拡張:
  - `pub fn contains_triple(&self, subj: &str, pred: &str, obj: &str) -> Option<usize>` (BFS distance を返す)
  - `pub fn find_conflicting_edges(&self, subj: &str, pred: &str) -> Vec<(String, String)>` (同 subject + 同 predicate で異なる object)
- `src/agent/experiment.rs` AgentHER hook 直前で `run_factcheck_pass(failed_trajectories, &kg) -> FactCheckSummary` 呼出
- `src/agent/event_store.rs` `AuditAction::FactCheck` variant 追加 (overhead 計測用)

### Phase 3 (Refactor)
- `extract_triples_from_text` を `pure fn` として保持 (BFS 経路と無関係に test 可能)
- confidence/weight 二軸: extraction confidence は `0.0..1.0`、weight (graph 内重要度) は edge weight 経由で別途算出
- `pub struct FactCheckSummary { total: usize, matched: usize, unknown: usize, conflicting: usize, mean_path_len: f64 }` で集約

### Phase 4 (Smoke)
- G-4a (env unset = default OFF): bonsai 既存挙動互換確証 (factcheck 配線が agent_loop に副作用ゼロ)
- G-4b (BONSAI_KG_FACTCHECK_ENABLED=1 + SMOKE): 7 task で `[INFO][lab.factcheck]` log emit 確認、failed trajectory 0-3 件で `total>=1` 検証
- G-4c (smoke + hallucination-inducing task 1 件追加): 「Bonsai-8B は GPT-5 の派生モデルである」など false fact を含む回答誘発 → `conflicting>=1` または `unknown>=1` 期待

### Phase 5 (Effectiveness — 別 plan、Lab v20)
- paired t-test `BONSAI_KG_FACTCHECK_ENABLED` ON/OFF 5 paired cycle
- ACCEPT 基準: ON cycle で `conflicting + unknown` 集計が trajectory failure rate と相関 (Pearson r ≥ 0.3) = fact-check は実機 hallucination 検出能力獲得
- Lab v17 paired pattern (項目 215) を template に流用

## §4. 直接転用しない判断

### 記事 #2 案 #1-6 (KG-Enhanced RAG / Text2Cypher / GraphRAG 等)
- bonsai は既に項目 228 3-stream RRF で paper R@10=0.986 達成、KG 検索精度の上積み余地少
- Cypher 言語の導入は Rust 純度を毀損 (Neo4j 依存 = SQLite 単純構成からの逸脱)
- **転用しない**

### 記事 #3 EidoGraph 三層モデル (A/B/C 層)
- HumanLM 論文 (arxiv 2603.03303) 起点で「personality simulation」用、bonsai の汎用 agent と目的ずれ
- 三層 schema の port は YAGNI、本 plan は **C 層 (Experience/Evidence) 単層**で十分
- **思想だけ参考** (confidence/weight 二軸分離は採用)

### LLM ベース triple extraction
- 案として検討したが overhead が許容外 (1 trajectory あたり +1 LLM call、cycle 時間 +20%)
- Phase 2 では **rule-based regex extraction** (低 recall 高 precision) で開始
- Phase 5 で「regex で extract 不十分なら LLM call」のフォールバック検討、ただし separate plan

## §5. 期待効果 (仮説、Phase 5 で検証)

| 仮説 | 反証条件 (Phase 5) |
|---|---|
| H1: bonsai-8B 失敗 trajectory の 30% 以上に `Conflict` triple が含まれる | conflicting rate < 0.10 で REJECT |
| H2: `Unknown` rate は task カテゴリと相関する (一般知識 task > tool-use task) | カテゴリ間で `Unknown` rate 差 < 0.05 で REJECT |
| H3: Lab v20 で fact-check ON cycle は failure rate 検出能力 +20% | Pearson r < 0.3 で REJECT |

H1 反証なら **bonsai-8B の失敗原因は fact 不整合より tool selection / parsing 由来**と確証、scaffolding 重点を G1 Critic / AgentHER に集中する判断。
H1 成立なら fact-check を production 移行候補とし、Lab v21+ で `Conflict` 検出時の retry hook 検証 (案 A を後発で導入)。

## §6. 起票候補項目

- **項目 230** = 本 plan の Phase 1-4 完遂 + Phase 4 G-4a/b/c smoke
- **項目 231** (将来) = Lab v20 paired t-test ACCEPT/REJECT 判定

## §7. 依存 / 順序

### 前提 (完遂済)
- 項目 161 KnowledgeGraph BFS 実装 (基盤)
- 項目 201-205 AgentHER hindsight relabel pipeline (hook 場所の前例)
- 項目 228 3-stream RRF graph fusion (graph stream 経路確立)

### 推奨実装順序
1. **本 plan Phase 1-4 完遂** (~6-8h、本 session 完了後)
2. Lab v19 (frontier effectiveness、項目 229 plan §3 Phase 5) 並行可能 (排他リソース無し)
3. Phase 5 Lab v20 (fact-check effectiveness) を Lab v19 完走後に起動

### 排他なし (並行可能)
- 本 plan は production agent_loop に touch しないため、Lab v18 (G1 Critic) / Lab v19 (frontier) と並行起動可

## §8. 不要転用 (rejected)

- 記事 #2 GraphRAG コミュニティ検出 — Neo4j 依存で Rust 純度毀損
- 記事 #2 Text2Cypher — Cypher 言語導入が SQLite 単純構成と矛盾
- 記事 #3 HumanLM A 層 (Belief/Goal/Value) — personality simulation 用で汎用 agent と乖離
- 記事 #1 SIRA (BM25 only) — bonsai は 3-stream RRF で paper 比 R@10=+0.295 上回り、転用価値なし

## §9. ロールバック戦略

- production default OFF (`BONSAI_KG_FACTCHECK_ENABLED` 未設定で no-op)
- 失敗 trajectory にのみ追加 overhead、successful trajectory は影響ゼロ
- Phase 5 REJECT 確定時 = env opt-in のまま放置 (項目 213 ERL Heuristics Pool 同 pattern)
- Phase 5 REJECT + Lab v22+ で完全 dead-code 判定なら、別 plan で wiring removal (項目 216/222 pattern)

## §10. 補足 — 1bit 制約への配慮

- Triple extraction は **rule-based regex** で開始 = 1bit LLM の生成多様性低い特性に依存しない
- KG 検証は既存 BFS で完結 = 推論 cost ゼロ
- Phase 4 hallucination-inducing task 設計には **既知 false fact (Bonsai-8B vs GPT-5 系) を含む prompt** を採用 = 1bit model の典型 fabricate pattern を活用

## §11. Quick Start (Phase 1 着手時)

```bash
# 前提: 本 plan 起票後、frontier Phase 4 commit 完了済
cd /Users/keizo/bonsai-agent

# Phase 1 Red — 失敗 test 6 件追加
# (vcsdd-tdd 経路 or 通常 TDD で実装)
rtk cargo test --lib factcheck 2>&1 | tail -5   # 全 test 失敗確認

# Phase 2 Green — 実装
# src/memory/factcheck.rs 新規、graph.rs 拡張、experiment.rs hook 追加

# Phase 3 Refactor — confidence/weight 二軸
# clippy 0 / fmt clean 維持

# Phase 4 Smoke (要 llama-server 起動)
BONSAI_LAB_SMOKE=1 BONSAI_KG_FACTCHECK_ENABLED=1 ./target/release/bonsai --lab --lab-experiments=1

# Phase 5 別 plan で別途
```
