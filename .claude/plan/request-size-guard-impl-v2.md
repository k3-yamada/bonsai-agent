# Plan v2: F3 RequestSizeGuard 実装 — CCG review 反映版 (2026-05-05)

> **由来**: v1 (`request-size-guard-impl.md`) を CCG review (Codex CONDITIONAL APPROVAL + Gemini MUST/SHOULD/CONSIDER) で v2 化。
>
> **v1→v2 主要変更 (10 件)**:
> 1. **HIGH 1 (Codex)**: Assistant content の `<tool_call>...</tool_call>` 等の **保護タグを含む場合 skip** 仕様化 (実 `Message` 構造で `content: String` のみ + `<tool_call>` JSON が content 内文字列、と確認済)
> 2. **HIGH 2 (Codex)**: disabled 互換明確化 — auto-enable (`context_length * 0.4` 自動派生) を**廃止**、`Option<u32>::None | Some(0) => disabled`、明示設定で enabled
> 3. **MEDIUM 1 (Codex)**: char count → **token estimator 統一** (compaction.rs `estimate_tokens` の `max(chars/3, bytes*0.4)` を message 単位)、field 名 `f3_max_message_tokens`、threshold `n_ctx*0.4 = 4915 tokens` (in -c 12288)
> 4. **MEDIUM 2 (Codex)**: chain 順序を実コードに整合 — 現行 4 件 `[Audit, ToolTrack, Compaction, TokenBudget]` (Stall は除外済) に F3 を挿入し **5 件** `[Audit, ToolTrack, F3, Compaction, TokenBudget]`
> 5. **MEDIUM 3 (Codex) + MUST 1 (Gemini)**: TDD 5 → **10 tests** へ拡張 + suffix 累積防止 (idempotent)
> 6. **MEDIUM 4 (Codex) + CONSIDER 1 (Gemini)**: G-4 強化 — paired baseline で `score_delta >= -0.01`、core 22 verify オプション、score 閾値矛盾解消 (v1 line 207 の `>= 0.70` → `>= 0.74` に統一)
> 7. **LOW (Codex)**: F3 log に `role/index/original_size/new_size` を出力、suffix 依存削減
> 8. **MUST 2 (Gemini) = HIGH 1 と一体**: ToolCall JSON 破壊回避を仕様化
> 9. **SHOULD 1 (Gemini)**: `AuditAction::F3SizeGuard` を**採用** (実装コスト低、observability 大)
> 10. **SHOULD 2 (Gemini)**: CLAUDE.md 項目 190 で **F3 (単発 burst) / F2 (累積) / 項目 116 (tool 出力単一切捨)** の 3 段役割分離を明記

---

## Task Type
- [x] Backend (production code 変更、TDD strict)

## Background — 実コード検証済の前提

| 観点 | 検証コマンド/出典 | 結果 |
|------|------------------|------|
| Message struct | `src/agent/conversation.rs:21-28` | `pub struct Message { role: Role, content: String, tool_call_id: Option<String> }` のみ。**`tool_calls: Vec<ToolCall>` field は Message に存在せず**、ToolCall は別 struct (line 87) で `<tool_call>...</tool_call>` JSON tag を **content 内文字列** として埋込み |
| parse.rs tool_call 抽出 | `src/agent/parse.rs:32-53` | `<tool_call>{...}</tool_call>` を find/parse、close tag 不在で `bail!` |
| 別 fmt | `src/agent/parse.rs:79+` | `<start_function_call>call:name{...}<end_function_call>` も併存、両方の保護必要 |
| 現行 chain | `src/agent/middleware.rs:353-368, 600-606` | `[audit, tool_tracking, compaction, token_budget]` 4 件、Stall は除外済 (Advisor inject_replan_on_stall が上位互換) |
| F2 token estimator | `src/agent/compaction.rs::estimate_tokens` | `max(chars/3, bytes*0.4, 1)` (項目 187 hybrid)、F3 はこれを再利用 |

