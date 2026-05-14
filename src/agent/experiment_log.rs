use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use crate::agent::benchmark::{BenchmarkResult, MultiRunBenchmarkResult};

/// 変異テーマ（1 iteration 1 themeの原則、経験的プロンプトチューニング知見）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum MutationTheme {
    /// 精密性: ツール使用精度・出力品質向上
    Precision,
    /// 探索性: 多様な解法・創造的アプローチ
    Exploration,
    /// 効率性: ステップ数・トークン消費削減
    Efficiency,
    /// 堅牢性: エラー回復・安定性向上
    Robustness,
}

impl MutationTheme {
    /// サイクル番号からテーマを決定（固定マッピング）
    pub fn from_cycle(cycle: usize) -> Self {
        match cycle % 14 {
            0..=3 => Self::Precision,     // プロンプトルール（8候補ローテーション）
            4..=5 => Self::Efficiency,    // max_iterations ±2
            6..=7 => Self::Exploration,   // max_tools_selected ±2
            8..=9 => Self::Robustness,    // max_retries ±2
            10..=11 => Self::Exploration, // temperature変更（探索軸）
            _ => Self::Efficiency,        // 12, 13: max_tool_output_chars変更
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Precision => "precision",
            Self::Exploration => "exploration",
            Self::Efficiency => "efficiency",
            Self::Robustness => "robustness",
        }
    }
}

/// 変異の種類
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MutationType {
    /// システムプロンプトのルール変更
    PromptRule,
    /// エージェントパラメータ変更
    AgentParam,
    /// Dreamer insightからのヒント追加
    PromptHint,
    /// Hyperagentsメタ変異: 過去のACCEPT変異を組み合わせた複合変異
    MetaMutation,
}

impl MutationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PromptRule => "prompt_rule",
            Self::AgentParam => "agent_param",
            Self::PromptHint => "prompt_hint",
            Self::MetaMutation => "meta_mutation",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "prompt_rule" => Some(Self::PromptRule),
            "agent_param" => Some(Self::AgentParam),
            "prompt_hint" => Some(Self::PromptHint),
            "meta_mutation" => Some(Self::MetaMutation),
            _ => None,
        }
    }
}

/// 過去のACCEPT変異を構造化したアーカイブエントリ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptedMutation {
    /// 変異の種類
    pub mutation_type: MutationType,
    /// 変異の詳細説明
    pub detail: String,
    /// ベースラインからの改善幅
    pub delta: f64,
    /// 適用時のベースラインスコア
    pub baseline_score: f64,
    /// 記録時刻（Unix epoch秒）
    pub timestamp: u64,
}

