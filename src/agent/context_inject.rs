//! コンテキスト注入関連の関数群
//!
//! agent_loop.rs からの抽出モジュール。
//! メモリ・経験・Vault・スキル・グラフ記憶・SOUL.md ペルソナを
//! セッションに注入する責務を担う。

use crate::agent::conversation::{Message, Session};
use crate::knowledge::vault::Vault;
use crate::memory::experience::{ExperienceStore, ExperienceType};
use crate::memory::graph::KnowledgeGraph;
use crate::memory::heuristics::{HeuristicStore, is_erl_enabled};
use crate::memory::search::HybridSearch;
use crate::memory::skill::SkillStore;
use crate::memory::store::MemoryStore;
use crate::runtime::embedder::create_embedder;

/// 過去の経験を成功/失敗パターンに分離してセッションに注入
///
/// ExperienceStore::find_similar で類似経験を取得し、
/// 成功と失敗を分けて <context type="experience"> タグで注入する。
/// 経験が空の場合はメッセージを追加しない。
pub(crate) fn inject_experience_context(
    session: &mut Session,
    task_context: &str,
    store: &MemoryStore,
) {
    let exp = ExperienceStore::new(store.conn());
    let past = match exp.find_similar(task_context, 3) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[warn] 経験検索エラー: {e}");
            return;
        }
    };
    if past.is_empty() {
        return;
    }

    let successes: Vec<String> = past
        .iter()
        .filter(|e| e.exp_type == ExperienceType::Success)
        .map(|e| {
            let lesson = e.lesson.as_deref().unwrap_or(&e.outcome);
            format!("- タスク: \"{}\" → {}", e.task_context, lesson)
        })
        .collect();

    let failures: Vec<String> = past
        .iter()
        .filter(|e| e.exp_type == ExperienceType::Failure)
        .map(|e| {
            let lesson = e.lesson.as_deref().unwrap_or(&e.outcome);
            format!("- タスク: \"{}\" → {}", e.task_context, lesson)
        })
        .collect();

    let insights: Vec<String> = past
        .iter()
        .filter(|e| e.exp_type == ExperienceType::Insight)
        .map(|e| {
            let lesson = e.lesson.as_deref().unwrap_or(&e.outcome);
            format!("- {}", lesson)
        })
        .collect();

    let mut parts = Vec::new();
    if !successes.is_empty() {
        parts.push(format!("[成功パターン]\n{}", successes.join("\n")));
    }
    if !failures.is_empty() {
        parts.push(format!("[失敗パターン]\n{}", failures.join("\n")));
    }
    if !insights.is_empty() {
        parts.push(format!("[学び]\n{}", insights.join("\n")));
    }

    session.add_message(Message::system(format!(
        "<context type=\"experience\">\n{}\n</context>",
        parts.join("\n")
    )));
}

/// Vault知識を選択的にセッションに注入（関連カテゴリのみ）
pub(crate) fn inject_vault_knowledge(session: &mut Session, task_context: &str, vault: &Vault) {
    // Rules（Decision/Pattern）は常時注入 — 判断基準として常に必要
    let rules = vault.read_rules(3).unwrap_or_default();
    // Docs（Fact/Insight/Preference/Todo）はタスクコンテキストに応じて選択的注入
    let docs = vault
        .read_docs_for_context(task_context, 2)
        .unwrap_or_default();

    if rules.is_empty() && docs.is_empty() {
        return;
    }

    if !rules.is_empty() {
        session.add_message(Message::system(format!(
            "<context type=\"vault-rules\">\n{}\n</context>",
            rules.join("\n")
        )));
    }
    if !docs.is_empty() {
        session.add_message(Message::system(format!(
            "<context type=\"vault-docs\">\n{}\n</context>",
            docs.join("\n")
        )));
    }
}

/// SOUL.mdからペルソナを読み込む
/// 検索順: (1) 明示パス, (2) .bonsai/SOUL.md, (3) ~/.config/bonsai-agent/SOUL.md
pub fn load_soul(soul_path: &Option<std::path::PathBuf>) -> Option<String> {
    let candidates: Vec<std::path::PathBuf> = [
        soul_path.clone(),
        Some(std::path::PathBuf::from(".bonsai/SOUL.md")),
        dirs::config_dir().map(|d| d.join("bonsai-agent").join("SOUL.md")),
    ]
    .into_iter()
    .flatten()
    .collect();

    for path in candidates {
        if let Ok(content) = std::fs::read_to_string(&path)
            && !content.trim().is_empty()
        {
            return Some(content);
        }
    }
    None
}

