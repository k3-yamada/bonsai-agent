# KG Lint Coverage Check — KnowledgeGraph 整合性 lint pass (項目 244 候補)

**状態**: planning-only (2026-05-18 起票)
**推奨度**: ★★ (Lab v20 structural finding 事前検出 + LLM Wiki Lint パターン適用)
**推定工数**: ~3-4h plan + Phase 1-3 (TDD strict) + ~30 min Phase 4 smoke
**起点**:
- Zenn 記事「LLMの真価は「要約」ではなく「複数ソース間の知識統合」」(2026-05、tsurubee 著)
- Karpathy LLM Wiki パターンの 4 軸 (Schema / Concept / Log / Lint) のうち **Lint 軸が bonsai-agent 未実装**
- 項目 241 Lab v20 structural finding (`matched=0 deterministic`、wall 19h 9m 投下後に発覚) の事前検出経路確立

---

## §1. 問題定義

### 1.1 Lab v20 で 19h 9m を投下した structural blunder
Lab v20 paired (10 cycle) は ACCEPT 基準 (a) Pearson r >= 0.3 を要求するが、KG seed と
benchmark task の構造的不整合 (halluc task のみで matched=0 必然) が paired 起動後に発覚。
ACCEPT 基準 (b) `total >= 1` は満たすが (a) が deterministic NaN。

**事前検出可能だった**: 起動前に以下の simple check で 100% detect 可能:
- KG seed の全 triple に対し対称な benchmark task が存在するか?
- benchmark task の expected_keywords が KG seed triple を 1 つ以上 hit するか?

### 1.2 bonsai-agent の Lint 未実装範囲
| 領域 | 既実装 | 未実装 (Lint なし) |
|------|--------|---------------------|
| KG (`memory/graph.rs`) | add_edge / find_path / contains_triple | 矛盾 triple 検出、orphan node 検出、benchmark coverage check |
| Vault (`knowledge/vault.rs`) | append-only md 蓄積 | 重複ページ検出、表記揺れ検出、孤立ページ検出 |
| Memory (`memory/store.rs`) | A-MEM 蓄積 | 矛盾 experience 検出、stale skill 検出 |

本 plan は **KG lint** に focus (最も urgent な Lab paired blocker 防止)。

### 1.3 LLM Wiki Lint パターン (Zenn 記事より転用)
> Lintによる一貫性検証 — 定期的なヘルスチェックで矛盾・孤立ページ・表記揺れを自動検出し、知識ベース品質を維持

bonsai 適用:
- **矛盾** = `find_conflicting_edges` で実装済 (factcheck.rs 経由)、ただし **proactive run なし**
- **孤立** = `KnowledgeGraph::orphan_nodes()` 未実装
- **表記揺れ** = entity normalize (case-insensitive 検索など)、現行は case-sensitive

---

## §2. 設計 — 3 案比較 (推奨 = 案 B)

| 案 | 内容 | 採否候補 |
|---|---|---|
| A | standalone test として `cargo test -- --include-ignored kg_lint` 経路、Lab 起動時は実行しない | ★ 最小 scope |
| **B** | Lab smoke startup に `lint_kg_for_lab_warning()` 注入、FAIL でも **warning 出力のみで Lab 続行** | ★★★ 推奨 |
| C | Lab smoke startup に lint gate、FAIL で abort | ★ 開発時厳しすぎ |

### 2.1 案 B (推奨): Lab smoke startup warning + ignored test

**変更**:

1. `src/memory/factcheck.rs` に `lint_kg_for_lab() -> LintReport` 新規:
   ```rust
   pub struct LintReport {
       pub conflicting_triples: Vec<(Triple, Triple)>,
       pub orphan_nodes: Vec<String>,
       pub uncovered_seed_triples: Vec<Triple>,
       pub case_variant_nodes: Vec<(String, String)>,
   }
   impl LintReport {
       pub fn is_clean(&self) -> bool { ... }
       pub fn warn_log(&self);  // tracing::warn! で各カテゴリ件数を出力
   }
   ```

2. `src/agent/experiment.rs` の `run_lab` 開始時に lint 呼出:
   ```rust
   if std::env::var("BONSAI_KG_FACTCHECK_ENABLED").is_ok() {
       let kg = ...;  // ephemeral or seed-only KG
       let report = factcheck::lint_kg_for_lab(&kg, &benchmark_suite);
       if !report.is_clean() {
           report.warn_log();  // Lab 続行、ただし log に明示
       }
   }
   ```

