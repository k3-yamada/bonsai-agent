# Plan: Self-Verification Dilemma — Advisor 検証 step の経験ベース動的 skip

> **由来**: arxiv 2602.03485 (Self-Verification Dilemma, 2026-02) の知見を bonsai-agent Advisor 検証 step に適用、**項目 17/18** (完了前自己検証 / max_uses=3 静的制限) を **経験統計 (EventStore + AuditLog) ベースの動的 skip 判断** に拡張する。
>
> **目的**: Lab v8〜v15 で観測された Advisor 検証 step の **過剰発動による天井 5 連続** をデータ駆動で削減、score 維持しつつ Advisor 由来 token / latency オーバーヘッドを縮小。

## Task Type
- [ ] Frontend
- [x] Backend (`AdvisorConfig` 拡張、`agent_loop/advisor_inject.rs` skip 判定、`EventStore` クエリ追加)
- [ ] Fullstack

## 1. 背景
### 1.1 arxiv 2602.03485 Self-Verification Dilemma 要点
- 観察: LLM agent は self-verification を「呼べば呼ぶほど良い」前提で過剰使用。検証自体が hallucinate / 単純タスクで token 浪費 / 検証成功率低い文脈では呼ばない方が score 高い
- 提案: 過去の検証経験を task type 別に集計、ROI 低い文脈で skip
- 主張: Reflexion 系 agent で +6〜12% 成功率改善 (報告)

### 1.2 bonsai 既存の限界 (項目 17/18)
- 項目 17: `inject_verification_step` が iteration > 0 + 複雑タスク + `[検証済]` 未含有で常に発火
- 項目 18: `AdvisorConfig::max_uses=3` の絶対上限、role 横断共有
- 項目 89: `verification_prompt` / `replan_prompt` 統一文字列、task type 別最適化なし

### 1.3 Lab 観測根拠
- v8/v9/v10/v14/v15 = 5 連続天井: プロンプト変異 ACCEPT 率 ≤ 11%、Advisor 検証 step が score を上げないケース累積
- v9 ACCEPT 1 件 (項目 136 +0.0157) は事前確認、検証 step 自体は退行傾向

## 2. 目的
1. 動的 skip 機構: 過去検証 ROI が閾値未満の task pattern で skip
2. fallback 安全性: cold-start で max_uses=3 fallback (退行ゼロ保証)
3. 観測可能性: skip 判断を AuditLog + TSV `verify_skip_rate` 列で残す
4. 天井打破: Advisor 過剰発動抑止、Lab v15+ で score +0.005〜+0.015 期待

## 3. 既存項目との関係
- 項目 17: `inject_verification_step` 冒頭に skip 判定 hook、ヒット時 false 返却
- 項目 18: 保持 (fallback 上限)
- 項目 89: AdvisorConfig に `dynamic_skip_threshold: f64` / `min_samples_for_skip: usize` 追加 (default OFF で後方互換)
- 項目 162: `EventStore::verification_success_rate(task_type)` 集計クエリ追加
- 項目 200: skip rate を informational metric 化、ACCEPT 判定不変
- 項目 201/202/203/205: events stream を skip 判定でも使用

## 4. 設計
### 4.1 アーキテクチャ
```
inject_verification_step(...) {
    // [新規] 経験ベース skip 判定 (default OFF)
    if advisor.dynamic_skip_threshold > 0.0
        && let Some(store) = store
        && let Some(rate) = EventStore::new(store.conn())
                              .verification_success_rate(task_type, advisor.min_samples_for_skip)?
        && rate < advisor.dynamic_skip_threshold
    {
        log_event(Info, "advisor.skip", ...);
        emit AuditAction::AdvisorSkip { reason, rate, threshold };
        return false;
    }
    // 既存 gate (iteration / can_advise / complexity / [検証済] / iter上限)
    ...
}
```

