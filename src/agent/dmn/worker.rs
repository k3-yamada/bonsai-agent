//! DMN自発思考ワーカ (三段階ゲート評価と記憶還元)

use std::path::PathBuf;
use std::time::Instant;

use crate::agent::dmn::outcome::DmnOutcome;
use crate::agent::dmn::scheduler::DmnScheduler;
use crate::knowledge::extractor::{StockCategory, StockEntry};
use crate::knowledge::vault::Vault;
use crate::memory::experience::{ExperienceStore, ExperienceType, RecordParams};
use crate::memory::store::MemoryStore;

pub struct DmnWorker {
    pub scheduler: DmnScheduler,
    pub speak_threshold: f64,
    pub last_tick: Instant,
    pub current_delay: std::time::Duration,
    pub vault_path: Option<PathBuf>,
    pub idle_since: Option<Instant>,
    pub dream_threshold_secs: Option<f64>,
    pub last_dreamed: Option<Instant>,
}

impl DmnWorker {
    pub fn new(mean_sec: f64, min_gap_sec: f64, speak_threshold: f64) -> Self {
        let mut scheduler = DmnScheduler::new(mean_sec, min_gap_sec);
        let current_delay = scheduler.next_delay();
        Self {
            scheduler,
            speak_threshold,
            last_tick: Instant::now(),
            current_delay,
            vault_path: None,
            idle_since: None,
            dream_threshold_secs: None,
            last_dreamed: None,
        }
    }

    pub fn with_vault(mut self, path: PathBuf) -> Self {
        self.vault_path = Some(path);
        self
    }

    pub fn with_dream_threshold(mut self, secs: f64) -> Self {
        self.dream_threshold_secs = Some(secs);
        self
    }

    /// 三段階ゲートを評価し、自発思考（DMNステップ）を1回実行する。
    pub fn tick(
        &mut self,
        is_busy: bool,
        store: Option<&MemoryStore>,
        generator: impl FnOnce() -> (String, f64),
    ) -> DmnOutcome {
        // Gate 0: 会話が進行中／タスクが処理中か
        if is_busy {
            self.idle_since = None;
            return DmnOutcome::SkippedBusy;
        }

        if self.idle_since.is_none() {
            self.idle_since = Some(Instant::now());
        }

        // 長時間アイドルのメタ認知（Deep Dreaming）自律実行
        if let (Some(threshold_secs), Some(idle_start)) =
            (self.dream_threshold_secs, self.idle_since)
        {
            let idle_dur = idle_start.elapsed().as_secs_f64();
            let should_dream = match self.last_dreamed {
                Some(last) => last.elapsed().as_secs_f64() >= threshold_secs,
                None => idle_dur >= threshold_secs,
            };

            if should_dream {
                self.last_dreamed = Some(Instant::now());
                if let Some(s) = store {
                    self.trigger_deep_dream(s);
                }
            }
        }

        // Gate 1: スケジューラが返した待機間隔を消化したか
        let elapsed = self.last_tick.elapsed();
        if elapsed < self.current_delay {
            return DmnOutcome::SkippedNotDue;
        }

        // タイマー更新
        self.last_tick = Instant::now();
        self.current_delay = self.scheduler.next_delay();

        // 内省の生成（洞察テキストと重要度スコア 0.0..1.0）
        let (insight, significance) = generator();

        // 1. 既存の四層記憶（experiences テーブルの Insight）へ還元
        if let Some(s) = store {
            let exp_store = ExperienceStore::new(s.conn());
            let _ = exp_store.record(&RecordParams {
                exp_type: ExperienceType::Insight,
                task_context: "DMN自発思考ループ",
                action: "internal_reflection",
                outcome: &insight,
                lesson: Some(&format!("significance={significance:.2}")),
                tool_name: None,
                error_type: None,
                error_detail: None,
            });

            // 重要度が発話閾値以上の場合、A-MEM（統合記憶 `memories` ＆ `memory_links`）にも還元
            if significance >= self.speak_threshold {
                let tags = vec![
                    "dmn".to_string(),
                    "reflection".to_string(),
                    "insight".to_string(),
                ];
                if let Ok(new_id) = s.save_memory(&insight, "insight", &tags) {
                    // 直近の既存メモリと連想リンク（reflects_on）を自動形成
                    if let Ok(all) = s.all_memories()
                        && let Some(prev) = all.iter().rev().find(|m| m.id != new_id)
                    {
                        let _ = s.link_memories(new_id, prev.id, "reflects_on");
                    }
                }
            }
        }

        // 2. ナレッジVault（insights.md）および KnowledgeGraph への同期（発話相当の重要度の場合）
        if significance >= self.speak_threshold
            && let Some(vault_dir) = &self.vault_path
            && let Ok(vault) = Vault::new(vault_dir)
        {
            let entry = StockEntry {
                category: StockCategory::Insight,
                content: insight.clone(),
                source: "DMN自発思考ループ".to_string(),
            };
            let _ = vault.append(&entry);

            // store がある場合は KnowledgeGraph にもノード・エッジ登録
            if let Some(s) = store {
                let kg = crate::memory::graph::KnowledgeGraph::new(s.conn());
                let _ = vault.record_to_graph(&entry, &kg);
            }
        }

        // Gate 2: 沈黙／発話判定
        if significance < self.speak_threshold {
            DmnOutcome::SilentReflection { insight }
        } else {
            let message = format!("【自発的内省】{insight}");
            DmnOutcome::SpokenReflection { insight, message }
        }
    }

