use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// Dreamingシステム: 定期的な振り返り + パターン検出（exbrain方式）
///
/// SOUL = 不変のアイデンティティ（config.toml system_prompt）
/// MEMORY = 動的な経験蓄積（experiences + memories テーブル）
/// DREAMS = メタ認知（このモジュール）
/// パターン（行動の傾向を定量追跡）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub name: String,
    pub count: i64,
    pub trend: String, // "increasing" / "stable" / "decreasing"
    pub last_seen: String,
}

/// 振り返りレポート
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamReport {
    pub timestamp: String,
    pub tool_usage: Vec<(String, i64)>,       // ツール使用頻度
    pub failure_patterns: Vec<(String, i64)>, // 失敗パターン
    pub success_rate: f64,                    // 成功率
    pub insights: Vec<String>,                // 洞察
    pub emerging_patterns: Vec<Pattern>,      // 新興パターン
    pub phase: DreamPhase,                    // Light or Deep
    pub skill_promotions: Vec<String>,        // Deep時のスキル昇格推薦
}

/// Dreamingフェーズ
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DreamPhase {
    /// Light: 高速な候補収集・重複排除（毎ステップ実行可能）
    Light,
    /// Deep: パターン分析・スキル昇格推薦（定期実行）
    Deep,
}

/// Dreamingエンジン
pub struct Dreamer<'a> {
    conn: &'a Connection,
}

