//! LLM-as-judge 評価モジュール（ADK 知見 P0 取込、Phase B1: Green）
//!
//! Google ADK の `rubric_based_final_response_quality_v1` メトリクスに対応する
//! ルーブリックベース最終応答品質評価。`AdvisorConfig`（OpenAI 互換 / claude-code）を
//! 流用して judge LLM を呼び出し、completeness / correctness / reasoning_quality の
//! 3 軸を 0.0–1.0 で評価する。
//!
//! Phase B1 では `try_remote_with_prompt` / `try_claude_code_with_prompt` を用いて
//! rubric prompt を送信し、JSON 応答を `parse_judge_response` でパースする。
//! どちらのバックエンドも未設定の場合は `Err` を返す（呼出側で skip 判断）。

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::observability::logger::{LogLevel, log_event};
use crate::runtime::model_router::AdvisorConfig;

/// ルーブリック評価スコア
///
/// 各軸 0.0–1.0、`composite()` で重み付け合成。
/// `raw_judge_response` は監査用に judge の生応答を保持する。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RubricScore {
    /// 要求の網羅性（タスクのすべての要件に応えているか）
    pub completeness: f64,
    /// 事実誤りの有無（実際のツール結果や既知事実と一致しているか）
    pub correctness: f64,
    /// 推論の妥当性（思考の論理構造が破綻していないか）
    pub reasoning_quality: f64,
    /// judge LLM の生応答（監査・デバッグ用）
    pub raw_judge_response: String,
}

impl RubricScore {
    /// 重み付け合成スコア（0.0–1.0）
    ///
    /// completeness 40% + correctness 40% + reasoning_quality 20%。
    /// 完全性と正確性を等しく重視し、推論品質は補助指標として扱う。
    pub fn composite(&self) -> f64 {
        0.4 * self.completeness + 0.4 * self.correctness + 0.2 * self.reasoning_quality
    }

    /// 全軸ゼロのスコア（malformed JSON や judge 失敗時のフォールバック）
    pub fn zero_with_raw(raw: impl Into<String>) -> Self {
        Self {
            completeness: 0.0,
            correctness: 0.0,
            reasoning_quality: 0.0,
            raw_judge_response: raw.into(),
        }
    }
}

/// LLM judge トレイト
///
/// ベンチマークタスクの応答を rubric で評価し、`RubricScore` を返す。
/// 実装は HTTP API・claude-code・mock など差し替え可能。
pub trait LlmJudge {
    fn evaluate(
        &mut self,
        task_description: &str,
        response: &str,
        trajectory: &[String],
    ) -> Result<RubricScore>;
}

/// `AdvisorConfig` を流用した judge 実装
///
/// `try_remote_with_prompt` → `try_claude_code_with_prompt` の順でフォールバックを試み、
/// 成功した raw 応答を `parse_judge_response` でパースする。両方とも利用不可なら `Err`。
///
/// **設計判断: 借用維持**（`&'a mut AdvisorConfig`）
/// AdvisorConfig は per-session 可変状態（max_uses カウンター + キャッシュ + AdvisorStats）。
/// judge が値所有すると stats が判定経路と本流で分裂するため、`BenchmarkSuite::run_k`
/// 経由の per-call 注入（`Option<&mut dyn LlmJudge>`）が想定される。
/// `Box<dyn LlmJudge>` 化は将来 BenchmarkSuite が judge を「保有」したくなった時点で再検討。
pub struct HttpAdvisorJudge<'a> {
    /// per-session 可変状態（max_uses / cache / stats）を保持する advisor への借用。
    /// `&'a mut` 維持判断は型ドキュメント参照（stats 分裂回避）。
    pub advisor: &'a mut AdvisorConfig,
    /// system 側プロンプト（採点指示）。default は `default_rubric_template()`。
    /// JSON 出力形式を必ず明示すること（`parse_judge_response` の期待形式に準拠）。
    pub rubric_template: String,
}

impl<'a> HttpAdvisorJudge<'a> {
    pub fn new(advisor: &'a mut AdvisorConfig) -> Self {
        Self {
            advisor,
            rubric_template: default_rubric_template(),
        }
    }

    pub fn with_template(advisor: &'a mut AdvisorConfig, rubric_template: String) -> Self {
        Self {
            advisor,
            rubric_template,
        }
    }
}

