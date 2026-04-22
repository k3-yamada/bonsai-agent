use crate::tools::ToolResult;
use crate::tools::permission::Permission;
use crate::tools::typed::TypedTool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
pub struct ArxivArgs {
    /// 検索クエリ
    query: String,
}

pub struct ArxivTool;

impl TypedTool for ArxivTool {
    type Args = ArxivArgs;
    const NAME: &'static str = "arxiv_search";
    const DESCRIPTION: &'static str = super::descriptions::ARXIV_SEARCH;
    const PERMISSION: Permission = Permission::Auto;
    const READ_ONLY: bool = true;

    fn execute(&self, args: ArxivArgs) -> Result<ToolResult> {
        match crate::memory::evolution::search_arxiv(&args.query, 5) {
            Ok(entries) if entries.is_empty() => Ok(ToolResult {
                output: format!("「{}」の論文なし", args.query),
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
    use crate::tools::Tool;

    #[test]
    fn t_meta() {
        assert_eq!(ArxivTool.name(), "arxiv_search");
        assert!(ArxivTool.is_read_only());
    }

    #[test]
    fn t_miss() {
        assert!(ArxivTool.call(serde_json::json!({})).is_err());
    }

    #[test]
    fn t_schema_has_query() {
        let schema = ArxivTool.parameters_schema();
        assert!(schema.get("properties").unwrap().get("query").is_some());
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
