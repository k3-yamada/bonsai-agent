use anyhow::Result;

/// 埋め込みベクトルの次元数（EmbeddingGemma 768d、Matryoshka MRLで256dに縮小可能）
pub const DEFAULT_EMBEDDING_DIM: usize = 256;

/// 埋め込みモデルの抽象化トレイト。
/// fastembed有効時はEmbeddingGemma、無効時はSimpleEmbedder（TF-IDFライク）を使用。
pub trait Embedder: Send + Sync {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dim(&self) -> usize;
}

/// 簡易埋め込み（fastembed無しでも動作するフォールバック）。
/// 文字のハッシュベースで固定長ベクトルを生成。精度は低いがゼロ依存。
pub struct SimpleEmbedder {
    dim: usize,
}

impl SimpleEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl Default for SimpleEmbedder {
    fn default() -> Self {
        Self::new(DEFAULT_EMBEDDING_DIM)
    }
}

impl Embedder for SimpleEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| hash_embed(text, self.dim))
            .collect())
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

/// 文字列をハッシュベースで固定長ベクトルに変換（簡易実装）
fn hash_embed(text: &str, dim: usize) -> Vec<f32> {
    let mut vec = vec![0.0f32; dim];
    for (i, c) in text.chars().enumerate() {
        let idx = (c as usize).wrapping_mul(31).wrapping_add(i) % dim;
        vec[idx] += 1.0;
    }
    // L2正規化
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    }
    vec
}

/// fastembed有効時のローカルONNX埋め込みラッパー（AllMiniLML6V2）
#[cfg(feature = "embeddings")]
pub struct FastEmbedder {
    // fastembed::TextEmbedding::embed()が&mut selfを要求するため内部可変性を持たせる
    model: std::sync::Mutex<fastembed::TextEmbedding>,
    dim: usize,
}

#[cfg(feature = "embeddings")]
impl FastEmbedder {
    pub fn new() -> Result<Self> {
        let options = fastembed::TextInitOptions::new(fastembed::EmbeddingModel::AllMiniLML6V2);
        let model = fastembed::TextEmbedding::try_new(options)?;
        Ok(Self {
            model: std::sync::Mutex::new(model),
            dim: DEFAULT_EMBEDDING_DIM,
        })
    }
}

#[cfg(feature = "embeddings")]
impl Embedder for FastEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let mut guard = self
            .model
            .lock()
            .map_err(|e| anyhow::anyhow!("FastEmbedder Mutex poisoned: {e}"))?;
        let embeddings = guard.embed(owned, None)?;
        // Matryoshka: 384d→256dに切り詰め（AllMiniLML6V2の出力は384次元）
        Ok(embeddings
            .into_iter()
            .map(|v| v.into_iter().take(self.dim).collect())
            .collect())
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

/// 最適なEmbedderを作成（fastembed有効時はFastEmbedder、無効時はSimpleEmbedder）
pub fn create_embedder() -> Box<dyn Embedder> {
    #[cfg(feature = "embeddings")]
    {
        match FastEmbedder::new() {
            Ok(e) => return Box::new(e),
            Err(err) => {
                eprintln!("FastEmbedder初期化失敗、SimpleEmbedderにフォールバック: {err}");
            }
        }
    }
    Box::new(SimpleEmbedder::default())
}

/// コサイン類似度
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_embedder() {
        let embedder = SimpleEmbedder::default();
        let vecs = embedder.embed(&["hello world"]).unwrap();
        assert_eq!(vecs.len(), 1);
        assert_eq!(vecs[0].len(), DEFAULT_EMBEDDING_DIM);
    }

    #[test]
    fn test_simple_embedder_normalized() {
        let embedder = SimpleEmbedder::default();
        let vecs = embedder.embed(&["test"]).unwrap();
        let norm: f32 = vecs[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_similar_texts_higher_similarity() {
        let embedder = SimpleEmbedder::default();
        let vecs = embedder
            .embed(&["rust programming", "rust language", "python snake"])
            .unwrap();
        let sim_same = cosine_similarity(&vecs[0], &vecs[1]);
        let sim_diff = cosine_similarity(&vecs[0], &vecs[2]);
        assert!(sim_same > sim_diff);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_multiple_texts() {
        let embedder = SimpleEmbedder::default();
        let vecs = embedder.embed(&["a", "b", "c"]).unwrap();
        assert_eq!(vecs.len(), 3);
    }

    #[test]
    fn test_create_embedder() {
        let embedder = create_embedder();
        assert_eq!(embedder.dim(), DEFAULT_EMBEDDING_DIM);
    }
}
