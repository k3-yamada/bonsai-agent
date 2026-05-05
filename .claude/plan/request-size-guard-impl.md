# Plan: F3 RequestSizeGuard 実装 — 単発 burst HTTP 400 抑制 (★★ TODO)

> **由来**: 項目 188 (F2 level3 fire 0 で真因 = 単発 request burst) + 項目 189 (Zone B 維持で F2 regression なし、HTTP 400=5/66=7.6% 残存)。F3 は F2 (累積保護) と相補的な「単発 message size guard」 middleware。
>
> **scope**: 本 plan は実装計画書、本セッションでは plan 作成のみ。次セッションで CCG review (任意) → multi-execute or direct 実装。
>
> **前提**: F2 ContextOverflowGuard (項目 187、累積保護) は production 安全と確証済 (項目 189 Zone B PASS)、F3 はその上に乗る独立 middleware。F2 を破壊しない設計。

## Task Type

- [ ] Frontend
- [x] Backend (production code 変更、TDD strict)
- [ ] Fullstack

## Background

### 真因再確認 (項目 188 / 189 統合)

| 観測事実 | 含意 |
|----------|------|
| F2 level3 fire = 0 (smoke + core 22 双方) | F2 設計対象 (累積) は実 task の failure mode (単発 burst) と一致しない |
| HTTP 400 = core 22 で 5 件 / 66 run = 7.6% | LLM が出力する単発 message (file_write の長 new_text、巨大 tool args) が n_ctx を burst |
| Abort = 0 | F2 は graceful Abort も発動せず、bonsai 側で n_ctx 超過を未然防止できていない |
| ハーネス recovery (項目 167 socket timeout + 非ストリーミング fallback) で完走 | 7.6% は recovery で吸収されているが、効率的ではない (duration overhead 推定 5-10%) |
| MLX-primary B1a 構成は 24 件、llama-only は 11/5 件 | backend に依存しない、bonsai 側で抑制すべき |

### 比較メトリクス (実装後の評価基準)

| 構成 | HTTP 400 | duration | score |
|------|---------|----------|-------|
| **F2 only (項目 189 = baseline)** | 5 | 47.5 min | 0.7560 |
| **F2 + F3 (本 plan 実装後の期待)** | < 2 (smoke 0 件 / core 22 < 2 件) | -5% (recovery 削減) | ≥ 0.74 (Zone B 以下維持) |

## Design

### 責務分離 (核心設計原則)

```
┌──────────────────────────────────────────────────────────────┐
│                  Conversation 累積 size 保護                   │
│  F2 ContextOverflowGuard (CompactionMiddleware)              │
│  - 全 messages 合計 token > n_ctx * 0.7 (=8602 @-c 12288)     │
│  - compact_level3 強制発火 → 不足なら Abort                   │
│  - 項目 187 で実装済、項目 189 で regression なし確証          │
└──────────────────────────────────────────────────────────────┘
                          ↑ 累積層
                          │
┌──────────────────────────────────────────────────────────────┐
│                  単発 message size 保護                        │
│  F3 RequestSizeGuard (本 plan)                                │
│  - 個別 message (assistant/tool) の size > threshold          │
│  - 末尾 truncate + suffix `[truncated by F3 size_guard]`      │
│  - 全 truncate 後も累積超過なら F2 が引き継ぐ                 │
└──────────────────────────────────────────────────────────────┘
                          ↑ 単発層
```

### 中核仕様

```rust
// 新規 struct (src/agent/middleware.rs)
pub struct RequestSizeGuard {
    pub max_message_chars: u32,  // 0 = disabled
    pub truncate_suffix: String,  // default = "\n[truncated by F3 size_guard]"
}

impl Middleware for RequestSizeGuard {
    fn before_step(&self, session: &mut Session) -> MiddlewareSignal {
        if self.max_message_chars == 0 {
            return MiddlewareSignal::Continue; // disabled
        }
        let mut truncated_count = 0;
        for msg in session.messages.iter_mut() {
            // user message は truncate しない (task 指示の保護)
            // system message も truncate しない
            if !matches!(msg.role, Role::Assistant | Role::Tool) {
                continue;
            }
            if msg.content.chars().count() > self.max_message_chars as usize {
                let cutoff = self.max_message_chars as usize;
                msg.content = format!(
                    "{}{}",
                    msg.content.chars().take(cutoff).collect::<String>(),
                    self.truncate_suffix
                );
                truncated_count += 1;
            }
        }
        if truncated_count > 0 {
            log_event(LogLevel::Info, "middleware:f3_size_guard",
                &format!("truncated {} messages (threshold={})",
                truncated_count, self.max_message_chars));
        }
        MiddlewareSignal::Continue
    }
}
```