## Design — 責務分離 3 段構造

```
Layer 1: tool 出力 単発切捨 (project 項目 116, truncate_tool_output)
   - tool 1 件の出力 > max_tool_output_chars で末尾切捨
   - 既存実装、無変更
                             ↓
Layer 2: F3 RequestSizeGuard (本 plan、新規)
   - session.messages 全走査、Tool/Assistant の単発 size > thresh
   - tool_call tag を含む Assistant は skip
   - 末尾切捨 + suffix `[truncated by F3 size_guard]`
   - chain 位置: Audit/ToolTrack の後、F2 の前
                             ↓
Layer 3: F2 ContextOverflowGuard (項目 187, 実装済 + 規制 OK)
   - 全 messages 累積 token > n_ctx*0.7 で compact_level3 強制
   - F3 通過後の単発巨大 (tool_call 含み skip 済) を最終防衛
   - 不足なら graceful Abort
```

## 中核仕様

### F3 struct + impl

```rust
// src/agent/middleware.rs

pub struct RequestSizeGuard {
    pub max_message_tokens: u32,  // 0 = disabled
    pub truncate_suffix: String,
}

impl RequestSizeGuard {
    pub fn new(max_message_tokens: u32) -> Self {
        Self {
            max_message_tokens,
            truncate_suffix: "\n[truncated by F3 size_guard]".to_string(),
        }
    }

    pub fn disabled() -> Self {
        Self::new(0)
    }

    fn has_protected_tags(content: &str) -> bool {
        const TAGS: &[&str] = &[
            "<tool_call>", "</tool_call>",
            "<start_function_call>", "<end_function_call>",
        ];
        TAGS.iter().any(|t| content.contains(t))
    }
}

impl Middleware for RequestSizeGuard {
    fn name(&self) -> &str { "request_size_guard" }

    fn after_step(&mut self, _: &mut Session, _: &StepResult) -> MiddlewareSignal {
        MiddlewareSignal::Ok
    }

    fn before_step(&mut self, session: &mut Session, _iteration: usize) -> MiddlewareSignal {
        if self.max_message_tokens == 0 { return MiddlewareSignal::Ok; }
        let suffix_tokens = compaction::estimate_tokens(&self.truncate_suffix);
        if suffix_tokens >= self.max_message_tokens as usize {
            return MiddlewareSignal::Ok;  // 極端 config 防護
        }
        let cutoff_tokens = self.max_message_tokens as usize - suffix_tokens;
        let mut truncated = 0_usize;

        for (idx, msg) in session.messages.iter_mut().enumerate() {
            if !matches!(msg.role, Role::Assistant | Role::Tool) { continue; }
            if msg.content.ends_with(&self.truncate_suffix) { continue; }  // idempotent
            if msg.role == Role::Assistant && Self::has_protected_tags(&msg.content) {
                continue;  // tool_call tag 保護 (HIGH 1 / MUST 2)
            }

            let cur_tokens = compaction::estimate_tokens(&msg.content);
            if cur_tokens <= self.max_message_tokens as usize { continue; }

            let original_size = msg.content.len();
            let target_chars = cutoff_tokens.saturating_mul(3);
            let prefix: String = msg.content.chars().take(target_chars).collect();
            let mut chars: Vec<char> = prefix.chars().collect();
            while !chars.is_empty()
                && compaction::estimate_tokens(&chars.iter().collect::<String>()) > cutoff_tokens
            {
                chars.truncate(chars.len().saturating_sub(64));
            }
            let truncated_text: String = chars.into_iter().collect();
            msg.content = format!("{}{}", truncated_text, self.truncate_suffix);
            truncated += 1;

            log_event(LogLevel::Info, "middleware:f3_size_guard",
                &format!("truncated role={:?} idx={} original_size={} new_size={} threshold_tokens={}",
                    msg.role, idx, original_size, msg.content.len(), self.max_message_tokens));
        }

        if truncated > 0 {
            log_event(LogLevel::Info, "middleware:f3_size_guard",
                &format!("step truncated {} messages (threshold_tokens={})",
                    truncated, self.max_message_tokens));
        }
        MiddlewareSignal::Ok
    }
}
```

