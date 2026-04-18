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

// ============================================================
// tree-sitter AST ベースのシンボル抽出（Rust/Python/TS/JS/Go）
// feature "tree-sitter" 有効時のみコンパイル
// ============================================================

#[cfg(feature = "tree-sitter")]
mod ast {
    /// tree-sitter対応言語かどうか判定し、パーサーを返す
    pub fn try_parse(ext: &str, source: &str) -> Option<tree_sitter::Tree> {
        let lang = match ext {
            "rs" => tree_sitter_rust::LANGUAGE.into(),
            "py" => tree_sitter_python::LANGUAGE.into(),
            "ts" | "tsx" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "js" => tree_sitter_javascript::LANGUAGE.into(),
            "go" => tree_sitter_go::LANGUAGE.into(),
            _ => return None,
        };
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang).ok()?;
        parser.parse(source, None)
    }

    /// ASTからシンボルを抽出（行番号付き）
    pub fn extract_from_tree(
        tree: &tree_sitter::Tree,
        source: &str,
        ext: &str,
    ) -> Vec<String> {
        let mut syms = Vec::new();
        let mut seen = std::collections::HashSet::new();
        walk_node(&tree.root_node(), source, ext, &mut syms, &mut seen);
        syms.truncate(30);
        syms
    }

    fn walk_node(
        node: &tree_sitter::Node,
        source: &str,
        ext: &str,
        syms: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        let line = node.start_position().row + 1;
        let kind = node.kind();

        let entry = match ext {
            "rs" => extract_rust_symbol(node, source, kind, line),
            "py" => extract_python_symbol(node, source, kind, line),
            "ts" | "tsx" | "js" => extract_ts_symbol(node, source, kind, line),
            "go" => extract_go_symbol(node, source, kind, line),
            _ => None,
        };

        if let Some(e) = entry
            && seen.insert(e.clone())
        {
            syms.push(e);
        }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i as u32) {
                walk_node(&child, source, ext, syms, seen);
            }
        }
    }

    // --- Rust ---
    fn extract_rust_symbol(
        node: &tree_sitter::Node,
        source: &str,
        kind: &str,
        line: usize,
    ) -> Option<String> {
        match kind {
            "function_item" => {
                let name = field_text(node, "name", source)?;
                let vis = visibility_text(node, source);
                // async は function_modifiers > async にある
                let is_async = has_child_kind(node, "async")
                    || has_grandchild_kind(node, "function_modifiers", "async");
                let async_kw = if is_async { "async " } else { "" };
                Some(format!("L{line}: {vis}{async_kw}fn {name}"))
            }
            "struct_item" => {
                let name = field_text(node, "name", source)?;
                let vis = visibility_text(node, source);
                Some(format!("L{line}: {vis}struct {name}"))
            }
            "enum_item" => {
                let name = field_text(node, "name", source)?;
                let vis = visibility_text(node, source);
                Some(format!("L{line}: {vis}enum {name}"))
            }
            "trait_item" => {
                let name = field_text(node, "name", source)?;
                let vis = visibility_text(node, source);
                Some(format!("L{line}: {vis}trait {name}"))
            }
            "impl_item" => {
                let type_name = field_text(node, "type", source)?;
                if let Some(trait_name) = field_text(node, "trait", source) {
                    Some(format!("L{line}: impl {trait_name} for {type_name}"))
                } else {
                    Some(format!("L{line}: impl {type_name}"))
                }
            }
            "mod_item" => {
                let name = field_text(node, "name", source)?;
                let vis = visibility_text(node, source);
                Some(format!("L{line}: {vis}mod {name}"))
            }
            "type_item" => {
                let name = field_text(node, "name", source)?;
                let vis = visibility_text(node, source);
                Some(format!("L{line}: {vis}type {name}"))
            }
            "const_item" => {
                let name = field_text(node, "name", source)?;
                let vis = visibility_text(node, source);
                Some(format!("L{line}: {vis}const {name}"))
            }
            "static_item" => {
                let name = field_text(node, "name", source)?;
                let vis = visibility_text(node, source);
                Some(format!("L{line}: {vis}static {name}"))
            }
            _ => None,
        }
    }

    // --- Python ---
    fn extract_python_symbol(
        node: &tree_sitter::Node,
        source: &str,
        kind: &str,
        line: usize,
    ) -> Option<String> {
        match kind {
            "function_definition" => {
                let name = field_text(node, "name", source)?;
                let is_async = has_child_kind(node, "async");
                if is_async {
                    Some(format!("L{line}: async def {name}"))
                } else {
                    Some(format!("L{line}: def {name}"))
                }
            }
            "class_definition" => {
                let name = field_text(node, "name", source)?;
                Some(format!("L{line}: class {name}"))
            }
            _ => None,
        }
    }

    // --- TypeScript / JavaScript ---
    fn extract_ts_symbol(
        node: &tree_sitter::Node,
        source: &str,
        kind: &str,
        line: usize,
    ) -> Option<String> {
        match kind {
            "function_declaration" => {
                let name = field_text(node, "name", source)?;
                let is_async = has_child_kind(node, "async");
                let prefix = if is_async { "async function" } else { "function" };
                Some(format!("L{line}: {prefix} {name}"))
            }
            "class_declaration" => {
                let name = field_text(node, "name", source)?;
                Some(format!("L{line}: class {name}"))
            }
            "interface_declaration" => {
                let name = field_text(node, "name", source)?;
                Some(format!("L{line}: interface {name}"))
            }
            "type_alias_declaration" => {
                let name = field_text(node, "name", source)?;
                Some(format!("L{line}: type {name}"))
            }
            _ => None,
        }
    }

    // --- Go ---
    fn extract_go_symbol(
        node: &tree_sitter::Node,
        source: &str,
        kind: &str,
        line: usize,
    ) -> Option<String> {
        match kind {
            "function_declaration" => {
                let name = field_text(node, "name", source)?;
                Some(format!("L{line}: func {name}"))
            }
            "method_declaration" => {
                let name = field_text(node, "name", source)?;
                if let Some(params) = node.child_by_field_name("receiver") {
                    let recv = &source[params.byte_range()];
                    Some(format!("L{line}: func {recv} {name}"))
                } else {
                    Some(format!("L{line}: func {name}"))
                }
            }
            "type_declaration" => {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i as u32)
                        && child.kind() == "type_spec"
                        && let Some(name_node) = child.child_by_field_name("name")
                    {
                        let name = &source[name_node.byte_range()];
                        if let Some(type_node) = child.child_by_field_name("type") {
                            let type_kind = type_node.kind();
                            return Some(format!("L{line}: type {name} {type_kind}"));
                        }
                        return Some(format!("L{line}: type {name}"));
                    }
                }
                None
            }
            _ => None,
        }
    }

    // --- ヘルパー ---

    fn field_text<'a>(
        node: &tree_sitter::Node,
        field: &str,
        source: &'a str,
    ) -> Option<&'a str> {
        let child = node.child_by_field_name(field)?;
        Some(&source[child.byte_range()])
    }

    fn visibility_text(node: &tree_sitter::Node, source: &str) -> String {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i as u32)
                && child.kind() == "visibility_modifier"
            {
                let vis = &source[child.byte_range()];
                return format!("{vis} ");
            }
        }
        String::new()
    }

    /// 指定した子ノードの中に孫ノードがあるか（例: function_modifiers > async）
    fn has_grandchild_kind(node: &tree_sitter::Node, parent_kind: &str, child_kind: &str) -> bool {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i as u32)
                && child.kind() == parent_kind
            {
                return has_child_kind(&child, child_kind);
            }
        }
        false
    }

    fn has_child_kind(node: &tree_sitter::Node, kind: &str) -> bool {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i as u32)
                && child.kind() == kind
            {
                return true;
            }
        }
        false
    }
}

