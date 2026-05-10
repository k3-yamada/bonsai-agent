# Plan: PASS@(k,T) — k × T 二軸 capability/efficiency 分離メトリクス

> **由来**: arxiv 2604.14877 (2026-04) "Does RL Expand the Capability Boundary of LLM Agents? PASS@(k,T) Analysis"。**sampling budget k × interaction depth T** の 2 次元評価で **capability expansion** (k 軸) と **efficiency improvement** (T 軸) を分離する。CLAUDE.md 項目 207 (Lab v15 天井 5 連続) / 項目 215 (Lab v17 天井 7 連続) で「pass^k 単独 (k 軸のみ)」では「成功するが遅い 1bit Bonsai」の改善を捉えきれていないという課題が顕在化、本 plan は **T 軸 (時間 / step 制約)** を informational-only で導入し、変異の 2 D 効果マップを可視化する基盤を整える。
>
> **由来 plan / handoff**:
> - `research_arxiv_2026_05_07.md` 領域 2 ★★★ 高優先 #2 (PASS@(k,T))
> - 既存 plan `beyond-pass1-rdc-vaf-impl.md` (項目 200、`MultiRunTaskScore` 拡張パターン雛形)
> - 既存 plan `agentfloor-tier-eval-impl.md` (T 軸将来拡張で 3D = k × T × tier 解析の足場)
> - CLAUDE.md 項目 1 (pass^k baseline) / 項目 200 (Beyond pass@1) / 項目 207 (Lab v15 天井確定) / 項目 215 (Lab v17 REJECT)

## Task Type

- [ ] Frontend
- [x] Backend (純メトリクス拡張、`benchmark.rs` / `experiment.rs` / `experiment_log.rs` / `db/migrate.rs`)
- [ ] Fullstack
- [x] Docs (CLAUDE.md 項目 223 候補 + 1 行 summary)

---

## 1. 背景

### 1.1 PASS@(k,T) の概念 (論文要約)

論文 (arxiv 2604.14877) の主張は **「pass@k は capability の天井を測るが、agent 系では interaction depth T (step 数 / 時間) の制約下での成功率も capability の重要次元」** というもの。具体的には:

| 軸 | 意味 | 既存 pass^k での扱い |
|----|------|------------------|
| **k** (sampling budget) | 同一タスクを k 回試行、k 回中 1 回でも成功すれば pass | `pass_at_k` で直接測定済 |
| **T** (interaction depth) | 各試行で許容する step 数 / 時間の上限 | `max_iterations` を **task 固定値** で hard-cap、複数 T で計測する経路なし |

**論文式**:

```
PASS@(k, T) = P[ ∃ i ∈ [1, k] : success(i) ∧ depth(i) ≤ T ]
```

すなわち「k 回中、**T 制約を満たして** 成功した試行が 1 回でもあるか」の確率。論文知見:

- **capability expansion**: k を増やしても T 制約下の成功率が上がらないなら「真の能力上限」。RL は真の capability boundary をどこまで押し上げたかを (k, T) 平面で可視化。
- **efficiency improvement**: 大きな T では成功するが小さな T では失敗する場合、改善は efficiency (less interaction) であり capability ではない。
- **2 D heatmap**: agent 評価の標準として `pass_at_k_t[i][j]` (i=k 段階、j=T 段階) を出力推奨。

### 1.2 bonsai 既存メトリクスとの対応

```rust
// benchmark.rs:330-358 (現状)
pub struct MultiRunTaskScore {
    pub pass_at_k: f64,            // ← k 軸のみ (T = max_iterations 固定)
    pub pass_consecutive_k: f64,   // ← p^n 制約 (連続性軸、独立)
    pub mean_score: f64,
    pub variance: f64,
    pub individual_scores: Vec<f64>,
    pub reliability_decay: f64,    // ← 項目 200、iteration 軸の "decay 傾き" 観測
    pub graceful_degradation: f64, // ← 項目 200、failure proximity
    // ...
}
```

**現状の T 軸**: 各 `BenchmarkTask.max_iterations` (3〜10) を「タスク固有の T 上限」として hard-cap、計測軸として複数 T を回す経路は **存在しない**。

### 1.3 Bonsai-8B における必要性 (Lab 天井 7 連続)

CLAUDE.md 項目 207 / 215 / lab_history で観測されてきた天井 7 連続 (v8/v9/v10/v14/v15/v16/v17) のうち、**v15 副次知見** (Zone A 突入 0.7812) と **v17 副次知見** (項目 215「ON pair 1-4 variance std≈0.010 vs OFF std≈0.034 で stability 軸 ON 顕著優位」) は、**「成功率 (k 軸) の天井に達した変異群でも、T 軸 (efficiency) では改善している可能性」** を示唆する。

具体例:
- 項目 197 (Layer 1 緩和 4000→8000): `Δscore=+0.0226 / Δduration=+12.8%` → score 微増 + duration 増 = efficiency 悪化、score 軸単独では境界判定だが PASS@(k, T=低 budget) で見ると T=低 で REJECT、T=高 で ACCEPT が分かれる可能性。
- 項目 198 (MLX sticky recovery): `core 22 score=0.7837 / +33.7% vs sticky` → 同一 score 帯でも throughput 大幅改善 = capability 不変 / efficiency 改善 の典型、PASS@(k,T) で初めて可視化可能。

つまり **PASS@(k,T) は「変異を ACCEPT すべきか REJECT すべきか」の判定を、score 単軸から (capability, efficiency) 2 D に分解** する。

### 1.4 1bit variance との両立 (重要設計制約)

bonsai は k=3 が default、k=5 への増加は cost 線形。**本 plan は k 軸を増やさず、T 軸を 2-3 段で計測** することで cost 増を最小化 (k=3 で取得済の per-run データを post-hoc 集計 → cost 0 増、後述 §4.2)。

---

## 2. 目的

1. **`MultiRunTaskScore` に T 制約付き pass 率を informational-only で追加** — `pass_at_k_t_steps: Vec<(usize, f64)>` / `pass_at_k_t_seconds: Vec<(f64, f64)>`
2. **per-run の T 値 (steps + wallclock) を `run_k` 内で per-task 収集** — 既存 `iterations_per_run` に加え `durations_per_run` を Vec<f64> で計測し、`from_scores_with_metrics_v2` 新メソッドで T 軸計算
3. **env 制御で T 閾値設定可能化** — `BONSAI_PASS_K_T_SECONDS=300` / `BONSAI_PASS_K_T_STEPS=5` で複数 T 値指定 (CSV `300,600,900`)

### 非目標

- T 軸 ACCEPT 判定への即時統合 (本 plan は informational only、active gate 化は別 plan、項目 200 と同じパターン)
- `max_iterations` の動的変更 / Lab 内で T を増やしての追加実行 (cost 倍増を回避、既存 run データを post-hoc 分析)
- 論文の RL 学習側比較実験 (capability boundary expansion の RL 比較は scope 外)
- 2D heatmap UI / dashboard (本 plan は数値出力 + log のみ、可視化は別 plan)
- 既存 `pass_at_k` / `pass_consecutive_k` の semantic 変更 (signature 変更ゼロ)

---

## 3. 既存項目との関係

