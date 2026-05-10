# Plan: vllm-mlx Backend 統合 — Apple Silicon 高 throughput 推論

> **Task Type**: Backend (Rust 2024 edition)
> **由来**: arxiv 2601.19139 *Native LLM Inference at Scale on Apple Silicon* (vllm-mlx, 2026-01) — `research_arxiv_2026_05_07.md` 領域 3 「★★★ 高優先 #6 = vllm-mlx backend 評価」 / CLAUDE.md カテゴリ索引「Backend / Inference: 項目 35, 36, 49, 53, 56, 60, 61, 63, 67, 90, 103, 105, 130, 167, 168, 174, 195, 198」/ MLX 経緯 = 項目 173 (環境劣化仮説) → 項目 184 (MLX vs llama-server REJECT) → 項目 195 (sticky fix) → 項目 198 (sticky recovery 実機検証 score=0.7837 Zone A)。
>
> **位置付け**: Lab v9-v15 「天井 5 連続」(v8/v9/v10/v14/v15) の打開仮説のうち **副次目的** = 構造変異とは別軸の「推論 throughput 軸」改善。Lab cycle 時間短縮 (75 min → 推定 50-60 min) で実験密度を上げ、構造変異 plan 群 (ERL / AgentFloor / AgentHER) との合算効果を狙う。
>
> **本 plan の責務**: plan 起票のみ。**production code 変更ゼロ**。実装は別 session (`/oh-my-claudecode:autopilot` or `/everything-claude-code:prp-implement`)。

## Task Type
- [x] Backend (Rust 2024 / `src/runtime/`)
- [ ] Frontend
- [ ] Fullstack
- [x] Production code 変更 = 実装 session のみ ALLOWED (本 plan session = ZERO)
- [x] Additive only (既存 `LlamaServerBackend` / `FallbackBackend` 削除なし、env opt-in)

---

## 1. 背景

### 1.1 vllm-mlx の概念

vllm-mlx (arxiv 2601.19139, 2026-01) は **vLLM の MLX backend port**。Apple Silicon の統合メモリ (unified memory architecture) を zero-copy で活用しつつ、vLLM の以下 2 大スケジューラ機構を MLX 上で動かす:

1. **PagedAttention** — KV cache を block 単位で管理、断片化解消で大 batch 投入時のメモリ効率を 2-4x 改善
2. **Continuous Batching** — request 単位ではなく iteration 単位で batch を再構成、待ち時間 ≈ 0 で同時 N request を捌く

論文での実測:
- **llama.cpp 比 21-87% throughput 向上** (Qwen3-0.6B 〜 Nemotron-30B 帯域)
- **M4 Max で 525 tok/s** (single request、Qwen3-8B)
- **continuous batching で 16 並列 → 4.3x throughput** (バッチ化スケーリング)
- API は **OpenAI 互換** (vLLM 既存仕様を継承、`/v1/chat/completions` + SSE streaming)

### 1.2 既存 `LlamaServerBackend` との対比

`src/runtime/llama_server.rs` (848 行、LlamaServerBackend) の現状:

| 観点 | LlamaServerBackend (現状) | vllm-mlx (target) |
|------|---|---|
| Server | `llama-server` (llama.cpp、GGUF 1bit) | `python -m vllm.entrypoints.openai.api_server` (MLX backend) |
| API | OpenAI 互換 `/v1/chat/completions` | **OpenAI 互換** (同 endpoint、互換性あり) |
| Streaming | SSE (`stream: true`) | **SSE (vLLM 既存仕様)** |
| Batching | **request 直列** (1 request = 1 推論) | **continuous batching** (iteration 単位 N 並列) |
| KV cache | flash-attn `on` + `--cache-type-k/v` | PagedAttention (block 16 token default) |
| MLX 互換 | `mlx_compatible=true` で `top_k`/`min_p`/`repeat_penalty` を除外 (項目 67) | vllm OpenAI 互換 layer に依存 (要 Phase 0 実機確認) |
| sticky fail | 項目 195/198 で `recover_after_n_success=10` 機構 | 同機構踏襲 (FallbackChain 配下) |

**重要観測**: `LlamaServerBackend` は **mlx-lm server も同じ class で扱う**設計 (`with_mlx_compatible(true)`、main.rs:470)。vllm-mlx も OpenAI 互換のため「`LlamaServerBackend` を OpenAI 互換 backend として general 化 → vllm-mlx 専用機能 (batch hint) を別 trait method で拡張」という選択肢が浮上 (4.6 で詳述)。

### 1.3 MLX 経緯と本 plan の意義

| 項目 | 内容 | 本 plan との関係 |
|------|------|-----|
| 173 | MLX 環境劣化仮説 (mlx-lm hang / degrade) | vllm-mlx で別実装に移行することで **mlx-lm 1 並列実装の脆弱性を回避**できるか検証 |
| 184 | MLX core 22 vs llama-server REJECT (score=0.7976、Zone A 寄り C) | 本 plan の **比較対象**。vllm-mlx が項目 184 score を上回れば「MLX 系の優位を実装層で発揮」が確証 |
| 195 | MLX sticky fix (`recover_after_n_success=10`) | vllm-mlx でも同 sticky pattern が起こり得る → FallbackChain 配下で機構踏襲 (4.5) |
| 198 | sticky recovery 実機検証 (core 22 score=0.7837、recovery 2 件発動) | **production default 残置決定**。vllm-mlx も同 default で開始 |

### 1.4 「Scaffolding > Model」原則と整合

CLAUDE.md 巻頭原則 = 「1bit モデル改善余地は限定的、ハーネス側で底上げ」。本 plan は **モデル変更ゼロ** (Bonsai-8B 1bit ternary 維持) で **推論層を別実装に置換**する。改善経路は:
- **Lab cycle 時間短縮** → 同 wall 時間で実験本数 1.3-1.5x 増 → ACCEPT 確率向上 (期待値計算、Lab v17 12 cycle/15h → 18 cycle/15h)
- **k=3 batch 並列化** → 同タスク 3 run を 1 server request で投入 → variance 削減 (項目 200 stability 改善)

