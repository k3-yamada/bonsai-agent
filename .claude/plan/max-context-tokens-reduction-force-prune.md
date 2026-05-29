# max_context_tokens 縮小 plan — 4 軸 prune 強制発火による真効果測定

**起票日**: 2026-05-30
**前提**: 項目 264 G-T6-D-2 REJECT + DEFINITIVE root cause finding (全 3 smoke で実 prune 不発火)
**関連 plan**: `.claude/plan/dynamic-budget-phase6-kg-microtune.md` (§9 前提崩壊警告)
**関連 memory**: `compaction_budget_static_finding_2026_05_30.md`
**優先度**: ★★★ (項目 263 真効果再評価 + Phase 6 plan ACCEPT/REJECT 判定の prerequisite)
**production code touch**: あり (defaults 1 行 + test 数件、env-gated wiring 既存)

---

## 1. 背景: なぜ縮小が必要か

### 1.1 観測 finding (項目 264 session)

全 3 smoke (G-DB-R-2 / G-DB-R-3 / G-T6-D-1) で `compaction.budget` emit 多数だが prune marker 全て 0:

| smoke | compaction.budget emit | `[prev:` | `[summarized]` | `[saved:` |
|-------|------------------------|----------|----------------|-----------|
| G-DB-R-2 mixed | 48 | **0** | **0** | **0** |
| G-DB-R-3 T6 | 42 | **0** | **0** | **0** |
| G-T6-D-1 baseline | 45 | **0** | **0** | **0** |

→ **production smoke では実 compaction prune が一切発火していない**。

### 1.2 root cause (compaction.rs:756-790 解析)

```
compact_if_needed():
  if estimate_tokens(messages) > max_context_tokens * 3/4 → compact_level1
  if estimate_tokens(messages) > max_context_tokens * 9/10 → compact_level2
  if estimate_tokens(messages) > max_context_tokens     → compact_level3
```

`max_context_tokens` の default = **14000** (compaction.rs:35)。
Smoke `BONSAI_LAB_TASK_LIMIT=5` × T6 lh_* tier では session token が **14000 × 0.75 = 10500** に達せず、level1 すら発火しない。

`BONSAI_DYNAMIC_BUDGET=1` の効果は実質 **`compaction.budget` log emit のみ** (compact_if_needed 内 wiring も `allocated` の axis-priority prune も level1/2 発火時のみ意味を持つ dead code path)。

### 1.3 影響: ratio tune 真効果測定不可能

項目 263 default 30/30/15/25 / 項目 264 案 D-2 等の BUDGET 系 feature ACCEPT/REJECT は全て:
- ratio が compaction 実行時に kg/entities/summary/buffer の bytes 切詰めに影響する
という前提に依存。しかし production smoke で level1/2 不発火 = ratio 効果ゼロ。

→ 項目 263 ACCEPT 判定 (+9.5% G-DB-R-3 / -0.32% G-DB-R-2) と 項目 264 -12.0% finding は両方とも **measurement noise (H-A4)** の可能性。Phase 6 plan (kg 25→30%) もこの状態で ACCEPT/REJECT 判定不能。

→ **prune 強制発火経路を確保するには `max_context_tokens` を smoke session token 範囲内まで縮小する必要がある**。

---

## 2. ゴール

1. **production smoke で level1 必発火** = 全 task で `[prev:` marker 1 件以上 emit、5 task × k=3 = 15 run で発火率 ≥ 80%
2. **項目 263/264 真効果の paired re-evaluation 可能化** = ratio tune が実 compaction 経路で意味を持つ前提を確保
3. **Lab v22+ paired metric の信頼性確保** = σ_Δ noise floor 確立 (A/A test, 別 plan) と組合せで真効果 ≥ noise floor 判定可能化
4. **backward compat 完全維持** = env-gated 推奨 (本 plan 推奨案 = `BONSAI_LAB_MAX_CTX=N` env override)、default 14000 不変

---