| 項目 | 関係 | 影響 |
|------|------|------|
| **項目 1** (pass^k 基本指標) | 本 plan は pass_at_k に T 制約軸を追加。既存 `pass_at_k` / `pass_consecutive_k` を semantic 不変で残す | API additive |
| **項目 200** (Beyond pass@1 RDC/VAF/GDS) | T 軸の拡張、informational-only / `Experiment.stability_delta` 同様の活用パターン | SQLite migration を V12 → V13 で予約 (V12 は項目 218 既使用) |
| **項目 207** (Lab v15 天井 5 連続) | 「次の打開点 = 構造的変異」副次知見の T 軸での可視化 | Lab v18+ で PASS@(k,T) を informational 出力 |
| **項目 213** (ERL Heuristics Pool) | SCHEMA_V10 が ERL plan v2 で確保。**本 plan は V13 で上乗せ** (依存順序: ERL V10 → ReviewState V12 → 本 plan V13) | V13 確保 |
| **項目 215** (Lab v17 REJECT、stability 軸 ON 優位) | T 軸が efficiency 改善を捉えられるなら ERL の真効力評価で再活用可 | Lab v18+ informational 確認、active gate 化は別 plan |
| **項目 218** (Cerememory ReviewState V12) | SCHEMA_V12 既使用。**本 plan は V13 で上乗せ** | V13 確保 |
| **項目 222** (sqlite-vec wiring 削除後) | 直交 (永続化 schema 別領域) | 影響なし |
| `agentfloor-tier-eval-impl.md` (項目 213 候補) | 3D = k × T × tier の足場、本 plan が T 軸を独立に確保した上で別 plan の AgentFloor が tier 軸を追加可能 | 直交軸として共存 |
| `beyond-pass1-rdc-vaf-impl.md` (項目 200) | 既存 RDC は iteration 軸の負相関 proxy、本 plan は T 軸の絶対閾値判定 (相補的) | 同一 `MultiRunTaskScore` 上で共存、`from_scores_with_metrics` 拡張 |

---

## 4. 設計

### 4.1 `MultiRunTaskScore` 拡張 (benchmark.rs:330-358)

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
    #[serde(default = "default_reliability_decay")]
    pub reliability_decay: f64,
    #[serde(default = "default_graceful_degradation")]
    pub graceful_degradation: f64,

    // ── 新規 (項目 223、PASS@(k,T)、後方互換のため Vec default で空) ──
    /// step 軸 PASS@(k, T_steps) 列。各要素は `(T_steps, pass_rate)`。
    /// 例: T_steps=3 で pass_rate=0.33 (3 run 中 1 run が <=3 step で成功)
    /// `BONSAI_PASS_K_T_STEPS` 未指定 / k=0 の場合は空 Vec (= 既存挙動と同等)。
    #[serde(default)]
    pub pass_at_k_t_steps: Vec<(usize, f64)>,
    /// 時間軸 PASS@(k, T_seconds) 列。各要素は `(T_seconds, pass_rate)`。
    /// 例: T_seconds=300.0 で pass_rate=0.66 (3 run 中 2 run が <=300 sec で成功)
    /// `BONSAI_PASS_K_T_SECONDS` 未指定の場合は空 Vec。
    #[serde(default)]
    pub pass_at_k_t_seconds: Vec<(f64, f64)>,
}
```

**設計判断**: `Vec<(T, rate)>` は閾値ごとの直接対応で grep 容易、JSON でも素直に表現される。**HashMap でなく Vec** にする理由:
1. 順序保持で deterministic な log / serde 出力
2. 閾値数は 1-3 程度で線形検索 cost 無視可
3. `serde_json` でキー型制約なし (HashMap<f64, f64> はキー string 化が必要)

### 4.2 集計ロジック (per-run T 値の収集)

`run_k` (benchmark.rs:1184-1288) で **per-run の wallclock 計測を新規追加**。既存 `iterations_per_run: Vec<usize>` の隣に `durations_per_run: Vec<f64>` を並置。

```rust
// 擬似 Rust (benchmark.rs run_k 内)
let mut iterations_per_run: Vec<usize> = Vec::with_capacity(multi.k);
let mut durations_per_run: Vec<f64> = Vec::with_capacity(multi.k);  // 新規

for run_idx in 0..multi.k {
    if cancel.is_cancelled() { break; }
    store.reset_session_data_for_lab()?;
    // ... config 構築
    let run_start = std::time::Instant::now();              // 新規
    let result = run_agent_loop(/* ... */);
    let run_duration = run_start.elapsed().as_secs_f64();   // 新規

    let score = match result {
        Ok(ref loop_result) => {
            last_run_capture = Some((loop_result.answer.clone(), loop_result.tools_called.clone()));
            iterations_per_run.push(loop_result.iterations_used);
            durations_per_run.push(run_duration);            // 新規
            evaluate_task_response(task, loop_result).score()
        }
        Err(_) => {
            iterations_per_run.push(task.max_iterations);
            durations_per_run.push(run_duration);            // 失敗 run も計測
            0.0
        }
    };
    scores.push(score);
}

// 項目 200 + 223: from_scores_with_metrics_v2 で RDC/GDS/PASS@(k,T) を計算
let mut task_score = MultiRunTaskScore::from_scores_with_metrics_v2(
    task.id.clone(),
    scores,
    iterations_per_run,
    durations_per_run,
    task.max_iterations,
    pass_threshold,
    &t_steps_thresholds,    // Vec<usize>、env 由来
    &t_seconds_thresholds,  // Vec<f64>、env 由来
);
```

`AgentLoopResult` への `duration_secs` フィールド追加は **本 plan では行わない** (per-run wallclock は `run_k` 内 `Instant::now()` で計測十分、`AgentLoopResult` signature 不変)。

### 4.3 `from_scores_with_metrics_v2` 新メソッド

既存 `from_scores_with_metrics` は **保持** (test fixture / legacy 経路)、新規 `from_scores_with_metrics_v2` を追加:

```rust
impl MultiRunTaskScore {
    /// 項目 223: PASS@(k,T) 込みフル版。`from_scores_with_metrics` に T 軸 2 種を加えた拡張版。
    /// - `t_steps_thresholds`: 例 [3, 5, 7]、各値ごとに `pass_at_k_t_steps[i]` を計算
    /// - `t_seconds_thresholds`: 例 [60.0, 180.0, 600.0]
    /// 空 Vec の場合は対応フィールドが空 Vec に (既存挙動互換)。
    pub fn from_scores_with_metrics_v2(
        task_id: String,
        scores: Vec<f64>,
        iterations_used: Vec<usize>,
        durations_secs: Vec<f64>,
        iteration_budget: usize,
        pass_threshold: f64,
        t_steps_thresholds: &[usize],
        t_seconds_thresholds: &[f64],
    ) -> Self {
        // 既存 RDC/GDS は from_scores_with_metrics 経由で計算
        let mut score = Self::from_scores_with_metrics(
            task_id,
            scores.clone(),
            iterations_used.clone(),
            iteration_budget,
            pass_threshold,
        );
        // PASS@(k, T_steps): k 回中、(score >= pass_threshold && iter <= T) の割合
        score.pass_at_k_t_steps = compute_pass_at_k_t_steps(
            &scores, &iterations_used, pass_threshold, t_steps_thresholds,
        );
        // PASS@(k, T_seconds): k 回中、(score >= pass_threshold && duration <= T) の割合
        score.pass_at_k_t_seconds = compute_pass_at_k_t_seconds(
            &scores, &durations_secs, pass_threshold, t_seconds_thresholds,
        );
        score
    }
}

