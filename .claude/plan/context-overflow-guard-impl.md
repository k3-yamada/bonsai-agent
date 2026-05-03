# Plan: F1 + F2 — Context Overflow Guard 実装

> **由来**: 前 handoff (`session_2026_05_04_handoff.md`、CLAUDE.md 項目 186) Plan 2 の H6 (CONTEXT_OVERFLOW) 確証後の根本対策。Lab v15 A-side smoke で HTTP 400 が 26 件再現し、bonsai 経由で長 prompt が n_ctx=8192 を超過することが確定。本 plan は F1 (llama-server `-c 16384` 再起動) + F2 (bonsai 側 token 推定 + 強制 compaction、TDD) を統合実装する。

## Task Type

- [ ] Frontend
- [x] Backend (→ Codex architect で 1 ラウンド検証済、SESSION_ID: `019def01-d210-7e00-a909-d43525f735fd`)
- [ ] Fullstack

## Background

### 確定済み事実 (handoff 項目 186 より)

| 観点 | 値 | 影響 |
|------|-----|------|
| llama-server `-c` (現状) | **8192** | 入力 + 出力の合計上限 |
| bonsai config `context_length` (現状) | 8192 | 一致しているが hard cap として機能していない |
| `CompactionConfig.max_context_tokens` (default) | **14000** | n_ctx=8192 より **大きく圧縮が発火しない** |
| `compaction.rs::estimate_tokens` | `len()/4` (`compaction.rs:29-31`) | 日本語混在で楽観的、UTF-8 bytes ÷ 4 だと char 数 / 12 相当 |
| 既存 `compact_if_needed` 呼出位置 | `CompactionMiddleware::after_step` (`middleware.rs:246`) | 次の LLM 呼出前に発火するが threshold が無効 |
| Lab v15 A-side smoke での 400 件数 | **26 件** (5 実験中、全て `file_write` retry 連鎖中) | 1bit モデルが malformed JSON を出力 → retry → conversation 蓄積 → n_ctx 超過 |

### 設計欠陥の三重要因 (root cause analysis)

1. **設定不整合**: `max_context_tokens=14000 > n_ctx=8192` → 圧縮が発火しない
2. **token 推定の楽観性**: 日本語比率高いと実 BPE token 数の 50% 程度しか見積らない
3. **before_step guard の不在**: after_step 圧縮は次回 LLM 呼出前にしか効かないが、tool result が 1 回で context を超過させるケースを防げない

## Implementation Plan

### F1: llama-server `-c 16384` + config.toml 同期 (user action + 1 行 config)

**実行者**: user (5 min)

```bash
# 1. 既存 llama-server を停止
pkill -f "llama-server.*Bonsai-8B"

# 2. -c 16384 で再起動 (他 flag は維持)
llama-server -m /Users/keizo/Bonsai-demo/models/gguf/8B/Bonsai-8B.gguf \
  -c 16384 -ngl 99 --flash-attn on \
  -ctk q8_0 -ctv q8_0 \
  --alias Bonsai-8B \
  --port 8080 &

# 3. n_ctx 確認
curl -s http://127.0.0.1:8080/slots | jq '.[].n_ctx'
# 期待: 16384
```

**config.toml 更新**:

```diff
 [model]
 backend = "llama-server"
 server_url = "http://127.0.0.1:8080"
 model_id = "Bonsai-8B"
-context_length = 8192
+context_length = 16384
 kv_cache_type = "q8_0"
 gguf_path = "/Users/keizo/Bonsai-demo/models/gguf/8B/Bonsai-8B.gguf"
```

**VRAM 影響見積** (M2 16GB):
- n_ctx=8192 → 16384 で q8_0 KV cache が約 +1.5〜2GB 増加
- 8B モデル本体 (1bit) ≈ 1.3GB + 推論時 ≈ 4GB → 余裕あり、ただし Lab smoke 中の MLX primary も並走するため要実測
- 起動失敗時のフォールバック: `-c 12288` で再試行

**期待効果**: 即時 ~95% の HTTP 400 削減 (handoff 項目 186 推定)。ただし**根本対策ではない** — F2 完了まで bonsai は backend の n_ctx に依存し続ける。

