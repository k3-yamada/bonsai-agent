use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

/// 推論結果キャッシュ。model_id + messages + tools のハッシュをキーに使用。
pub struct InferenceCache {
    cache: HashMap<u64, CacheEntry>,
    max_entries: usize,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    response: String,
    access_count: u32,
}

impl InferenceCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: HashMap::new(),
            max_entries,
        }
    }

    /// キャッシュキーを計算する
    pub fn compute_key(model_id: &str, prompt_hash: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        model_id.hash(&mut hasher);
        prompt_hash.hash(&mut hasher);
        hasher.finish()
    }

    /// キャッシュを検索
    pub fn get(&mut self, key: u64) -> Option<&str> {
        if let Some(entry) = self.cache.get_mut(&key) {
            entry.access_count += 1;
            Some(&entry.response)
        } else {
            None
        }
    }

    /// キャッシュに保存。上限超過時は最もアクセス数の少ないエントリを削除。
    pub fn put(&mut self, key: u64, response: String) {
        if self.cache.len() >= self.max_entries && !self.cache.contains_key(&key) {
            self.evict_least_used();
        }
        self.cache.insert(
            key,
            CacheEntry {
                response,
                access_count: 0,
            },
        );
    }

    /// 最もアクセス数の少ないエントリを1件削除
    fn evict_least_used(&mut self) {
        if let Some((&key, _)) = self
            .cache
            .iter()
            .min_by_key(|(_, entry)| entry.access_count)
        {
            self.cache.remove(&key);
        }
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

impl Default for InferenceCache {
    fn default() -> Self {
        Self::new(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_miss() {
        let mut cache = InferenceCache::new(10);
        assert!(cache.get(12345).is_none());
    }

    #[test]
    fn test_cache_hit() {
        let mut cache = InferenceCache::new(10);
        let key = InferenceCache::compute_key("bonsai-8b", "hello");
        cache.put(key, "回答".to_string());
        assert_eq!(cache.get(key), Some("回答"));
    }

    #[test]
    fn test_different_models_different_keys() {
        let key_a = InferenceCache::compute_key("bonsai-8b", "hello");
        let key_b = InferenceCache::compute_key("gemma4-e4b", "hello");
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn test_same_model_different_prompts() {
        let key_a = InferenceCache::compute_key("bonsai-8b", "hello");
        let key_b = InferenceCache::compute_key("bonsai-8b", "goodbye");
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn test_eviction_on_max() {
        let mut cache = InferenceCache::new(2);
        cache.put(1, "a".to_string());
        cache.put(2, "b".to_string());
        assert_eq!(cache.len(), 2);

        // 3件目で最もアクセスの少ないものが削除される
        cache.put(3, "c".to_string());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_eviction_keeps_frequently_accessed() {
        let mut cache = InferenceCache::new(2);
        cache.put(1, "a".to_string());
        cache.put(2, "b".to_string());

        // key=1を3回アクセスして優先度を上げる
        cache.get(1);
        cache.get(1);
        cache.get(1);

        // key=3を追加 → key=2（アクセス0回）が削除されるはず
        cache.put(3, "c".to_string());
        assert!(cache.get(1).is_some()); // key=1は残る
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_overwrite_existing_key() {
        let mut cache = InferenceCache::new(10);
        cache.put(1, "old".to_string());
        cache.put(1, "new".to_string());
        assert_eq!(cache.get(1), Some("new"));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_clear() {
        let mut cache = InferenceCache::new(10);
        cache.put(1, "a".to_string());
        cache.put(2, "b".to_string());
        cache.clear();
        assert!(cache.is_empty());
    }
}