### Config 統合

```rust
// src/config.rs::ModelConfig
pub struct ModelConfig {
    // ... existing fields ...
    #[serde(default)]
    pub f3_max_message_tokens: Option<u32>,  // None or Some(0) = disabled (legacy 互換)
}

// src/agent/agent_loop/config.rs::AgentConfig
pub struct AgentConfig {
    // ... existing fields ...
    pub f3_max_message_tokens: u32,
}

// src/main.rs (構築時)
let f3_max_message_tokens = cfg.model.f3_max_message_tokens.unwrap_or(0);
// ↑ HIGH 2: auto-derive を廃止、user 明示設定のみ enabled
```

### config.toml example

```toml
[model]
context_length = 12288
f3_max_message_tokens = 4915  # = 12288 * 0.4、0 or 未設定で disabled
```

### Middleware chain order

```rust
pub fn build_default_chain<'a>(
    session_id: &str,
    store: Option<&'a MemoryStore>,
    n_ctx_budget: Option<u32>,
    f3_max_message_tokens: u32,  // ← 新規引数
) -> MiddlewareChain<'a> {
    let mut chain = MiddlewareChain::new();
    chain.add(Box::new(AuditMiddleware::new(session_id.to_string(), store)));
    chain.add(Box::new(ToolTrackingMiddleware::new()));
    chain.add(Box::new(RequestSizeGuard::new(f3_max_message_tokens)));
    chain.add(Box::new(CompactionMiddleware::with_n_ctx_budget(n_ctx_budget)));
    chain.add(Box::new(TokenBudgetMiddleware::default()));
    chain
}
```

### AuditAction::F3SizeGuard (Gemini SHOULD 1)

```rust
// src/observability/audit.rs::AuditAction
pub enum AuditAction {
    // ... existing ...
    F3SizeGuard {
        role: String,
        message_index: usize,
        original_size: u64,
        new_size: u64,
        threshold_tokens: u32,
    },
}
// action_type = "f3_size_guard"
```

---

## Implementation Plan (TDD strict)

### Phase 1 (Red): failing tests 10 件 — 約 35 min

```rust
#[test] fn t_f3_truncates_oversized_assistant_message() { /* tool_call tag なし、純粋 text */ }
#[test] fn t_f3_truncates_oversized_tool_message() { /* Tool role */ }
#[test] fn t_f3_preserves_under_threshold_messages() { /* 不変 */ }
#[test] fn t_f3_disabled_when_threshold_zero() { /* no-op */ }
#[test] fn t_f3_skips_user_and_system_messages() { /* User/System 保護 */ }
#[test] fn t_f3_skips_assistant_with_tool_call_tag() { /* HIGH 1: <tool_call> 含み skip */ }
#[test] fn t_f3_skips_assistant_with_function_call_tag() { /* HIGH 1: <start_function_call> 含み skip */ }
#[test] fn t_f3_idempotent_does_not_accumulate_suffix() { /* MUST 1: 2 回通しても suffix 1 回 */ }
#[test] fn t_f3_token_estimator_handles_japanese() { /* MEDIUM 1: bytes*0.4 が効く */ }
#[test] fn t_build_default_chain_includes_f3() { /* MEDIUM 2: chain.len()==5、names 含む */ }
```

期待: 10 件 fail。

### Phase 2 (Green): 実装 — 約 65 min