3. `BONSAI_KG_LINT_STRICT=1` env で gate mode (FAIL で abort) を opt-in 化:
   - 開発時は warning のみ、CI / paired Lab 起動時は strict gate

4. `cargo test --include-ignored kg_lint` で seed_kg_for_factcheck_lab 整合性検証:
   - 各 seed triple に対し対称 benchmark task 存在を assert
   - case-insensitive entity match coverage を assert

**Pros**:
- Lab v20 同種 blunder を事前検出 (warn_log で 19h 投下前に判明)
- 開発フローを阻害せず (warning のみ)、CI で gate 強制可能
- LLM Wiki Lint パターン適用、設計原則の整合性確保

**Cons**:
- benchmark_suite と factcheck の cross-module dependency 増 (mod 構造要 review)
- `BONSAI_KG_LINT_STRICT` 環境変数追加で env 表面積拡大

### 2.2 案 A (棄却): standalone test のみ
ignored test は明示実行が必要で、Lab 起動前の運用 protocol に組込む手間増。CI で run しても feedback delay が大きい。

### 2.3 案 C (棄却): strict gate (FAIL で abort)
開発時に lint warning を修正する前に Lab smoke を試したいケースで運用 friction。
案 B の `BONSAI_KG_LINT_STRICT=1` で必要な時のみ gate 化、これで十分。

---

## §3. 実装 — TDD strict 5 phase

### Phase 1 (Red) — 6 failing test

1. `t_lint_kg_detects_conflicting_triples`: KG に矛盾 triple (A->B, A->!B) seed 後 lint で `conflicting_triples.len() >= 1`
2. `t_lint_kg_detects_orphan_nodes`: relation を持たない node を `add_node` 後 lint で orphan 検出
3. `t_lint_kg_detects_uncovered_seed_triples`: seed triple のうち benchmark task expected_keywords で hit しないものを検出
4. `t_lint_kg_detects_case_variant_nodes`: "Bonsai-Agent" と "bonsai-agent" 両方 KG に存在で variant 検出
5. `t_lint_report_is_clean_returns_true_for_empty_kg`: 空 KG で clean=true
6. `t_lint_seed_kg_for_factcheck_lab_is_clean`: 現行 seed_kg_for_factcheck_lab の出力 + benchmark.smoke_tasks で clean (regression gate)

### Phase 2 (Green)

- `src/memory/factcheck.rs` に `LintReport` + `lint_kg_for_lab()` (~80 行)
- `src/memory/graph.rs` に `orphan_nodes()` / `case_variant_pairs()` helper (~30 行)
- 全 6 test PASS

### Phase 3 (Refactor)

- LintReport の `warn_log()` 経路を `tracing::warn!` から `[INFO][lab.lint]` prefix に統一
- docstring に項目 244 起源 + LLM Wiki Lint パターン参照
- clippy/fmt clean

### Phase 4 (Smoke G-8a/b/c)

| Gate | env | 期待 |
|------|-----|------|
| G-8a | `BONSAI_KG_FACTCHECK_ENABLED=1` | Lab 起動時 `[INFO][lab.lint]` 出力、clean=true で warn なし |
| G-8b | KG seed を意図的に破壊 (uncovered triple 1 件追加) + `BONSAI_KG_FACTCHECK_ENABLED=1` | warn_log に `uncovered_seed_triples.len()=1` 出力、Lab 続行 |
| G-8c | G-8b 同条件 + `BONSAI_KG_LINT_STRICT=1` | Lab abort (exit code 非ゼロ)、`Lint FAIL` メッセージ |

### Phase 5 (本番運用、別 plan)
- Lab v21 paired 起動前に G-8a check (clean=true 確認)
- 矛盾 detect 時は seed 修正 → 再 lint → paired 起動

---

## §4. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | benchmark_suite と factcheck の cross-module dependency で循環参照 | benchmark.rs → factcheck.rs の単方向 (factcheck から benchmark 参照は test のみ) |
| R2 | lint 実行時間が Lab startup を遅延 | KG seed = 8 triple 程度、orphan 検出も O(V+E) で <1 ms 想定 |
| R3 | `BONSAI_KG_LINT_STRICT=1` env 増で表面積拡大 | 既存 BONSAI_* env と整合 (BONSAI_KG_FACTCHECK_ENABLED 等)、env-gated default OFF |
| R4 | case variant detection が false positive 多発 | configurable case-sensitivity (`BONSAI_KG_LINT_CASE=strict\|loose\|off`)、default strict |
| R5 | uncovered_seed_triples の判定で benchmark task expected_keywords substring match が緩い | exact triple round-trip check 追加 (subject/predicate/object 全 hit) |