    /// 長時間アイドル時にメタ認知エンジン（Dreamer）を自律起動し、
    /// 過去7日間のツール・失敗パターン分析結果を多層記憶とナレッジVaultに固定化（Consolidation）する。
    fn trigger_deep_dream(&self, store: &MemoryStore) {
        let dreamer = crate::memory::dreams::Dreamer::new(store.conn());
        if let Ok(report) = dreamer.generate_report(7) {
            let exp_store = ExperienceStore::new(store.conn());
            for insight in &report.insights {
                let _ = exp_store.record(&RecordParams {
                    exp_type: ExperienceType::Insight,
                    task_context: "DMN自律Dreaming",
                    action: "deep_dream_consolidation",
                    outcome: insight,
                    lesson: Some("長時間アイドル時の自発的メタ認知・記憶統合"),
                    tool_name: None,
                    error_type: None,
                    error_detail: None,
                });
            }

            // ナレッジVaultへの保存
            if let Some(vault_dir) = &self.vault_path
                && let Ok(vault) = Vault::new(vault_dir)
            {
                for insight in &report.insights {
                    let entry = StockEntry {
                        category: StockCategory::Insight,
                        content: format!("【メタ認知Dreaming】{insight}"),
                        source: "DMN自律記憶統合".to_string(),
                    };
                    let _ = vault.append(&entry);
                }
            }
        }
    }
}

