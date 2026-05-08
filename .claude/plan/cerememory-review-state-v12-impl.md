# Plan: Cerememory ReviewState Port — HeuristicStore に Freshness 軸を追加 (Strength から分離)

> **由来**: 項目 213 (ERL Heuristics Pool、commit `41b6ac3`) の HeuristicStore は `score / used_count / success_after_use` のみ = **Strength 軸 1 本**。Cerememory ADR-011 (`docs/adr/011-adaptive-review-and-freshness.md` 12 KB、最新最大 ADR、Status: Accepted、commit b08d201) は「**Strength** (durability) と **Freshness** (still-safe-truth) を分離」する設計判断を確立。bonsai に欠落しているこの 2 軸目を `ReviewState` 構造体として port する。
>
> **目的**: 「frequently used memory が dangerous な場合がある」(ADR-011) という洞察を bonsai の 1bit ハーネスに導入。バックエンド変更で陳腐化する助言 (例: 「MCP detach 必須」「`-c 12288` 推奨」「FallbackChain 設定」) を **freshness < threshold で injection skip** することで、heavy-used heuristic でも陳腐化を構造的に検知する。Lab v17 進行中 (項目 214) と独立、production default OFF (env opt-in) で観測動作完全互換。
>
> **前提**: Plan 1 (`cerememory-decay-port-impl.md`、SCHEMA V11 で `stability` 列追加) との**スキーマバージョン順序制約**あり (V11→V12)。両 plan は機構として独立 (相互依存ロジックなし)、ただしスキーマ migration は順序維持。`HeuristicStore` (項目 213) の既存 inject/extract path に最小変更。

## Task Type
- [ ] Frontend
- [x] Backend (memory layer 構造体追加 + SCHEMA V12 9 列 + review_tick API + inject_heuristics に freshness gate)
- [ ] Fullstack

## 1. 背景
### 1.1 ADR-011 の核心 (Cerememory 2026-05-08)
> "A frequently used memory can be important, but it can also be **dangerous** when it describes a changing fact: user preferences, project status, API behavior, credentials policy, deployment state, dependency versions, or external claims."

ADR-011 は **Strength (durability/decay)** と **Freshness (still-safe-truth)** を分離。SRS (Spaced Repetition System) の前提「再 recall = 再強化」は study tools には妥当だが、agent memory では「変化する事実」を再強化することが**逆に危険**になる。

### 1.2 bonsai HeuristicStore の Strength 寄り設計 (項目 213 commit `41b6ac3`)
| 既存 field (V10) | 軸 |
|---|---|
| `used_count` | Strength (利用回数) |
| `success_after_use` | Strength (有効性) |
| `score` | Strength (合成 utility) |
| `last_used_at` | Strength (recency proxy) |
| `created_at` | metadata |
| `category` (failure_recovery/efficiency/verification) | metadata |

**Freshness 軸ゼロ**。例えば「llama-server `-c 12288` 推奨」(Layer 1 仮説、項目 116) は high-used だが llama-server 設定変更で即陳腐化、しかし HeuristicStore は検知不能で 1bit モデルへ inject 続ける。

### 1.3 ADR-011 提案構造体 (Cerememory port 候補)
```rust
pub struct ReviewState {
    pub status: ReviewStatus,
    pub importance: f64,         // forget cost
    pub volatility: f64,         // 変化頻度予測
    pub freshness: f64,          // 今 valid な確度
    pub source_confidence: Option<f64>,
    pub last_reviewed_at: Option<DateTime<Utc>>,
    pub next_review_at: Option<DateTime<Utc>>,
    pub review_count: u32,
    pub stale_count: u32,
}

pub enum ReviewStatus {
    Unknown, Current, Due, Stale, Superseded, NeedsEvidence, Pinned,
}
```

全 numeric 0.0..=1.0 normalize、defaults `serde(default)` で V10→V12 後方互換 (ADR-011 §"Data Model" 明示要求)。

### 1.4 Lab v17 進行中の独立性
- Lab v17 は ERL inject/extract 機構の effectiveness 検証中 (~12-18h、項目 214)
- 本 plan は **inject 側の skip ロジック追加 + post-cycle review 機構**で、ERL の extract には触れない
- production default OFF (`BONSAI_REVIEW_ENABLED` env unset = freshness gate 無効化) → Lab v17 ACCEPT/REJECT どちらでも独立に commit 可
- env name は項目 214 / Plan 1 と方向対称 = opt-in (`BONSAI_REVIEW_ENABLED=1`)

