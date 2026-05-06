# Plan: Beyond pass@1 — RDC / VAF / Graceful Degradation 信頼性メトリクス拡張

> **由来**: arxiv 2026-03 "Beyond pass@1: A Reliability Science Framework for Long-Horizon LLM Agents" (2603.29231) を参考に、既存 `MultiRunTaskScore` (項目 1, pass^k) に第 3 軸 stability を追加する。CLAUDE.md 項目 198 / handoff 05-07 で「★★★ arxiv 高優先 10 件」のうち最も scope が小さく単一 PR で実装可能な候補として抽出。`research_arxiv_2026_05_07.md` 領域 2 引用: 「Beyond pass@1 (RDC/VAF) — `MultiRunTaskScore` 拡張、Lab stability metric 追加 (1 PR scope)」。

## Task Type

- [ ] Frontend
- [x] Backend (純メトリクス拡張、benchmark.rs / experiment.rs / experiment_log.rs)
- [ ] Fullstack

## Background

### なぜ pass^k 単独では不十分か

現在 Lab v15 baseline は `score=0.7837 / pass@k=0.9242 / pass_consec=0.9091` (項目 198、core 22 task, k=3) で評価されている。pass^k は「k 回中の成功率と最長連続成功率」を捉えるが、**以下の信頼性側面が観測できない**:

| 観点 | pass^k で見える | pass^k で見えない |
|------|-------------|---------------|
| 成功 / 失敗の二値 | ✓ pass_at_k | — |
| 連続性 | ✓ pass_consecutive_k | — |
| 平均品質 | ✓ mean_score | — |
| **iteration 軸での減衰** | ✗ | iteration 0→max_iterations で task 完了率がどう変化するか |
| **試行間 stability** | △ (variance あるが正規化なし) | baseline 比でどれだけ variance が増幅したか |
| **partial credit 距離** | △ (mean_score) | 「失敗したラン群が pass_threshold にどれだけ近かったか」 |

特に **1bit Bonsai-8B は variance が大きい** ことが項目 184 (MLX core 22 = 0.7976) → 項目 185 再現 (0.8131) の +0.0155 ばらつきや、項目 197 Layer 1 緩和 (+0.0226) が variance 境界に位置する事実から定量的に確認されている。**stability を独立軸で観測しないと「変異が真に効いたか / variance に埋もれたか」を識別できない。**

### Lab 採否判定の現状 (`Experiment::from_multi_results`)

```rust
// experiment_log.rs:179-211
let baseline_score = baseline.composite_score();
let experiment_score = experiment.composite_score();
let delta = experiment_score - baseline_score;
Self {
    accepted: delta > 0.0,           // ← 第 1 軸: mean_score delta
    pass_at_k: Some(...),            // ← 観測のみ、判定には使われない
    pass_consecutive_k: Some(...),
    score_variance: Some(...),       // ← 既に集計はされている
    ...
}
```

判定軸:
1. `delta > 0.0` (mean_score 差、現行 ACCEPT 基準)
2. judge_threshold (項目 163、Phase B2、`Option<f64>` で opt-in、本 plan 範囲外)

**本 plan は第 3 軸 stability の前段階としての metric 追加** にスコープ限定する (active gate 化は smoke データ蓄積後の別 plan)。

## 新メトリクス定義 (3 つ)

> **論文式の出典**: arxiv 2603.29231 v1 の abstract / 領域 2 要約 (research_arxiv_2026_05_07.md:46-50) ベース。**論文本文 (PDF / HTML) のフル取得未完で、precise な閉形式式は確認待ち**。本 plan の式は bonsai 既存データから導出可能な近似実装案であり、論文と差異が判明した場合は Phase 2c で式更新する。**論文式取得待ちの定義は表中で明記**。

### 1. RDC (Reliability Decay Curve) — iteration 軸減衰

**論文定義 (要約引用)**: task duration 軸での pass 率減衰関数。長尺 task で時間経過とともに信頼性がどう低下するかをカーブで表現。

**bonsai 適用案 (近似実装)**:

bonsai は wallclock duration を per-iteration で取らず、`iterations_used / iteration_budget` を持つ (benchmark.rs:59-79 `TaskScore.iterations_used`)。これを iteration 軸の proxy とし、k 回ラン内で「**iteration 数が増えるほど成功率が下がる**」傾向をスカラー化する。