---

## 2. 目的

1. **M2 16GB 上 throughput 向上** — Bonsai-8B 1bit (1.28GB) で llama-server (現状) 比 +30% 以上の throughput を実測。論文値 21-87% の下限保守的見積もり。
2. **Lab cycle 時間短縮** — Lab v17 の 1 cycle ≈ 75 min を **50-60 min** に短縮 (k=3 batch 並列が効けば 60 min 切り目標)、同 wall で実験密度 +25-50%。
3. **k=3 同時実行 (continuous batching)** — `MultiRunBenchmarkSuite::run_k` の `k` 内ループを backend 側 batch hint で 1 request に集約、variance (RDC/VAF、項目 200) を再評価する base data 取得。

### 非目標
- `LlamaServerBackend` の **削除** (default 維持、env opt-in、Lab v17 default 0.7560 path 完全保護)
- vllm-mlx の MLX backend 自体の改造 (upstream 利用のみ、bonsai 側は HTTP client のみ)
- mlx-lm server (現存 `MlxLm` variant) の **削除** (項目 195/198 sticky 機構 + production default 残置決定 と独立して並走、3 backend 共存: llama-server / mlx-lm / vllm-mlx)
- Bonsai-8B model 自体の差替 (1bit ternary 維持、tokenizer 等の変更なし)
- Cross-Family Speculative Decoding (arxiv 2604.16368、領域 3 別 plan 候補) の同時実装

---

## 3. 既存項目との関係

| 項目 | 内容 | 本 plan との関係 |
|------|------|-----|
| 35, 36 | LlamaServerBackend 基盤 (Step 12 fallback chain wrap) | **2nd backend 並走**、wrap 順序 `Cached(Fallback(Llama vs Vllm))` 維持 |
| 49 | flash-attn `on` 強制 | vllm-mlx は PagedAttention で別経路、flash-attn 不要 (Phase 0 確認) |
| 53 | sse_chunk_timeout_secs (config 駆動 60-180s) | **vllm-mlx でも踏襲**、初期値 180s (MLX 系推奨、項目 112) |
| 56, 60, 61 | LlamaServerBackend mlx 互換モード (項目 67 top_k 除外等) | **vllm OpenAI 互換 layer の挙動を Phase 0 で実測**し、`with_mlx_compatible(false)` か `true` かを決定 |
| 63 | SSE parse + non-streaming fallback | **vllm-mlx でも同 logic 流用** (build_request_body / parse_sse_stream をほぼ直接再利用) |
| 67 | mlx_compatible モード (top_k/min_p/repeat_penalty 除外) | vllm-mlx は OpenAI 仕様 strict なので **`top_p`/`temperature`/`max_tokens` のみ採用** が無難 (4.2 で詳述) |
| 90, 103, 105 | LlamaServerProcess 起動管理 | vllm-mlx の起動は **本 plan では out-of-scope** (user-started、`bonsai --diagnose` で health check のみ) |
| 130 | streaming_agent (timeout_recv_body) | **そのまま流用**、追加なし |
| 167, 168 | FallbackChain (Step 12) + recovery | vllm-mlx を `[[fallback_chain.entries]]` の primary 候補として登録可、既存 chain 機構が transparent に支える |
| 174 | β fix (FallbackChain state recovery) | 既存機構踏襲、本 plan で再設計なし |
| 195 | MLX sticky fix (`recover_after_n_success=10`) | vllm-mlx でも同 sticky 起こり得る → **FallbackChain 配下で機構流用** (4.5) |
| 198 | sticky recovery production default 残置 | 同 default 維持、vllm-mlx 用に閾値変更ナシ |

---

## 4. 設計

### 4.1 `VllmMlxBackend` struct (新規)

`src/runtime/vllm_mlx.rs` (新規ファイル) に実装:

```rust
//! vllm-mlx (vLLM MLX backend) HTTP client。
//!
//! OpenAI 互換 endpoint (`/v1/chat/completions`) で `LlamaServerBackend` と同等の
//! generate API を提供。差分は (1) request body の paramater whitelist (vLLM strict)
//! (2) batch hint API (4.4) (3) PagedAttention 関連 metric の取得 (任意)。
//!
//! 由来: arxiv 2601.19139 / CLAUDE.md 項目 (本 plan implements).

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::agent::conversation::Message;
use crate::cancel::CancellationToken;
use crate::config::InferenceParams;
use crate::runtime::http_agent::{shared_agent, short_agent, streaming_agent};
use crate::runtime::inference::{GenerateResult, LlmBackend, TokenUsage};
use crate::tools::ToolSchema;

pub struct VllmMlxBackend {
    base_url: String,
    model_id: String,
    inference: InferenceParams,
    /// vLLM strict OpenAI mode: `top_k` / `min_p` / `repeat_penalty` を除外
    /// (vllm OpenAI 互換 layer は extra param を 422 で reject するケースあり、Phase 0 実測)
    strict_openai: bool,
    seed: u64,
    sse_chunk_timeout_secs: u64,
}

impl VllmMlxBackend {
    pub fn connect(base_url: &str, model_id: &str) -> Self { /* ... */ }
    pub fn connect_with_params(base_url: &str, model_id: &str, inference: InferenceParams) -> Self { /* ... */ }
    pub fn with_strict_openai(mut self, strict: bool) -> Self { /* ... */ }
    pub fn with_seed(mut self, seed: u64) -> Self { /* ... */ }
    pub fn with_sse_timeout(mut self, secs: u64) -> Self { /* ... */ }
    pub fn is_healthy(&self) -> bool { /* /health → /v1/models 経路 */ }

    fn build_request_body(&self, messages: &[Message], tools: &[ToolSchema]) -> serde_json::Value {
        // strict_openai: temperature / top_p / max_tokens / stream のみ
        // strict_openai=false: top_k / min_p / repeat_penalty も含める (Phase 0 で 422 が出れば true 切替)
    }

    fn parse_sse_stream(&self, /* ... 既存 LlamaServerBackend と同型 */) -> Result<(String, usize, usize)> { /* ... */ }
    fn generate_non_streaming(&self, /* ... */) -> Result<GenerateResult> { /* ... */ }
}

impl LlmBackend for VllmMlxBackend {
    fn model_id(&self) -> &str { &self.model_id }
    fn generate(&self, /* ... */) -> Result<GenerateResult> { /* SSE 経路 → non-streaming 経路 */ }
    // generate_with_params はデフォルト実装で OK (内部で generate に delegate)
}
```