### 設定 (config.toml + AgentConfig)

```toml
# ~/Library/Application Support/bonsai-agent/config.toml
[model]
context_length = 12288
f3_max_message_chars = 4915  # = context_length * 0.4 = n_ctx の 40% を 1 msg 上限

# 0 or 未設定で disabled (legacy 互換)
```

```rust
// src/config.rs: ModelConfig
pub struct ModelConfig {
    // ... existing fields ...
    #[serde(default)]
    pub f3_max_message_chars: Option<u32>,  // None or Some(0) = disabled
}

// src/agent/agent_loop/config.rs: AgentConfig
pub struct AgentConfig {
    // ... existing fields ...
    pub f3_max_message_chars: u32,  // 0 = disabled
}

// src/main.rs: AgentConfig 構築時に派生
let f3_max_message_chars = match cfg.model.f3_max_message_chars {
    Some(n) if n > 0 => n,
    _ if cfg.model.context_length > 0 => (cfg.model.context_length as f32 * 0.4) as u32,
    _ => 0, // disabled
};
```

### Middleware chain order

`build_default_chain` で **F3 を F2 の前** に挿入:

```
[Audit, ToolTrack, Stall, F3_RequestSizeGuard, F2_CompactionMiddleware, TokenBudget]
                          ^^^ NEW                ^^^ existing
```

理由:
- F3 で単発 burst を抑える → F2 で累積保護 (chain 順序が逆だと F2 が触らない単発巨大 message が残る)
- F3 が active でも F2 不発火を維持 (累積 < 70% で不要発火しない)

## Implementation Plan (TDD strict, Red→Green→Refactor)

### Phase 1 (Red): failing tests — 約 30 min

`src/agent/middleware.rs::tests` に追加:

```rust
#[test]
fn t_f3_truncates_oversized_assistant_message() {
    // 5000 chars assistant message + threshold=1000 → 1000 chars + suffix
}

#[test]
fn t_f3_truncates_oversized_tool_message() {
    // 同上、Role::Tool 対象
}

#[test]
fn t_f3_preserves_under_threshold_messages() {
    // 500 chars < 1000 threshold → 不変
}

#[test]
fn t_f3_disabled_when_threshold_zero() {
    // threshold=0 → 5000 chars message も truncate されない
}

#[test]
fn t_f3_skips_user_and_system_messages() {
    // User + System message は 5000 chars でも truncate されない (task 指示保護)
}
```

期待: 5 件 fail (struct 未実装 or impl 不在)。

### Phase 2 (Green): 実装 — 約 60 min

1. `src/agent/middleware.rs` に `RequestSizeGuard` struct + `impl Middleware`
2. `src/config.rs` ModelConfig に `f3_max_message_chars: Option<u32>`
3. `src/agent/agent_loop/config.rs` AgentConfig に `f3_max_message_chars: u32`
4. `src/main.rs` で派生計算 (n_ctx * 0.4)
5. `build_default_chain` 引数に追加、F2 の前に F3 を挿入

期待: 1031 passed (1026 + 5 新規)。

### Phase 3 (協調確認): F2 との相互作用 — 約 15 min

1. F3 truncate suffix `[truncated by F3 size_guard]` が F2 compact_level3 で preserved thinking として残らないか確認
2. F2 既存 5 tests + F3 新規 5 tests + 統合 1 test (F3 → F2 順で動く) = +6 tests 想定

期待: 1032 passed (1031 + 1 統合 test)、または整理して 1031。

### Phase 4 (smoke 検証): 実機 — 約 25 min

```bash
# config 切替 (B1b 構成、llama-only) で smoke
BONSAI_LAB_SMOKE=1 cargo run --release -- --lab --lab-experiments 5 \
  2>&1 | tee /tmp/bonsai-llama/lab-v15-smoke-with-f3-2026-05-XX.log
```

