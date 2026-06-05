//! 概念ページ合成 (agent 層、LLM、env-gated)。
//!
//! 知識基盤強化計画 Phase 2 (`.claude/plan/knowledge-base-concept-pages.md`)。
//! knowledge 層が検出した概念候補 ([[concept]]) について、**member の生 entry を再読込** し
//! LLM backend で「概要 / 横断的知見 / 未解決の問い」を合成、`concepts/<slug>.md` + graph に永続化。
//!
//! 層分離: LLM backend (`domain::llm::LlmBackend` port) の consume は本 agent 層に閉じる。
//! 永続化は knowledge 層 (`Vault::write_concept_page` / `record_concept_to_graph`) に委譲。
//! `BONSAI_CONCEPT_SYNTHESIS=1` 既定 OFF (env unset で完全 no-op、後方互換)。

use anyhow::Result;
use std::path::PathBuf;

use crate::cancel::CancellationToken;
use crate::domain::conversation::Message;
use crate::domain::llm::LlmBackend;
use crate::knowledge::concept::{
    ConceptConfig, ConceptPage, detect_concept_candidates, member_entries,
};
use crate::knowledge::extractor::StockEntry;
use crate::knowledge::vault::Vault;
use crate::memory::graph::KnowledgeGraph;

/// 1 概念候補について LLM 合成用の messages を構築する純粋関数。
///
/// member の **生 content + source** を列挙し、inline `[[source]]` 出典と推測禁止を指示する。
pub fn build_synthesis_messages(theme: &str, members: &[&StockEntry]) -> Vec<Message> {
    let system = "あなたは知識統合の専門家です。複数の出典を横断して 1 つの概念ページを合成してください。\n\
         必ず次の 3 節を書く: 『概要』『横断的知見』『未解決の問い』。\n\
         全ての主張に inline 出典 `[[source]]` を付け、出典に無い情報を推測で補わないこと。";
    let mut body = format!("# 概念テーマ: {theme}\n\n以下の出典を統合してください。\n\n");
    for m in members {
        let src = if m.source.is_empty() {
            "unknown"
        } else {
            &m.source
        };
        body.push_str(&format!("- [[{}]] {}\n", src, m.content));
    }
    vec![Message::system(system), Message::user(body)]
}

/// 概念ページを合成して永続化する (env-gated orchestration)。
///
/// 手順: env チェック → 候補検出 → 各候補で member 生 entry 再読込 → LLM 合成 →
/// `Vault::write_concept_page` + `record_concept_to_graph`。
/// 戻り値: 書き出した概念ページのパス一覧。env OFF / 候補なしで空 Vec。
#[allow(clippy::too_many_arguments)]
pub fn synthesize_concepts(
    entries: &[StockEntry],
    vault: &Vault,
    graph: &KnowledgeGraph,
    backend: &dyn LlmBackend,
    cancel: &CancellationToken,
    config: &ConceptConfig,
    updated_at: &str,
) -> Result<Vec<PathBuf>> {
    if !crate::config::is_concept_synthesis_enabled() {
        return Ok(Vec::new());
    }

    let candidates = detect_concept_candidates(entries, config);
    let mut written = Vec::new();
    for candidate in &candidates {
        if cancel.is_cancelled() {
            break;
        }
        let members = member_entries(candidate, entries);
        if members.is_empty() {
            continue;
        }
        let messages = build_synthesis_messages(&candidate.theme_key, &members);
        // raw 再読込済の member を LLM に渡し横断的知見を合成。Err は graceful skip。
        let body = match backend.generate(&messages, &[], &mut |_| {}, cancel) {
            Ok(result) => result.text,
            Err(_) => continue,
        };
        let page = ConceptPage {
            theme_key: candidate.theme_key.clone(),
            sources: candidate.member_sources.clone(),
            body,
            status: "draft".to_string(),
        };
        let path = vault.write_concept_page(&page, updated_at)?;
        vault.record_concept_to_graph(&page, graph)?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::MockLlmBackend;
    use crate::knowledge::extractor::StockCategory;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn entry(content: &str, source: &str) -> StockEntry {
        StockEntry {
            category: StockCategory::Fact,
            content: content.to_string(),
            source: source.to_string(),
        }
    }

    fn rust_entries() -> Vec<StockEntry> {
        vec![
            entry("rust ownership prevents data races", "session_a"),
            entry("rust borrow checker enforces lifetimes", "session_b"),
            entry("rust zero cost abstractions improve speed", "session_c"),
        ]
    }

    #[test]
    fn t_build_synthesis_messages_embeds_raw_and_sources() {
        let entries = rust_entries();
        let refs: Vec<&StockEntry> = entries.iter().collect();
        let msgs = build_synthesis_messages("rust", &refs);
        assert_eq!(msgs.len(), 2);
        let user = &msgs[1].content;
        assert!(
            user.contains("rust ownership prevents data races"),
            "生 content"
        );
        assert!(user.contains("[[session_a]]"), "inline 出典");
        assert!(msgs[0].content.contains("未解決の問い"), "3 節指示");
    }

    #[test]
    fn t_env_off_is_noop() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: single-threaded via ENV_LOCK、test 専用
        unsafe { std::env::remove_var("BONSAI_CONCEPT_SYNTHESIS") };
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::new(dir.path()).unwrap();
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let graph = KnowledgeGraph::new(store.conn());
        let backend = MockLlmBackend::single("body [[session_a]]");
        let cancel = CancellationToken::new();
        let paths = synthesize_concepts(
            &rust_entries(),
            &vault,
            &graph,
            &backend,
            &cancel,
            &ConceptConfig::default(),
            "2026-06-05 10:00",
        )
        .unwrap();
        assert!(paths.is_empty(), "env OFF で no-op");
        assert!(!dir.path().join("concepts").exists(), "concepts dir 未作成");
    }

    #[test]
    fn t_env_on_synthesizes_and_persists() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: single-threaded via ENV_LOCK、test 専用
        unsafe { std::env::set_var("BONSAI_CONCEPT_SYNTHESIS", "1") };
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::new(dir.path()).unwrap();
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let graph = KnowledgeGraph::new(store.conn());
        let backend = MockLlmBackend::single("概要: rust は所有権で安全 [[session_a]]。");
        let cancel = CancellationToken::new();
        let paths = synthesize_concepts(
            &rust_entries(),
            &vault,
            &graph,
            &backend,
            &cancel,
            &ConceptConfig::default(),
            "2026-06-05 10:00",
        )
        .unwrap();
        unsafe { std::env::remove_var("BONSAI_CONCEPT_SYNTHESIS") };

        assert_eq!(paths.len(), 1, "1 概念ページ生成: {paths:?}");
        let content = std::fs::read_to_string(&paths[0]).unwrap();
        assert!(content.contains("theme: rust"));
        assert!(content.contains("[[session_a]]"), "LLM 合成本文に出典保持");
        assert!(content.contains("status: draft"));
        // graph に concept ノード + synthesizes エッジ
        let neighbors = graph.neighbors("rust", 1).unwrap();
        assert!(
            neighbors.iter().any(|(_, rel, _)| rel == "synthesizes"),
            "synthesizes エッジ: {neighbors:?}"
        );
    }
}
