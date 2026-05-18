# Dynamic Token Budget Compaction — Per-Memory-Type Budget Allocation (項目 248 候補)

**状態**: planning-only (2026-05-19 起票)
**推奨度**: ★★ (Zenn 記事の 4 ratio 配分を 1bit context 制約下で適用、効果は要 Lab 計測)
**推定工数**: ~3-4h plan + Phase 1-3 (TDD strict) + ~30 min Phase 4 smoke + Lab v22+ paired (~8h)
**起点**:
- Zenn 記事「LLMの記憶アーキテクチャ4種類を整理」(2026-05、kenimo49 著)
  <https://zenn.dev/kenimo49/articles/llm-memory-context-engineering-4-architectures>
- 提案: メモリ種別ごとの token budget 動的配分
  ```python
  budget_ratios = {
      "recent_buffer":         0.4,
      "conversation_summary":  0.3,
      "relevant_entities":     0.2,
      "knowledge_graph":       0.1,
  }
  # 関連性スコア × 優先度で動的調整、超過時は均等縮小
  ```
- bonsai-agent 現状: `compaction.rs` は 4 段 stage (L0/L1/L2/L3、`compact_if_needed` line 642)
  だが、メモリ種別ごとの ratio 配分は明示なし。`CompactionConfig` (line 10) は単一 budget。

---

## §1. 問題定義

### 1.1 現行 compaction の budget 設計
- `CompactionConfig.from_n_ctx_budget()` で n_ctx の **70% ratio** を bonsai 側全体 budget とする
- L0/L1/L2/L3 の段階 prune は閾値駆動 (size-based)、メモリ種別 (buffer / summary / entity / KG) ごとの ratio 配分なし
- 結果: KG search 結果が長文だと buffer (直近会話) が早期 prune される、または逆に buffer が長いと KG が圧縮される

### 1.2 Zenn 記事の主張
- buffer 40% / summary 30% / entity 20% / KG 10% を base ratio として固定し
- 「関連性スコア × 優先度」で dynamic 調整
- 超過時は scale factor で全枠均等縮小

### 1.3 bonsai-agent 適用余地
- **1bit Bonsai-8B の context_length = 12,288**、ratio 配分で各種別の安定動作領域を保証可能
- AgentHER (entity)、KG search RRF (KG)、Vault read_rules (summary) を統合 prompt の各セクションに割当てる際、現状は ad-hoc → 種別 budget で再設計可能
- Lab v20 structural finding (matched=0 deterministic) を補強する変動軸として、budget ratio もパラメータ化可能

---

## §2. 設計 — 3 案比較 (推奨 = 案 B)

| 案 | 内容 | 採否候補 |
|---|---|---|
| A | 固定 ratio (Zenn 記事そのまま 40/30/20/10) で実装、env で OFF/ON 切替 | ★ 最小、効果不明 |
| **B** | base ratio + 「関連性スコア × 優先度」動的調整、env-gated `BONSAI_DYNAMIC_BUDGET=1` | ★★★ 推奨 |
| C | base ratio + LLM 介在の自己診断で配分決定 (advisor で再計算) | ★ 1bit advisor 信頼性低 + cost 増 |

### 2.1 案 B (推奨)

**変更**:

1. `src/agent/compaction.rs` に新規型 + helper:
   ```rust
   #[derive(Debug, Clone)]
   pub struct BudgetRatios {
       pub recent_buffer: f32,         // default 0.4
       pub conversation_summary: f32,  // default 0.3
       pub relevant_entities: f32,     // default 0.2
       pub knowledge_graph: f32,       // default 0.1
   }

   #[derive(Debug, Clone)]
   pub struct AllocatedBudget {
       pub total: usize,
       pub buffer: usize,
       pub summary: usize,
       pub entities: usize,
       pub kg: usize,
   }

   impl BudgetRatios {
       pub fn allocate(&self, total: usize) -> AllocatedBudget;
       pub fn adjusted(
           &self,
           relevance: &MemoryRelevance,
           priority: &MemoryPriority,
       ) -> BudgetRatios;
   }

   pub struct MemoryRelevance {
       pub buffer: f32,    // 直近 N 往復、常に 1.0
       pub summary: f32,   // summary 直近 score
       pub entities: f32,  // entity hit rate
       pub kg: f32,        // graph path success rate
   }
   ```

2. `CompactionConfig` に `budget_ratios: Option<BudgetRatios>` 追加。`None` で従来挙動 (backward compatible)。

3. `compact_if_needed` を分岐:
   - `budget_ratios.is_some()` のとき → 各 ratio で個別 prune
   - `None` のとき → 既存 4 段 stage 動作

4. env gate:
   - `BONSAI_DYNAMIC_BUDGET=1` で `BudgetRatios::default()` 適用
   - `BONSAI_DYNAMIC_BUDGET_RATIOS="0.4,0.3,0.2,0.1"` で カンマ区切り override

