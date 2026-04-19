use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::{HashSet, VecDeque};

/// グラフ構造の連想記憶。エンティティ間の関係をSQLiteで保持し、
/// N階隣接探索でコンテキストを生成する。
/// Agentic Engram知見: 「ファイルA→修正パターンB→ツールC」のような
/// 関係グラフにより、1ビットモデルの限られたコンテキストで精度の高い想起を実現。
pub struct KnowledgeGraph<'a> {
    conn: &'a Connection,
}

impl<'a> KnowledgeGraph<'a> {
    /// SQLiteコネクションを受け取って初期化
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// ノードを追加（UPSERT: 既存ならIDを返す）
    pub fn add_node(&self, node_type: &str, name: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO knowledge_nodes (node_type, name) VALUES (?1, ?2) \
             ON CONFLICT(name) DO UPDATE SET node_type = excluded.node_type",
            params![node_type, name],
        )?;
        let id = self.conn.query_row(
            "SELECT id FROM knowledge_nodes WHERE name = ?1",
            params![name],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(id)
    }

    /// エッジを追加（UPSERT: 既存ならweight加算）
    pub fn add_edge(
        &self,
        source_id: i64,
        target_id: i64,
        relation: &str,
        weight: f64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO knowledge_edges (source_id, target_id, relation, weight) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(source_id, target_id, relation) \
             DO UPDATE SET weight = knowledge_edges.weight + excluded.weight",
            params![source_id, target_id, relation, weight],
        )?;
        Ok(())
    }

    /// N階隣接ノードを取得。(ノード名, 関係タイプ, 重み)のリストを返す。
    /// depth=1で直接隣接、depth=2で2ホップ先まで探索。
    pub fn neighbors(&self, name: &str, depth: u32) -> Result<Vec<(String, String, f64)>> {
        let start_id = self.conn.query_row(
            "SELECT id FROM knowledge_nodes WHERE name = ?1",
            params![name],
            |row| row.get::<_, i64>(0),
        );
        let start_id = match start_id {
            Ok(id) => id,
            Err(_) => return Ok(Vec::new()),
        };

        let mut visited: HashSet<i64> = HashSet::new();
        visited.insert(start_id);
        let mut queue: VecDeque<(i64, u32)> = VecDeque::new();
        queue.push_back((start_id, 0));
        let mut results: Vec<(String, String, f64)> = Vec::new();

        while let Some((current_id, current_depth)) = queue.pop_front() {
            if current_depth >= depth {
                continue;
            }
            // 出方向エッジ
            let mut stmt = self.conn.prepare(
                "SELECT kn.id, kn.name, ke.relation, ke.weight \
                 FROM knowledge_edges ke \
                 JOIN knowledge_nodes kn ON kn.id = ke.target_id \
                 WHERE ke.source_id = ?1",
            )?;
            let outgoing = stmt.query_map(params![current_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            })?;
            for edge in outgoing {
                let (neighbor_id, neighbor_name, relation, weight) = edge?;
                if !visited.contains(&neighbor_id) {
                    visited.insert(neighbor_id);
                    results.push((neighbor_name, relation, weight));
                    queue.push_back((neighbor_id, current_depth + 1));
                }
            }
            // 入方向エッジ（双方向探索）
            let mut stmt_in = self.conn.prepare(
                "SELECT kn.id, kn.name, ke.relation, ke.weight \
                 FROM knowledge_edges ke \
                 JOIN knowledge_nodes kn ON kn.id = ke.source_id \
                 WHERE ke.target_id = ?1",
            )?;
            let incoming = stmt_in.query_map(params![current_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            })?;
            for edge in incoming {
                let (neighbor_id, neighbor_name, relation, weight) = edge?;
                if !visited.contains(&neighbor_id) {
                    visited.insert(neighbor_id);
                    results.push((neighbor_name, relation, weight));
                    queue.push_back((neighbor_id, current_depth + 1));
                }
            }
        }

        // 重みの降順でソート（関連度の高い順）
        results.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        Ok(results)
    }

    /// クエリに関連するノードをたどってコンテキスト文字列を生成。
    /// 1ビットモデルのプロンプト注入に最適化された簡潔なフォーマット。
    pub fn related_context(&self, query: &str, max_results: usize) -> Result<String> {
        let neighbors = self.neighbors(query, 2)?;
        if neighbors.is_empty() {
            return Ok(String::new());
        }
        let limited: Vec<_> = neighbors.into_iter().take(max_results).collect();
        let mut lines: Vec<String> = Vec::new();
        for (name, relation, weight) in &limited {
            lines.push(format!("- {query} --[{relation}({weight:.1})]--> {name}"));
        }
        Ok(lines.join("\n"))
    }

    /// ツール使用→ファイル関係を記録（ツール成功時に呼ぶ）
    pub fn record_tool_usage(&self, tool_name: &str, file_path: &str) -> Result<()> {
        let tool_id = self.add_node("tool", tool_name)?;
        let file_id = self.add_node("file", file_path)?;
        self.add_edge(tool_id, file_id, "uses", 1.0)?;
        Ok(())
    }

    /// エラー→ファイル→ツール関係を記録（エラー発生時に呼ぶ）
    pub fn record_error_pattern(
        &self,
        error_type: &str,
        file_path: &str,
        tool_name: &str,
    ) -> Result<()> {
        let error_id = self.add_node("error", error_type)?;
        let file_id = self.add_node("file", file_path)?;
        let tool_id = self.add_node("tool", tool_name)?;
        self.add_edge(error_id, file_id, "caused_by", 1.0)?;
        self.add_edge(tool_id, file_id, "fixes", 1.0)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::MIGRATIONS;

    /// テスト用のインメモリDBを全マイグレーション適用済みで返す
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        for m in MIGRATIONS {
            conn.execute_batch(m.sql).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_version (version) VALUES (?1)",
                params![m.version],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn test_add_node_returns_id() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let id = graph.add_node("file", "src/main.rs").unwrap();
        assert!(id > 0);
    }

    #[test]
    fn test_add_node_upsert_returns_same_id() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let id1 = graph.add_node("file", "src/main.rs").unwrap();
        let id2 = graph.add_node("file", "src/main.rs").unwrap();
        assert_eq!(id1, id2, "UPSERT時は同じIDが返るべき");
    }

    #[test]
    fn test_add_node_different_names_different_ids() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let id1 = graph.add_node("file", "src/main.rs").unwrap();
        let id2 = graph.add_node("tool", "shell").unwrap();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_add_edge_creates_relation() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let src = graph.add_node("tool", "shell").unwrap();
        let tgt = graph.add_node("file", "src/main.rs").unwrap();
        graph.add_edge(src, tgt, "uses", 1.0).unwrap();

        let neighbors = graph.neighbors("shell", 1).unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].0, "src/main.rs");
        assert_eq!(neighbors[0].1, "uses");
        assert!((neighbors[0].2 - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_add_edge_upsert_accumulates_weight() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let src = graph.add_node("tool", "shell").unwrap();
        let tgt = graph.add_node("file", "src/main.rs").unwrap();
        graph.add_edge(src, tgt, "uses", 1.0).unwrap();
        graph.add_edge(src, tgt, "uses", 2.5).unwrap();

        let neighbors = graph.neighbors("shell", 1).unwrap();
        assert_eq!(neighbors.len(), 1);
        assert!(
            (neighbors[0].2 - 3.5).abs() < f64::EPSILON,
            "weightが加算されるべき"
        );
    }

    #[test]
    fn test_neighbors_depth_2() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let a = graph.add_node("tool", "shell").unwrap();
        let b = graph.add_node("file", "src/main.rs").unwrap();
        let c = graph.add_node("pattern", "edit_compile_check").unwrap();
        graph.add_edge(a, b, "uses", 1.0).unwrap();
        graph.add_edge(b, c, "depends_on", 2.0).unwrap();

        // depth=1: shellからmain.rsのみ
        let n1 = graph.neighbors("shell", 1).unwrap();
        assert_eq!(n1.len(), 1);

        // depth=2: shellからmain.rs + edit_compile_check
        let n2 = graph.neighbors("shell", 2).unwrap();
        assert_eq!(n2.len(), 2);
    }

    #[test]
    fn test_neighbors_unknown_node_returns_empty() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let result = graph.neighbors("nonexistent", 1).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_related_context_generates_string() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let a = graph.add_node("tool", "shell").unwrap();
        let b = graph.add_node("file", "src/main.rs").unwrap();
        graph.add_edge(a, b, "uses", 3.0).unwrap();

        let ctx = graph.related_context("shell", 5).unwrap();
        assert!(ctx.contains("shell"));
        assert!(ctx.contains("src/main.rs"));
        assert!(ctx.contains("uses"));
    }

    #[test]
    fn test_related_context_empty_for_unknown() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let ctx = graph.related_context("nonexistent", 5).unwrap();
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_related_context_respects_max_results() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let center = graph.add_node("tool", "center").unwrap();
        for i in 0..10 {
            let n = graph.add_node("file", &format!("file_{i}.rs")).unwrap();
            graph.add_edge(center, n, "uses", 1.0).unwrap();
        }
        let ctx = graph.related_context("center", 3).unwrap();
        let line_count = ctx.lines().count();
        assert_eq!(line_count, 3, "max_resultsで制限されるべき");
    }

    #[test]
    fn test_record_tool_usage() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        graph.record_tool_usage("file_write", "src/lib.rs").unwrap();

        let neighbors = graph.neighbors("file_write", 1).unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].0, "src/lib.rs");
        assert_eq!(neighbors[0].1, "uses");
    }

    #[test]
    fn test_record_error_pattern() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        graph
            .record_error_pattern("borrow_error", "src/agent.rs", "shell")
            .unwrap();

        // エラーからファイルへのcaused_byエッジ
        let error_neighbors = graph.neighbors("borrow_error", 1).unwrap();
        assert!(
            error_neighbors
                .iter()
                .any(|(name, rel, _)| name == "src/agent.rs" && rel == "caused_by")
        );

        // ツールからファイルへのfixesエッジ
        let tool_neighbors = graph.neighbors("shell", 1).unwrap();
        assert!(
            tool_neighbors
                .iter()
                .any(|(name, rel, _)| name == "src/agent.rs" && rel == "fixes")
        );
    }

    #[test]
    fn test_record_tool_usage_accumulates() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        graph.record_tool_usage("shell", "src/main.rs").unwrap();
        graph.record_tool_usage("shell", "src/main.rs").unwrap();
        graph.record_tool_usage("shell", "src/main.rs").unwrap();

        let neighbors = graph.neighbors("shell", 1).unwrap();
        assert_eq!(neighbors.len(), 1);
        assert!(
            (neighbors[0].2 - 3.0).abs() < f64::EPSILON,
            "3回呼び出しでweight=3.0"
        );
    }

    #[test]
    fn test_bidirectional_traversal() {
        let conn = setup_db();
        let graph = KnowledgeGraph::new(&conn);
        let a = graph.add_node("tool", "shell").unwrap();
        let b = graph.add_node("file", "src/main.rs").unwrap();
        graph.add_edge(a, b, "uses", 1.0).unwrap();

        // ファイル側からもツールが見える（双方向探索）
        let neighbors = graph.neighbors("src/main.rs", 1).unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].0, "shell");
    }
}