/// 項目 223: PASS@(k, T_steps) の計算
fn compute_pass_at_k_t_steps(
    scores: &[f64],
    iterations_used: &[usize],
    pass_threshold: f64,
    thresholds: &[usize],
) -> Vec<(usize, f64)> {
    let k = scores.len();
    if k == 0 || iterations_used.len() != k {
        return Vec::new();
    }
    thresholds.iter().map(|&t| {
        let pass_count = scores.iter()
            .zip(iterations_used.iter())
            .filter(|(s, &iter)| **s >= pass_threshold && iter <= t)
            .count();
        (t, pass_count as f64 / k as f64)
    }).collect()
}

/// 項目 223: PASS@(k, T_seconds) の計算 (durations は f64)
fn compute_pass_at_k_t_seconds(
    scores: &[f64],
    durations_secs: &[f64],
    pass_threshold: f64,
    thresholds: &[f64],
) -> Vec<(f64, f64)> {
    let k = scores.len();
    if k == 0 || durations_secs.len() != k {
        return Vec::new();
    }
    thresholds.iter().map(|&t| {
        let pass_count = scores.iter()
            .zip(durations_secs.iter())
            .filter(|(s, &dur)| **s >= pass_threshold && dur <= t)
            .count();
        (t, pass_count as f64 / k as f64)
    }).collect()
}
```

### 4.4 `MultiRunBenchmarkResult` composite メソッド (集計)

```rust
impl MultiRunBenchmarkResult {
    /// 項目 223: 全タスク平均 PASS@(k, T_steps) を閾値ごとに集計。
    /// 各 task の `pass_at_k_t_steps` で同じ T_steps 値の pass_rate を平均。
    pub fn composite_pass_at_k_t_steps(&self) -> Vec<(usize, f64)> {
        if self.task_scores.is_empty() { return Vec::new(); }
        // 同一 T_steps 値での pass_rate 平均
        let mut acc: std::collections::BTreeMap<usize, (f64, usize)> = Default::default();
        for ts in &self.task_scores {
            for &(t, rate) in &ts.pass_at_k_t_steps {
                let entry = acc.entry(t).or_insert((0.0, 0));
                entry.0 += rate;
                entry.1 += 1;
            }
        }
        acc.into_iter().map(|(t, (sum, n))| (t, sum / n as f64)).collect()
    }

    /// 項目 223: 全タスク平均 PASS@(k, T_seconds) を閾値ごとに集計。
    pub fn composite_pass_at_k_t_seconds(&self) -> Vec<(f64, f64)> {
        if self.task_scores.is_empty() { return Vec::new(); }
        // f64 のキー集約は近似比較 (差 < 1e-6 を同一視) で BTreeMap 化のため特殊化
        let mut buckets: Vec<(f64, f64, usize)> = Vec::new();
        for ts in &self.task_scores {
            for &(t, rate) in &ts.pass_at_k_t_seconds {
                if let Some(b) = buckets.iter_mut().find(|b| (b.0 - t).abs() < 1e-6) {
                    b.1 += rate;
                    b.2 += 1;
                } else {
                    buckets.push((t, rate, 1));
                }
            }
        }
        buckets.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        buckets.into_iter().map(|(t, sum, n)| (t, sum / n as f64)).collect()
    }
}
```

### 4.5 env 由来 T 閾値の解析

`benchmark.rs` の private helper として:

```rust
/// 項目 223: env `BONSAI_PASS_K_T_STEPS` から T_steps 閾値列を解析。
/// 例: "3,5,7" -> vec![3, 5, 7]、未指定 / 空 / 解析失敗 -> vec![]
fn parse_t_steps_env() -> Vec<usize> {
    std::env::var("BONSAI_PASS_K_T_STEPS")
        .ok()
        .map(|s| s.split(',')
            .filter_map(|p| p.trim().parse::<usize>().ok())
            .collect())
        .unwrap_or_default()
}

/// 項目 223: env `BONSAI_PASS_K_T_SECONDS` から T_seconds 閾値列を解析。
/// 例: "60,180,600" or "60.0,180.0,600.0" -> vec![60.0, 180.0, 600.0]
fn parse_t_seconds_env() -> Vec<f64> {
    std::env::var("BONSAI_PASS_K_T_SECONDS")
        .ok()
        .map(|s| s.split(',')
            .filter_map(|p| p.trim().parse::<f64>().ok())
            .filter(|v| *v > 0.0)
            .collect())
        .unwrap_or_default()
}
```

`run_k` 入口で 1 度だけ解析、各 task で再利用 (env read を per-task で repeat しない)。

### 4.6 `Experiment` 拡張 (experiment_log.rs:131-162)

```rust
pub struct Experiment {
    // 既存フィールド全保持 (項目 200 の RDC/VAF/GDS/stability_delta 含む)
    // ...

