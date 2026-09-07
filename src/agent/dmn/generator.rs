//! DMN内省ジェネレータ (経験・記憶・未解決パターンからの内省生成)
//!
//! 過去の失敗経験や蓄積された記憶から自発的に改善テーマを抽出し、
//! 内省テキストと重要度スコア (significance) を生成する。

use crate::cancel::CancellationToken;
use crate::domain::conversation::Message;
use crate::domain::llm::LlmBackend;
use crate::memory::experience::{ExperienceStore, ExperienceType};
use crate::memory::store::MemoryStore;

/// DMN内省ジェネレータ
pub struct DmnGenerator;

impl DmnGenerator {
    /// MemoryStore から過去の失敗・教訓を抽出し、LLM（指定時）またはルールにより内省（洞察テキストと重要度スコア）を生成する。
    pub fn generate_reflection_with_llm(
        store: Option<&MemoryStore>,
        backend: Option<&dyn LlmBackend>,
    ) -> (String, f64) {
        let Some(s) = store else {
            return (
                "システム待機中。記憶ストアが未初期化です。".to_string(),
                0.1,
            );
        };

        let exp_store = ExperienceStore::new(s.conn());

        // 1. 直近の経験（失敗含む）を検索
        let recent_exps = exp_store.find_similar("", 5).unwrap_or_default();

        let recent_failure = recent_exps
            .iter()
            .find(|e| e.exp_type == ExperienceType::Failure);

        if let Some(fail) = recent_failure {
            let lesson = fail.lesson.as_deref().unwrap_or(&fail.outcome);

            // LLM バックエンドが指定されている場合、自発的思考プロンプトで生成
            if let Some(b) = backend {
                let prompt = format!(
                    "あなたは自律AIエージェントの自己内省サブシステムです。過去の失敗『タスク: {}、結果: {}、教訓: {}』を踏まえ、再発防止の具体的対策と自己洞察を日本語1文（100文字以内）で出力してください。",
                    fail.task_context, fail.outcome, lesson
                );
                let messages = vec![Message::user(prompt)];
                let cancel = CancellationToken::new();
                if let Ok(res) = b.generate(&messages, &[], &mut |_| {}, &cancel) {
                    let text = res.text.trim();
                    if !text.is_empty() {
                        return (text.to_string(), 0.85);
                    }
                }
            }

            let text = format!(
                "直近の失敗パターンを内省: タスク「{}」での教訓「{}」を再評価。同一パターンの再発防止策を自己更新。",
                fail.task_context, lesson
            );
            // 失敗からの反省は高重要度（発話閾値 0.70 を超えやすい 0.85）
            (text, 0.85)
        } else if let Some(sensor_exp) = recent_exps
            .iter()
            .find(|e| e.task_context.starts_with("external_sensor:"))
        {
            // 2. 外部知覚センサー経験（ファイル更新・アプリ切り替え・離席復帰）からの内省
            let context_summary = &sensor_exp.outcome;
            if let Some(b) = backend {
                let prompt = format!(
                    "あなたは自律AIエージェントの自己内省サブシステムです。直近の知覚イベント『{}』を踏まえ、ユーザーの作業支援や状況変化に対する自発的考察を日本語1文（100文字以内）で出力してください。",
                    context_summary
                );
                let messages = vec![Message::user(prompt)];
                let cancel = CancellationToken::new();
                if let Ok(res) = b.generate(&messages, &[], &mut |_| {}, &cancel) {
                    let text = res.text.trim();
                    if !text.is_empty() {
                        return (text.to_string(), 0.75);
                    }
                }
            }

            let text = format!(
                "直近の環境変化を内省: {context_summary}。ユーザーの作業文脈に応じた支援準備を整えました。"
            );
            (text, 0.75)
        } else {
            // 3. 失敗も知覚変化もない場合は定常的な記憶整理・確認
            let text =
                "対話履歴と知識グラフの整合性を確認。異常や未解決の矛盾は検出されず。".to_string();
            // 定常整理は低重要度（沈黙内省として記録される 0.40）
            (text, 0.40)
        }
    }

    /// 既存呼び出し互換のラッパー（LLMなし版）
    pub fn generate_reflection(store: Option<&MemoryStore>) -> (String, f64) {
        Self::generate_reflection_with_llm(store, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::experience::RecordParams;

    #[test]
    fn test_dmn_generator_no_store() {
        let (text, score) = DmnGenerator::generate_reflection(None);
        assert_eq!(score, 0.1);
        assert!(text.contains("未初期化"));
    }

    #[test]
    fn test_dmn_generator_empty_store_returns_routine_reflection() {
        let store = MemoryStore::in_memory().unwrap();
        let (text, score) = DmnGenerator::generate_reflection(Some(&store));
        assert_eq!(score, 0.40);
        assert!(text.contains("整合性を確認"));
    }

    #[test]
    fn test_dmn_generator_with_sensor_experience() {
        let store = MemoryStore::in_memory().unwrap();
        let exp_store = ExperienceStore::new(store.conn());

        exp_store
            .record(&RecordParams {
                exp_type: ExperienceType::Insight,
                task_context: "external_sensor:file_watch",
                action: "file_modified",
                outcome: "ファイル更新検知: main.rs (workspace)",
                lesson: Some("file_changed"),
                tool_name: None,
                error_type: None,
                error_detail: None,
            })
            .unwrap();

        let (text, score) = DmnGenerator::generate_reflection(Some(&store));
        assert_eq!(score, 0.75);
        assert!(text.contains("直近の環境変化を内省"));
        assert!(text.contains("main.rs"));
    }

    #[test]
    fn test_dmn_generator_with_failure_returns_high_significance() {
        let store = MemoryStore::in_memory().unwrap();
        let exp_store = ExperienceStore::new(store.conn());

        exp_store
            .record(&RecordParams {
                exp_type: ExperienceType::Failure,
                task_context: "ファイル書き込み",
                action: "file_write",
                outcome: "Permission denied",
                lesson: Some("書き込み前に権限を確認すべき"),
                tool_name: Some("file_write"),
                error_type: Some("Permission"),
                error_detail: None,
            })
            .unwrap();

        let (text, score) = DmnGenerator::generate_reflection(Some(&store));
        assert!(score >= 0.80);
        assert!(text.contains("ファイル書き込み"));
        assert!(text.contains("書き込み前に権限を確認すべき"));
    }

    #[test]
    fn test_dmn_generator_with_llm_generates_autonomous_reflection() {
        use crate::domain::llm::MockLlmBackend;

        let store = MemoryStore::in_memory().unwrap();
        let exp_store = ExperienceStore::new(store.conn());

        exp_store
            .record(&RecordParams {
                exp_type: ExperienceType::Failure,
                task_context: "API接続",
                action: "web_fetch",
                outcome: "Connection refused",
                lesson: Some("ポート番号を確認すべき"),
                tool_name: Some("web_fetch"),
                error_type: Some("Network"),
                error_detail: None,
            })
            .unwrap();

        let mock_backend =
            MockLlmBackend::single("ポート確認とヘルスチェックを事前実行するよう方針を改定。");
        let (text, score) =
            DmnGenerator::generate_reflection_with_llm(Some(&store), Some(&mock_backend));
        assert_eq!(score, 0.85);
        assert_eq!(
            text,
            "ポート確認とヘルスチェックを事前実行するよう方針を改定。"
        );
    }
}
