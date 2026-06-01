//! 能動的記憶ツール: `remember`(保存) / `recall`(想起)。
//!
//! production の自動注入経路 (`context_inject::inject_contextual_memories`) は
//! 受動的にトップ K 記憶を注入するのみ。本ツールはエージェントが**意図的に**
//! 事実を保存・想起する経路を提供する (パーソナル知識デーモン ①Phase 1)。
//!
//! `MemoryStore` は `Connection`(`!Sync`)を保持するため `Tool`(`Send + Sync`)に
//! 直接持たせられない。よって `db_path: String` のみ保持し、`execute` 内で都度
//! `MemoryStore::open` する (SQLite WAL で並行安全、`try_clone_for_thread` と同設計)。

use crate::tools::ToolResult;
use crate::tools::permission::Permission;
use crate::tools::typed::TypedTool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;

/// `remember` ツール: 長期記憶へ事実を保存する。
pub struct RememberTool {
    db_path: String,
}

impl RememberTool {
    pub fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct RememberArgs {
    /// 記憶する内容(事実・好み・指示など)
    content: String,
    /// 分類(任意、既定 "fact")
    #[serde(default)]
    category: Option<String>,
    /// 検索用タグ(任意)
    #[serde(default)]
    tags: Option<Vec<String>>,
}

impl TypedTool for RememberTool {
    type Args = RememberArgs;
    const NAME: &'static str = "remember";
    const DESCRIPTION: &'static str = super::descriptions::REMEMBER;
    const PERMISSION: Permission = Permission::Auto;
    const READ_ONLY: bool = false;

    fn execute(&self, args: RememberArgs) -> Result<ToolResult> {
        let store = crate::memory::store::MemoryStore::open(&self.db_path)?;
        let category = args.category.as_deref().unwrap_or("fact");
        let tags = args.tags.unwrap_or_default();
        let id = store.save_memory(&args.content, category, &tags)?;
        Ok(ToolResult {
            output: format!("記憶を保存しました (id={id}, category={category})"),
            success: true,
        })
    }
}

/// `recall` ツール: 保存済み記憶を検索して想起する。
pub struct RecallTool {
    db_path: String,
}

impl RecallTool {
    pub fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct RecallArgs {
    /// 検索キーワード
    query: String,
    /// 最大件数(任意、既定 5)
    #[serde(default)]
    limit: Option<usize>,
}

impl TypedTool for RecallTool {
    type Args = RecallArgs;
    const NAME: &'static str = "recall";
    const DESCRIPTION: &'static str = super::descriptions::RECALL;
    const PERMISSION: Permission = Permission::Auto;
    const READ_ONLY: bool = true;

