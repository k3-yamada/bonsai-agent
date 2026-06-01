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
        // (CJK 後方互換を維持)。クエリを script 境界で分割し ASCII 語 + CJK bigram に
        // トークン化 (`tokenize_recall_query`)、各 token を OR で union 検索 → Rust 側で
        // IDF 重み付き overlap スコアリング (score desc、同点は id desc) で関連度を
        // 擬似 hybrid 風に近似する。CJK bigram 化で助詞膠着クエリも想起可 (項目 271)。
        let tokens = tokenize_recall_query(&args.query);
        let hits = recall_scored(store.conn(), &tokens, limit)?;
        if hits.is_empty() {
            return Ok(ToolResult {
                output: format!("「{}」に該当する記憶なし", args.query),
                success: true,
            });
        }
        // 長大 content は match 周辺の snippet に短縮 (context 圧迫防止)。
        let tokens_lc: Vec<String> = tokens.iter().map(|t| t.to_lowercase()).collect();
        let mut o = format!("{}件の記憶:\n", hits.len());
        for (category, content, tags) in &hits {
            let display = make_snippet(content, &tokens_lc, RECALL_SNIPPET_MAX_CHARS);
            // ingest chunk は出典ファイル名を併記し provenance を与える (ccg gemini 推奨)。
            // 非 ingest (remember fact 等) の tag は topical ラベルのため出典化しない。
            match (category.as_str(), source_filename(tags)) {
                ("ingest", Some(src)) => {
                    o.push_str(&format!("- [{category}] {display} (出典: {src})\n"))
                }
                _ => o.push_str(&format!("- [{category}] {display}\n")),
            }
        }
        Ok(ToolResult {
            output: o,
            success: true,
        })
    }
}

/// recall 出力 1 件あたりの最大表示文字数。これを超える content は
/// match 周辺の snippet に短縮し、context 圧迫と読み疲れを抑える (ccg gemini snippet)。
const RECALL_SNIPPET_MAX_CHARS: usize = 160;

/// `haystack` 中に `needle` の char 列が最初に現れる開始 char index を返す。
/// 全て char 空間で比較するため byte/char position のズレが生じない。
fn find_char_subsequence(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == *needle)
}

/// content が `max_chars` を超える場合、最初に一致した token 周辺の snippet に短縮する。
/// char 単位でスライスし UTF-8 境界を壊さない。前後を省略した側に `…` を付す。
/// 一致語が無い (tag のみ一致) 場合は先頭から `max_chars` を切り出す。
///
/// 一致位置探索は char 空間 + `to_ascii_lowercase` (常に 1:1) で行うため、
/// `to_lowercase()` が一部 Unicode を伸長 (ß→ss 等) して byte/char index がズレる
/// 問題を回避する (ecc review MEDIUM)。token は ASCII/CJK 主体で本変換と整合する。
fn make_snippet(content: &str, tokens_lc: &[String], max_chars: usize) -> String {
    let chars: Vec<char> = content.chars().collect();
    if chars.len() <= max_chars {
        return content.to_string();
    }
    let lc_chars: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let match_char_idx = tokens_lc
        .iter()
        .filter_map(|t| {
            let needle: Vec<char> = t.chars().map(|c| c.to_ascii_lowercase()).collect();
            find_char_subsequence(&lc_chars, &needle)
        })
        .min()
        .unwrap_or(0);
    // match を窓の前方 1/3 付近に置く。末尾寄りなら start を巻き戻して窓幅を保つ。
    let lead = max_chars / 3;
    let mut start = match_char_idx.saturating_sub(lead);
    let mut end = (start + max_chars).min(chars.len());
    start = end.saturating_sub(max_chars);
    end = (start + max_chars).min(chars.len());
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.extend(&chars[start..end]);
    if end < chars.len() {
        s.push('…');
    }
    s
}

/// tags (JSON 配列文字列、例 `["notes.md"]`) から出典ファイル名 = 先頭要素を取り出す。
/// parse 不能/空配列なら None。ingest chunk の provenance 表示に用いる。
fn source_filename(tags: &str) -> Option<String> {
    serde_json::from_str::<Vec<String>>(tags)
        .ok()
        .and_then(|v| v.into_iter().next())
        .filter(|s| !s.is_empty())
}