```rust
// 擬似 Rust
fn reliability_decay(individual_scores: &[f64], iterations_used: &[usize], budget: usize) -> f64 {
    // iteration 比 [0, 1] でスコアを並べる
    // late-iteration の低スコアを penalize
    // formula (近似): RDC = 1 - corr(iteration_ratio, 1 - score)
    // 0.0 = 完全減衰 (late が必ず fail) / 1.0 = 減衰なし (時間に依らず安定)
}
```

**論文式取得待ち**: 論文の正準 RDC は survival function ベースの可能性が高い (S(t) = P[success | duration ≥ t])。bonsai では k=3 で sample size 不足のため、**Phase 2 では proxy としてスカラー指標 `reliability_decay` のみ追加**、curve 配列は v2 plan へ defer。

**前提**: `BenchmarkTask.max_iterations` (5-15)、`AgentLoopResult.iterations_used` がすでに存在 → 計測機構の追加なしで導出可能。task duration (wallclock) は `MultiRunBenchmarkResult.duration_secs` のタスク全体値しか持たない (per-run 個別 wallclock は計測されていない)。

### 2. VAF (Variance Amplification Factor) — baseline 比 variance 増幅率

**論文定義 (要約引用)**: ベースライン variance に対する変異後 variance の増幅率。

**bonsai 適用案**:

```rust
// 擬似 Rust
fn variance_amplification(
    baseline_variance: f64,
    experiment_variance: f64,
) -> Option<f64> {
    // VAF = experiment_var / baseline_var
    // 1.0 = 不変 / >1.0 = 不安定化 / <1.0 = 安定化
    // baseline_var が 0 (k=1 や全 run 同点) なら None
    if baseline_variance.abs() < 1e-10 { return None; }
    Some(experiment_variance / baseline_variance)
}
```

**正規化代替 (RSE)**: variance の絶対値は mean に依存するため、Relative Standard Error (`sqrt(var) / mean`) を補助メトリクスとして並置候補。

**前提**: `MultiRunTaskScore.variance` (k>1 で計算済) と `MultiRunBenchmarkResult.mean_variance()` (benchmark.rs:346-352) を再利用、新規計測ゼロ。

### 3. GDS (Graceful Degradation Score) — partial credit 距離

**論文定義 (要約引用)**: 部分成功 (partial credit) を考慮した degradation の度合い。失敗時に「どれだけ閾値に近かったか」を表現。

**bonsai 適用案**:

```rust
// 擬似 Rust
fn graceful_degradation(scores: &[f64], pass_threshold: f64) -> f64 {
    // 失敗 run のスコアの「pass_threshold への近さ」を平均
    // formula (近似): GDS = mean(score / pass_threshold) for score < pass_threshold
    //                 = 1.0 if すべて pass
    let failures: Vec<f64> = scores.iter().filter(|&&s| s < pass_threshold).copied().collect();
    if failures.is_empty() { return 1.0; }
    let avg_proximity: f64 = failures.iter().map(|s| s / pass_threshold).sum::<f64>()
        / failures.len() as f64;
    avg_proximity  // [0, 1)、1 に近いほど graceful
}
```

**前提**: `MultiRunTaskScore.individual_scores` (benchmark.rs:209) から導出、新規計測ゼロ。

## API spec

### `MultiRunTaskScore` 拡張 (benchmark.rs:198-280)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiRunTaskScore {
    // ── 既存フィールド (無変更) ───────────────────────────
    pub task_id: String,
    pub pass_at_k: f64,
    pub pass_consecutive_k: f64,
    pub mean_score: f64,
    pub variance: f64,
    pub individual_scores: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_response: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_trajectory: Option<Vec<String>>,

    // ── 新規 (項目 199、後方互換のため #[serde(default)]) ──
    /// RDC スカラー値 [0, 1]。1.0 = 減衰なし、0.0 = 完全減衰。
    /// **論文式取得待ち**: proxy 実装 (iteration_ratio と (1 - score) の負相関)。
    #[serde(default = "default_reliability_decay")]
    pub reliability_decay: f64,
    /// GDS [0, 1]。1.0 = 全 pass or 失敗時も threshold 近く、0.0 = 完全失敗。
    #[serde(default = "default_graceful_degradation")]
    pub graceful_degradation: f64,
    // 注: VAF は task 単位ではなく aggregate (composite) で算出するため、
    // task struct には含めない (R5 で詳述)。
}

