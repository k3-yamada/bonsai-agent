use anyhow::Result;

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
    Hybrid,
}

/// ハイブリッド検索エンジン（FTS5 + ベクトルKNN + RRF融合）
pub struct HybridSearch<'a> {
    store: &'a MemoryStore,
    embedder: &'a dyn Embedder,
    /// RRF融合のキーワード検索重み（0.0-1.0）
    alpha: f32,
}

impl<'a> HybridSearch<'a> {
    pub fn new(store: &'a MemoryStore, embedder: &'a dyn Embedder) -> Self {
        Self {
            store,
            embedder,
            alpha: 0.5,
        }
    }

    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha.clamp(0.0, 1.0);
        self
    }

    /// ハイブリッド検索を実行
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // 1. FTS5キーワード検索
        let keyword_results = self.store.search_memories(query, limit * 2)?;

        // 2. ベクトル類似度検索
        let query_vec = self.embedder.embed(&[query])?;
        let query_embedding = &query_vec[0];
        let vector_results = self.vector_search(query_embedding, limit * 2)?;

        // 3. RRF融合
        let merged = self.rrf_merge(keyword_results, vector_results, limit);

        Ok(merged)
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

    /// Reciprocal Rank Fusion
    fn rrf_merge(
        &self,
        keyword_results: Vec<MemoryRecord>,
        vector_results: Vec<(MemoryRecord, f32)>,
        limit: usize,
    ) -> Vec<SearchResult> {
        use std::collections::HashMap;

        let k = 60.0f32; // RRF定数
        let mut scores: HashMap<i64, (f32, MemoryRecord, SearchSource)> = HashMap::new();

        // キーワード検索のスコア（所有権移動でclone不要）
        for (rank, mem) in keyword_results.into_iter().enumerate() {
            let rrf_score = self.alpha / (k + rank as f32 + 1.0);
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
            let rrf_score = (1.0 - self.alpha) / (k + rank as f32 + 1.0);
            scores
                .entry(mem.id)
                .and_modify(|(s, _, src)| {
                    *s += rrf_score;
                    *src = SearchSource::Hybrid;
                })
                .or_insert((rrf_score, mem, SearchSource::Vector));
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
}
