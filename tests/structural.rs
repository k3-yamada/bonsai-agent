//! Structural lint tests (Z-4 layer linter、項目 256 候補).
//!
//! Zenn dragon1208/66547a030c0236 Step 4 模倣の 4 軸 lint:
//! - SIZE-001: 800 行超過 (CLAUDE.md 慣例)
//! - DEP-001: module layer 順違反 (docs/architecture/module-layer-rules.md)
//! - LOG-001: production code で eprintln! 直接使用 (log_event 経由が原則、項目 248 critic F1)
//! - META: 上記 lint の panic message に修正方法 docs link 含有確証 (記事 Step 4 核心)
//!
//! TDD strict Phase 1 Red: whitelist 空、既存 violation で FAIL.
//! Phase 2 Green: whitelist 追加 + lint logic 完備で PASS、新規 violation 注入で fail catch.

use std::fs;
use std::path::{Path, PathBuf};

const LAYER_ORDER: &[&str] = &[
    "db",
    "observability",
    "safety",
    "memory",
    "knowledge",
    "runtime",
    "tools",
    "agent",
    "main",
];

const SIZE_LIMIT: usize = 800;

/// SIZE-001 whitelist — Phase 2 Green で既存 violation 20 件を全件許容.
/// path string substr match. 各 file は今後の Z-4 follow-up plan で
/// 分割検討対象 (項目 248 Phase 5 axis prune と並列の compaction 分割含む).
const WHITELIST_OVER_800: &[&str] = &[
    "src/tools/repomap.rs",
    "src/tools/mod.rs",
    "src/tools/file.rs",
    "src/memory/heuristics.rs",
    "src/memory/store.rs",
    "src/memory/factcheck.rs",
    "src/config.rs",
    "src/runtime/llama_server.rs",
    "src/runtime/model_router.rs",
    "src/agent/benchmark.rs",
    "src/agent/subagent.rs",
    "src/agent/experiment_log.rs",
    "src/agent/agent_loop/tests.rs",
    "src/agent/middleware.rs",
    "src/agent/compaction.rs",
    "src/agent/error_recovery.rs",
    "src/agent/event_store.rs",
    "src/agent/experiment.rs",
    "src/observability/audit.rs",
    "src/main.rs",
];

/// DEP-001 whitelist — Phase 1 Red baseline で検出された 32 件を許容.
/// (path, current_layer, imported_layer) tuple、現状は type-only import や
/// cross-cutting reference として legitimate. 真の防御は逆方向 function call で、
/// type read だけは layer 順を弱めて許容. follow-up plan で個別 audit 推奨.
const WHITELIST_DEP: &[(&str, &str, &str)] = &[
    (
        "src/memory/mocks/event_repository_mock.rs",
        "memory",
        "agent",
    ),
    ("src/memory/experience.rs", "memory", "agent"),
    ("src/memory/heuristics.rs", "memory", "agent"),
    ("src/memory/heuristics.rs", "memory", "runtime"),
    ("src/memory/store.rs", "memory", "runtime"),
    ("src/memory/store.rs", "memory", "agent"),
    ("src/memory/skill.rs", "memory", "agent"),
    ("src/memory/search.rs", "memory", "runtime"),
    ("src/runtime/cache.rs", "runtime", "agent"),
    ("src/runtime/cache.rs", "runtime", "tools"),
    ("src/runtime/llama_server.rs", "runtime", "agent"),
    ("src/runtime/llama_server.rs", "runtime", "tools"),
    ("src/runtime/model_router.rs", "runtime", "agent"),
    ("src/runtime/inference.rs", "runtime", "agent"),
    ("src/runtime/inference.rs", "runtime", "tools"),
    ("src/observability/audit.rs", "observability", "memory"),
];

/// LOG-001 whitelist — Phase 2 Green で operator visibility 用途を許容.
/// path string substr match. 一部 (advisor_inject.rs) は log_event 化候補で
/// follow-up plan の TODO.
const WHITELIST_EPRINTLN: &[&str] = &[
    "src/main.rs",                            // CLI 出力、operator visibility
    "src/bin/longmemeval_bench.rs",           // bench CLI 出力
    "src/agent/experiment.rs",                // Lab 進捗 (log_event 化検討中)
    "src/agent/agent_loop/advisor_inject.rs", // log_event 化候補 (follow-up TODO)
    "src/agent/agent_loop/step.rs",           // step 進捗
    "src/agent/context_inject.rs",            // context 注入 trace
    "src/runtime/llama_server.rs",            // server log
    "src/runtime/embedder.rs",                // embed log
    "src/observability/logger.rs",            // logger 内部 (log_event implementation)
    "src/safety/secrets.rs",                  // security warning
    "src/knowledge/vault_lint.rs",            // 項目 246 implementation の意図的 eprintln
    "src/memory/store.rs",                    // memory store warning
    "src/eval/longmemeval/runner.rs",         // longmemeval runner trace
];

/// src/ 配下の全 .rs ファイルを再帰収集.
fn walk_src() -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk_dir(Path::new("src"), &mut files);
    files
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn count_lines(path: &Path) -> usize {
    fs::read_to_string(path).unwrap_or_default().lines().count()
}

/// "src/<module>/..." から最上位 module 名を抽出.
fn module_of(path: &Path) -> Option<String> {
    let components: Vec<_> = path.components().collect();
    if components.len() < 2 {
        return None;
    }
    components
        .get(1)
        .map(|c| c.as_os_str().to_string_lossy().to_string())
}

fn layer_index(module: &str) -> Option<usize> {
    LAYER_ORDER.iter().position(|&m| m == module)
}

