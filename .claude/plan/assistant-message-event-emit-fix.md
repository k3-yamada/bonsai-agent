# Plan A AssistantMessage Event Emit Fix (項目 236 候補、Lab v20 第 2 層 blocker)

**状態**: planning-only (2026-05-16 起票、G-5b 実機 evidence-backed)
**推奨度**: ★★★ (Lab v20 起動の絶対前提、項目 235 案 A 単独では効力ゼロ)
**推定工数**: ~2-3h Phase 1-3 (TDD strict) + Phase 4 G-6 smoke ~25-40 min
**起点**:
- 項目 235 = factcheck Trajectory Scope Expansion Phase 1-3 完遂 (env-gated all-trajectories)
- 項目 236 候補 = 本 plan = G-5b 実機で第 2 層真因確定 (production AssistantMessage event emit 不在)
- `.claude/plan/kg-grounded-fact-check-impl.md` (Plan A 親)
- `.claude/plan/factcheck-trajectory-scope-expansion.md` (項目 235 plan、第 1 層 fix)
- `.claude/plan/lab-v20-kg-factcheck-effectiveness.md` (Lab v20 plan、本 plan が真の前提)

---

## §1. G-5b 実機 evidence (本 plan 起点 finding)

### 1.1 G-5b 観測 data
| 観測項目 | 期待 (項目 235 案 A 効果) | 実機結果 | 判定 |
|---|---|---|---|
| `[INFO][lab.factcheck]` log emit | 1 行 | 1 行 (audit_log id=9713) | ✓ wiring PASS |
| audit_log factcheck row 増加 | 3→4 件 | 3→4 件 | ✓ wiring PASS |
| total (extract triple 数) | >= 1 | **0** | ✗ **効力ゼロ** |
| matched + unknown + conflicting | >= 1 | **0** | ✗ |

### 1.2 真因 = production AssistantMessage event emit 不在
`src/agent/experiment.rs:1666-1679` (`run_factcheck_pass_lab` 内):
```rust
let events = event_store.replay(&candidate.session_id)?;
for ev in events {
    if ev.event_type == "assistant_message" {
        // ...event_data から content 抽出...
        texts.push(text);
    }
}
```

EventStore に `event_type='assistant_message'` の row が **1 件も存在しない** (any time、全 DB 検索で 0 件):
```bash
sqlite3 bonsai.db "SELECT COUNT(*) FROM events WHERE event_type='assistant_message';"
# → 0
```

`EventType::AssistantMessage` の append 箇所全 4 件、すべて test fixture:
| location | 性質 |
|---|---|
| `advisor_inject.rs:525` (`seed_verification_history`) | test helper |
| `advisor_inject.rs:643` (`mock.append(..)`) | mock backend test |
| `advisor_inject.rs:698` (`.append(.., AssistantMessage, ..)`) | test |
| `experiment.rs:3197` (`seed_session_with_assistant`) | test helper (項目 235 で追加) |

**production agent_loop の経路 (`src/agent/agent_loop/core.rs` 等) で AssistantMessage event を EventStore::append する実装が完全不在**。LLM 応答は `Session.messages` (TOML config 経由) または `Conversation` 構造体に保存されるが、`events` テーブルへの event 発行 hook が未実装。

### 1.3 項目 235 案 A 拡張は構造的に効果ゼロ
- `extract_failed_trajectories_since_id` ↔ `extract_successful_trajectories_since_id` の chain は session 選定軸の拡張のみ
- 選定後の `event_store.replay(session_id)` で AssistantMessage event がそもそも返らない = chain の片方も他方も texts 空
- `factcheck::run_factcheck_pass(&[], &graph)` → total=0 確定 (空入力で短絡)

これは Plan A §1 設計原則「failed-only でも success+failed でも、両者共に AssistantMessage event の存在を前提」と、production agent_loop の実装の **構造的 mismatch**。

---

## §2. 設計 — 3 案比較 (推奨 = 案 A、要 user 判断)

| 案 | 概要 | 採否候補 |
|---|---|---|
| **A** | `agent_loop/core.rs` で LLM 応答後に `EventStore::append(AssistantMessage)` を追加 | ★★★ 推奨 |
| B | factcheck pass を AssistantMessage event 不要に再設計 (Conversation/Session の messages から抽出) | ★★ 副案 |
| C | 両軸併用 (event emit 追加 + factcheck source 拡張) | ★ scope creep |

### 2.1 案 A (推奨): production AssistantMessage event 配線
**変更**:
- `src/agent/agent_loop/core.rs` の `run_agent_loop` 内、LLM 応答受信後の `Session::push_assistant_message` 呼出付近で:
  - `event_store.append(&session_id, &EventType::AssistantMessage, &json!({"content": &text}).to_string(), None)?`
