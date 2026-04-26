//! LLM-as-judge 評価モジュール（ADK 知見 P0 取込、Phase A1: TDD Red）
//!
//! Google ADK の `rubric_based_final_response_quality_v1` メトリクスに対応する
//! ルーブリックベース最終応答品質評価。`HttpAdvisor`（OpenAI 互換 / claude-code）を
//! 流用して judge LLM を呼び出し、completeness / correctness / reasoning_quality の
//! 3 軸を 0.0–1.0 で評価する。
//!
//! Phase A1 は型定義 + 純粋関数のみ実装し、`HttpAdvisorJudge::evaluate()` は
//! Phase B1 で wire される（現時点では `not yet implemented` を返す）。

use anyhow::Result;
use serde::{Deserialize, Serialize};

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

/// `HttpAdvisor` を流用した judge 実装
///
/// Phase A1（現時点）: `evaluate()` は stub（Err 返却）。
/// Phase B1 で `try_remote_advice` / `try_claude_code_advice` を呼び出して
/// rubric prompt を送信し、JSON 応答を `parse_judge_response` でパースする。
///
/// **Phase B1 設計メモ**: 現在は `&'a mut AdvisorConfig` 借用。`Box<dyn LlmJudge + 'static>`
/// で `BenchmarkSuite` に保持したい場合は、`AdvisorConfig` を値で保有する別実装
/// （例: `OwnedHttpAdvisorJudge`）を追加するか、本構造体を `'static` 化する設計判断が必要。
/// 借用は per-evaluation コール用、所有は per-suite コール用という両立も可。
pub struct HttpAdvisorJudge<'a> {
    pub advisor: &'a mut AdvisorConfig,
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
        _task_description: &str,
        _response: &str,
        _trajectory: &[String],
    ) -> Result<RubricScore> {
        // Phase A1 Red: 実装は Phase B1 で wire される
        Err(anyhow::anyhow!(
            "HttpAdvisorJudge::evaluate is not yet implemented (Phase B1)"
        ))
    }
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
    fn test_judge_failure_returns_error() {
        // Phase A1 Red: HttpAdvisorJudge::evaluate は未実装 → Err
        let mut advisor = AdvisorConfig::default();
        let mut judge = HttpAdvisorJudge::new(&mut advisor);
        let result = judge.evaluate("test task", "test response", &[]);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not yet implemented"));
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