/// 過去のACCEPT変異アーカイブをDBから読み込み
pub fn load_accepted_archive(conn: &Connection) -> Result<Vec<AcceptedMutation>> {
    let mut stmt = conn.prepare(
        "SELECT mutation_type, mutation_detail, delta, baseline_score, \
         strftime('%s', created_at) as ts \
         FROM experiments WHERE accepted = 1 ORDER BY id ASC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    let mut archive = Vec::new();
    for row in rows {
        let (mt_str, detail, delta, baseline_score, ts_str) = row?;
        let mutation_type = MutationType::parse(&mt_str).unwrap_or(MutationType::PromptRule);
        let timestamp = ts_str.parse::<u64>().unwrap_or(0);
        archive.push(AcceptedMutation {
            mutation_type,
            detail,
            delta,
            baseline_score,
            timestamp,
        });
    }
    Ok(archive)
}

/// 単一の実験記録
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experiment {
    pub experiment_id: String,
    pub mutation_type: MutationType,
    pub mutation_detail: String,
    pub baseline_score: f64,
    pub experiment_score: f64,
    pub delta: f64,
    pub accepted: bool,
    pub duration_secs: f64,
    pub config_snapshot: HashMap<String, String>,
    pub pass_at_k: Option<f64>,
    pub pass_consecutive_k: Option<f64>,
    pub score_variance: Option<f64>,
    /// プリスクリーニングで早期棄却された実験（フルベンチマーク未実行）
    #[serde(default)]
    pub prescreened: bool,
    /// 項目 200 (Beyond pass@1): 実験結果の RDC composite。
    /// `MultiRunBenchmarkResult::composite_reliability_decay` の値。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reliability_decay: Option<f64>,
    /// 項目 200: VAF (baseline.mean_variance に対する experiment.mean_variance の比)。
    /// `baseline.mean_variance() == 0` なら None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance_amplification: Option<f64>,
    /// 項目 200: 実験結果の GDS composite。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graceful_degradation: Option<f64>,
    /// 項目 200: stability_delta = (1 - VAF) + (RDC_exp - RDC_base) + (GDS_exp - GDS_base)。
    /// 本 plan では計算のみ、ACCEPT 判定には未使用 (active gate 化は別 plan)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stability_delta: Option<f64>,
    /// AgentFloor tier map (plan §4.5/§4.6): CapabilityTier T1..T6 の平均スコア。
    /// `MultiRunBenchmarkResult::tier_avg_scores` から設定。ladder mode 非使用時は全 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_t1: Option<f64>, // CapabilityTier::InstructionFollowing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_t2: Option<f64>, // CapabilityTier::SingleToolUse
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_t3: Option<f64>, // CapabilityTier::ToolSelection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_t4: Option<f64>, // CapabilityTier::MultiStepToolChain
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_t5: Option<f64>, // CapabilityTier::ErrorRecovery
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_t6: Option<f64>, // CapabilityTier::LongHorizonPlanning
    /// 項目 225 (arxiv 2604.14877): 実験結果の PASS@(k, T_steps) composite。
    /// 各要素は `(T_steps, pass_rate)`。env `BONSAI_PASS_K_T_STEPS` 未指定なら空 Vec。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_at_k_t_steps: Vec<(usize, f64)>,
    /// 項目 225 (arxiv 2604.14877): 実験結果の PASS@(k, T_seconds) composite。
    /// 各要素は `(T_seconds, pass_rate)`。env `BONSAI_PASS_K_T_SECONDS` 未指定なら空 Vec。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_at_k_t_seconds: Vec<(f64, f64)>,
    /// Frontier benchmark (`frontier-benchmark-impl.md`、antirez/ds4 ds4-bench inspired):
    /// 第 6 軸 context-length axis のため、task ごとの累積 token を bucket 別に集計した
    /// `(bucket_index, mean_score)` を保存。env `BONSAI_FRONTIER_ENABLED` 未指定なら空 Vec。
    /// Sub-Phase 2B では Experiment 持ち回りまで実装、Sub-Phase 2C で `run_k` から populate。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frontier_bucket_scores: Vec<(usize, f64)>,
    /// Frontier benchmark の T6 inject variant (案 C 2nd pillar):
    /// `(filler_kb, score)` ペア。Sub-Phase 2E で T6-LongHorizon に filler context inject する
    /// 設計、Sub-Phase 2B 時点では常に空 Vec で持ち回りのみ。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frontier_inject_scores: Vec<(usize, f64)>,
}

impl Experiment {
    /// ベースラインとベンチマーク結果から実験記録を生成
    pub fn from_results(
        experiment_id: String,
        mutation_type: MutationType,
        mutation_detail: String,
        baseline: &BenchmarkResult,
        experiment: &BenchmarkResult,
        config_snapshot: HashMap<String, String>,
    ) -> Self {
        let baseline_score = baseline.composite_score();
        let experiment_score = experiment.composite_score();
        let delta = experiment_score - baseline_score;
        Self {
            experiment_id,
            mutation_type,
            mutation_detail,
            baseline_score,
            experiment_score,
            delta,
            accepted: delta > 0.0,
            duration_secs: experiment.duration_secs,
            config_snapshot,
            pass_at_k: None,
            pass_consecutive_k: None,
            score_variance: None,
            prescreened: false,
            reliability_decay: None,
            variance_amplification: None,
            graceful_degradation: None,
            stability_delta: None,
            tier_t1: None,
            tier_t2: None,
            tier_t3: None,
            tier_t4: None,
            tier_t5: None,
            tier_t6: None,
            // 項目 225: legacy BenchmarkResult 経路は PASS@(k,T) を計算しない (multi-run のみ対応)
            pass_at_k_t_steps: Vec::new(),
            pass_at_k_t_seconds: Vec::new(),
            frontier_bucket_scores: Vec::new(),
            frontier_inject_scores: Vec::new(),
        }
    }

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
        // 項目 200 (Beyond pass@1): 信頼性メトリクス 3 軸を集計
        let vaf = experiment.variance_amplification_vs(baseline);
        let rdc_exp = experiment.composite_reliability_decay();
        let rdc_base = baseline.composite_reliability_decay();
        let gds_exp = experiment.composite_graceful_degradation();
        let gds_base = baseline.composite_graceful_degradation();
        // stability_delta は VAF が None なら計算不能 (baseline variance=0 のケース)
        let stability_delta = vaf.map(|v| (1.0 - v) + (rdc_exp - rdc_base) + (gds_exp - gds_base));
        // tier_avg_scores から tier_t1..t6 を展開 (ladder mode 非使用時は全 None)
        let tiers = experiment.tier_avg_scores;
        // 項目 225: PASS@(k,T) composite を T 軸 (steps/seconds) 別に集計
        let pass_at_k_t_steps = experiment.composite_pass_at_k_t_steps();
        let pass_at_k_t_seconds = experiment.composite_pass_at_k_t_seconds();
        Self {
            experiment_id,
            mutation_type,
            mutation_detail,
            baseline_score,
            experiment_score,
            delta,
            accepted: delta > 0.0, // 本 plan では ACCEPT 判定基準は変更しない
            duration_secs: experiment.duration_secs,
            config_snapshot,
            pass_at_k: Some(experiment.composite_pass_at_k()),
            pass_consecutive_k: Some(experiment.composite_pass_consecutive_k()),
            score_variance: Some(
                experiment
                    .task_scores
                    .iter()
                    .map(|s| s.variance)
                    .sum::<f64>()
                    / experiment.task_scores.len().max(1) as f64,
            ),
            prescreened: false,
            reliability_decay: Some(rdc_exp),
            variance_amplification: vaf,
            graceful_degradation: Some(gds_exp),
            stability_delta,
            tier_t1: tiers.and_then(|t| t[0]),
            tier_t2: tiers.and_then(|t| t[1]),
            tier_t3: tiers.and_then(|t| t[2]),
            tier_t4: tiers.and_then(|t| t[3]),
            tier_t5: tiers.and_then(|t| t[4]),
            tier_t6: tiers.and_then(|t| t[5]),
            pass_at_k_t_steps,
            pass_at_k_t_seconds,
            // Sub-Phase 2C で `experiment.composite_frontier_bucket_scores()` などから populate 予定。
            // 本 Sub-Phase 2B では struct field の persistence 配線が scope のため Vec::new() で固定。
            frontier_bucket_scores: Vec::new(),
            frontier_inject_scores: Vec::new(),
        }
    }
}

/// 実験ログの永続化（SQLite + TSV）
pub struct ExperimentLog;