評価:
- HTTP 400 が **0-2 件** (< 5/66=7.6% from 項目 189) に減少
- score ≥ 0.70 (smoke variance 含めて Zone B 維持目標)
- F3 fire log (`middleware:f3_size_guard`) 出力 = active 確認
- 副次: F2 level3 fire は引き続き 0 のはず (F3 が単発を吸収するため)

### Phase 5 (handoff + commit): 約 15 min

3 commits 想定:
1. `test(middleware): F3 RequestSizeGuard failing tests (Red phase)`
2. `feat(middleware,config): F3 RequestSizeGuard 実装 (Green phase)`
3. (任意) `refactor(middleware): F3 audit fix` (Codex/Gemini audit 後)

CLAUDE.md 項目 190 追記、handoff `session_2026_05_XX_handoff.md`。

## Key Files

| File | Operation | Description |
|------|-----------|-------------|
| `src/agent/middleware.rs` | Modify | `RequestSizeGuard` struct + impl + 5 tests + `build_default_chain` 引数追加 |
| `src/config.rs` | Modify | `ModelConfig.f3_max_message_chars: Option<u32>` |
| `src/agent/agent_loop/config.rs` | Modify | `AgentConfig.f3_max_message_chars: u32` |
| `src/main.rs` | Modify | n_ctx \* 0.4 派生計算 + AgentConfig 構築 |
| `src/agent/benchmark.rs` | Modify | AgentConfig 直接構築箇所 (Lab/Bench でも F3 有効) |
| `src/agent/experiment.rs` | Modify | 同上 |
| `~/Library/Application Support/bonsai-agent/config.toml` | (任意 user 編集) | `f3_max_message_chars` を明示設定で override 可 |
| `.claude/plan/request-size-guard-impl.md` | (本 plan) | 本ファイル |

## Risks and Mitigation

| # | Risk | 確率 | Mitigation |
|---|------|------|------------|
| **R1** | F3 が active 過剰 → critical context (assistant の重要 reasoning) 破壊で score 退行 | **中** | threshold = n_ctx \* 0.4 で個別 message が context の 40% に達する場合のみ発火 (通常は 1-5% で大半が無関係)、smoke で score ≥ 0.74 確認後採用 |
| **R2** | F3 thresh 変えると Lab v15 baseline 変動、re-baseline 必要 | 低 | F3 disabled (`f3_max_message_chars=0`) で項目 189 baseline 完全互換、opt-in 設計 |
| **R3** | F3 + F2 chain order の edge case | 低 | Phase 3 統合 test で chain 順検証、F3→F2 順固定 |
| **R4** | F3 truncate で LLM が tool_call 解釈失敗 → recovery 増 = duration 増 | **中** | suffix `[truncated by F3 size_guard]` で LLM に通知、tool 引数中の文字列 truncate は最後の手段で `Tool` role のみ対象、Assistant role は thinking/explanation の truncate は許容 |
| **R5** | char count vs token count の乖離 (日本語混在) | 低 | hybrid estimator (項目 187) で max(chars/3, bytes\*0.4) を補助使用、char count は近似 |
| **R6** | tool_call argument の中身 truncate で valid JSON が壊れる | **中** | tool_call 部分は truncate せず content 部分のみ対象、もし JSON 中で発生したら graceful Abort へ fallback |
| R7 | regression in 1026 passed | 低 | TDD Red→Green で都度 cargo test、Green 完了で 1031 passed 確認 |
| R8 | smoke log の F3 fire 観測ができない (log 0 件) | 低 | log_event(LogLevel::Info) で warn level より緩く、必ず stderr に出る |

## Test Strategy

| 区分 | 検証 | 方法 |
|------|------|------|
| Red phase | 5 件 fail | `cargo test --release --lib` で count: 1031 - 5 = 1026 passed |
| Green phase | 1031 passed | `cargo test --release --lib` で count + F3 5 件 pass |
| Phase 3 統合 | 1032 passed (統合 test) | F3 → F2 順序の動作 |
| F3 disabled 互換 | 1026 baseline 維持 | `f3_max_message_chars=0` で項目 189 と同 score |
| smoke quality | score ≥ 0.74 | Zone B 維持確認 |
| smoke 400 削減 | < 2/15 | 7.6% → 0-13% (smoke variance 想定範囲) |
| clippy | 0 warning | `cargo clippy --release --lib --tests -D warnings` |
| fmt | clean | `cargo fmt --check` |

## YAGNI / 見送り

