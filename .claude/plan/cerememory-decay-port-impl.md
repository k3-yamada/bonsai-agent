# Plan: Cerememory Power-law Decay Port — HeuristicStore.prune() 拡張 (opt-in)

> **由来**: 項目 213 (ERL Heuristics Pool、commit `41b6ac3`) の `HeuristicStore.prune()` は score 昇順で 200 件超過時に削除する**静的 prune**。Cerememory (`co-r-e/cerememory` MIT、commit b08d201、2026-05-08) の `cerememory-decay/src/math.rs` (8 KB、ADR-005) から 4 純関数を **そのまま port** し、prune に **decay-adjusted score 経路を opt-in 追加**する。
>
> **目的**: 「古い + 低使用頻度のみ自然削減」を opt-in で可能にし、HeuristicStore pool 健全性を時間軸で保つ選択肢を提供。Lab v17 (項目 214 進行中) の **結果と独立**に効果を持つ。production default は OFF (項目 214 toggle と対称) で既存挙動 100% 維持、`BONSAI_DECAY_ENABLED=1` で opt-in 適用。
>
> **前提**: Cerememory `decay/math.rs` MIT、4 関数すべて `#[inline]` の純関数 (副作用なし、stateless、rayon 並列安全)。bonsai 側は新規 `src/memory/decay.rs` 1 ファイル + `HeuristicStore` の prune/record_outcome 周辺最小変更で port 完了。

## Task Type
- [ ] Frontend
- [x] Backend (memory layer 新規 module + HeuristicStore prune 拡張、SCHEMA V11 1 列追加)
- [ ] Fullstack

## 1. 背景
### 1.1 現状の HeuristicStore prune (項目 213 commit `41b6ac3`)
- `prune(max_size: 200)` は単純な `score` 昇順 LIMIT 削除
- `last_used_at` は記録されているが prune では参照しない (= 時間軸無視)
- `used_count` は score 計算に組み込まれているが、過去 1 日 / 1 週間 / 1 ヶ月の利用頻度差を区別しない
- 結果として「数 cycle 前に高 score だが今は使われない助言」が残り続け、上位 K 注入で陳腐化したノイズになるリスク

### 1.2 Cerememory power-law decay (ADR-005)
- 4 公式すべて純関数、純粋数学的 (state なし、rayon `par_iter` 安全):
  ```text
  F(t) = F_0 * (1 + t/S)^(-d) * E_mod
  N(t) = N_0 + interference_rate * sqrt(t) * (1 - F(t))
  S_new = S_old * (1 + retrieval_boost * S_old^(-0.2))
  E_mod = 1.0 + emotion_intensity * 0.5
  ```
- defaults: `d=0.3`, `retrieval_boost=1.5`, `interference_rate=0.1`
- 全 `#[inline]` で hot path 性能影響軽微
- license MIT、author Masato Okuwaki @ CORe Inc.、bonsai に法的取り込み OK

### 1.3 Lab v17 進行中の独立性
- Lab v17 は `BONSAI_ERL_DISABLED=1` toggle で ERL 機構の ON/OFF を比較中 (~12-18h、項目 214)
- 本 plan は **prune 経路の追加**であり、ERL 機構の inject/extract には触れない
- production default OFF (`BONSAI_DECAY_ENABLED` env unset = 既存挙動 100% 維持) → Lab v17 ACCEPT/REJECT どちらでも独立に commit 可
- env name は項目 214 `BONSAI_ERL_DISABLED` (opt-out) と方向対称: `BONSAI_DECAY_ENABLED` (opt-in) — 本機能は新規追加で既存挙動と異なる経路のため opt-in が一貫

## 2. 目的
1. **Cerememory `decay/math.rs` 4 関数を `src/memory/decay.rs` に逐語 port** (~120 行、license header + attribution コメント)
2. **HeuristicStore に `stability: f64 DEFAULT 1.0` フィールド追加** (SCHEMA V11、1 列のみ)
3. **`record_outcome` で stability boost 適用** (`compute_stability_boost(s_old, retrieval_boost=1.5)`)
4. **`prune` に decay-adjusted score 経路を opt-in 追加** (env=`BONSAI_DECAY_ENABLED=1` で発動、それ以外は legacy)
5. **TDD strict 5 phase + production code 最小変更**: `src/memory/decay.rs` 新規 + `heuristics.rs` の prune/record_outcome 周辺のみ

