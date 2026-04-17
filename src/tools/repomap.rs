use crate::tools::permission::Permission;
use crate::tools::{Tool, ToolResult};
use anyhow::Result;
use regex::Regex;
use std::path::{Path, PathBuf};

pub struct RepoMapTool;

impl Tool for RepoMapTool {
    fn name(&self) -> &str {
        "repo_map"
    }
    fn description(&self) -> &str {
        "コード構造を要約。"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string"}}})
    }
    fn permission(&self) -> Permission {
        Permission::Auto
    }
    fn is_read_only(&self) -> bool {
        true
    }
    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let p = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        Ok(ToolResult {
            output: gen_map(Path::new(p), 3)?,
            success: true,
        })
    }
}

pub fn gen_map(root: &Path, depth: usize) -> Result<String> {
    let mut out = String::new();
    let mut files = Vec::new();
    collect(root, depth, 0, &mut files);
    files.sort();
    for f in &files {
        let rel = f.strip_prefix(root).unwrap_or(f);
        let syms = extract_syms(f);
        if syms.is_empty() {
            out.push_str(&format!("{}\n", rel.display()));
        } else {
            out.push_str(&format!("{}:\n", rel.display()));
            for s in &syms {
                out.push_str(&format!("  {s}\n"));
            }
        }
    }
    if out.is_empty() {
        out = "(no source files)\n".into();
    }
    Ok(out)
}

/// 対応拡張子一覧
const SUPPORTED_EXTS: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "go", "java", "c", "cpp", "h",
    "kt", "swift",
];

fn collect(dir: &Path, mx: usize, cur: usize, files: &mut Vec<PathBuf>) {
    if cur > mx {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if matches!(
                n.as_str(),
                "target"
                    | "node_modules"
                    | ".git"
                    | ".venv"
                    | "__pycache__"
                    | "vendor"
                    | "build"
                    | "dist"
            ) {
                continue;
            }
            collect(&p, mx, cur + 1, files);
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str())
            && SUPPORTED_EXTS.contains(&ext)
        {
            files.push(p);
        }
    }
}

/// シンボル抽出用パターン定義
fn symbol_patterns(ext: &str) -> Vec<&'static str> {
    match ext {
        "rs" => vec![
            r"pub(\(crate\))?\s+(async\s+)?fn\s+\w+",
            r"pub(\(crate\))?\s+struct\s+\w+",
            r"pub(\(crate\))?\s+enum\s+\w+",
            r"pub(\(crate\))?\s+trait\s+\w+",
            r"pub(\(crate\))?\s+(const|static)\s+\w+",
            r"pub\s+type\s+\w+",
            r"pub\s+mod\s+\w+",
            r"impl(<[^>]+>)?\s+\w+",
        ],
        "py" => vec![
            r"(async\s+)?def\s+\w+\s*\(",
            r"class\s+\w+",
        ],
        "ts" | "tsx" | "js" => vec![
            r"(export\s+)?(async\s+)?function\s+\w+",
            r"(export\s+)?class\s+\w+",
            r"(export\s+)?(interface|type)\s+\w+",
        ],
        "go" => vec![
            r"func\s+(\(\w+\s+\*?\w+\)\s+)?\w+",
            r"type\s+\w+\s+(struct|interface)",
        ],
        "java" => vec![
            r"(public\s+|private\s+|protected\s+)?(static\s+)?class\s+\w+",
            r"(public\s+|private\s+|protected\s+)?(static\s+)?\w+\s+\w+\s*\(",
        ],
        "c" | "cpp" | "h" => vec![
            r"(typedef\s+)?struct\s+\w+",
            r"(typedef\s+)?enum\s+\w+",
            r"\w+\s+\w+\s*\([^)]*\)\s*[;{]",
        ],
        "kt" => vec![
            r"(private\s+|internal\s+|protected\s+)?(suspend\s+)?fun\s+\w+",
            r"(data\s+|sealed\s+|abstract\s+|open\s+)?class\s+\w+",
            r"(object|interface|enum\s+class)\s+\w+",
        ],
        "swift" => vec![
            r"(public\s+|private\s+|internal\s+|open\s+)?(static\s+|class\s+)?func\s+\w+",
            r"(public\s+|private\s+|internal\s+|open\s+)?(final\s+)?class\s+\w+",
            r"(struct|enum|protocol|actor)\s+\w+",
        ],
        _ => vec![],
    }
}

