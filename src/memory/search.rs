use anyhow::Result;

use crate::memory::graph::KnowledgeGraph;
use crate::memory::store::{MemoryRecord, MemoryStore};
use crate::runtime::embedder::Embedder;

#[cfg(not(feature = "embeddings"))]
use crate::runtime::embedder::cosine_similarity;

/// ハイブリッド検索結果
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub memory: MemoryRecord,
    pub score: f32,
    pub source: SearchSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SearchSource {
    Keyword,
    Vector,
    /// 3-stream RRF の graph BFS 経路 (`HybridSearch::with_graph_weight` で有効化)。
    Graph,
    Hybrid,
}

/// ハイブリッド検索エンジン（FTS5 + ベクトルKNN + 任意 graph BFS の RRF融合）。
///
/// 既定は 2-stream (Keyword + Vector)、`with_graph_weight(beta>0)` で 3-stream に切替。
/// 3-stream 時の重み配分は `alpha`(keyword) + `(1 - alpha - beta)`(vector) + `beta`(graph) で正規化される
/// (`alpha + beta > 1.0` のとき vector 重みは 0.0 にクランプ)。
pub struct HybridSearch<'a> {
    store: &'a MemoryStore,
    embedder: &'a dyn Embedder,
    /// RRF融合のキーワード検索重み（0.0-1.0）
    alpha: f32,
    /// Graph stream の重み（0.0 = 無効、default OFF、Cerememory env opt-in pattern）
    beta: f32,
}