1. `RequestSizeGuard` struct + `impl Middleware` + `has_protected_tags()` helper
2. `ModelConfig.f3_max_message_tokens: Option<u32>`
3. `AgentConfig.f3_max_message_tokens: u32`
4. `main.rs` で `cfg.model.f3_max_message_tokens.unwrap_or(0)` 伝播 (auto-derive 廃止)
5. `build_default_chain` シグネチャ拡張、F3 を F2 前に挿入
6. `agent_loop/core.rs` 呼出更新、`benchmark.rs` / `experiment.rs` の AgentConfig 構築箇所 3 件更新
7. `AuditAction::F3SizeGuard` variant 追加
8. F3 内で `AuditLog::log()` 呼出 (store option 経由)
9. 既存 test `test_build_default_chain_has_5_middlewares` の値を 5 件に同期

期待: **1036 passed** (1026 baseline + 10 新規)。clippy 0、fmt clean。

### Phase 3 (協調): F3+F2 統合 test 3 パターン — 約 25 min

```rust
#[test] fn t_f3_active_f2_no_fire() { /* F3 truncate 後、累積 < 70% で F2 不発火 */ }
#[test] fn t_f3_then_f2_compact() { /* F3 後でも累積 > 70% で F2 compact_level3 発火 */ }
#[test] fn t_f3_and_f2_both_active_in_one_step() { /* 両 log 出力確認 */ }
```

期待: **1039 passed**。

### Phase 4 (smoke): 約 30 min

```bash
BONSAI_LAB_SMOKE=1 cargo run --release -- --lab --lab-experiments 5 \
  2>&1 | tee /tmp/bonsai-llama/lab-v15-smoke-with-f3-2026-05-XX.log

grep "middleware:f3_size_guard" $LOG | wc -l
grep "HTTP 400" $LOG | wc -l
```

### Phase 5 (commit + handoff): 約 20 min

3 commits:
1. `test(middleware): F3 RequestSizeGuard failing tests (Red phase) - v2`
2. `feat(middleware,config,audit): F3 RequestSizeGuard 実装 + AuditAction (Green phase) - v2`
3. `test(middleware): F3+F2 chain 統合 test 3 パターン`

CLAUDE.md 項目 190 追記 (3 段役割分離 + plan path link)、handoff 作成。

---

## Decision Gates v2

| Gate | Phase | 条件 | True | False |
|------|-------|------|------|-------|
| **G-1** | Phase 1 (Red) | 10 件 fail + 1026 baseline 維持 | Phase 2 進行 | test logic 修正 |
| **G-2** | Phase 2 (Green) | 1036 passed + clippy 0 + fmt clean | Phase 3 進行 | 実装修正 |
| **G-3** | Phase 3 (協調) | 1039 passed (3 統合 test) | Phase 4 進行 | chain 修正 |
| **G-4** v2 | Phase 4 (smoke) | **以下 3 条件 AND**: ① score >= 0.74 ② HTTP 400 削減 (smoke 5 task で baseline 比 50%↓ 推奨) ③ score_delta >= -0.01 vs 同日 paired baseline | Phase 5 進行 (採用) | rollback or threshold 調整 |
| **G-4-bonus** | Phase 4 (任意) | core 22 で score >= 0.74 + 400 件数 baseline 比 50%↓ | 強い採用根拠 | smoke のみ採用 |
| **G-5** | Phase 5 | handoff + 3 commits + CLAUDE.md 項目 190 (3 段役割分離記載) | 完了 | n/a |

**v1 との G-4 差分**:
- v1: `score >= 0.74 AND HTTP 400 < 2` (絶対値、母数小で退行検知弱)
- v2: paired baseline 比率ベース、score 閾値矛盾 (v1 line 207 `>= 0.70` vs G-4 `>= 0.74`) を `>= 0.74` に統一

## Risks v2