## 3. 既存項目との関係
| 項目 | 関係 | 改修要否 |
|---|---|---|
| **213** ERL Phase 2 Green | HeuristicStore に field 1 個追加 (V11 migration)、prune に分岐追加 | 最小改修 |
| **214** Lab v17 toggle 機構 | env toggle pattern を踏襲 (opt-in 方向) | 設計踏襲 |
| **209** EventRepository trait | 影響なし、events に新 access なし | 参照のみ |
| **172** Tier::Core/Extended | benchmark 影響なし、pure memory layer 変更 | 参照のみ |
| **205** Option A 移行 | `&MemoryStore` 必須化済、本 plan は同設計に従う | 設計踏襲 |
| **80/83** Dreaming light/deep | 同様の decay 概念は dreams.rs で部分実装、本 plan は HeuristicStore に閉じる | 参照のみ |

## 4. 設計
### 4.1 新規 module `src/memory/decay.rs` (~120 行 MIT port)
```rust
//! Power-law fidelity decay model.
//!
//! Ported from cerememory-decay/src/math.rs (MIT, Masato Okuwaki @ CORe Inc.,
//! commit b08d201, 2026-05-08). See ADR-005 of the source repository.

/// `BONSAI_DECAY_ENABLED=1` (or `true`) で decay 経路 opt-in。
pub(crate) fn is_decay_enabled() -> bool {
    std::env::var("BONSAI_DECAY_ENABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// F(t) = F_0 * (1 + t/S)^(-d) * E_mod、clamp [0.0, 1.0]
#[inline]
pub(crate) fn compute_fidelity(
    f0: f64, t_secs: f64, stability: f64, decay_exponent: f64, emotion_mod: f64,
) -> f64 { /* port そのまま */ }

/// N(t) = N_0 + interference_rate * sqrt(t) * (1 - F(t))、clamp [0.0, 1.0]
#[inline]
pub(crate) fn compute_noise(/* ... */) -> f64 { /* port そのまま */ }

/// S_new = S_old * (1 + retrieval_boost * S_old^(-0.2))
#[inline]
pub(crate) fn compute_stability_boost(s_old: f64, retrieval_boost: f64) -> f64 { /* port */ }

/// E_mod = 1.0 + emotion_intensity * 0.5
#[inline]
pub(crate) fn compute_emotion_mod(emotion_intensity: f64) -> f64 { /* port */ }
```

### 4.2 SCHEMA V11 migration
```sql
-- src/db/schema.rs SCHEMA_V11
ALTER TABLE heuristics ADD COLUMN stability REAL NOT NULL DEFAULT 1.0;
```
V10 既存 field (12 列) は不変。`emotion_intensity` field は本 plan では追加しない (Cerememory case D「Emotional metadata」は別 plan)。`E_mod=1.0` 固定で port (decay 効果のみ)。

### 4.3 `HeuristicStore::record_outcome` 拡張
```rust
pub fn record_outcome(&self, id: i64, was_used_with_success: bool) -> Result<()> {
    // 既存: used_count += 1, success_after_use += (was_used_with_success as i64), last_used_at = now
    // 追加 (opt-in): stability_new = compute_stability_boost(stability_old, 1.5)
    if decay::is_decay_enabled() {
        // SELECT stability + UPDATE stability に伸長
    }
    // ... (既存 score 更新は不変)
}
```

### 4.4 `HeuristicStore::prune` 拡張 (opt-in 分岐、core)
```rust
pub fn prune(&self, max_size: usize) -> Result<usize> {
    if decay::is_decay_enabled() {
        self.prune_decay_adjusted(max_size, chrono::Utc::now().timestamp() as f64)
    } else {
        self.prune_legacy(max_size) // 既存 score 昇順、観測動作完全互換
    }
}

fn prune_legacy(&self, max_size: usize) -> Result<usize> { /* 既存 SQL を内部移動のみ */ }

fn prune_decay_adjusted(&self, max_size: usize, now_secs: f64) -> Result<usize> {
    // SELECT id, score, stability, last_used_at_secs FROM heuristics
    // For each: fidelity = compute_fidelity(score, max(0, now-last_used), stability, 0.3, 1.0)
    // sort ASC by fidelity → DELETE bottom (count - max_size)
}
```
- `prune_decay_adjusted` の `now_secs` は **fn 引数化** = test では決定論的 fixed 値で再現可能 (R3 軽減)

