//! 統合記憶および知識グラフのエクスポート＆可視化用スナップショット生成
//!
//! 第7章「可視化層への投資」に基づき、A-MEM（長期記憶・連想リンク）と
//! Knowledge Graph（エンティティ・リレーション）を統合したグラフスナップショットを生成する。
//! プライバシー規律 (7.5) に従い、APIキー・秘密変数・パスワード等の機微情報を自動サニタイズする。

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::memory::store::MemoryStore;
use crate::safety::secrets::SecretsFilter;
use crate::safety::sensor_filter::PrivacyFilter;

/// グラフ可視化用の個別ノード
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub category: String,
    pub details: String,
    pub tags: Vec<String>,
    pub weight: f64,
}

/// グラフ可視化用の関係エッジ
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub weight: f64,
}

/// メモリおよびナレッジの全体グラフスナップショット
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphSnapshot {
    pub exported_at: String,
    pub total_memories: usize,
    pub total_entities: usize,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl GraphSnapshot {
    /// JSON 文字列としてシリアライズ
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| anyhow::anyhow!(e))
    }
}

/// メモリストアからノードおよびエッジを抽出し、GraphSnapshot を構築する。
///
/// `filter_privacy` が true の場合、SecretsFilter および PrivacyFilter による
/// 機微情報のマスキング（APIキー、トークン、秘密変数名等）が実行される。
pub fn export_memory_graph(store: &MemoryStore, filter_privacy: bool) -> Result<GraphSnapshot> {
    let conn = store.conn();
    let secrets_filter = if filter_privacy {
        Some(SecretsFilter::default_patterns())
    } else {
        None
    };
    let privacy_filter = if filter_privacy {
        Some(PrivacyFilter::new())
    } else {
        None
    };

    let sanitize = |text: &str| -> String {
        if let Some(ref sf) = secrets_filter {
            let mut s = sf.redact(text);
            if let Some(ref pf) = privacy_filter {
                for keyword in [
                    ".env",
                    "password",
                    "token",
                    "secret",
                    "credentials",
                    "id_rsa",
                ] {
                    if s.to_lowercase().contains(keyword) {
                        s = s.replace(keyword, "***REDACTED***");
                    }
                }
                let _ = pf;
            }
            s
        } else {
            text.to_string()
        }
    };

    let make_label = |text: &str| -> String {
        let clean = text.lines().next().unwrap_or("").trim();
        if clean.chars().count() > 36 {
            let prefix: String = clean.chars().take(33).collect();
            format!("{}...", prefix)
        } else if clean.is_empty() {
            "(empty)".to_string()
        } else {
            clean.to_string()
        }
    };

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut node_degree: HashMap<String, usize> = HashMap::new();

    // 1. A-MEM メモリの抽出
    let mut stmt =
        conn.prepare("SELECT id, content, category, tags FROM memories ORDER BY id ASC")?;
    let mut mem_rows = stmt.query([])?;
    let mut mem_count = 0;

    while let Some(row) = mem_rows.next()? {
        let id: i64 = row.get(0)?;
        let raw_content: String = row.get(1)?;
        let raw_category: String = row.get(2)?;
        let tags_str: String = row.get(3)?;

        let sanitized_content = sanitize(&raw_content);
        let label = make_label(&sanitized_content);
        let node_id = format!("mem_{}", id);

        let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();

        nodes.push(GraphNode {
            id: node_id.clone(),
            label,
            category: if raw_category.is_empty() {
                "memory".to_string()
            } else {
                raw_category
            },
            details: sanitized_content,
            tags,
            weight: 1.0,
        });
        node_degree.insert(node_id, 0);
        mem_count += 1;
    }

    // 2. メモリ間連想リンク (memory_links) の抽出
    let mut stmt = conn.prepare("SELECT source_id, target_id, relation FROM memory_links")?;
    let mut link_rows = stmt.query([])?;
    while let Some(row) = link_rows.next()? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let relation: String = row.get(2)?;

        let source = format!("mem_{}", source_id);
        let target = format!("mem_{}", target_id);

        *node_degree.entry(source.clone()).or_insert(0) += 1;
        *node_degree.entry(target.clone()).or_insert(0) += 1;

        edges.push(GraphEdge {
            source,
            target,
            relation,
            weight: 1.0,
        });
    }

    // 3. ナレッジグラフ (knowledge_nodes) の抽出
    let mut stmt =
        conn.prepare("SELECT id, node_type, name FROM knowledge_nodes ORDER BY id ASC")?;
    let mut kg_node_rows = stmt.query([])?;
    let mut entity_count = 0;

    while let Some(row) = kg_node_rows.next()? {
        let id: i64 = row.get(0)?;
        let node_type: String = row.get(1)?;
        let raw_name: String = row.get(2)?;

        let sanitized_name = sanitize(&raw_name);
        let label = make_label(&sanitized_name);
        let node_id = format!("kg_{}", id);

        nodes.push(GraphNode {
            id: node_id.clone(),
            label,
            category: format!("kg_{}", node_type),
            details: sanitized_name,
            tags: vec![node_type],
            weight: 1.0,
        });
        node_degree.insert(node_id, 0);
        entity_count += 1;
    }

    // 4. ナレッジグラフエッジ (knowledge_edges) の抽出
    let mut stmt =
        conn.prepare("SELECT source_id, target_id, relation, weight FROM knowledge_edges")?;
    let mut kg_edge_rows = stmt.query([])?;
    while let Some(row) = kg_edge_rows.next()? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let relation: String = row.get(2)?;
        let weight: f64 = row.get(3)?;

        let source = format!("kg_{}", source_id);
        let target = format!("kg_{}", target_id);

        *node_degree.entry(source.clone()).or_insert(0) += 1;
        *node_degree.entry(target.clone()).or_insert(0) += 1;

        edges.push(GraphEdge {
            source,
            target,
            relation,
            weight,
        });
    }

    // 5. ノードの weight に次数 (degree) を反映 (1.0 + ln(1 + degree))
    for node in &mut nodes {
        let deg = node_degree.get(&node.id).copied().unwrap_or(0);
        node.weight = 1.0 + ((deg + 1) as f64).ln();
    }

    Ok(GraphSnapshot {
        exported_at: Utc::now().to_rfc3339(),
        total_memories: mem_count,
        total_entities: entity_count,
        nodes,
        edges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::graph::KnowledgeGraph;

    #[test]
    fn test_export_empty_memory_graph() {
        let store = MemoryStore::in_memory().expect("in memory store init");
        let snapshot = export_memory_graph(&store, true).expect("export graph");
        assert_eq!(snapshot.total_memories, 0);
        assert_eq!(snapshot.total_entities, 0);
        assert!(snapshot.nodes.is_empty());
        assert!(snapshot.edges.is_empty());

        let json = snapshot.to_json().expect("to_json");
        assert!(json.contains("\"nodes\": []"));
        assert!(json.contains("\"edges\": []"));
    }

    #[test]
    fn test_export_memory_graph_with_links_and_kg() {
        let store = MemoryStore::in_memory().expect("in memory store init");

        // A-MEM メモリを保存してリンクを作成
        let id1 = store
            .save_memory("Rust is memory safe", "fact", &["rust".to_string()])
            .expect("save 1");
        let id2 = store
            .save_memory(
                "Borrow checker prevents data races",
                "rule",
                &["rust".to_string()],
            )
            .expect("save 2");
        store
            .link_memories(id1, id2, "explains")
            .expect("link memories");

        // KnowledgeGraph ノードとエッジを作成
        {
            let kg = KnowledgeGraph::new(store.conn());
            let kg1 = kg.add_node("language", "Rust").expect("add kg 1");
            let kg2 = kg.add_node("feature", "Ownership").expect("add kg 2");
            kg.add_edge(kg1, kg2, "has_feature", 2.5)
                .expect("add kg edge");
        }

        let snapshot = export_memory_graph(&store, false).expect("export graph");
        assert_eq!(snapshot.total_memories, 2);
        assert_eq!(snapshot.total_entities, 2);
        assert_eq!(snapshot.nodes.len(), 4);
        assert_eq!(snapshot.edges.len(), 2);

        // memory_links のエッジ検証
        let mem_edge = snapshot
            .edges
            .iter()
            .find(|e| e.relation == "explains")
            .expect("mem edge exists");
        assert_eq!(mem_edge.source, format!("mem_{}", id1));
        assert_eq!(mem_edge.target, format!("mem_{}", id2));

        // kg_edges のエッジ検証
        let kg_edge = snapshot
            .edges
            .iter()
            .find(|e| e.relation == "has_feature")
            .expect("kg edge exists");
        assert_eq!(kg_edge.weight, 2.5);

        // 次数が反映されているか検証 (id1 のノード weight > 1.0)
        let node1 = snapshot
            .nodes
            .iter()
            .find(|n| n.id == format!("mem_{}", id1))
            .expect("node1 exists");
        assert!(node1.weight > 1.0);
    }

    #[test]
    fn test_export_memory_graph_privacy_sanitized() {
        let store = MemoryStore::in_memory().expect("in memory store init");

        // 機微情報（APIキーやトークン、パスワード）を含むメモリ
        store
            .save_memory(
                "My secret token is ghp_123456789012345678901234567890123456 and password=supersecret",
                "secret_cred",
                &["auth".to_string()],
            )
            .expect("save secret");

        let snapshot = export_memory_graph(&store, true).expect("export sanitized");
        assert_eq!(snapshot.nodes.len(), 1);

        let node = &snapshot.nodes[0];
        assert!(
            !node.details.contains("supersecret"),
            "Details must not contain plaintext password"
        );
        assert!(
            !node
                .details
                .contains("ghp_123456789012345678901234567890123456"),
            "Details must not contain GitHub token"
        );
        assert!(
            node.details.contains("***REDACTED***"),
            "Redacted marker should be present"
        );
    }
}
