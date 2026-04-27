# Model Fallback Chain 実装スケルトン

**Source:** `macos26-agent-learnings-v2.md` ★★ 採用候補 D
**Date:** 2026-04-27
**Status:** 設計（Lab v13 完了後の実装候補）

---

## 動機

**問題**: 現状 bonsai-agent の `[advisor] backend = "claude-code"` フォールバックは **advisor (検証/再計画) 専用**で、メイン推論 (`[model]`) には適用されない。

長時間 Lab 運転中に MLX サーバ接続断 / API 失敗が発生すると、main 推論が止まり Lab 全体が中断する。macOS26/Agent の `FallbackChainService` は (provider, model) のチェーンを持ち、N 回連続失敗で次の provider に自動切替する。

**ゴール**: メイン推論 (`LlmBackend`) にもフォールバックチェーンを適用、夜間 Lab の自動復旧を実現。

## 設計

### モジュール配置

```
src/runtime/
├── model_router.rs      ← 既存、FallbackChain を追加
├── inference.rs         ← 既存、LlmBackend を fallback wrapper でラップ
└── ...
```

### 公開 API

```rust
// src/runtime/model_router.rs に追加

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::Arc;

/// バックエンド種別 + モデル ID の組
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FallbackEntry {
    pub backend: ServerBackend,  // 既存 enum: LlamaServer/MlxLm/BitNet/MockLlm
    pub model_id: String,
    pub server_url: String,
}

/// 連続失敗時に次の backend へ切替えるチェーン
///
/// 既存 `[advisor]` の backend フォールバックは advice 専用。
/// このチェーンはメイン推論 (`LlmBackend::generate`) に適用される。
#[derive(Debug)]
pub struct FallbackChain {
    /// プライマリ + 順序付きフォールバックリスト
    entries: Vec<FallbackEntry>,
    /// 現在のインデックス (-1 = primary、0 以上 = フォールバック中)
    current_idx: AtomicI32,
    /// 現在の provider での連続失敗数
    consecutive_failures: AtomicUsize,
    /// 切替閾値（macOS26/Agent と同じデフォルト 2）
    max_failures_before_fallback: usize,
}

impl FallbackChain {
    pub fn new(entries: Vec<FallbackEntry>) -> Self {
        Self {
            entries,
            current_idx: AtomicI32::new(-1),
            consecutive_failures: AtomicUsize::new(0),
            max_failures_before_fallback: 2,
        }
    }

    /// 現在の backend を取得（primary なら entries[0] と互換、fallback 中ならそれ）
    pub fn current(&self) -> Option<&FallbackEntry> {
        let idx = self.current_idx.load(Ordering::SeqCst);
        if idx < 0 {
            self.entries.first()
        } else {
            self.entries.get(idx as usize)
        }
    }

    /// 失敗を記録、必要なら次の backend に切替
    pub fn record_failure(&self) -> Option<&FallbackEntry> {
        let count = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        if count >= self.max_failures_before_fallback {
            // 次へ
            let next_idx = self.current_idx.load(Ordering::SeqCst) + 1;
            if (next_idx as usize) < self.entries.len() {
                self.current_idx.store(next_idx, Ordering::SeqCst);
                self.consecutive_failures.store(0, Ordering::SeqCst);
                return self.entries.get(next_idx as usize);
            }
        }
        None
    }

    /// 成功を記録、failure カウンタをリセット
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
    }

    /// プライマリへ復帰（手動 reset 用）
    pub fn reset_to_primary(&self) {
        self.current_idx.store(-1, Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
    }
}
```

### `LlmBackend` ラッパー

```rust
// src/runtime/inference.rs に追加

/// Fallback chain で複数 backend をラップする LlmBackend 実装
pub struct FallbackBackend {
    chain: Arc<FallbackChain>,
    backends: HashMap<String, Box<dyn LlmBackend>>,
}

impl FallbackBackend {
    pub fn new(chain: FallbackChain, backends: HashMap<String, Box<dyn LlmBackend>>) -> Self {
        Self { chain: Arc::new(chain), backends }
    }
}

impl LlmBackend for FallbackBackend {
    fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
        cancel: &CancellationToken,
    ) -> Result<GenerateResult> {
        let mut last_err: Option<anyhow::Error> = None;
        // 最大 entries.len() 回までリトライ
        for _ in 0..self.chain.entries.len() {
            let entry = self.chain.current()
                .ok_or_else(|| anyhow::anyhow!("fallback chain exhausted"))?;
            let key = format!("{:?}:{}", entry.backend, entry.model_id);
            let backend = self.backends.get(&key)
                .ok_or_else(|| anyhow::anyhow!("backend not registered: {key}"))?;
            match backend.generate(messages, tools, on_token, cancel) {
                Ok(result) => {
                    self.chain.record_success();
                    return Ok(result);
                }
                Err(e) => {
                    log_event(LogLevel::Warn, "fallback",
                        &format!("backend {key} failed: {e}"));
                    last_err = Some(e);
                    // 切替試行
                    if self.chain.record_failure().is_none() {
                        // 切替不要 (failure 閾値未到達) → そのまま再試行
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("fallback exhausted")))
    }
    // generate_with_params も同様にラップ
}
```