### 4.5 production default OFF (一貫した opt-in 方針)
- env unset = `is_decay_enabled() == false` → `prune` は legacy 経路 = **観測動作完全互換**
- `BONSAI_DECAY_ENABLED=1` で opt-in
- 項目 214 (`BONSAI_ERL_DISABLED` opt-out 方向) と env name は逆方向だが、**「production default は既存挙動」**という基本方針は一貫
- migration 直後の old `last_used_at` 行があっても、env unset なら decay 経路に入らないため R1 (大量削除リスク) は構造的に発生しない

### 4.6 attribution + license
- `src/memory/decay.rs` 冒頭に MIT attribution + Cerememory commit hash 明記
- `docs/THIRD_PARTY_LICENSES.md` (新規) で MIT 全文 + Copyright Masato Okuwaki / CORe Inc. 記載
- `Cargo.toml` 直接依存追加なし (source code を逐語 port するため、cargo dep ではない)

## 5. TDD strict 5 phase
### Phase 1 — Red (新規 ~12 test)
**`src/memory/decay.rs` 純関数 8 test**:
- `t_compute_fidelity_no_time_elapsed_returns_f0_times_emod`
- `t_compute_fidelity_decreases_over_time`
- `t_compute_fidelity_emotion_mod_amplifies`
- `t_compute_noise_increases_with_sqrt_t`
- `t_compute_stability_boost_increases_monotonically`
- `t_compute_emotion_mod_linear`
- `t_is_decay_enabled_default_false`
- `t_is_decay_enabled_explicit_true`

**`HeuristicStore` 統合 4 test** (env=enabled で動作確認):
- `t_record_outcome_boosts_stability_when_enabled`
- `t_prune_decay_adjusted_with_fixed_now_removes_old_low_score`
- `t_prune_legacy_when_disabled_observable_unchanged`
- `t_schema_v11_migration_adds_stability_default_1_0`

期待: compile error (新規 module / SCHEMA_V11 未定義 / `prune_decay_adjusted` 未定義) → Red 確認。

### Phase 2 — Green
1. `src/memory/decay.rs` 4 関数 + is_decay_enabled (~120 行 MIT port + attribution)
2. `src/db/schema.rs` SCHEMA_V11 = ALTER TABLE heuristics ADD COLUMN stability
3. `src/memory/heuristics.rs::record_outcome` で env-gate stability boost
4. `src/memory/heuristics.rs::prune` を 2 経路に分岐、`prune_legacy` / `prune_decay_adjusted(max_size, now)` 実装

期待: **1104 → 1116 passed (+12 / clippy 0 / fmt 0)**、env unset で既存全 test 退行ゼロ

### Phase 3 — Refactor
- `decay.rs` docstring に Cerememory 由来明記 + ADR-005 reference
- `prune_decay_adjusted` の SQL を prepared statement 化
- env mutation race を test-local Mutex で serialize (項目 214 と同パターン)

### Phase 4 — Smoke (G-4)
- `cargo test --release decay heuristics` で 12 新規 test green
- 既存 1104 test 退行ゼロ (env unset path で SQL 観測動作完全互換)
- `BONSAI_DECAY_ENABLED=1` で simulate clock fixed `now=1_000_000_000` の test fixture で「stability=1, score=0.5, last_used=1_000_000_000-86400 (1 日前) の row」が「stability=1, score=0.5, last_used=now の row」より低 fidelity で先に削除されること確認

### Phase 5 — Effectiveness (Lab v18 候補、別 plan)
- Lab v17 結果次第で実機検証要否判断
- ERL ACCEPT 時 = Lab v18 で `BONSAI_DECAY_ENABLED` paired t-test (decay ON/OFF) で pool 健全性影響を測定
- ERL REJECT 時 = HeuristicStore 自体が dead-code 候補化、本 plan の decay は他 store に転用
- 本 plan の delivery は P1-P4 までで完結