- 既存 emit pattern (`SessionStart` / `UserMessage` / `ToolCallStart` / `ToolCallEnd` / `SessionEnd`) と統一
- non-fatal (append 失敗で agent_loop 継続)

**Pros**:
- Plan A §1 設計と整合 (AssistantMessage event ベース factcheck の原設計)
- event sourcing 完全性向上 (現状は LLM 応答だけ events から欠落 = 不対称)
- 既存 test (`seed_verification_history` 等) と production の event flow が一致 = test 信頼性向上
- 項目 235 案 A 拡張が真に効力発揮可

**Cons**:
- production agent_loop に新規 append 1 箇所追加 (副作用は audit/storage のみ)
- SQLite 書込量 +1 row/LLM-turn (1 task ~3-5 LLM turn → +3-5 row、Lab 1 cycle 22 task × 3 k → +200-330 row、許容範囲)

### 2.2 案 B (副案): factcheck source 変更
**変更**:
- `run_factcheck_pass_lab` を `event_store.replay()` 経由ではなく `Conversation::all_assistant_messages()` 経由に書換え
- session_id 経由 Session 復元 → assistant_message 抽出

**Pros**:
- production code 変更ゼロ (factcheck pass のみ修正)

**Cons**:
- Session 永続化は events 経由 (現行)、Conversation 単独復元は scope 外
- 既存 factcheck 設計 (event sourcing 軸) を破壊
- event-driven 観測の一貫性失う = future の AgentHER / hindsight extension に悪影響

### 2.3 案 C (棄却 = scope creep)
両軸併用は scope 過大、案 A 単独で 100% カバー可。

---

## §3. TDD strict 5 phase (案 A 採用想定)

### Phase 1 (Red) — 4 failing test
1. `t_agent_loop_emits_assistant_message_event_after_llm_response`
   - MockLlmBackend で 1 turn run、EventStore::replay() で `assistant_message` event >= 1 件
2. `t_assistant_message_event_data_contains_content_field`
   - emit された event の event_data JSON で `{"content": "..."}` 形式確認
3. `t_factcheck_pass_lab_extracts_triple_with_event_emit`
   - 統合 test: agent_loop run → factcheck pass → total >= 1 確認 (Phase 1 で fail)
4. `t_assistant_message_event_emit_non_fatal_on_append_failure`
   - EventStore append が Err 返しても agent_loop 継続する non-fatal 性確認

### Phase 2 (Green)
- `src/agent/agent_loop/core.rs` の LLM 応答受信後 hook で `EventStore::append` 追加
- 既存 test 1271 → 1275 passed (+4) 期待
- 既存 emit pattern (`SessionStart` 等) と統一形式

### Phase 3 (Refactor)
- log_event での emit 成否 trace 追加 (debug level)
- clippy/fmt clean

### Phase 4 (Smoke G-6a/b)
| Gate | env | 期待 |
|------|-----|------|
| G-6a | unset | 1275 pass、後方互換 (audit_log factcheck row 不変 = factcheck disabled) |
| G-6b | `BONSAI_KG_FACTCHECK_ENABLED=1 + BONSAI_FACTCHECK_ALL_TRAJECTORIES=1` | **total >= 1** (本 plan 効果)、AssistantMessage event row >= 数十件 |

### Phase 5 (Lab v20 effectiveness)
本 plan G-6b PASS で Lab v20 起動、Pearson r ≥ 0.3 paired t-test (~10-15h wall、別 session)。

---

## §4. 期待効果

### Plan A 機構の真の起動
- 項目 230 (Phase 1-4 wiring) + 項目 235 (trajectory scope expansion) + 本項目 (event emit) で 3 段配線完成
- factcheck total >= 1 観測 = effectiveness 検証経路 (Pearson r 計算) 確立

### Event Sourcing 完全性
- 現状: LLM 応答だけ events 不在 = 観測の不対称
- 本 plan 後: 全 agent turn が events 経由再現可能 = AgentHER / hindsight / future extension の前提整備

---

## §5. risks / mitigations
| # | Risk | Mitigation |
|---|------|-----------|
| R1 | SQLite 書込負荷増 (200-330 row/Lab cycle) | non-fatal append、Lab cycle ~22-30 min wall に対し SQLite write は millisecond オーダで誤差 |
| R2 | event_data JSON 形式の既存テストとの非互換 | `seed_verification_history` (line 525) と完全同型 `{"content": "..."}` で統一 |
| R3 | append 失敗で agent_loop crash | non-fatal `let _ = event_store.append(..)`、`log_event(Warn)` で記録 |
| R4 | LLM streaming で部分応答ごとに emit してしまう | 完了応答 (`Session::push_assistant_message` 呼出時) のみで emit、streaming token は対象外 |