    // ── 新規 (項目 223、PASS@(k,T)) ───────────────────────────
    /// 項目 223: 実験結果の PASS@(k, T_steps) composite。
    /// 各要素: (T_steps, pass_rate)。空配列なら env 未指定。
    /// JSON serialize 時は `"pass_at_k_t_steps":[[3,0.33],[5,0.66]]`。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_at_k_t_steps: Vec<(usize, f64)>,
    /// 項目 223: 実験結果の PASS@(k, T_seconds) composite。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_at_k_t_seconds: Vec<(f64, f64)>,
}
```

`Experiment::from_multi_results` で:

```rust
let pass_at_k_t_steps = experiment.composite_pass_at_k_t_steps();
let pass_at_k_t_seconds = experiment.composite_pass_at_k_t_seconds();
Self {
    // ... 既存
    pass_at_k_t_steps,
    pass_at_k_t_seconds,
    // ...
}
```

### 4.7 SQLite / TSV 永続化

#### SQLite V12 → V13 migration

> **migration 番号確保元**: 項目 222 (sqlite-vec wiring 削除) は既存 V11/V12 を保持、項目 218 (ReviewState) が V12 を確保済。本 plan は **V13** で上乗せ。

```sql
-- migration V13 (項目 223、PASS@(k,T))
ALTER TABLE experiments ADD COLUMN pass_at_k_t_steps TEXT;    -- JSON 配列 "[[3,0.33],[5,0.66]]"
ALTER TABLE experiments ADD COLUMN pass_at_k_t_seconds TEXT;  -- JSON 配列 "[[60.0,0.5],[300.0,0.83]]"
```

**設計判断**: 配列を **TEXT (JSON 文字列)** で永続化:
1. 列数固定 (T 閾値数は env 駆動で可変) で ALTER 困難
2. 解析スクリプトで `jq '.[] | .[0]'` 等で扱いやすい
3. NULL OK (env 未指定セッションは NULL)

#### TSV (experiment_log.rs:285-323): 15 列 → 17 列

**末尾 2 列追加** (`pass_at_k_t_steps` / `pass_at_k_t_seconds` の JSON encode):

```
... \treliability_decay\tvariance_amplification\tgraceful_degradation\tpass_at_k_t_steps\tpass_at_k_t_seconds
```

例:
```
exp_001\tprompt_rule\t...\t0.95\t1.02\t0.87\t[[3,0.33],[5,0.66]]\t[[60.0,0.50],[300.0,0.83]]
```

env 未指定 → `-` (既存 None 表現と一貫):
```
exp_002\tprompt_rule\t...\t0.95\t1.02\t0.87\t-\t-
```

### 4.8 Lab summary 出力形式

`run_experiment_loop` (experiment.rs) の baseline / experiment 直後 log:

```
[INFO][lab.pass_k_t] baseline: composite_score=0.7812 pass_at_k=0.9242 pass_consec=0.9091
[INFO][lab.pass_k_t]   PASS@(k=3, T_steps): T=3:0.45 T=5:0.68 T=7:0.81
[INFO][lab.pass_k_t]   PASS@(k=3, T_seconds): T=60.0:0.32 T=180.0:0.55 T=600.0:0.79
```

env 未指定 → 該当行を出力しない (既存 log との混在回避)。

`Experiment::from_multi_results` 経由で informational 軸として記録。**ACCEPT 判定は `delta > 0.0` のまま** (本 plan は判定変更ゼロ)。

---

## 5. TDD strict 5 phase

### Phase 1 (Red): failing tests 作成 (>=5 件)

**File**: `src/agent/benchmark.rs` (tests module、既存 1093+ tests に追加)

```rust
#[test]
fn t_pass_at_k_t_steps_basic() {
    // 3 run: scores=[1.0, 1.0, 0.0], iters=[2, 5, 10], threshold=0.5
    // T=3 -> run0 (s=1.0, i=2) のみ -> 1/3
    // T=5 -> run0+run1 (s=1.0, i=5) -> 2/3
    // T=10 -> run2 は score<0.5 で fail (run0+run1 両方 i<=10) -> 2/3
    let task = MultiRunTaskScore::from_scores_with_metrics_v2(
        "t".into(),
        vec![1.0, 1.0, 0.0],
        vec![2, 5, 10],
        vec![10.0, 30.0, 100.0],
        10,
        0.5,
        &[3, 5, 10],
        &[],
    );
    assert_eq!(task.pass_at_k_t_steps.len(), 3);
    let map: std::collections::HashMap<usize, f64> =
        task.pass_at_k_t_steps.iter().copied().collect();
    assert!((map[&3] - 1.0/3.0).abs() < 1e-6);
    assert!((map[&5] - 2.0/3.0).abs() < 1e-6);
    assert!((map[&10] - 2.0/3.0).abs() < 1e-6);
}

#[test]
fn t_pass_at_k_t_seconds_basic() {
    // 同じ scores、durations=[10, 60, 300], thresholds=[30, 120, 600]
    // T=30 -> run0 のみ pass、1/3
    // T=120 -> run0+run1 pass、2/3 (run1 dur=60 <= 120 + score=1.0)
    // T=600 -> run0+run1 (run2 score<0.5 で fail)、2/3
    let task = MultiRunTaskScore::from_scores_with_metrics_v2(
        "t".into(),
        vec![1.0, 1.0, 0.0],
        vec![2, 5, 10],
        vec![10.0, 60.0, 300.0],
        10,
        0.5,
        &[],
        &[30.0, 120.0, 600.0],
    );
    let map: Vec<(f64, f64)> = task.pass_at_k_t_seconds.clone();
    assert!((map[0].1 - 1.0/3.0).abs() < 1e-6);  // T=30
    assert!((map[1].1 - 2.0/3.0).abs() < 1e-6);  // T=120
    assert!((map[2].1 - 2.0/3.0).abs() < 1e-6);  // T=600
}

#[test]
fn t_pass_at_k_t_empty_thresholds_returns_empty_vec() {
    // env 未指定相当 -> 空 Vec で既存挙動互換
    let task = MultiRunTaskScore::from_scores_with_metrics_v2(
        "t".into(),
        vec![1.0, 0.5],
        vec![1, 2],
        vec![10.0, 20.0],
        5,
        0.5,
        &[],
        &[],
    );
    assert!(task.pass_at_k_t_steps.is_empty());
    assert!(task.pass_at_k_t_seconds.is_empty());
}

#[test]
fn t_pass_at_k_t_all_pass_within_t() {
    // 全 run が T 制約内で pass -> pass_rate=1.0
    let task = MultiRunTaskScore::from_scores_with_metrics_v2(
        "t".into(),
        vec![1.0, 1.0, 1.0],
        vec![1, 1, 1],
        vec![5.0, 5.0, 5.0],
        10,
        0.5,
        &[3],
        &[10.0],
    );
    assert!((task.pass_at_k_t_steps[0].1 - 1.0).abs() < 1e-6);
    assert!((task.pass_at_k_t_seconds[0].1 - 1.0).abs() < 1e-6);
}

#[test]
fn t_pass_at_k_t_all_exceed_t_returns_zero() {
    // 全 run の iter > T_steps -> pass_rate=0.0 (score>=threshold でも T 違反)
    let task = MultiRunTaskScore::from_scores_with_metrics_v2(
        "t".into(),
        vec![1.0, 1.0, 1.0],
        vec![5, 6, 7],
        vec![100.0, 100.0, 100.0],
        10,
        0.5,
        &[3],
        &[50.0],
    );
    assert!((task.pass_at_k_t_steps[0].1 - 0.0).abs() < 1e-6);
    assert!((task.pass_at_k_t_seconds[0].1 - 0.0).abs() < 1e-6);
}

#[test]
fn t_composite_pass_at_k_t_steps_average_across_tasks() {
    // 2 task: T=5 で task_a=0.5, task_b=1.0 -> average=0.75
    let task_a = MultiRunTaskScore::from_scores_with_metrics_v2(
        "a".into(), vec![1.0, 0.0], vec![3, 4], vec![10.0, 20.0],
        10, 0.5, &[5], &[],
    );
    let task_b = MultiRunTaskScore::from_scores_with_metrics_v2(
        "b".into(), vec![1.0, 1.0], vec![3, 4], vec![10.0, 20.0],
        10, 0.5, &[5], &[],
    );
    let result = MultiRunBenchmarkResult {
        task_scores: vec![task_a, task_b],
        duration_secs: 0.0,
        core_avg_score: None,
        extended_avg_score: None,
    };
    let composite = result.composite_pass_at_k_t_steps();
    assert_eq!(composite.len(), 1);
    assert_eq!(composite[0].0, 5);
    assert!((composite[0].1 - 0.75).abs() < 1e-6, "got {}", composite[0].1);
}

#[test]
fn t_serde_backward_compat_old_json_loads_pass_k_t_empty() {
    // 旧 JSON (pass_at_k_t_* なし) -> load 成功 / Vec が空
    let old = r#"{"task_id":"x","pass_at_k":1.0,"pass_consecutive_k":1.0,
                  "mean_score":0.8,"variance":0.0,"individual_scores":[0.8,0.8]}"#;
    let task: MultiRunTaskScore = serde_json::from_str(old).unwrap();
    assert!(task.pass_at_k_t_steps.is_empty());
    assert!(task.pass_at_k_t_seconds.is_empty());
}
```

**File**: `src/agent/experiment_log.rs` (tests module)

```rust
#[test]
fn t_experiment_serde_with_pass_k_t() {
    let mut exp = sample_experiment("e1", 0.05);
    exp.pass_at_k_t_steps = vec![(3, 0.33), (5, 0.66)];
    exp.pass_at_k_t_seconds = vec![(60.0, 0.5), (300.0, 0.83)];
    let json = serde_json::to_string(&exp).unwrap();
    let exp2: Experiment = serde_json::from_str(&json).unwrap();
    assert_eq!(exp2.pass_at_k_t_steps.len(), 2);
    assert_eq!(exp2.pass_at_k_t_seconds.len(), 2);
}