/// `use crate::<module>::` 形式の import を全件抽出 (string→module 列).
fn extract_use_crate_modules(content: &str) -> Vec<String> {
    let mut modules = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("use crate::") {
            // "use crate::agent::experiment::*;" → "agent"
            let module = rest
                .split(|c: char| c == ':' || c == ';' || c == ' ' || c == ',' || c == '{')
                .next()
                .unwrap_or("");
            if !module.is_empty() {
                modules.push(module.to_string());
            }
        }
    }
    modules
}

#[test]
fn t_no_new_src_file_over_800_lines() {
    let violations: Vec<(PathBuf, usize)> = walk_src()
        .into_iter()
        .filter_map(|path| {
            let lines = count_lines(&path);
            if lines > SIZE_LIMIT
                && !WHITELIST_OVER_800
                    .iter()
                    .any(|w| path.to_string_lossy().contains(w))
            {
                Some((path, lines))
            } else {
                None
            }
        })
        .collect();
    assert!(
        violations.is_empty(),
        "[LINT:SIZE-001] {} 件のファイルが {} 行超過. 修正方法: 機能を sub-module に分割するか whitelist 追加. 参照: docs/architecture/module-layer-rules.md\nViolations: {:?}",
        violations.len(),
        SIZE_LIMIT,
        violations
    );
}

#[test]
fn t_layer_order_no_upward_dep() {
    let mut violations: Vec<(PathBuf, String, String)> = Vec::new();
    for path in walk_src() {
        let Some(current_mod) = module_of(&path) else {
            continue;
        };
        let Some(current_idx) = layer_index(&current_mod) else {
            continue; // main.rs や lib.rs は対象外
        };
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for imported_mod in extract_use_crate_modules(&content) {
            // self への参照は除外
            if imported_mod == current_mod {
                continue;
            }
            let Some(imported_idx) = layer_index(&imported_mod) else {
                continue; // cross-cutting (cancel, config) は LAYER_ORDER 外
            };
            if imported_idx > current_idx {
                // Phase 2 Green: whitelist 適用 (path, current, imported) tuple match.
                let path_str = path.to_string_lossy().to_string();
                let whitelisted = WHITELIST_DEP.iter().any(|(p, c, i)| {
                    path_str == *p && c == &current_mod.as_str() && i == &imported_mod.as_str()
                });
                if !whitelisted {
                    violations.push((path.clone(), current_mod.clone(), imported_mod.clone()));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "[LINT:DEP-001] {} 件の layer 違反 (whitelist 外). 上位 layer への依存は禁止. 修正方法: 該当機能を下位 layer に再 implement、or cross-cutting concern なら cancel/config に移行、or 一時的に WHITELIST_DEP に追加. 参照: docs/architecture/module-layer-rules.md\nViolations: {:?}",
        violations.len(),
        violations
    );
}

#[test]
fn t_no_eprintln_in_production() {
    let mut violations: Vec<(PathBuf, usize)> = Vec::new();
    for path in walk_src() {
        let path_str = path.to_string_lossy().to_string();
        if WHITELIST_EPRINTLN.iter().any(|w| path_str.contains(w)) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        // 簡易検出: cfg(test) より前の eprintln! を検出
        let mut in_test = false;
        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            // #[cfg(test)] / mod tests スコープに入ったら以降は test fixture 扱い
            if trimmed.starts_with("#[cfg(test)]")
                || trimmed.starts_with("#[test]")
                || trimmed.starts_with("mod tests")
            {
                in_test = true;
            }
            if !in_test && trimmed.contains("eprintln!") {
                violations.push((path.clone(), i + 1));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "[LINT:LOG-001] {} 件の eprintln! production 使用. 修正方法: log_event(LogLevel::*, category, msg) 経由に置換、or operator visibility 用途なら whitelist 追加. 参照: docs/architecture/module-layer-rules.md\nViolations: {:?}",
        violations.len(),
        violations
    );
}

/// `[LINT:<UPPERCASE>` pattern を含む source line のみ count (code 自体の `[LINT:` 言及を除外).
/// META meta-test の self-reference false positive 回避.
fn contains_lint_code(line: &str) -> bool {
    line.find("[LINT:").is_some_and(|i| {
        line.as_bytes()
            .get(i + 6)
            .is_some_and(|c| c.is_ascii_uppercase())
    })
}

#[test]
fn t_lint_error_messages_include_docs_link() {
    // meta-test: 本 file の panic message を read して `docs/` link が含まれているか.
    let content = fs::read_to_string("tests/structural.rs").expect("read self");
    let mut lint_codes = Vec::new();
    let mut lint_codes_with_docs = Vec::new();
    for line in content.lines() {
        if contains_lint_code(line) {
            lint_codes.push(line.to_string());
            if line.contains("docs/") {
                lint_codes_with_docs.push(line.to_string());
            }
        }
    }
    assert!(
        !lint_codes.is_empty(),
        "[LINT:META] panic message に [LINT:CODE] 形式が 1 件以上含まれること. 参照: docs/architecture/module-layer-rules.md"
    );
    assert_eq!(
        lint_codes.len(),
        lint_codes_with_docs.len(),
        "[LINT:META] 全 [LINT:CODE] panic message に docs/ link 必須. 修正方法: panic! や assert! の format に '参照: docs/architecture/...' を追加. 参照: docs/architecture/module-layer-rules.md\n全 [LINT:CODE]: {}, docs 含む: {}",
        lint_codes.len(),
        lint_codes_with_docs.len()
    );
}