---

### F2: bonsai 側 ContextOverflowGuard (TDD 3-phase)

**実行者**: Claude (2-3h、`run_in_background=true` で長時間 build/test も可)

#### Architecture / Data Flow

```text
src/config.rs
  ModelConfig.context_length: u32
        ↓
src/main.rs::create_backend / handle_*_mode で
  AgentConfig.n_ctx_budget: Option<u32> を設定
        ↓
src/agent/agent_loop/core.rs::run_agent_loop_with_session
  build_default_chain(session_id, store, config.n_ctx_budget)
        ↓
src/agent/middleware.rs::CompactionMiddleware::with_n_ctx_budget(...)
        ↓
src/agent/compaction.rs::CompactionConfig::from_n_ctx_budget(...)
  → max_context_tokens = n_ctx * 0.70 (or 14000 if None/0)
```

#### 設計の要点

1. **完全 opt-in**: `n_ctx_budget = None` で legacy 動作 (max_context_tokens=14000)、既存テスト全部温存
2. **ratio = 70%**: n_ctx の 70% を bonsai 側 budget とする (BPE 推定誤差 + tool 出力余裕分のヘッドルーム)
3. **before_step に guard を追加**: `compact_level3` を強制発火 (after_step の段階圧縮とは独立、緊急専用)
4. **再帰圧縮しない**: level3 後も超過なら `MiddlewareSignal::Abort` で graceful 停止 → LLM 呼出を防ぐ (HTTP 400 を回避)
5. **estimate_tokens のハイブリッド**: `chars/3` と `bytes*0.4` の `max` を取り、ASCII/日本語両方で保守的

#### Phase 2a (Red): failing tests

**File**: `src/agent/compaction.rs` (tests module)

```rust
#[test]
fn t_estimate_tokens_is_japanese_aware() {
    let ascii = estimate_tokens(&[Message::user("hello world")]);
    // 11 bytes -> max(chars/3=4, bytes*4/10=5) = 5
    assert_eq!(ascii, 5);

    let japanese = estimate_tokens(&[Message::user("こんにちは世界")]);
    // 7 chars / 21 bytes -> max(7/3=3, 21*4/10=9) = 9
    assert_eq!(japanese, 9);
}

#[test]
fn t_compaction_config_derives_from_n_ctx_budget() {
    let derived = CompactionConfig::from_n_ctx_budget(Some(8192));
    assert_eq!(derived.max_context_tokens, 5734); // floor(8192 * 70 / 100)

    let none = CompactionConfig::from_n_ctx_budget(None);
    assert_eq!(none.max_context_tokens, 14000); // legacy default

    let zero = CompactionConfig::from_n_ctx_budget(Some(0));
    assert_eq!(zero.max_context_tokens, 14000); // invalid -> default
}
```

**File**: `src/agent/middleware.rs` (tests module)

```rust
#[test]
fn t_context_overflow_guard_compacts_before_llm_call() {
    let mut mw = CompactionMiddleware::with_n_ctx_budget(Some(8192));
    let mut session = Session::new();
    session.add_message(Message::system("s"));
    for i in 0..12 {
        session.add_message(Message::user(format!("q{i}")));
        session.add_message(Message::assistant("あ".repeat(700)));
        session.add_message(Message::tool("い".repeat(700), format!("tool-{i}")));
    }
    assert!(estimate_tokens(&session.messages) > 6000);

    let signal = mw.before_step(&mut session, 0);

    assert!(matches!(signal, MiddlewareSignal::Ok));
    assert!(estimate_tokens(&session.messages) < 6000);
    assert!(session.messages.len() <= 6, "system + handoff + emergency_keep");
}

#[test]
fn t_context_overflow_guard_no_op_below_threshold() {
    let mut mw = CompactionMiddleware::with_n_ctx_budget(Some(8192));
    let mut session = Session::new();
    session.add_message(Message::user("short"));
    let signal = mw.before_step(&mut session, 0);
    assert!(matches!(signal, MiddlewareSignal::Ok));
    assert_eq!(session.messages.len(), 1, "短いセッションは触らない");
}

#[test]
fn t_context_overflow_guard_aborts_when_unrecoverable() {
    let mut mw = CompactionMiddleware::with_n_ctx_budget(Some(64)); // 極小 budget
    let mut session = Session::new();
    session.add_message(Message::system(&"x".repeat(10000)));
    for _ in 0..5 {
        session.add_message(Message::assistant(&"y".repeat(2000)));
    }
    let signal = mw.before_step(&mut session, 0);
    assert!(matches!(signal, MiddlewareSignal::Abort(_)));
}
```