### 4.2 task_type 分類 (cold-start risk 緩和)
- 4 カテゴリ: `code_edit` / `code_read` / `shell_exec` / `other`
- 分類関数: `task_context` (タスク文 + tool 履歴 hash) を regex / tool prefix 集計で deterministic 決定
- benchmark.rs の Tier (Core/Extended) とは独立軸

### 4.3 verification_success_rate 集計
```rust
pub fn verification_success_rate(&self, task_type: &str, min_samples: usize) -> Result<Option<f64>>
```
- 入力: task_type, min_samples (default 5)
- 出力: Some(0.0..=1.0) if sample ≥ min_samples、None if 不足 (cold-start)
- 「成功」定義: verification call 後同セッション内で FinalAnswer に [検証済] 含有 + ToolCallEnd error_count = 0
- SQLite index `idx_audit_advisor_role` 追加 (migration V10)

### 4.4 後方互換性
- default = 0.0 で既存挙動 100% 維持
- TOML `[advisor] dynamic_skip_threshold = 0.4` で opt-in
- 既存 1058 passed は default 0.0 で無変更

### 4.5 メトリクス
- TSV 12 列 → 13 列: `verify_skip_rate` 追加 (cycle 内 skipped/would-have-fired 比)
- ACCEPT 判定には使わない (informational のみ)

## 5. TDD strict 5 phase
### Phase 1 — Red
新規 test (`src/agent/agent_loop/tests/verify_skip_tests.rs`):
1. `test_skip_when_rate_below_threshold` — store に過去 verification call 5 件 全 fail seed、threshold=0.4 で false 返却
2. `test_no_skip_when_samples_insufficient` — sample 3 件 → 既存挙動
3. `test_no_skip_when_threshold_zero` — default 0.0 = 後方互換
4. `test_skip_emits_audit_log` — skip 発火時 AuditAction::AdvisorSkip 記録
5. `test_task_type_classification_*` — 4 ケース deterministic 分類
6. `test_verification_success_rate_empty_returns_none`
7. `test_verification_success_rate_with_samples_returns_ratio`

`cargo test` 全 7 fail 確認、commit `test(self-verify): Phase 1 Red`

### Phase 2 — Green
- `model_router.rs::AdvisorConfig` 2 フィールド追加 + Default 更新
- `agent_loop/advisor_inject.rs::inject_verification_step` 冒頭 skip hook
- `classify_task_type` private fn
- `event_store.rs::verification_success_rate` 実装 (SQL + index)
- `audit.rs::AuditAction::AdvisorSkip` variant 追加
- `db/migrate.rs` V10 (`idx_audit_advisor_role` index)

`cargo test` 1058 + 7 = 1065 PASS、commit `feat(self-verify): Phase 2 Green`

### Phase 3 — Refactor
- 早期 return ladder 整理 (collapsible_if 警告対応)
- regex を `static once_cell` 化
- docstring 追加 (Self-Verification Dilemma 引用 + 項目番号 cross-ref)
- `cargo fmt` + clippy clean、commit `refactor(self-verify): Phase 3`

### Phase 4 — Smoke (実機 1 cycle)
- `BONSAI_BENCH_TIER=smoke` + `dynamic_skip_threshold=0.4` を config.toml で有効化
- 7 task k=3 (~15 min)
- 観測: skip rate ∈ [30%, 70%] / score Δ ∈ [-0.02, +∞) / AdvisorSkip log emit / verify_skip_rate TSV 追加

### Phase 5 — Contract (Lab variant 化、別 session)
- Lab v15 variant pool に threshold ∈ {0.3, 0.4, 0.5} 追加
- core 22 baseline (0.0) vs variant の delta で ACCEPT 判定 (+0.005 以上で defaults 昇格候補)

## 6. API 影響
### 6.1 公開 API (新規)
```rust
pub struct AdvisorConfig {
    pub dynamic_skip_threshold: f64,
    pub min_samples_for_skip: usize,
}

impl<'a> EventStore<'a> {
    pub fn verification_success_rate(&self, task_type: &str, min_samples: usize) -> Result<Option<f64>>;
}

pub enum AuditAction {
    AdvisorSkip { reason: String, rate: f64, threshold: f64 },
}
```