5. 配分計算:
   - `allocate(total)` = `total * ratio` を 4 軸に分配、余りは buffer に寄せる
   - `adjusted(relevance, priority)` = base ratio × (1 + (relevance - 0.5) * α)、α=0.2、正規化で合計 1.0
   - 超過検出時は均等縮小 (`scale_factor = total / sum`)

**Pros**:
- 単一の `total` budget から 4 軸 prune の予算境界が明確化
- 1bit context での「KG 長文で buffer 死亡」「entity 増殖で summary 圧縮」を予防
- Lab paired で base ratio (40/30/20/10 vs 50/30/15/5 等) を試行錯誤可能 (新たな実験軸)
- 既存 backward compatible (env unset で挙動変化なし)

**Cons**:
- `MemoryRelevance` 計測の追加 overhead (entity hit rate / kg path success rate の集計)
- ratio adjusted の係数 (α=0.2) が arbitrary、Lab で tuning 要
- 4 軸 prune は 1 軸 prune より複雑、bug risk 増

### 2.2 案 A (棄却): 固定 ratio
動的調整なしでは 1bit 1 task ごとに必要 ratio が変動する状況に追従できない。base のみは MVP として Phase 1 で先行する余地はあるが、最終形には不適。

### 2.3 案 C (棄却): LLM 自己診断
1bit Bonsai-8B の advisor 経路は項目 226 R5 gate で Uncertain 92.3% という結果あり、自己診断信頼性低。cost も上乗せ。

---

## §3. 実装 — TDD strict 5 phase

### Phase 1 (Red) — 5 failing test

1. `t_budget_ratios_default_sums_to_one`: default ratio の合計 == 1.0 ±ε
2. `t_allocate_distributes_total`: `allocate(10000)` で buffer=4000/summary=3000/entities=2000/kg=1000、余り 0
3. `t_allocate_handles_remainder`: `allocate(10003)` で余り 3 が buffer に寄る (buffer=4003)
4. `t_adjusted_increases_high_relevance`: `adjusted({buffer:1.0, summary:0.3, entities:0.8, kg:0.2})` で entities ratio が base 0.2 より増加
5. `t_compact_if_needed_uses_budget_ratios_when_some`: `CompactionConfig.budget_ratios = Some(_)` で 4 軸個別 prune が走り、`None` で従来動作

### Phase 2 (Green)

- `src/agent/compaction.rs` に `BudgetRatios` / `AllocatedBudget` / `MemoryRelevance` 追加 (~100 行)
- `CompactionConfig` に `budget_ratios: Option<BudgetRatios>` 追加 + Default 実装更新
- `compact_if_needed` 分岐実装 (~50 行)
- env getter `dynamic_budget_ratios()` 関数 (~30 行)
- 全 5 test PASS、1294 → 1299 passed (+5)

### Phase 3 (Refactor)

- env getter SSOT 抽出 (`BONSAI_DYNAMIC_BUDGET` / `BONSAI_DYNAMIC_BUDGET_RATIOS` の 2 軸を 1 関数で読む)
- `MemoryRelevance::current()` を `agent_loop` から呼べる public helper 化
- clippy/fmt clean、項目 226 R5 cross-file env mutex pattern 適用 (test 並行性)

### Phase 4 (Smoke G-10a/b/c)

| Gate | env | 期待 |
|------|-----|------|
| G-10a | env unset | 1 cycle smoke (15 task) で従来挙動と完全一致 (`Δscore = 0`)、backward compat 確証 |
| G-10b | `BONSAI_DYNAMIC_BUDGET=1` | 1 cycle smoke で 4 軸 prune が走る (`[INFO][compaction.budget] buffer/summary/entities/kg=...`)、score 微変動 OK |
| G-10c | `BONSAI_DYNAMIC_BUDGET_RATIOS="0.5,0.25,0.15,0.10"` | override が反映され、log の ratio が指定値、score 計測 |

### Phase 5 (本番運用 / Lab v22+ paired)

- Lab v22 paired (15 task smoke × 5 cycle = ~8h) で env on/off Δscore 計測
- ACCEPT 基準: paired t-test Δ ≥ +0.005 (Lab v17 同基準)、または matched 軸 variance 増 (Pearson r ≥ 0.3)
- REJECT 時は base ratio の調整 (50/30/15/5 等) で再試行 1-2 回

---

## §4. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | MemoryRelevance の計測 overhead で latency 増 | Phase 2 で計測自体は <1ms (整数演算)、KG path success rate は graph search の既存統計を流用 |
| R2 | ratio adjusted の係数 α=0.2 が arbitrary、効果が出ない | env で `BONSAI_DYNAMIC_BUDGET_ALPHA=0.2` を expose、Lab で 0.1/0.2/0.3 を試行 |
| R3 | 4 軸 prune 追加で bug 混入 (既存 4 段 stage 動作との conflict) | env unset = 完全な従来動作 (Phase 4 G-10a で確証)、env on でのみ 4 軸 prune 走る |
| R4 | budget 超過時の均等縮小で重要 buffer も削られる | scale_factor 適用前に「最小確保量」(buffer は total*0.2 等) で floor、最後の手段として均等縮小 |
| R5 | Lab v22 で REJECT 時の knowledge debt | REJECT 結果も memory に記録、ratio (40/30/20/10) の Bonsai-8B 1bit における実証 not-good を確証 |