/// ファイルからシンボルを抽出（行番号付き）
fn extract_syms(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let pats = symbol_patterns(ext);
    if pats.is_empty() {
        return vec![];
    }

    // 行番号マップ構築: byte_offset → line_number
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(content.match_indices('\n').map(|(i, _)| i + 1))
        .collect();

    let byte_to_line = |offset: usize| -> usize {
        match line_starts.binary_search(&offset) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    };

    let mut syms = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in &pats {
        if let Ok(re) = Regex::new(p) {
            for m in re.find_iter(&content) {
                let raw = m.as_str().trim();
                let s = if raw.len() > 80 {
                    format!("{}...", &raw[..80])
                } else {
                    raw.to_string()
                };
                let line = byte_to_line(m.start());
                let entry = format!("L{line}: {s}");
                if seen.insert(entry.clone()) {
                    syms.push(entry);
                }
            }
        }
    }
    syms.truncate(30);
    syms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_src() {
        let m = gen_map(Path::new("src"), 2).unwrap();
        assert!(m.contains("main.rs"));
    }

    #[test]
    fn t_tool() {
        let t = RepoMapTool;
        assert_eq!(t.name(), "repo_map");
        let r = t.call(serde_json::json!({"path":"src"})).unwrap();
        assert!(r.success);
    }

    #[test]
    fn t_none() {
        let m = gen_map(Path::new("/nonexistent"), 2).unwrap();
        assert!(m.contains("no source"));
    }

    #[test]
    fn t_go_syms() {
        let dir = tempfile::tempdir().unwrap();
        let go_file = dir.path().join("main.go");
        std::fs::write(&go_file, "package main\n\nfunc main() {\n}\n\ntype Server struct {\n}\n").unwrap();
        let syms = extract_syms(&go_file);
        assert!(syms.iter().any(|s| s.contains("func main")), "Go func: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("type Server")), "Go type: {syms:?}");
    }

    #[test]
    fn t_java_syms() {
        let dir = tempfile::tempdir().unwrap();
        let java_file = dir.path().join("App.java");
        std::fs::write(&java_file, "public class App {\n  public void run() {}\n}\n").unwrap();
        let syms = extract_syms(&java_file);
        assert!(syms.iter().any(|s| s.contains("class App")), "Java class: {syms:?}");
    }

    #[test]
    fn t_c_syms() {
        let dir = tempfile::tempdir().unwrap();
        let c_file = dir.path().join("util.h");
        std::fs::write(&c_file, "typedef struct Node {\n  int val;\n} Node;\n\nvoid init_node(Node *n);\n").unwrap();
        let syms = extract_syms(&c_file);
        assert!(syms.iter().any(|s| s.contains("struct Node")), "C struct: {syms:?}");
    }

    // --- 新規テスト ---

    #[test]
    fn t_kotlin_syms() {
        let dir = tempfile::tempdir().unwrap();
        let kt_file = dir.path().join("App.kt");
        std::fs::write(&kt_file, "data class User(val name: String)\n\nfun main() {\n}\n\ninterface Repository {\n}\n").unwrap();
        let syms = extract_syms(&kt_file);
        assert!(syms.iter().any(|s| s.contains("class User")), "Kotlin data class: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("fun main")), "Kotlin fun: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("interface Repository")), "Kotlin interface: {syms:?}");
    }

    #[test]
    fn t_swift_syms() {
        let dir = tempfile::tempdir().unwrap();
        let swift_file = dir.path().join("App.swift");
        std::fs::write(&swift_file, "class ViewController {\n  func viewDidLoad() {}\n}\n\nstruct Config {\n}\n\nprotocol Serviceable {\n}\n").unwrap();
        let syms = extract_syms(&swift_file);
        assert!(syms.iter().any(|s| s.contains("class ViewController")), "Swift class: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("func viewDidLoad")), "Swift func: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("struct Config")), "Swift struct: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("protocol Serviceable")), "Swift protocol: {syms:?}");
    }

    #[test]
    fn t_rust_pub_crate() {
        let dir = tempfile::tempdir().unwrap();
        let rs_file = dir.path().join("lib.rs");
        std::fs::write(&rs_file, "pub(crate) fn internal_fn() {}\npub(crate) struct InternalStruct;\npub mod utils;\npub type Alias = String;\npub const MAX: usize = 100;\n").unwrap();
        let syms = extract_syms(&rs_file);
        assert!(syms.iter().any(|s| s.contains("fn internal_fn")), "pub(crate) fn: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("struct InternalStruct")), "pub(crate) struct: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("pub mod utils")), "pub mod: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("pub type Alias")), "pub type: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("pub const MAX") || s.contains("const MAX")), "pub const: {syms:?}");
    }

    #[test]
    fn t_ts_interface() {
        let dir = tempfile::tempdir().unwrap();
        let ts_file = dir.path().join("api.ts");
        std::fs::write(&ts_file, "export interface ApiResponse {\n  data: any;\n}\nexport type UserId = string;\n").unwrap();
        let syms = extract_syms(&ts_file);
        assert!(syms.iter().any(|s| s.contains("interface ApiResponse")), "TS interface: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("type UserId")), "TS type: {syms:?}");
    }

    #[test]
    fn t_python_async_def() {
        let dir = tempfile::tempdir().unwrap();
        let py_file = dir.path().join("handler.py");
        std::fs::write(&py_file, "async def handle_request(req):\n    pass\n\ndef sync_helper():\n    pass\n").unwrap();
        let syms = extract_syms(&py_file);
        assert!(syms.iter().any(|s| s.contains("async def handle_request")), "async def: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("def sync_helper")), "sync def: {syms:?}");
    }

    #[test]
    fn t_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let rs_file = dir.path().join("lines.rs");
        std::fs::write(&rs_file, "// comment\n\npub fn foo() {}\n\npub fn bar() {}\n").unwrap();
        let syms = extract_syms(&rs_file);
        // foo は3行目、barは5行目
        assert!(syms.iter().any(|s| s.starts_with("L3:")), "foo at L3: {syms:?}");
        assert!(syms.iter().any(|s| s.starts_with("L5:")), "bar at L5: {syms:?}");
    }

    #[test]
    fn t_dedup_via_hashset() {
        let dir = tempfile::tempdir().unwrap();
        let rs_file = dir.path().join("dup.rs");
        // impl Foo が2回出ても HashSet で重複排除
        std::fs::write(&rs_file, "impl Foo {\n  pub fn a() {}\n}\nimpl Foo {\n  pub fn b() {}\n}\n").unwrap();
        let syms = extract_syms(&rs_file);
        let impl_count = syms.iter().filter(|s| s.contains("impl Foo")).count();
        assert_eq!(impl_count, 2, "異なる行のimplは両方表示: {syms:?}");
    }

    #[test]
    fn t_supported_exts() {
        assert!(SUPPORTED_EXTS.contains(&"kt"));
        assert!(SUPPORTED_EXTS.contains(&"swift"));
        assert!(SUPPORTED_EXTS.contains(&"rs"));
        assert!(!SUPPORTED_EXTS.contains(&"md"));
    }
}
