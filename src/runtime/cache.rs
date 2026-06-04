use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

use crate::cancel::CancellationToken;
use crate::config::InferenceParams;
use crate::domain::conversation::Message;
use crate::domain::tool_schema::ToolSchema;
use crate::runtime::inference::{GenerateResult, LlmBackend, TokenUsage};

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

/// キャッシュ付きLLMバックエンド（--lab専用）
/// 同一入力に対して同一出力を返すことでベンチマーク結果を安定化
pub struct CachedBackend {
    inner: Box<dyn LlmBackend>,
    cache: Mutex<InferenceCache>,
}

impl CachedBackend {
    pub fn new(inner: Box<dyn LlmBackend>, max_entries: usize) -> Self {
        Self {
            inner,
            cache: Mutex::new(InferenceCache::new(max_entries)),
        }
    }

    /// messagesとtoolsからハッシュキーを生成
    fn compute_prompt_hash(messages: &[Message], tools: &[ToolSchema]) -> String {
        let mut hasher = DefaultHasher::new();
        for msg in messages {
            // roleも含めて異なるメッセージ順序を区別
            format!("{:?}", msg.role).hash(&mut hasher);
            msg.content.hash(&mut hasher);
        }
        for tool in tools {
            tool.name.hash(&mut hasher);
        }
        format!("{:x}", hasher.finish())
    }

    /// G1 Critic 別 LLM 分離 (項目 226 候補): `InferenceParams` 込みのハッシュキー生成。
    /// 同一 messages/tools でも temperature 等が違えば別キャッシュとして扱う。
    /// f64 は `to_bits()` で bit-level 比較し NaN ≠ NaN を区別する (Lab 再現性確保)。
    fn compute_prompt_hash_with_params(
        messages: &[Message],
        tools: &[ToolSchema],
        params: &InferenceParams,
    ) -> String {
        let mut hasher = DefaultHasher::new();
        Self::compute_prompt_hash(messages, tools).hash(&mut hasher);
        params.temperature.to_bits().hash(&mut hasher);
        params.top_p.to_bits().hash(&mut hasher);
        params.top_k.hash(&mut hasher);
        params.min_p.to_bits().hash(&mut hasher);
        params.max_tokens.hash(&mut hasher);
        params.repeat_penalty.to_bits().hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}

impl LlmBackend for CachedBackend {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
    ) -> anyhow::Result<GenerateResult> {
        let prompt_hash = Self::compute_prompt_hash(messages, tools);
        let key = InferenceCache::compute_key(self.inner.model_id(), &prompt_hash);

        // キャッシュヒット
        let mut guard = self
            .cache
            .lock()
            .map_err(|e| anyhow::anyhow!("キャッシュロック取得失敗: {e}"))?;
        if let Some(cached) = guard.get(key).map(|s| s.to_string()) {
            drop(guard);
            on_token(&cached);
            return Ok(GenerateResult {
                text: cached,
                usage: TokenUsage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    duration: std::time::Duration::ZERO,
                },
                model_id: self.inner.model_id().to_string(),
            });
        }

        drop(guard);

        // キャッシュミス: 内部バックエンド呼び出し
        let result = self.inner.generate(messages, tools, on_token, cancel)?;
        self.cache
            .lock()
            .map_err(|e| anyhow::anyhow!("キャッシュロック取得失敗: {e}"))?
            .put(key, result.text.clone());
        Ok(result)
    }

    /// G1 Critic 別 LLM 分離 (項目 226 候補): params 入りキーで cache し、inner へ params 透過。
    /// trait default は `generate` に委譲して params を捨てるため、critic temperature override を
    /// production で効かせるには本 override が必須。
    fn generate_with_params(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
        params: &InferenceParams,
    ) -> anyhow::Result<GenerateResult> {
        let prompt_hash = Self::compute_prompt_hash_with_params(messages, tools, params);
        let key = InferenceCache::compute_key(self.inner.model_id(), &prompt_hash);

        let mut guard = self
            .cache
            .lock()
            .map_err(|e| anyhow::anyhow!("キャッシュロック取得失敗: {e}"))?;
        if let Some(cached) = guard.get(key).map(|s| s.to_string()) {
            drop(guard);
            on_token(&cached);
            return Ok(GenerateResult {
                text: cached,
                usage: TokenUsage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    duration: std::time::Duration::ZERO,
                },
                model_id: self.inner.model_id().to_string(),
            });
        }
        drop(guard);

        let result = self
            .inner
            .generate_with_params(messages, tools, on_token, cancel, params)?;
        self.cache
            .lock()
            .map_err(|e| anyhow::anyhow!("キャッシュロック取得失敗: {e}"))?
            .put(key, result.text.clone());
        Ok(result)
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

#[cfg(test)]
mod cached_tests {
    use super::*;
    use crate::config::ServerBackend;
    use crate::runtime::inference::{FallbackBackend, MockLlmBackend};
    use crate::runtime::model_router::{FallbackChain, FallbackEntry};