## 3. 案比較 (3 案 × 5 軸)

| 軸 | 案 A (default 縮小: 14000→8000) | 案 B (env override: BONSAI_LAB_MAX_CTX) | 案 C (smoke-only auto: BONSAI_LAB_SMOKE=1 で 6000) |
|----|----------------------------------|------------------------------------------|-----------------------------------------------------|
| prune 発火確実性 | ★★★ (全 path、production 含む) | ★★ (env=1 時のみ) | ★★★ (smoke 全 path) |
| backward compat | ★ (default 変更で既存 user 影響) | ★★★ (unset で 100% 既存挙動) | ★★★ (smoke 外で 100% 既存挙動) |
| 工数 | ★★★ (1 行 + test) | ★★ (env getter + factory chain、2 phase) | ★★ (smoke check + factory chain、2 phase) |
| 項目 263/264 再評価条件 | 即可 | env 設定で可 | smoke mode で可 |
| risk | 高 (production 退行可能性、p99 latency 増 / score 退行) | 低 (opt-in、rollback 1 行 unset) | 低 (smoke scope 限定、production 不影響) |
| **総合** | 6/15 | **12/15** | 13/15 |

### 推奨 = **案 C (smoke-only auto)**

理由:
- production 影響ゼロ (smoke 外で max_context_tokens=14000 維持)
- 項目 263/264 paired re-eval が `BONSAI_LAB_SMOKE=1` 既存 env で自動 enable
- A/A test (`lab_v22_aa_test.sh`) は既に `BONSAI_LAB_SMOKE=1` set 済 → 即時自動適用
- 副次: smoke wall 短縮効果 (compaction を実 path に流して memory pressure 下げる、若干 latency 改善期待)

ただし **案 B 併用** = `BONSAI_LAB_MAX_CTX=N` override も追加 (smoke 外の検証や Phase 6 plan kg 30% 等の N variation 評価のため)。

### 縮小値の根拠 (6000 推奨)

| value | level1 (75%) | level2 (90%) | level3 (100%) | 想定発火 |
|-------|---------------|---------------|----------------|----------|
| 8000 | 6000 | 7200 | 8000 | 中規模 T6 で間欠発火 |
| **6000** | **4500** | **5400** | **6000** | **5 task × k=3 全 run で確実** |
| 4000 | 3000 | 3600 | 4000 | level3 頻発、過剰圧縮 risk |

`emergency_keep=4` (Message 4 件以上は保持) と `prune_protect_tokens=4000` (直近 4000 tokens 保護) の関係も保ち、`max_context_tokens=6000` で `from_n_ctx_budget` clamp logic (`prune_protect_tokens = min(prune_protect, max_context/2) = 3000`) も整合。

---

## 4. ACCEPT 条件 (Phase 4 Smoke 後)

### 4.1 prune 発火確証
- (a) `[prev:` marker count ≥ 5 (5 task × k=3 = 15 run 中、80%+ 発火率)
- (b) `compaction.budget` emit が level1/2 と同 time window で対応
- (c) cargo test --lib 1372+ passed 退行ゼロ

### 4.2 ratio tune 真効果可視化 (副次 ACCEPT)
- (d) BONSAI_DYNAMIC_BUDGET=1 (env=1) vs unset で `[prev:` marker count 差分が観測される
  (env=1 で axis-priority prune が overflow 軸を優先選択 → tool message prune 順序が変動)
- (e) (d) が成立すれば項目 263 ratio tune effect は **measurable**、不成立なら axis-priority prune も実質 inactive (要 follow-up debug)

---

## 5. TDD strict 実装 outline (3 phase)