**設計判断**: `LlamaServerBackend` を generic 化せず **別 struct に複製** (estimated +250-350 行)。理由:
- `LlamaServerBackend` の `mlx_compatible` flag は **mlx-lm server 専用**の歴史的経緯あり、vllm-mlx の strict_openai と semantics が異なる
- 共通化は 7-Risks R7 (semantics 混合) を生むため YAGNI
- 既存 LlamaServerBackend の test 22 件 (line 517-846) を**完全保護** (signature 変更ゼロ)

### 4.2 HTTP API spec (vllm OpenAI 互換 endpoint)

vllm-mlx は upstream vLLM の OpenAI 互換 layer を継承。bonsai 側で利用する endpoint:

| Endpoint | Method | 用途 |
|---|---|---|
| `/health` | GET | health check (vLLM 標準) |
| `/v1/models` | GET | health check 代替経路 (vLLM 標準) + model_id 確認 |
| `/v1/chat/completions` | POST | text generation (SSE streaming + non-streaming) |

**Request body (strict_openai=true、recommended initial)**:
```json
{
  "model": "Bonsai-8B",
  "messages": [{"role": "user", "content": "..."}],
  "temperature": 0.7,
  "top_p": 0.9,
  "max_tokens": 1024,
  "stream": true,
  "seed": 42
}
```

**Request body (strict_openai=false、Phase 0 で 422 出なければ採用)**:
```json
{
  ...,
  "top_k": 40,
  "min_p": 0.05,
  "repetition_penalty": 1.1
}
```

**Response (SSE chunk、vLLM 仕様)**:
```
data: {"choices":[{"delta":{"content":"Hello"}}]}

data: {"choices":[{"delta":{"content":" world"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}

data: [DONE]
```

→ **既存 `parse_sse_stream` (LlamaServerBackend line 211-268) と完全互換**。コピー or 共通 helper 抽出 (4.6)。

### 4.3 既存 `LlamaServerBackend` との切替機構

#### A. `ServerBackend` enum 拡張

`src/config.rs:288` の `ServerBackend` enum に variant 追加:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerBackend {
    #[default]
    LlamaServer,
    MlxLm,
    #[serde(rename = "bitnet")]
    BitNet,
    /// vllm-mlx (vLLM MLX backend、PagedAttention + continuous batching)
    #[serde(rename = "vllm-mlx")]
    VllmMlx,
}
```

#### B. env opt-in `BONSAI_BACKEND=vllm-mlx`

`config.rs:128` 周辺の backend 判定に env override を追加:

```rust
let backend = match std::env::var("BONSAI_BACKEND").ok().as_deref() {
    Some("vllm-mlx") => ServerBackend::VllmMlx,
    Some("llama-server") => ServerBackend::LlamaServer,
    Some("mlx-lm") => ServerBackend::MlxLm,
    Some("bitnet") => ServerBackend::BitNet,
    _ => /* config.toml 経路、既存挙動 */
};
```

**既存 default 完全互換**: env 未設定 + config.toml `backend = "llama-server"` (default) で挙動完全一致。

#### C. `create_backend()` 分岐拡張

`src/main.rs:453-525` の `create_backend()` に分岐:

```rust
fn create_backend(ctx: &AppContext) -> Box<dyn LlmBackend> {
    if ctx.mock { /* 既存 */ }
    if let Some(chain) = ctx.app_config.fallback_chain.build_chain() {
        // FallbackChain entries も VllmMlx 対応 (4.5)
        for entry in chain.entries() {
            let b: Box<dyn LlmBackend> = match entry.backend {
                ServerBackend::VllmMlx => Box::new(
                    VllmMlxBackend::connect_with_params(&entry.server_url, &entry.model_id, inference.clone())
                        .with_sse_timeout(sse_timeout)
                        .with_strict_openai(true),
                ),
                _ => Box::new(LlamaServerBackend::connect_with_params(/* 既存 */)),
            };
            // health check + 登録
        }
    }
    // 単一 backend 経路
    let backend = &ctx.app_config.model.backend;
    let b: Box<dyn LlmBackend> = match backend {
        ServerBackend::VllmMlx => Box::new(
            VllmMlxBackend::connect_with_params(&ctx.server_url, &ctx.app_config.model.model_id, ctx.app_config.model.inference.clone())
                .with_sse_timeout(ctx.app_config.model.sse_chunk_timeout_secs)
                .with_strict_openai(true),
        ),
        _ => Box::new(LlamaServerBackend::connect_with_params(/* 既存 */)
            .with_mlx_compatible(*backend == ServerBackend::MlxLm)
            .with_sse_timeout(ctx.app_config.model.sse_chunk_timeout_secs)),
    };
    // health check + 既存ロジック
}
```

#### D. config.toml 例

```toml
[model]
backend = "vllm-mlx"
server_url = "http://127.0.0.1:8001"   # vllm-mlx 既定 port
model_id = "Bonsai-8B"
context_length = 16384
sse_chunk_timeout_secs = 180
```

または FallbackChain に登録:

```toml
[fallback_chain]
max_failures = 2

[[fallback_chain.entries]]
backend = "vllm-mlx"
model_id = "Bonsai-8B"
server_url = "http://127.0.0.1:8001"