| # | Risk | 確率 | Mitigation |
|---|------|------|------------|
| **R1** v2 | 重要 context (assistant thinking) 破壊で score 退行 | 低 | (i) tool_call tag 保護で reasoning chain は無傷、(ii) thinking 部分のみ truncate、(iii) smoke G-4 score_delta >= -0.01 で検知 |
| R2 | re-baseline 必要 | 低 | f3_max_message_tokens=0 で disabled 完全互換 |
| R3 | F3+F2 chain edge case | 低 | Phase 3 統合 test 3 パターンで検証 |
| **R4** v2 | F3 truncate で LLM 解釈失敗 | 低 | (i) suffix marker で LLM に通知、(ii) Tool role 専用安全 truncate、(iii) Assistant tool_call は skip で無傷 |
| R5 | char vs token 乖離 | **解消** | Codex MEDIUM 1 反映、token estimator 統一 |
| **R6** v2 | tool_call argument JSON 破壊 | **解消** | Codex HIGH 1 反映、`has_protected_tags()` skip で完全回避 |
| R7 | regression in 1026 passed | 低 | TDD Red→Green、既存 4→5 値変更 1 件のみ |
| R8 | smoke log で F3 fire 観測不可 | 低 | log_event(Info) + AuditAction SQLite 永続化 |
| **R9** new | suffix 累積で context 肥大化 | **解消** | Gemini MUST 1 反映、idempotent 仕様 |
| **R10** new | 極端 config (suffix > threshold) で no-op + 警告なし | 低 | Phase 1 test で仕様化、安全側 no-op で graceful |

## YAGNI v2

| 案 | v1 判定 | v2 判定 | 理由 |
|----|---------|---------|------|
| LlmBackend trait 拡張で厳密 token count | 不要 | **不要** | hybrid estimator + 安全マージン 40% で十分 |
| F3 専用 SQLite log table | 不要 | **採用** | `AuditAction::F3SizeGuard` で既存 audit_log table 追記、コスト低 |
| F3 GUI/CLI flag | 不要 | **不要** | config.toml で十分 |
| Tool role 専用 max_chars | 別 plan | **別 plan + struct 拡張余地** | 単一 threshold で開始、Tool 専用 field 追加余地は struct で残す |
| Adaptive threshold | 不要 | **不要** | 固定で十分、smoke 検証後判断 |
| extended tier 評価 | 別 plan | **別 plan** | core tier の Zone B 維持を主眼 |
| AuditAction::F3SizeGuard | 任意 | **採用** | observability 大、低コスト |

## 完了基準 v2

1. `cargo test --release --lib` **1036 passed** (Phase 2)、または **1039 passed** (Phase 3)
2. clippy 0 warning、fmt clean
3. F3 enabled で smoke score >= 0.74 + HTTP 400 baseline 比 50%↓ + score_delta >= -0.01
4. handoff + CLAUDE.md 項目 190 追記 (3 段役割分離)
5. commits 3 件 (Red + Green + 統合 test、任意 Refactor)
6. config.toml backup + plan path を CLAUDE.md 項目 190 に記載

## 推定所要時間 v2

| Phase | 所要 |
|-------|------|
| Phase 1 (Red) | 35 min |
| Phase 2 (Green) | 65 min |
| Phase 3 (協調 3 test) | 25 min |
| Phase 4 (smoke) | 30 min |
| Phase 5 (handoff + commit) | 20 min |
| **合計** | **約 3.0h** |

## CCG review 反映 traceability

| v2 変更 | Codex finding | Gemini finding |
|---------|--------------|----------------|
| has_protected_tags() skip | HIGH 1 | MUST 2 |
| auto-enable 廃止 | HIGH 2 | — |
| token estimator 統一 | MEDIUM 1 | — |
| chain order 4→5 件 | MEDIUM 2 | — |
| TDD 5→10 拡張 | MEDIUM 3 | MUST 1 |
| G-4 paired baseline | MEDIUM 4 | CONSIDER 1 |
| F3 log 詳細化 | LOW | — |
| AuditAction 採用 | — | SHOULD 1 |
| 3 段役割分離 doc | — | SHOULD 2 |
| Role 分離余地 (struct 拡張可) | — | CONSIDER 2 |

## Future Work

1. ★ MLX primary + fallback sticky 動作見直し
2. (任意) extended tier baseline + F3 効果評価
3. (任意) accept_threshold field 導入
4. (任意) bonsai 内部 task progress marker 追加