// ============================================================
// 正規表現フォールバック（Java/C/C++/Kotlin/Swift + tree-sitter無効時）
// ============================================================

/// シンボル抽出用パターン定義（正規表現フォールバック）
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

/// 正規表現ベースのシンボル抽出（フォールバック用）
fn extract_syms_regex(content: &str, ext: &str) -> Vec<String> {
    let pats = symbol_patterns(ext);
    if pats.is_empty() {
        return vec![];
    }

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
            for m in re.find_iter(content) {
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

/// ファイルからシンボルを抽出（行番号付き）
/// tree-sitter対応言語はASTベース、それ以外は正規表現フォールバック
fn extract_syms(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    // tree-sitterが有効で対応言語ならAST版を使用
    #[cfg(feature = "tree-sitter")]
    {
        if let Some(tree) = ast::try_parse(ext, &content) {
            return ast::extract_from_tree(&tree, &content, ext);
        }
    }

    // フォールバック: 正規表現版
    extract_syms_regex(&content, ext)
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
        assert!(syms.iter().any(|s| s.contains("mod utils")), "pub mod: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("type Alias")), "pub type: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("const MAX")), "pub const: {syms:?}");
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
        assert!(syms.iter().any(|s| s.starts_with("L3:")), "foo at L3: {syms:?}");
        assert!(syms.iter().any(|s| s.starts_with("L5:")), "bar at L5: {syms:?}");
    }

    #[test]
    fn t_dedup_via_hashset() {
        let dir = tempfile::tempdir().unwrap();
        let rs_file = dir.path().join("dup.rs");
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

    // --- tree-sitter AST テスト ---

    #[test]
    fn t_rust_impl_trait() {
        let dir = tempfile::tempdir().unwrap();
        let rs_file = dir.path().join("impl.rs");
        std::fs::write(
            &rs_file,
            "pub trait Drawable {\n    fn draw(&self);\n}\n\nimpl Drawable for Circle {\n    fn draw(&self) {}\n}\n",
        ).unwrap();
        let syms = extract_syms(&rs_file);
        assert!(syms.iter().any(|s| s.contains("trait Drawable")), "trait: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("impl Drawable for Circle")), "impl for: {syms:?}");
    }

    #[test]
    fn t_rust_async_fn() {
        let dir = tempfile::tempdir().unwrap();
        let rs_file = dir.path().join("async.rs");
        std::fs::write(&rs_file, "pub async fn fetch_data() {}\n").unwrap();
        let syms = extract_syms(&rs_file);
        assert!(syms.iter().any(|s| s.contains("async") && s.contains("fn fetch_data")), "async fn: {syms:?}");
    }

    #[test]
    fn t_go_method() {
        let dir = tempfile::tempdir().unwrap();
        let go_file = dir.path().join("server.go");
        std::fs::write(&go_file, "package main\n\ntype Server struct{}\n\nfunc (s *Server) Run() {}\n").unwrap();
        let syms = extract_syms(&go_file);
        assert!(syms.iter().any(|s| s.contains("type Server")), "Go type: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("func") && s.contains("Run")), "Go method: {syms:?}");
    }

    #[test]
    fn t_ts_async_function() {
        let dir = tempfile::tempdir().unwrap();
        let ts_file = dir.path().join("handler.ts");
        std::fs::write(&ts_file, "export async function handleRequest() {}\n").unwrap();
        let syms = extract_syms(&ts_file);
        assert!(syms.iter().any(|s| s.contains("async function handleRequest")), "TS async fn: {syms:?}");
    }

    #[test]
    fn t_python_class_method() {
        let dir = tempfile::tempdir().unwrap();
        let py_file = dir.path().join("model.py");
        std::fs::write(&py_file, "class Model:\n    def forward(self, x):\n        pass\n").unwrap();
        let syms = extract_syms(&py_file);
        assert!(syms.iter().any(|s| s.contains("class Model")), "Python class: {syms:?}");
        assert!(syms.iter().any(|s| s.contains("def forward")), "Python method: {syms:?}");
    }

    #[test]
    fn t_go_interface() {
        let dir = tempfile::tempdir().unwrap();
        let go_file = dir.path().join("iface.go");
        std::fs::write(&go_file, "package main\n\ntype Reader interface {\n\tRead(p []byte) (int, error)\n}\n").unwrap();
        let syms = extract_syms(&go_file);
        assert!(syms.iter().any(|s| s.contains("type Reader")), "Go interface: {syms:?}");
    }

    #[test]
    fn t_rust_enum() {
        let dir = tempfile::tempdir().unwrap();
        let rs_file = dir.path().join("color.rs");
        std::fs::write(&rs_file, "pub enum Color {\n    Red,\n    Green,\n    Blue,\n}\n").unwrap();
        let syms = extract_syms(&rs_file);
        assert!(syms.iter().any(|s| s.contains("enum Color")), "enum: {syms:?}");
    }
}