---

## §5. 期待効果

### Lab v20 同種 blunder 防止
Lab v20 投入 19h 9m を Lab paired 起動前の <1 sec lint で防止可能化。19h × 1bit GPU 電力
+ user attention 節約効果。

### LLM Wiki パターン整合
Karpathy LLM Wiki 4 軸 (Schema/Concept/Log/Lint) のうち未実装 Lint 軸を確立。Vault lint
(項目 245 候補) / Memory lint (項目 246 候補) への拡張 base 確立。

### KG 品質の継続的 visibility
lab.lint info log で paired 全 10 cycle 開始時に integrity status 出力、`audit_log` 経由
で長期的な KG quality trend 観測可能。

---

## §6. 起票候補項目

- **項目 244** = 本 plan の Phase 1-3 完遂 (lint_kg_for_lab + smoke G-8a/b/c)
- 項目 245 (将来) = Vault lint (孤立ページ + 表記揺れ検出)
- 項目 246 (将来) = Memory lint (矛盾 experience + stale skill 検出)

---

## §7. 依存 / 並行性

### 完遂前提
- Plan A 系列 (230 → 242) 完結 ✅
- 項目 242 Phase 4 G-7a/b PASS ✅

### 並行可
- Phase 5 Lab v21 paired と並行可 (本 plan は別 binary、Phase 1-3 は production code 変更ある)
- Lab v21 paired 起動後の wall 15-20h 中に Phase 1-3 完遂可能

### 排他
- benchmark.rs と factcheck.rs の 2 ファイル変更で、Phase 4 G-7c smoke 完了後に start 推奨

---

## §8. ロールバック戦略

- `lint_kg_for_lab` は新規 public function、削除で完全 rollback
- env-gated default OFF (`BONSAI_KG_FACTCHECK_ENABLED` 既存 enable 時のみ lint も実行)
- 完全 rollback = `git revert <commit>` で 1 commit reversal

---

## §9. Quick Start

```bash
cd /Users/keizo/bonsai-agent
git log -3 --oneline

# Phase 1 Red
$EDITOR src/memory/factcheck.rs  # LintReport + lint_kg_for_lab() 仕様
cargo test --lib --quiet kg_lint 2>&1 | tail -10  # 6 FAIL

# Phase 2 Green
cargo test --lib  # 1286 → 1292 passed (+6)
cargo clippy -- -D warnings
cargo fmt -- --check

# Phase 3 Refactor + commit
git add -A && git commit -m "feat(factcheck): 項目 244 KG lint pass (LLM Wiki Lint パターン適用)"

# Phase 4 Smoke G-8a/b/c
cargo build --release

# G-8a: 既存 seed で clean=true
BONSAI_LAB_SMOKE=1 BONSAI_KG_FACTCHECK_ENABLED=1 \
  ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/g8a.log
grep "lab.lint" /tmp/g8a.log  # clean=true

# G-8b: 故意破壊 (test fixture で uncovered triple 注入) → warn 検出
# (test fixture 経由のため smoke ではなく unit test で代替検証推奨)

# G-8c: strict gate で abort
BONSAI_LAB_SMOKE=1 BONSAI_KG_FACTCHECK_ENABLED=1 BONSAI_KG_LINT_STRICT=1 \
  ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/g8c.log
echo "Exit: $?"  # 0 (clean なら正常)、もし破壊版なら非ゼロ確認

# 完了後の reporting
git log -1 --stat
```

---

## §10. 参考

- Zenn 記事「LLMの真価は「要約」ではなく「複数ソース間の知識統合」」 (https://zenn.dev/tsurubee/articles/llm-wiki-connecting-knowledge)
- `src/memory/factcheck.rs` (Plan A factcheck 親実装、本 plan の host module)
- `src/memory/graph.rs::find_conflicting_edges` (既存矛盾検出、本 plan の補助)
- `src/agent/benchmark.rs::SMOKE_TASK_IDS` (lint 対象 benchmark suite 参照)
- 項目 241 Lab v20 REJECT (本 plan の起源 structural finding)
- 項目 242 Lab v21 KG seed 拡張 (本 plan で coverage check 対象)