## 2. 目的
1. **`ReviewState` 構造体を `src/memory/review.rs` 新規** (Cerememory ADR-011 から型 port、~150 行、attribution コメント)
2. **SCHEMA V12 で `heuristics` テーブルに 9 列追加** (review_status / importance / volatility / freshness / source_confidence / last_reviewed_at / next_review_at / review_count / stale_count)
3. **`HeuristicStore::review_tick(now: DateTime<Utc>) -> Vec<i64>`** = `next_review_at <= now` の row を返す scheduler API (Cerememory `lifecycle.review_tick` 相当)
4. **`HeuristicStore::record_review(id, outcome: ReviewOutcome) -> Result<()>`** = freshness 更新 + `review_count++` + 次回 `next_review_at` 計算 (Cerememory `lifecycle.record_review` 相当)
5. **`inject_heuristics` に freshness gate 追加**: env=enabled で `freshness < threshold (default 0.35)` の row は skip、それ以外は legacy 互換
6. **volatility 自動推定**: `category` から推定 (failure_recovery=0.7 / verification=0.5 / efficiency=0.3)、save 時 default 値投入
7. **next_review_at 計算式**: Cerememory `review.semantic.base_interval_secs=2592000 (30 day)` を起点、`base / (volatility * 4 + 1)` で volatility 高ほど短間隔
8. **production default OFF + TDD strict 5 phase + 最小変更**

## 3. 既存項目との関係
| 項目 | 関係 | 改修要否 |
|---|---|---|
| **213** ERL Phase 2 Green | HeuristicStore に 9 列追加 (V12 migration)、inject に gate 追加 | 拡張 |
| **214** Lab v17 toggle | env opt-in pattern 踏襲 | 設計踏襲 |
| **Plan 1** decay port | V11 `stability` 列、V11→V12 スキーマ順序制約のみ、機構独立 | 順序のみ |
| **209** EventRepository trait | 影響なし | 参照のみ |
| **80/83** Dreaming | dream_tick で review_tick を呼出する案あり (将来) | 参照のみ |
| **82** ContextOverflowGuard | freshness gate で inject 削減 → context 圧迫軽減副次効果 | 副次相補 |

## 4. 設計
### 4.1 新規 module `src/memory/review.rs` (~150 行)
```rust
//! Adaptive review and freshness scheduling.
//!
//! Ported design from cerememory ADR-011 (Cerememory, MIT, commit b08d201,
//! 2026-05-08). Strength (decay) と Freshness (truth-maintenance) を分離する
//! 設計判断を bonsai HeuristicStore に適用。

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewStatus {
    Unknown, Current, Due, Stale, Superseded, NeedsEvidence, Pinned,
}

impl ReviewStatus {
    pub fn as_db_str(&self) -> &'static str { /* "unknown" / "current" / ... */ }
    /// ADR-011 §"Data Model" 「Defaults must be backward-compatible」要件で
    /// 未知文字列は Unknown に復元 (typo / V10 legacy 互換)。
    pub fn from_db_str(s: &str) -> Self { /* match -> Unknown */ }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewState {
    pub status: ReviewStatus,
    pub importance: f64,
    pub volatility: f64,
    pub freshness: f64,
    pub source_confidence: Option<f64>,
    pub last_reviewed_at: Option<DateTime<Utc>>,
    pub next_review_at: Option<DateTime<Utc>>,
    pub review_count: u32,
    pub stale_count: u32,
}

impl Default for ReviewState {
    fn default() -> Self {
        Self {
            status: ReviewStatus::Unknown,
            importance: 0.5,
            volatility: 0.5,
            freshness: 1.0,
            source_confidence: None,
            last_reviewed_at: None,
            next_review_at: None,
            review_count: 0,
            stale_count: 0,
        }
    }
}

/// volatility 推定 (HeuristicStore.save 時に default 値として投入)。
/// category 4 値の MVP マッピング、将来 MetaMemory plane で精緻化 (R1)。
pub(crate) fn estimate_volatility_from_category(category: &str) -> f64 {
    match category {
        "failure_recovery" => 0.7,
        "verification" => 0.5,
        "efficiency" => 0.3,
        _ => 0.5,
    }
}

/// 次回 review 日時 (volatility 高ほど短間隔)。base=30 day をスケール。
pub(crate) fn compute_next_review_at(
    now: DateTime<Utc>, volatility: f64, base_secs: i64,
) -> DateTime<Utc> {
    let scale = (volatility * 4.0 + 1.0).max(1.0);
    now + Duration::seconds(((base_secs as f64) / scale) as i64)
}

/// freshness gate: env=enabled かつ freshness < threshold で skip。
pub(crate) fn should_skip_for_freshness(state: &ReviewState, threshold: f64) -> bool {
    is_review_enabled() && state.freshness < threshold
}

pub(crate) fn is_review_enabled() -> bool {
    std::env::var("BONSAI_REVIEW_ENABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy)]
pub enum ReviewOutcome {
    Confirmed,        // freshness ← 1.0、status=Current、stale_count reset
    StillCurrent,     // freshness ← min(1.0, freshness+0.2)、status=Current
    Stale,            // freshness ← max(0.0, freshness-0.3)、status=Stale、stale_count++
    Superseded,       // freshness ← 0.0、status=Superseded
    NeedsEvidence,    // freshness 不変、status=NeedsEvidence
}

impl ReviewOutcome {
    pub(crate) fn apply_to(&self, s: &mut ReviewState) { /* match self */ }
}
```