**Red 確認コマンド**:
```bash
cargo test --release --lib agent::compaction::tests::t_estimate_tokens_is_japanese_aware
cargo test --release --lib agent::compaction::tests::t_compaction_config_derives_from_n_ctx_budget
cargo test --release --lib agent::middleware::tests::t_context_overflow_guard
```

#### Phase 2b (Green): 最小実装

**A. `compaction.rs`**: `CompactionConfig::from_n_ctx_budget` + estimator hybrid

```rust
const DEFAULT_MAX_CONTEXT_TOKENS: usize = 14_000;
const CONTEXT_GUARD_RATIO_NUM: usize = 70;   // n_ctx の 70%
const CONTEXT_GUARD_RATIO_DEN: usize = 100;

impl CompactionConfig {
    pub fn from_n_ctx_budget(n_ctx_budget: Option<u32>) -> Self {
        let mut config = Self::default();
        let Some(n_ctx) = n_ctx_budget else { return config; };
        if n_ctx == 0 { return config; }
        let derived = (n_ctx as usize)
            .saturating_mul(CONTEXT_GUARD_RATIO_NUM) / CONTEXT_GUARD_RATIO_DEN;
        config.max_context_tokens = derived.max(config.emergency_keep);
        // prune_protect_tokens が新 budget の半分を超えないようクランプ
        config.prune_protect_tokens = config.prune_protect_tokens.min(config.max_context_tokens / 2);
        config
    }
}

pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages.iter().map(|m| estimate_message_tokens(&m.content)).sum()
}

fn estimate_message_tokens(content: &str) -> usize {
    let by_chars = content.chars().count().div_ceil(3);
    let by_utf8 = (content.len() * 4).div_ceil(10); // bytes * 0.4 切り上げ
    by_chars.max(by_utf8).max(1)
}
```

> **既存テスト退行リスク**: `t_tok` は `Message::user("hello world")` に対し `estimate_tokens == 3` を期待。新 estimator は 11 bytes → `max(chars/3=4, bytes*4/10=5) = 5` で **退行する**。
>
> **採用方針**: `t_tok` を新値 `5` に更新 (token 推定の保守化はプロジェクト全体方針、項目 165 type coercion と同類)。

**B. `agent_loop/config.rs`**: `AgentConfig.n_ctx_budget`

```rust
pub struct AgentConfig {
    // ...既存フィールド
    /// LLM context budget。None で legacy compaction 動作 (max_context_tokens=14000)。
    pub n_ctx_budget: Option<u32>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            // ...
            n_ctx_budget: None,
        }
    }
}
```

明示的 literal 修正対象 (実装前に `rg "AgentConfig \\{" src` で確定):
- `src/agent/experiment.rs` (1268 行付近、benchmark 用 AgentConfig 構築)
- `src/agent/agent_loop/tests.rs` 内の test fixture
- `src/main.rs` メイン構築箇所
- `src/agent/benchmark.rs` 直接構築箇所 (ある場合)

**C. `main.rs`**: ModelConfig → AgentConfig 伝播

```rust
n_ctx_budget: if app_config.model.context_length > 0 {
    Some(app_config.model.context_length)
} else {
    None
},
```

**D. `middleware.rs`**: trait 拡張 + ContextOverflowGuard