#[test]
fn t_experiment_db_roundtrip_with_pass_k_t_v13() {
    let conn = setup_test_db_v13();  // V1, V2, V7, V9, V13 適用済
    let mut exp = sample_experiment("e_pkt", 0.05);
    exp.pass_at_k_t_steps = vec![(3, 0.33), (5, 0.66)];
    exp.pass_at_k_t_seconds = vec![(60.0, 0.5)];
    ExperimentLog::save_to_db(&conn, &exp).unwrap();
    let results = ExperimentLog::recent_experiments(&conn, 1).unwrap();
    assert_eq!(results[0].pass_at_k_t_steps.len(), 2);
    assert_eq!(results[0].pass_at_k_t_seconds.len(), 1);
}
```

**Red 確認**:
```bash
rtk cargo test --release --lib agent::benchmark::tests::t_pass_at_k_t \
                              agent::benchmark::tests::t_composite_pass_at_k_t \
                              agent::benchmark::tests::t_serde_backward_compat_old_json_loads_pass_k_t \
                              agent::experiment_log::tests::t_experiment_serde_with_pass_k_t \
                              agent::experiment_log::tests::t_experiment_db_roundtrip_with_pass_k_t
```

期待: **9 件 compile error または assertion fail で Red 確認** -> Phase 2 へ。

### Phase 2 (Green): 最小実装

1. `MultiRunTaskScore` に `pass_at_k_t_steps: Vec<(usize, f64)>` / `pass_at_k_t_seconds: Vec<(f64, f64)>` 追加 + `#[serde(default)]`
2. `compute_pass_at_k_t_steps` / `compute_pass_at_k_t_seconds` private fn 実装
3. `from_scores_with_metrics_v2` 新メソッド (既存 `from_scores_with_metrics` を内部 delegate)
4. `MultiRunBenchmarkResult::composite_pass_at_k_t_steps` / `composite_pass_at_k_t_seconds` 実装
5. `parse_t_steps_env` / `parse_t_seconds_env` private fn (env 解析)
6. `BenchmarkSuite::run_k` で:
   - 関数入口で env 1 回解析 (`let t_steps = parse_t_steps_env(); let t_secs = parse_t_seconds_env();`)
   - per-run `Instant::now()` で `durations_per_run` 計測 (失敗 run も elapsed を push)
   - `from_scores_with_metrics_v2` 呼出に切替
7. `Experiment` に 2 フィールド追加 + `#[serde(default, skip_serializing_if = "Vec::is_empty")]`
8. `Experiment::from_multi_results` で composite 計算
9. SQLite migration V12 -> V13 (`db/migrate.rs`、`ALTER TABLE experiments ADD COLUMN pass_at_k_t_{steps,seconds} TEXT`)
10. `save_to_db` / `recent_experiments` SQL に `pass_at_k_t_steps` / `pass_at_k_t_seconds` 列追加 (JSON encode/decode、`serde_json::to_string` / `from_str`)
11. TSV `append_tsv` に 2 列追加 (env 未指定なら `-`、指定なら JSON 文字列)

**Green 確認**:
```bash
rtk cargo test --release --lib agent::benchmark agent::experiment_log
# 期待: 1150 + 9 = 1159 passed (本 plan で +9 件、退行 0)
rtk cargo clippy --release --lib --tests -- -D warnings
rtk cargo fmt --check
```

### Phase 3 (Refactor): 後方互換 + dedup helper

1. **TSV 後方互換**: 旧 15 列 (V12 時点) でも壊れないこと、新規セッションは 17 列で start (`needs_header` 判定で空ファイルか確認、既存ロジック維持)
2. **SQLite migration テスト**: `migrate::get_migration_sql(13)` を in-memory DB に apply し、V12 までの既存 row が NULL で残るか確認
3. **JSON serde 後方互換**: 旧 `MultiRunTaskScore` JSON (pass_at_k_t_* なし) -> load 成功で `Vec::new()` (Phase 1 test で carry-over)
4. **Lab summary ヘルパー**: 既存 baseline summary 出力箇所を grep し、env 指定時のみ `[INFO][lab.pass_k_t]` 行を追加 (1 関数化、log.rs / experiment.rs で macro 化)
5. **docstring**: 各 fn / field に `項目 223、arxiv 2604.14877` 由来コメント、論文式参照リンク

### Phase 4 (smoke 実機検証): baseline 再現 + T 閾値出力確認

**条件**:
- llama-only (`[fallback_chain]` 一時 comment-out、handoff 05-06f Quick Start 通り)
- MCP detach 維持 (項目 180)
- env: `BONSAI_LAB_SMOKE=1 BONSAI_PASS_K_T_STEPS=3,5,7 BONSAI_PASS_K_T_SECONDS=60,180,600`
- smoke 7 task k=3 = 21 run

```bash
BONSAI_LAB_SMOKE=1 \
BONSAI_PASS_K_T_STEPS=3,5,7 \
BONSAI_PASS_K_T_SECONDS=60,180,600 \
rtk cargo run --release -- --lab --lab-experiments 0 \
  2>&1 | tee /tmp/bonsai-llama/pass-k-t-smoke-baseline-2026-05-XX.log

# 確認
rtk grep -E "lab.pass_k_t" /tmp/bonsai-llama/pass-k-t-smoke-baseline-2026-05-XX.log
# 期待: T=3, T=5, T=7 で steps 系、T=60.0, T=180.0, T=600.0 で seconds 系の 6 値出力

# TSV 17 列ヘッダ確認
head -1 ~/Library/Application\ Support/bonsai-agent/experiments.tsv
# 期待末尾: ...\tpass_at_k_t_steps\tpass_at_k_t_seconds
```

**期待出力例 (synthetic、production data ではない)**:
```
[INFO][lab.pass_k_t] baseline: composite_score=0.7253 pass_at_k=0.78 pass_consec=0.71
[INFO][lab.pass_k_t]   PASS@(k=3, T_steps): T=3:0.45 T=5:0.68 T=7:0.78
[INFO][lab.pass_k_t]   PASS@(k=3, T_seconds): T=60.0:0.20 T=180.0:0.55 T=600.0:0.78
```

**判定条件 (3 段)**:
- **G-4a**: env 未指定経路で smoke 完走 + 既存挙動 100% 互換 (TSV 末尾 2 列が `-`)
- **G-4b**: env 指定経路で T_steps / T_seconds 両軸が log + TSV に出力
- **G-4c**: 手計算検証 1 件以上一致 (例: smoke task `error_recovery` k=3 で iter=[3,4,5], scores=[1,1,0] / threshold=0.5、T=3 -> 1/3、T=5 -> 2/3 が log と一致)

### Phase 5 (docs + commit)

CLAUDE.md 項目 223 追加 (1 行 summary):