/// 文字を ASCII 英数字 / CJK / 区切り の 3 クラスに分類する。
/// CJK は ひらがな・カタカナ・漢字 (拡張 A + 互換漢字) を 1 クラスに統合し、
/// 「使い方」のような漢字+ひらがな混在語を 1 run として扱えるようにする。
#[derive(Debug, PartialEq, Clone, Copy)]
enum CharClass {
    Ascii,
    Cjk,
    Sep,
}

fn classify_char(c: char) -> CharClass {
    if c.is_ascii_alphanumeric() {
        CharClass::Ascii
    } else if matches!(c as u32,
        0x3040..=0x309F |   // ひらがな
        0x30A0..=0x30FF |   // カタカナ
        0x31F0..=0x31FF |   // カタカナ音声拡張
        0x3400..=0x4DBF |   // CJK 拡張 A
        0x4E00..=0x9FFF |   // CJK 統合漢字
        0xF900..=0xFAFF |   // CJK 互換漢字
        0xFF65..=0xFF9F |   // 半角カタカナ (legacy data)
        0x20000..=0x2A6DF | // CJK 拡張 B
        0x2A700..=0x2EE5F | // CJK 拡張 C-F, I
        0x2F800..=0x2FA1F   // CJK 互換漢字補助
    ) {
        CharClass::Cjk
    } else {
        CharClass::Sep
    }
}

/// recall クエリ 1 件あたりの最大 token 数。長大 CJK クエリ (貼付け文等) が
/// 過剰な bigram → SQL LIKE param 膨張 + IDF の N+1 クエリを誘発するのを防ぐ
/// (user 入力境界のガード、ecc review MEDIUM)。通常の想起クエリには十分な上限。
const MAX_RECALL_TOKENS: usize = 32;