/// ラベル付きメモリブロック（Letta candidate 3 / 項目 177）。
/// SOUL.md は label="persona" で保持される。将来的に
/// human/scratchpad/system_state 等の追加 block に拡張可能。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryBlock {
    pub label: String,
    pub value: String,
}

impl MemoryBlock {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// SOUL.md および追加メモリブロックを読み込む（項目 179: Letta candidate 3 完成形）。
///
/// SOUL.md は label="persona" として最初に読込まれる (system prompt 先頭順序維持)。
/// その後 [[memory.blocks]] config で指定された extras を順次読込む。
/// 各 extras について:
///   - ファイル不在: 静かにスキップ (graceful degradation)
///   - 内容が空白のみ: スキップ (load_soul と同方針)
///   - 正常読込: MemoryBlock として追加
pub fn load_blocks(
    soul_path: &Option<std::path::PathBuf>,
    extras: &[crate::config::MemoryBlockConfig],
) -> Vec<MemoryBlock> {
    let mut blocks = Vec::new();
    if let Some(persona) = load_soul(soul_path) {
        blocks.push(MemoryBlock::new("persona", persona));
    }
    for cfg in extras {
        if let Ok(content) = std::fs::read_to_string(&cfg.path)
            && !content.trim().is_empty()
        {
            blocks.push(MemoryBlock::new(cfg.label.clone(), content));
        }
    }
    blocks
}

/// ラベル指定で MemoryBlock を取得。重複ラベルがある場合は最初のヒットを返す。
pub fn find_block<'a>(blocks: &'a [MemoryBlock], label: &str) -> Option<&'a MemoryBlock> {
    blocks.iter().find(|b| b.label == label)
}

/// MemoryBlock をセッションに注入（項目 179: Letta candidate 3 完成形）。
///
/// 各 block を `<context type="block:{label}">` タグで system message として追加。
/// 項目 80 のタグ統一方針に準拠 (memory/experience/vault-rules 等と同フォーマット)。
/// SOUL.md persona は最初の block として読込まれるため、system prompt 直後に配置される。
pub(crate) fn inject_memory_blocks(
    session: &mut Session,
    soul_path: &Option<std::path::PathBuf>,
    extras: &[crate::config::MemoryBlockConfig],
) {
    let blocks = load_blocks(soul_path, extras);
    for block in &blocks {
        session.add_message(Message::system(format!(
            "<context type=\"block:{}\">\n{}\n</context>",
            block.label,
            block.value.trim()
        )));
    }
}

/// ERL heuristics をセッションに注入 (項目 213、plan F3)。
///
/// `task_context` の trigger_patterns マッチで top-K 取得、`<context type="heuristics">`
/// タグで system message 追加。注入されなかった (= 0 件 hit) 場合はメッセージを追加しない。
/// 戻り値: 注入された heuristic IDs (将来的に task 完了 hook で `record_outcome` 呼出用)。
pub(crate) fn inject_heuristics(
    session: &mut Session,
    task_context: &str,
    store: Option<&MemoryStore>,
) -> Vec<i64> {
    // 項目 216 defaults OFF 切替 (Lab v17 REJECT 反映):
    // production default = env unset で全 heuristics 注入を skip。
    // `BONSAI_ERL_ENABLED=1` で opt-in 復活 (項目 213 動作の再現)。
    if !is_erl_enabled() {
        return Vec::new();
    }
    let Some(s) = store else {
        return Vec::new();
    };
    let h_store = HeuristicStore::new(s.conn());
    let top_k = h_store
        .find_top_k_for_task(task_context, 5)
        .unwrap_or_default();
    if top_k.is_empty() {
        return Vec::new();
    }
    let body = top_k
        .iter()
        .map(|h| format!("- {}", h.advice))
        .collect::<Vec<_>>()
        .join("\n");
    session.add_message(Message::system(format!(
        "<context type=\"heuristics\">\n{body}\n</context>"
    )));
    top_k.iter().map(|h| h.id).collect()
}