impl ExperimentLog {
    /// SQLiteに実験を記録
    pub fn save_to_db(conn: &Connection, exp: &Experiment) -> Result<()> {
        // 項目 225 (PASS@(k,T)): T 閾値数は env 駆動で可変のため JSON TEXT として保存。
        // 空 Vec は `"[]"` に encode され、reader は from_str で空 Vec を得る (NULL と区別)。
        let pass_at_k_t_steps_json = serde_json::to_string(&exp.pass_at_k_t_steps)?;
        let pass_at_k_t_seconds_json = serde_json::to_string(&exp.pass_at_k_t_seconds)?;
        // V16 (frontier benchmark): Vec<(usize, f64)> も同 pattern で JSON TEXT 永続化。
        let frontier_bucket_json = serde_json::to_string(&exp.frontier_bucket_scores)?;
        let frontier_inject_json = serde_json::to_string(&exp.frontier_inject_scores)?;
        conn.execute(
            "INSERT INTO experiments (experiment_id, mutation_type, mutation_detail, \
             baseline_score, experiment_score, delta, accepted, duration_secs, prescreened, \
             reliability_decay, variance_amplification, graceful_degradation, stability_delta, \
             tier_t1, tier_t2, tier_t3, tier_t4, tier_t5, tier_t6, \
             pass_at_k_t_steps, pass_at_k_t_seconds, \
             frontier_bucket_scores, frontier_inject_scores) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, \
             ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![
                exp.experiment_id,
                exp.mutation_type.as_str(),
                exp.mutation_detail,
                exp.baseline_score,
                exp.experiment_score,
                exp.delta,
                exp.accepted as i32,
                exp.duration_secs,
                exp.prescreened as i32,
                exp.reliability_decay,
                exp.variance_amplification,
                exp.graceful_degradation,
                exp.stability_delta,
                exp.tier_t1,
                exp.tier_t2,
                exp.tier_t3,
                exp.tier_t4,
                exp.tier_t5,
                exp.tier_t6,
                pass_at_k_t_steps_json,
                pass_at_k_t_seconds_json,
                frontier_bucket_json,
                frontier_inject_json,
            ],
        )?;