```markdown
223. **PASS@(k,T) 二軸 capability/efficiency 分離メトリクス追加 (★★★ arxiv 2604.14877 高優先 2/10)**:
arxiv 2026-04 "Does RL Expand the Capability Boundary" 知見を `MultiRunTaskScore` に拡張、
**T_steps 軸** (`pass_at_k_t_steps: Vec<(usize, f64)>`) と **T_seconds 軸** (`pass_at_k_t_seconds: Vec<(f64, f64)>`) の
2 種を informational-only で追加。env `BONSAI_PASS_K_T_STEPS=3,5,7` / `BONSAI_PASS_K_T_SECONDS=60,180,600` で閾値指定、
未指定で既存挙動 100% 互換。`run_k` で per-run wallclock を `Instant::now()` 計測 (`AgentLoopResult` signature 不変)。
SQLite V12->V13 migration で 2 列 (TEXT JSON encode) 追加、TSV 15->17 列、serde `#[serde(default)]` で旧データ後方互換。
1150->1159 passed (+9 tests、退行ゼロ)、smoke baseline=X.XXXX で T 軸 log 出力確認。次=★★ active gate 化
(PASS@(k,T) 閾値超で ACCEPT に追加、別 plan)、Lab v18+ で informational 観測。
```

5 commits 構成:
1. `test(benchmark): Phase 1 Red — PASS@(k,T) 9 件 failing tests`
2. `feat(benchmark): Phase 2 Green — pass_at_k_t_{steps,seconds} + from_scores_with_metrics_v2`
3. `feat(experiment_log): Phase 2 — Experiment 2 フィールド + V13 migration + TSV 17 列`
4. `refactor(benchmark): Phase 3 — env 解析 dedup + Lab summary log helper`
5. `docs(claude.md): 項目 223 — PASS@(k,T) informational metric 完遂 + smoke G-4 PASS`

---

## 6. API 影響 (additive 確証)

| API | 変更 | 後方互換 |
|---|---|---|
| `MultiRunTaskScore` | 2 Vec フィールド追加 | OK `#[serde(default)]` で旧 JSON load 成功 |
| `MultiRunTaskScore::from_scores` | 無変更 | OK 新 Vec が空のまま |
| `MultiRunTaskScore::from_scores_with_metrics` | 無変更 | OK 新 Vec が空のまま |
| `MultiRunTaskScore::from_scores_with_metrics_v2` | 新規 method | — |
| `MultiRunBenchmarkResult::composite_pass_at_k_t_steps/seconds` | 新規 method | — |
| `Experiment` | 2 Vec フィールド追加 | OK `#[serde(default)]` |
| `Experiment::from_multi_results` | 無変更 (内部で composite 呼出追加) | OK signature 不変 |
| `Experiment::from_results` | 無変更 (新 Vec 空で初期化) | OK |
| `BenchmarkSuite::run_k` | 内部で per-run `Instant::now()` 計測追加 | OK signature 不変 |
| `AgentLoopResult` | 無変更 | OK |
| `AgentConfig` | 無変更 | OK env 経由で T 制御 |
| SQLite | V12 -> V13 (2 列 ALTER TABLE) | OK additive |
| TSV | 15 -> 17 列 (末尾追加) | NOTE header 駆動 reader OK、列番号 grep 注意 |
| env | `BONSAI_PASS_K_T_STEPS` / `BONSAI_PASS_K_T_SECONDS` 新規 | OK default 未設定で既存挙動 |

**signature 変更ゼロ** — 全 additive、項目 205 のような必須化はなし、`run_k` の caller (4 箇所、experiment.rs:582/595/868/1017) は無変更で動作。

---

## 7. Risks / Mitigations

| # | Risk | 影響 | Mitigation |
|---|------|-----|----------|
| **R1** | env 未指定 default の T 値選定基準不明確 (3, 5, 7? 60, 300, 600?) | 出力の解釈困難 | (i) **default 未指定 = 出力なし** (= active gate 化なしの informational 設計と整合) (ii) docs に「smoke 5 task の `max_iterations` 中央値が 4 -> T_steps=[3,5,7] 推奨、Lab core 22 baseline 平均 duration_secs / k ≈ 50 sec/run -> T_seconds=[60,180,600] 推奨」と明記 (iii) Lab v18+ baseline で実測値 percentile 取得後に default 候補化 (別 plan) |
| **R2** | k=3 で T 軸 sample size 不足、PASS@(k,T) noisy | active gate 化困難 | (i) **本 plan は active gate 化しない** (ii) k>=5 推奨は別 plan、Lab v15 で k=3 が default (iii) CLAUDE.md / docstring に「k=3 では PASS@(k,T) は noisy、実用判断は累積複数 cycle 観測で」明記 |
| **R3** | per-run `Instant::now()` 計測で retry / 例外パスが含まれる | duration が真の interaction depth と乖離 | (i) `run_agent_loop` 内 retry 含む total wallclock は agent 系の正しい T 軸 (論文の interaction depth 定義と整合) (ii) 失敗 run も elapsed を push (RDC と一貫) |
| **R4** | SQLite V13 migration で本番 DB 互換性 | 既存 .bonsai/db で ALTER TABLE 失敗 | (i) `migrate.rs` 既存パターン (V11/V12 と同形) (ii) `PRAGMA user_version` チェック必須 (iii) Phase 2 で in-memory DB migration test 必須 |
| **R5** | TSV 17 列化で外部解析 script 破損 | grafana / jupyter / shell 解析 | (i) 末尾 2 列追加で前 15 列 semantic 不変 (ii) header 駆動 reader robust (iii) CLAUDE.md / Phase 5 で TSV 列番号変更明記 |
| **R6** | `pass_at_k_t_seconds` の f64 キーが浮動小数点比較で BTreeMap キー不可 | composite 集計で同一閾値が分裂 | (i) `composite_pass_at_k_t_seconds` で `1e-6 epsilon` 比較で bucket 化 (§4.4 実装) (ii) env 解析時に `parse_t_seconds_env` で重複除去推奨 (iii) test で確認 |
| **R7** | env 1 回解析で global state 化、test で 並列実行時の干渉 | flaky test | (i) env read は `run_k` 入口で 1 度のみ、`thread::current()` レベルで isolation (ii) test は `compute_pass_at_k_t_steps` を直接 test、`from_scores_with_metrics_v2` 引数経由で env 依存ゼロ |
| **R8** | T_seconds の wallclock 計測がモデル backend に依存 (MLX vs llama-server で 28% 差、項目 183) | 異なる backend 比較で score 軸と PASS@(k, T_sec) 軸が逆転する | (i) **本 plan は backend 比較を scope 外** (ii) 同一 backend 内での変異効果評価のみ (iii) backend 比較は別 plan で T_steps 軸 (backend 非依存) を中心に |
| **R9** | composite 集計の Vec<(f64, f64)> 順序保証 | log 出力で T 値順序が乱れる | (i) `composite_pass_at_k_t_seconds` 末尾で `sort_by(partial_cmp)` で T 昇順保証 (ii) test で順序確認 |

---

## 8. Quality Gates