### 4.2 SCHEMA V12 migration
```sql
-- src/db/schema.rs SCHEMA_V12
ALTER TABLE heuristics ADD COLUMN review_status TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE heuristics ADD COLUMN importance REAL NOT NULL DEFAULT 0.5;
ALTER TABLE heuristics ADD COLUMN volatility REAL NOT NULL DEFAULT 0.5;
ALTER TABLE heuristics ADD COLUMN freshness REAL NOT NULL DEFAULT 1.0;
ALTER TABLE heuristics ADD COLUMN source_confidence REAL NULL;
ALTER TABLE heuristics ADD COLUMN last_reviewed_at TEXT NULL;
ALTER TABLE heuristics ADD COLUMN next_review_at TEXT NULL;
ALTER TABLE heuristics ADD COLUMN review_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE heuristics ADD COLUMN stale_count INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_heuristics_next_review ON heuristics(next_review_at);
```

V11 (Plan 1) 適用後に V12 migration が走る順序制約あり (R3)。

### 4.3 `HeuristicStore::save` 拡張
- 既存 caller の API 不変
- 内部で `volatility = estimate_volatility_from_category(category)` 推定、`next_review_at = compute_next_review_at(now, volatility, base=2_592_000 (30 day))` 計算
- 新規 row は `freshness=1.0 / status='unknown' / review_count=0`

### 4.4 `HeuristicStore::review_tick(now)` 新規 (~30 行)
```rust
pub fn review_tick(&self, now: DateTime<Utc>) -> Result<Vec<i64>> {
    if !review::is_review_enabled() {
        return Ok(Vec::new()); // env unset = scheduler 無効
    }
    let now_str = now.to_rfc3339();
    let ids: Vec<i64> = /* SELECT id FROM heuristics WHERE next_review_at IS NOT NULL AND next_review_at <= ?1 ORDER BY next_review_at ASC LIMIT 50 */;
    // status を 'due' に更新
    Ok(ids)
}
```

### 4.5 `HeuristicStore::record_review(id, outcome)` 新規 (~40 行)
```rust
pub fn record_review(&self, id: i64, outcome: ReviewOutcome, now: DateTime<Utc>) -> Result<()> {
    let mut state: ReviewState = /* SELECT review_state fields FROM heuristics WHERE id */;
    outcome.apply_to(&mut state);
    state.last_reviewed_at = Some(now);
    state.next_review_at = Some(compute_next_review_at(now, state.volatility, 2_592_000));
    /* UPDATE heuristics SET review_status, freshness, review_count, stale_count, last_reviewed_at, next_review_at */
    Ok(())
}
```

### 4.6 `inject_heuristics` に freshness gate (core)
```rust
// src/agent/context_inject.rs::inject_heuristics
pub(crate) fn inject_heuristics(/* ... */) -> Vec<i64> {
    if heuristics::is_erl_disabled() {
        return Vec::new(); // 項目 214 既存 short-circuit
    }
    let candidates = store.find_top_k_for_task(...)?;
    let filtered: Vec<_> = candidates.into_iter().filter(|h| {
        !review::should_skip_for_freshness(&h.review_state, 0.35)
    }).collect();
    // filtered を inject (top-K=5 unchanged)
}
```
- env=disabled で `should_skip_for_freshness` は **常に false 返却** = legacy 動作完全互換
- env=enabled かつ freshness < 0.35 で skip
- threshold は将来 `AdvisorConfig` field 化候補 (本 plan では const)

