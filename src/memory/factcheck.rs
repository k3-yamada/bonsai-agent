//! KG-Grounded Hallucination Check (Plan A、項目 230 候補)
//!
//! plan: `.claude/plan/kg-grounded-fact-check-impl.md`
//!
//! LLM 出力テキストから (Subject, Predicate, Object) トリプルを抽出し、
//! KnowledgeGraph 上での path 検証で `Match / Unknown / Conflict` を判定する
//! post-hoc Lab metric。
//!
//! Phase 1 (Red): 本 module は API skeleton のみ。実装は Phase 2 Green で行う。
//! 全 public fn は `todo!()` で panic し、6 test が Red になることを確証する。
//!
//! production default OFF (`BONSAI_KG_FACTCHECK_ENABLED` env opt-in、
//! Cerememory 三本柱と同 pattern)。

use crate::memory::graph::KnowledgeGraph;
use regex::Regex;
use std::sync::LazyLock;

/// "X is the Y of Z" pattern (e.g., "Alice is the parent of Bob")。
/// 大文字始まり英単語の subject/object、小文字 predicate を要件とする保守的 pattern。
/// LLM 出力の典型的英語表現に focus、日本語混在は Phase 5 別 plan。
static RE_IS_THE_OF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-Z][A-Za-z0-9_]*)\s+is\s+the\s+([a-z][a-z_]*)\s+of\s+([A-Z][A-Za-z0-9_]*)")
        .expect("static regex must compile")
});

/// "X is a Y" pattern (e.g., "Bonsai-8B is a model")。
static RE_IS_A: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-Z][A-Za-z0-9_\-]*)\s+is\s+an?\s+([a-z][a-z0-9_]*)\b")
        .expect("static regex must compile")
});

/// LLM 出力から抽出した (Subject, Predicate, Object) トリプル。
///
/// `confidence` は EidoGraph 由来の extraction confidence (`0.0..1.0`)、
/// graph 上の edge weight とは別軸 (Phase 3 で二軸分離を refactor)。
#[derive(Debug, Clone, PartialEq)]
pub struct Triple {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f64,
}

/// トリプル検証結果。
///
/// - `Match { path_len }`: KG 内に triple と一致する path 発見 (BFS distance)
/// - `Unknown`: subject/object のいずれかが KG に未登録 (= 一般知識、false positive 回避)
/// - `Conflict { conflicting_edge }`: subject + predicate 同一だが object 不一致 (= fabricate 疑い)
#[derive(Debug, Clone, PartialEq)]
pub enum FactCheckResult {
    Match { path_len: usize },
    Unknown,
    Conflict { conflicting_edge: String },
}

/// 複数 triple 検証結果の集約 (Lab post-hoc metric)。
///
/// production agent_loop は本 struct を生成しない (default OFF)。Lab cycle 末尾の
/// AgentHER hook 直前で `run_factcheck_pass` 経由で生成、`[INFO][lab.factcheck]` log 出力。
///
/// confidence/weight 二軸分離 (plan §3 Phase 3):
/// - extraction confidence: Triple.confidence (0.0..1.0、regex pattern 信頼度)
/// - graph 内 weight: edge weight (KnowledgeGraph 経由、本 summary では未集約、別 plan)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FactCheckSummary {
    pub total: usize,
    pub matched: usize,
    pub unknown: usize,
    pub conflicting: usize,
    /// matched triple の平均 path_len (matched=0 のとき 0.0)
    pub mean_path_len: f64,
}

/// LLM 出力テキストから triple を rule-based regex で抽出。
///
/// 低 recall 高 precision (保守的) 方針: 確信度高い 2 pattern のみ match。
/// LLM ベース抽出は overhead 許容外のため rule-based で開始、Phase 5 別 plan で LLM
/// フォールバック検討。
///
/// 対応 pattern:
/// - `"X is the Y of Z"` → `Triple { X, Y_of, Z, confidence: 0.85 }`
/// - `"X is a Y"` / `"X is an Y"` → `Triple { X, is_a, Y, confidence: 0.80 }`
///
/// confidence は EidoGraph 由来 (Phase 3 で weight 軸と分離 refactor)。
pub fn extract_triples_from_text(text: &str) -> Vec<Triple> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let mut results = Vec::new();
    for cap in RE_IS_THE_OF.captures_iter(text) {
        results.push(Triple {
            subject: cap[1].to_string(),
            predicate: format!("{}_of", &cap[2]),
            object: cap[3].to_string(),
            confidence: 0.85,
        });
    }
    for cap in RE_IS_A.captures_iter(text) {
        results.push(Triple {
            subject: cap[1].to_string(),
            predicate: "is_a".to_string(),
            object: cap[2].to_string(),
            confidence: 0.80,
        });
    }
    results
}