```rust
// trait 変更: before_step の Session を &mut に変更
pub trait Middleware {
    fn name(&self) -> &str;
    fn after_step(&mut self, session: &mut Session, result: &StepResult) -> MiddlewareSignal;
    fn before_step(&mut self, _session: &mut Session, _iteration: usize) -> MiddlewareSignal {
        MiddlewareSignal::Ok
    }
}
// MiddlewareChain::run_before_step は既に `&mut Session` なので影響なし
// 既存 test middleware impl (BeforeInjectMw / BeforeAbortMw / DefaultBeforeMw) は &Session → &mut Session に更新

impl CompactionMiddleware {
    pub fn with_n_ctx_budget(n_ctx_budget: Option<u32>) -> Self {
        Self::new(CompactionConfig::from_n_ctx_budget(n_ctx_budget))
    }
}

impl Middleware for CompactionMiddleware {
    // ...既存 after_step は維持
    fn before_step(&mut self, session: &mut Session, iteration: usize) -> MiddlewareSignal {
        let estimated = estimate_tokens(&session.messages);
        if estimated < self.config.max_context_tokens {
            return MiddlewareSignal::Ok;
        }
        compact_level3(&mut session.messages, &self.config);
        let after = estimate_tokens(&session.messages);
        log_event(LogLevel::Warn, "middleware:context_guard",
            &format!("level3 applied before llm iter={iteration} before={estimated} after={after} budget={}",
                self.config.max_context_tokens));
        if after > self.config.max_context_tokens {
            return MiddlewareSignal::Abort(format!(
                "context overflow remains after emergency compaction: tokens={after} budget={}",
                self.config.max_context_tokens));
        }
        MiddlewareSignal::Ok
    }
}
```

**E. `build_default_chain` シグネチャ拡張**

```rust
pub fn build_default_chain<'a>(
    session_id: &str,
    store: Option<&'a MemoryStore>,
    n_ctx_budget: Option<u32>,  // 新規追加
) -> MiddlewareChain<'a> {
    let mut chain = MiddlewareChain::new();
    chain.add(Box::new(AuditMiddleware::new(session_id.to_string(), store)));
    chain.add(Box::new(ToolTrackingMiddleware::new()));
    chain.add(Box::new(CompactionMiddleware::with_n_ctx_budget(n_ctx_budget)));
    chain.add(Box::new(TokenBudgetMiddleware::default()));
    chain
}
```

**F. `agent_loop/core.rs`**: chain 構築箇所更新

```rust
// 既存 (line 149)
state.middleware_chain = crate::agent::middleware::build_default_chain(&session.id, store);
// 変更後
state.middleware_chain = crate::agent::middleware::build_default_chain(
    &session.id, store, config.n_ctx_budget,
);
```

既存テスト `build_default_chain("test", None)` (4 箇所程度) は `build_default_chain("test", None, None)` に修正。

#### Phase 2c (Refactor): 後始末

1. **AuditAction::ContextGuard** (任意、優先度低): `MultiFileNudge` (項目 171) と同パターンで監査ログに発火回数を記録 → Lab で観測可能化
2. **既存 t_tok 等の hardcoded token expectation 更新**: estimate_tokens 値変更による退行を 1 commit でまとめて修正
3. **CLAUDE.md 項目追記**: `Step 14 ContextOverflowGuard` (項目番号は実装時点で連番割当、現状の最終 = 186)

### Phase 2d: 検証 + 計測

```bash
# Unit tests
cargo test --release --lib agent::compaction agent::middleware agent::agent_loop

# Clippy
cargo clippy --release --lib -- -D warnings

# Format
cargo fmt --check

# Integration: smoke で 400 件数を測定 (F1+F2 後)
BONSAI_LAB_SMOKE=1 cargo run --release -- --lab --lab-experiments 5 \
  2>&1 | tee /tmp/bonsai-llama/lab-v15-smoke-after-f1f2-2026-05-XX.log

# 期待: HTTP 400 件数が 26 → 0 (推定) or <5 (実測許容)
grep -c "http status: 400" /tmp/bonsai-llama/lab-v15-smoke-after-f1f2-*.log
grep -c "context overflow remains" /tmp/bonsai-llama/lab-v15-smoke-after-f1f2-*.log
```

## Key Files