### config.toml 拡張

```toml
[fallback_chain]
# 連続失敗 N 回で次に切替
max_failures = 2

[[fallback_chain.entries]]
backend = "mlx-lm"
model_id = "ternary-bonsai-8b"
server_url = "http://localhost:8000"

[[fallback_chain.entries]]
backend = "llama-server"
model_id = "bonsai-8b-gguf"
server_url = "http://localhost:8080"

[[fallback_chain.entries]]
backend = "bitnet"
model_id = "bitnet-b1-58-3b"
server_url = "http://localhost:8090"
```

### TDD 計画（推定 6 テスト）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_chain_starts_at_primary() {
        let chain = FallbackChain::new(vec![entry("a"), entry("b")]);
        assert_eq!(chain.current().unwrap().model_id, "a");
    }

    #[test]
    fn t_chain_does_not_switch_below_threshold() {
        let chain = FallbackChain::new(vec![entry("a"), entry("b")]);
        chain.record_failure(); // 1 回目、threshold=2 未到達
        assert_eq!(chain.current().unwrap().model_id, "a");
    }

    #[test]
    fn t_chain_switches_on_threshold() {
        let chain = FallbackChain::new(vec![entry("a"), entry("b")]);
        chain.record_failure();
        chain.record_failure(); // 2 回目、threshold 到達
        assert_eq!(chain.current().unwrap().model_id, "b");
    }

    #[test]
    fn t_chain_returns_none_when_exhausted() {
        let chain = FallbackChain::new(vec![entry("a")]);
        chain.record_failure();
        chain.record_failure();
        // 1 件しかない → 次がない
        assert_eq!(chain.current().unwrap().model_id, "a"); // 維持
    }

    #[test]
    fn t_chain_success_resets_counter() {
        let chain = FallbackChain::new(vec![entry("a"), entry("b")]);
        chain.record_failure(); // count=1
        chain.record_success(); // count=0
        chain.record_failure(); // count=1
        // threshold=2 にまだ到達していない
        assert_eq!(chain.current().unwrap().model_id, "a");
    }

    #[test]
    fn t_chain_reset_to_primary() {
        let chain = FallbackChain::new(vec![entry("a"), entry("b")]);
        chain.record_failure();
        chain.record_failure();
        chain.reset_to_primary();
        assert_eq!(chain.current().unwrap().model_id, "a");
    }

    fn entry(id: &str) -> FallbackEntry {
        FallbackEntry {
            backend: ServerBackend::MlxLm,
            model_id: id.to_string(),
            server_url: "http://localhost:8000".into(),
        }
    }
}
```

## 実装手順（推定 3h）

| Step | 内容 | テスト | 工数 |
|---|---|---|---|
| 1 | `model_router.rs` に FallbackChain + 6 テスト Red | +6 失敗 | 30分 |
| 2 | Green 実装 (Atomic 操作、record_failure/success) | 950→956 pass | 45分 |
| 3 | `inference.rs` に FallbackBackend ラッパー | 956 維持 | 60分 |
| 4 | `config.rs` に `[fallback_chain]` セクション | 956 維持 | 30分 |
| 5 | `main.rs` で起動時にチェーン構築 | 956 維持 | 20分 |
| 6 | CLAUDE.md 項目追記 + commit | — | 15分 |

合計 200 分（3.3h）、目標 3h 内に収まる。

## リスク

| リスク | 対策 |
|---|---|
| Atomic の並列性不整合 | record_failure → switch を SeqCst で順序保証 |
| バックエンド同時起動コスト | 起動は遅延（最初に必要になった時にスポーン）、停止時に kill |
| フォールバック中も SSE タイムアウト発生 | sse_chunk_timeout_secs を全 backend に統一 (180s) |
| プライマリ復帰タイミング不明 | 手動 `reset_to_primary` API、または周期的 health check で自動復帰 |

## 採否判定ゲート

実装前に確認:

- [ ] Lab v13 で MLX 接続断絶 / API 失敗が 2 回以上発生すること
- [ ] llama-server / BitNet など複数バックエンドの動作確認済
- [ ] 切替時のセッション状態（in-flight token）の扱い設計確認

該当なしなら見送り（YAGNI）。

## 関連
- 親計画: `.claude/plan/macos26-agent-learnings-v2.md`
- 既存類似機能: 項目19 HttpAdvisor、項目89 Claude Code Advisor
- 並行: `.claude/plan/diffstore-rust-impl.md` (★★★)、`.claude/plan/edit-cycle-detector-impl.md` (★★)