        for (key, value) in &exp.config_snapshot {
            conn.execute(
                "INSERT INTO experiment_config (experiment_id, config_key, config_value) \
                 VALUES (?1, ?2, ?3)",
                params![exp.experiment_id, key, value],
            )?;
        }
        Ok(())
    }

    /// TSVファイルに追記（ヘッダーがなければ追加）
    pub fn append_tsv(path: &Path, exp: &Experiment) -> Result<()> {
        let needs_header = !path.exists() || std::fs::metadata(path)?.len() == 0;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        if needs_header {
            writeln!(
                file,
                "experiment_id\tmutation_type\tmutation_detail\tbaseline_score\texperiment_score\t\
                 delta\taccepted\tduration_secs\tpass_at_k\tpass_consecutive_k\tscore_variance\t\
                 prescreened\treliability_decay\tvariance_amplification\tgraceful_degradation\t\
                 tier_t1\ttier_t2\ttier_t3\ttier_t4\ttier_t5\ttier_t6\t\
                 pass_at_k_t_steps\tpass_at_k_t_seconds\t\
                 frontier_bucket_scores\tfrontier_inject_scores"
            )?;
        }

        // 項目 225 (PASS@(k,T)): TSV 末尾 2 列。空 Vec は `-` で表記 (環境変数未指定 /
        // pre-screen REJECT 等で計測がない場合と一致)、非空時は JSON 配列をそのまま記録。
        // `replace('\t', " ")` は JSON 内に `\t` を含まないため不要だが防御的に統一。
        let pass_at_k_t_steps = if exp.pass_at_k_t_steps.is_empty() {
            "-".to_string()
        } else {
            serde_json::to_string(&exp.pass_at_k_t_steps).unwrap_or_else(|_| "-".into())
        };
        let pass_at_k_t_seconds = if exp.pass_at_k_t_seconds.is_empty() {
            "-".to_string()
        } else {
            serde_json::to_string(&exp.pass_at_k_t_seconds).unwrap_or_else(|_| "-".into())
        };
        // V16 (frontier benchmark): TSV 末尾 2 列。PASS@(k,T) 同 pattern で空 Vec → `-`。
        let frontier_bucket = if exp.frontier_bucket_scores.is_empty() {
            "-".to_string()
        } else {
            serde_json::to_string(&exp.frontier_bucket_scores).unwrap_or_else(|_| "-".into())
        };
        let frontier_inject = if exp.frontier_inject_scores.is_empty() {
            "-".to_string()
        } else {
            serde_json::to_string(&exp.frontier_inject_scores).unwrap_or_else(|_| "-".into())
        };

        writeln!(
            file,
            "{}\t{}\t{}\t{:.4}\t{:.4}\t{:.4}\t{}\t{:.2}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            exp.experiment_id,
            exp.mutation_type.as_str(),
            exp.mutation_detail.replace('\t', " "),
            exp.baseline_score,
            exp.experiment_score,
            exp.delta,
            exp.accepted,
            exp.duration_secs,
            exp.pass_at_k.map_or("-".to_string(), |v| format!("{v:.4}")),
            exp.pass_consecutive_k
                .map_or("-".to_string(), |v| format!("{v:.4}")),
            exp.score_variance
                .map_or("-".to_string(), |v| format!("{v:.6}")),
            exp.prescreened,
            exp.reliability_decay
                .map_or("-".to_string(), |v| format!("{v:.4}")),
            exp.variance_amplification
                .map_or("-".to_string(), |v| format!("{v:.4}")),
            exp.graceful_degradation
                .map_or("-".to_string(), |v| format!("{v:.4}")),
            exp.tier_t1.map_or("-".to_string(), |v| format!("{v:.4}")),
            exp.tier_t2.map_or("-".to_string(), |v| format!("{v:.4}")),
            exp.tier_t3.map_or("-".to_string(), |v| format!("{v:.4}")),
            exp.tier_t4.map_or("-".to_string(), |v| format!("{v:.4}")),
            exp.tier_t5.map_or("-".to_string(), |v| format!("{v:.4}")),
            exp.tier_t6.map_or("-".to_string(), |v| format!("{v:.4}")),
            pass_at_k_t_steps,
            pass_at_k_t_seconds,
            frontier_bucket,
            frontier_inject,
        )?;
        Ok(())
    }

    /// 直近N件の実験をDBから取得（新しい順）
    pub fn recent_experiments(conn: &Connection, limit: usize) -> Result<Vec<Experiment>> {
        let mut stmt = conn.prepare(
            "SELECT experiment_id, mutation_type, mutation_detail, \
             baseline_score, experiment_score, delta, accepted, duration_secs, \
             COALESCE(prescreened, 0), \
             reliability_decay, variance_amplification, graceful_degradation, stability_delta, \
             tier_t1, tier_t2, tier_t3, tier_t4, tier_t5, tier_t6, \
             pass_at_k_t_steps, pass_at_k_t_seconds, \
             frontier_bucket_scores, frontier_inject_scores \
             FROM experiments ORDER BY id DESC LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, i32>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, i32>(8)?,
                row.get::<_, Option<f64>>(9)?,
                row.get::<_, Option<f64>>(10)?,
                row.get::<_, Option<f64>>(11)?,
                row.get::<_, Option<f64>>(12)?,
                row.get::<_, Option<f64>>(13)?,
                row.get::<_, Option<f64>>(14)?,
                row.get::<_, Option<f64>>(15)?,
                row.get::<_, Option<f64>>(16)?,
                row.get::<_, Option<f64>>(17)?,
                row.get::<_, Option<f64>>(18)?,
                row.get::<_, Option<String>>(19)?,
                row.get::<_, Option<String>>(20)?,
                row.get::<_, Option<String>>(21)?,
                row.get::<_, Option<String>>(22)?,
            ))
        })?;

        // config_snapshot用のステートメントをループ外で準備
        let mut config_stmt = conn.prepare(
            "SELECT config_key, config_value FROM experiment_config WHERE experiment_id = ?1",
        )?;

        let mut experiments = Vec::new();
        for row in rows {
            let (
                id,
                mt,
                detail,
                baseline,
                score,
                delta,
                accepted,
                dur,
                prescreened,
                rdc,
                vaf,
                gds,
                stab,
                t1,
                t2,
                t3,
                t4,
                t5,
                t6,
                pass_at_k_t_steps_json,
                pass_at_k_t_seconds_json,
                frontier_bucket_json,
                frontier_inject_json,
            ) = row?;
            let mutation_type = MutationType::parse(&mt).unwrap_or(MutationType::PromptRule);

            let config: HashMap<String, String> = config_stmt
                .query_map(params![id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .collect();

            // 項目 225 (PASS@(k,T)): TEXT 列を JSON decode。NULL / 解析失敗 → 空 Vec。
            // env 未指定セッションの旧 row (V15 migration 前) は NULL になるため
            // `and_then(|json| serde_json::from_str(...))` で `None` を吸収する。
            let pass_at_k_t_steps = pass_at_k_t_steps_json
                .and_then(|json| serde_json::from_str::<Vec<(usize, f64)>>(&json).ok())
                .unwrap_or_default();
            let pass_at_k_t_seconds = pass_at_k_t_seconds_json
                .and_then(|json| serde_json::from_str::<Vec<(f64, f64)>>(&json).ok())
                .unwrap_or_default();
            // V16 (frontier benchmark): PASS@(k,T) 同 pattern で NULL / parse 失敗 → 空 Vec。
            let frontier_bucket_scores = frontier_bucket_json
                .and_then(|json| serde_json::from_str::<Vec<(usize, f64)>>(&json).ok())
                .unwrap_or_default();
            let frontier_inject_scores = frontier_inject_json
                .and_then(|json| serde_json::from_str::<Vec<(usize, f64)>>(&json).ok())
                .unwrap_or_default();

            experiments.push(Experiment {
                experiment_id: id,
                mutation_type,
                mutation_detail: detail,
                baseline_score: baseline,
                experiment_score: score,
                delta,
                accepted: accepted != 0,
                duration_secs: dur,
                config_snapshot: config,
                pass_at_k: None,
                pass_consecutive_k: None,
                score_variance: None,
                prescreened: prescreened != 0,
                reliability_decay: rdc,
                variance_amplification: vaf,
                graceful_degradation: gds,
                stability_delta: stab,
                tier_t1: t1,
                tier_t2: t2,
                tier_t3: t3,
                tier_t4: t4,
                tier_t5: t5,
                tier_t6: t6,
                pass_at_k_t_steps,
                pass_at_k_t_seconds,
                frontier_bucket_scores,
                frontier_inject_scores,
            });
        }
        Ok(experiments)
    }

    /// 承認率を計算
    pub fn acceptance_rate(conn: &Connection) -> Result<f64> {
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM experiments", [], |r| r.get(0))?;
        if total == 0 {
            return Ok(0.0);
        }
        let accepted: i64 = conn.query_row(
            "SELECT COUNT(*) FROM experiments WHERE accepted = 1",
            [],
            |r| r.get(0),
        )?;
        Ok(accepted as f64 / total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::benchmark::{BenchmarkResult, TaskScore};
    use crate::db::migrate;
    use tempfile::TempDir;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // V1, V2, V7 (prescreened), V9 (信頼性メトリクス), V14 (tier map), V15 (PASS@(k,T)),
        // V16 (frontier benchmark) 適用 — save_to_db が V16 列を INSERT するため必須。
        for version in [1, 2, 7, 9, 14, 15, 16] {
            let sql = migrate::get_migration_sql(version).unwrap();
            conn.execute_batch(sql).unwrap();
        }
        conn
    }

    fn sample_experiment(id: &str, delta: f64) -> Experiment {
        Experiment {
            experiment_id: id.into(),
            mutation_type: MutationType::PromptRule,
            mutation_detail: "ツール使用前に<think>で考える".into(),
            baseline_score: 0.5,
            experiment_score: 0.5 + delta,
            delta,
            accepted: delta > 0.0,
            duration_secs: 10.0,
            config_snapshot: HashMap::from([("max_iterations".into(), "10".into())]),
            pass_at_k: None,
            pass_consecutive_k: None,
            score_variance: None,
            prescreened: false,
            reliability_decay: None,
            variance_amplification: None,
            graceful_degradation: None,
            stability_delta: None,
            tier_t1: None,
            tier_t2: None,
            tier_t3: None,
            tier_t4: None,
            tier_t5: None,
            tier_t6: None,
            pass_at_k_t_steps: Vec::new(),
            pass_at_k_t_seconds: Vec::new(),
            frontier_bucket_scores: Vec::new(),
            frontier_inject_scores: Vec::new(),
        }
    }

    #[test]
    fn test_mutation_type_roundtrip() {
        for mt in [
            MutationType::PromptRule,
            MutationType::AgentParam,
            MutationType::PromptHint,
        ] {
            let s = mt.as_str();
            let back = MutationType::parse(s).unwrap();
            assert_eq!(mt, back);
        }
    }

    #[test]
    fn test_experiment_from_results() {
        let baseline = BenchmarkResult {
            task_scores: vec![TaskScore {
                task_id: "a".into(),
                completed: true,
                correct_tools: 1.0,
                keyword_hits: 1.0,
                iterations_used: 1,
                iteration_budget: 3,
            }],
            duration_secs: 5.0,
        };
        let experiment = BenchmarkResult {
            task_scores: vec![TaskScore {
                task_id: "a".into(),
                completed: true,
                correct_tools: 1.0,
                keyword_hits: 1.0,
                iterations_used: 0,
                iteration_budget: 3,
            }],
            duration_secs: 6.0,
        };
        let exp = Experiment::from_results(
            "exp_001".into(),
            MutationType::AgentParam,
            "max_iterations: 10→12".into(),
            &baseline,
            &experiment,
            HashMap::new(),
        );
        assert!(exp.delta > 0.0);
        assert!(exp.accepted);
    }

    #[test]
    fn test_save_and_retrieve_from_db() {
        let conn = setup_test_db();
        let exp = sample_experiment("exp_db_01", 0.1);
        ExperimentLog::save_to_db(&conn, &exp).unwrap();
        let results = ExperimentLog::recent_experiments(&conn, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].experiment_id, "exp_db_01");
        assert!(results[0].accepted);
        assert_eq!(
            results[0].config_snapshot.get("max_iterations").unwrap(),
            "10"
        );
    }

    #[test]
    fn test_save_rejected_experiment() {
        let conn = setup_test_db();
        let exp = sample_experiment("exp_reject", -0.05);
        ExperimentLog::save_to_db(&conn, &exp).unwrap();
        let results = ExperimentLog::recent_experiments(&conn, 10).unwrap();
        assert!(!results[0].accepted);
    }

    #[test]
    fn test_recent_experiments_ordering() {
        let conn = setup_test_db();
        ExperimentLog::save_to_db(&conn, &sample_experiment("exp_01", 0.1)).unwrap();
        ExperimentLog::save_to_db(&conn, &sample_experiment("exp_02", -0.05)).unwrap();
        ExperimentLog::save_to_db(&conn, &sample_experiment("exp_03", 0.2)).unwrap();
        let results = ExperimentLog::recent_experiments(&conn, 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].experiment_id, "exp_03");
    }

    #[test]
    fn test_acceptance_rate() {
        let conn = setup_test_db();
        ExperimentLog::save_to_db(&conn, &sample_experiment("a", 0.1)).unwrap();
        ExperimentLog::save_to_db(&conn, &sample_experiment("b", -0.1)).unwrap();
        ExperimentLog::save_to_db(&conn, &sample_experiment("c", 0.2)).unwrap();
        let rate = ExperimentLog::acceptance_rate(&conn).unwrap();
        assert!((rate - 2.0 / 3.0).abs() < 0.001);
    }

    #[test]
    fn test_acceptance_rate_empty() {
        let conn = setup_test_db();
        let rate = ExperimentLog::acceptance_rate(&conn).unwrap();
        assert!((rate).abs() < f64::EPSILON);
    }

    #[test]
    fn test_tsv_append() {
        let dir = TempDir::new().unwrap();
        let tsv_path = dir.path().join("experiments.tsv");
        ExperimentLog::append_tsv(&tsv_path, &sample_experiment("tsv_01", 0.1)).unwrap();
        ExperimentLog::append_tsv(&tsv_path, &sample_experiment("tsv_02", -0.05)).unwrap();
        let content = std::fs::read_to_string(&tsv_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("experiment_id"));
    }

    #[test]
    fn test_tsv_tab_in_detail_is_sanitized() {
        let dir = TempDir::new().unwrap();
        let tsv_path = dir.path().join("experiments.tsv");
        let mut exp = sample_experiment("tsv_tab", 0.0);
        exp.mutation_detail = "with\ttab".into();
        ExperimentLog::append_tsv(&tsv_path, &exp).unwrap();
        let content = std::fs::read_to_string(&tsv_path).unwrap();
        let data_line = content.lines().nth(1).unwrap();
        // 項目 200/223/225 + V16 frontier: 12 列 + 信頼性メトリクス 3 列 (rdc/vaf/gds) + tier 6 列
        // + PASS@(k,T) 2 列 (steps/seconds) + frontier 2 列 (bucket/inject) = 25 列
        assert_eq!(data_line.split('\t').count(), 25);
    }

    #[test]
    fn test_duplicate_experiment_id_rejected() {
        let conn = setup_test_db();
        let exp = sample_experiment("dup_01", 0.1);
        ExperimentLog::save_to_db(&conn, &exp).unwrap();
        assert!(ExperimentLog::save_to_db(&conn, &exp).is_err());
    }

    #[test]
    fn test_mutation_type_meta_mutation_roundtrip() {
        let mt = MutationType::MetaMutation;
        assert_eq!(mt.as_str(), "meta_mutation");
        let parsed = MutationType::parse("meta_mutation").unwrap();
        assert_eq!(parsed, MutationType::MetaMutation);
    }

    #[test]
    fn test_load_accepted_archive_empty() {
        let conn = setup_test_db();
        let archive = load_accepted_archive(&conn).unwrap();
        assert!(archive.is_empty());
    }

    #[test]
    fn test_load_accepted_archive_filters_rejected() {
        let conn = setup_test_db();
        ExperimentLog::save_to_db(&conn, &sample_experiment("acc_01", 0.1)).unwrap();
        ExperimentLog::save_to_db(&conn, &sample_experiment("rej_01", -0.05)).unwrap();
        let archive = load_accepted_archive(&conn).unwrap();
        assert_eq!(archive.len(), 1, "ACCEPTのみアーカイブされる");
        assert!(archive[0].delta > 0.0);
    }

    #[test]
    fn test_load_accepted_archive_preserves_order() {
        let conn = setup_test_db();
        ExperimentLog::save_to_db(&conn, &sample_experiment("a_first", 0.05)).unwrap();
        ExperimentLog::save_to_db(&conn, &sample_experiment("b_second", 0.2)).unwrap();
        let archive = load_accepted_archive(&conn).unwrap();
        assert_eq!(archive.len(), 2);
        assert!(archive[0].delta < archive[1].delta);
    }

    #[test]
    fn test_load_accepted_archive_fields() {
        let conn = setup_test_db();
        let mut exp = sample_experiment("field_test", 0.15);
        exp.mutation_type = MutationType::PromptRule;
        exp.mutation_detail = "test mutation detail".into();
        exp.baseline_score = 0.75;
        ExperimentLog::save_to_db(&conn, &exp).unwrap();
        let archive = load_accepted_archive(&conn).unwrap();
        assert_eq!(archive.len(), 1);
        assert_eq!(archive[0].mutation_type, MutationType::PromptRule);
        assert_eq!(archive[0].detail, "test mutation detail");
        assert!((archive[0].delta - 0.15).abs() < 0.001);
        assert!((archive[0].baseline_score - 0.75).abs() < 0.001);
    }

    #[test]
    fn t_experiment_from_multi_results_includes_stability_delta() {
        // 項目 200 (Beyond pass@1): from_multi_results が信頼性メトリクスを設定すること
        use crate::agent::benchmark::{MultiRunBenchmarkResult, MultiRunTaskScore};
        let baseline = MultiRunBenchmarkResult {
            task_scores: vec![MultiRunTaskScore::from_scores(
                "t".into(),
                vec![1.0, 1.0, 1.0],
                0.5,
            )], // var=0 → VAF=None
            duration_secs: 0.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        let experiment_result = MultiRunBenchmarkResult {
            task_scores: vec![MultiRunTaskScore::from_scores(
                "t".into(),
                vec![1.0, 0.5, 0.0],
                0.5,
            )],
            duration_secs: 0.0,
            core_avg_score: None,
            extended_avg_score: None,
            tier_avg_scores: None,
            critic_stats: None,
        };
        let exp = Experiment::from_multi_results(
            "e1".into(),
            MutationType::PromptRule,
            "test".into(),
            &baseline,
            &experiment_result,
            HashMap::new(),
        );
        // RDC/GDS は composite メソッドから設定される
        assert!(
            exp.reliability_decay.is_some(),
            "reliability_decay should be Some after from_multi_results"
        );
        assert!(
            exp.graceful_degradation.is_some(),
            "graceful_degradation should be Some after from_multi_results"
        );
        // baseline var=0 → VAF=None → stability_delta=None
        assert!(
            exp.variance_amplification.is_none(),
            "VAF should be None when baseline variance is 0"
        );
        assert!(
            exp.stability_delta.is_none(),
            "stability_delta should be None when VAF is None"
        );
    }

    #[test]
    fn test_accepted_mutation_struct_clone() {
        let am = AcceptedMutation {
            mutation_type: MutationType::MetaMutation,
            detail: "compound test".into(),
            delta: 0.05,
            baseline_score: 0.8,
            timestamp: 1000,
        };
        let cloned = am.clone();
        assert_eq!(cloned.mutation_type, MutationType::MetaMutation);
        assert_eq!(cloned.detail, "compound test");
    }

    // ── Phase 4 AgentFloor tier map tests (plan §4.5/§4.6) ──────────────────

    fn setup_test_db_v14() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // V14 (AgentFloor tier 列) まで必要な migration を適用。
        // 項目 225 で save_to_db が V15 列 (pass_at_k_t_*) を INSERT するため
        // V15 も併せて適用しないと既存 tier round-trip テストが失敗する。
        // V16 (frontier benchmark) も save_to_db が INSERT するため必須。
        for version in [1, 2, 7, 9, 14, 15, 16] {
            let sql = migrate::get_migration_sql(version).unwrap();
            conn.execute_batch(sql).unwrap();
        }
        conn
    }

    fn sample_experiment_with_tiers(id: &str, delta: f64) -> Experiment {
        Experiment {
            experiment_id: id.into(),
            mutation_type: MutationType::PromptRule,
            mutation_detail: "tier test".into(),
            baseline_score: 0.5,
            experiment_score: 0.5 + delta,
            delta,
            accepted: delta > 0.0,
            duration_secs: 10.0,
            config_snapshot: HashMap::new(),
            pass_at_k: None,
            pass_consecutive_k: None,
            score_variance: None,
            prescreened: false,
            reliability_decay: None,
            variance_amplification: None,
            graceful_degradation: None,
            stability_delta: None,
            tier_t1: Some(0.80),
            tier_t2: Some(0.70),
            tier_t3: Some(0.60),
            tier_t4: Some(0.50),
            tier_t5: Some(0.40),
            tier_t6: Some(0.30),
            pass_at_k_t_steps: Vec::new(),
            pass_at_k_t_seconds: Vec::new(),
            frontier_bucket_scores: Vec::new(),
            frontier_inject_scores: Vec::new(),
        }
    }

    /// 1. Experiment struct に tier_t1..t6 フィールドが存在し default None
    #[test]
    fn test_experiment_tier_fields_default_none() {
        let exp = Experiment {
            experiment_id: "def".into(),
            mutation_type: MutationType::PromptRule,
            mutation_detail: "".into(),
            baseline_score: 0.0,
            experiment_score: 0.0,
            delta: 0.0,
            accepted: false,
            duration_secs: 0.0,
            config_snapshot: HashMap::new(),
            pass_at_k: None,
            pass_consecutive_k: None,
            score_variance: None,
            prescreened: false,
            reliability_decay: None,
            variance_amplification: None,
            graceful_degradation: None,
            stability_delta: None,
            tier_t1: None,
            tier_t2: None,
            tier_t3: None,
            tier_t4: None,
            tier_t5: None,
            tier_t6: None,
            pass_at_k_t_steps: Vec::new(),
            pass_at_k_t_seconds: Vec::new(),
            frontier_bucket_scores: Vec::new(),
            frontier_inject_scores: Vec::new(),
        };
        assert!(exp.tier_t1.is_none());
        assert!(exp.tier_t2.is_none());
        assert!(exp.tier_t3.is_none());
        assert!(exp.tier_t4.is_none());
        assert!(exp.tier_t5.is_none());
        assert!(exp.tier_t6.is_none());
        assert!(exp.pass_at_k_t_steps.is_empty());
        assert!(exp.pass_at_k_t_seconds.is_empty());
    }

    /// 2. tier 値ありの場合 23 列出力 (header + data row、項目 225 で 21→23 拡張)
    #[test]
    fn test_append_tsv_23_columns_with_tier() {
        let dir = TempDir::new().unwrap();
        let tsv_path = dir.path().join("tier_23.tsv");
        let exp = sample_experiment_with_tiers("tsv_tier_01", 0.1);
        ExperimentLog::append_tsv(&tsv_path, &exp).unwrap();
        let content = std::fs::read_to_string(&tsv_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "header + 1 data row");
        let header_cols = lines[0].split('\t').count();
        let data_cols = lines[1].split('\t').count();
        assert_eq!(header_cols, 25, "header は 25 列 (V16 frontier 2 列追加)");
        assert_eq!(data_cols, 25, "data row は 25 列 (V16 frontier 2 列追加)");
        assert!(
            lines[0].contains("tier_t1"),
            "header に tier_t1 が含まれること"
        );
        assert!(
            lines[0].contains("tier_t6"),
            "header に tier_t6 が含まれること"
        );
        assert!(
            lines[0].ends_with(
                "pass_at_k_t_steps\tpass_at_k_t_seconds\tfrontier_bucket_scores\tfrontier_inject_scores"
            ),
            "frontier 列は PASS@(k,T) 列の後に追加されること"
        );
        assert!(
            lines[1].contains("0.8000"),
            "t1 score が data に含まれること"
        );
    }

    /// 3. tier 全 None + PASS@(k,T) + frontier 全 None / 空 Vec でも 25 列出力、末尾 10 列は "-"
    #[test]
    fn test_append_tsv_23_columns_tier_none_dash() {
        let dir = TempDir::new().unwrap();
        let tsv_path = dir.path().join("tier_none.tsv");
        let exp = sample_experiment("tsv_none_01", 0.1);
        ExperimentLog::append_tsv(&tsv_path, &exp).unwrap();
        let content = std::fs::read_to_string(&tsv_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let data_cols: Vec<&str> = lines[1].split('\t').collect();
        assert_eq!(
            data_cols.len(),
            25,
            "tier None + PASS@(k,T) + frontier 空でも 25 列固定 (V16 で 2 列追加)"
        );
        // 末尾 10 列 (tier 6 + PASS@(k,T) 2 + frontier 2) は全て "-"
        for col in &data_cols[15..25] {
            assert_eq!(*col, "-", "tier/PASS@(k,T)/frontier None 列は '-' 表現");
        }
    }

    /// 4. SQLite に tier 列 6 件 round-trip
    #[test]
    #[allow(clippy::type_complexity)] // 項目 223 由来の 6-Option<f64> 型注釈、テスト内 round-trip 検証専用
    fn test_save_to_db_tier_columns() {
        let conn = setup_test_db_v14();
        let exp = sample_experiment_with_tiers("tier_rt_01", 0.1);
        ExperimentLog::save_to_db(&conn, &exp).unwrap();
        let (t1, t2, t3, t4, t5, t6): (
            Option<f64>,
            Option<f64>,
            Option<f64>,
            Option<f64>,
            Option<f64>,
            Option<f64>,
        ) = conn
            .query_row(
                "SELECT tier_t1, tier_t2, tier_t3, tier_t4, tier_t5, tier_t6 \
                 FROM experiments WHERE experiment_id = ?1",
                rusqlite::params!["tier_rt_01"],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert!((t1.unwrap() - 0.80).abs() < 1e-6);
        assert!((t2.unwrap() - 0.70).abs() < 1e-6);
        assert!((t3.unwrap() - 0.60).abs() < 1e-6);
        assert!((t4.unwrap() - 0.50).abs() < 1e-6);
        assert!((t5.unwrap() - 0.40).abs() < 1e-6);
        assert!((t6.unwrap() - 0.30).abs() < 1e-6);
    }

    /// 5. migration 後 PRAGMA で tier_t1..t6 列存在
    #[test]
    fn test_migration_v14_adds_tier_columns() {
        let conn = setup_test_db_v14();
        let mut stmt = conn.prepare("PRAGMA table_info(experiments)").unwrap();
        let col_names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for col in [
            "tier_t1", "tier_t2", "tier_t3", "tier_t4", "tier_t5", "tier_t6",
        ] {
            assert!(
                col_names.contains(&col.to_string()),
                "列 '{col}' が experiments テーブルに存在すること"
            );
        }
    }

    /// 6. tier None の実験を DB に保存し recent_experiments で取得できること
    #[test]
    fn test_save_to_db_tier_columns_null_roundtrip() {
        let conn = setup_test_db_v14();
        let exp = sample_experiment("tier_null_01", 0.05);
        ExperimentLog::save_to_db(&conn, &exp).unwrap();
        let results = ExperimentLog::recent_experiments(&conn, 1).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].tier_t1.is_none());
        assert!(results[0].tier_t6.is_none());
    }

    // ── 項目 225 (PASS@(k,T)) Red phase tests ──────────────────────────────

    /// serde: Experiment に PASS@(k,T) 2 軸を設定して round-trip
    #[test]
    fn t_experiment_serde_with_pass_k_t() {
        let mut exp = sample_experiment("e1", 0.05);
        exp.pass_at_k_t_steps = vec![(3, 0.33), (5, 0.66)];
        exp.pass_at_k_t_seconds = vec![(60.0, 0.5), (300.0, 0.83)];
        let json = serde_json::to_string(&exp).unwrap();
        let exp2: Experiment = serde_json::from_str(&json).unwrap();
        assert_eq!(exp2.pass_at_k_t_steps.len(), 2);
        assert_eq!(exp2.pass_at_k_t_seconds.len(), 2);
        assert_eq!(exp2.pass_at_k_t_steps[0], (3, 0.33));
        assert!((exp2.pass_at_k_t_seconds[1].0 - 300.0).abs() < 1e-6);
    }

    /// SQLite V15: PASS@(k,T) 列に JSON encode で保存 → recent_experiments で decode
    #[test]
    fn t_experiment_db_roundtrip_with_pass_k_t_v15() {
        let conn = setup_test_db();
        let mut exp = sample_experiment("e_pkt", 0.05);
        exp.pass_at_k_t_steps = vec![(3, 0.33), (5, 0.66)];
        exp.pass_at_k_t_seconds = vec![(60.0, 0.5)];
        ExperimentLog::save_to_db(&conn, &exp).unwrap();
        let results = ExperimentLog::recent_experiments(&conn, 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].pass_at_k_t_steps.len(), 2);
        assert_eq!(results[0].pass_at_k_t_seconds.len(), 1);
        assert!((results[0].pass_at_k_t_seconds[0].0 - 60.0).abs() < 1e-6);
    }

    /// SQLite V16: frontier_bucket_scores / frontier_inject_scores 列に JSON encode で保存
    /// → recent_experiments で decode、roundtrip 整合性を検証。
    #[test]
    fn t_experiment_db_roundtrip_with_frontier_v16() {
        let conn = setup_test_db();
        let mut exp = sample_experiment("e_frontier", 0.03);
        exp.frontier_bucket_scores = vec![(0, 0.85), (1, 0.72), (2, 0.55), (3, 0.30)];
        exp.frontier_inject_scores = vec![(0, 0.90), (4, 0.78), (8, 0.61), (16, 0.42)];
        ExperimentLog::save_to_db(&conn, &exp).unwrap();
        let results = ExperimentLog::recent_experiments(&conn, 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].frontier_bucket_scores.len(), 4);
        assert_eq!(results[0].frontier_inject_scores.len(), 4);
        assert_eq!(results[0].frontier_bucket_scores[0], (0, 0.85));
        assert_eq!(results[0].frontier_inject_scores[3], (16, 0.42));
    }

    /// V16 列に NULL (旧 row、frontier 未設定セッション) が入っているケース → 空 Vec で返却。
    #[test]
    fn t_frontier_v16_null_roundtrip_returns_empty_vec() {
        let conn = setup_test_db();
        let exp = sample_experiment("e_frontier_null", 0.01);
        // empty Vec で save → JSON `"[]"` が保存される (NULL ではない)
        ExperimentLog::save_to_db(&conn, &exp).unwrap();
        let results = ExperimentLog::recent_experiments(&conn, 1).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].frontier_bucket_scores.is_empty());
        assert!(results[0].frontier_inject_scores.is_empty());
    }

    /// serde: V16 field が無い旧 JSON でも `#[serde(default)]` で空 Vec として load できる。
    #[test]
    fn t_frontier_serde_backward_compat_old_json_loads_empty() {
        // V16 field 不在の旧 JSON (PASS@(k,T) 追加直後の形式を模倣)
        let old_json = r#"{
            "experiment_id": "old",
            "mutation_type": "PromptRule",
            "mutation_detail": "old",
            "baseline_score": 0.5,
            "experiment_score": 0.55,
            "delta": 0.05,
            "accepted": true,
            "duration_secs": 10.0,
            "config_snapshot": {},
            "pass_at_k": null,
            "pass_consecutive_k": null,
            "score_variance": null
        }"#;
        let exp: Experiment = serde_json::from_str(old_json).expect("legacy JSON load");
        assert!(exp.frontier_bucket_scores.is_empty());
        assert!(exp.frontier_inject_scores.is_empty());
    }
}
