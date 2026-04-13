use crate::tools::permission::Permission;
use crate::tools::{Tool, ToolResult};
use anyhow::Result;
pub struct ArxivTool;
impl Tool for ArxivTool {
    fn name(&self) -> &str {
        "arxiv_search"
    }
    fn description(&self) -> &str {
        "arxiv論文を検索する。queryパラメータに検索クエリを指定。"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]})
    }
    fn permission(&self) -> Permission {
        Permission::Auto
    }
    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("'query'が必要"))?;
        match crate::memory::evolution::search_arxiv(query, 5) {
            Ok(entries) if entries.is_empty() => Ok(ToolResult {
                output: format!("「{query}」の論文なし"),
                success: true,
            }),
            Ok(entries) => {
                let mut o = format!("arxiv: {}件\n\n", entries.len());
                for e in &entries {
                    o.push_str(&format!("- [{}] {}\n  {}\n\n", e.id, e.title, e.summary));
                }
                Ok(ToolResult {
                    output: o,
                    success: true,
                })
            }
            Err(e) => Ok(ToolResult {
                output: format!("エラー: {e}"),
                success: false,
            }),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn t_meta() {
        assert_eq!(ArxivTool.name(), "arxiv_search");
    }
    #[test]
    fn t_miss() {
        assert!(ArxivTool.call(serde_json::json!({})).is_err());
    }
    #[test]
    #[ignore]
    fn t_live() {
        let r = ArxivTool
            .call(serde_json::json!({"query":"1-bit LLM"}))
            .unwrap();
        assert!(r.success);
    }
}