| File | Operation | Description | LOC 見積 |
|------|-----------|-------------|---------|
| `src/agent/compaction.rs` | Modify | `from_n_ctx_budget`, `estimate_message_tokens`, 既存 `estimate_tokens` 置換、テスト追加 | +60 / -3 |
| `src/agent/middleware.rs` | Modify | `Middleware trait` の `before_step` 引数を `&mut Session` 化、`CompactionMiddleware::with_n_ctx_budget`/`before_step`、`build_default_chain` 引数追加、テスト追加 | +80 / -10 |
| `src/agent/agent_loop/config.rs` | Modify | `AgentConfig.n_ctx_budget` フィールド + Default | +5 |
| `src/agent/agent_loop/core.rs` | Modify | `build_default_chain` 呼出引数 1 つ追加 | +1 / -1 |
| `src/main.rs` | Modify | `app_config.model.context_length` → `AgentConfig.n_ctx_budget` 伝播 | +5 |
| `src/agent/experiment.rs` | Modify | benchmark 用 `AgentConfig` literal に `n_ctx_budget: None` 追加 | +1 |
| `src/agent/benchmark.rs` | Modify | (もし `AgentConfig` 直接構築箇所あれば) `n_ctx_budget: None` 追加 | +1 |
| `src/agent/agent_loop/tests.rs` | Modify | `build_default_chain("test", None, None)` への引数追加 + test fixtures | +5 / -5 |
| `~/Library/Application Support/bonsai-agent/config.toml` | Modify (F1) | `context_length = 16384` | +1 / -1 |
| (実機) llama-server プロセス | User restart (F1) | `-c 16384` | n/a |
| `/tmp/bonsai-llama/lab-v15-smoke-after-f1f2-XXX.log` | Write (Phase 2d) | smoke 結果ログ | n/a |

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| **R1**: estimate_tokens 過度に保守化で不要 compact 頻発 → 性能劣化 (Lab score 低下) | 0.4 byte 係数は `llama_server.rs:484` (項目 67) で実証済。Phase 2d smoke で確認。1bit モデルでは context が空くほどスコア向上の傾向 (項目 137 MCP detach 結果と整合) |
| **R2**: before_step compact_level3 が SemanticScorer 不在で粗圧縮 → 重要 message 消失 | `compact_level3` は handoff summary 構築 (項目 47-58) で品質維持済、emergency 専用なので頻発しない設計 |
| **R3**: `Middleware trait` の `before_step` 引数変更で既存実装 (BeforeInjectMw 等) が壊れる | `&Session → &mut Session` は単純拡張、既存 test middleware 3 件を一括修正 (中身は `_session` で借用変更不要) |
| **R4**: AgentConfig field 追加で実装テスト fixture 全部に修正必要 | `..Default::default()` の literal は影響なし、明示 literal のみ修正 (実装前に `rg "AgentConfig \\{" src` で件数確定) |
| **R5**: `compact_level3` 後も超過で Abort 発生時に Lab Loop が中断 | 監査ログで原因可視化、頻発する場合は emergency_keep を縮小する追加対策 (Phase 3 にて) |
| **R6**: F1 (llama-server `-c 16384`) で M2 16GB の VRAM 不足 | フォールバック: `-c 12288`、KV cache type を q4_0 に変更 (品質低下のトレードオフ)。事前に `top` でメモリ確認 |
| **R7**: F1+F2 後も 400 が残る (別の root cause) | F2 の Abort で graceful 停止 → fallback chain で llama→llama 切替も発生せず、Lab 完走可能。次セッションで詳細分析 |
| **R8**: `t_tok` 等の token expectation hardcoded test の退行 | estimate_tokens 変更時に同 commit で test 値も一括更新 (hardcode を `// new estimator: bytes*0.4 ceil` コメント付きで置換) |
| **R9**: MLX primary fallback 構成下で MLX 側 n_ctx と llama 側 n_ctx が異なる | bonsai は config.toml `context_length` 1 値しか持たないため、両 backend の最小値を採用するのが安全。本 plan では F1 で 16384 統一を前提、MLX 側も `mlx_lm.server` 起動時に同等値を指定 |

## Decision Gate