| 案 | 判定 | 理由 |
|----|------|------|
| LlmBackend trait 拡張で厳密 token count (tiktoken) | 不要 | hybrid estimator (項目 187) + char count 近似で十分、BPE strict は別 plan |
| F3 専用 SQLite log table | 不要 | log_event(Info, "middleware:f3_size_guard") で代替、observability 改善は別 plan |
| F3 GUI/CLI flag | 不要 | AgentConfig field + config.toml で十分 |
| Handoff summary 自動生成 (truncate 後) | 別 plan | F2 が既存で対応 (compact_level3 で handoff) |
| `Tool` role 専用 max\_chars (異なる threshold) | 別 plan | 単一 threshold で開始、必要に応じ後続 plan |
| Adaptive threshold (history dependent) | 不要 | 固定 threshold で十分、項目 84 SemanticScorer のような dynamic は別 plan |
| F3 同 plan で extended tier 評価 | 別 plan | core tier の Zone B 維持を主眼、extended は別 |
| `AuditAction::F3SizeGuard` SQLite 記録 | 任意 | log_event で代替、必要なら次 iteration |

## Decision Gates 一覧

| Gate | Phase | 条件 | True | False |
|------|-------|------|------|-------|
| **G-1** | Phase 1 (Red) | 5 件 fail + 1026 passed | Phase 2 進行 | test logic 修正 |
| **G-2** | Phase 2 (Green) | 1031 passed + clippy 0 + fmt clean | Phase 3 進行 | 実装修正 |
| **G-3** | Phase 3 (協調) | F3 + F2 chain で 1032 passed | Phase 4 進行 | chain 修正 |
| **G-4** | Phase 4 (smoke) | score ≥ 0.74 AND 400 < 2 (smoke 5 task) | Phase 5 進行 (採用) | rollback or threshold 調整 |
| **G-5** | Phase 5 | handoff + commit + CLAUDE.md 項目 190 | 完了 | n/a |

## 完了基準

1. cargo test --release --lib **1031 passed** (Phase 2 終了時)、または **1032 passed** (Phase 3 統合 test 含む)
2. clippy 0 warning、fmt clean
3. F3 enabled 状態で smoke score ≥ 0.74 + HTTP 400 < 2 (5 task)
4. handoff + CLAUDE.md 項目 190 追記
5. commits: 2-3 件 (Red + Green + 任意 Refactor)
6. config.toml backup + 本 plan path を CLAUDE.md 項目 190 に記載

## 推定所要時間

| Phase | 所要 | 実行者 |
|-------|------|--------|
| Phase 1 (Red) | 30 min | Claude |
| Phase 2 (Green) | 60 min | Claude |
| Phase 3 (協調) | 15 min | Claude |
| Phase 4 (smoke) | 25 min | Claude (background) |
| Phase 5 (handoff + commit) | 15 min | Claude |
| **合計** | **約 2.5h** | (CCG review 含めず) |

## SESSION_ID + CCG review 戦略

- 本 plan は **CCG review 推奨** (production code 変更 + middleware chain 影響大、項目 189 派生で scope 限定にできない)
- 推奨: `/oh-my-claudecode:ccg` で Codex + Gemini 並列 review、12 件想定 finding を v2 に反映後 multi-execute
- 任意: 直接実装も可 (TDD strict + smoke 検証で audit 代替)、ただし F2 (項目 187) で実証済の review pattern を踏襲が安全

## 副次効果 (期待される間接的効果)

1. **duration -5% 推定**: 7.6% の 400 recovery (項目 167 socket timeout + 非ストリーミング fallback) が削減され、平均 task 時間短縮
2. **observability 向上**: F3 fire log で「巨大 message が出力された」イベントを観測可能、LLM の異常出力 (tool_call 巨大化) の早期検知
3. **F2 chain 順序の確証**: F3→F2 で chain 順序の重要性が文書化、今後の middleware 追加時の guideline
4. **項目 188 ★★ TODO 完遂**: 次セッション TODO list の重み軽減

## Future Work (本 plan 完了後)

1. ★ MLX primary + fallback sticky 動作見直し (handoff 05-05b TODO の続き)
2. (任意) extended tier baseline + F3 効果評価
3. (任意) `accept_threshold` field 導入 (smoke 補正の真の false-accept 防止)
4. (任意) bonsai 内部 task progress marker 追加 (`[lab] task X/22 complete`)
