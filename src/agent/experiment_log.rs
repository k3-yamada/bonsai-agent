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
        match cycle % 10 {
            0..=2 => Self::Precision,
            3..=4 => Self::Efficiency,
            5..=6 => Self::Exploration,
            _ => Self::Robustness, // 7, 8, 9
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
        }
    }
}

/// 実験ログの永続化（SQLite + TSV）
pub struct ExperimentLog;

impl ExperimentLog {
    /// SQLiteに実験を記録
    pub fn save_to_db(conn: &Connection, exp: &Experiment) -> Result<()> {
        conn.execute(
            "INSERT INTO experiments (experiment_id, mutation_type, mutation_detail, \
             baseline_score, experiment_score, delta, accepted, duration_secs, prescreened) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
                "experiment_id\tmutation_type\tmutation_detail\tbaseline_score\texperiment_score\tdelta\taccepted\tduration_secs\tpass_at_k\tpass_consecutive_k\tscore_variance\tprescreened"
            )?;
        }

        writeln!(
            file,
            "{}\t{}\t{}\t{:.4}\t{:.4}\t{:.4}\t{}\t{:.2}\t{}\t{}\t{}\t{}",
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
        )?;
        Ok(())
    }

    /// 直近N件の実験をDBから取得（新しい順）
    pub fn recent_experiments(conn: &Connection, limit: usize) -> Result<Vec<Experiment>> {
        let mut stmt = conn.prepare(
            "SELECT experiment_id, mutation_type, mutation_detail, \
             baseline_score, experiment_score, delta, accepted, duration_secs, \
             COALESCE(prescreened, 0) \
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
            ))
        })?;

        // config_snapshot用のステートメントをループ外で準備
        let mut config_stmt = conn.prepare(
            "SELECT config_key, config_value FROM experiment_config WHERE experiment_id = ?1",
        )?;

        let mut experiments = Vec::new();
        for row in rows {
            let (id, mt, detail, baseline, score, delta, accepted, dur, prescreened) = row?;
            let mutation_type = MutationType::parse(&mt).unwrap_or(MutationType::PromptRule);

            let config: HashMap<String, String> = config_stmt
                .query_map(params![id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .collect();

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
        // V7まで適用（prescreenedカラム含む）
        for version in [1, 2, 7] {
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
        assert_eq!(data_line.split('\t').count(), 12);
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
}
