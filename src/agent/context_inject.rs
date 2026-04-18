//! コンテキスト注入関連の関数群
//!
//! agent_loop.rs からの抽出モジュール。
//! メモリ・経験・Vault・スキル・グラフ記憶・SOUL.md ペルソナを
//! セッションに注入する責務を担う。

use crate::agent::conversation::{Message, Session};
use crate::knowledge::vault::Vault;
use crate::memory::experience::{ExperienceStore, ExperienceType};
use crate::memory::graph::KnowledgeGraph;
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
pub(crate) fn inject_vault_knowledge(
    session: &mut Session,
    task_context: &str,
    vault: &Vault,
) {
    // Rules（Decision/Pattern）は常時注入 — 判断基準として常に必要
    let rules = vault.read_rules(3).unwrap_or_default();
    // Docs（Fact/Insight/Preference/Todo）はタスクコンテキストに応じて選択的注入
    let docs = vault.read_docs_for_context(task_context, 2).unwrap_or_default();

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
        session.add_message(Message::system(format!("<context type=\"memory\">\n関連する過去の記憶:\n{ctx}\n</context>")));
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
            "<context type=\"skills\">\n利用可能なスキル（過去の成功パターン）:\n{ctx}\n上記のパターンが適用可能な場合は参考にしてください。\n</context>"
        )));
    }

    // グラフ構造連想記憶: 関連コンテキスト注入
    let graph = KnowledgeGraph::new(s.conn());
    let graph_ctx = graph.related_context(task_context, 5).unwrap_or_default();
    if !graph_ctx.is_empty() {
        session.add_message(Message::system(format!(
            "<context type=\"graph\">\n関連するグラフ知識:\n{graph_ctx}\n</context>"
        )));
    }
}