fn default_reliability_decay() -> f64 { 1.0 }      // serde 後方互換用、旧データは「減衰なし」扱い
fn default_graceful_degradation() -> f64 { 1.0 }   // 同上
```

### `MultiRunTaskScore::from_scores` 拡張

iteration_used を引数追加するか、別メソッド `from_scores_with_iterations()` を新設するか **設計判断**:

```rust
// 案 A: 新メソッド追加 (推奨、既存 from_scores 100% 後方互換)
impl MultiRunTaskScore {
    pub fn from_scores(task_id: String, scores: Vec<f64>, pass_threshold: f64) -> Self { /* 既存 */ }

    /// 信頼性メトリクス込みのフル版 (項目 199)
    pub fn from_scores_with_metrics(
        task_id: String,
        scores: Vec<f64>,
        iterations_used: Vec<usize>,
        iteration_budget: usize,
        pass_threshold: f64,
    ) -> Self { /* 新規 */ }
}
```

`run_k` (benchmark.rs:843) の呼出側を `from_scores_with_metrics` に切替、`iterations_used` を `AgentLoopResult.iterations_used` (line 1088) からタスクごと収集する。`from_scores` は legacy entry として残置 (テスト/外部呼出影響ゼロ)。

### `MultiRunBenchmarkResult` composite メソッド 3 つ追加

```rust
impl MultiRunBenchmarkResult {
    // 既存: composite_pass_at_k / composite_pass_consecutive_k / composite_score / mean_variance

    /// 全タスクの平均 reliability_decay。
    pub fn composite_reliability_decay(&self) -> f64 {
        if self.task_scores.is_empty() { return 1.0; }
        let sum: f64 = self.task_scores.iter().map(|s| s.reliability_decay).sum();
        sum / self.task_scores.len() as f64
    }

    /// baseline result 比較で VAF を算出 (aggregate level)。
    /// `baseline.mean_variance() == 0` なら None。
    pub fn variance_amplification_vs(&self, baseline: &Self) -> Option<f64> {
        let bv = baseline.mean_variance();
        if bv.abs() < 1e-10 { return None; }
        Some(self.mean_variance() / bv)
    }

    /// 全タスクの平均 graceful_degradation。
    pub fn composite_graceful_degradation(&self) -> f64 {
        if self.task_scores.is_empty() { return 1.0; }
        let sum: f64 = self.task_scores.iter().map(|s| s.graceful_degradation).sum();
        sum / self.task_scores.len() as f64
    }
}
```

### `Experiment` 拡張 (experiment_log.rs:131-147)

```rust
pub struct Experiment {
    // 既存フィールド全保持
    pub experiment_id: String,
    // ...
    pub pass_at_k: Option<f64>,
    pub pass_consecutive_k: Option<f64>,
    pub score_variance: Option<f64>,
    pub prescreened: bool,