    fn execute(&self, args: RecallArgs) -> Result<ToolResult> {
        let store = crate::memory::store::MemoryStore::open(&self.db_path)?;
        let limit = args.limit.unwrap_or(5);
        // FTS5(unicode61)は日本語の部分一致を tokenize できない(空白区切り前提で
        // 連続CJKを1トークン化する)ため、言語非依存の LIKE 部分一致で想起する。
        // 個人用途では memories 件数が小さくフルスキャンを許容。FTS5/hybrid による
        // 関連度ランキングは後続フェーズ。
        let pattern = format!("%{}%", args.query);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT category, content
             FROM memories
             WHERE content LIKE ?1 OR tags LIKE ?1
             ORDER BY id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![pattern, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut hits: Vec<(String, String)> = Vec::new();
        for row in rows {
            hits.push(row?);
        }
        if hits.is_empty() {
            return Ok(ToolResult {
                output: format!("「{}」に該当する記憶なし", args.query),
                success: true,
            });
        }
        let mut o = format!("{}件の記憶:\n", hits.len());
        for (category, content) in &hits {
            o.push_str(&format!("- [{category}] {content}\n"));
        }
        Ok(ToolResult {
            output: o,
            success: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;

    /// 一時 DB ファイルパスを生成(プロセス内ユニーク、file-backed)。
    fn temp_db_path() -> String {
        let dir = std::env::temp_dir();
        let unique = format!(
            "bonsai_mem_tool_test_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        dir.join(unique).to_string_lossy().to_string()
    }

    #[test]
    fn t_remember_meta() {
        let tool = RememberTool::new("/tmp/x.db");
        assert_eq!(tool.name(), "remember");
        assert!(!tool.is_read_only(), "remember は書込ツール");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn t_recall_meta() {
        let tool = RecallTool::new("/tmp/x.db");
        assert_eq!(tool.name(), "recall");
        assert!(tool.is_read_only(), "recall は読取専用");
    }

    #[test]
    fn t_remember_schema_has_content() {
        let tool = RememberTool::new("/tmp/x.db");
        let schema = tool.parameters_schema();
        assert!(
            schema
                .get("properties")
                .and_then(|p| p.get("content"))
                .is_some(),
            "content プロパティ必要"
        );
    }

    #[test]
    fn t_recall_schema_has_query() {
        let tool = RecallTool::new("/tmp/x.db");
        let schema = tool.parameters_schema();
        assert!(
            schema
                .get("properties")
                .and_then(|p| p.get("query"))
                .is_some(),
            "query プロパティ必要"
        );
    }

    #[test]
    fn t_remember_missing_content_errors() {
        let tool = RememberTool::new("/tmp/x.db");
        assert!(tool.call(serde_json::json!({})).is_err());
    }

    #[test]
    fn t_recall_missing_query_errors() {
        let tool = RecallTool::new("/tmp/x.db");
        assert!(tool.call(serde_json::json!({})).is_err());
    }

    #[test]
    fn t_remember_returns_success() {
        let path = temp_db_path();
        let tool = RememberTool::new(&path);
        let r = tool
            .call(serde_json::json!({"content": "keizo は日本語での回答を好む"}))
            .expect("remember は成功すべき");
        assert!(r.success, "保存成功すべき");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn t_remember_then_recall_roundtrip() {
        let path = temp_db_path();
        // 保存
        RememberTool::new(&path)
            .call(serde_json::json!({
                "content": "プロジェクトの締切は金曜日",
                "tags": ["deadline"]
            }))
            .expect("remember 成功");
        // 想起
        let r = RecallTool::new(&path)
            .call(serde_json::json!({"query": "締切"}))
            .expect("recall 成功");
        assert!(r.success);
        assert!(
            r.output.contains("金曜日"),
            "保存した内容が想起されるべき: {}",
            r.output
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn t_recall_empty_when_no_match() {
        let path = temp_db_path();
        RememberTool::new(&path)
            .call(serde_json::json!({"content": "りんごは赤い"}))
            .expect("remember 成功");
        let r = RecallTool::new(&path)
            .call(serde_json::json!({"query": "全く無関係なクエリxyzzy"}))
            .expect("recall 成功");
        assert!(r.success, "ヒット 0 でも success=true(エラーではない)");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn t_recall_respects_limit() {
        let path = temp_db_path();
        let remember = RememberTool::new(&path);
        for i in 0..5 {
            remember
                .call(serde_json::json!({"content": format!("memo apple {i}")}))
                .expect("remember 成功");
        }
        let r = RecallTool::new(&path)
            .call(serde_json::json!({"query": "apple", "limit": 2}))
            .expect("recall 成功");
        // limit=2 で 2 件以下に制限されるべき(出力中の "apple" 出現数で近似確認)
        let hit_lines = r.output.matches("apple").count();
        assert!(hit_lines <= 2, "limit=2 を超過: {}", r.output);
        let _ = std::fs::remove_file(&path);
    }

    // ---------- Phase 3 Red: 関連度ランキング (token overlap scoring) ----------

    #[test]
    fn t_recall_ranks_multi_token_higher() {
        // 2 token 一致が 1 token 一致より上位に来るべき。
        // 現状: LIKE %apple banana% は literal 一致しないため空 → Red 確証。
        let path = temp_db_path();
        let r = RememberTool::new(&path);
        r.call(serde_json::json!({"content": "apple only"})).unwrap();
        r.call(serde_json::json!({"content": "apple and banana together"}))
            .unwrap();
        r.call(serde_json::json!({"content": "banana only"})).unwrap();
        let recall = RecallTool::new(&path)
            .call(serde_json::json!({"query": "apple banana", "limit": 10}))
            .expect("recall 成功");
        let out = &recall.output;
        let pos_both = out.find("apple and banana together").unwrap_or(usize::MAX);
        let pos_apple = out.find("apple only").unwrap_or(usize::MAX);
        let pos_banana = out.find("banana only").unwrap_or(usize::MAX);
        assert!(
            pos_both < pos_apple && pos_both < pos_banana,
            "2-token 一致が先頭に来るべき: {out}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn t_recall_multi_token_cjk_ranking() {
        // CJK 多 token クエリでも overlap で ranking されるべき (日本語実利用)。
        let path = temp_db_path();
        let r = RememberTool::new(&path);
        r.call(serde_json::json!({"content": "金曜日は会議"})).unwrap();
        r.call(serde_json::json!({"content": "金曜日に締切がある会議"}))
            .unwrap();
        r.call(serde_json::json!({"content": "土曜日に予定"})).unwrap();
        let recall = RecallTool::new(&path)
            .call(serde_json::json!({"query": "金曜 締切", "limit": 10}))
            .expect("recall 成功");
        let out = &recall.output;
        let pos_both = out.find("金曜日に締切がある会議").unwrap_or(usize::MAX);
        let pos_one = out.find("金曜日は会議").unwrap_or(usize::MAX);
        assert!(
            pos_both < pos_one,
            "CJK 2-token 一致が先頭に来るべき: {out}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn t_recall_cjk_single_token_preserved() {
        // 単一 token (CJK 部分一致) は後方互換: 従来の LIKE %query% と同等にヒット。
        let path = temp_db_path();
        RememberTool::new(&path)
            .call(serde_json::json!({"content": "今週の予定は会議"}))
            .unwrap();
        let r = RecallTool::new(&path)
            .call(serde_json::json!({"query": "予定"}))
            .expect("recall 成功");
        assert!(
            r.output.contains("今週の予定は会議"),
            "単一 CJK token 部分一致が動くべき: {}",
            r.output
        );
        let _ = std::fs::remove_file(&path);
    }
}