| Gate | 条件 | 失敗時の対処 |
|------|------|-----------|
| **G-1** | `rtk cargo test --release --lib` で **1150 -> 1159 passed** (退行ゼロ) | 既存 tests 退行は即修正、新規 9 tests Red 残ったら Phase 2 不完全 |
| **G-2** | smoke baseline `composite_score` の variance 範囲内 (vs handoff 05-08 baseline 0.7344 ± 0.03) | regression なら新メトリクス計算が hot path を阻害している疑い、`run_k` per-run `Instant::now()` overhead 計測 (1ms/run 未満で問題なし) |
| **G-3** | PASS@(k, T) の手計算検証 1 件以上一致 | 例: scores=[1.0, 1.0, 0.0], iters=[2, 5, 10], threshold=0.5, T_steps=[3, 5, 10] で `[(3, 0.333), (5, 0.667), (10, 0.667)]` を Python で別計算し ±1e-6 一致 |
| **G-4** (informational) | (a) env 未指定 smoke で TSV 17 列末尾 `-`、既存挙動 100% 互換 (b) env 指定 smoke で T_steps / T_seconds 両軸 log + TSV 出力 (c) 手計算検証 1 件以上一致 | 失敗時は env 解析 / `from_scores_with_metrics_v2` 経路を順に debug |
| **G-5** (informational) | clippy / fmt 0 warning | 通常通り |

---

## 9. 完了条件

1. `cargo test --release --lib`: 1150 -> **1159 passed** (+9 新規、退行ゼロ)
2. `cargo clippy --release --lib --tests -- -D warnings`: 0 warning
3. `cargo fmt --check`: 0 件
4. SQLite V12 -> V13 migration が in-memory DB で適用成功 (既存 row 保持)
5. TSV 17 列ヘッダで新規セッションが起動成功、env 未指定で末尾 2 列が `-`
6. smoke baseline `composite_score` が直近 baseline (handoff 05-08, 0.7344 等) の variance 範囲内 (±0.03)
7. env 指定 smoke で `[INFO][lab.pass_k_t]` ログが T_steps / T_seconds 両軸出力
8. PASS@(k, T) の手計算検証 1 件で ±1e-6 一致 (G-3)
9. CLAUDE.md 項目 223 追記 (1 行 summary)
10. 5 commits 単位で push 可能、未 commit は plan 修正コミットのみ

---

## 10. 見積もり

| Phase | 内容 | 所要 |
|-------|------|------|
| Phase 1 | Red tests 9 件 (benchmark 7 件 / experiment_log 2 件) + helper fixture | ~45 min |
| Phase 2 | Green 実装: 2 fn + from_scores_with_metrics_v2 + composite × 2 + env parser × 2 + run_k integration + Experiment 2 field + migration V13 + TSV 17 列 + save/load SQL | ~120 min |
| Phase 3 | Refactor: dedup helper / Lab log macro / docstring / migration test | ~30 min |
| Phase 4 | smoke 3 段 (env 未指定 / 指定 / 手計算検証、llama-only ~25 min wallclock) | ~45 min |
| Phase 5 | CLAUDE.md 項目 223 + handoff + 5 commits | ~30 min |
| Buffer | 1bit variance debug / TSV grep 互換性確認 | ~30 min |
| **合計** | | **~5h (0.5-1 day scope)** |

---

## 11. Quick Start

```bash
# 0. 既存 caller / call site 全網羅
rtk grep -rn "from_scores_with_metrics" /Users/keizo/bonsai-agent/src/        # 期待 1 caller
rtk grep -rn "MultiRunTaskScore::from_scores" /Users/keizo/bonsai-agent/src/  # 期待 multi
rtk grep -rn "BONSAI_PASS_K_T" /Users/keizo/bonsai-agent/src/                 # 期待 0 (新規 env)
rtk grep -rn "iterations_per_run\|durations_per_run" /Users/keizo/bonsai-agent/src/

# 1. Phase 1 Red
$EDITOR /Users/keizo/bonsai-agent/src/agent/benchmark.rs
rtk cargo test --release --lib pass_at_k_t          # compile error or fail = Red

# 2. Phase 2 Green
$EDITOR /Users/keizo/bonsai-agent/src/agent/benchmark.rs        # pass_at_k_t_* + from_scores_with_metrics_v2 + run_k integration + parse_t_*_env
$EDITOR /Users/keizo/bonsai-agent/src/agent/experiment_log.rs   # Experiment 2 field + save/load SQL + TSV 17 列
$EDITOR /Users/keizo/bonsai-agent/src/db/migrate.rs             # V12 -> V13 ALTER TABLE × 2
rtk cargo test --release --lib                                  # 1159 passed

# 3. Phase 3 Refactor
$EDITOR /Users/keizo/bonsai-agent/src/agent/experiment.rs       # Lab summary log macro
$EDITOR /Users/keizo/bonsai-agent/src/agent/benchmark.rs        # docstring + dedup
rtk cargo clippy --release --lib --tests -- -D warnings
rtk cargo fmt --check

# 4. Phase 4 Smoke 3 段 (要 llama-server 起動)
rtk cargo build --release
# 4a: env 未指定 (既存挙動互換)
BONSAI_LAB_SMOKE=1 ./target/release/bonsai-agent --lab --lab-experiments 0  2>&1 | tee /tmp/g4a.log
# 4b: env 指定 (T_steps + T_seconds 両軸)
BONSAI_LAB_SMOKE=1 BONSAI_PASS_K_T_STEPS=3,5,7 BONSAI_PASS_K_T_SECONDS=60,180,600 \
  ./target/release/bonsai-agent --lab --lab-experiments 0  2>&1 | tee /tmp/g4b.log
rtk grep "lab.pass_k_t" /tmp/g4b.log
# 4c: 手計算検証 1 件
rtk grep "PASS@(k=3" /tmp/g4b.log

# 5. Commit + handoff + CLAUDE.md 項目 223
```

---

## 12. Coordination

> **multi-plan 並列**: AgentFloor (`agentfloor-tier-eval-impl.md`) / ERL Heuristics (`erl-heuristics-pool-impl-v2.md`) と本 plan が並行して `Experiment` / SQLite を拡張中。本 plan は以下で名前空間を独立に保つ。
> - **plan ファイル名**: `pass-k-t-metric-impl.md` (AgentFloor / ERL plan と重複なし)
> - **API 名前空間**: `MultiRunTaskScore.pass_at_k_t_steps` / `pass_at_k_t_seconds`、`MultiRunBenchmarkResult::composite_pass_at_k_t_steps` / `composite_pass_at_k_t_seconds`、`MultiRunTaskScore::from_scores_with_metrics_v2`、`Experiment.pass_at_k_t_steps` / `pass_at_k_t_seconds` (AgentFloor は `tier_avg_scores`、ERL は `heuristics_*` を想定 — 重複なし)
> - **SQLite migration version**: 本 plan は **V13** を予約 (項目 218 が V12 既使用)。AgentFloor (V14 予約候補) / ERL (V10 既確保) と順序付け、本 plan が先行 merge した場合 AgentFloor は V14、後続なら自動上乗せ。
> - **env 名空間**: `BONSAI_PASS_K_T_STEPS` / `BONSAI_PASS_K_T_SECONDS` 新規 (`BONSAI_BENCH_TIER` / `BONSAI_BENCH_LADDER` / `BONSAI_ERL_ENABLED` 等と重複なし)

---

## 13. SESSION_ID (for /ccg:execute)

