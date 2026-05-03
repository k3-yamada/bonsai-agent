# Plan: Extended Tier (18 task) 品質改善

> **Multi-plan dispatch**: 項目 173 (extended baseline=0.3410, k=1) と項目 184 (Phase 5 同条件で再計測なし、core=0.7976 vs extended=0.3410 のギャップが -0.46 と大きい) を起点に、extended tier の score 構造を分析し改善 candidates を提案する設計 plan。**実装変更前に Phase 1 失点源 quantification が必須**（仮説駆動で task を弄ると ground truth が失われる）。

## Task Type

- [ ] Frontend
- [x] Backend (→ Codex)
- [ ] Fullstack

## Background

### 数値 fact

| Tier | Tasks | Baseline (k=1, MLX 04-29c) | Baseline (k=3, MLX 04-29) | gap |
|------|-------|---------|---------|-----|
| Core | 22 | 0.4763 | 0.7976→0.8131 (項目 184/185) | reproducible |
| Extended | 18 | 0.3410 | (未計測) | **-0.46 vs core** |
| All (40) | 40 | 0.4763 (Phase 5 暫定 = core only) | 0.5192 (v14 baseline, 1bit + bench 拡張) | extended pull-down |

### Extended 18 task の categorical 内訳 (`benchmark.rs:650-836`)

| Category | Count | Tasks (例) | 想定難度 (1bit) |
|----------|-------|------------|----------------|
| MultiStep | 6 | 3-file rename / 4-file sig change / 10-step chain / repomap+read+edit / grep+multiedit | 高 |
| CodeGeneration | 2 | FizzBuzz拡張 / (もう 1 件は MultiFileEdit) | 中 |
| ToolUse | 4 | MCP filesystem list / MCP search&replace / fact_check (Cargo.toml) | 中-高 (MCP 切離で 2 件不利) |
| Reasoning | 4 | nested logic / 2^8 mod 3 / 自己検算 / vague log | 中 |
| Summarization | 2 | multi-file role / git log | 中 |
| ErrorRecovery | 2 | tool fail pivot / corrupt JSON repair | 中 |

### 失点源仮説

| ID | 仮説 | 根拠 | 検証コスト |
|----|------|------|-----------|
| HX1 | MCP 依存 2 task が tool 不在で keyword-only 採点に格下げ | 項目 180 で MCP filesystem 恒久切離 | 低 (log 確認) |
| HX2 | max_iterations 過大 (10steps_chain は max=10、4-file sig は max=8) で 1bit モデルに不利 | 1bit は 5-6 step 後の精度劣化が顕著 | 中 (per-task score 集計) |
| HX3 | 4-file sig change / 3-file rename は 1bit で不可能なクラスタ | task 設計が「ハーネスの限界 vs モデルの限界」を分離していない | 中 |
| HX4 | Reasoning task (2^8 mod 3 / nested logic) は 1bit でランダム水準 | 数値推論は 1bit の弱点 | 低 (実機 fact-check) |
| HX5 | extended の評価関数 (keyword + tool match) が core より厳格 | benchmark.rs の score 算出ロジック差分 | 中 (code 確認) |

## Technical Solution

### Phase 1 (必須): 失点源 Quantification

実装変更前に、extended 18 task の **per-task score** を実機計測 + dump し、HX1-HX5 のどれが支配的か確定。

#### Phase 1 Step a: TSV / DB から既存 task 別スコア抽出

```bash
# extended baseline は 04-29c 計測済 (k=1) → SQLite に記録あり
sqlite3 ~/Library/Application\ Support/bonsai-agent/agent.db \
  "SELECT json_extract(value, '$.task_results') FROM experiments
   WHERE composite_score < 0.5 ORDER BY rowid DESC LIMIT 1;" \
  | jq '.[]| {id: .task_id, score: .score, tools: .tools_used}'
```

(task_results JSON 構造は要確認、なければ `experiment_log.rs` 参照)

#### Phase 1 Step b: 不足なら直接実行 (k=1 で 28 min で完了見込み)

```bash
BONSAI_BENCH_TIER=extended cargo run --release -- --lab --lab-experiments 0 \
    2>&1 | tee /tmp/bonsai-llama/extended-quant.log

# 個別 task score を抽出
grep -E "task=.*score=" /tmp/bonsai-llama/extended-quant.log | sort -k3
```