/// DMN自発思考の常駐バックグラウンドランナー (排他制御・キャンセル安全)
pub struct DmnRunner {
    pub is_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    cancel: crate::cancel::CancellationToken,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl DmnRunner {
    pub fn spawn<F, G>(
        mut worker: DmnWorker,
        is_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
        cancel: crate::cancel::CancellationToken,
        db_path: Option<String>,
        generator: G,
        mut on_spoken: F,
    ) -> Self
    where
        G: Fn(Option<&MemoryStore>) -> (String, f64) + Send + 'static,
        F: FnMut(String) + Send + 'static,
    {
        let busy_clone = is_busy.clone();
        let cancel_clone = cancel.clone();

        let handle = std::thread::spawn(move || {
            let store = db_path.as_deref().and_then(|p| MemoryStore::open(p).ok());
            while !cancel_clone.is_cancelled() {
                let busy = busy_clone.load(std::sync::atomic::Ordering::Relaxed);
                let outcome = worker.tick(busy, store.as_ref(), || generator(store.as_ref()));
                if let DmnOutcome::SpokenReflection { message, .. } = outcome {
                    on_spoken(message);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });

        Self {
            is_busy,
            cancel,
            handle: Some(handle),
        }
    }

    pub fn stop(&mut self) {
        self.cancel.cancel();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for DmnRunner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dmn_gate0_busy_skips() {
        let mut worker = DmnWorker::new(60.0, 10.0, 0.7);
        let outcome = worker.tick(true, None, || ("test".to_string(), 0.8));
        assert_eq!(outcome, DmnOutcome::SkippedBusy);
    }

    #[test]
    fn test_dmn_gate1_not_due_skips() {
        let mut worker = DmnWorker::new(60.0, 10.0, 0.7);
        // 直後はまだ待機時間を消化していない
        let outcome = worker.tick(false, None, || ("test".to_string(), 0.8));
        assert_eq!(outcome, DmnOutcome::SkippedNotDue);
    }

    #[test]
    fn test_dmn_gate2_silent_vs_spoken() {
        let mut worker = DmnWorker::new(60.0, 10.0, 0.7);
        // 強制的に時間を過去にする
        worker.last_tick = Instant::now() - std::time::Duration::from_secs(100);

        let store = MemoryStore::in_memory().unwrap();

        // 閾値未満 ➔ Silent
        let outcome_silent = worker.tick(false, Some(&store), || {
            ("低重要度の気づき".to_string(), 0.4)
        });
        assert!(matches!(
            outcome_silent,
            DmnOutcome::SilentReflection { .. }
        ));

        // 再度過去にして、閾値以上 ➔ Spoken
        worker.last_tick = Instant::now() - std::time::Duration::from_secs(100);
        let outcome_spoken = worker.tick(false, Some(&store), || ("重要な閃き".to_string(), 0.9));
        assert!(matches!(
            outcome_spoken,
            DmnOutcome::SpokenReflection { .. }
        ));
    }

    #[test]
    fn test_dmn_runner_respects_is_busy_and_cancels() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        let mut worker = DmnWorker::new(0.05, 0.01, 0.5);
        worker.last_tick = Instant::now() - std::time::Duration::from_secs(1);

        let is_busy = Arc::new(AtomicBool::new(true));
        let cancel = crate::cancel::CancellationToken::new();
        let spoken_messages = Arc::new(Mutex::new(Vec::new()));

        let spoken_clone = spoken_messages.clone();
        let mut runner = DmnRunner::spawn(
            worker,
            is_busy.clone(),
            cancel.clone(),
            None,
            |_| ("重要な洞察".to_string(), 0.9),
            move |msg| {
                spoken_clone.lock().unwrap().push(msg);
            },
        );

        // is_busy = true の間は発話されない
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert!(spoken_messages.lock().unwrap().is_empty());

        // is_busy = false にすると発話される
        is_busy.store(false, Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert!(!spoken_messages.lock().unwrap().is_empty());

        // stop すると停止する
        runner.stop();
    }

    #[test]
    fn test_dmn_worker_tick_records_insight_to_store() {
        let store = MemoryStore::in_memory().unwrap();
        let mut worker = DmnWorker::new(0.01, 0.001, 0.7);
        worker.last_tick = Instant::now() - std::time::Duration::from_secs(1);

        let outcome = worker.tick(false, Some(&store), || ("新発見の知見".to_string(), 0.8));

        assert!(matches!(outcome, DmnOutcome::SpokenReflection { .. }));

        // store に Insight が記録されたか検証
        let exp_store = ExperienceStore::new(store.conn());
        let insights = exp_store.find_similar("", 5).unwrap();
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].exp_type, ExperienceType::Insight);
        assert_eq!(insights[0].outcome, "新発見の知見");
    }

    #[test]
    fn test_dmn_worker_syncs_insight_to_vault() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut worker = DmnWorker::new(0.01, 0.001, 0.7).with_vault(temp_dir.path().to_path_buf());
        worker.last_tick = Instant::now() - std::time::Duration::from_secs(1);

        let outcome = worker.tick(false, None, || {
            ("Vaultに保存すべき重要な洞察".to_string(), 0.85)
        });
        assert!(matches!(outcome, DmnOutcome::SpokenReflection { .. }));

        let insights_md = temp_dir.path().join("insights.md");
        assert!(insights_md.exists());
        let content = std::fs::read_to_string(insights_md).unwrap();
        assert!(content.contains("Vaultに保存すべき重要な洞察"));
    }

    #[test]
    fn test_dmn_worker_syncs_insight_to_vault_and_kg_and_amem() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::in_memory().unwrap();
        let mut worker = DmnWorker::new(0.01, 0.001, 0.7).with_vault(temp_dir.path().to_path_buf());
        worker.last_tick = Instant::now() - std::time::Duration::from_secs(1);

        // 事前に既存の関連メモリを1件登録しておく（リンク形成テスト用）
        let existing_id = store
            .save_memory(
                "Rustのメモリ安全性とライフタイムについて",
                "concept",
                &["rust".to_string()],
            )
            .unwrap();

        let outcome = worker.tick(false, Some(&store), || {
            ("Rustの所有権モデルに関する重要な洞察".to_string(), 0.85)
        });
        assert!(matches!(outcome, DmnOutcome::SpokenReflection { .. }));

        // 1. Vault の insights.md に追記されていること
        let insights_md = temp_dir.path().join("insights.md");
        assert!(insights_md.exists());
        let content = std::fs::read_to_string(insights_md).unwrap();
        assert!(content.contains("Rustの所有権モデルに関する重要な洞察"));

        // 2. KnowledgeGraph にノードとエッジが登録されていること
        let kg = crate::memory::graph::KnowledgeGraph::new(store.conn());
        let neighbors = kg.neighbors("insights", 1).unwrap();
        assert!(
            neighbors
                .iter()
                .any(|(name, rel, _)| rel == "contains" && name.contains("Rustの所有権モデル"))
        );

        // 3. A-MEM (memories テーブル) に保存され、FTS5で検索可能であること
        let all = store.all_memories().unwrap();
        assert!(all.len() >= 2);
        let new_mem = all
            .iter()
            .find(|m| m.content.contains("所有権モデル"))
            .expect("New insight memory must exist in memories table");

        // タグ "dmn" による FTS5 全文検索でもヒットすることを検証
        let searched_by_tag = store.search_memories("dmn", 5).unwrap();
        assert!(searched_by_tag.iter().any(|m| m.id == new_mem.id));

        // 4. memory_links に直前メモリとの連想リンク（reflects_on）が記録されていること
        let link_count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM memory_links WHERE (source_id = ?1 AND target_id = ?2) OR (source_id = ?2 AND target_id = ?1)",
                rusqlite::params![new_mem.id, existing_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(link_count, 1);
    }

    #[test]
    fn test_dmn_triggers_deep_dream_on_prolonged_idle() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::in_memory().unwrap();

        // 過去の経験レコード（ツール使用など）を投入しておく
        let exp_store = ExperienceStore::new(store.conn());
        let _ = exp_store.record(&RecordParams {
            exp_type: ExperienceType::Failure,
            task_context: "file_operation",
            action: "read_missing_file",
            outcome: "ファイルが見つかりません",
            lesson: Some("事前存在確認が必要"),
            tool_name: Some("file_read"),
            error_type: Some("FileNotFound"),
            error_detail: None,
        });

        // dream_threshold = 0.05 秒（高速テスト用）
        let mut worker = DmnWorker::new(60.0, 10.0, 0.7)
            .with_vault(temp_dir.path().to_path_buf())
            .with_dream_threshold(0.05);

        // 最初は通常アイドル
        worker.tick(false, Some(&store), || ("通常内省".to_string(), 0.2));

        // アイドル開始を過去にする
        worker.idle_since = Some(Instant::now() - std::time::Duration::from_millis(100));

        // 再度 tick
        worker.tick(false, Some(&store), || ("通常内省2".to_string(), 0.2));

        // Dreaming により、experiences テーブルに "DMN自律Dreaming" が記録されていること
        let dreams_exps = exp_store.find_similar("DMN自律Dreaming", 5).unwrap();
        assert!(!dreams_exps.is_empty(), "Dreaming 洞察が記録されていること");

        // ナレッジ Vault にも記録されていること
        let insights_md = temp_dir.path().join("insights.md");
        assert!(
            insights_md.exists(),
            "Vault に insights.md が作成されていること"
        );
        let content = std::fs::read_to_string(insights_md).unwrap();
        assert!(content.contains("【メタ認知Dreaming】"));
    }
}