---

## §6. ロールバック戦略
- append 1 行を `// FIXME: rollback` でコメントアウト → factcheck total=0 戻り、agent_loop 既存挙動 100% 維持
- git revert 1 commit (Phase 2 Green) で clean rollback
- 新規 test 4 件削除でも production 影響ゼロ (event emit がデフォルト経路にならない構造)

---

## §7. 起票候補項目
- **項目 236** = 完遂時 (assistant-message-event-emit-fix、test 1271→1275、G-6a/b PASS)
- **項目 237** = Lab v20 起動 (項目 230 + 235 + 236 累積 Phase 5、effectiveness paired t-test)

---

## §8. Quick Start
```bash
# 前提確認
git log --oneline -3
sqlite3 "/Users/keizo/Library/Application Support/bonsai-agent/bonsai.db" \
  "SELECT COUNT(*) FROM events WHERE event_type='assistant_message';"  # baseline=0

# Phase 1 Red
$EDITOR src/agent/agent_loop/core.rs   # locate Session::push_assistant_message
$EDITOR src/agent/agent_loop/tests.rs  # 新 test 4 件追加
cargo test --lib t_agent_loop_emits_assistant_message 2>&1 | tail -10  # 4 FAIL

# Phase 2 Green
$EDITOR src/agent/agent_loop/core.rs   # EventStore::append(AssistantMessage) 追加
cargo test --lib                       # 1275 passed

# Phase 3 Refactor + clippy/fmt
cargo clippy --tests -- -D warnings
cargo fmt -- --check

# Phase 4 G-6a/b smoke (要 llama-server + cargo build --release)
cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/g6a.log
BONSAI_LAB_SMOKE=1 BONSAI_KG_FACTCHECK_ENABLED=1 BONSAI_FACTCHECK_ALL_TRAJECTORIES=1 \
  ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/g6b.log
grep "FactCheck post-Lab" /tmp/g6b.log   # total >= 1 確証
sqlite3 ".../bonsai.db" "SELECT COUNT(*) FROM events WHERE event_type='assistant_message';"  # >= 数十

# Phase 5 = Lab v20 (別 session、~10-15h wall)
```

---

## §9. 設計選択の正当化

### なぜ「production code に append 追加」を Plan A 真の前提とするか
1. **既存 event sourcing アーキの完成**: `SessionStart` / `UserMessage` / `ToolCallStart` / `ToolCallEnd` / `SessionEnd` は emit 済 = LLM 応答だけ欠落の不対称を解消
2. **factcheck source は実装詳細**: event vs Session vs Conversation は実装手段、event を選んだ Plan A の設計判断は維持
3. **AgentHER / hindsight も AssistantMessage event を picking する想定**: 項目 202-205 で実装した AgentHER pass も同経路 = 本 fix で benefit 共有
4. **将来の event-driven extension**: replay, sleep-time consolidation, dream cycle 等の future feature が event 完全性を前提

### なぜ案 B (factcheck source 変更) ではダメか
- Plan A §1.3 「failed-only」 = 「event_store 経由 trajectory 選定」前提の設計と整合しない
- Session/Conversation 経由は session_id → 復元 → 全 message 走査の cost が event_store::replay() より大きい
- event 完全性軸での観測の一貫性失う

---

## §10. 参考
- 項目 230 Plan A KG fact-check Phase 1-4 完遂 + G-4a/b smoke 2/2 PASS
- 項目 235 factcheck Trajectory Scope Expansion (本 plan の第 1 層、案 A env-gated)
- 項目 236 候補 = 本 plan = 第 2 層真因 fix
- `src/agent/agent_loop/core.rs::run_agent_loop` (production emit 追加位置)
- `src/agent/event_store.rs::EventType::AssistantMessage` (line 10, 23)
- `src/agent/agent_loop/advisor_inject.rs:525` (test fixture の同型 emit 参照実装)
- `src/agent/experiment.rs:1666-1679` (factcheck pass の AssistantMessage picking 経路)
- `.claude/plan/factcheck-trajectory-scope-expansion.md` (項目 235 plan)
- `.claude/plan/kg-grounded-fact-check-impl.md` (Plan A 親)
- `.claude/plan/lab-v20-kg-factcheck-effectiveness.md` (Lab v20 = 本 plan + 235 + 230 累積の Phase 5)