#### Phase 1 Step c: 仮説判定

| 観察 | 確定仮説 |
|------|---------|
| MCP 2 task の score が 0.0-0.2 範囲 | HX1 |
| MultiStep 5+ step task の score が 0.1-0.3 範囲 | HX2 |
| Reasoning 計算系 task の score が 0.3-0.5 範囲 | HX4 |
| 全 task が 0.3 前後で平坦 | HX5 (評価関数差分) |

### Phase 2: 改善 candidates (Phase 1 結果次第で 1-2 件採用)

#### Option α: max_iterations を tier 別動的化 (HX2 hit 時)

```rust
// benchmark.rs::run_k or task 構築時
let actual_max_iter = match (task.tier, base_config.bitness_aware) {
    (TaskTier::Extended, true) => task.max_iterations.min(6),  // 1bit cap
    _ => task.max_iterations,
};
```

trade-off: 既存ベースラインとの直接比較不能化。代わりに **tier 別 cap を専用 config field 化** し、env override で旧値復元可能にする。

#### Option β: MCP 依存 task の expected_tools 削除 or 削除 (HX1 hit 時)

`benchmark.rs:732-752` の 2 task (`mcp_filesystem_list`, `mcp_search_replace`) を keyword-only に変更:

```rust
BenchmarkTask {
    id: "mcp_filesystem_list".into(),
    name: "MCP filesystem list (keyword-only)".into(),
    input: "filesystem MCP で `/tmp` ディレクトリの一覧を取得する例を示して".into(),
    expected_tools: vec![],  // ← 削除 (項目 180 で MCP 切離済)
    expected_keywords: vec!["filesystem".into(), "list".into()],
    max_iterations: 4,
    category: TaskCategory::Reasoning,  // ← ToolUse から変更
    tier: TaskTier::Extended,
},
```

または **tier から外す**（filesystem MCP 復活時に extended_tasks に再合流できる構造）。

#### Option γ: 4-file sig change を 2-file 版に縮小 (HX3 hit 時)

`benchmark.rs:660` の `sig_change_4files` を `sig_change_2files` に縮小、max_iterations=4 で安定化。

#### Option δ: Reasoning 計算 task の評価緩和 (HX4 hit 時)

「2^8 mod 3」task の expected_keywords を緩和: `["1", "256"]` → `["1"]` (1 つでも当たれば fully credit)。

### Phase 3: 採用候補の TDD + 実機評価

```rust
// 例: max_iterations cap test
#[test]
fn test_extended_tier_max_iter_cap() {
    let task = BenchmarkSuite::extended_tasks()
        .tasks
        .iter()
        .find(|t| t.id == "tool_chain_10steps")
        .cloned()
        .unwrap();
    let config = AgentConfig {
        extended_tier_max_iter_cap: Some(6),
        ..Default::default()
    };
    let actual_cap = effective_max_iter(&task, &config);
    assert_eq!(actual_cap, 6, "Extended cap が適用される");
}
```

実機評価:
- Phase 3a: smoke ではなく直接 extended_tasks() を実行 (k=1, 28 min)
- Phase 3b: 採用候補で extended baseline 計測 → +0.05 以上の改善があれば採用、未満なら revert

## Implementation Steps

### Step 1: [Phase 1 計測] extended baseline per-task dump

```bash
BONSAI_BENCH_TIER=extended cargo run --release -- --lab --lab-experiments 0 \
    2>&1 | tee /tmp/bonsai-llama/extended-quant.log
```

実行時間 ~28 min (MLX backend、k=1 想定)。

### Step 2: [Phase 1 解析] task 別 score 集計

```bash
# log から task 別 score 抽出 (実 log フォーマットは要確認)
awk '/^\[bench\] task=/ {match($0, /task=([^ ]+)/, a); match($0, /score=([0-9.]+)/, b); print a[1], b[1]}' \
    /tmp/bonsai-llama/extended-quant.log | sort -k2 -n > /tmp/bonsai-llama/extended-per-task.tsv
cat /tmp/bonsai-llama/extended-per-task.tsv
```