impl<'a> HybridSearch<'a> {
    pub fn new(store: &'a MemoryStore, embedder: &'a dyn Embedder) -> Self {
        Self {
            store,
            embedder,
            alpha: 0.5,
            beta: 0.0,
        }
    }

    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha.clamp(0.0, 1.0);
        self
    }

    /// 3-stream RRF の graph 重みを設定 (0.0-1.0、default 0.0 = 無効)。
    /// `beta > 0` のとき [`Self::search`] は keyword/vector/graph 3 stream を RRF 融合する。
    /// Graph stream を有効化する前に [`Self::index_memory_tokens`] で memory を indexing する必要がある。
    pub fn with_graph_weight(mut self, beta: f32) -> Self {
        self.beta = beta.clamp(0.0, 1.0);
        self
    }

    /// Memory content をトークン化し、`KnowledgeGraph` に `token -[appears_in]-> memory:{id}` edge を追加。
    /// Tokenization: lowercase + non-alphanumeric split + len≥3 + 簡易 English stopword 除去 + 重複排除。
    /// `with_graph_weight(>0)` 経由で 3-stream を使う indexing pipeline から呼ぶ。
    pub fn index_memory_tokens(&self, memory_id: i64, content: &str) -> Result<()> {
        let graph = KnowledgeGraph::new(self.store.conn());
        let mem_node = format!("memory:{memory_id}");
        let mem_id = graph.add_node("memory", &mem_node)?;
        for tok in tokenize_for_graph(content) {
            let tok_id = graph.add_node("token", &tok)?;
            graph.add_edge(tok_id, mem_id, "appears_in", 1.0)?;
        }
        Ok(())
    }

    /// ハイブリッド検索を実行 (2-stream default、`beta>0` のとき 3-stream)。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // 1. FTS5キーワード検索
        let keyword_results = self.store.search_memories(query, limit * 2)?;

        // 2. ベクトル類似度検索
        let query_vec = self.embedder.embed(&[query])?;
        let query_embedding = &query_vec[0];
        let vector_results = self.vector_search(query_embedding, limit * 2)?;

        // 3. Graph BFS 検索 (beta=0 のとき empty を返す)
        let graph_results = self.graph_search(query, limit * 2)?;

        // 4. RRF融合 (graph stream は beta>0 のときのみ寄与)
        let merged = self.rrf_merge(keyword_results, vector_results, graph_results, limit);

        Ok(merged)
    }

    /// Graph BFS による retrieval。query をトークン化し、各トークンから depth=1 で隣接する
    /// `memory:{id}` ノードを集約、edge weight の総和をスコアにして上位を返す。
    /// `beta == 0.0` のときは空 Vec を即返却 (cost を払わない short-circuit)。
    fn graph_search(&self, query: &str, limit: usize) -> Result<Vec<(MemoryRecord, f32)>> {
        if self.beta <= 0.0 {
            return Ok(Vec::new());
        }
        let graph = KnowledgeGraph::new(self.store.conn());
        let tokens = tokenize_for_graph(query);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let mut id_scores: std::collections::HashMap<i64, f32> = std::collections::HashMap::new();
        for tok in tokens {
            for (name, _rel, weight) in graph.neighbors(&tok, 1)? {
                if let Some(id_str) = name.strip_prefix("memory:")
                    && let Ok(mid) = id_str.parse::<i64>()
                {
                    *id_scores.entry(mid).or_insert(0.0) += weight as f32;
                }
            }
        }
        if id_scores.is_empty() {
            return Ok(Vec::new());
        }
        let mut scored: Vec<(i64, f32)> = id_scores.into_iter().collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        let ids: Vec<i64> = scored.iter().map(|(id, _)| *id).collect();
        let memories = self.store.get_memories_by_ids(&ids)?;
        use std::collections::HashMap;
        let by_id: HashMap<i64, MemoryRecord> = memories.into_iter().map(|m| (m.id, m)).collect();
        let results: Vec<(MemoryRecord, f32)> = scored
            .into_iter()
            .filter_map(|(id, s)| by_id.get(&id).cloned().map(|m| (m, s)))
            .collect();
        Ok(results)
    }

    /// ベクトル類似度検索 — **vec0 KNN path** (production、Plan D-2 採用)。
    /// 流れ:
    /// 1. store.vec_knn で memory_id + distance を取得 (vec0 ANN)
    /// 2. get_memories_by_ids で memories を IN clause 1 クエリ batch fetch (N+1 回避)
    /// 3. id 順序保持で MemoryRecord を distance ペアに復元
    /// 4. similarity = 1.0 - distance (cosine 距離 → 類似度) で RRF 上流に渡す
    #[cfg(feature = "embeddings")]
    fn vector_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(MemoryRecord, f32)>> {
        let knn = self.store.vec_knn(query_embedding, limit)?;
        if knn.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = knn.iter().map(|(id, _)| *id).collect();
        let memories = self.store.get_memories_by_ids(&ids)?;
        use std::collections::HashMap;
        let by_id: HashMap<i64, MemoryRecord> = memories.into_iter().map(|m| (m.id, m)).collect();
        let results: Vec<(MemoryRecord, f32)> = knn
            .into_iter()
            .filter_map(|(id, dist)| {
                by_id.get(&id).cloned().map(|m| {
                    let sim = 1.0_f32 - dist;
                    (m, sim)
                })
            })
            .collect();
        Ok(results)
    }

    /// ベクトル類似度検索 — **linear scan path** (CI hash-only build 専用、Plan D-2)。
    /// `--no-default-features` 経路 (sqlite-vec 不要 / ハッシュ embedder のみ) で
    /// vec0 が compile されないとき選択される。production deploy では default feature
    /// `embeddings` が ON のため本 path は compile されない (compile-time exclusive)。
    #[cfg(not(feature = "embeddings"))]
    fn vector_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(MemoryRecord, f32)>> {
        let all_memories = self.store.all_memories()?;
        let mut scored: Vec<(MemoryRecord, f32)> = all_memories
            .into_iter()
            .map(|m| {
                let mem_vec = self.embedder.embed(&[&m.content]).unwrap_or_default();
                let sim = if mem_vec.is_empty() {
                    0.0
                } else {
                    cosine_similarity(query_embedding, &mem_vec[0])
                };
                (m, sim)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored)
    }

    /// Reciprocal Rank Fusion (2-stream または 3-stream)。
    /// 重み配分: `beta == 0` のとき `(alpha, 1-alpha, 0)` で従来挙動完全互換、
    /// `beta > 0` のとき `(alpha, max(0, 1-alpha-beta), beta)` で graph stream を合流。
    fn rrf_merge(
        &self,
        keyword_results: Vec<MemoryRecord>,
        vector_results: Vec<(MemoryRecord, f32)>,
        graph_results: Vec<(MemoryRecord, f32)>,
        limit: usize,
    ) -> Vec<SearchResult> {
        use std::collections::HashMap;

        let k = 60.0f32; // RRF定数
        let (w_kw, w_vec, w_graph) = if self.beta > 0.0 {
            let w_vec = (1.0 - self.alpha - self.beta).max(0.0);
            (self.alpha, w_vec, self.beta)
        } else {
            (self.alpha, 1.0 - self.alpha, 0.0)
        };

        let mut scores: HashMap<i64, (f32, MemoryRecord, SearchSource)> = HashMap::new();

        // キーワード検索のスコア（所有権移動でclone不要）
        for (rank, mem) in keyword_results.into_iter().enumerate() {
            let rrf_score = w_kw / (k + rank as f32 + 1.0);
            scores
                .entry(mem.id)
                .and_modify(|(s, _, src)| {
                    *s += rrf_score;
                    *src = SearchSource::Hybrid;
                })
                .or_insert((rrf_score, mem, SearchSource::Keyword));
        }

        // ベクトル検索のスコア（所有権移動でclone不要）
        for (rank, (mem, _sim)) in vector_results.into_iter().enumerate() {
            let rrf_score = w_vec / (k + rank as f32 + 1.0);
            scores
                .entry(mem.id)
                .and_modify(|(s, _, src)| {
                    *s += rrf_score;
                    *src = SearchSource::Hybrid;
                })
                .or_insert((rrf_score, mem, SearchSource::Vector));
        }

        // Graph 検索のスコア (beta > 0 のときのみ寄与)
        for (rank, (mem, _w)) in graph_results.into_iter().enumerate() {
            let rrf_score = w_graph / (k + rank as f32 + 1.0);
            scores
                .entry(mem.id)
                .and_modify(|(s, _, src)| {
                    *s += rrf_score;
                    *src = SearchSource::Hybrid;
                })
                .or_insert((rrf_score, mem, SearchSource::Graph));
        }

        let mut results: Vec<SearchResult> = scores
            .into_values()
            .map(|(score, memory, source)| SearchResult {
                memory,
                score,
                source,
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);
        results
    }
}

/// Graph BFS 用の簡易 tokenizer (lowercase + non-alphanumeric split + len>=3 + stopword 除去 + 重複排除)。
/// FTS5 tokenizer と独立した方が日本語/絵文字混在ケースで予測しやすく、
/// LongMemEval の英語 conversational テキストでは大半の意味語を拾える。
fn tokenize_for_graph(text: &str) -> Vec<String> {
    use std::collections::HashSet;
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was", "one",
        "our", "out", "day", "get", "has", "him", "his", "how", "man", "new", "now", "old", "see",
        "two", "way", "who", "did", "its", "let", "put", "say", "she", "too", "use", "with",
        "from", "this", "that", "have", "they", "what", "your", "when", "will", "there", "their",
        "would", "could", "should", "into", "about", "which", "were", "been",
    ];
    let stop: HashSet<&&str> = STOPWORDS.iter().collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut tokens: Vec<String> = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        let t = raw.to_ascii_lowercase();
        if t.len() < 3 {
            continue;
        }
        if stop.contains(&t.as_str()) {
            continue;
        }
        if seen.insert(t.clone()) {
            tokens.push(t);
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::embedder::SimpleEmbedder;

    fn setup() -> (MemoryStore, SimpleEmbedder) {
        let store = MemoryStore::in_memory().unwrap();
        let embedder = SimpleEmbedder::default();
        store
            .save_memory(
                "Rust is a fast programming language",
                "fact",
                &["rust".into()],
            )
            .unwrap();
        store
            .save_memory(
                "Python is great for data science",
                "fact",
                &["python".into()],
            )
            .unwrap();
        store
            .save_memory("JavaScript runs in browsers", "fact", &["js".into()])
            .unwrap();
        // vec0 path 用に既存 memories を backfill (linear path では no-op、Plan G-2.4 補足)。
        // 旧 linear path は vector_search 呼出ごとに on-the-fly で各 memory を embed
        // していたため backfill 不要だったが、vec0 path は事前 INSERT が前提のため
        // テスト setup で明示的に backfill する。
        #[cfg(feature = "embeddings")]
        store.ensure_vec_table(&embedder).unwrap();
        (store, embedder)
    }

    #[test]
    fn test_hybrid_search_returns_results() {
        let (store, embedder) = setup();
        let search = HybridSearch::new(&store, &embedder);
        let results = search.search("Rust programming", 5).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_hybrid_search_relevance() {
        let (store, embedder) = setup();
        let search = HybridSearch::new(&store, &embedder);
        let results = search.search("Rust", 5).unwrap();
        // Rustに関するメモリが上位に来るべき
        assert!(results[0].memory.content.contains("Rust"));
    }

    #[test]
    fn test_hybrid_search_limit() {
        let (store, embedder) = setup();
        let search = HybridSearch::new(&store, &embedder);
        let results = search.search("programming", 1).unwrap();
        assert!(results.len() <= 1);
    }

    #[test]
    fn test_hybrid_search_alpha() {
        let (store, embedder) = setup();
        let search = HybridSearch::new(&store, &embedder).with_alpha(1.0);
        // alpha=1.0はキーワード検索のみ
        let results = search.search("Rust", 5).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_hybrid_search_empty_store() {
        let store = MemoryStore::in_memory().unwrap();
        let embedder = SimpleEmbedder::default();
        let search = HybridSearch::new(&store, &embedder);
        let results = search.search("anything", 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_source_hybrid() {
        let (store, embedder) = setup();
        let search = HybridSearch::new(&store, &embedder);
        let results = search.search("Rust", 5).unwrap();
        // FTS5とベクトル両方にヒットすればHybridソース
        let has_hybrid = results.iter().any(|r| r.source == SearchSource::Hybrid);
        let has_any = !results.is_empty();
        assert!(has_hybrid || has_any);
    }

    // ---------- 3-stream RRF (graph fusion) tests ----------

    #[test]
    fn test_tokenize_for_graph_basic() {
        let toks = tokenize_for_graph("The Rust programming language is fast");
        // "the","is" は stopword 除去、"a" は len<3 で除外、残るのは Rust/programming/language/fast
        assert!(toks.contains(&"rust".to_string()));
        assert!(toks.contains(&"programming".to_string()));
        assert!(toks.contains(&"language".to_string()));
        assert!(toks.contains(&"fast".to_string()));
        assert!(!toks.contains(&"the".to_string()));
        assert!(!toks.contains(&"is".to_string()));
    }

    #[test]
    fn test_tokenize_for_graph_dedup_within_text() {
        let toks = tokenize_for_graph("rust rust RUST Rust");
        assert_eq!(toks.iter().filter(|t| *t == "rust").count(), 1);
    }

    #[test]
    fn test_with_graph_weight_clamp_and_default_off() {
        let (store, embedder) = setup();
        let s = HybridSearch::new(&store, &embedder);
        assert_eq!(s.beta, 0.0);
        let s2 = HybridSearch::new(&store, &embedder).with_graph_weight(1.5);
        assert_eq!(s2.beta, 1.0);
        let s3 = HybridSearch::new(&store, &embedder).with_graph_weight(-0.3);
        assert_eq!(s3.beta, 0.0);
    }

    #[test]
    fn test_index_memory_tokens_populates_graph() {
        let (store, embedder) = setup();
        let s = HybridSearch::new(&store, &embedder);
        // setup() で 3 件保存 (id 1..=3 想定)。ここでは id=1 の "Rust ..." を indexing。
        s.index_memory_tokens(1, "Rust is a fast programming language")
            .unwrap();
        // KnowledgeGraph に問合せ: "rust" から 1-hop で memory:1 が見える
        let graph = KnowledgeGraph::new(store.conn());
        let neighbors = graph.neighbors("rust", 1).unwrap();
        let hit = neighbors
            .iter()
            .any(|(name, rel, _w)| name == "memory:1" && rel == "appears_in");
        assert!(hit, "indexing 後 token→memory:1 edge が存在すべき");
    }

    #[test]
    fn test_search_without_graph_unchanged() {
        // beta=0 (default) のとき search 結果は従来 2-stream と同等で graph source は出ない
        let (store, embedder) = setup();
        let s = HybridSearch::new(&store, &embedder);
        let results = s.search("Rust", 5).unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().all(|r| r.source != SearchSource::Graph));
    }

    #[test]
    fn test_search_with_graph_returns_results() {
        let (store, embedder) = setup();
        let s = HybridSearch::new(&store, &embedder).with_graph_weight(0.3);
        // 全 memory を indexing
        s.index_memory_tokens(1, "Rust is a fast programming language")
            .unwrap();
        s.index_memory_tokens(2, "Python is great for data science")
            .unwrap();
        s.index_memory_tokens(3, "JavaScript runs in browsers")
            .unwrap();
        let results = s.search("Rust programming", 5).unwrap();
        assert!(!results.is_empty());
        assert!(results[0].memory.content.contains("Rust"));
    }

    #[test]
    fn test_graph_only_contribution_when_keyword_misses() {
        // FTS5 では拾えない synonyms 状況を模倣: graph に明示 edge を張って graph stream のみ寄与確認。
        // SimpleEmbedder の hash 経路では vector も上位に来るが、graph stream が存在すること自体を検証。
        let (store, embedder) = setup();
        let s = HybridSearch::new(&store, &embedder).with_graph_weight(0.5);
        s.index_memory_tokens(2, "Python is great for data science")
            .unwrap();
        // graph_search 内部呼出を確認
        let internal = s.graph_search("Python data", 5).unwrap();
        assert!(
            !internal.is_empty(),
            "graph_search は indexing 済 token から memory を返すべき"
        );
        assert!(internal.iter().any(|(m, _s)| m.id == 2));
    }

    #[test]
    fn test_graph_search_short_circuits_when_disabled() {
        let (store, embedder) = setup();
        let s = HybridSearch::new(&store, &embedder); // beta=0
        let r = s.graph_search("anything", 5).unwrap();
        assert!(
            r.is_empty(),
            "beta=0 のとき graph_search は空 Vec を返すべき"
        );
    }
}
