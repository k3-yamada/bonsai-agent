# Step 9 (テストカバレッジ強化) 設計書

**Source:** `structural-improvements-v2.md` Step 9（P3、未着手）
**Date:** 2026-04-27
**Status:** 設計（Lab v13 完了後の着手判断）

---

## 現状

- **総 LOC**: 31,277（src/ 配下）
- **テスト件数**: 950 件（cargo test --lib）
- **未計測**: カバレッジ% 未取得
- **Step 9 plan**: tarpaulin 80% 目標、+50 テスト追加

## 戦略変更提案: tarpaulin → 軽量代替

### tarpaulin の問題

- macOS で動作不安定（過去レポートあり）
- 計測時にビルド時間 2-3 倍
- Lab 中はビルド競合不可

### 推奨代替: **cargo-llvm-cov**

```bash
cargo install cargo-llvm-cov     # 一度だけ
cargo llvm-cov --lib --html      # 計測 + HTML レポート
cargo llvm-cov --lib --summary-only  # 数値のみ
```

利点:
- macOS で安定動作（Apple Clang ベース）
- HTML レポートが見やすい
- ビルドオーバーヘッドが tarpaulin より少ない

## 計測対象優先順位

### ★★★ 高優先（巨大ファイル + 機能重要）

| ファイル | LOC | 推定既存テスト | 推定カバレッジ | 改善余地 |
|---|---:|---:|---:|---|
| `agent/benchmark.rs` | 1812 | ~30 | 推定 60% | run_k 周辺のエッジケース |
| `agent/experiment.rs` | 1779 | ~15 | 推定 40% | judge_gate / oracle feedback |
| `agent/error_recovery.rs` | 1278 | ~30 | 推定 70% | StructuredFeedback path |
| `tools/file.rs` | 1243 | ~25 | 推定 75% | fuzzy 9 戦略の境界条件 |
| `agent/compaction.rs` | 1182 | ~10 | 推定 50% | level3 引継ぎ + Preserved Thinking |

### ★★ 中優先（中規模、機能重要）

| ファイル | LOC | 既存推定 |
|---|---:|---:|
| `tools/mod.rs` | 1290 | ~20 |
| `tools/repomap.rs` | 1072 | ~15 |
| `runtime/model_router.rs` | 1003 | ~10 |
| `runtime/llama_server.rs` | 844 | ~5 |

### ★ 低優先（テスト充実 or 単純）

- `agent_loop/*` (8 ファイル): 950 中 68 件 = 7% を占有、十分
- `safety/*`: 04-23 で +4 テスト追加済（項目155）
- `memory/*`: ストア系、テスト充実

## 追加テスト案（+50 件目標）

### experiment.rs +12 件

- `judge_gate_check` 各 path（fail-open / threshold pass / threshold fail / empty scores）4件
- `extract_worst_reasoning` の境界（n=0/1/many、全 ACCEPT/全 REJECT 混在）3件
- `LabStagnationDetector` の adaptive trigger（Stagnation/VarianceCollapse 各種閾値）3件
- `add_worst_reasoning_insights` の oracle insight 注入 2件

### benchmark.rs +10 件

- `MultiRunTaskScore::from_scores` の 0/1/k=3/k=10 各種 4件
- `pass_consecutive_k` 計算の境界 2件
- `judge_gate` 失敗時 fail-open 検証 2件
- `BenchmarkSuite::run_k` のタスク並列実行検証 2件

### compaction.rs +10 件

- `compact_level1_with_scorer` 各メッセージ役割（System/User/Assistant/Tool）の重み付け 4件
- `level3` 引継ぎサマリーの「Resolved/Remaining」分類 3件
- `Preserved Thinking` 抽出（複数 think ブロック、ネスト、空）3件

### tools/file.rs +8 件

- fuzzy 戦略 9 個の優先順位（複数該当時の選択）3件
- `MultiEdit` のロールバック（1件失敗時の全戻し）2件
- truncate_tool_output の Unicode 安全境界（半角/全角混在）3件

### error_recovery.rs +5 件

- `StructuredFeedback::from_trial_summary` 各カテゴリ（Tool/Network/Parse 失敗混在）3件
- `decide_recovery` の Retryable / AuthFailure 分岐 2件

### その他 +5 件

- `model_router.rs` AdvisorRole 全 path 2件
- `llama_server.rs` SSE タイムアウトフォールバック 1件
- `repomap.rs` PageRank の収束判定 1件
- `tools/mod.rs` semantic select の cosine 計算 1件

## 実装手順

### 段階 1: 計測基盤（30分、Lab 中不可）

```bash
cargo install cargo-llvm-cov
cargo llvm-cov --lib --summary-only > /tmp/coverage-baseline.txt
```

### 段階 2: 優先 5 ファイルに +50 テスト（TDD、推定 8h）

- 各ファイル、Red → Green → カバレッジ確認のサイクル
- 1 セッションで 1 ファイル分 (10 件) 完結

### 段階 3: CI 統合（1h）

- `.github/workflows/coverage.yml` で PR ごとに計測
- 80% 未満ファイルを警告（fail はしない、漸進的改善）

## 採否判定ゲート

着手前に確認:

- [ ] Lab v13 完了 + ベースライン安定確認
- [ ] cargo llvm-cov 動作確認（`cargo install` 成功）
- [ ] 現状カバレッジ % 取得（基準点として）
- [ ] 80% 目標が現実的か（段階1の結果次第で 60-70% に下方修正）

## リスク

| リスク | 対策 |
|---|---|
| 80% 目標が現実的でない | 段階1の計測結果に応じて 60-70% に再設定 |
| 追加テストで Lab v13 ベースライン変動 | テストは MockLlmBackend ベースで実機影響なし |
| カバレッジ駆動で意味のないテストが増える | `#[allow(dead_code)]` 経路は除外、ビジネスロジック優先 |

## 結論

**Step 9 は Lab v13 完了後 + 構造改善 v3 の 1 段落として実施推奨**。

優先順位:
1. Lab v13 完了 → 結果分析
2. 構造改善 v3 (DiffStore 等)
3. **Step 9 (本書、+50 テスト)**
4. Step 8 (依存最適化、軽量実施)

合計工数 9h、目標 950 → 1000 テスト、推定カバレッジ 70-80%。

## 関連
- 親計画: `.claude/plan/structural-improvements-v2.md` Step 9
- 並行: `.claude/plan/step-8-dependency-eval.md`（Step 8 軽量化提案）