[[fallback_chain.entries]]
backend = "llama-server"
model_id = "Bonsai-8B"
server_url = "http://127.0.0.1:8080"
```

### 4.4 batch 並列 (k=3 を 1 request にまとめる) — **Phase B 案 (defer)**

vLLM の continuous batching は **server 側 transparent** = N 並列 request を投げると自動 batch 化される。bonsai 側選択肢:

| 案 | 内容 | Phase |
|---|---|---|
| **B-1** | server 側 transparent、bonsai は **既存 sequential 投げ** | **Phase A (本 plan 採用)** — 何もしない、server batching に依存 |
| B-2 | `MultiRunBenchmarkSuite::run_k` の k 内ループを `std::thread::spawn` で N 並列 | Phase B (別 plan、本 plan scope 外) |
| B-3 | vllm `n` parameter 利用 (`{"n": 3}` で 1 request → 3 completion) | Phase B 候補 (vLLM 仕様確認後) |

**本 plan は B-1 のみ実装**。理由:
- B-2 / B-3 は scoring logic (`MultiRunTaskScore`) との整合確認が必要 → 別 plan で TDD strict 実施
- B-1 でも server 側 batch が効けば throughput 向上は得られる (論文 16 並列で 4.3x、3 並列で推定 2.0-2.5x)
- 副次目標 (k=3 同時) は **計測データ取得後に B 案を検討**、項目 200 RDC/VAF と直交化

→ **目的 #3 (k=3 batch 並列) は Phase B として handoff、本 plan の Quality Gate は throughput +30% のみ**。

### 4.5 sticky / recovery 機構の踏襲 (項目 195 / 198)

vllm-mlx でも sticky failure pattern (連続失敗 → 別 entry へ切替 → MLX-side 健全復帰してもしばらく primary に戻らない) は起こり得る。`FallbackChain` (`src/runtime/model_router.rs`) の `recover_after_n_success` 機構は **backend-agnostic** (FallbackEntry 単位の state 機構) のため:

- `VllmMlxBackend` は `FallbackEntry { backend: ServerBackend::VllmMlx, ... }` で chain に登録すれば **production default `recover_after_n_success=10` がそのまま効く**
- 機構変更ゼロ、test 追加ゼロ (既存 `t_fallback_backend_*` 6 件が共通保護)
- vllm-mlx 自体の sticky 振る舞いの実測は **smoke phase (Phase 4) で観察記録**、production default 変更は別 plan

### 4.6 共通 helper 抽出 — **defer (本 plan scope 外)**

`LlamaServerBackend` と `VllmMlxBackend` で **`parse_sse_stream` / `estimate_tokens_*` / `format_tool_schemas` が完全重複**する。共通化選択肢:

| 案 | 内容 | 判断 |
|---|---|---|
| C-1 | trait extraction (`OpenAiCompatibleClient` trait) | YAGNI — 2 backend だけで trait は overkill、3rd backend (将来) 追加時に再評価 |
| C-2 | free function module (`runtime::openai_compat::parse_sse_stream`) | **本 plan 推奨だが scope 外** — refactor は別 commit/別 plan、本 plan は新規実装のみ |
| **C-3** | duplicate (本 plan 採用) | **YAGNI**、+200 行 duplication 受容、refactor plan を handoff TODO 化 |

→ **本 plan は C-3** (重複受容)、refactor は後続 plan で対応。重複を test で binding する (Phase 1 test #5 = parity assertion)。

---

## 5. TDD strict 5 phase

### Phase 1 — Red (test ≥ 6 件、全 fail/compile-error 確証)

`src/runtime/vllm_mlx.rs` 末尾 `#[cfg(test)] mod tests` に追加 + 一部は `src/config.rs` に追加:

| # | Test | 期待 (Red) | 期待 (Green) |
|---|---|---|---|
| 1 | `test_vllm_mlx_connect_basic` — `VllmMlxBackend::connect` で `model_id()` 一致 | compile-error | PASS |
| 2 | `test_vllm_mlx_build_request_body_strict` — strict_openai=true で `top_k`/`min_p`/`repeat_penalty` キー不在 | compile-error | PASS、4 key only (`messages`/`temperature`/`top_p`/`max_tokens`/`stream`) |
| 3 | `test_vllm_mlx_parse_sse_basic` — vLLM 仕様 SSE 4 chunk 投入で `text="Hello world!"` | compile-error | PASS、既存 LlamaServerBackend `test_parse_sse_stream_basic` と parity |
| 4 | `test_vllm_mlx_parse_sse_cancel` — `cancel.cancel()` 後の SSE で error 返却 | compile-error | PASS、`bail!("ストリーミング中にキャンセルされました")` |
| 5 | `test_vllm_mlx_health_check_fails_without_server` — dead port で `is_healthy()=false` | compile-error | PASS |
| 6 | `test_server_backend_vllm_mlx_serde` (`config.rs`) — `"vllm-mlx"` ⇄ `ServerBackend::VllmMlx` 双方向 serde | compile-error | PASS |
| 7 (任意) | `test_vllm_mlx_env_override` — `BONSAI_BACKEND=vllm-mlx` で config の override | compile-error | PASS、env race は `serial_test` または local Mutex で隔離 |
| 8 (任意) | `test_vllm_mlx_llmbackend_trait_object` — `Box<dyn LlmBackend>` 経由で `model_id()` 呼出可 | compile-error | PASS、trait object 互換確証 |

**生成基本 / batch / error / 復帰 / env 切替 / trait 互換** の必須カバレッジ充足 (test 1=生成基本、test 5=error、test 7=env、test 8=trait、batch は Phase 4 smoke でカバー、復帰は既存 FallbackChain test 共通保護で済)。

```bash
cargo test --lib --release vllm_mlx 2>&1 | tail -10  # compile-error 確証
git add -A && git commit -m "test(vllm_mlx): Phase 1 Red — 6+ test for VllmMlxBackend"
```

### Phase 2 — Green (実装 + 全 test PASS)

実装順序:

1. **`src/config.rs`** — `ServerBackend::VllmMlx` variant 追加 + env override (test 6, 7 PASS)
2. **`src/runtime/vllm_mlx.rs`** 新規ファイル (~250-350 行):
   - `VllmMlxBackend` struct + 6 builder method (`connect`, `connect_with_params`, `with_strict_openai`, `with_seed`, `with_sse_timeout`, `is_healthy`)
   - `build_request_body` (strict_openai 分岐、4.2 spec)
   - `parse_sse_stream` (LlamaServerBackend line 211-268 を **コピー**、4.6 で defer 確認済)
   - `generate_non_streaming` (LlamaServerBackend line 271-310 を コピー)
   - `LlmBackend` trait 実装 (`model_id` / `generate`)