- **Phase 2a Red 完了**: 7 tests 全件 fail を確認 → Phase 2b へ
- **Phase 2b Green 完了**: 7 tests 全件 pass + 既存 1020 + 7 = 1027 passed → Phase 2c へ
- **Phase 2c Refactor 完了**: clippy 0 warning + fmt 0 → Phase 2d へ
- **Phase 2d 検証**: smoke 5 task で `http status: 400` 0 件 (推奨) or <5 件 (許容)、`context overflow remains` Abort 0 件 → **F1+F2 完了、handoff 記録**
- **Smoke で 400 件数が 26→25 等で減らない**: F2 が機能していない疑い → debug log で `level3 applied before llm` 出力数を確認、estimate_tokens の閾値を再調整 (60% に下げる)

## Test Strategy 要約

| # | Test | 入力 | 期待 |
|---|------|------|------|
| 1 | `t_estimate_tokens_is_japanese_aware` | "こんにちは世界" (7 chars / 21 bytes) | 9 (旧 5) |
| 2 | `t_compaction_config_derives_from_n_ctx_budget_some` | `Some(8192)` | `max_context_tokens == 5734` |
| 3 | `t_compaction_config_derives_from_n_ctx_budget_none` | `None` | `max_context_tokens == 14000` |
| 4 | `t_compaction_config_derives_from_n_ctx_budget_zero` | `Some(0)` | `max_context_tokens == 14000` |
| 5 | `t_context_overflow_guard_compacts_before_llm_call` | `Some(8192)` budget + 12 段会話 (>6000 tok) | `Ok` + len ≤ 6 + tokens < 6000 |
| 6 | `t_context_overflow_guard_no_op_below_threshold` | `Some(8192)` budget + 1 短メッセージ | `Ok` + 変更なし |
| 7 | `t_context_overflow_guard_aborts_when_unrecoverable` | `Some(64)` budget + 巨大 system message | `Abort(_)` |

## YAGNI / 見送り

| 案 | 見送り理由 |
|----|----------|
| **再帰 compact_level3** (Codex 提案でも明示拒否) | level3 後の再削減は handoff summary 破壊リスク、Abort で graceful 停止が安全 |
| **backend trait に `context_capacity()` 追加** | n_ctx は config 側で十分、実機 backend からの取得は将来の MLX vs llama 統一用、本 plan のスコープ外 |
| **AuditAction::ContextGuard** (発火回数ログ) | 観測のみ Phase 2d で smoke log の grep で代替、必要なら別 plan |
| **estimate_tokens を BPE tokenizer で正確化 (tiktoken 等)** | 依存追加コスト > 精度向上価値、保守的 estimator で十分 |
| **F4 (1bit retry 閾値調整)** | F1+F2 で 400 が解消するなら不要、smoke で残る場合のみ別 plan |
| **F3 (400 → 自動 retry + compact)** | F2 完了で 400 自体が発生しないため不要 |

## SESSION_ID (for /ccg:execute)

- **CODEX_SESSION**: `019def01-d210-7e00-a909-d43525f735fd` (Phase 2 backend architect 検証済、再開可能)
- **GEMINI_SESSION**: (n/a — analyzer prompt 不在のため Claude direct synthesis)

## 完了基準

1. `cargo test --release --lib`: 1020 → **1027 passed** (+7 新規 tests)
2. `cargo clippy --release --lib`: 0 warning
3. `cargo fmt --check`: 0 件
4. F1: `curl -s http://127.0.0.1:8080/slots | jq '.[].n_ctx'` → `16384`
5. config.toml `context_length = 16384`
6. Lab v15 smoke 5 task 実行で `http status: 400` 件数が **<5** (smoke ベース、target=0)
7. `context overflow remains after emergency compaction` Abort 件数 = 0 (Lab 完走を維持)
8. CLAUDE.md に項目追記 (例: 項目 187 = `Step 14 ContextOverflowGuard`)
9. handoff 記録 (`.claude/projects/.../memory/session_2026_05_XX_handoff.md`)
10. commit 5 件目安: `test(compaction)` Red → `feat(compaction)` Green core → `feat(middleware)` Green guard → `refactor(agent_loop)` 配線 → `docs(claude.md)` 項目追記