/// トリプルを KnowledgeGraph で検証して `FactCheckResult` を返す。
///
/// 判定優先順位 (false positive 回避を最優先):
/// 1. KG に exact match path → `Match { path_len }`
/// 2. subject + predicate 同一だが object が KG 内の既存 object と不一致 → `Conflict`
/// 3. 上記いずれにも該当しない (subject/object 未登録 or 不在) → `Unknown`
///
/// `Conflict` は subject + predicate が KG 内に登録されている前提で、object のみ
/// LLM 出力と異なるケース = 1bit LLM の典型的 fabricate pattern。
/// KG が網羅していない一般知識は `Unknown` に分類し、production retry trigger に
/// しないことで false positive を回避 (plan §1 設計原則)。
pub fn verify_triple_in_kg(triple: &Triple, kg: &KnowledgeGraph<'_>) -> FactCheckResult {
    if let Some(path_len) = kg.contains_triple(&triple.subject, &triple.predicate, &triple.object) {
        return FactCheckResult::Match { path_len };
    }
    let conflicts = kg.find_conflicting_edges(&triple.subject, &triple.predicate);
    let conflicting = conflicts
        .iter()
        .find(|(obj, _)| obj != &triple.object)
        .map(|(obj, rel)| format!("({}, {}, {})", triple.subject, rel, obj));
    if let Some(edge) = conflicting {
        return FactCheckResult::Conflict {
            conflicting_edge: edge,
        };
    }
    FactCheckResult::Unknown
}