3. **`src/runtime/mod.rs`** — `pub mod vllm_mlx;` 追加
4. **`src/main.rs:453-525`** — `create_backend()` 分岐 + FallbackChain entries match 拡張 (3 ヶ所)

**期待**: 既存 1150 passed + 新規 6-8 = **1156-1158 passed** / clippy 0 / fmt 0 / 退行ゼロ

```bash
cargo build --release 2>&1 | tail -10
cargo test --lib --release 2>&1 | tail -3   # 1156+ passed 期待
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
cargo fmt -- --check
git add -A && git commit -m "feat(vllm_mlx): Phase 2 Green — VllmMlxBackend + ServerBackend::VllmMlx"
```

### Phase 3 — Refactor

- `VllmMlxBackend` docstring 整備 (4.1 設計判断 / 由来項目 / arxiv 2601.19139 参照を `//!` module-level に明記)
- magic number 排除: `default port 8001` を `pub const VLLM_MLX_DEFAULT_PORT: u16 = 8001;` に昇格
- error message 日本語化 (CLAUDE.md global rule、user-centric "Reason + Action")
- `with_*` builder の戻り値型を `Self` 統一 (LlamaServerBackend と同 pattern)
- 4.6 共通 helper の **plan TODO** を `// TODO(refactor): parse_sse_stream を runtime::openai_compat::parse_sse_stream に共通化、別 plan` コメントとして記録 (実装はしない)

```bash
cargo test --lib --release 2>&1 | tail -3   # 退行ゼロ確認
git add -A && git commit -m "refactor(vllm_mlx): Phase 3 — docstring + magic number + error message"
```

### Phase 4 — Smoke 検証 (実 server 起動、user-started 前提)

> **重要**: vllm-mlx の起動は本 plan scope 外 (`bonsai` プロセスは起動しない、user が事前に起動)。`bonsai --diagnose` で health check のみ。
>
> **user 起動コマンド (Quick Start §11 で詳述)**:
> ```bash
> # vllm-mlx 起動 (user side、事前に conda env / venv 構築)
> python -m vllm.entrypoints.openai.api_server \
>   --model Bonsai-8B \
>   --port 8001 \
>   --device mlx \
>   --max-model-len 16384
> ```

#### G-4a: env opt-in 経路 (新規)

```bash
BONSAI_BACKEND=vllm-mlx cargo test --release --test integration -- --ignored vllm_mlx 2>&1 | tail -10
# 期待: live integration test PASS (vllm-mlx :8001 必須)
```

新規 `#[ignore]` test 追加 (`src/runtime/vllm_mlx.rs` test mod 末尾):

```rust
#[test]
#[ignore]
fn test_generate_with_live_vllm_mlx_server() {
    let backend = VllmMlxBackend::connect("http://127.0.0.1:8001", "Bonsai-8B");
    assert!(backend.is_healthy(), "vllm-mlx が :8001 で起動していません");
    let messages = vec![Message::user("1+1は？")];
    let cancel = CancellationToken::new();
    let mut output = String::new();
    let result = backend
        .generate(&messages, &[], &mut |t| output.push_str(t), &cancel)
        .unwrap();
    assert!(!result.text.is_empty());
    assert!(result.usage.completion_tokens > 0);
}
```

#### G-4b: throughput 比較 smoke (5-task)

```bash
# 1. baseline: llama-server (既存)
BONSAI_BACKEND=llama-server BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 \
  > /tmp/bonsai-llama/llama-baseline-$(date +%s).log 2>&1
# duration / score / throughput (tok/s 平均) を記録

# 2. vllm-mlx 経路
BONSAI_BACKEND=vllm-mlx BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 \
  > /tmp/bonsai-llama/vllm-mlx-baseline-$(date +%s).log 2>&1
```

**Acceptance** (G-4b throughput):
- 完走 (panic なし、SSE timeout=0)
- duration **−25% 以上** (= throughput +33%、論文下限 21% を保守的に丸め)
- score 退行 **−0.05 以内** (smoke 5-task variance 想定範囲)

#### G-4c: FallbackChain 共存確認

vllm-mlx primary + llama-server 補助の config で smoke 実行、`[fallback_chain] entries=2 threshold=2` ログ + 完走確認。

```bash
# config.toml に [fallback_chain] entries 2 件追加 (4.3D)
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 \
  > /tmp/bonsai-llama/vllm-mlx-fallback-smoke.log 2>&1
grep "FallbackBackend を構築" /tmp/bonsai-llama/vllm-mlx-fallback-smoke.log
```

判定:
- ✅ G-4a: live test PASS
- ✅ G-4b: throughput +30% 以上 + score 退行 −0.05 以内
- ✅ G-4c: FallbackBackend 構築ログ + 完走

### Phase 5 — Commit + handoff + CLAUDE.md 項目 (新規)

5 commits:

1. `test(vllm_mlx): Phase 1 Red — VllmMlxBackend test 6+ 件追加`
2. `feat(vllm_mlx): Phase 2 Green — ServerBackend::VllmMlx + VllmMlxBackend 実装`
3. `refactor(vllm_mlx): Phase 3 — docstring + magic number + error message`
4. `feat(main): Phase 4 — create_backend + FallbackChain entries に VllmMlx 分岐`
5. `docs(claude.md): 項目 (新規) — vllm-mlx backend 統合完遂 + smoke G-4 PASS`

handoff = `memory/session_YYYY_MM_DD_handoff.md` 新規:
- throughput 実測 (G-4b duration delta)
- score 退行幅
- FallbackChain 共存確認
- 次 TODO = Phase B (k=3 同時実行 別 plan) / 共通 helper refactor 別 plan / Lab v18 vllm-mlx primary core 22 baseline