    #[test]
    fn test_cached_backend_miss_then_hit() {
        let mock = MockLlmBackend::new(vec!["回答A".to_string()]);
        let cached = CachedBackend::new(Box::new(mock), 10);
        let cancel = CancellationToken::new();
        let msgs = vec![Message::user("hello")];

        // 1回目: キャッシュミス → モックから取得
        let r1 = cached.generate(&msgs, &[], &mut |_| {}, &cancel).unwrap();
        assert_eq!(r1.text, "回答A");

        // 2回目: キャッシュヒット → モックは空だが成功
        let r2 = cached.generate(&msgs, &[], &mut |_| {}, &cancel).unwrap();
        assert_eq!(r2.text, "回答A");
    }

    #[test]
    fn test_cached_backend_different_prompts() {
        let mock = MockLlmBackend::new(vec!["回答1".to_string(), "回答2".to_string()]);
        let cached = CachedBackend::new(Box::new(mock), 10);
        let cancel = CancellationToken::new();

        let r1 = cached
            .generate(&[Message::user("a")], &[], &mut |_| {}, &cancel)
            .unwrap();
        let r2 = cached
            .generate(&[Message::user("b")], &[], &mut |_| {}, &cancel)
            .unwrap();
        assert_ne!(r1.text, r2.text);
    }

    // ─── R13: CachedBackend × FallbackBackend wrap order 観察事項テスト ───
    //
    // Plan: .claude/plan/cached-backend-wrap-order-tests.md (handoff 05-02b R13)
    //
    // 観察事項: FallbackBackend::model_id() = "fallback-chain" 定数のため、
    // CachedBackend(FallbackBackend(...)) wrap で cache key が primary/fallback を
    // 区別しない。同一プロンプト → 同一 cached text を返す現状仕様を test で契約化し、
    // 将来の FallbackBackend::model_id() 動的化変更を regression 検出可能にする。

    fn build_cached_fallback(
        primary_responses: Vec<String>,
        secondary_responses: Vec<String>,
    ) -> CachedBackend {
        let entries = vec![
            FallbackEntry {
                backend: ServerBackend::MlxLm,
                model_id: "primary-mlx".into(),
                server_url: "http://127.0.0.1:8000".into(),
            },
            FallbackEntry {
                backend: ServerBackend::LlamaServer,
                model_id: "secondary-llama".into(),
                server_url: "http://127.0.0.1:8080".into(),
            },
        ];
        let chain = FallbackChain::with_threshold(entries.clone(), 1);
        let mut backends: HashMap<String, Box<dyn LlmBackend>> = HashMap::new();
        backends.insert(
            FallbackBackend::key_for(&entries[0]),
            Box::new(MockLlmBackend::new(primary_responses)),
        );
        backends.insert(
            FallbackBackend::key_for(&entries[1]),
            Box::new(MockLlmBackend::new(secondary_responses)),
        );
        let fallback = FallbackBackend::new(chain, backends);
        CachedBackend::new(Box::new(fallback), 10)
    }

    #[test]
    fn test_cached_fallback_key_uses_synthetic_id() {
        // CachedBackend が wrap する FallbackBackend の model_id() = "fallback-chain"
        // → cache key は primary/fallback を区別しない (R13 観察事項を契約化)
        let cached =
            build_cached_fallback(vec!["primary-resp".into()], vec!["fallback-resp".into()]);
        assert_eq!(
            cached.model_id(),
            "fallback-chain",
            "CachedBackend は inner FallbackBackend の synthetic_id を返す"
        );
    }

    #[test]
    fn test_cached_fallback_hit_returns_same_text() {
        // 同一 prompt の 2 回目呼出は cache hit で 1 回目と同じ text を返す。
        // FallbackBackend wrap の有無に依存せず CachedBackend 契約は維持される。
        let cached = build_cached_fallback(
            vec!["primary-resp".into(), "primary-resp-2".into()],
            vec!["fallback-resp".into()],
        );
        let cancel = CancellationToken::new();
        let messages = vec![Message::user("hi")];

        // 1 回目: cache miss → primary 応答
        let r1 = cached
            .generate(&messages, &[], &mut |_| {}, &cancel)
            .expect("first call");
        assert_eq!(r1.text, "primary-resp", "1 回目は primary 応答");

        // 2 回目: cache hit → primary backend は呼ばれない。
        // cache が効かなければ MockLlmBackend が "primary-resp-2" を返す。
        let r2 = cached
            .generate(&messages, &[], &mut |_| {}, &cancel)
            .expect("second call");
        assert_eq!(
            r2.text, "primary-resp",
            "同一 prompt は cache hit で同じ text"
        );
    }

    #[test]
    fn test_cached_fallback_hit_model_id_field() {
        // 現状仕様: cache hit 時 model_id は inner.model_id() = synthetic_id (R13)。
        // 将来 FallbackBackend::model_id() を current entry に動的化する場合は
        // この assert を反転して再評価する。
        let cached =
            build_cached_fallback(vec!["primary-resp".into()], vec!["fallback-resp".into()]);
        let cancel = CancellationToken::new();
        let messages = vec![Message::user("hi")];

        let _r1 = cached
            .generate(&messages, &[], &mut |_| {}, &cancel)
            .expect("first call");

        let r2 = cached
            .generate(&messages, &[], &mut |_| {}, &cancel)
            .expect("second call");

        assert_eq!(
            r2.model_id, "fallback-chain",
            "現状: cache hit 時 model_id は synthetic_id (R13)"
        );
    }
}