### Phase 1 Red (~30 min)
- `src/agent/compaction.rs::CompactionConfig` に新 field 追加なし (既存 `max_context_tokens` 流用)
- `src/agent/compaction.rs` に新関数 `with_smoke_or_env_override(self) -> Self` 追加 (stub `unimplemented!()`)
- 4 failing test (本 plan §6 参照):
  1. `t_smoke_override_reduces_max_context` (env BONSAI_LAB_SMOKE=1 で max_context=6000)
  2. `t_env_override_takes_precedence` (BONSAI_LAB_MAX_CTX=4000 で 4000 採用)
  3. `t_no_override_preserves_default` (両 env unset で 14000 維持)
  4. `t_prune_protect_clamped_after_override` (override 後 prune_protect ≤ max_context/2)

### Phase 2 Green (~1 h)
- `with_smoke_or_env_override` 本実装:
  ```rust
  pub fn with_smoke_or_env_override(mut self) -> Self {
      // env > smoke > default 優先順位
      if let Some(n) = lab_max_ctx_from_env() {
          self.max_context_tokens = n;
      } else if is_smoke_mode() {
          self.max_context_tokens = SMOKE_DEFAULT_MAX_CTX; // 6000
      }
      self.prune_protect_tokens = self.prune_protect_tokens.min(self.max_context_tokens / 2);
      self
  }
  ```
- env getter `lab_max_ctx_from_env() -> Option<usize>` (range 1..=14000、`BONSAI_LAB_MAX_CTX`)
- env getter `is_smoke_mode() -> bool` (`BONSAI_LAB_SMOKE` 既存 matcher 流用)
- const `SMOKE_DEFAULT_MAX_CTX: usize = 6000`
- `BudgetRatios::adjusted` は影響なし (allocate(N) で sum=N が動的に追従)

### Phase 3 Refactor + wiring (~30 min)
- rustdoc 強化 (env list、smoke 自動適用 contract、prune_protect clamp)
- `src/agent/middleware.rs` の `CompactionMiddleware::Default` / `with_n_ctx_budget` に chain 統合 (with_dynamic_budget_from_env と同 pattern):
  ```rust
  Self::new(
      CompactionConfig::default()
          .with_smoke_or_env_override()
          .with_dynamic_budget_from_env(),
  )
  ```
- env mutex (既存 `LAB_RUNTIME_ENV_TEST_LOCK` 流用) で cross-test serialize

---

## 6. test cases

### Unit (compaction.rs::tests)

```rust
#[test]
fn t_smoke_override_reduces_max_context() {
    let _g = LAB_RUNTIME_ENV_TEST_LOCK.lock();
    unsafe {
        std::env::set_var("BONSAI_LAB_SMOKE", "1");
        std::env::remove_var("BONSAI_LAB_MAX_CTX");
    }
    let config = CompactionConfig::default().with_smoke_or_env_override();
    assert_eq!(config.max_context_tokens, 6000);
    assert_eq!(config.prune_protect_tokens, 3000); // clamped to max/2
    unsafe {
        std::env::remove_var("BONSAI_LAB_SMOKE");
    }
}

#[test]
fn t_env_override_takes_precedence() {
    let _g = LAB_RUNTIME_ENV_TEST_LOCK.lock();
    unsafe {
        std::env::set_var("BONSAI_LAB_MAX_CTX", "4000");
        std::env::set_var("BONSAI_LAB_SMOKE", "1");
    }
    let config = CompactionConfig::default().with_smoke_or_env_override();
    assert_eq!(config.max_context_tokens, 4000);
    unsafe {
        std::env::remove_var("BONSAI_LAB_MAX_CTX");
        std::env::remove_var("BONSAI_LAB_SMOKE");
    }
}

#[test]
fn t_no_override_preserves_default() {
    let _g = LAB_RUNTIME_ENV_TEST_LOCK.lock();
    unsafe {
        std::env::remove_var("BONSAI_LAB_MAX_CTX");
        std::env::remove_var("BONSAI_LAB_SMOKE");
    }
    let config = CompactionConfig::default().with_smoke_or_env_override();
    assert_eq!(config.max_context_tokens, 14000); // unchanged
}

#[test]
fn t_prune_protect_clamped_after_override() {
    let _g = LAB_RUNTIME_ENV_TEST_LOCK.lock();
    unsafe {
        std::env::set_var("BONSAI_LAB_MAX_CTX", "5000");
    }
    let config = CompactionConfig::default().with_smoke_or_env_override();
    assert!(config.prune_protect_tokens <= config.max_context_tokens / 2);
    unsafe {
        std::env::remove_var("BONSAI_LAB_MAX_CTX");
    }
}
```