### 6.2 内部 API (新規 private)
- `classify_task_type(task_context: &str) -> &'static str`
- `should_skip_verification(advisor, store, task_context) -> Option<(f64, f64)>`

### 6.3 公開 API 破壊的変更
なし (`AdvisorConfig` 新規フィールドは Default fallback、既存 caller 無変更)

## 7. Risks (5+ 項目)
| # | risk | severity | mitigation |
|---|------|----------|------------|
| R1 | skip 過剰で品質劣化 | HIGH | task_type 分類で `code_edit` 保護、threshold default 0.0 + opt-in、Lab smoke で score Δ ∈ [-0.02, +∞) gate |
| R2 | cold-start sample 不足で誤判定 | MEDIUM | min_samples_for_skip=5、不足時 None で既存挙動 fallback |
| R3 | task_type 4 カテゴリ境界曖昧 | MEDIUM | Phase 1 で deterministic test 4 件、`other` バケット独立統計 |
| R4 | 既存テスト破壊 (return ladder 順序) | MEDIUM | 既存 9 個 advisor テスト全 PASS を Phase 2 通過 gate |
| R5 | 1bit variance で skip 判断ノイズ高い | MEDIUM | min_samples ≥ 5、threshold 0.4 推奨 (0.5 振動帯回避) |
| R6 | AuditAction::AdvisorSkip schema 変更 | LOW | enum で前方互換 |
| R7 | migration V10 index 失敗 | LOW | CREATE INDEX IF NOT EXISTS、warn ログ + 機構 OFF 継続 |
| R8 | LLM が `[検証済]` を意図せず出力 | LOW | success 定義は [検証済] + error=0 の AND |

## 8. Gates
- G-1 Compile: `cargo build` clean
- G-2 Test: 1058 + 7 = 1065 PASS、clippy 0、fmt clean
- G-3 Smoke: skip rate ∈ [30%, 70%]
- G-4 退行ゼロ: smoke score Δ ∈ [-0.02, +∞) vs threshold=0.0 baseline
- G-5 過剰発動抑制: Lab cycle 内 verification 発火回数が baseline 比 ≥ 30% 削減
- G-6 Audit emit: AdvisorSkip log と skip 発火が 1:1 対応

G-4 NG なら Phase 5 Lab variant は実施せず棚保留

## 9. 見積もり
| Phase | 内容 | 時間 |
|-------|------|------|
| Phase 0 | review + 詳細読込 | 0.3h |
| Phase 1 | Red — test 7 件 | 0.7h |
| Phase 2 | Green — 実装 | 1.5h |
| Phase 3 | Refactor | 0.3h |
| Phase 4 | Smoke 1 cycle | 1.0h |
| 文書化 | CLAUDE.md + handoff | 0.2h |
| **合計** | | **~4.0h (≈ 0.5 day)** |

Phase 5 (Lab variant) 別 session ~3h

## 10. 後続候補
1. Replan stage 拡張 (max_uses 動的化、項目 18 deprecate 道筋)
2. task_type 細粒度化 (4 → 8 カテゴリ)
3. per-tool verification ROI
4. PASS@(k,T) (arxiv 2604.14877) 統合: T 軸 と skip rate の 2D 観測
5. AgentFloor 6-tier 別 skip 戦略

## ファイル一覧 (実装対象)
- `src/runtime/model_router.rs` — AdvisorConfig 2 フィールド
- `src/agent/agent_loop/advisor_inject.rs` — skip hook + classify_task_type
- `src/agent/event_store.rs` — verification_success_rate
- `src/observability/audit.rs` — AdvisorSkip variant
- `src/db/migrate.rs` — V10 index
- `src/agent/agent_loop/tests.rs` — 7 test
- `src/agent/benchmark.rs` — TSV 13 列 + counter
- `CLAUDE.md` — 項目 208 として追記