CLAUDE.md 直近項目に 1 行サマリー追加 (項目 1-219 archive pattern)。

---

## 6. API 影響

| API | 変更 | 後方互換 |
|---|---|---|
| `LlmBackend` trait | **拡張ゼロ** | ✅ 既存 1150 test 完全保護 |
| `Box<dyn LlmBackend>` | **plug-in 経由のみ** (trait object 互換性 = test 8 で確証) | ✅ |
| `ServerBackend` enum | `VllmMlx` variant 追加 | ✅ serde rename `vllm-mlx` で kebab-case 統一 / 既存 3 variant 不変 / `#[derive(Default)]` で `LlamaServer` default 維持 |
| `VllmMlxBackend` struct | **新規** | — (新規追加のみ) |
| `create_backend()` (main.rs) | match 分岐 1 ヶ所 + FallbackChain entries match 1 ヶ所 | ✅ default arm = 既存 LlamaServerBackend 経路 |
| `BONSAI_BACKEND` env | **新規** | ✅ unset で既存挙動 (config.toml 駆動) |
| config.toml `[model] backend = "vllm-mlx"` | 新値受容 | ✅ kebab-case rename / 既存値で動作不変 |
| FallbackChain `[[fallback_chain.entries]] backend = "vllm-mlx"` | 新値受容 | ✅ |
| SQLite schema | **変更なし** | ✅ |
| TSV columns | **変更なし** | ✅ |
| `LlamaServerBackend` struct | **変更ゼロ** | ✅ 既存 22 test 完全保護 |
| `MultiRunBenchmarkSuite::run_k` | **変更ゼロ** | ✅ Phase A は server 側 batch 任せ、Phase B (k=3 並列) は別 plan |

**signature 変更ゼロ** — 全 additive、項目 205 (Option A 移行) のような必須化なし。

---

## 7. Risks / Mitigations

| # | Risk | 影響 | Mitigation | Detection Signal |
|---|---|---|---|---|
| R1 | vllm-mlx の MLX backend 自体が未成熟 (alpha 段階) で hang / segfault | smoke 中断、Lab cycle 暴走 | (i) **user-started 前提** で bonsai は spawn しない (ii) `sse_chunk_timeout_secs=180` 維持 (iii) FallbackChain 配下で llama-server を 2 番目 entry として登録 | SSE timeout 多発、`/v1/models` 200 が hang、smoke duration 異常超過 |
| R2 | vllm OpenAI 互換 layer の **422 reject** (top_k/min_p 等) | 全 generate 失敗 | strict_openai=true をデフォルトに、Phase 0 で 422 が出なければ false 切替検討 (本 plan は **true 固定**で安全側) | request log の 422 / `Unprocessable Entity` |
| R3 | M2 16GB メモリ不足 (vllm-mlx + llama-server 並走で OOM) | プロセス kill、bonsai panic | (i) FallbackChain 推奨は「片方のみ起動 + entries 両方登録」、並走必須なら user 側で `vm_stat` 確認 (ii) Phase 4 smoke で `Pages free + inactive + purgeable` を pre-check | OOM kill、`vm_stat` で free RAM <2GB |
| R4 | **項目 184 教訓再発 — MLX 環境劣化仮説** (mlx-lm 1 並列実装の脆弱性が vllm-mlx でも再現) | score 退行、Lab 信頼性低下 | (i) Phase 4 G-4b で **score 退行 -0.05 以内** を Acceptance に明記 (ii) 退行検出時は env unset で即時復元 (iii) production default は llama-server 維持、vllm-mlx は opt-in のみ (iv) 項目 195/198 sticky 機構を FallbackChain 配下で踏襲 | smoke score < 0.7 (項目 188 baseline 0.7440 比 -0.04 以上)、SSE timeout 増加 |
| R5 | vllm-mlx の continuous batching が **single request では効かない** (1 並列実行で benefit なし) | 期待 throughput +30% 未達、目的 #2 未達 | (i) Phase A は B-1 (sequential 投げ) で計測、benefit が単発でも論文下限 21% で +30% gate 達成可能性あり (ii) 未達なら Phase B (k=3 並列、別 plan) で再検証、本 plan ACCEPT 不可なら **REJECT** で項目 RECORD | G-4b duration delta < -25%、tok/s metric が llama-server と同等 |
| R6 | `parse_sse_stream` 重複 (4.6 C-3) で **bug fix の片側漏れ** | 将来 LlamaServerBackend に bug fix 入った時 vllm-mlx 側に追従漏れ | (i) Phase 1 test #3 = parity assertion (LlamaServerBackend test_parse_sse_stream_basic と完全同一 input/output) (ii) refactor 別 plan を handoff TODO 化 (iii) docstring に `// TODO(refactor): parse_sse_stream を runtime::openai_compat::parse_sse_stream に共通化` 明記 | 片側 bug 検知時の grep `parse_sse_stream` で 2 ヶ所 hit |
| R7 | mlx_compatible (LlamaServerBackend) と strict_openai (VllmMlxBackend) の **semantics 混同** | 将来 maintainer が誤って共通化、retro fit 困難 | (i) docstring で「vllm strict OpenAI ≠ mlx-lm 互換」を明記 (ii) Phase 1 test 2 で strict_openai=true の whitelist 4 key を hard-code 検証 | code review で「mlx-lm 互換と同じでは?」コメント |
| R8 | vllm-mlx version drift (upstream 仕様変更で SSE format 破壊) | 全 generate 失敗 | (i) `Cargo.lock` に依存なし (HTTP client 経由のみ、Rust 側で vllm-mlx pin 不可) (ii) Phase 4 G-4a live test を `#[ignore]` で CI から外し、手動実行で検出 (iii) handoff に vllm-mlx 検証 version を記録 | live test PASS 後の prod 実行で SSE parse error |

---

## 8. Quality Gates