### Wiring (middleware.rs::tests)
- `t_compaction_middleware_default_applies_smoke_override` (env=BONSAI_LAB_SMOKE=1 で middleware.config.max_context=6000)

---

## 7. Phase 4 Smoke acceptance (G-MCT1/2/3)

### G-MCT1: baseline (env unset)
- 既存 G-DB-R-3 と同 env (BONSAI_LAB_SMOKE=1 + AUGMENT=1 + BUDGET=1)、本 plan 適用前 binary で実機 (前 session の baseline 0.7726 / [prev:=0)
- skip も可 (既 evidence 十分)

### G-MCT2: 本 plan 適用 (env unset、smoke 自動 enable)
- 同 env、本 plan 適用後 binary で実機
- ACCEPT 条件 §4.1 (a)(b)(c) 確認:
  - (a) `[prev:` marker ≥ 5 (15 run 中)
  - (b) compaction.budget emit と prune marker の time window 整合
  - (c) cargo test 退行ゼロ

### G-MCT3: env=1 vs env=unset 比較 (副次)
- BONSAI_DYNAMIC_BUDGET=1 と unset で paired (各 1 cycle、5 task × k=3 = 15 run × 2)
- ACCEPT 条件 §4.2 (d): `[prev:` marker count 差分が観測される (axis-priority prune の効果可視化)

wall: G-MCT2 ~60-80 min / G-MCT3 ~150 min (paired)、要 MLX server。

---

## 8. Rollback strategy

- `BONSAI_LAB_MAX_CTX` env unset + `BONSAI_LAB_SMOKE` unset で 100% 既存挙動 (max_context=14000 維持)
- 緊急 rollback: `with_smoke_or_env_override()` chain 削除 1 行 revert で完全 disable
- middleware.rs chain は env getter 経由なので、env unset で side effect ゼロ

---

## 9. dependencies + cross-references

- 前提 plan: `.claude/plan/dynamic-budget-phase6-kg-microtune.md` (§9 で本 plan 起票必要性を明示)
- 連動 plan: `.claude/plan/lab-v22-paired-metric-mandatory.md` (本 plan ACCEPT 後、A/A test → paired re-eval flow に統合)
- 関連項目: 261 (Phase 5 axis-priority prune wiring) / 263 (Phase 6 micro-tune の前提) / 264 (Option α REJECT の真因確証)
- production code change: `src/agent/compaction.rs` (default unchanged) + `src/agent/middleware.rs` (chain 統合)、退行ゼロ
- 既存 env mutex: `LAB_RUNTIME_ENV_TEST_LOCK` (compaction.rs:1133 等で既存)

---

## 10. follow-up (本 plan ACCEPT 後の次手)

1. ★★★ Phase 4 Smoke G-MCT2/3 実機検証 (要 MLX server、~3-4h)
2. ★★★ 項目 263 paired re-evaluation = G-DB-R-2/3 を BONSAI_LAB_SMOKE=1 (=自動 max_ctx=6000) で paired re-run、Lab v22 metric `--mode paired` で σ_Δ 比較
3. ★★ Phase 6 plan (kg 25→30%) ACCEPT/REJECT 判定 = (2) σ_Δ 確立後に paired 比較
4. ★★ 項目 264 D-2 真効果再評価 = AUGMENT × BUDGET interaction が prune 実行下でも destructive か検証 (要 paired)
5. ★ axis-priority prune の実効果 audit = G-MCT3 で env=1 vs unset で prune 順序差分の audit log 解析