    // ── 新規 (項目 199) ───────────────────────────
    /// RDC composite (`MultiRunBenchmarkResult::composite_reliability_decay`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reliability_decay: Option<f64>,
    /// VAF (baseline.mean_variance に対する experiment.mean_variance の比)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance_amplification: Option<f64>,
    /// GDS composite
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graceful_degradation: Option<f64>,
    /// stability_delta = (1 - VAF) + (RDC_exp - RDC_base) + (GDS_exp - GDS_base)
    /// **本 plan では計算のみ、ACCEPT 判定には未使用** (active gate 化は別 plan)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stability_delta: Option<f64>,
}
```

### `Experiment::from_multi_results` 拡張

```rust
pub fn from_multi_results(
    experiment_id: String,
    mutation_type: MutationType,
    mutation_detail: String,
    baseline: &MultiRunBenchmarkResult,
    experiment: &MultiRunBenchmarkResult,
    config_snapshot: HashMap<String, String>,
) -> Self {
    let baseline_score = baseline.composite_score();
    let experiment_score = experiment.composite_score();
    let delta = experiment_score - baseline_score;

    // 項目 199: 信頼性メトリクス
    let vaf = experiment.variance_amplification_vs(baseline);
    let rdc_exp = experiment.composite_reliability_decay();
    let rdc_base = baseline.composite_reliability_decay();
    let gds_exp = experiment.composite_graceful_degradation();
    let gds_base = baseline.composite_graceful_degradation();
    let stability_delta = vaf.map(|v| (1.0 - v) + (rdc_exp - rdc_base) + (gds_exp - gds_base));

    Self {
        // ... 既存
        accepted: delta > 0.0,           // ← 判定変更なし (本 plan)
        reliability_decay: Some(rdc_exp),
        variance_amplification: vaf,
        graceful_degradation: Some(gds_exp),
        stability_delta,
        ..
    }
}
```

### TSV / SQLite 永続化 (experiment_log.rs:248-282)

**TSV**: 11 列 → 14 列 (3 列追加、`-` で None 表現、ヘッダ自動再生成)。**既存 TSV ファイルとの互換性**: 旧 11 列 TSV を読み込む経路は現状なし (TSV は append-only ログ専用、読み返しは SQLite から)。新規実行で旧 TSV に追記するとヘッダ不一致になる懸念があるため、**新セッションは新 TSV ファイルへ** という方針 (現状の Lab 起動でも tsv_path は `Option<PathBuf>`、user 設定で切替可能)。

**SQLite**: `experiments` テーブルに 4 列追加 (rdc / vaf / gds / stability_delta、すべて `REAL NULL`)。**スキーマ migration が必要** (V8 → V9)。

```sql
-- migration V9
ALTER TABLE experiments ADD COLUMN reliability_decay REAL;
ALTER TABLE experiments ADD COLUMN variance_amplification REAL;
ALTER TABLE experiments ADD COLUMN graceful_degradation REAL;
ALTER TABLE experiments ADD COLUMN stability_delta REAL;
```

`recent_experiments` (experiment_log.rs:285) と `save_to_db` (line 220) も 4 列追加対応 (`COALESCE(reliability_decay, NULL)` で旧データから読み返し可)。

## TDD strict 5 phase

### Phase 1 (Red): failing tests 作成

**File**: `src/agent/benchmark.rs` (tests module、line 1093 以降)

```rust
#[test]
fn t_rdc_perfect_no_decay() {
    // 全 run が iteration 0 で完了 + 全 success → RDC = 1.0
    let task = MultiRunTaskScore::from_scores_with_metrics(
        "t".into(),
        vec![1.0, 1.0, 1.0],
        vec![0, 0, 0],
        10,
        0.5,
    );
    assert!((task.reliability_decay - 1.0).abs() < 1e-6);
}

#[test]
fn t_rdc_full_decay_late_iterations_fail() {
    // 早期 (iter=0) は成功、後期 (iter=9) は失敗 → 強い負相関 → RDC 低
    let task = MultiRunTaskScore::from_scores_with_metrics(
        "t".into(),
        vec![1.0, 0.5, 0.0],
        vec![0, 5, 9],
        10,
        0.5,
    );
    assert!(task.reliability_decay < 0.5,
            "expected decay < 0.5, got {}", task.reliability_decay);
}

#[test]
fn t_vaf_baseline_zero_variance_returns_none() {
    let baseline = mk_result(vec![1.0, 1.0, 1.0]); // var=0
    let experiment = mk_result(vec![1.0, 0.5, 0.0]); // var>0
    assert!(experiment.variance_amplification_vs(&baseline).is_none());
}

#[test]
fn t_vaf_amplification_doubled() {
    // baseline var (k-1 補正) と experiment var の比が 1.0 を超える設定
    let baseline = mk_result(vec![1.0, 0.5, 0.0]);
    let experiment = mk_result(vec![1.0, 0.0, 0.0]);
    let vaf = experiment.variance_amplification_vs(&baseline).unwrap();
    assert!(vaf > 1.0, "expected amplification, got {}", vaf);
}

#[test]
fn t_gds_all_pass_returns_one() {
    let task = MultiRunTaskScore::from_scores_with_metrics(
        "t".into(),
        vec![1.0, 0.8, 0.6],
        vec![0, 1, 2],
        10,
        0.5,
    );
    assert!((task.graceful_degradation - 1.0).abs() < 1e-6);
}

#[test]
fn t_gds_partial_credit_proximity() {
    // pass_threshold = 0.5、失敗 run: 0.4, 0.45 → proximity = (0.4 + 0.45) / (2 * 0.5) = 0.85
    let task = MultiRunTaskScore::from_scores_with_metrics(
        "t".into(),
        vec![0.4, 0.45, 1.0],
        vec![0, 1, 2],
        10,
        0.5,
    );
    let expected = (0.4 / 0.5 + 0.45 / 0.5) / 2.0;
    assert!((task.graceful_degradation - expected).abs() < 1e-6);
}