---

## §5. 期待効果

### 1bit context 制約下での安定性向上
12,288 token context で「KG 長文応答 → buffer 圧縮」のリスクを 4 軸 ratio 配分で構造的に予防。
1 task ごとに必要な ratio 変動を `MemoryRelevance` で動的調整。

### Lab v22+ paired の新規実験軸確立
ratio (base + α) 軸で paired runs を実施、Lab v20 structural finding (matched=0 deterministic) に
追加変動軸を導入。Pearson r 計算可能性が上がる可能性。

### Zenn 4 architecture 取り込みの実装証跡
記事提案の 4 ratio を実装することで、bonsai-agent の memory stack 設計が外部設計指針と整合
確証 (= 設計原則の妥当性根拠強化)。

---

## §6. 起票候補項目

- **項目 248** = 本 plan の Phase 1-3 完遂 (Dynamic budget config + smoke G-10a/b/c)
- 項目 249 (将来) = Lab v22 paired で base ratio search (40/30/20/10 vs 50/30/15/5 vs ...)
- 項目 250 (将来) = MemoryRelevance を AgentHER feedback loop に組込 (relevance 学習)

---

## §7. 依存 / 並行性

### 完遂前提
- 項目 244 KG lint 完遂 ✅ (compaction の KG 軸入力が clean な前提)
- 項目 245 Vault lint plan 起票済 (summary 軸入力 quality 検証経路)

### 並行可
- Smoke 15-task paired 5-cycle (2026-05-18 起動) 完走後に Phase 1 着手
- 項目 245 Phase 1-3 と本 plan Phase 1-3 はファイル独立 (vault_lint.rs vs compaction.rs)、並行可

### 排他
- compaction.rs の編集中は Lab paired 起動禁止 (production binary 不整合 risk)
- Phase 4 smoke は cargo build --release 後 (Lab 同時稼働不可)

---

## §8. ロールバック戦略

- 全変更は `CompactionConfig.budget_ratios: Option<BudgetRatios>` 追加 + env-gated 分岐のみ
- env unset = 完全な従来動作 (G-10a で確証)、即時 rollback 可
- 完全 rollback = `git revert <commit>` で 1-2 commit reversal
- 万一の merge 後 buggy → env unset で disable、bug fix 後再 enable

---

## §9. Quick Start

```bash
cd /Users/keizo/bonsai-agent
git log -3 --oneline

# Phase 1 Red
$EDITOR src/agent/compaction.rs  # BudgetRatios / AllocatedBudget / MemoryRelevance 仕様
cargo test --lib --quiet budget_ratios 2>&1 | tail -10  # 5 FAIL

# Phase 2 Green
cargo test --lib  # 1294 → 1299 passed (+5)
cargo clippy -- -D warnings
cargo fmt -- --check

# Phase 3 Refactor + commit
git add -A && git commit -m "feat(compaction): 項目 248 Dynamic budget ratios (Zenn 4 arch 配分)"

# Phase 4 Smoke G-10a/b/c
cargo build --release  # binary 更新
./target/release/bonsai --lab --lab-experiments 0 2>&1 | tail -20  # G-10a (env unset)
BONSAI_DYNAMIC_BUDGET=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | grep "compaction.budget"  # G-10b
BONSAI_DYNAMIC_BUDGET=1 BONSAI_DYNAMIC_BUDGET_RATIOS="0.5,0.25,0.15,0.10" ./target/release/bonsai --lab --lab-experiments 0 2>&1 | grep "compaction.budget"  # G-10c

# Phase 5 Lab v22 paired (別 session、~8h wall)
nohup ./scripts/lab_v22_paired_dynamic_budget.sh ./lab-v22-logs > /tmp/lab_v22_run.log 2>&1 &
# ~8h 後
python3 scripts/lab_v22_paired_ttest.py ./lab-v22-logs
```

---

## §10. metadata

- 起点 commit: `5108e44` (項目 244 final docs)
- 起点 article: <https://zenn.dev/kenimo49/articles/llm-memory-context-engineering-4-architectures>
- 関連 plan: `kg-lint-coverage-check.md` (項目 244)、`vault-lint-coverage-check.md` (項目 245)
- 関連 memory: `ternary_bonsai_paths_2026_05_19.md` (本 session)
- 想定 commit 範囲: 3-4 commits (Phase 1 / Phase 2 / Phase 3 / Phase 4 smoke + Phase 5 Lab harness)
- 想定 line 範囲: +200 行 / -10 行 (compaction.rs に集中、`mod` 構造影響なし)