impl<'a> LlmJudge for HttpAdvisorJudge<'a> {
    fn evaluate(
        &mut self,
        task_description: &str,
        response: &str,
        trajectory: &[String],
    ) -> Result<RubricScore> {
        let system = self.rubric_template.clone();
        let user = build_judge_user_prompt(task_description, response, trajectory);
        // 全バックエンド失敗時の診断用に各エラーを保持（rust-reviewer 監査 LOW#2 対応）。
        let mut errors: Vec<String> = Vec::new();

        // 1. リモート HTTP API を試行（api_endpoint 設定時のみ）
        match self.advisor.try_remote_with_prompt(&system, &user) {
            Ok(Some(raw)) => return Ok(parse_judge_response(&raw)),
            Ok(None) => {} // 未設定 → 次のバックエンドへ
            Err(e) => {
                let msg = format!("remote: {e}");
                log_event(
                    LogLevel::Warn,
                    "judge",
                    &format!("{msg}, falling back to claude-code"),
                );
                errors.push(msg);
            }
        }

        // 2. Claude Code CLI を試行（backend=ClaudeCode 時のみ）
        match self.advisor.try_claude_code_with_prompt(&system, &user) {
            Ok(Some(raw)) => return Ok(parse_judge_response(&raw)),
            Ok(None) => {} // 未設定
            Err(e) => {
                let msg = format!("claude-code: {e}");
                log_event(LogLevel::Warn, "judge", &msg);
                errors.push(msg);
            }
        }

        // 3. どのバックエンドも利用不可。エラーがあれば診断情報を含める。
        if errors.is_empty() {
            anyhow::bail!(
                "judge: no advisor backend available (api_endpoint or backend=claude-code が必要)"
            )
        } else {
            anyhow::bail!("judge: all backends failed: {}", errors.join("; "))
        }
    }
}

/// judge LLM への user メッセージを構築する純粋関数
///
/// system 側は `rubric_template`（採点指示）、user 側はタスク・応答・軌跡を構造化。
/// テスト容易性のため evaluate() から分離。
pub fn build_judge_user_prompt(task: &str, response: &str, trajectory: &[String]) -> String {
    let trajectory_str = if trajectory.is_empty() {
        "(なし)".to_string()
    } else {
        trajectory.join(" → ")
    };
    format!(
        "タスク: {task}\n\n応答:\n{response}\n\n軌跡: {trajectory_str}\n\n\
         上記を採点し、JSON のみを出力してください。"
    )
}

/// judge LLM の JSON 応答をパースする
///
/// 期待形式: `{"completeness": x, "correctness": y, "reasoning_quality": z}` (各 0.0–1.0)
///
/// 不正 JSON の場合はエラーを返さず、全軸 0.0 + 生応答を保持した `RubricScore` を返す
/// （graceful degradation: judge が壊れてもベンチマークパイプラインは継続）。
/// 値は 0.0–1.0 にクランプする（仕様外の値からの保護）。
pub fn parse_judge_response(raw: &str) -> RubricScore {
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return RubricScore::zero_with_raw(raw),
    };

    let extract = |key: &str| -> f64 {
        let raw_value = value.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);
        // NaN / 非有限値の防御: clamp は NaN を通してしまうため明示ガード。
        // 下流の benchmark 平均計算が NaN 汚染されないようにする。
        if raw_value.is_finite() {
            raw_value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };

    RubricScore {
        completeness: extract("completeness"),
        correctness: extract("correctness"),
        reasoning_quality: extract("reasoning_quality"),
        raw_judge_response: raw.to_string(),
    }
}