/// クエリを Unicode script 境界で分割し、検索 token 配列を返す (重複・空除去)。
///
/// - ASCII 英数字 run → 語 token (例 "API"、"config")。
/// - CJK run (長さ >= 2) → overlapping bigram (例 "使い方" → ["使い","い方"])。
/// - CJK run (長さ 1) → unigram 保持 (例 "鍵" → ["鍵"])。
/// - 区切り (空白・記号・CJK 句読点) → 分割のみ。
///
/// 日本語は分かち書きしないため、助詞 (の/を/は…) が content 語に膠着すると
/// 旧 whitespace 分割では LIKE 一致しなかった。bigram 化で content 語片に
/// 部分一致させ、汎用的な橋渡し bigram (の使 等) は corpus-wide IDF (`compute_idf_weights`)
/// が down-weight する。辞書/形態素解析器を持たず最小依存で日本語想起を改善する
/// (ccg codex/gemini 双方が Option A = CJK bigram を最適と評価、Phase 5、項目 271)。
fn tokenize_recall_query(query: &str) -> Vec<String> {
    fn push_unique(
        tok: String,
        seen: &mut std::collections::HashSet<String>,
        out: &mut Vec<String>,
    ) {
        if out.len() < MAX_RECALL_TOKENS && !tok.is_empty() && seen.insert(tok.clone()) {
            out.push(tok);
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let chars: Vec<char> = query.chars().collect();
    let mut i = 0;
    while i < chars.len() && out.len() < MAX_RECALL_TOKENS {
        let cls = classify_char(chars[i]);
        let start = i;
        while i < chars.len() && classify_char(chars[i]) == cls {
            i += 1;
        }
        match cls {
            CharClass::Ascii => {
                push_unique(chars[start..i].iter().collect(), &mut seen, &mut out);
            }
            CharClass::Cjk => {
                let run = &chars[start..i];
                if run.len() == 1 {
                    push_unique(run[0].to_string(), &mut seen, &mut out);
                } else {
                    for w in run.windows(2) {
                        push_unique(w.iter().collect(), &mut seen, &mut out);
                    }
                }
            }
            CharClass::Sep => {}
        }
    }
    out
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
) -> Result<Vec<(String, String, String)>> {
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
    // token を小文字化して保持 (SQL LIKE の ASCII 大小無視と scoring を整合させる)。
    // CJK の to_lowercase は no-op のため CJK 一致挙動は不変。
    let tokens_lc: Vec<String> = tokens.iter().map(|t| t.to_lowercase()).collect();
    let mut scored: Vec<(i64, f64, String, String, String)> = Vec::new();
    for row in rows {
        let (id, category, content, tags) = row?;
        // 行ごとに content/tags を 1 度だけ小文字化して case-insensitive 比較する。
        let content_lc = content.to_lowercase();
        let tags_lc = tags.to_lowercase();
        // idf 重みは常に有限・正値 (compute_idf_weights 参照)。一致 token が無ければ
        // sum() == 0.0 となり下の guard で除外される (NaN は発生しない)。
        let score: f64 = tokens_lc
            .iter()
            .zip(idf.iter())
            .filter(|(t, _)| content_lc.contains(t.as_str()) || tags_lc.contains(t.as_str()))
            .map(|(_, w)| *w)
            .sum();
        if score > 0.0 {
            scored.push((id, score, category, content, tags));
        }
    }
    // (score desc, id desc) で安定ソート。score は f64 なので total_cmp。
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    // 同一 content の重複を除去 (ソート後なので高スコア側を残す)。
    // 別 source に同一段落がある場合に limit 枠を浪費せず context 圧迫を防ぐ (ccg dedup)。
    let mut seen_content = std::collections::HashSet::new();
    scored.retain(|(_, _, _, content, _)| seen_content.insert(content.clone()));
    scored.truncate(limit);
    Ok(scored
        .into_iter()
        .map(|(_id, _score, cat, content, tags)| (cat, content, tags))
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

    // ---------- Phase 5 Red: CJK bigram tokenization (助詞非分離の根治) ----------

    #[test]
    fn t_tokenize_cjk_bigram() {
        // CJK run は overlapping bigram、ASCII run は語 token、script 境界で分割。
        // 旧 tokenizer (whitespace 分割のみ) では "設定の使い方" は 1 token のまま → Red。
        assert_eq!(
            tokenize_recall_query("設定の使い方"),
            vec!["設定", "定の", "の使", "使い", "い方"],
            "CJK run は overlapping bigram 化されるべき"
        );
        // ASCII↔CJK script 境界で分割 (混在 token を解体)。
        assert_eq!(
            tokenize_recall_query("API設定"),
            vec!["API", "設定"],
            "script 境界で ASCII run と CJK run に分割されるべき"
        );
        // 単一 CJK char は unigram として保持 (drop しない)。
        assert_eq!(
            tokenize_recall_query("鍵"),
            vec!["鍵"],
            "単一 CJK char は unigram 保持"
        );
        // 空白区切り + CJK 複合: ASCII 語 + 各 CJK run の bigram。
        assert_eq!(
            tokenize_recall_query("config 設定方法"),
            vec!["config", "設定", "定方", "方法"],
            "ASCII 語と CJK bigram の混在"
        );
    }

    #[test]
    fn t_tokenize_halfwidth_katakana() {
        // 半角カタカナも CJK として bigram 化される (legacy data 対応、ecc review MEDIUM)。
        // 旧 range では FF65-FF9F が Sep 扱いで全 drop → Red。
        let toks = tokenize_recall_query("ｱﾌﾟﾘ");
        assert!(
            toks.contains(&"ｱﾌ".to_string()),
            "半角カナ run が bigram 化されるべき: {toks:?}"
        );
    }

    #[test]
    fn t_tokenize_token_cap() {
        // 長大 CJK クエリでも token 数を上限で抑え SQL param 膨張を防ぐ (ecc review MEDIUM)。
        // 100 連続の相異なる漢字 → 99 distinct bigram → cap で 32 以下に (旧実装 cap なし = Red)。
        let long_q: String = (0x4E00u32..0x4E00 + 100)
            .filter_map(char::from_u32)
            .collect();
        let toks = tokenize_recall_query(&long_q);
        assert!(
            toks.len() <= 32,
            "token 数は上限以下に抑えるべき: {}",
            toks.len()
        );
    }

    #[test]
    fn t_recall_cjk_particle_glued_query() {
        // 実利用: 助詞で連結されたクエリ「金曜の締切」(空白なし) を想起できる。
        // 旧 tokenizer は ["金曜の締切"] 1 token → LIKE %金曜の締切% は
        // 語順/助詞が異なる content に一致せず 0 件 (Red)。
        // bigram 化で "金曜"/"締切" が content に一致 (Green)。
        let path = temp_db_path();
        let r = RememberTool::new(&path);
        r.call(serde_json::json!({"content": "重要な締切は金曜に設定されている"}))
            .unwrap();
        r.call(serde_json::json!({"content": "土曜は休みの予定"}))
            .unwrap();
        let recall = RecallTool::new(&path)
            .call(serde_json::json!({"query": "金曜の締切", "limit": 5}))
            .expect("recall 成功");
        assert!(
            recall.output.contains("重要な締切は金曜"),
            "助詞連結クエリでも content 語の bigram 一致で想起すべき: {}",
            recall.output
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn t_recall_dedups_identical_content() {
        // 同一 content の重複 hit は 1 件に集約される (ccg gemini dedup、limit 枠浪費防止)。
        let path = temp_db_path();
        let r = RememberTool::new(&path);
        r.call(serde_json::json!({"content": "重複する知識ブロック", "category": "ingest", "tags": ["a.md"]}))
            .unwrap();
        r.call(serde_json::json!({"content": "重複する知識ブロック", "category": "ingest", "tags": ["b.md"]}))
            .unwrap();
        r.call(serde_json::json!({"content": "別の内容ブロック", "category": "ingest", "tags": ["c.md"]}))
            .unwrap();
        let recall = RecallTool::new(&path)
            .call(serde_json::json!({"query": "ブロック", "limit": 10}))
            .expect("recall 成功");
        let count = recall.output.matches("重複する知識ブロック").count();
        assert_eq!(
            count, 1,
            "同一 content は 1 件に集約されるべき: {}",
            recall.output
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn t_recall_snippets_long_content() {
        // 長い content は match 周辺の snippet に短縮される (ccg gemini snippet)。
        let path = temp_db_path();
        let long = format!("{}キーワード{}", "あ".repeat(200), "い".repeat(200));
        RememberTool::new(&path)
            .call(serde_json::json!({"content": long, "category": "ingest", "tags": ["big.md"]}))
            .unwrap();
        let r = RecallTool::new(&path)
            .call(serde_json::json!({"query": "キーワード"}))
            .expect("recall 成功");
        assert!(
            r.output.contains("キーワード"),
            "snippet は match 語を含むべき: {}",
            r.output
        );
        assert!(r.output.contains('…'), "短縮は省略記号で示すべき");
        assert!(
            !r.output.contains(&"あ".repeat(200)),
            "全文ではなく snippet 化されるべき"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn t_recall_includes_source_provenance() {
        // ingest chunk の recall 出力に出典ファイル名が含まれる (provenance、ccg gemini 推奨)。
        // 旧出力は `- [category] content` のみで出典なし → Red。
        let path = temp_db_path();
        RememberTool::new(&path)
            .call(serde_json::json!({
                "content": "Rust の所有権は重要な概念",
                "category": "ingest",
                "tags": ["rust_guide.md"]
            }))
            .unwrap();
        let r = RecallTool::new(&path)
            .call(serde_json::json!({"query": "所有権"}))
            .expect("recall 成功");
        assert!(
            r.output.contains("rust_guide.md"),
            "ingest chunk は出典ファイル名を表示すべき: {}",
            r.output
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn t_recall_non_ingest_no_source_label() {
        // ingest 以外 (remember fact 等) は出典ラベルを付けない (topical tag を誤って出典化しない)。
        let path = temp_db_path();
        RememberTool::new(&path)
            .call(serde_json::json!({
                "content": "締切は金曜日",
                "tags": ["deadline"]
            }))
            .unwrap();
        let r = RecallTool::new(&path)
            .call(serde_json::json!({"query": "締切"}))
            .expect("recall 成功");
        assert!(
            !r.output.contains("出典"),
            "非 ingest memory に出典ラベルを付けないべき: {}",
            r.output
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn t_recall_ascii_case_insensitive() {
        // ASCII クエリは大小無視で想起する (SQL LIKE と scoring の整合、ecc finding)。
        // content 小文字 / query 大文字 → 旧 contains() は case-sensitive で
        // SQL LIKE が返した行を scoring が落とし 0 件 (Red)。
        let path = temp_db_path();
        RememberTool::new(&path)
            .call(serde_json::json!({"content": "claude code は便利"}))
            .unwrap();
        let r = RecallTool::new(&path)
            .call(serde_json::json!({"query": "CLAUDE"}))
            .expect("recall 成功");
        assert!(
            r.output.contains("claude code は便利"),
            "ASCII 大小無視で想起すべき: {}",
            r.output
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
