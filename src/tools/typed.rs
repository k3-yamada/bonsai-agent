use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::permission::Permission;
use crate::tools::ToolResult;

/// 型駆動ツール定義トレイト
/// 構造体でパラメータを定義し、スキーマ自動生成+型安全パースを実現
pub trait TypedTool: Send + Sync {
    /// パラメータ型（Deserialize + JsonSchemaを自動derive）
    type Args: for<'de> Deserialize<'de> + JsonSchema;

    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    const PERMISSION: Permission;
    const READ_ONLY: bool = false;

    /// 型安全な引数で実行
    fn execute(&self, args: Self::Args) -> Result<ToolResult>;
}

/// schemarsのJSON SchemaをLLM向けに簡素化
/// $schema, title, $defs等のメタフィールドを除去
pub fn simplify_schema<T: JsonSchema>() -> serde_json::Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(schema).unwrap_or_default();
    strip_meta(&mut value);
    value
}

/// メタフィールドを再帰的に除去
fn strip_meta(value: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = value {
        map.remove("$schema");
        map.remove("title");
        map.remove("definitions");
        map.remove("$defs");
        for (_, v) in map.iter_mut() {
            strip_meta(v);
        }
    }
}

/// TypedTool → Tool ブランケット実装
impl<T: TypedTool> crate::tools::Tool for T {
    fn name(&self) -> &str {
        T::NAME
    }

    fn description(&self) -> &str {
        T::DESCRIPTION
    }

    fn parameters_schema(&self) -> serde_json::Value {
        simplify_schema::<T::Args>()
    }

    fn permission(&self) -> Permission {
        T::PERMISSION
    }

    fn is_read_only(&self) -> bool {
        T::READ_ONLY
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let mut args = args;
        crate::agent::parse::coerce_tool_arguments(&mut args);
        let typed: T::Args = serde_json::from_value(args)
            .map_err(|e| anyhow::anyhow!("パラメータ解析エラー: {e}"))?;
        self.execute(typed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize, JsonSchema)]
    struct DummyArgs {
        /// テスト用クエリ
        query: String,
    }

    struct DummyTool;

    impl TypedTool for DummyTool {
        type Args = DummyArgs;
        const NAME: &'static str = "dummy";
        const DESCRIPTION: &'static str = "テスト用ダミーツール";
        const PERMISSION: Permission = Permission::Auto;
        const READ_ONLY: bool = true;

        fn execute(&self, args: DummyArgs) -> Result<ToolResult> {
            Ok(ToolResult {
                output: format!("query={}", args.query),
                success: true,
            })
        }
    }

    #[test]
    fn t_typed_tool_as_tool() {
        use crate::tools::Tool;
        let tool = DummyTool;
        assert_eq!(tool.name(), "dummy");
        assert_eq!(tool.description(), "テスト用ダミーツール");
        assert!(tool.is_read_only());
    }

    #[test]
    fn t_typed_tool_call() {
        use crate::tools::Tool;
        let tool = DummyTool;
        let result = tool.call(serde_json::json!({"query": "hello"})).unwrap();
        assert!(result.success);
        assert_eq!(result.output, "query=hello");
    }

    #[test]
    fn t_typed_tool_missing_param() {
        use crate::tools::Tool;
        let tool = DummyTool;
        let result = tool.call(serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn t_typed_tool_coerce_string_to_type() {
        use crate::tools::Tool;
        // queryは文字列なのでcoerceは影響なし、正常動作確認
        let tool = DummyTool;
        let result = tool.call(serde_json::json!({"query": "test"})).unwrap();
        assert!(result.success);
    }

    #[test]
    fn t_simplify_schema_removes_meta() {
        let schema = simplify_schema::<DummyArgs>();
        assert!(schema.get("$schema").is_none());
        assert!(schema.get("title").is_none());
        // propertiesは保持される
        assert!(schema.get("properties").is_some());
    }

    #[test]
    fn t_simplify_schema_has_required() {
        let schema = simplify_schema::<DummyArgs>();
        let required = schema.get("required").and_then(|v| v.as_array());
        assert!(required.is_some());
        let req_strs: Vec<&str> = required.unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert!(req_strs.contains(&"query"));
    }

    #[test]
    fn t_typed_tool_registry_compat() {
        use crate::tools::ToolRegistry;
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(DummyTool));
        assert!(reg.get("dummy").is_some());
    }
}
