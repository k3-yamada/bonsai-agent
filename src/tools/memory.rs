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
        // FTS5(unicode61) は CJK を 1 トークン化するため LIKE 部分一致で想起する
        // (CJK 後方互換を維持)。多 token クエリでは literal 一致が極稀になるため、
        // whitespace で分割した各 token を OR で union 検索 → Rust 側で
        // overlap スコアリング (一致 token 数 desc、同点は id desc) で
        // 関連度を擬似 hybrid 風に近似する (Phase 3、項目 270)。
        let tokens = tokenize_recall_query(&args.query);
        let hits = recall_scored(store.conn(), &tokens, limit)?;
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

/// クエリを whitespace で分割し、重複・空 token を除去した検索 token 配列を返す。
/// 単一 token (CJK 含む) のときは長さ 1 配列となり従来 LIKE 挙動と等価。
/// 重複除去により同一語の連続入力 ("apple apple") がスコアを不当に増幅しない。
/// 空/空白のみクエリは空配列を返し、呼出側 (`recall_scored`) で 0 件にフォールバックする。
fn tokenize_recall_query(query: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    query
        .split_whitespace()
        .filter(|t| !t.is_empty() && seen.insert(*t))
        .map(|t| t.to_string())
        .collect()
}

/// per-token LIKE の OR で候補を集め、各候補を **IDF 重み付き overlap** で
/// 降順ソート、同点は id desc (recency) で安定化する。
///
/// raw count (一致 token 数) では `使い方` 等の頻出汎用語が低関連 chunk を引き上げる
/// ノイズが出る (実 vault 9079 chunk で観測)。各 token の希少度 (IDF = ln((N+1)/(df+1))+1)
/// で重み付けし、稀な語の一致を高評価することで関連度を改善する。
fn recall_scored(
    conn: &rusqlite::Connection,
    tokens: &[String],
    limit: usize,
) -> Result<Vec<(String, String)>> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    // 各 token の IDF 重みを事前計算 (corpus 全体の df を基準)。
    let idf = compute_idf_weights(conn, tokens)?;

    // OR 連結の where 句を動的生成 (パラメータは tokens の 2 倍 = content/tags 各 1)。
    let conditions: Vec<&str> = (0..tokens.len())
        .map(|_| "content LIKE ? OR tags LIKE ?")
        .collect();
    let where_clause = conditions.join(" OR ");
    let sql = format!(
        "SELECT id, category, content, tags FROM memories WHERE {where_clause} ORDER BY id DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<String> = Vec::with_capacity(tokens.len() * 2);
    for t in tokens {
        let pat = format!("%{t}%");
        params.push(pat.clone());
        params.push(pat);
    }
    let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut scored: Vec<(i64, f64, String, String)> = Vec::new();
    for row in rows {
        let (id, category, content, tags) = row?;
        // idf 重みは常に有限・正値 (compute_idf_weights 参照)。一致 token が無ければ
        // sum() == 0.0 となり下の guard で除外される (NaN は発生しない)。
        let score: f64 = tokens
            .iter()
            .zip(idf.iter())
            .filter(|(t, _)| content.contains(t.as_str()) || tags.contains(t.as_str()))
            .map(|(_, w)| *w)
            .sum();
        if score > 0.0 {
            scored.push((id, score, category, content));
        }
    }
    // (score desc, id desc) で安定ソート。score は f64 なので total_cmp。
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    scored.truncate(limit);
    Ok(scored
        .into_iter()
        .map(|(_id, _score, cat, content)| (cat, content))
        .collect())
}

/// 各 token の IDF 重み `ln((N+1)/(df+1)) + 1.0` を返す (token と同順)。
/// N = memories 総数、df = その token を content/tags に含む memory 数。
/// 平滑化 (+1) により df=0 や N=0 でも有限・正値を保つ。頻出語ほど重みが小さくなる。
fn compute_idf_weights(conn: &rusqlite::Connection, tokens: &[String]) -> Result<Vec<f64>> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
    let n = n as f64;
    let mut weights = Vec::with_capacity(tokens.len());
    for t in tokens {
        let pat = format!("%{t}%");
        let df: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE content LIKE ?1 OR tags LIKE ?1",
            rusqlite::params![pat],
            |row| row.get(0),
        )?;
        let idf = ((n + 1.0) / (df as f64 + 1.0)).ln() + 1.0;
        weights.push(idf);
    }
    Ok(weights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;

    /// 一時 DB ファイルパスを生成(プロセス内ユニーク、file-backed)。
    /// nanos のみでは並列テストで時刻衝突し同一 DB を共有→ SQL ロックで flaky 化するため、
    /// プロセス単調増加 atomic counter を併用して衝突を排除する。
    fn temp_db_path() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir();
        let unique = format!(
            "bonsai_mem_tool_test_{}_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            seq
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
        r.call(serde_json::json!({"content": "apple only"}))
            .unwrap();
        r.call(serde_json::json!({"content": "apple and banana together"}))
            .unwrap();
        r.call(serde_json::json!({"content": "banana only"}))
            .unwrap();
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
        r.call(serde_json::json!({"content": "金曜日は会議"}))
            .unwrap();
        r.call(serde_json::json!({"content": "金曜日に締切がある会議"}))
            .unwrap();
        r.call(serde_json::json!({"content": "土曜日に予定"}))
            .unwrap();
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
    fn t_tokenize_recall_query_dedup() {
        // 重複 token はスコア増幅を防ぐため除去される。
        let toks = tokenize_recall_query("apple apple banana");
        assert_eq!(toks.len(), 2, "重複除去後 2 token: {toks:?}");
        assert!(toks.contains(&"apple".to_string()));
        assert!(toks.contains(&"banana".to_string()));
        // 空/空白のみクエリは空配列。
        assert!(tokenize_recall_query("").is_empty());
        assert!(tokenize_recall_query("   ").is_empty());
    }

    #[test]
    fn t_recall_idf_ranks_rare_token_higher() {
        // IDF 重み付け: 稀な語の一致を頻出語の一致より高評価する。
        // raw count では全候補 score=1 で同点 → id desc tiebreak で common 側 (後挿入) が上位になる。
        // IDF では rareword (df=1) >> common (df=6) のため rareword 保有 chunk が上位に来るべき。
        let path = temp_db_path();
        let r = RememberTool::new(&path);
        // id=1: rareword 保有 (df=1、最古)
        r.call(serde_json::json!({"content": "rareword beta"}))
            .unwrap();
        // id=2..=6: common を 5 件 (df を押し上げる)
        for i in 0..5 {
            r.call(serde_json::json!({"content": format!("common filler {i}")}))
                .unwrap();
        }
        // id=7: common 保有 (最新 = id desc なら raw 同点で先頭)
        r.call(serde_json::json!({"content": "common gamma"}))
            .unwrap();

        let recall = RecallTool::new(&path)
            .call(serde_json::json!({"query": "common rareword", "limit": 3}))
            .expect("recall 成功");
        let out = &recall.output;
        let pos_rare = out.find("rareword beta").unwrap_or(usize::MAX);
        let pos_common = out.find("common gamma").unwrap_or(usize::MAX);
        assert!(
            pos_rare < pos_common,
            "稀な語 rareword 保有 chunk が頻出語 common chunk より上位に来るべき (IDF): {out}"
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