### 4.7 production default OFF (一貫した opt-in)
- env unset → `is_review_enabled() == false` → `review_tick` は空 Vec、`should_skip_for_freshness` は false
- すべての SQL は migrate 後も既存 caller から不可視 (新 9 列は SELECT に含まれず query 観測不変)
- **観測動作完全互換**

### 4.8 attribution
- ADR-011 は code port ではなく**設計 port** (構造体定義 + 公式 + 命名)、license 上は MIT 攻めの port よりも軽い
- `src/memory/review.rs` 冒頭に「Ported design from cerememory ADR-011 (Cerememory, MIT, commit b08d201, 2026-05-08)」コメント
- `docs/THIRD_PARTY_LICENSES.md` に Cerememory 全文を Plan 1 と共通で記載 (Plan 1 が先行で作成、本 plan は追記のみ)

## 5. TDD strict 5 phase
### Phase 1 — Red (新規 ~14 test)
**`src/memory/review.rs` 純関数 + 構造体 8 test**:
- `t_review_state_default_values`
- `t_review_status_db_str_roundtrip` (V10 legacy 文字列 → Unknown 復元含む)
- `t_estimate_volatility_failure_recovery_high`
- `t_estimate_volatility_efficiency_low`
- `t_compute_next_review_at_volatility_scaling`
- `t_review_outcome_confirmed_resets_freshness`
- `t_review_outcome_stale_decreases_freshness`
- `t_should_skip_for_freshness_gated_by_env`

**`HeuristicStore` 統合 4 test** (env=enabled で動作確認):
- `t_save_initializes_review_state_with_volatility_from_category`
- `t_review_tick_returns_due_ids_only`
- `t_record_review_confirmed_updates_freshness_and_next_review_at`
- `t_schema_v12_migration_adds_9_columns`

**`inject_heuristics` 統合 2 test**:
- `t_inject_skips_low_freshness_when_enabled`
- `t_inject_legacy_observable_unchanged_when_disabled`

期待: compile error (新規 module / SCHEMA_V12 / review_tick / record_review 未定義) → Red 確認。

### Phase 2 — Green
1. `src/memory/review.rs` 構造体 + 5 純関数 + ReviewOutcome (~150 行)
2. `src/db/schema.rs` SCHEMA_V12 = 9 ALTER + 1 INDEX
3. `src/memory/heuristics.rs::save` で volatility/next_review_at 投入
4. `src/memory/heuristics.rs::review_tick` / `record_review` 新規 method
5. `src/agent/context_inject.rs::inject_heuristics` に env-gate filter

期待: **1104 → 1118 passed (+14 / clippy 0 / fmt 0)**、env unset で既存全 test 退行ゼロ

### Phase 3 — Refactor
- `review.rs` docstring に ADR-011 reference + Strength/Freshness 分離 rationale
- `record_review` の SQL を prepared statement 化
- env mutation race を test-local Mutex で serialize (項目 214 と同パターン)

### Phase 4 — Smoke (G-4)
- `cargo test --release review heuristics inject_heuristics` で 14 新規 test green
- 既存 1104 test 退行ゼロ
- env=enabled で fixed-clock fixture: `freshness=0.30 (< threshold 0.35)` の row が `inject_heuristics` 結果に含まれない確認
- env=enabled で `review_tick(now)` が `next_review_at < now` の row のみ返却確認
- `record_review(Stale)` で freshness が 0.3 減少 + stale_count+1 確認

### Phase 5 — Effectiveness (Lab v19 候補、別 plan)
- Lab v17 結果次第 + Plan 1 (decay) Phase 5 結果次第で実機検証要否判断
- Lab v19 paired t-test (`BONSAI_REVIEW_ENABLED` ON/OFF) で freshness gate の Lab effectiveness 測定
- ACCEPT 基準: Δscore ≥ +0.015 AND p < 0.1
- REJECT 時 = freshness gate を default 維持で env-only feature として残置