impl<'a> Dreamer<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// 振り返りレポートを生成（データ駆動、LLM不要）
    pub fn generate_report(&self, days: i64) -> Result<DreamReport> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let now = chrono::Utc::now().to_rfc3339();

        // ツール使用頻度
        let tool_usage = self.tool_usage_stats(&cutoff)?;

        // 失敗パターン
        let failure_patterns = self.failure_stats(&cutoff)?;

        // 成功率
        let success_rate = self.success_rate(&cutoff)?;

        // 洞察の生成（ルールベース）
        let insights = self.generate_insights(&tool_usage, &failure_patterns, success_rate);

        // パターン検出
        let emerging_patterns = self.detect_patterns(&cutoff)?;

        Ok(DreamReport {
            timestamp: now,
            tool_usage,
            failure_patterns,
            success_rate,
            insights,
            emerging_patterns,
            phase: DreamPhase::Deep,
            skill_promotions: Vec::new(),
        })
    }


    /// Light Dreaming: 高速な統計収集（毎回実行可能）
    pub fn dream_light(&self, days: i64) -> Result<DreamReport> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let now = chrono::Utc::now().to_rfc3339();
        let tool_usage = self.tool_usage_stats(&cutoff)?;
        let success_rate = self.success_rate(&cutoff)?;
        let insights = self.generate_insights(&tool_usage, &[], success_rate);

        Ok(DreamReport {
            timestamp: now,
            tool_usage,
            failure_patterns: Vec::new(),
            success_rate,
            insights,
            emerging_patterns: Vec::new(),
            phase: DreamPhase::Light,
            skill_promotions: Vec::new(),
        })
    }

    /// Deep Dreaming: パターン分析+スキル昇格推薦（定期実行）
    pub fn dream_deep(&self, days: i64) -> Result<DreamReport> {
        let mut report = self.generate_report(days)?;
        report.phase = DreamPhase::Deep;

        // スキル昇格候補を検出
        let skill_store = crate::memory::skill::SkillStore::new(self.conn);
        if let Ok(promoted) = skill_store.promote_from_experiences(self.conn, 3) {
            report.skill_promotions = promoted;
        }

        Ok(report)
    }

    /// ツール使用頻度を集計
    fn tool_usage_stats(&self, cutoff: &str) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT tool_name, COUNT(*) as cnt
             FROM experiences
             WHERE created_at > ?1 AND tool_name IS NOT NULL
             GROUP BY tool_name
             ORDER BY cnt DESC",
        )?;
        let rows = stmt.query_map(params![cutoff], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 失敗パターンを集計
    fn failure_stats(&self, cutoff: &str) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT error_detail, COUNT(*) as cnt
             FROM experiences
             WHERE type = 'failure' AND created_at > ?1 AND error_detail IS NOT NULL
             GROUP BY error_detail
             ORDER BY cnt DESC
             LIMIT 10",
        )?;
        let rows = stmt.query_map(params![cutoff], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 成功率を計算
    fn success_rate(&self, cutoff: &str) -> Result<f64> {
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM experiences WHERE created_at > ?1",
            params![cutoff],
            |row| row.get(0),
        )?;
        let success: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM experiences WHERE type = 'success' AND created_at > ?1",
            params![cutoff],
            |row| row.get(0),
        )?;
        if total == 0 {
            Ok(0.0)
        } else {
            Ok(success as f64 / total as f64)
        }
    }

    /// ルールベースの洞察生成
    fn generate_insights(
        &self,
        tool_usage: &[(String, i64)],
        failure_patterns: &[(String, i64)],
        success_rate: f64,
    ) -> Vec<String> {
        let mut insights = Vec::new();

        if success_rate < 0.5 {
            insights.push(format!(
                "成功率が{:.0}%と低い。プロンプトやツール選択の改善が必要",
                success_rate * 100.0
            ));
        } else if success_rate > 0.9 {
            insights.push(format!(
                "成功率{:.0}%は良好。現在のアプローチは効果的",
                success_rate * 100.0
            ));
        }

        if let Some((top_fail, count)) = failure_patterns.first()
            && *count >= 3
        {
            insights.push(format!(
                "'{top_fail}' エラーが{count}回発生。回避戦略の検討が必要"
            ));
        }

        if let Some((top_tool, count)) = tool_usage.first() {
            insights.push(format!("最も使用されたツール: {top_tool} ({count}回)"));
        }

        if tool_usage.len() <= 1 && !tool_usage.is_empty() {
            insights.push("ツールの多様性が低い。他のツールの活用を検討".to_string());
        }

        insights
    }

    /// パターン検出（行動の傾向を定量追跡）
    fn detect_patterns(&self, cutoff: &str) -> Result<Vec<Pattern>> {
        let mut patterns = Vec::new();
        let now = chrono::Utc::now().to_rfc3339();

        // 連続成功パターン
        let consecutive_success: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM (
                SELECT type FROM experiences WHERE created_at > ?1 ORDER BY id DESC LIMIT 10
            ) WHERE type = 'success'",
            params![cutoff],
            |row| row.get(0),
        )?;
        if consecutive_success >= 8 {
            patterns.push(Pattern {
                name: "高成功率の持続".to_string(),
                count: consecutive_success,
                trend: "stable".to_string(),
                last_seen: now.clone(),
            });
        }

        // 繰り返し失敗パターン
        let repeated_failures: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT error_detail) FROM experiences
             WHERE type = 'failure' AND created_at > ?1
             GROUP BY error_detail HAVING COUNT(*) >= 3
             LIMIT 1",
                params![cutoff],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if repeated_failures > 0 {
            patterns.push(Pattern {
                name: "繰り返し失敗".to_string(),
                count: repeated_failures,
                trend: "increasing".to_string(),
                last_seen: now.clone(),
            });
        }

        // スキル蓄積パターン
        let skill_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM skills", [], |row| row.get(0))?;
        if skill_count > 0 {
            patterns.push(Pattern {
                name: "スキル蓄積".to_string(),
                count: skill_count,
                trend: "increasing".to_string(),
                last_seen: now,
            });
        }

        Ok(patterns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::experience::{ExperienceStore, ExperienceType, RecordParams};
    use crate::memory::store::MemoryStore;

    fn setup() -> MemoryStore {
        let store = MemoryStore::in_memory().unwrap();
        let exp = ExperienceStore::new(store.conn());
        for i in 0..5 {
            exp.record(&RecordParams {
                exp_type: ExperienceType::Success,
                task_context: &format!("task {i}"),
                action: "shell: ls",
                outcome: "OK",
                lesson: None,
                tool_name: Some("shell"),
                error_type: None,
                error_detail: None,
            })
            .unwrap();
        }
        for _ in 0..2 {
            exp.record(&RecordParams {
                exp_type: ExperienceType::Failure,
                task_context: "fail task",
                action: "shell: bad",
                outcome: "error",
                lesson: None,
                tool_name: Some("shell"),
                error_type: Some("ToolExecError"),
                error_detail: Some("Timeout"),
            })
            .unwrap();
        }
        store
    }

    #[test]
    fn test_generate_report() {
        let store = setup();
        let dreamer = Dreamer::new(store.conn());
        let report = dreamer.generate_report(7).unwrap();
        assert!(!report.tool_usage.is_empty());
        assert!(report.success_rate > 0.0);
    }

    #[test]
    fn test_tool_usage_stats() {
        let store = setup();
        let dreamer = Dreamer::new(store.conn());
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
        let usage = dreamer.tool_usage_stats(&cutoff).unwrap();
        assert_eq!(usage[0].0, "shell");
        assert_eq!(usage[0].1, 7); // 5 success + 2 failure
    }

    #[test]
    fn test_success_rate() {
        let store = setup();
        let dreamer = Dreamer::new(store.conn());
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
        let rate = dreamer.success_rate(&cutoff).unwrap();
        // 5 success / 7 total ≈ 0.714
        assert!(rate > 0.7 && rate < 0.75);
    }

    #[test]
    fn test_insights_low_success() {
        let store = MemoryStore::in_memory().unwrap();
        let dreamer = Dreamer::new(store.conn());
        let insights = dreamer.generate_insights(&[], &[], 0.3);
        assert!(insights.iter().any(|i| i.contains("低い")));
    }

    #[test]
    fn test_insights_high_success() {
        let store = MemoryStore::in_memory().unwrap();
        let dreamer = Dreamer::new(store.conn());
        let insights = dreamer.generate_insights(&[], &[], 0.95);
        assert!(insights.iter().any(|i| i.contains("良好")));
    }

    #[test]
    fn test_empty_report() {
        let store = MemoryStore::in_memory().unwrap();
        let dreamer = Dreamer::new(store.conn());
        let report = dreamer.generate_report(7).unwrap();
        assert_eq!(report.success_rate, 0.0);
        assert!(report.tool_usage.is_empty());
    }

    #[test]
    fn test_dream_light() {
        let store = setup();
        let dreamer = Dreamer::new(store.conn());
        let report = dreamer.dream_light(7).unwrap();
        assert_eq!(report.phase, DreamPhase::Light);
        assert!(report.failure_patterns.is_empty(), "Lightはfailure_patternsを収集しない");
        assert!(!report.tool_usage.is_empty());
    }

    #[test]
    fn test_dream_deep() {
        let store = setup();
        let dreamer = Dreamer::new(store.conn());
        let report = dreamer.dream_deep(7).unwrap();
        assert_eq!(report.phase, DreamPhase::Deep);
        assert!(!report.tool_usage.is_empty());
    }

    #[test]
    fn test_dream_phase_difference() {
        let store = setup();
        let dreamer = Dreamer::new(store.conn());
        let light = dreamer.dream_light(7).unwrap();
        let deep = dreamer.dream_deep(7).unwrap();
        assert_eq!(light.phase, DreamPhase::Light);
        assert_eq!(deep.phase, DreamPhase::Deep);
        // Deepはfailure_patternsも分析
        // Lightは空
        assert!(light.failure_patterns.is_empty());
    }
}