| Gate | 内容 | Acceptance |
|---|---|---|
| **G-1** Phase 1 Red | 6+ test compile-error or 全 fail | `cargo test vllm_mlx` で全 fail/compile-error 確証、build は通る |
| **G-2** Phase 2 Green | 6+ test PASS + 1150 維持 | 1156-1158 passed / clippy 0 / fmt 0 / 退行ゼロ |
| **G-3** Phase 3 Refactor | docstring 完備 + magic number 排除 + 既存退行ゼロ | 1156-1158 passed 維持、clippy/fmt clean、`grep "TODO(refactor)" src/runtime/vllm_mlx.rs` で共通化 plan TODO 記録 |
| **G-4a** Phase 4 live integration | live `#[ignore]` test PASS | vllm-mlx :8001 起動下で `cargo test -- --ignored vllm_mlx` PASS |
| **G-4b** throughput smoke | llama-server vs vllm-mlx 比較 (smoke 5-task) | **duration −25% 以上** (= throughput +33%) かつ **score 退行 −0.05 以内** (= smoke 5-task variance 想定範囲) |
| **G-4c** FallbackChain 共存 | vllm-mlx primary + llama-server を 2 番目 entry で smoke 完走 | `[fallback] FallbackBackend を構築` ログ + smoke 完走 + score 健全 |
| **G-5** Final | CLAUDE.md 項目追記 + handoff 起票 + 5 commits | CLAUDE.md 項目番号採番、`memory/session_YYYY_MM_DD_handoff.md` 新規、5 commits ahead |

**G-4b 未達時の判定**:
- duration −10% 〜 −24% (緩い改善) → **PROVISIONAL ACCEPT** (項目 RECORD、production default 切替なし、Phase B で再評価)
- duration ≥ 0% (改善なし) → **REJECT** (項目 RECORD、env opt-in 機構は実装残置で「切替可能性」を担保、production default 不変)
- score 退行 −0.05 超 → **REJECT** (R4 mitigation、env opt-in でも非推奨記録)

---

## 9. 完了条件

1. ✅ `ServerBackend::VllmMlx` variant 追加 (config.rs)
2. ✅ `BONSAI_BACKEND=vllm-mlx` env override 実装
3. ✅ `VllmMlxBackend` struct 実装 (`src/runtime/vllm_mlx.rs` 新規 ~250-350 行)
4. ✅ `LlmBackend` trait 実装 + `Box<dyn LlmBackend>` plug-in 互換
5. ✅ `create_backend()` (main.rs) 分岐 + FallbackChain entries match 拡張
6. ✅ Phase 1 Red 6+ test 全 fail/compile-error 確証
7. ✅ Phase 2 Green 1156+ passed / clippy 0 / fmt 0
8. ✅ Phase 4 G-4a live test PASS
9. ✅ Phase 4 G-4b throughput +30% 以上 + score 退行 −0.05 以内 (or PROVISIONAL/REJECT を明記)
10. ✅ Phase 4 G-4c FallbackChain 共存 PASS
11. ✅ CLAUDE.md 項目追記 + `memory/session_*_handoff.md` 起票 + 5 commits
12. ✅ `LlamaServerBackend` (default 経路) **変更ゼロ + 退行ゼロ** (Lab v17 default 0.7560 path 完全保護)

---

## 10. 見積もり

| Phase | 内容 | 時間 |
|---|---|---|
| Phase 0 | vllm-mlx インストール + 起動検証 (user side、conda/venv 構築 + Bonsai-8B model load + `/v1/models` 200 確認) | 3-4h (初回) / 0.5h (2 回目以降) |
| Phase 1 | Red — 6-8 test 追加 (vllm_mlx.rs + config.rs) | 1.5h |
| Phase 2 | Green — VllmMlxBackend 実装 + create_backend 分岐 + ServerBackend variant | 4.0h |
| Phase 3 | Refactor — docstring / magic number / error message | 1.0h |
| Phase 4 | Smoke 3 段 (G-4a live + G-4b throughput + G-4c FallbackChain、うち G-4b は smoke 5-task × 2 経路 = ~30 min 実機) | 3.0h (実機 wall 1.5h) |
| Phase 5 | Commit + CLAUDE.md 項目 + handoff session 起票 | 1.5h |
| Buffer | R1/R2/R5 mitigation debug、Phase 0 環境差分 (Python version / wheel build 失敗等) | 3.0h |
| **合計** | | **~17h ≈ 2-3 day** (Phase 0 含む、初回) / **~13h ≈ 1.5-2 day** (vllm-mlx 既起動下) |

**critical path**: Phase 0 (vllm-mlx 起動成功) — ここで失敗するなら全体 ABORT、別 backend 検討 (mlx-lm 残置、項目 198 production default 維持)。

---

## 11. Quick Start

### 11.1 vllm-mlx インストール (user side、Phase 0)

```bash
# 1. Python 3.11+ 必須 (vllm-mlx wheel)
python --version  # 3.11+ 確認、未満なら pyenv install 3.11

# 2. venv 作成 (project root 外推奨、ホームディレクトリ等)
mkdir -p ~/.vllm-mlx && cd ~/.vllm-mlx
python -m venv .venv
source .venv/bin/activate

# 3. vllm-mlx インストール (PyPI or git source、upstream 状況で 2 通り)
# 案 A: PyPI (将来公式 release)
pip install vllm-mlx

# 案 B: git source (現状 alpha 段階の場合)
# pip install git+https://github.com/vllm-project/vllm-mlx.git
# (注: 実 URL は arxiv 2601.19139 の公式 repo 確認後、Phase 0 で確定)

# 4. Bonsai-8B model 配置
# (既存 Bonsai-8B 1bit weight が HuggingFace local cache にあればそのまま利用)
ls ~/.cache/huggingface/hub/ | grep -i bonsai

# 5. vllm-mlx server 起動 (port 8001、context 16384、MLX backend)
python -m vllm.entrypoints.openai.api_server \
  --model Bonsai-8B \
  --port 8001 \
  --device mlx \
  --max-model-len 16384 \
  > /tmp/vllm-mlx-server.log 2>&1 &

VLLM_PID=$!
echo "vllm-mlx PID: $VLLM_PID"
sleep 30  # cold start 待機 (model load)

# 6. health check
curl -fsS http://127.0.0.1:8001/v1/models    # 200 + model list 確認
curl -fsS -X POST http://127.0.0.1:8001/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"Bonsai-8B","messages":[{"role":"user","content":"hi"}],"max_tokens":5}'
# → JSON response 確認、422 なら strict_openai 必須確証
```