## 6. API 影響
| modulo path | 関数 / 構造体 | 種別 |
|---|---|---|
| `crate::memory::review::ReviewState` | pub struct + Default + 9 fields | 新規 |
| `crate::memory::review::ReviewStatus` | pub enum 7 variants | 新規 |
| `crate::memory::review::ReviewOutcome` | pub enum 5 variants | 新規 |
| `crate::memory::review::is_review_enabled` | pub(crate) fn | 新規 |
| `crate::memory::review::estimate_volatility_from_category` | pub(crate) fn | 新規 |
| `crate::memory::review::compute_next_review_at` | pub(crate) fn | 新規 |
| `crate::memory::review::should_skip_for_freshness` | pub(crate) fn | 新規 |
| `HeuristicStore::review_tick(now)` | pub fn -> Vec<i64> | 新規 |
| `HeuristicStore::record_review(id, outcome, now)` | pub fn -> Result<()> | 新規 |
| `HeuristicStore::save` | signature 不変、内部で volatility/next_review_at 投入 | 拡張 |
| `inject_heuristics` | signature 不変、env-gate filter | 拡張 |
| `heuristics` SQLite columns (V12) | 9 列追加 + 1 index | 新規 |

**API 完全 additive** (signature 変更ゼロ、後方互換 100%、env unset で観測動作完全互換)。

## 7. risks / mitigations
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| **R1** | volatility 推定が category 4 値で粗い | freshness gate の精度低下 | MVP 受容、将来 MetaMemory plane 拡張 plan で intent/evidence 経由の精緻化 |
| **R2** | freshness が時間経過で自動減衰しない (record_review 経由のみ更新) | 「review されない助言は永遠に freshness=1.0」 | Plan 1 (decay) と組合せ、または別 plan で `freshness *= power_law(t)` 自動減衰を後段追加 |
| **R3** | V11→V12 連続 migration 順序 | スキーマ順序違反 | Plan 1 を先行 commit して V11 確定後に本 plan の V12、`schema.rs` の version 連番遵守 |
| **R4** | `next_review_at` SQLite NULL 比較で SQL bug | review_tick が誤 row 返却 | `WHERE next_review_at IS NOT NULL AND next_review_at <= ?1` 両条件明示、test fixture で検証 |
| **R5** | `compute_next_review_at` の `volatility * 4 + 1` scale が arbitrary | 将来 tune 必要 | const 化 + Plan 1 と同様 Phase 5 effectiveness で再評価、必要なら `AdvisorConfig` field 化 |
| **R6** | Lab v17 進行中の DB 状態破壊 | Lab v17 結果無効化 | 本 plan は Lab v17 完了後に着手必須 (G-1 着手前 checklist) |
| **R7** | `review_status` enum SQLite TEXT で V10 legacy 文字列 / typo | runtime parse 失敗 | `from_db_str` で未知文字列 → Unknown 復元 (ADR-011 §"Data Model" 要件)、roundtrip test で確認 |
| **R8** | freshness threshold=0.35 が arbitrary | inject 過剰 / 過少 skip | const start + Phase 5 で sweep (0.25/0.35/0.45)、Lab paired t-test |

## 8. quality gates
| Gate | 内容 | 検証 |
|---|---|---|
| **G-1 (Phase 1 Red)** | 14 test compile error or assertion fail | `cargo test --lib review heuristics inject_heuristics` |
| **G-2 (Phase 2 Green)** | 1104 → 1118 passed + clippy 0 + fmt 0、env unset で既存退行ゼロ | `cargo test/clippy/fmt` |
| **G-3 (Phase 3 Refactor)** | docstring + prepared statement + test mutex | self-review |
| **G-4 (Phase 4 Smoke)** | env=enabled で fixed-clock fixture で freshness gate / review_tick / record_review 動作 | unit test simulate |
| **G-5 (license)** | `review.rs` ADR-011 attribution + `docs/THIRD_PARTY_LICENSES.md` Cerememory 記載 | grep 確認 |
| **G-6 (Effectiveness、別 plan)** | Lab v19 paired t-test で freshness gate ON/OFF 比較 | 別 plan |

## 9. 見積もり
| Phase | 内容 | 所要 |
|---|---|---|
| P1 (Red) | 14 test、cargo test Red 確認 | 1h |
| P2 (Green) | review.rs port + V12 migration + save 拡張 + 2 新 method + inject filter | 5h |
| P3 (Refactor) | docstring + SQL prepared + test mutex | 1h |
| P4 (Smoke) | env toggle 確認 + simulate clock で 3 path 動作 | 1h |
| P6 (commit + handoff) | 3-4 commits + CLAUDE.md 項目 216 候補 + MEMORY.md | 1h |
| **計** | | **~9h ≈ 1.5 day** |

P5 effectiveness は別 plan (Lab v19 候補、~6h、本 plan の delivery 範囲外)。