#[test]
fn t_composite_reliability_decay_average() {
    let result = MultiRunBenchmarkResult {
        task_scores: vec![
            MultiRunTaskScore { reliability_decay: 1.0, ..mk_score("a") },
            MultiRunTaskScore { reliability_decay: 0.5, ..mk_score("b") },
        ],
        duration_secs: 0.0,
        core_avg_score: None,
        extended_avg_score: None,
    };
    assert!((result.composite_reliability_decay() - 0.75).abs() < 1e-6);
}

#[test]
fn t_serde_backward_compat_old_json_loads() {
    // 旧 JSON (rdc/gds なし) → デフォルト値が入って load 成功
    let old = r#"{"task_id":"x","pass_at_k":1.0,"pass_consecutive_k":1.0,
                  "mean_score":0.8,"variance":0.0,"individual_scores":[0.8,0.8]}"#;
    let task: MultiRunTaskScore = serde_json::from_str(old).unwrap();
    assert!((task.reliability_decay - 1.0).abs() < 1e-6);
    assert!((task.graceful_degradation - 1.0).abs() < 1e-6);
}
```

**File**: `src/agent/experiment.rs` (tests module、line 358 以降)

```rust
#[test]
fn t_experiment_from_multi_results_includes_stability_delta() {
    let baseline = mk_multi_result(vec![vec![1.0, 1.0, 1.0]]);  // var=0
    let experiment = mk_multi_result(vec![vec![1.0, 0.5, 0.0]]);
    let exp = Experiment::from_multi_results(
        "e1".into(),
        MutationType::PromptRule,
        "test".into(),
        &baseline,
        &experiment,
        HashMap::new(),
    );
    assert!(exp.reliability_decay.is_some());
    assert!(exp.graceful_degradation.is_some());
    // baseline var=0 → VAF=None → stability_delta=None
    assert!(exp.variance_amplification.is_none());
    assert!(exp.stability_delta.is_none());
}
```

**Red 確認**:
```bash
cargo test --release --lib agent::benchmark::tests::t_rdc \
                          agent::benchmark::tests::t_vaf \
                          agent::benchmark::tests::t_gds \
                          agent::benchmark::tests::t_composite_reliability \
                          agent::benchmark::tests::t_serde_backward \
                          agent::experiment::tests::t_experiment_from_multi_results_includes_stability
```
9 件すべて compile error or assertion fail で Red 確定 → Phase 2 へ。

### Phase 2 (Green): 最小実装

1. `MultiRunTaskScore` に 2 フィールド追加 (rdc/gds、VAF は task 単位なし) + serde default
2. `from_scores_with_metrics` 新メソッド実装 (RDC/GDS 計算、`from_scores` を内部委譲)
3. `MultiRunBenchmarkResult` に composite メソッド 3 つ追加 (`composite_reliability_decay` / `variance_amplification_vs` / `composite_graceful_degradation`)
4. `BenchmarkSuite::run_k` (line 843-941) で `iterations_used` を per-run 収集 → `from_scores_with_metrics` 呼出に切替
5. `Experiment` に 4 フィールド追加 + serde default
6. `Experiment::from_multi_results` で stability_delta 算出
7. SQLite migration V8 → V9 (`db/migrate.rs`)、`save_to_db` / `recent_experiments` SQL 4 列対応
8. TSV ヘッダ 11 → 14 列対応 (`append_tsv`)

**Green 確認**:
```bash
cargo test --release --lib agent::benchmark agent::experiment
# 期待: 1032 + 9 = 1041 passed (本 plan で +9 件、退行 0)
cargo clippy --release --lib --tests -- -D warnings
cargo fmt --check
```

### Phase 3 (Refactor): 後方互換確認

1. **TSV 旧フォーマット読込テスト**: 現状 read 経路がないため不要、ただし「旧 TSV を grep する shell script」が壊れないかチェック (列番号依存の grep があれば書き換え必要)。
2. **SQLite migration テスト**: `migrate::get_migration_sql(9)` で V8 → V9 を適用しても既存データが残るか手動検証 (in-memory DB で V8 まで apply → row 1 件 INSERT → V9 apply → row 取得確認)。
3. **JSON serde 互換**: 旧 `MultiRunBenchmarkResult` JSON (rdc/vaf/gds なし) を deserialize → デフォルト値で load 成功 (Phase 1 の `t_serde_backward_compat_old_json_loads` で carry-over)。
4. **Lab 永続化 SHA**: `experiment_log` tests (existing) が新フィールドで壊れないこと、`load_accepted_archive` の SQL は新列を SELECT しないので影響なし。

### Phase 4 (smoke 実機検証): 既存 baseline 再現 + 新メトリクス出力確認

**条件**:
- llama-only (`[fallback_chain]` 一時 comment-out、handoff 05-06f Quick Start 通り)
- MCP detach 維持 (項目 180)
- `BONSAI_LAB_SMOKE=1`、smoke 5 task k=3 = 15 run

```bash
BONSAI_LAB_SMOKE=1 cargo run --release -- --lab --lab-experiments 0 \
  2>&1 | tee /tmp/bonsai-llama/rdc-vaf-smoke-baseline-2026-05-XX.log