/// env opt-in 判定 (`BONSAI_KG_FACTCHECK_ENABLED=1` または `true` で ON、default OFF)。
///
/// Cerememory 三本柱と同 pattern: 項目 217 decay / 218 review / 219 working_memory。
/// 大文字小文字を問わず `"true"` / `"1"` を ON 判定、それ以外は OFF。
pub fn is_factcheck_enabled() -> bool {
    std::env::var("BONSAI_KG_FACTCHECK_ENABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Plan A G-4c 用に KG に 3 fact を投入する seed 関数 (Lab cycle 開始時のみ呼出)。
///
/// halluc benchmark task 3 件 (benchmark.rs:halluc_*) の正解 fact を KG に登録し、
/// LLM が捏造 (false fact) すれば `verify_triple_in_kg` が `Conflict` 判定を出す
/// 経路を確立する。冪等 (add_node / add_edge は UPSERT、再 seed で weight 加算のみ)。
///
/// 投入する 3 fact (G-4c v1 反証受けて大文字始まり化、Pattern 1/2 regex match 経路確保):
///   - (Bonsai-8B, parent_of, Qwen3-8B) — halluc_parent_of_false_fact 正解
///   - (Prism-ml, is_a, ternary_model) — halluc_is_a_false_type 正解 (subject 大文字始まり)
///   - (Bonsai-Agent, child_of, Bonsai-8B) — halluc_t2_file_context_misalign 正解
///     (file fixture `/tmp/bonsai_halluc_ctx.txt` と integrity 一致、subject/object 両大文字始まり)
///
/// 呼出元: `experiment.rs::run_factcheck_pass_lab` 内、env-gated 経路で 1 度のみ。
/// production agent_loop は本 fn を呼ばない (`is_factcheck_enabled()` で OFF 時短絡)。
pub fn seed_kg_for_factcheck_lab(kg: &KnowledgeGraph<'_>) -> anyhow::Result<()> {
    let facts: &[(&str, &str, &str)] = &[
        ("Bonsai-8B", "parent_of", "Qwen3-8B"),
        ("Prism-ml", "is_a", "ternary_model"),
        ("Bonsai-Agent", "child_of", "Bonsai-8B"),
    ];
    for (subj, pred, obj) in facts {
        let s = kg.add_node("entity", subj)?;
        let o = kg.add_node("entity", obj)?;
        kg.add_edge(s, o, pred, 1.0)?;
    }
    Ok(())
}

/// 複数テキストに対し triple 抽出 + KG 検証を一括実行し、集約 summary を返す。
///
/// Lab cycle 末尾の AgentHER hook 直前で `failed_trajectories` 由来テキストを渡す想定
/// (Phase 4 Smoke で配線、Phase 5 別 plan で effectiveness 検証)。
/// production agent_loop は本 fn を呼ばない (`is_factcheck_enabled()` で OFF 時は
/// caller 側で短絡)。
pub fn run_factcheck_pass(texts: &[String], kg: &KnowledgeGraph<'_>) -> FactCheckSummary {
    let mut summary = FactCheckSummary::default();
    let mut path_len_sum: usize = 0;
    for text in texts {
        for triple in extract_triples_from_text(text) {
            summary.total += 1;
            match verify_triple_in_kg(&triple, kg) {
                FactCheckResult::Match { path_len } => {
                    summary.matched += 1;
                    path_len_sum += path_len;
                }
                FactCheckResult::Unknown => summary.unknown += 1,
                FactCheckResult::Conflict { .. } => summary.conflicting += 1,
            }
        }
    }
    if summary.matched > 0 {
        summary.mean_path_len = path_len_sum as f64 / summary.matched as f64;
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::MemoryStore;

    /// テスト用のインメモリ DB。
    /// `MemoryStore::in_memory()` 経由で vec0 auto-load + 全 migration を流す
    /// (V13 sqlite-vec vec0 virtual table を含むため、`Connection::open_in_memory`
    /// 直接呼出では `no such module: vec0` で失敗する。store.rs:11-36
    /// `init_vec_extension` が process-global once-load を保証)。
    fn setup_db() -> MemoryStore {
        MemoryStore::in_memory().unwrap()
    }

    /// Phase 1 Red — 基本 triple 抽出。
    /// 期待: `"A is the parent of B"` → `[Triple { "A", "parent_of", "B", _ }]`
    /// Phase 2 Green で `todo!()` 解除後、上記アサーションが Green になる。
    #[test]
    fn t_extract_triples_from_text_basic() {
        let triples = extract_triples_from_text("Alice is the parent of Bob");
        assert!(
            !triples.is_empty(),
            "「A is the parent of B」から triple 抽出されるべき"
        );
        assert_eq!(triples[0].subject, "Alice");
        assert_eq!(triples[0].predicate, "parent_of");
        assert_eq!(triples[0].object, "Bob");
    }

    /// Phase 1 Red — 空文字列は空 Vec。
    /// Phase 2 Green で `todo!()` 解除後、空入力でも panic せず空 Vec を返す挙動に。
    #[test]
    fn t_extract_triples_handles_empty() {
        let triples = extract_triples_from_text("");
        assert!(triples.is_empty(), "空文字列は空 Vec を返すべき");
    }

    /// Phase 1 Red — KG 一致 → Match。
    /// 期待: graph に (Alice, parent_of, Bob) edge ありで Match { path_len: 1 }
    #[test]
    fn t_verify_triple_in_kg_match() {
        let store = setup_db();
        let graph = KnowledgeGraph::new(store.conn());
        let alice = graph.add_node("entity", "Alice").unwrap();
        let bob = graph.add_node("entity", "Bob").unwrap();
        graph.add_edge(alice, bob, "parent_of", 1.0).unwrap();

        let triple = Triple {
            subject: "Alice".into(),
            predicate: "parent_of".into(),
            object: "Bob".into(),
            confidence: 1.0,
        };
        let result = verify_triple_in_kg(&triple, &graph);
        assert!(matches!(result, FactCheckResult::Match { .. }));
    }

    /// Phase 1 Red — KG に未登録 → Unknown (NOT Conflict)。
    /// 期待: graph が空で `Unknown` (一般知識を KG が網羅できない false positive 回避)
    #[test]
    fn t_verify_triple_in_kg_unknown() {
        let store = setup_db();
        let graph = KnowledgeGraph::new(store.conn());

        let triple = Triple {
            subject: "UnknownEntity".into(),
            predicate: "is_a".into(),
            object: "SomeType".into(),
            confidence: 1.0,
        };
        let result = verify_triple_in_kg(&triple, &graph);
        assert_eq!(result, FactCheckResult::Unknown);
    }

    /// Phase 1 Red — KG に対立 edge → Conflict。
    /// 期待: (Alice, parent_of, Bob) が graph 内、(Alice, parent_of, Charlie) 検証で Conflict
    #[test]
    fn t_verify_triple_in_kg_conflict() {
        let store = setup_db();
        let graph = KnowledgeGraph::new(store.conn());
        let alice = graph.add_node("entity", "Alice").unwrap();
        let bob = graph.add_node("entity", "Bob").unwrap();
        graph.add_edge(alice, bob, "parent_of", 1.0).unwrap();

        let triple = Triple {
            subject: "Alice".into(),
            predicate: "parent_of".into(),
            object: "Charlie".into(),
            confidence: 1.0,
        };
        let result = verify_triple_in_kg(&triple, &graph);
        assert!(matches!(result, FactCheckResult::Conflict { .. }));
    }

    /// Phase 3 Refactor — `run_factcheck_pass` 集約。
    /// 期待: matched=1 / unknown=1 / conflicting=1 / total=3 / mean_path_len=1.0
    #[test]
    fn t_run_factcheck_pass_aggregates_three_outcomes() {
        let store = setup_db();
        let graph = KnowledgeGraph::new(store.conn());
        // KG: (Alice, parent_of, Bob)
        let alice = graph.add_node("entity", "Alice").unwrap();
        let bob = graph.add_node("entity", "Bob").unwrap();
        graph.add_edge(alice, bob, "parent_of", 1.0).unwrap();

        let texts = vec![
            "Alice is the parent of Bob".to_string(),     // Match
            "Alice is the parent of Charlie".to_string(), // Conflict
            "Dave is the friend of Eve".to_string(),      // Unknown (Dave 未登録)
        ];
        let summary = run_factcheck_pass(&texts, &graph);
        assert_eq!(summary.total, 3, "3 triple 抽出されるべき");
        assert_eq!(summary.matched, 1);
        assert_eq!(summary.conflicting, 1);
        assert_eq!(summary.unknown, 1);
        assert!((summary.mean_path_len - 1.0).abs() < f64::EPSILON);
    }

    /// Phase 3 Refactor — 空入力で空 summary。
    #[test]
    fn t_run_factcheck_pass_empty_input() {
        let store = setup_db();
        let graph = KnowledgeGraph::new(store.conn());
        let summary = run_factcheck_pass(&[], &graph);
        assert_eq!(summary, FactCheckSummary::default());
    }

    /// Phase 3 Refactor — regex precision: 小文字始まり subject は match しない。
    /// 期待: rule-based pattern が高 precision (低 recall) を維持、false positive 回避。
    /// "alice is the parent of bob" → 空 Vec (大文字始まりが pattern 要件)
    #[test]
    fn t_extract_triples_rejects_lowercase_subject() {
        let triples = extract_triples_from_text("alice is the parent of bob");
        assert!(
            triples.is_empty(),
            "小文字始まり subject は extract されないべき (高 precision 保証)"
        );
    }

    /// Phase 3 Refactor — Pattern 2 ("X is a Y") の独立動作検証。
    /// 期待: "Bonsai-8B is a model" → Triple { Bonsai-8B, is_a, model, confidence: 0.80 }
    #[test]
    fn t_extract_triples_pattern_2_is_a_basic() {
        let triples = extract_triples_from_text("Bonsai-8B is a model");
        assert_eq!(triples.len(), 1, "Pattern 2 で 1 件 extract されるべき");
        assert_eq!(triples[0].subject, "Bonsai-8B");
        assert_eq!(triples[0].predicate, "is_a");
        assert_eq!(triples[0].object, "model");
        assert!((triples[0].confidence - 0.80).abs() < f64::EPSILON);
    }

    /// Phase 3 Refactor — Pattern 1 のみ登録 / Pattern 2 重複なし。
    /// 期待: "Alice is the parent of Bob" は Pattern 1 のみで 1 件 (Pattern 2 "is a" は match しない)
    #[test]
    fn t_extract_triples_no_pattern_overlap() {
        let triples = extract_triples_from_text("Alice is the parent of Bob");
        assert_eq!(
            triples.len(),
            1,
            "Pattern 1 単独 match、Pattern 2 二重 extract なし"
        );
        assert_eq!(triples[0].predicate, "parent_of");
    }

    /// Phase 3 Refactor — subject 既登録 + object 未登録 → Unknown。
    /// 期待: graph に Alice のみ登録、Bob 未登録 → KG path 不在 + conflicting edge 不在 = Unknown
    /// (false positive 回避: object 未登録の場合に Conflict 判定を出さない設計)
    #[test]
    fn t_verify_triple_in_kg_subject_known_object_unknown() {
        let store = setup_db();
        let graph = KnowledgeGraph::new(store.conn());
        let _ = graph.add_node("entity", "Alice").unwrap(); // object 未登録

        let triple = Triple {
            subject: "Alice".into(),
            predicate: "knows".into(),
            object: "UnknownPerson".into(),
            confidence: 1.0,
        };
        let result = verify_triple_in_kg(&triple, &graph);
        assert_eq!(
            result,
            FactCheckResult::Unknown,
            "subject 登録 + predicate 未関連 = Unknown (Conflict ではない)"
        );
    }

    /// Phase 1 Red (Plan A G-4c) — `seed_kg_for_factcheck_lab` で 3 fact が冪等に
    /// KG に投入される。
    /// 起点: `.claude/plan/hallucination-inducing-benchmark-task.md` §4.1
    /// 期待:
    ///   - (Bonsai-8B, parent_of, Qwen3-8B)
    ///   - (prism-ml, is_a, ternary_model)
    ///   - (bonsai-agent, child_of, bonsai-8B)
    ///
    /// 2 回呼出で重複 add_edge が weight 加算で冪等 (UPSERT 仕様)、行数増えない。
    /// Phase 2 Green で `pub fn seed_kg_for_factcheck_lab` 実装後 PASS。
    #[test]
    fn t_seed_kg_for_halluc_tasks_populates_three_facts() {
        let store = setup_db();
        let graph = KnowledgeGraph::new(store.conn());

        seed_kg_for_factcheck_lab(&graph).expect("seed failed");

        assert!(
            graph
                .contains_triple("Bonsai-8B", "parent_of", "Qwen3-8B")
                .is_some(),
            "fact (Bonsai-8B, parent_of, Qwen3-8B) が KG に存在すべき"
        );
        assert!(
            graph
                .contains_triple("Prism-ml", "is_a", "ternary_model")
                .is_some(),
            "fact (Prism-ml, is_a, ternary_model) が KG に存在すべき (大文字始まり)"
        );
        assert!(
            graph
                .contains_triple("Bonsai-Agent", "child_of", "Bonsai-8B")
                .is_some(),
            "fact (Bonsai-Agent, child_of, Bonsai-8B) が KG に存在すべき (大文字始まり)"
        );

        seed_kg_for_factcheck_lab(&graph).expect("second seed failed");
        assert!(
            graph
                .contains_triple("Bonsai-8B", "parent_of", "Qwen3-8B")
                .is_some(),
            "2 回 seed しても fact 検証可能 (冪等)"
        );
    }

    /// Phase 1 Red — env opt-in default OFF。
    /// 期待: 未設定で false (Cerememory 三本柱と同 pattern、項目 217-219)
    #[test]
    fn t_factcheck_env_opt_in_default_off() {
        // SAFETY: 他 test と env 干渉のリスクあり。Phase 2 Green で
        // env mutex guard 化を検討 (heuristics.rs CRITIC_TEST_LOCK と同 pattern)。
        unsafe { std::env::remove_var("BONSAI_KG_FACTCHECK_ENABLED") };
        assert!(!is_factcheck_enabled());

        unsafe { std::env::set_var("BONSAI_KG_FACTCHECK_ENABLED", "1") };
        assert!(is_factcheck_enabled());

        unsafe { std::env::remove_var("BONSAI_KG_FACTCHECK_ENABLED") };
    }
}