/// デフォルトの rubric prompt テンプレート
///
/// judge LLM への指示を含む安定した文字列。タスク詳細・応答・軌跡は
/// 呼び出し側で format して付加する。Phase B1 で実 prompt として使用される。
pub fn default_rubric_template() -> String {
    "あなたはエージェントの応答品質を評価する judge です。\n\
     以下の3軸を0.0〜1.0で評価し、JSON のみを出力してください:\n\
     - completeness: タスクの要求事項を網羅しているか\n\
     - correctness: 事実誤りや矛盾がないか\n\
     - reasoning_quality: 推論の論理構造が妥当か\n\
     出力形式: {\"completeness\": 0.x, \"correctness\": 0.x, \"reasoning_quality\": 0.x}"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rubric_score_composite_weights() {
        let perfect = RubricScore {
            completeness: 1.0,
            correctness: 1.0,
            reasoning_quality: 1.0,
            raw_judge_response: String::new(),
        };
        assert!((perfect.composite() - 1.0).abs() < f64::EPSILON);

        let half = RubricScore {
            completeness: 0.5,
            correctness: 0.5,
            reasoning_quality: 0.5,
            raw_judge_response: String::new(),
        };
        assert!((half.composite() - 0.5).abs() < f64::EPSILON);

        // completeness のみ満点 → 0.4
        let only_completeness = RubricScore {
            completeness: 1.0,
            correctness: 0.0,
            reasoning_quality: 0.0,
            raw_judge_response: String::new(),
        };
        assert!((only_completeness.composite() - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn test_judge_response_parses_json() {
        let raw = r#"{"completeness": 0.8, "correctness": 0.9, "reasoning_quality": 0.7}"#;
        let score = parse_judge_response(raw);
        assert!((score.completeness - 0.8).abs() < f64::EPSILON);
        assert!((score.correctness - 0.9).abs() < f64::EPSILON);
        assert!((score.reasoning_quality - 0.7).abs() < f64::EPSILON);
        assert_eq!(score.raw_judge_response, raw);
    }

    #[test]
    fn test_judge_response_handles_malformed_json() {
        let raw = "this is not json at all";
        let score = parse_judge_response(raw);
        assert_eq!(score.completeness, 0.0);
        assert_eq!(score.correctness, 0.0);
        assert_eq!(score.reasoning_quality, 0.0);
        assert_eq!(score.raw_judge_response, raw);
    }

    #[test]
    fn test_judge_response_clamps_out_of_range() {
        // 仕様外の値（負・1.0超）は 0.0–1.0 にクランプされる
        let raw = r#"{"completeness": 1.5, "correctness": -0.3, "reasoning_quality": 0.5}"#;
        let score = parse_judge_response(raw);
        assert!((score.completeness - 1.0).abs() < f64::EPSILON);
        assert_eq!(score.correctness, 0.0);
        assert!((score.reasoning_quality - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_judge_response_handles_non_finite_and_null() {
        // NaN / Infinity / null / 文字列値はすべて 0.0 にサニタイズされる
        // （clamp は NaN を通すため明示ガードが必要、benchmark 平均の NaN 汚染防止）
        let raw = r#"{"completeness": null, "correctness": "high", "reasoning_quality": 0.5}"#;
        let score = parse_judge_response(raw);
        assert_eq!(score.completeness, 0.0); // null
        assert_eq!(score.correctness, 0.0); // 文字列 → as_f64() が None
        assert!((score.reasoning_quality - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_judge_returns_err_when_no_backend_available() {
        // Phase B1 Green: AdvisorConfig::default() は backend=Local かつ api_endpoint=None
        // → try_remote_with_prompt も try_claude_code_with_prompt も None を返す
        // → evaluate() は明示的なエラーを返す（呼出側で skip 判断）
        let mut advisor = AdvisorConfig::default();
        let mut judge = HttpAdvisorJudge::new(&mut advisor);
        let result = judge.evaluate("test task", "test response", &[]);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no advisor backend available"),
            "expected backend-unavailable error, got: {err}"
        );
    }

    #[test]
    fn test_build_judge_user_prompt_empty_trajectory() {
        let prompt = build_judge_user_prompt("ファイル一覧を取得", "ls 実行結果", &[]);
        assert!(prompt.contains("タスク: ファイル一覧を取得"));
        assert!(prompt.contains("応答:\nls 実行結果"));
        assert!(prompt.contains("軌跡: (なし)"));
        assert!(prompt.contains("JSON のみを出力"));
    }

    #[test]
    fn test_build_judge_user_prompt_with_trajectory() {
        let trajectory = vec![
            "shell".to_string(),
            "file_read".to_string(),
            "shell".to_string(),
        ];
        let prompt = build_judge_user_prompt("バグ調査", "原因はXでした", &trajectory);
        // 軌跡は " → " 区切りで連結される
        assert!(prompt.contains("軌跡: shell → file_read → shell"));
        assert!(prompt.contains("タスク: バグ調査"));
        assert!(prompt.contains("応答:\n原因はXでした"));
    }

    #[test]
    fn test_default_rubric_template_nonempty() {
        let template = default_rubric_template();
        assert!(!template.is_empty());
        // 安定文字列：3軸の名前を含むこと
        assert!(template.contains("completeness"));
        assert!(template.contains("correctness"));
        assert!(template.contains("reasoning_quality"));
    }
}