# 確認
grep -E "(reliability_decay|variance_amplification|graceful_degradation)" \
  /tmp/bonsai-llama/rdc-vaf-smoke-baseline-2026-05-XX.log
# TSV の 14 列ヘッダ確認
head -1 ~/Library/Application\ Support/bonsai-agent/experiments.tsv
```

**期待出力例 (synthetic、production data ではない)**:
```
[lab] baseline: composite_score=0.75 pass_at_k=0.80 pass_consec=0.80
       rdc=0.95 gds=0.92 mean_var=0.034
```

### Phase 5 (docs): CLAUDE.md 項目 199 追記

```markdown
199. **Beyond pass@1 RDC/VAF/GDS 信頼性メトリクス追加 (★★★ arxiv 2603.29231 高優先 1/10)**:
arxiv 2026-03 "Beyond pass@1" 知見を `MultiRunTaskScore` に拡張、3 メトリクス追加 —
**RDC** (Reliability Decay Curve、iteration 軸の信頼性減衰スカラー、proxy 実装で論文式取得待ち) /
**VAF** (Variance Amplification Factor、baseline 比 variance 増幅、aggregate のみ) /
**GDS** (Graceful Degradation Score、failure 時の閾値近接度)。`Experiment.stability_delta` を
新設し ACCEPT 判定の第 3 軸基盤とする (本項では計算のみ、active gate 化は別 plan)。
SQLite V8→V9 migration で 4 列追加、TSV 11→14 列、serde `#[serde(default)]` で旧データ後方互換。
1032→1041 passed (+9 tests、退行ゼロ)、smoke baseline=X.XXXX で既存値再現確認、
新メトリクス全 task で出力確認。次=★★ active gate 化 (stability_delta > 0 を ACCEPT 条件に追加)、
RDC 論文式 precise 化、PASS@(k,T) plan (arxiv 2604.14877)。
```

## Decision Gate

| Gate | 条件 | 失敗時の対処 |
|------|------|-----------|
| **G-1** | `cargo test --release --lib` で **1032 → 1041 passed** (退行ゼロ) | 既存 tests 退行は即修正、新規 9 tests Red 残ったら Phase 2 不完全 |
| **G-2** | smoke baseline `composite_score` の variance 範囲内 (vs 直近 baseline ± 0.03) | regression なら新メトリクス計算が hot path を阻害している疑い、benchmark.rs run_k のパフォーマンス計測 |
| **G-3** | RDC/VAF/GDS の手計算検証 1 件以上一致 | 例: scores=[1.0, 0.5, 0.0], iters=[0,5,9], budget=10, threshold=0.5 で RDC/GDS を Python で別計算し ±1e-6 一致 |
| **G-4** (informational) | TSV 14 列ヘッダ + DB V9 migration が新規セッションで成功 | 旧 TSV ファイル併存テストは別途確認 |
| **G-5** (informational) | clippy / fmt 0 warning | 通常通り |

## Risks / Mitigation

| # | Risk | Mitigation |
|---|------|-----------|
| **R1** | RDC proxy 式が論文 precise 式と乖離、Lab 採否で誤判断 | 本 plan では active gate 化しない (informational のみ)、precise 式は別 plan で論文 PDF 取得後に置換、フィールド名は維持 |
| **R2** | k=3 で sample size 不足、RDC/VAF が信頼区間広く noise dominant | k>=5 推奨は別 plan、本 plan は「観測の足場」のみ。CLAUDE.md に「k=3 では指標は noisy、k>=5 推奨」と明記 |
| **R3** | SQLite migration V9 が既存 DB を破壊 | `ALTER TABLE ADD COLUMN` は SQLite で安全 (既存 row は NULL で埋まる)、in-memory DB テストで事前検証 |
| **R4** | TSV 14 列化で旧 TSV と混在するとヘッダ不一致 | append_tsv の `needs_header = !path.exists() \|\| size==0` 既存ロジックは新規ファイルのみヘッダを書くため、旧 11 列ファイルへの追記時は警告ログ + skip 推奨。または「TSV path 自体を切り替える」運用を docs に明記 |
| **R5** | `MultiRunTaskScore::variance_amplification` フィールドを task 単位で持たせると task 個別 baseline 比較を要求される設計汚染 | **本 plan の設計判断**: VAF は **aggregate 専用** (`MultiRunBenchmarkResult::variance_amplification_vs`)、task 単位フィールドは持たない。Experiment に composite VAF だけ持たせる |
| **R6** | `from_scores` legacy entry の存続でカバレッジ漏れ | run_k は `from_scores_with_metrics` に完全切替、`from_scores` は public API として残すが全 production 経路では不使用、テスト fixture のみ使用 |
| **R7** | iterations_used を per-run で収集する追加データフローが run_k を膨張させる | 既に `loop_result.iterations_used` (benchmark.rs:1088) は計測済、Vec<usize> を 1 つ push するだけ、コード増加最小 |
| **R8** | RDC が `iter_used / budget` ratio に依存し、budget = 0 で除算エラー | budget == 0 なら RDC = 1.0 (early return)、既存 TaskScore::score の efficiency 計算と同パターン (line 68-72) |

## YAGNI / 見送り (本 plan 範囲外)

| 案 | 見送り理由 | 再評価トリガー |
|----|----------|------------|
| **active gate 化** (`stability_delta > 0` を ACCEPT 条件追加) | smoke データ蓄積が必要、現行 ACCEPT 基準 (`delta > 0`) との同時運用で混乱、judge_gate (項目 163) と同様 opt-in にすべき | smoke 10+ サイクル経て stability_delta の noise 範囲確定後、別 plan で `stability_threshold: Option<f64>` を `ExperimentLoopConfig` に追加 |
| **RDC duration 軸対応** (wallclock per-run 計測) | `AgentLoopResult` に duration を追加要、`run_k` の per-run `Instant::now()` も追加 | 長尺 task (max_iterations >= 20) を benchmark に追加した時点で再評価 |
| **論文 precise 式採用** | 論文 PDF / HTML の本文取得が現セッションで未完、proxy で先行 | 論文 PDF を user / firecrawl で取得後、Phase 2c の式置換のみで対応可 |
| **task 単位 VAF** | task 個別 baseline 比較を要求し API が複雑化、aggregate VAF で十分 | per-task drift 分析が必要になった場合 |
| **RDC curve 配列** (`Vec<(f64, f64)>` で iteration ratio - score) | k=3 で curve 描画意味なし、UI/dashboard 側の要望次第 | dashboard で curve 表示要件発生時 |
| **TSV migration script** (旧 11 列 → 新 14 列) | 既存 TSV は append-only ログで読み返し経路なし、運用上「新セッション = 新 TSV」で回避 | TSV 履歴を統合解析する仕組みを別途構築する場合 |

## Scope Outside / 別 plan 候補

- **★★ active stability gate**: `ExperimentLoopConfig.stability_threshold: Option<f64>`、`accepted = (delta > 0) && (stability_delta.unwrap_or(0.0) > stability_threshold)`。smoke 10+ サイクルで baseline の stability_delta noise floor 計測後着手。
- **★★ PASS@(k,T) (arxiv 2604.14877)**: k 軸 + T 軸 (max_iterations 別実験) の 2D マップ、Lab 1D→2D 拡張。本 plan の RDC iteration 軸を T 軸に拡張する自然な後継。
- **★★ Judge Reliability Harness (arxiv 2603.05399)**: 項目 163 judge gate の stress test、本 plan が判定軸を増やすほど重要性が高まる。

## Key Files

| File | Operation | Description | LOC 見積 |
|------|-----------|-------------|---------|
| `src/agent/benchmark.rs` | Modify | `MultiRunTaskScore` に 2 フィールド追加 / `from_scores_with_metrics` 新メソッド / `MultiRunBenchmarkResult` に composite 3 メソッド / `run_k` で `iterations_used` 収集切替 / tests +7 件 | +120 / -5 |
| `src/agent/experiment.rs` | Modify | `from_multi_results` で stability_delta 計算追加 / tests +1 件 | +30 |
| `src/agent/experiment_log.rs` | Modify | `Experiment` に 4 フィールド追加 / `save_to_db` 4 列追加 SQL / `recent_experiments` 4 列追加 SQL / `append_tsv` 14 列ヘッダ + body | +60 / -10 |
| `src/db/migrate.rs` | Modify | V8 → V9 migration SQL (`ALTER TABLE experiments ADD COLUMN ...` × 4) | +20 |
| `src/db/schema.rs` | Modify | `LATEST_SCHEMA_VERSION` 8 → 9 (定数あれば) | +1 / -1 |
| `CLAUDE.md` | Modify | 項目 199 追加 | +10 |
| `~/.claude/projects/.../session_2026_05_XX_handoff.md` | Write | handoff 記録 | n/a (Phase 5) |
| `/tmp/bonsai-llama/rdc-vaf-smoke-baseline-2026-05-XX.log` | Write (Phase 4) | smoke 結果ログ | n/a |

## 見積もり

| Phase | 内容 | 所要 |
|-------|------|------|
| Phase 1 | Red tests 9 件 + 既存 fixture import / `mk_score` 等 helper | ~45 min |
| Phase 2 | Green 実装 (benchmark / experiment / experiment_log / migrate) | ~120 min |
| Phase 3 | Refactor / 後方互換確認 / clippy / fmt | ~30 min |
| Phase 4 | smoke 5 task 実機 (llama-only、MCP detach、~25 min wallclock) | ~30 min |
| Phase 5 | CLAUDE.md 項目 199 追記 + handoff | ~30 min |
| **合計** | | **~4h (0.5 day scope)** |

## Coordination

> **multi-plan 並列**: AgentHER plan / A-RAG plan が別 agent で並行起草中。本 plan は以下で名前空間を独立に保つ。
> - **plan ファイル名**: `beyond-pass1-rdc-vaf-impl.md` (AgentHER は `agenther-hindsight-relabel-impl.md` 等を想定、A-RAG は `arag-hierarchical-interfaces-impl.md` 等を想定)
> - **API 名前空間**: `MultiRunTaskScore.reliability_decay` / `graceful_degradation`、`MultiRunBenchmarkResult::variance_amplification_vs` / `composite_reliability_decay` / `composite_graceful_degradation`、`Experiment.stability_delta` / `reliability_decay` / `variance_amplification` / `graceful_degradation` (AgentHER は `experience.rs` / `skill.rs`、A-RAG は `tools/search.rs` / 新規 `arag.rs` を想定 — 重複なし)
> - **SQLite migration version**: 本 plan は V8 → V9 を予約。AgentHER / A-RAG plan が同 migration を要する場合、起草後に version 番号調整 (本 plan を V9、AgentHER を V10、A-RAG を V11 等の順序付け)

## SESSION_ID (for /ccg:execute)

- **CODEX_SESSION**: (未取得 — 本 plan 起草時点では analyzer prompt 不在、実装着手時に CCG review で取得推奨)
- **GEMINI_SESSION**: (同上)

## 完了基準

1. `cargo test --release --lib`: 1032 → **1041 passed** (+9 新規、退行ゼロ)
2. `cargo clippy --release --lib --tests -- -D warnings`: 0 warning
3. `cargo fmt --check`: 0 件
4. SQLite V8 → V9 migration が in-memory DB で適用成功 (既存 row 保持)
5. TSV 14 列ヘッダで新規セッションが起動成功
6. smoke baseline `composite_score` が直近 baseline (handoff 05-06f, score=0.7253 等) の variance 範囲内 (±0.03)
7. RDC/VAF/GDS が smoke ログに出力され、手計算検証 1 件で ±1e-6 一致 (G-3)
8. CLAUDE.md 項目 199 追記
9. handoff 記録
10. commit 単位: `test(benchmark)` Red → `feat(benchmark)` Green core → `feat(experiment_log)` 永続化 → `feat(db)` migration V9 → `docs(claude.md)` 項目 199
