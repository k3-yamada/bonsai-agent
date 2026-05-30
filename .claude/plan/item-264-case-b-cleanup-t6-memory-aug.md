# 項目 264 案 D-2 case B Cleanup — T6 memory aug infrastructure 完全削除

**起票日**: 2026-05-31
**起源**: 項目 266 G-PAIRED-265 DEFINITIVE REJECT (Cohen's dz=-10.60、paired evidence overwhelming destructive)
**前提 memo**: `item_264_d2_cleanup_audit_2026_05_30.md` case B 実施条件 (paired re-eval REJECT 確証) = **本 plan で満たされた**
**優先度**: ★★★ (paired evidence で REJECT 確定、infrastructure 維持コスト > 削除コスト に転換)
**production code touch**: あり (1 module 削除 + benchmark.rs 3 wiring + 6 test 削除)

---

## 1. 背景

### 1.1 削除根拠

項目 266 G-PAIRED-265 で 4 paired ABAB...AB を実機検証、4 paired 全て consistent:

| pair | A (MEMORY_AUG=0) | B (MEMORY_AUG=1) | Δ (B-A) |
|------|------------------|------------------|---------|
| 1 | 0.8536 | 0.7093 | -0.1443 |
| 2 | 0.8267 | 0.6779 | -0.1488 |
| 3 | 0.7698 | 0.6286 | -0.1412 |
| 4 | 0.7692 | 0.6498 | -0.1194 |

**統計**: mean Δ = -0.1384、Cohen's dz = -10.60、paired t = -21.2 = statistically overwhelming destructive。

`item_264_d2_cleanup_audit_2026_05_30.md` §4 case B 実施条件 (「paired re-eval REJECT 確証後の ~2h cleanup」) = **paired evidence で confirmed**、本 plan で実施。

### 1.2 残置 cost

- 認知負荷: src/agent/t6_memory_aug.rs (~270 LOC) + benchmark.rs 3 wiring site が project codebase に常在
- test 維持: 6 test (t6_memory_aug::tests::* 5 件 + t_benchmark_suite_t6_memory_aug_appends_history_in_session 1 件) が cargo test --lib 経路で常時 run
- binary 増: production binary に ~270 LOC + 1500 token system prompt 用 buffer
- 心理的負担: paired REJECT 確定後も「いつ復活させるか」の意思決定が project context に残る

### 1.3 削除実施で得るもの

- bonsai 設計原則「Scaffolding > Model」の確認 (REJECT 機構は速やかに撤去、半完成状態を温存しない)
- benchmark.rs の wiring site 3 件 (#2114/#2252/#2374) 削除で benchmark loop の cognitive complexity 低下
- cargo test --lib 1378 → 1371 (-7) で test count 圧縮、test execution time 微減

---

## 2. ゴール

1. **`src/agent/t6_memory_aug.rs` 完全削除** (~270 LOC)
2. **`src/agent/mod.rs` の `pub mod t6_memory_aug;` 削除** (1 行)
3. **`src/agent/benchmark.rs` の import + 3 wiring site + 2 t6_history local Vec 削除**
4. **6 test 削除** (5 t6_memory_aug::tests + 1 benchmark wiring test)
5. **`BONSAI_T6_MEMORY_AUG` env reference 全削除** (docs/execution/runbook.md env table 含む)
6. **cargo test --lib 1378 → 1371 (-7、退行ゼロ確証)** / clippy clean / fmt clean / drift linter All PASS
7. **backward compat**: env unset で既存挙動と完全同等 (削除前後で env unset 経路の behavior 同一)

---

## 3. 削除手順 (atomic、~2h)

### Phase 1 (~30 min): infrastructure 削除
1. `rm src/agent/t6_memory_aug.rs` (~270 LOC、6 test 含む)
2. `src/agent/mod.rs:20` の `pub mod t6_memory_aug;` 削除 (1 行)
3. `src/agent/benchmark.rs:6-7` の import 削除:
   ```rust
   use crate::agent::t6_memory_aug::{
       T6SuccessRecord, augment_system_prompt_with_memory, tokenize_task_input,
   };
   ```
4. `src/agent/benchmark.rs:2114, 2252, 2374` 3 wiring site delete (各 5-7 行):
   - augment_system_prompt_with_memory(...) 呼出
   - Vec<T6SuccessRecord> push hook
   - integration test fixture
5. `src/agent/benchmark.rs` の 2 local Vec<T6SuccessRecord> 削除

### Phase 2 (~15 min): docs / runbook update
6. `docs/execution/runbook.md` env table から `BONSAI_T6_MEMORY_AUG` 行削除
7. `scripts/g_paired_265_v2.sh` の MEMORY_AUG 切替 comment / runner 削除 (script 全体は次 phase で削除判断、PER 別 plan で `g_paired_265_v2` 自体を delete recommend)

### Phase 3 (~15 min): test verification
8. `rtk cargo test --lib 2>&1 | tail -5` で 1378 → 1371 (-7) 退行ゼロ確認
9. `cargo clippy --lib -- -D warnings 2>&1 | tail -5` clean
10. `cargo fmt -- --check src/agent/{mod,benchmark}.rs` clean
11. `cargo test --test structural 2>&1 | tail -3` で Z-4 4 passed 維持

### Phase 4 (~30 min): commit + push + docs
12. atomic commit: `refactor(t6_memory_aug): 項目 266 case B 実施 — REJECT 確定後の infrastructure 完全削除`
13. CLAUDE.md 項目 267 entry append (削除完了 record)
14. archive (harness_patterns_archive.md) 267 verbatim sync
15. drift linter run_lint.sh All PASS 確認
16. `git push origin master`

### Phase 5 (~30 min): handoff update + cleanup audit close
17. memory `item_264_d2_cleanup_audit_2026_05_30.md` の action items table を更新 (case B 実施完了)
18. session_2026_05_31a_handoff.md 起票 (本 session 全成果 record)
19. MEMORY.md index entry update

---

## 4. ACCEPT 条件

### 4.1 構造的 ACCEPT
- (a) `src/agent/t6_memory_aug.rs` not exists
- (b) `grep -rn 't6_memory_aug\|T6_MEMORY_AUG\|T6SuccessRecord\|augment_system_prompt_with_memory' src/ tests/ scripts/` で hit ゼロ (docs/ + memory/ + archive/ は historical reference として残置可)
- (c) `pub mod t6_memory_aug;` `src/agent/mod.rs` から除去
- (d) `BONSAI_T6_MEMORY_AUG` env reference src/ + scripts/ から除去

### 4.2 functional ACCEPT
- (e) cargo test --lib **1378 → 1371 passed** (-7 退行、退行ゼロ確証)
- (f) clippy clean (No issues found)
- (g) fmt clean (cargo fmt --check)
- (h) cargo test --test structural 4 passed (Z-4 layer linter)
- (i) drift linter run_lint.sh All PASS

### 4.3 documentation ACCEPT
- (j) CLAUDE.md item 267 entry append
- (k) archive 267 verbatim sync (drift cross-ref 維持)
- (l) docs/execution/runbook.md env table BONSAI_T6_MEMORY_AUG 行削除
- (m) memory `item_264_d2_cleanup_audit_2026_05_30.md` の action items 更新 (case B done)

---

## 5. Rollback strategy

### 緊急 revert (削除直後)
- `git revert <cleanup-commit-sha>` で全削除を一括 revert
- env unset 状態で挙動同等のため、production 影響ゼロ (削除 commit 自体が backward compat 維持)

### 部分 rollback (将来再評価時)
- git history (commit `c97ec9a..458a175` の 4 commits) で原状復元可能
- ただし bonsai 既存 design pattern を逸脱する可能性のため、新規実装が推奨

---

## 6. 影響範囲

### 削除対象 (production code + test):
| ファイル | 削除内容 | LOC 削減 |
|---------|---------|---------|
| `src/agent/t6_memory_aug.rs` | 全削除 | -270 |
| `src/agent/mod.rs` | `pub mod t6_memory_aug;` 1 行 | -1 |
| `src/agent/benchmark.rs` | import (2 行) + 3 wiring site (各 5-7 行) + 2 local Vec | ~-20 |
| **合計** | | **~-291 LOC** |

### 削除対象 (docs / scripts):
| ファイル | 削除内容 |
|---------|---------|
| `docs/execution/runbook.md` | env table BONSAI_T6_MEMORY_AUG 行 (1 行) |
| `scripts/g_paired_265_v2.sh` | 削除全体 (95 LOC、別 plan、本 plan scope 外) |

### 影響なし (保持):
- `~/.claude/.../memory/item_264_d2_cleanup_audit_2026_05_30.md` (historical record)
- `~/.claude/.../memory/harness_patterns_archive.md` の 264/266 entries (historical record)
- CLAUDE.md 264/266 entries (historical record、267 entry で削除 record append)
- `.claude/plan/agentfloor-t6-kg-augmented-phase2.md` (起源 plan、historical reference)

---

## 7. 依存 + cross-references

- 起源: 項目 266 G-PAIRED-265 DEFINITIVE REJECT (paired evidence)
- 前提 memo: `item_264_d2_cleanup_audit_2026_05_30.md` case B 実施条件
- 連動 plan: `.claude/plan/lab-v22-paired-metric-mandatory.md` Phase 2 完了 record (本 plan で target #2 final close)
- 関連項目: 264 (案 D-2 起源) / 265 (max_context Phase 1-3、構造的 finding) / 266 (paired evidence REJECT)

---

## 8. follow-up (本 plan ACCEPT 後の次手)

1. ★★★ 項目 263 ratio tune paired re-eval (`g_paired_263_v2.sh`、本 plan と並列実行可能)
2. ★★ Phase 6 plan (kg 30%) 保留継続 (前提崩壊継続、G-MCT2 構造的 finding 解消なし)
3. ★ `scripts/g_paired_265_v2.sh` 自体の削除判断 (T6_MEMORY_AUG env 削除後、runner script の purpose が消失)
4. ★ Z-NEW-E plan (deny-by-default tool whitelist) と統合検討 (項目 264 D-2 削除済前提で readonly tool 設計)