### 11.2 bonsai 側 build + smoke

```bash
cd /Users/keizo/bonsai-agent

# Phase 1 Red
$EDITOR src/runtime/vllm_mlx.rs    # 新規ファイル + 6+ test
$EDITOR src/config.rs              # ServerBackend::VllmMlx + env override
rtk cargo test --lib vllm_mlx 2>&1 | tail -10  # compile-error 確証

# Phase 2 Green
$EDITOR src/runtime/vllm_mlx.rs    # VllmMlxBackend 実装
$EDITOR src/runtime/mod.rs         # pub mod vllm_mlx;
$EDITOR src/main.rs                # create_backend 分岐 + FallbackChain entries match
rtk cargo build --release
rtk cargo test --lib --release 2>&1 | tail -3   # 1156+ passed
rtk cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
rtk cargo fmt -- --check

# Phase 3 Refactor
$EDITOR src/runtime/vllm_mlx.rs    # docstring + magic number + error message

# Phase 4 G-4a (live integration test)
BONSAI_BACKEND=vllm-mlx rtk cargo test --release --lib -- --ignored vllm_mlx 2>&1 | tail -10

# Phase 4 G-4b (throughput smoke、llama-server :8080 + vllm-mlx :8001 両起動下)
mkdir -p /tmp/bonsai-llama
BONSAI_BACKEND=llama-server BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 \
  > /tmp/bonsai-llama/llama-baseline.log 2>&1
BONSAI_BACKEND=vllm-mlx BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 \
  > /tmp/bonsai-llama/vllm-mlx-baseline.log 2>&1
grep -E "ベースライン:|duration|総時間" /tmp/bonsai-llama/{llama-baseline,vllm-mlx-baseline}.log

# Phase 4 G-4c (FallbackChain 共存)
# config.toml に [fallback_chain] entries 2 件追加 (4.3D)
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 \
  > /tmp/bonsai-llama/vllm-mlx-fallback-smoke.log 2>&1
grep "FallbackBackend を構築" /tmp/bonsai-llama/vllm-mlx-fallback-smoke.log

# Phase 5 commit + handoff
git log --oneline -5
$EDITOR CLAUDE.md   # 項目 (新規) 追記
$EDITOR /Users/keizo/.claude/projects/-Users-keizo-bonsai-agent/memory/session_$(date +%Y_%m_%d)_handoff.md
git add CLAUDE.md /Users/keizo/.claude/projects/-Users-keizo-bonsai-agent/memory/session_*.md
git commit -m "docs(claude.md): 項目 (新規) — vllm-mlx backend 統合完遂 + smoke G-4 PASS"
```

### 11.3 cleanup (smoke 終了後)

```bash
kill $VLLM_PID    # vllm-mlx 停止
deactivate        # venv 抜ける
# config.toml は user 任意で llama-server default に戻す (env unset で挙動不変)
```

---

## 12. 参考

### Primary
- arxiv **2601.19139** *Native LLM Inference at Scale on Apple Silicon* (vllm-mlx, 2026-01)
- `research_arxiv_2026_05_07.md` 領域 3 / ★★★ #6
- vLLM upstream docs (continuous batching / PagedAttention 仕様)
- vllm-mlx GitHub repo (Phase 0 で URL 確定)

### Bonsai 既存実装 (本 plan で **変更ゼロ**、refer-only)
- `src/runtime/llama_server.rs` (LlamaServerBackend 848 行、build_request_body / parse_sse_stream の **API contract reference**)
- `src/runtime/inference.rs` (LlmBackend trait + FallbackBackend、本 plan target trait)
- `src/runtime/model_router.rs` (FallbackChain、項目 168/174/195/198 sticky/復帰機構)
- `src/runtime/cache.rs` (CachedBackend、wrap 順序 `Cached(Fallback(...))`)
- `src/main.rs:453-525` (`create_backend()`)
- `src/config.rs:288-299` (ServerBackend enum)

### CLAUDE.md 関連項目
- 35, 36, 49, 53, 56, 60, 61, 63, 67, 90, 103, 105 (Backend 基盤)
- 130, 167, 168, 174 (FallbackChain / streaming_agent)
- 173, 184, 195, 198 (MLX 経緯)
- 200 (Beyond pass@1 RDC/VAF、Phase B k=3 並列で再評価対象)

### 派生候補 (本 plan 後の handoff TODO)
- **Phase B**: k=3 同時実行 (`MultiRunBenchmarkSuite::run_k` の k 内ループ並列化、別 plan)
- **共通 helper refactor**: `runtime::openai_compat::parse_sse_stream` 抽出 (R6 mitigation 完了)
- **Lab v18**: vllm-mlx primary + core 22 baseline (項目 184 比較)、ACCEPT で production default 切替検討
- **Cross-Family Speculative Decoding** (arxiv 2604.16368) — vllm-mlx + draft model、ただし M2 16GB tight (memory 制約 R3 mitigation 後の余裕次第)
- **PagedAttention metric 取得**: vllm-mlx の `/metrics` endpoint (Prometheus format) で KV cache utilization 観測、context_length 自動調整への伸展可能性

### Plan 品質基準 reference
- `mlx-fallback-smoke-correction-and-reproducibility.md` (Phase 0-5 構造 / Risk Matrix / Decision Gate / Backup 戦略)
- `agentfloor-tier-eval-impl.md` (TDD strict 5 phase / Quality Gate G-1〜G-5 / SQLite migration / Test 列挙)
- `agenther-option-a-migration.md` (signature 変更 + caller 更新 + smoke 再走)

---

**Plan 起票完了** — production code 変更ゼロ。実装は別 session (`/oh-my-claudecode:autopilot` or `/everything-claude-code:prp-implement`) で本 plan を input に Phase 0-5 を実行。