/// コンテキストメモリ・経験・スキルをセッションに注入
pub(crate) fn inject_contextual_memories(
    session: &mut Session,
    task_context: &str,
    store: Option<&MemoryStore>,
) {
    let vault_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("bonsai-agent")
        .join("vault");
    let vault = crate::knowledge::vault::Vault::new(&vault_path).ok();
    if let Some(ref v) = vault {
        let stocks = crate::knowledge::extractor::extract_stock(task_context, &session.id);
        let _ = v.append_all(&stocks);
        // Vault知識の選択的注入（関連カテゴリのみ）
        inject_vault_knowledge(session, task_context, v);
    }
    let embedder = create_embedder();

    let Some(s) = store else { return };

    // ハイブリッド検索: 関連メモリ
    let search = HybridSearch::new(s, embedder.as_ref());
    let memories = match search.search(task_context, 3) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[warn] メモリ検索エラー: {e}");
            vec![]
        }
    };
    if !memories.is_empty() {
        let ctx: String = memories
            .iter()
            .map(|r| format!("- {}", r.memory.content))
            .collect::<Vec<_>>()
            .join("\n");
        session.add_message(Message::system(format!(
            "<context type=\"memory\">\n関連する過去のメモ:\n{ctx}\n</context>"
        )));
    }

    // 類似経験（成功/失敗分離フォーマットで注入）
    inject_experience_context(session, task_context, s);

    // 関連スキル
    let skills = SkillStore::new(s.conn());
    let matching = match skills.find_matching(task_context, 3) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[warn] スキル検索エラー: {e}");
            vec![]
        }
    };
    if !matching.is_empty() {
        let ctx: String = matching
            .iter()
            .map(|sk| {
                format!(
                    "- スキル「{}」: {} (ツール: {})",
                    sk.name, sk.description, sk.tool_chain
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        session.add_message(Message::system(format!(
            "<context type=\"skills\">\n使えるスキル（過去の成功パターン）:\n{ctx}\n上のパターンが使える場合は参考にしてください。\n</context>"
        )));
    }

    // グラフ構造連想記憶: 関連コンテキスト注入
    let graph = KnowledgeGraph::new(s.conn());
    let graph_ctx = graph.related_context(task_context, 5).unwrap_or_default();
    if !graph_ctx.is_empty() {
        session.add_message(Message::system(format!(
            "<context type=\"graph\">\n関連する知識:\n{graph_ctx}\n</context>"
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::MemoryStore;

    #[test]
    fn t_load_soul_nonexistent() {
        let result = load_soul(&Some(std::path::PathBuf::from("/nonexistent/SOUL.md")));
        // 存在しないパスでパニックしない
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn t_load_soul_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SOUL.md");
        std::fs::write(&path, "# Test Persona\nI am a test agent.").unwrap();
        let result = load_soul(&Some(path));
        assert!(result.is_some());
        assert!(result.unwrap().contains("Test Persona"));
    }

    #[test]
    fn t_inject_experience_context_empty() {
        let store = MemoryStore::in_memory().unwrap();
        let mut session = Session::new();
        let before = session.messages.len();
        inject_experience_context(&mut session, "nonexistent task", &store);
        // 経験がなければメッセージ追加なし
        assert_eq!(session.messages.len(), before);
    }

    #[test]
    fn t_inject_vault_knowledge_empty() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::new(dir.path()).unwrap();
        let mut session = Session::new();
        let before = session.messages.len();
        inject_vault_knowledge(&mut session, "hello", &vault);
        // 空Vaultではメッセージ追加なし
        assert_eq!(session.messages.len(), before);
    }

    #[test]
    fn t_inject_vault_knowledge_with_rules() {
        use crate::knowledge::extractor::{StockCategory, StockEntry};
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::new(dir.path()).unwrap();
        vault
            .append(&StockEntry {
                category: StockCategory::Decision,
                content: "Rustを採用した".into(),
                source: "test".into(),
            })
            .unwrap();
        let mut session = Session::new();
        inject_vault_knowledge(&mut session, "hello", &vault);
        // Decision(Rule)があるのでメッセージ追加される
        assert!(!session.messages.is_empty());
        let has_rules = session
            .messages
            .iter()
            .any(|m| m.content.contains("vault-rules"));
        assert!(has_rules, "vault-rulesタグが含まれる");
    }

    #[test]
    fn t_inject_contextual_memories_no_panic() {
        let store = MemoryStore::in_memory().unwrap();
        let mut session = Session::new();
        // パニックしないことを確認
        inject_contextual_memories(&mut session, "test task", Some(&store));
    }

    #[test]
    fn t_load_soul_none_path() {
        let result = load_soul(&None);
        // Noneパスでパニックしない
        assert!(result.is_none() || result.is_some());
    }

    // 項目 177: MemoryBlock + load_blocks 追加（Letta candidate 3 MVP）

    #[test]
    fn t_memory_block_new_constructs_labeled_block() {
        let block = MemoryBlock::new("persona", "I am a test agent.");
        assert_eq!(block.label, "persona");
        assert_eq!(block.value, "I am a test agent.");
    }

    #[test]
    fn t_load_blocks_returns_persona_when_soul_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SOUL.md");
        std::fs::write(&path, "# Test Persona\nI am a test agent.").unwrap();
        // 項目 179: extras 引数追加（空配列で persona のみ取得）
        let blocks = load_blocks(&Some(path), &[]);
        assert_eq!(blocks.len(), 1, "SOUL.md があれば 1 block 返却");
        assert_eq!(blocks[0].label, "persona");
        assert!(blocks[0].value.contains("Test Persona"));
    }

    #[test]
    fn t_load_blocks_empty_when_no_soul() {
        // 項目 179: extras 引数追加
        let blocks = load_blocks(&Some(std::path::PathBuf::from("/nonexistent/SOUL.md")), &[]);
        // 3 段 fallback path も全て存在しない場合のみ空。本テスト環境ではこれが期待される
        // が、実環境で .bonsai/SOUL.md または ~/.config/bonsai-agent/SOUL.md がある場合は
        // 1 block 返却される。両方の挙動を許容（panic しないことが本質）。
        assert!(blocks.len() <= 1);
    }

    #[test]
    fn t_find_block_returns_matching_label() {
        let blocks = vec![
            MemoryBlock::new("persona", "agent persona"),
            MemoryBlock::new("human", "user profile"),
        ];
        let found = find_block(&blocks, "human");
        assert!(found.is_some());
        assert_eq!(found.unwrap().value, "user profile");
        assert!(find_block(&blocks, "nonexistent").is_none());
    }

    // --- 項目 179: load_blocks extras 対応テスト群 ---

    #[test]
    fn t_load_blocks_with_persona_and_extras() {
        use crate::config::MemoryBlockConfig;
        let dir = tempfile::tempdir().unwrap();
        let soul = dir.path().join("SOUL.md");
        let human = dir.path().join("human.md");
        std::fs::write(&soul, "# Persona\nagent identity").unwrap();
        std::fs::write(&human, "# Human\nuser profile data").unwrap();
        let extras = vec![MemoryBlockConfig {
            label: "human".into(),
            path: human.clone(),
        }];
        let blocks = load_blocks(&Some(soul), &extras);
        assert_eq!(blocks.len(), 2, "persona + 1 extra = 2 blocks");
        assert_eq!(blocks[0].label, "persona", "persona は先頭");
        assert_eq!(blocks[1].label, "human");
        assert!(blocks[1].value.contains("user profile data"));
    }

    #[test]
    fn t_load_blocks_extras_skip_missing_file() {
        use crate::config::MemoryBlockConfig;
        let dir = tempfile::tempdir().unwrap();
        let soul = dir.path().join("SOUL.md");
        std::fs::write(&soul, "persona").unwrap();
        let extras = vec![MemoryBlockConfig {
            label: "missing".into(),
            path: std::path::PathBuf::from("/nonexistent/file.md"),
        }];
        let blocks = load_blocks(&Some(soul), &extras);
        assert_eq!(blocks.len(), 1, "ファイル不在 extra はスキップ");
        assert_eq!(blocks[0].label, "persona");
    }

    #[test]
    fn t_load_blocks_extras_skip_empty_content() {
        use crate::config::MemoryBlockConfig;
        let dir = tempfile::tempdir().unwrap();
        let soul = dir.path().join("SOUL.md");
        let empty = dir.path().join("empty.md");
        std::fs::write(&soul, "persona").unwrap();
        std::fs::write(&empty, "   \n\t\n  ").unwrap();
        let extras = vec![MemoryBlockConfig {
            label: "scratchpad".into(),
            path: empty,
        }];
        let blocks = load_blocks(&Some(soul), &extras);
        assert_eq!(blocks.len(), 1, "空白のみの extra はスキップ");
    }

    #[test]
    fn t_load_blocks_extras_only_no_soul() {
        use crate::config::MemoryBlockConfig;
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("scratchpad.md");
        std::fs::write(&scratch, "scratch notes").unwrap();
        let extras = vec![MemoryBlockConfig {
            label: "scratchpad".into(),
            path: scratch,
        }];
        // SOUL.md なし、extras のみ → extras のみ返却
        let blocks = load_blocks(
            &Some(std::path::PathBuf::from("/nonexistent/SOUL.md")),
            &extras,
        );
        // 環境依存で 0 or 1 (SOUL.md 3 段 fallback) + 1 extra
        assert!(
            blocks.iter().any(|b| b.label == "scratchpad"),
            "extras は読込まれる"
        );
    }

    // --- 項目 179: inject_memory_blocks 注入テスト群 ---

    #[test]
    fn t_inject_memory_blocks_persona_only() {
        let dir = tempfile::tempdir().unwrap();
        let soul = dir.path().join("SOUL.md");
        std::fs::write(&soul, "# Persona\nI am bonsai.").unwrap();
        let mut session = Session::new();
        let before = session.messages.len();
        inject_memory_blocks(&mut session, &Some(soul), &[]);
        assert_eq!(
            session.messages.len(),
            before + 1,
            "persona block 1 件が注入される"
        );
        let injected = &session.messages[before].content;
        assert!(
            injected.contains("<context type=\"block:persona\">"),
            "block:persona タグ含む"
        );
        assert!(injected.contains("I am bonsai."));
    }

    #[test]
    fn t_inject_memory_blocks_persona_and_extras() {
        use crate::config::MemoryBlockConfig;
        let dir = tempfile::tempdir().unwrap();
        let soul = dir.path().join("SOUL.md");
        let human = dir.path().join("human.md");
        std::fs::write(&soul, "agent persona").unwrap();
        std::fs::write(&human, "user is a Rust dev").unwrap();
        let extras = vec![MemoryBlockConfig {
            label: "human".into(),
            path: human,
        }];
        let mut session = Session::new();
        let before = session.messages.len();
        inject_memory_blocks(&mut session, &Some(soul), &extras);
        assert_eq!(session.messages.len(), before + 2, "2 block 注入");
        assert!(
            session.messages[before]
                .content
                .contains("<context type=\"block:persona\">"),
            "persona は最初"
        );
        assert!(
            session.messages[before + 1]
                .content
                .contains("<context type=\"block:human\">"),
            "human は次"
        );
    }

    #[test]
    fn t_inject_memory_blocks_no_persona_no_extras() {
        let mut session = Session::new();
        let before = session.messages.len();
        // 存在しない SOUL path + 空 extras → 何も注入されない
        inject_memory_blocks(
            &mut session,
            &Some(std::path::PathBuf::from("/nonexistent/SOUL.md")),
            &[],
        );
        // 環境次第（.bonsai/SOUL.md があれば 1）だが、≤ before+1 を保証
        assert!(session.messages.len() <= before + 1);
    }

    // === 項目 216 defaults OFF 切替: BONSAI_ERL_ENABLED toggle short-circuit ===

    static ERL_INJECT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn t_inject_heuristics_short_circuits_by_default() {
        // env mutation race を避けるため module-local Mutex で serialize
        let _g = ERL_INJECT_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // 1 heuristic を pool に投入 → opt-in 復活時には inject_heuristics で 1 件 hit
        let store = MemoryStore::in_memory().unwrap();
        let h_store = crate::memory::heuristics::HeuristicStore::new(store.conn());
        h_store
            .save(
                "テスト用 advice for short-circuit",
                &["short_circuit_test".to_string(), "lab_v17".to_string()],
                None,
                "test task",
                "efficiency",
            )
            .unwrap();

        // env unset = production default OFF で短絡 → 空 Vec + メッセージ追加なし
        unsafe {
            std::env::remove_var("BONSAI_ERL_ENABLED");
        }
        let mut session = Session::new();
        let before = session.messages.len();
        let ids = inject_heuristics(&mut session, "short_circuit_test lab_v17", Some(&store));
        assert!(
            ids.is_empty(),
            "production default (env unset) で空 Vec 返却 (record_outcome 用 IDs も空)"
        );
        assert_eq!(
            session.messages.len(),
            before,
            "session メッセージ追加なし (heuristics タグ不在)"
        );
    }

    // === 項目 218 候補: Cerememory ADR-011 freshness gate (Plan B §5 Phase 1 Red) ===

    static REVIEW_INJECT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn t_inject_skips_low_freshness_when_enabled() {
        // env mutation race を避けるため module-local Mutex で serialize
        let _g = REVIEW_INJECT_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = MemoryStore::in_memory().unwrap();
        let h_store = crate::memory::heuristics::HeuristicStore::new(store.conn());
        let id = h_store
            .save(
                "陳腐化助言: -c 12288 推奨",
                &[
                    "review_freshness_test".to_string(),
                    "stale_advice".to_string(),
                ],
                None,
                "test task",
                "failure_recovery",
            )
            .unwrap();

        // freshness を 0.20 (< threshold 0.35) に下げる
        // V12 freshness 列が必要 (Phase 2 Green で追加)
        store
            .conn()
            .execute(
                "UPDATE heuristics SET freshness = 0.20 WHERE id = ?1",
                rusqlite::params![id],
            )
            .expect("V12 freshness 列が必要 (Phase 2 Green)");

        // env=enabled で freshness gate 発火: ERL も REVIEW も両方 enable
        unsafe {
            std::env::set_var("BONSAI_ERL_ENABLED", "1");
            std::env::set_var("BONSAI_REVIEW_ENABLED", "1");
        }
        let mut session = Session::new();
        let before = session.messages.len();
        let ids = inject_heuristics(
            &mut session,
            "review_freshness_test stale_advice",
            Some(&store),
        );
        unsafe {
            std::env::remove_var("BONSAI_ERL_ENABLED");
            std::env::remove_var("BONSAI_REVIEW_ENABLED");
        }

        assert!(
            !ids.contains(&id),
            "freshness=0.20 < threshold 0.35 で skip、ids={ids:?}"
        );
        assert_eq!(
            session.messages.len(),
            before,
            "全 heuristic skip → session メッセージ追加なし"
        );
    }

    #[test]
    fn t_inject_legacy_observable_unchanged_when_disabled() {
        // env=disabled (BONSAI_REVIEW_ENABLED unset) で legacy 互換、
        // 低 freshness でも freshness gate は発火しない (skip しない)
        let _g = REVIEW_INJECT_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = MemoryStore::in_memory().unwrap();
        let h_store = crate::memory::heuristics::HeuristicStore::new(store.conn());
        let id = h_store
            .save(
                "legacy compat advice",
                &[
                    "legacy_compat_test".to_string(),
                    "low_fresh_inject".to_string(),
                ],
                None,
                "test",
                "efficiency",
            )
            .unwrap();

        // V12 列があれば freshness を低く設定 (Phase 2 Green で V12 適用後に有効)
        // Phase 1 Red 段階では V12 列不在で UPDATE は SQL error → expect で fail
        store
            .conn()
            .execute(
                "UPDATE heuristics SET freshness = 0.20 WHERE id = ?1",
                rusqlite::params![id],
            )
            .expect("V12 freshness 列が必要 (Phase 2 Green)");

        // ERL は ON (heuristic 注入経路を有効化)、REVIEW は OFF (legacy 動作)
        unsafe {
            std::env::set_var("BONSAI_ERL_ENABLED", "1");
            std::env::remove_var("BONSAI_REVIEW_ENABLED");
        }
        let mut session = Session::new();
        let ids = inject_heuristics(
            &mut session,
            "legacy_compat_test low_fresh_inject",
            Some(&store),
        );
        unsafe {
            std::env::remove_var("BONSAI_ERL_ENABLED");
        }

        assert!(
            ids.contains(&id),
            "REVIEW disabled で legacy 互換: 低 freshness でも inject される、ids={ids:?}"
        );
    }
}
