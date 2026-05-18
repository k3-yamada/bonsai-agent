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

/// "X is the Y of Z" pattern (e.g., "Alice is the parent of Bob", "Bonsai-8B is the parent of Llama-3")。
/// 大文字始まり英単語の subject/object、小文字 predicate を要件とする保守的 pattern。
/// LLM 出力の典型的英語表現に focus、日本語混在は Phase 5 別 plan。
/// 項目 239 (Pattern 1 regex dash 対応): subject/object に dash (`-`) を許容、Pattern 2 (`RE_IS_A`) と統一。
/// 起源: 項目 236 G-5b + 項目 237 G-6b 副次 finding = hallucination task の dash entity
/// ("Bonsai-8B" 等) が旧 regex で reject されて extraction recall を不当に下げていた。
static RE_IS_THE_OF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b([A-Z][A-Za-z0-9_\-]*)\s+is\s+the\s+([a-z][a-z_]*)\s+of\s+([A-Z][A-Za-z0-9_\-]*)",
    )
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

/// 項目 235 候補: 全 trajectory 拡張モードの env opt-in 判定
/// (`BONSAI_FACTCHECK_ALL_TRAJECTORIES=1`、default OFF)。
///
/// Plan A G-4c v1/v2 反証 finding (項目 234) = halluc task は SUCCESS-by-design
/// (tool_success_rate=1.0 + total_steps<2) で `extract_failed_trajectories_since_id`
/// から構造的排除される問題への対応。本 env ON 時、`run_factcheck_pass_lab` は
/// failed + successful trajectory を chain で集計 + min_steps=0 で halluc 0-tool
/// session も対象化する (`.claude/plan/factcheck-trajectory-scope-expansion.md` §2.1)。
///
/// production default OFF (env unset で従来 failed-only / min_steps=2 完全互換)、
/// Lab v20 effectiveness 検証 (Pearson r ≥ 0.3 paired t-test) の起動前提として
/// ON 化、production agent_loop は本 path に到達しない。
pub fn is_all_trajectories_enabled() -> bool {
    std::env::var("BONSAI_FACTCHECK_ALL_TRAJECTORIES")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// テスト用 env mutex (`BONSAI_FACTCHECK_ALL_TRAJECTORIES` 操作の cross-file 排他)。
///
/// factcheck.rs `t_factcheck_all_trajectories_env_opt_in_default_off` と
/// experiment.rs `t_factcheck_*` 系 test の両方が同 env を mutate するため、
/// crate-level の単一 mutex で serialize する。
/// 同 pattern: 項目 226 CRITIC_TEST_LOCK / 項目 229 FRONTIER_TEST_LOCK (file-local 単独)。
#[cfg(test)]
pub(crate) static FACTCHECK_ALL_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Plan A G-4c + 項目 242 Lab v21 用に KG に 8 fact を投入する seed 関数
/// (Lab cycle 開始時のみ呼出)。
///
/// 構成 (8 fact = 3 halluc + 5 success):
///
/// 【halluc 3 fact】benchmark.rs:halluc_* task 3 件の正解 fact。
/// LLM が捏造 (false fact) すれば `verify_triple_in_kg` が `Conflict` 判定を出す
/// 経路を確立 (Lab v20 で conf=3 deterministic 確証済)。
///   - (Bonsai-8B, parent_of, Qwen3-8B) — halluc_parent_of_false_fact 正解
///   - (Prism-ml, is_a, ternary_model) — halluc_is_a_false_type 正解
///   - (Bonsai-Agent, child_of, Bonsai-8B) — halluc_t2_file_context_misalign 正解
///     (file fixture `/tmp/bonsai_halluc_ctx.txt` と integrity 一致)
///
/// 【success_fact 5 fact】benchmark.rs:success_* task 5 件の正解 fact (項目 242)。
/// LLM が正しく答えれば `Match` 判定 = matched>0 で `(conf+unk)/total < 1.0` の
/// variance 復活 → Pearson r 計算可能化 (Lab v20 structural finding 解消)。
///   - (Bonsai-Agent, is_a, rust_project) — Pattern 2 (is a)
///   - (Llama-server, runtime_of, Bonsai-Agent) — Pattern 1 (is the X of Y)
///   - (Sqlite, storage_of, Bonsai-Agent) — Pattern 1
///   - (Reflexion, loop_of, Bonsai-Agent) — Pattern 1
///   - (Path-Guard, sandbox_of, Bonsai-Agent) — Pattern 1 (dash subject)
///
/// 冪等 (add_node / add_edge は UPSERT、再 seed で weight 加算のみ、行数増えない)。
/// 全 subject/object は大文字始まり (Pattern 1/2 regex match 経路確保、項目 239 dash 対応済)。
///
/// 呼出元: `experiment.rs::run_factcheck_pass_lab` 内、env-gated 経路で 1 度のみ。
/// production agent_loop は本 fn を呼ばない (`is_factcheck_enabled()` で OFF 時短絡)。
pub fn seed_kg_for_factcheck_lab(kg: &KnowledgeGraph<'_>) -> anyhow::Result<()> {
    let facts: &[(&str, &str, &str)] = &[
        // halluc seed (Plan A G-4c)
        ("Bonsai-8B", "parent_of", "Qwen3-8B"),
        ("Prism-ml", "is_a", "ternary_model"),
        ("Bonsai-Agent", "child_of", "Bonsai-8B"),
        // success_fact seed (項目 242 Lab v21)
        ("Bonsai-Agent", "is_a", "rust_project"),
        ("Llama-server", "runtime_of", "Bonsai-Agent"),
        ("Sqlite", "storage_of", "Bonsai-Agent"),
        ("Reflexion", "loop_of", "Bonsai-Agent"),
        ("Path-Guard", "sandbox_of", "Bonsai-Agent"),
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

    /// 項目 239 候補 (Pattern 1 regex dash 対応) — subject に dash 含む場合の Pattern 1 抽出。
    /// 期待: "Bonsai-8B is the parent of Llama" → Triple { Bonsai-8B, parent_of, Llama, 0.85 }
    /// 起源: 項目 236 G-5b + 項目 237 G-6b で hallucination task の dash entity ("Bonsai-8B" 等) が
    /// Pattern 1 (`RE_IS_THE_OF` `[A-Z][A-Za-z0-9_]*`) で reject される問題 (Pattern 2 `RE_IS_A` は
    /// `[A-Za-z0-9_\-]*` で dash 許容済)、本 fix で extraction recall 向上を Lab v20+ で計測。
    #[test]
    fn t_extract_triples_pattern_1_subject_with_dash() {
        let triples = extract_triples_from_text("Bonsai-8B is the parent of Llama");
        assert_eq!(
            triples.len(),
            1,
            "dash subject Pattern 1 で 1 件 extract されるべき"
        );
        assert_eq!(triples[0].subject, "Bonsai-8B");
        assert_eq!(triples[0].predicate, "parent_of");
        assert_eq!(triples[0].object, "Llama");
        assert!((triples[0].confidence - 0.85).abs() < f64::EPSILON);
    }

    /// 項目 239 候補 (Pattern 1 regex dash 対応) — object に dash 含む場合の Pattern 1 抽出。
    /// 期待: "Alice is the parent of Bob-Junior" → Triple { Alice, parent_of, Bob-Junior, 0.85 }
    #[test]
    fn t_extract_triples_pattern_1_object_with_dash() {
        let triples = extract_triples_from_text("Alice is the parent of Bob-Junior");
        assert_eq!(
            triples.len(),
            1,
            "dash object Pattern 1 で 1 件 extract されるべき"
        );
        assert_eq!(triples[0].subject, "Alice");
        assert_eq!(triples[0].predicate, "parent_of");
        assert_eq!(triples[0].object, "Bob-Junior");
    }

    /// 項目 239 候補 (Pattern 1 regex dash 対応) — subject + object 両方 dash の Pattern 1 抽出。
    /// 期待: "Bonsai-8B is the parent of Llama-3" → Triple { Bonsai-8B, parent_of, Llama-3, 0.85 }
    #[test]
    fn t_extract_triples_pattern_1_both_with_dash() {
        let triples = extract_triples_from_text("Bonsai-8B is the parent of Llama-3");
        assert_eq!(
            triples.len(),
            1,
            "両側 dash Pattern 1 で 1 件 extract されるべき"
        );
        assert_eq!(triples[0].subject, "Bonsai-8B");
        assert_eq!(triples[0].predicate, "parent_of");
        assert_eq!(triples[0].object, "Llama-3");
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

    /// 項目 242 Phase 1 Red (Lab v21 KG seed 拡張) — `seed_kg_for_factcheck_lab` で
    /// success_fact task 用 5 fact が追加される。Phase 2 Green まで Red。
    /// 起点: `.claude/plan/lab-v21-kg-seed-expansion.md` §2.1 (案 A)
    /// 期待: 既存 3 halluc fact に加え、以下 5 fact が KG に存在:
    ///   - (Bonsai-Agent, is_a, rust_project)            ← Pattern 2 (is a)
    ///   - (Llama-server, runtime_of, Bonsai-Agent)      ← Pattern 1 (is the X of Y)
    ///   - (Sqlite, storage_of, Bonsai-Agent)            ← Pattern 1
    ///   - (Reflexion, loop_of, Bonsai-Agent)            ← Pattern 1
    ///   - (Path-Guard, sandbox_of, Bonsai-Agent)        ← Pattern 1 (dash subject)
    #[test]
    fn t_seed_kg_for_factcheck_lab_contains_5_success_facts() {
        let store = setup_db();
        let graph = KnowledgeGraph::new(store.conn());

        seed_kg_for_factcheck_lab(&graph).expect("seed failed");

        let success_facts: &[(&str, &str, &str)] = &[
            ("Bonsai-Agent", "is_a", "rust_project"),
            ("Llama-server", "runtime_of", "Bonsai-Agent"),
            ("Sqlite", "storage_of", "Bonsai-Agent"),
            ("Reflexion", "loop_of", "Bonsai-Agent"),
            ("Path-Guard", "sandbox_of", "Bonsai-Agent"),
        ];
        for (s, p, o) in success_facts {
            assert!(
                graph.contains_triple(s, p, o).is_some(),
                "success_fact ({s}, {p}, {o}) が KG に存在すべき (項目 242 Phase 2 Green 待ち)"
            );
        }
    }

    /// 項目 242 Phase 1 Red — 既存 3 halluc fact が `seed_kg_for_factcheck_lab` で
    /// 維持される (additive extension 不変、backward compat 保証)。
    /// 既存 `t_seed_kg_for_halluc_tasks_populates_three_facts` と独立に halluc seed
    /// 維持を検証 (success_fact 追加で halluc seed が消失しないことを保証)。
    #[test]
    fn t_seed_kg_for_factcheck_lab_preserves_3_halluc_facts() {
        let store = setup_db();
        let graph = KnowledgeGraph::new(store.conn());

        seed_kg_for_factcheck_lab(&graph).expect("seed failed");

        let halluc_facts: &[(&str, &str, &str)] = &[
            ("Bonsai-8B", "parent_of", "Qwen3-8B"),
            ("Prism-ml", "is_a", "ternary_model"),
            ("Bonsai-Agent", "child_of", "Bonsai-8B"),
        ];
        for (s, p, o) in halluc_facts {
            assert!(
                graph.contains_triple(s, p, o).is_some(),
                "halluc seed ({s}, {p}, {o}) は項目 242 後も維持されるべき"
            );
        }
    }

    /// 項目 242 Phase 1 Red — Pattern 2 (is a) と Pattern 1 (is the X of Y) の両軸を
    /// success_fact seed が cover する (variance 確保条件)。
    /// 期待: Pattern 2 fact >= 1 件、Pattern 1 fact >= 1 件 (両 regex 経路で matched 発火可能)。
    #[test]
    fn t_seed_kg_for_factcheck_lab_covers_both_regex_patterns() {
        let store = setup_db();
        let graph = KnowledgeGraph::new(store.conn());

        seed_kg_for_factcheck_lab(&graph).expect("seed failed");

        // Pattern 2 (is a) representative
        assert!(
            graph
                .contains_triple("Bonsai-Agent", "is_a", "rust_project")
                .is_some(),
            "Pattern 2 (is_a) seed fact が必要 (項目 242 success_bonsai_is_a_rust_project task 対応)"
        );
        // Pattern 1 (is the X of Y) representative
        assert!(
            graph
                .contains_triple("Llama-server", "runtime_of", "Bonsai-Agent")
                .is_some(),
            "Pattern 1 (runtime_of) seed fact が必要 (項目 242 success_llama_runtime_of_bonsai task 対応)"
        );
    }

    /// 項目 235 — `BONSAI_FACTCHECK_ALL_TRAJECTORIES` env opt-in default OFF。
    /// 期待: 未設定で false、`"1"` / `"true"` (case-insensitive) で true。
    /// Cerememory 三本柱 (217-219) + 既存 `t_factcheck_env_opt_in_default_off` と同 pattern。
    ///
    /// 注: experiment.rs `t_factcheck_*` 系 test と同 env を共有するため
    /// `FACTCHECK_ALL_ENV_TEST_LOCK` で cross-file serialize。
    #[test]
    fn t_factcheck_all_trajectories_env_opt_in_default_off() {
        let _g = FACTCHECK_ALL_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var("BONSAI_FACTCHECK_ALL_TRAJECTORIES") };
        assert!(!is_all_trajectories_enabled(), "未設定で false");

        unsafe { std::env::set_var("BONSAI_FACTCHECK_ALL_TRAJECTORIES", "1") };
        assert!(is_all_trajectories_enabled(), "\"1\" で true");

        unsafe { std::env::set_var("BONSAI_FACTCHECK_ALL_TRAJECTORIES", "TRUE") };
        assert!(
            is_all_trajectories_enabled(),
            "case-insensitive \"TRUE\" で true"
        );

        unsafe { std::env::set_var("BONSAI_FACTCHECK_ALL_TRAJECTORIES", "0") };
        assert!(!is_all_trajectories_enabled(), "\"0\" で false");

        unsafe { std::env::remove_var("BONSAI_FACTCHECK_ALL_TRAJECTORIES") };
    }

    // ── 項目 244 Phase 1 Red — KG Lint Coverage Check (LLM Wiki Lint パターン適用) ──
    //
    // 起点: .claude/plan/kg-lint-coverage-check.md §3 Phase 1 (Red) 6 failing tests
    //
    // 設計: `LintReport` 構造体 (conflicting_triples / orphan_nodes / uncovered_seed_triples
    //       / case_variant_nodes 4 軸) + `lint_kg_for_lab(kg, seed_triples, keyword_bundles)`
    //       で KG の整合性 lint pass。Phase 2 Green で実装、Phase 4 smoke G-8a/b/c で配線。
    //
    // 期待効果: Lab v20 structural finding (matched=0 deterministic) を <1 sec で事前検出、
    //           19h 投下 blunder 防止。

    /// Phase 1 Red — 矛盾 triple 検出。
    /// KG に (A->B, A->C) 同一 predicate seed 後 lint で conflicting_triples.len() >= 1。
    /// Phase 2 Green で `lint_kg_for_lab` 実装後 PASS 化。
    #[test]
    fn t_lint_kg_detects_conflicting_triples() {
        let store = setup_db();
        let kg = KnowledgeGraph::new(store.conn());
        let alice = kg.add_node("entity", "Alice").unwrap();
        let bob = kg.add_node("entity", "Bob").unwrap();
        let charlie = kg.add_node("entity", "Charlie").unwrap();
        kg.add_edge(alice, bob, "parent_of", 1.0).unwrap();
        kg.add_edge(alice, charlie, "parent_of", 1.0).unwrap();

        let report = lint_kg_for_lab(&kg, &[], &[]);
        assert!(
            !report.conflicting_triples.is_empty(),
            "Alice→Bob と Alice→Charlie の parent_of 矛盾を検出すべき"
        );
    }

    /// Phase 1 Red — orphan node 検出。
    /// edge 持たない node を add_node 後 lint で orphan_nodes に含む。
    /// Phase 2 Green で graph.rs::orphan_nodes() helper 追加 + lint 集約。
    #[test]
    fn t_lint_kg_detects_orphan_nodes() {
        let store = setup_db();
        let kg = KnowledgeGraph::new(store.conn());
        let _orphan = kg.add_node("entity", "OrphanNode").unwrap();
        let a = kg.add_node("entity", "EdgeA").unwrap();
        let b = kg.add_node("entity", "EdgeB").unwrap();
        kg.add_edge(a, b, "related_to", 1.0).unwrap();

        let report = lint_kg_for_lab(&kg, &[], &[]);
        assert!(
            report.orphan_nodes.iter().any(|n| n == "OrphanNode"),
            "OrphanNode は orphan_nodes に含まれるべき: got {:?}",
            report.orphan_nodes
        );
    }

    /// Phase 1 Red — uncovered seed triple 検出。
    /// seed triple のうち benchmark task expected_keywords で hit しないものを検出。
    /// 「hit する」= triple の subject/predicate/object のいずれかが keyword bundle に含む。
    /// Phase 2 Green で word-level coverage check 実装。
    #[test]
    fn t_lint_kg_detects_uncovered_seed_triples() {
        let store = setup_db();
        let kg = KnowledgeGraph::new(store.conn());
        let seed_triples = vec![
            Triple {
                subject: "Foo".into(),
                predicate: "is_a".into(),
                object: "Bar".into(),
                confidence: 1.0,
            },
            Triple {
                subject: "OrphanFact".into(),
                predicate: "covers".into(),
                object: "Nothing".into(),
                confidence: 1.0,
            },
        ];
        // 1 つ目の triple は keyword "Foo" で hit、2 つ目は hit しない (uncovered)
        let keyword_bundles = vec![vec!["Foo".to_string(), "is_a".to_string()]];

        let report = lint_kg_for_lab(&kg, &seed_triples, &keyword_bundles);
        assert!(
            report
                .uncovered_seed_triples
                .iter()
                .any(|t| t.subject == "OrphanFact"),
            "OrphanFact triple は uncovered_seed_triples に含まれるべき"
        );
    }

    /// Phase 1 Red — case variant 検出。
    /// "Bonsai-Agent" と "bonsai-agent" 両方 KG に存在で variant pair 検出。
    /// Phase 2 Green で graph.rs::case_variant_pairs() helper 追加。
    #[test]
    fn t_lint_kg_detects_case_variant_nodes() {
        let store = setup_db();
        let kg = KnowledgeGraph::new(store.conn());
        kg.add_node("entity", "Bonsai-Agent").unwrap();
        kg.add_node("entity", "bonsai-agent").unwrap();
        kg.add_node("entity", "UniqueName").unwrap();

        let report = lint_kg_for_lab(&kg, &[], &[]);
        assert!(
            !report.case_variant_nodes.is_empty(),
            "Bonsai-Agent / bonsai-agent の case variant を検出すべき"
        );
    }

    /// Phase 1 Red — empty KG は clean。
    /// 空 KG + 空 seed + 空 keywords で is_clean()=true (false positive 防止)。
    /// Phase 2 Green で全 4 軸の empty check 実装。
    #[test]
    fn t_lint_report_is_clean_returns_true_for_empty_kg() {
        let store = setup_db();
        let kg = KnowledgeGraph::new(store.conn());
        let report = lint_kg_for_lab(&kg, &[], &[]);
        assert!(
            report.is_clean(),
            "空 KG は clean であるべき: got conflicting={} orphan={} uncovered={} case_variant={}",
            report.conflicting_triples.len(),
            report.orphan_nodes.len(),
            report.uncovered_seed_triples.len(),
            report.case_variant_nodes.len()
        );
    }

    /// Phase 1 Red — regression gate: seed_kg_for_factcheck_lab は clean を維持。
    /// 現行 8 fact seed + smoke 15 task expected_keywords で clean=true 必須。
    /// Lab v21 paired 起動前の sanity check として機能 (項目 244 推奨運用 protocol)。
    /// Phase 2 Green で coverage logic 実装後 PASS、Phase 5 Lab v21+ で前提条件化。
    #[test]
    fn t_lint_seed_kg_for_factcheck_lab_is_clean_with_smoke_keywords() {
        let store = setup_db();
        let kg = KnowledgeGraph::new(store.conn());
        seed_kg_for_factcheck_lab(&kg).expect("seed failed");

        // smoke 15 task expected_keywords の subject/object 全 8 entity を cover
        // (Pattern 1/2 regex match 経路と整合)。本 list は benchmark.rs::SMOKE_TASK_IDS
        // 順序と対応、項目 242/243 の expected_keywords を mirror。
        let seed_triples = vec![
            Triple {
                subject: "Bonsai-8B".into(),
                predicate: "parent_of".into(),
                object: "Qwen3-8B".into(),
                confidence: 1.0,
            },
            Triple {
                subject: "Prism-ml".into(),
                predicate: "is_a".into(),
                object: "ternary_model".into(),
                confidence: 1.0,
            },
            Triple {
                subject: "Bonsai-Agent".into(),
                predicate: "child_of".into(),
                object: "Bonsai-8B".into(),
                confidence: 1.0,
            },
            Triple {
                subject: "Bonsai-Agent".into(),
                predicate: "is_a".into(),
                object: "rust_project".into(),
                confidence: 1.0,
            },
            Triple {
                subject: "Llama-server".into(),
                predicate: "runtime_of".into(),
                object: "Bonsai-Agent".into(),
                confidence: 1.0,
            },
            Triple {
                subject: "Sqlite".into(),
                predicate: "storage_of".into(),
                object: "Bonsai-Agent".into(),
                confidence: 1.0,
            },
            Triple {
                subject: "Reflexion".into(),
                predicate: "loop_of".into(),
                object: "Bonsai-Agent".into(),
                confidence: 1.0,
            },
            Triple {
                subject: "Path-Guard".into(),
                predicate: "sandbox_of".into(),
                object: "Bonsai-Agent".into(),
                confidence: 1.0,
            },
        ];
        let keyword_bundles: Vec<Vec<String>> = vec![
            // halluc 3 task
            vec!["Bonsai-8B".into(), "parent_of".into(), "Qwen3-8B".into()],
            vec!["Prism-ml".into(), "is_a".into(), "ternary_model".into()],
            vec![
                "Bonsai-Agent".into(),
                "child_of".into(),
                "Bonsai-8B".into(),
            ],
            // success_fact 5 task (項目 242)
            vec![
                "Bonsai-Agent".into(),
                "is_a".into(),
                "rust_project".into(),
            ],
            vec![
                "Llama-server".into(),
                "runtime_of".into(),
                "Bonsai-Agent".into(),
            ],
            vec![
                "Sqlite".into(),
                "storage_of".into(),
                "Bonsai-Agent".into(),
            ],
            vec![
                "Reflexion".into(),
                "loop_of".into(),
                "Bonsai-Agent".into(),
            ],
            vec![
                "Path-Guard".into(),
                "sandbox_of".into(),
                "Bonsai-Agent".into(),
            ],
        ];

        let report = lint_kg_for_lab(&kg, &seed_triples, &keyword_bundles);
        assert!(
            report.is_clean(),
            "現行 seed + smoke keywords は clean であるべき (Lab v21 paired 起動前 sanity gate): \
             conflicting={} orphan={} uncovered={} case_variant={}",
            report.conflicting_triples.len(),
            report.orphan_nodes.len(),
            report.uncovered_seed_triples.len(),
            report.case_variant_nodes.len()
        );
    }
}
