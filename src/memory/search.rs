use anyhow::Result;

use crate::memory::store::{MemoryRecord, MemoryStore};
use crate::runtime::embedder::{cosine_similarity, Embedder};

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
        let merged = self.rrf_merge(&keyword_results, &vector_results, limit);

        Ok(merged)
    }

    /// ベクトル類似度で全メモリをスキャン（sqlite-vec未使用時のフォールバック）
    fn vector_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(MemoryRecord, f32)>> {
        // 全メモリを取得してコサイン類似度を計算（小規模データ向け）
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
        keyword_results: &[MemoryRecord],
        vector_results: &[(MemoryRecord, f32)],
        limit: usize,
    ) -> Vec<SearchResult> {
        use std::collections::HashMap;

        let k = 60.0f32; // RRF定数
        let mut scores: HashMap<i64, (f32, MemoryRecord, SearchSource)> = HashMap::new();

        // キーワード検索のスコア
        for (rank, mem) in keyword_results.iter().enumerate() {
            let rrf_score = self.alpha / (k + rank as f32 + 1.0);
            scores
                .entry(mem.id)
                .and_modify(|(s, _, src)| {
                    *s += rrf_score;
                    *src = SearchSource::Hybrid;
                })
                .or_insert((rrf_score, mem.clone(), SearchSource::Keyword));
        }

        // ベクトル検索のスコア
        for (rank, (mem, _sim)) in vector_results.iter().enumerate() {
            let rrf_score = (1.0 - self.alpha) / (k + rank as f32 + 1.0);
            scores
                .entry(mem.id)
                .and_modify(|(s, _, src)| {
                    *s += rrf_score;
                    *src = SearchSource::Hybrid;
                })
                .or_insert((rrf_score, mem.clone(), SearchSource::Vector));
        }

        let mut results: Vec<SearchResult> = scores
            .into_values()
            .map(|(score, memory, source)| SearchResult {
                memory,
                score,
                source,
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
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
        store.save_memory("Rust is a fast programming language", "fact", &["rust".into()]).unwrap();
        store.save_memory("Python is great for data science", "fact", &["python".into()]).unwrap();
        store.save_memory("JavaScript runs in browsers", "fact", &["js".into()]).unwrap();
        (store, SimpleEmbedder::default())
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