### Step 3: [Phase 1 判定] 失点源仮説確定

per-task score 表から HX1-HX5 のどれが支配的か判定。`.claude/plan/extended-tier-quality-improvement-quant.md` に分析結果を保存。

### Step 4: [Phase 2 設計] 採用 Option 選定

判定結果に応じて Option α/β/γ/δ から 1-2 件選定。優先順:

1. HX1 hit → Option β (低コスト + 副作用なし)
2. HX2 hit → Option α (構造的改善)
3. HX3 hit → Option γ (task 設計修正)
4. HX4 hit → Option δ (評価関数調整)
5. HX5 hit → 別 plan (evaluation function refactor) として切り出し

### Step 5: [Phase 3 実装] TDD で採用 Option 実装

採用 Option ごとに Red→Green→Refactor。詳細は Step 4 確定後に追記。

### Step 6: [Phase 3 検証] 改善幅計測

```bash
BONSAI_BENCH_TIER=extended cargo run --release -- --lab --lab-experiments 0 \
    2>&1 | tee /tmp/bonsai-llama/extended-improved.log
```

`baseline (0.3410) → improved` の差を計測。

### Step 7: [判定] 採否 Decision Gate (下記参照)

## Key Files

| File | Operation | Description |
|------|-----------|-------------|
| `src/agent/benchmark.rs:30-50` (TaskTier enum) | Inspect | tier 構造確認 |
| `src/agent/benchmark.rs:419-839` (default_tasks) | Modify (Option β/γ/δ 採用時) | extended task 設計修正 |
| `src/agent/benchmark.rs:843+` (run_k) | Modify (Option α 採用時) | max_iterations cap 実装 |
| `src/config.rs (AgentConfig)` | Modify (Option α 採用時) | `extended_tier_max_iter_cap: Option<u32>` 追加 |
| `~/Library/Application Support/bonsai-agent/config.toml [agent]` | Append (Option α 採用時) | cap 値を config 化 |

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| R1: 既存 baseline (0.3410) との直接比較不能化 | tier 別 cap を opt-in flag 化、default は **OFF** で旧挙動保持。改善版は環境変数 `BONSAI_EXTENDED_CAP=6` で起動 |
| R2: extended task を弄ることで Lab 変異の評価系が揺れる | Lab 中は extended を使わない (BONSAI_BENCH_TIER 未指定 = default 40 task or core 22 推奨)、extended 改善は Lab とは別評価軸 |
| R3: HX1-HX5 のどれにも該当せず Phase 1 で結論不能 | Phase 1 完了後に re-quantify で k=3 計測 (84 min)、ノイズ起因か判定 |
| R4: MCP filesystem 再導入時に extended が再度劣化 | Option β を採用しても (project memory item 180 復活時手順あり) keyword-only 化された task は MCP 復活で expected_tools 復元の手順を CLAUDE.md に追記 |
| R5: 4-file rename の 2-file 化で MultiStep 難度バランスが core 寄りに偏る | task 別 difficulty rating の設計を Phase 4 で別 plan 化 |

## Decision Gate

- **Phase 1 完了**: per-task score 表が得られ HX1-HX5 のどれが支配的か確定 → Step 4 へ
- **Phase 1 で結論不能**: k=3 で再計測 (1h+ 投資) or extended_tasks 構造分析を別 plan に切り出し
- **Phase 3 実機で baseline +0.05 未満**: 採用 Option を revert、別 Option に切替
- **Phase 3 実機で baseline +0.05 以上**: 採用、CLAUDE.md 項目追加 + handoff 反映

## Estimate

- Phase 1 (Step 1-3): 1-1.5h (実行 28 min + 解析)
- Phase 2-3 (Step 4-6): Option 採用次第、1.5-3h
- 全体: 3-5h (k=1 baseline 前提)

## YAGNI Fence

- 評価関数の全面 refactor (HX5 hit) は本 plan で扱わず、別 plan 化
- task 別 difficulty rating の体系化は本 plan で扱わず、結果次第で別 plan 化
- core_tasks() の改修は範囲外 (本 plan は extended のみ)

## SESSION_ID (for /ccg:execute)

- CODEX_SESSION: (none — Claude direct planning)
- GEMINI_SESSION: (n/a)