- **CODEX_SESSION**: (未取得 — 本 plan 起草時点では analyzer prompt 不在、実装着手時に CCG review で取得推奨)
- **GEMINI_SESSION**: (同上)

---

## 14. YAGNI / 見送り (本 plan 範囲外)

| 案 | 見送り理由 | 再評価トリガー |
|----|----------|------------|
| **active gate 化** (`pass_at_k_t_steps[T] > threshold` を ACCEPT 条件追加) | smoke データ蓄積が必要、現行 ACCEPT 基準 (`delta > 0`) との同時運用で混乱、項目 200 stability_delta と同様 informational 先行 | smoke 10+ サイクル経て pass_at_k_t の noise 範囲確定後、別 plan で `ExperimentLoopConfig.t_threshold: Option<f64>` 追加 |
| **2D heatmap 可視化 / dashboard** | 数値出力で十分、UI 要件なし | dashboard で curve 表示要件発生時 |
| **`AgentLoopResult` への `duration_secs` フィールド追加** | per-run wallclock は `run_k` 内 `Instant::now()` で計測十分、API signature 変更不要 | retry 内訳の細分化要件発生時 |
| **論文の RL capability boundary 比較実験** | RL 学習側比較は scope 外、bonsai は post-hoc 評価のみ | RL 統合 (項目 BitRL #2) plan 着手時 |
| **k 軸の動的増加 (k=3->k=5)** | cost 線形増、本 plan は post-hoc 分析で T 軸を低 cost 追加 | k=3 で T 軸 noisy が確定した後 (Lab v18-19 観測後) |
| **同一 k 内で T を変えての追加実行** (max_iterations を Lab 実行時に変更) | cost 倍増、既存 run データを post-hoc 分析することで cost 0 増を達成 | 異なる max_iterations での capability 上限調査要件発生時 |
| **per-task `max_iterations` 動的調整** | task 設計の semantic 変更、本 plan の T 軸計測とは独立した別議論 | task balance 調整要件発生時 (別 plan) |
| **PASS@(k, T) 3D = k × T × tier (AgentFloor 統合)** | AgentFloor 別 plan が tier 軸を確保、本 plan で T 軸を独立に実装後に統合 plan で merge | AgentFloor 完了後 |

### Scope Outside / 別 plan 候補

- **★★ active T gate**: `ExperimentLoopConfig.t_steps_threshold: Option<f64>` 追加、`accepted = (delta > 0) && (pass_at_k_t_steps[primary_T] >= t_threshold)`。smoke 10+ サイクル後着手。
- **★★ AgentFloor × PASS@(k,T) 3D 統合**: `agentfloor-tier-eval-impl.md` 完了後、`tier_pass_at_k_t_steps: [Vec<(usize, f64)>; 6]` で各 tier 別の T 軸データを取得。
- **★ default T 値の自動 percentile 化**: Lab v18-19 baseline で実測 duration_secs / iteration の percentile (p50, p75, p90) を自動 default 値化、env 未指定で意味あるデフォルト出力。

---

## 15. Key Files

| File | Operation | Description | LOC 見積 |
|------|-----------|-------------|---------|
| `src/agent/benchmark.rs` | Modify | `MultiRunTaskScore` 2 Vec フィールド / `compute_pass_at_k_t_*` 2 fn / `from_scores_with_metrics_v2` 新メソッド / `MultiRunBenchmarkResult` composite 2 method / `parse_t_*_env` 2 fn / `run_k` 内 `Instant::now()` + env 解析 + `from_scores_with_metrics_v2` 切替 / tests +7 件 | +180 / -5 |
| `src/agent/experiment.rs` | Modify | `from_multi_results` で composite 呼出 / Lab summary log helper (env 指定時のみ) | +25 |
| `src/agent/experiment_log.rs` | Modify | `Experiment` 2 Vec フィールド / `save_to_db` 2 列追加 SQL (JSON encode) / `recent_experiments` 2 列追加 SQL (JSON decode) / `append_tsv` 17 列ヘッダ + body / tests +2 件 | +80 / -10 |
| `src/db/migrate.rs` | Modify | V12 -> V13 migration SQL (`ALTER TABLE experiments ADD COLUMN pass_at_k_t_{steps,seconds} TEXT`) | +15 |
| `src/db/schema.rs` | Modify | `LATEST_SCHEMA_VERSION` 12 -> 13 (定数あれば) | +1 / -1 |
| `CLAUDE.md` | Modify | 項目 223 追加 (1 行 summary、本 plan §5 Phase 5 に貼付済) | +5 |
| `~/.claude/projects/.../session_2026_05_XX_handoff.md` | Write | handoff 記録 | n/a (Phase 5) |
| `/tmp/bonsai-llama/pass-k-t-smoke-baseline-2026-05-XX.log` | Write (Phase 4) | smoke 結果ログ | n/a |

---

## 16. 参考

### 一次資料
- arxiv 2604.14877 (https://arxiv.org/abs/2604.14877) — Does RL Expand the Capability Boundary of LLM Agents? PASS@(k,T) Analysis
- arxiv 2603.29231 (https://arxiv.org/html/2603.29231v1) — Beyond pass@1 (項目 200 由来、相補的)

### bonsai 内部参照
- 既存 plan: `beyond-pass1-rdc-vaf-impl.md` (項目 200、品質基準・TDD strict 5 phase の手本)
- 既存 plan: `agentfloor-tier-eval-impl.md` (3D 統合の他軸、CapabilityTier × T 軸の足場)
- 既存 plan: `erl-heuristics-pool-impl-v2.md` (項目 213、SQLite V10 確保元)
- 既存 plan: `cerememory-review-state-v12-impl.md` (項目 218、SQLite V12 確保元)
- 既存 plan: `agenther-option-a-migration.md` (項目 205、`run_k` signature 必須化の precedent)
- CLAUDE.md 項目 1 / 200 / 207 / 213 / 215 / 218 / 222
- memory: `research_arxiv_2026_05_07.md` 領域 2 ★★★ 高優先 #2
- memory: `lab_history_v1_v6.md` (天井 7 連続の系譜)

### 既存実装の参照箇所
- `src/agent/benchmark.rs:330-358` — `MultiRunTaskScore` 構造体
- `src/agent/benchmark.rs:418-442` — `from_scores_with_metrics` (本 plan で `_v2` を新設)
- `src/agent/benchmark.rs:507-578` — `MultiRunBenchmarkResult` composite メソッド群
- `src/agent/benchmark.rs:1184-1288` — `BenchmarkSuite::run_k` (per-run iteration 収集の既存箇所)
- `src/agent/experiment_log.rs:131-244` — `Experiment` 構造体 + `from_multi_results`
- `src/agent/experiment_log.rs:285-323` — `append_tsv` (15 列ヘッダ既存)
- `src/agent/agent_loop/state.rs:25-29` — `AgentLoopResult` (本 plan では無変更)

### 派生候補 (本 plan 完了後)
- Lab v18+ で informational 観測 -> noise floor 把握 -> active gate 化 plan
- AgentFloor × PASS@(k,T) 3D 統合 plan (CapabilityTier × T_steps × T_seconds)
- default T percentile 化 plan (実測 baseline からの自動 default)
- backend 比較 plan (MLX vs llama-server で T_seconds 軸の差異定量化、項目 183 副次知見の延長)