## 6. API 影響
| modulo path | 関数 / 構造体 | 種別 |
|---|---|---|
| `crate::memory::decay::is_decay_enabled` | pub(crate) fn | 新規 |
| `crate::memory::decay::compute_fidelity` | pub(crate) fn | 新規 |
| `crate::memory::decay::compute_noise` | pub(crate) fn | 新規 |
| `crate::memory::decay::compute_stability_boost` | pub(crate) fn | 新規 |
| `crate::memory::decay::compute_emotion_mod` | pub(crate) fn | 新規 |
| `HeuristicStore::record_outcome` | signature 不変、env-gate stability boost | 拡張 |
| `HeuristicStore::prune` | signature 不変、env-gate 2 経路分岐 | 拡張 |
| `heuristics.stability` SQLite column | REAL NOT NULL DEFAULT 1.0 | 新規 (V11) |

**API 完全 additive** (signature 変更ゼロ、後方互換 100%、env unset で観測動作完全互換)。

## 7. risks / mitigations
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| **R1** | V11 migration 後の旧 `last_used_at` 行が突然 decay 削除 | 構造的に発生しない (default OFF) | env unset が default なので migration 自体は stability=1.0 を埋めるだけ、prune 経路は不変 |
| **R2** | `compute_stability_boost` が `s_old=0` で `S^(-0.2) = inf` panic | record_outcome 時の異常 | `debug_assert!(s_old > 0.0)` + production で `s_old.max(0.001)` clamp |
| **R3** | `prune_decay_adjusted` の決定論性が時刻依存 | reproducibility 損失 | `now_secs: f64` を fn 引数化 + test で fixed 値 (4.4 設計済) |
| **R4** | MIT license 表記不備 | 法的問題 | `decay.rs` 冒頭 attribution + `docs/THIRD_PARTY_LICENSES.md` 全文転記 |
| **R5** | `compute_fidelity` の powf が hot path で性能劣化 | prune 1000 row で +5ms 想定 | `#[inline]` 維持、bench で計測 (G-5 任意) |
| **R6** | Lab v17 進行中の DB 状態破壊 | Lab v17 結果無効化 | 本 plan は Lab v17 完了後に着手必須 (G-1 着手前 checklist) |
| **R7** | `heuristics.stability` 値が NULL 返却 | runtime panic | `DEFAULT 1.0` で SQLite が migration 時に既存行を埋める、`f64` で受け取り可 |
| **R8** | Phase 5 effectiveness で decay ON が改悪 | Lab 退行 | env=disabled 維持で legacy が default、Phase 5 が ACCEPT 時のみ defaults 化検討 (項目 214 と同 pattern) |

## 8. quality gates
| Gate | 内容 | 検証 |
|---|---|---|
| **G-1 (Phase 1 Red)** | 12 test compile error or assertion fail | `cargo test --lib decay heuristics` |
| **G-2 (Phase 2 Green)** | 1104 → 1116 passed + clippy 0 + fmt 0、env unset で既存 test 退行ゼロ | `cargo test/clippy/fmt` |
| **G-3 (Phase 3 Refactor)** | docstring + prepared statement + test mutex | self-review |
| **G-4 (Phase 4 Smoke)** | env=enabled で fixed-clock fixture により decay 動作確認、env=unset で legacy 観測互換 | unit test simulate |
| **G-5 (license)** | `decay.rs` attribution + `docs/THIRD_PARTY_LICENSES.md` MIT 全文 | grep 確認 |
| **G-6 (Effectiveness、別 plan)** | Lab v18 paired t-test で decay ON/OFF 比較 | 別 plan |

## 9. 見積もり
| Phase | 内容 | 所要 |
|---|---|---|
| P1 (Red) | 12 test、cargo test Red 確認 | 0.5h |
| P2 (Green) | decay.rs port + V11 migration + record_outcome + prune 分岐 | 2h |
| P3 (Refactor) | docstring + SQL prepared statement + test mutex | 0.5h |
| P4 (Smoke) | env toggle 確認 + simulate clock で decay 動作 | 0.5h |
| P6 (commit + handoff) | 2-3 commits + CLAUDE.md 項目 215 候補 + MEMORY.md | 0.5h |
| **計** | | **~4h = 0.5 day** |

