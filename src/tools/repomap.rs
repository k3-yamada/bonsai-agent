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
            && matches!(
                ext,
                "rs" | "py" | "ts" | "tsx" | "js" | "go" | "java" | "c" | "cpp" | "h"
            )
        {
            files.push(p);
        }
    }
}
fn extract_syms(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let pats: Vec<&str> = match ext {
        "rs" => vec![
            r"pub\s+(async\s+)?fn\s+\w+",
            r"pub\s+struct\s+\w+",
            r"pub\s+enum\s+\w+",
            r"pub\s+trait\s+\w+",
            r"impl\s+\w+",
        ],
        "py" => vec![r"def\s+\w+\s*\(", r"class\s+\w+"],
        "ts" | "tsx" | "js" => vec![r"(export\s+)?function\s+\w+", r"(export\s+)?class\s+\w+"],
        "go" => vec![r"func\s+(\(\w+\s+\*?\w+\)\s+)?\w+", r"type\s+\w+\s+(struct|interface)"],
        "java" => vec![
            r"(public\s+|private\s+|protected\s+)?(static\s+)?class\s+\w+",
            r"(public\s+|private\s+|protected\s+)?(static\s+)?\w+\s+\w+\s*\(",
        ],
        "c" | "cpp" | "h" => vec![
            r"(typedef\s+)?struct\s+\w+",
            r"(typedef\s+)?enum\s+\w+",
            r"\w+\s+\w+\s*\([^)]*\)\s*[;{]",
        ],
        _ => return vec![],
    };
    let mut syms = Vec::new();
    for p in &pats {
        if let Ok(re) = Regex::new(p) {
            for m in re.find_iter(&content) {
                let s = m.as_str().trim();
                syms.push(if s.len() > 80 {
                    format!("{}...", &s[..80])
                } else {
                    s.to_string()
                });
            }
        }
    }
    syms.dedup();
    syms.truncate(20);
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
}