## 10. 次の段階
### 着手判断
- ✅ Cerememory ADR-011 確認済 (commit b08d201、Status: Accepted)
- ✅ HeuristicStore (項目 213) production-ready (1104 passed)
- ⏳ Lab v17 進行中 (~12-18h)、完了後着手必須 (R6)
- ⏳ Plan 1 (decay) との順序: V11 → V12 順、Plan 1 先行 commit 推奨

### 先送り条件
- ❌ Lab v17 完了前 (DB 状態破壊リスク)
- ❌ Plan 1 未着手で V11 不在 (V12 migration 失敗)
- ❌ Cerememory 上流 ADR-011 が breaking change を入れた場合 (re-sync)

## 11. ★ 着手前チェックリスト
1. [ ] Lab v17 完了確認
2. [ ] Plan 1 (decay) commit 済 (V11 適用済)
3. [ ] Cerememory ADR-011 (commit b08d201) を最終確認 (上流変更なし)
4. [ ] `cargo test --lib heuristics` で 1116 passed baseline (Plan 1 後)
5. [ ] `docs/THIRD_PARTY_LICENSES.md` 既存有無 (Plan 1 で先行作成想定)

## 12. Quick Start
```bash
# 1. 着手前 verify
cargo test --lib heuristics --release 2>&1 | tail -5
# Plan 1 適用済 = 1116 passed 期待

# 2. Phase 1 Red
$EDITOR src/memory/review.rs             # ReviewState + ReviewStatus + ReviewOutcome (todo!())
$EDITOR src/memory/heuristics.rs         # review_tick / record_review test (todo!())
$EDITOR src/agent/context_inject.rs      # freshness gate test
$EDITOR src/db/schema.rs                 # SCHEMA_V12 const 定義
cargo test --lib --release review heuristics inject_heuristics 2>&1 | grep "test result"

# 3. Phase 2 Green
# review.rs を ADR-011 から構造体 + 5 純関数で実装 + attribution
# heuristics.rs::save / review_tick / record_review 新規
# context_inject.rs::inject_heuristics に env-gate filter
cargo test --lib --release && cargo clippy --lib --tests -- -D warnings && cargo fmt --check

# 4. Phase 4 Smoke
cargo test --lib heuristics --release 2>&1 | grep "review\|freshness"
BONSAI_REVIEW_ENABLED=1 cargo test --lib --release inject_skips_low_freshness 2>&1 | tail

# 5. license
$EDITOR docs/THIRD_PARTY_LICENSES.md     # Cerememory 全文 (Plan 1 で先行作成想定なら追記のみ)
$EDITOR src/memory/review.rs             # 冒頭 attribution

# 6. Commit
git add src/memory/review.rs src/memory/heuristics.rs src/agent/context_inject.rs src/db/schema.rs docs/THIRD_PARTY_LICENSES.md
git commit -m "feat(memory): Cerememory ADR-011 ReviewState port (項目 216 候補)"
```

## 13. 参考
- [co-r-e/cerememory](https://github.com/co-r-e/cerememory) commit b08d201 (2026-05-08)
- `docs/adr/011-adaptive-review-and-freshness.md` (12 KB、最新最大 ADR、Status: Accepted)
- 項目 213 ERL Phase 2 Green commit `41b6ac3` (前提実装)
- 項目 214 Lab v17 toggle 機構 commit `0013f31` (env opt-in 設計踏襲)
- Plan 1 (`cerememory-decay-port-impl.md`) — V11 stability 列、本 plan は V12 で連続
- 項目 80/83 Dreaming light/deep (将来 dream_tick で review_tick 呼出案、本 plan 範囲外)
- 項目 82 ContextOverflowGuard (freshness skip で inject 削減 → context 圧迫軽減副次効果)

## 14. SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: 新規取得 (本 plan は ADR-011 設計 port、Plan 1 と独立)
- GEMINI_SESSION: 任意

## 15. ★ 失敗時 (Phase 5 Effectiveness REJECT) handling
Lab v19 paired t-test で freshness gate-on の Δscore が +0.015 未満:
1. **production default `BONSAI_REVIEW_ENABLED` 未設定維持** (= legacy inject 既定化、本 plan の default と同じ、構造変更不要)
2. **review.rs / SCHEMA V12 は他 store (Skill / Experience / Vault) で再評価** (汎用基盤として残置)
3. **CLAUDE.md** に negative finding 記録
4. 後続 plan で freshness threshold sweep (0.25/0.35/0.45) や volatility 推定精緻化 (MetaMemory plane 拡張) 検討