P5 effectiveness は別 plan (Lab v18 候補、~6h、本 plan の delivery 範囲外)。

## 10. 次の段階
### 着手判断
- ✅ Cerememory `decay/math.rs` MIT 確認済 (commit b08d201)
- ✅ 4 関数すべて純関数 = port 容易
- ✅ HeuristicStore (項目 213) production-ready (1104 passed)
- ⏳ Lab v17 進行中 (~12-18h)、完了後着手必須 (R6)

### 先送り条件
- ❌ Lab v17 完了前 (DB 状態破壊リスク)
- ❌ Cerememory 上流が breaking change を入れた場合 (ADR-005 改訂時は port 内容を re-sync)

## 11. ★ 着手前チェックリスト
1. [ ] Lab v17 完了確認 (`scripts/lab_v17_paired_ttest.py` で ACCEPT/REJECT 出力済)
2. [ ] Cerememory commit b08d201 の `decay/math.rs` を最終確認 (上流変更なし)
3. [ ] `cargo test --lib heuristics` で 1104 passed baseline
4. [ ] `docs/THIRD_PARTY_LICENSES.md` 既存有無確認

## 12. Quick Start
```bash
# 1. 着手前 verify
cargo test --lib heuristics --release 2>&1 | tail -5

# 2. Phase 1 Red
$EDITOR src/memory/decay.rs              # 4 純関数 + is_decay_enabled (todo!())
$EDITOR src/memory/heuristics.rs         # 4 統合 test 追加 (todo!() expectation)
$EDITOR src/db/schema.rs                 # SCHEMA_V11 const 定義
cargo test --lib --release decay heuristics 2>&1 | grep "test result"

# 3. Phase 2 Green
# decay.rs の todo!() を Cerememory `cerememory-decay/src/math.rs` から逐語 port + attribution
# heuristics.rs::record_outcome / prune に env-gate 分岐
cargo test --lib --release && cargo clippy --lib --tests -- -D warnings && cargo fmt --check

# 4. Phase 4 Smoke
cargo test --lib heuristics --release 2>&1 | grep "decay\|prune"
BONSAI_DECAY_ENABLED=1 cargo test --lib heuristics --release prune_decay 2>&1 | tail

# 5. license
$EDITOR docs/THIRD_PARTY_LICENSES.md
$EDITOR src/memory/decay.rs              # 冒頭 attribution

# 6. Commit
git add src/memory/decay.rs src/memory/heuristics.rs src/db/schema.rs docs/THIRD_PARTY_LICENSES.md
git commit -m "feat(memory): Cerememory power-law decay port (項目 215 候補)"
```

## 13. 参考
- [co-r-e/cerememory](https://github.com/co-r-e/cerememory) commit b08d201 (2026-05-08)
- `cerememory-decay/src/math.rs` (8 KB、4 純関数、ADR-005)
- 項目 213 ERL Phase 2 Green commit `41b6ac3` (前提実装)
- 項目 214 Lab v17 toggle 機構 commit `0013f31` (env toggle 設計踏襲)
- 項目 80/83 Dreaming light/deep (将来 dreams.rs にも decay 適用候補)

## 14. SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: 新規取得 (本 plan は Cerememory port 設計のため、項目 213/214 の既存 session 不適)
- GEMINI_SESSION: 任意

## 15. ★ 失敗時 (Phase 5 Effectiveness REJECT) handling
Lab v18 paired t-test で decay-on の Δscore が +0.015 未満:
1. **production default `BONSAI_DECAY_ENABLED` 未設定維持** (= legacy prune 既定化、本 plan の default と同じ、構造変更不要)
2. **decay.rs は他 store (Skill / Experience / Vault / KnowledgeGraph) で再評価** (汎用基盤として残置)
3. **CLAUDE.md** に negative finding 記録
4. 後続 plan で他 store decay 適用検討 (Skill の cooldown、Experience の forgetting curve 等)
