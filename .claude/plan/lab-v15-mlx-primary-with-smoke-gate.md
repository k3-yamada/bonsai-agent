# Plan: Lab v15 MLX Primary + FallbackChain + smoke 補正 ACCEPT gate 連動

> **Multi-plan dispatch**: handoff 05-02b (項目 185) で残った TODO #2 (MLX primary 試験設定) + TODO #4 (smoke 補正の ACCEPT gate 連動) を統合。session 05-02b で実装済の `apply_smoke_correction_to_delta` (`experiment.rs:518`) は **prescreening 経路のみ** 連動済で、フル評価後の ACCEPT/REJECT 判定 (`composite_score()` 比較) には未連動。Lab v15 で MLX primary を実運用試験するなら、smoke→core retention 42% の補正は ACCEPT gate でも効かせるべき。

## Task Type

- [ ] Frontend
- [x] Backend (→ Codex)
- [ ] Fullstack

## Background

### 直近セッションの結論（順序）

| 項目 | 内容 | 判定 |
|------|------|------|
| 184 | MLX core 22 baseline=0.7976 (llama 比 +0.0405) | Zone A 寄り C |
| 185 Phase 1 | MLX core 22 再現性=0.8131 (+0.0155) | 許容再現 + Zone A NEAR-MISS |
| 185 Phase 2-3 | FallbackChain 機構実証 (forced/production both pass) | 機能 OK |
| 185 Phase 4 | smoke 補正 sign-aware ×0.42 を `apply_smoke_correction_to_delta` で実装 | prescreening のみ連動 |
| 174 | `handle_lab_mode` β fix: `create_backend(ctx)` 委譲で FallbackChain wrap が Lab 中も適用 | 適用済 |

### 解決すべき残課題

**A. 配備**: `config.toml [fallback_chain]` は **未設定**（SHA `0e33af13` が canonical = llama 単独）。MLX primary 試験には実 entries 注入が必要。

**B. ACCEPT gate**: `run_experiment_loop` のフル評価 ACCEPT 判定で `composite_score() - baseline.composite_score()` を直接 threshold 比較しており、smoke モード時に positive delta が ×0.42 補正されない → **smoke モードでフル ACCEPT が出ると inflated（補正前の値で判定）** という不整合。

## Technical Solution

### B-side: ACCEPT gate に smoke 補正を連動 (実装中の発見で設計修正)

> **2026-05-03 セッション内発見**: 当初 plan は「フル ACCEPT 判定に補正を挟むことで false-accept を防ぐ」という設計だったが、コード読了で **`Experiment::from_multi_results` (experiment_log.rs:197) の accept 判定は `accepted: delta > 0.0`** であり、smoke 補正は **sign-preserving** (positive のみ scaling、sign 不変) なので **threshold=0.0 比較に対しては no-op**。
>
> **修正設計**: 補正は ACCEPT 判定を変えず、**operator 可視化** の informational log として記録。真に false-accept を防ぐなら別途 `accept_threshold > 0.0` field 導入が必要だが、本 plan の YAGNI fence 外なので別 plan へ切出。

`experiment.rs:run_experiment_loop` の **2 箇所** で smoke 補正を扱う:

1. **prescreening (b2)**: 既に実装済 (項目 185, line 940-960) → 当セッションで `apply_smoke_correction_to_delta()` 関数経由に refactor (dead_code warning 解消)
2. **フル評価後 (judge gate 後 / eprintln 前)**: 当セッションで実装 — `lab_smoke_enabled()` 時のみ raw_delta vs adjusted_delta + accepted を log_event(Info) で出力 (accept 判定は変えない)

設計原則 (sign-aware):
- positive delta (改善) → `× smoke_correction_coefficient()` (default 0.42)
- negative delta (劣化) → 保持 (false-accept 防止 — 但し threshold=0.0 では sign-preserving のため判定影響なし)

prescreening 側のロジックは既存関数 `apply_smoke_correction_to_delta` で共通化済。log 用に `smoke_enabled` / `smoke_coeff` を 1 回 cache (Codex audit Low #1 race window mitigation 維持)。

### A-side: MLX primary + llama fallback を設定で配備

`config.toml`:

```toml
[fallback_chain]
max_failures = 2

[[fallback_chain.entries]]
backend = "mlx-lm"
model_id = "mlx-community/Ternary-Bonsai-8B"  # 実 model_id を起動済 mlx-lm から取得
server_url = "http://127.0.0.1:8000"

[[fallback_chain.entries]]
backend = "llama-server"
model_id = "Bonsai-8B"
server_url = "http://127.0.0.1:8080"
```

**注意**: 項目 185 Phase 2 で llama-server `Bonsai-8B` model に POST → HTTP 400 が発生した既知問題あり（plan-2 で別途調査）。Lab v15 開始前に Plan 2 完了が望ましい。**Plan 2 完了前は llama を fallback にせず、MLX 単独 + 健康チェック付き起動で代替検証**する選択肢あり (Step 5b 参照)。

## Implementation Status (2026-05-03 セッション)

| Step | 内容 | 状態 |
|------|------|------|
| Step 1 (D-Red) | フル ACCEPT gate smoke 補正テスト | **skip 判定** — 既存 `test_smoke_correction_*` 5 件 (experiment.rs:1602+) で関数挙動はカバー済、ACCEPT 判定への組込が **sign-preserving + threshold=0.0 で no-op** と確定したため新規テスト不要 |
| Step 2 (D-Green) | ACCEPT gate 連動 | **informational log で実装** — `experiment.rs` の judge gate 後に smoke=1 時のみ raw vs adjusted を log 出力 (accept 判定変更なし) |
| Step 3 (D-Refactor) | prescreening の inline → 関数経由化 | **完了** — `apply_smoke_correction_to_delta()` を実 production path で使用、dead_code warning 解消 |
| Step 4 (A-Setup) | `[fallback_chain]` config 配備 | **未実施** — server 状態確認待ち |
| Step 5 (A-Smoke) | Lab smoke 動作確認 | **未実施** — Step 4 後 |
| Step 6 (A-Full) | Lab v15 core 22 実機実行 | **未実施** — Step 5 後 |
| Step 7 (Verify) | 実機ログ整合性確認 | **未実施** |

### 検証 (2026-05-03 セッション完了時点)

- `cargo test --release --lib`: **1020 passed** (regression なし)
- `cargo clippy --release --lib`: **0 warning** (dead_code 警告解消)
- `cargo fmt --check`: 1 件 pre-existing (agent_loop/tests.rs:1318、本セッション無関係)

## Implementation Steps (A-side のみ未実施、以下原文保持)

### Step 1: [D-Red] フル ACCEPT gate の smoke 補正テスト追加（失敗確認）

`src/agent/experiment.rs` の `#[cfg(test)] mod tests` に以下を追加:

```rust
#[test]
fn test_full_accept_gate_applies_smoke_correction_positive() {
    // smoke モードで raw_delta=+0.10 が ACCEPT 閾値 +0.05 を超えていても、
    // 補正後 +0.042 < +0.05 で REJECT になることを確認
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("BONSAI_LAB_SMOKE", "1");
        std::env::remove_var("BONSAI_LAB_SMOKE_CORRECTION");  // default 0.42
    }
    let raw_delta = 0.10_f64;
    let adjusted = apply_smoke_correction_to_delta(raw_delta);
    let accept_threshold = 0.05;
    // 補正前: 0.10 > 0.05 → ACCEPT(false-accept)
    // 補正後: 0.042 < 0.05 → REJECT (正)
    assert!(adjusted < accept_threshold,
        "smoke モードで raw=+0.10 は補正後 0.042 になり threshold 0.05 を超えない");
}

#[test]
fn test_full_accept_gate_preserves_negative_delta() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("BONSAI_LAB_SMOKE", "1");
    }
    // negative delta は保持される (false-accept 防止)
    let raw_delta = -0.10_f64;
    let adjusted = apply_smoke_correction_to_delta(raw_delta);
    assert!((adjusted - raw_delta).abs() < 1e-9,
        "negative delta は scaling されない");
}
```

`cargo test --release --lib experiment::tests::test_full_accept_gate_applies_smoke_correction_positive` で失敗するはず（実は既存実装は関数自体は完成しているので失敗しないかも — 真の Red は **`run_experiment_loop` 内で実際に呼ばれていない** ことの検証）。

**修正テスト案** (より厳密):

`run_experiment_loop` のフル評価分岐をモックバックエンドで通し、smoke=1 + 観測 score=baseline+0.10 の状況で `accepted=false` が返ることを assert する integration test を追加。MockLlmBackend の応答を制御して baseline と modified の score 差を作る必要があり、実装コストはそれなり。

代替: **logging 検証** — smoke=1 時に `[lab] full-eval smoke correction: raw_delta=... adjusted_delta=... accept=...` のログ行が出力されることを assert。

### Step 2: [D-Green] フル ACCEPT 判定に smoke 補正を組み込む

`src/agent/experiment.rs:run_experiment_loop` のフル評価 ACCEPT 判定箇所（`experiment.composite_score() - baseline.composite_score()` を計算する付近、`accepted = ...` の代入 or `if delta >= accept_threshold` 分岐）を以下のように変更:

```rust
// ── 現状（未補正） ──
let raw_delta = experiment.composite_score() - baseline.composite_score();
let accepted = raw_delta >= accept_threshold;

// ── 変更後 ──
let raw_delta = experiment.composite_score() - baseline.composite_score();
let adjusted_delta = apply_smoke_correction_to_delta(raw_delta);
if lab_smoke_enabled() {
    log_event(
        LogLevel::Info,
        "lab",
        &format!(
            "full-eval smoke correction: raw_delta={:+.4} adjusted_delta={:+.4} threshold={:+.4}",
            raw_delta, adjusted_delta, accept_threshold,
        ),
    );
}
let accepted = adjusted_delta >= accept_threshold;
```

**重要**: TSV / SQLite に記録する `delta` フィールドは **raw_delta** を保持する（補正後の値ではない）。理由:
- TSV/DB は再現性が重要、後から補正係数を変えても遡及的に再評価可能であるべき
- 補正は **判定の側** に閉じ込め、**記録の側** は raw を保持する設計が原則

### Step 3: [D-Refactor] prescreening 側を `apply_smoke_correction_to_delta` 経由に統一

現状 line 940-946 の inline ロジック:

```rust
let smoke_enabled = lab_smoke_enabled();
let smoke_coeff = smoke_correction_coefficient();
let estimated_delta = if smoke_enabled && raw_estimated_delta > 0.0 {
    raw_estimated_delta * smoke_coeff
} else {
    raw_estimated_delta
};
```

を以下に置換:

```rust
let estimated_delta = apply_smoke_correction_to_delta(raw_estimated_delta);
// ログは smoke_enabled 時のみ
if lab_smoke_enabled() {
    log_event(
        LogLevel::Info,
        "lab",
        &format!(
            "pre-screen smoke correction: raw_delta={:+.4} coeff={:.2} adjusted_delta={:+.4} threshold={:+.4}",
            raw_estimated_delta,
            smoke_correction_coefficient(),
            estimated_delta,
            loop_config.prescreening_threshold,
        ),
    );
}
```

**注意**: Codex audit で指摘された「2 回 read で値が乖離するレースウィンドウ」は `apply_smoke_correction_to_delta` 内部で 1 回しか env 読まないので問題なし。ログ目的の追加 read は判定値とは独立。

### Step 4: [A-Setup] `config.toml [fallback_chain]` 設定追加

```bash
# 現在 SHA: 0e33af13 (canonical)
cp ~/Library/Application\ Support/bonsai-agent/config.toml \
   ~/Library/Application\ Support/bonsai-agent/config.toml.pre-v15-fallback
```

`config.toml` 末尾に `[fallback_chain]` セクション追加（上記 Technical Solution の TOML を貼る）。

**前提確認**:
- MLX server (port 8000) が起動済か `curl http://127.0.0.1:8000/v1/models` で確認
- llama-server (port 8080) が起動済か `curl http://127.0.0.1:8080/v1/models` で確認
- 両者 NG なら起動 (`mlx_lm.server` / `llama-server -m ...`)

### Step 5: [A-Smoke] Lab v14 smoke で MLX primary 動作確認

```bash
cd ~/bonsai-agent
BONSAI_LAB_SMOKE=1 cargo run --release -- --lab --lab-experiments 5 \
    2>&1 | tee /tmp/bonsai-llama/lab-v15-smoke.log
```

**確認ポイント**:
- 起動ログに `[fallback] FallbackBackend を構築しました（2 entries、threshold=2）`
- ベースライン計測 / 実験 1-5 で MLX が応答 (record_failure 出ない)
- prescreening / full-eval ログに `smoke correction: raw_delta=... adjusted_delta=...` 両方出力
- 完走時間 ~25 min (MLX cold-start + 5 task × k=3 + 5 prescreening)

**NG時の代替（Step 5b）**: llama HTTP 400 が再発する場合は entries を MLX のみに減らし、FallbackChain の wrap だけ享受する設定で smoke を続行（fallback-on-error は発生しないが、構造的整合性は維持）。Plan 2 完了後に再度両 entries 構成へ戻す。

### Step 6: [A-Full] Lab v15 core 22 実機実行

```bash
BONSAI_BENCH_TIER=core cargo run --release -- --lab --lab-experiments 6 \
    2>&1 | tee /tmp/bonsai-llama/lab-v15-core22.log
```

- `BONSAI_LAB_SMOKE` は未指定 (smoke 補正は OFF、純 raw delta で判定)
- core tier 22 task = baseline ~63 min × (1 + experiments_count) ≒ 6h 想定
- 1 実験 ≒ baseline + prescreening + full-eval = 約 60-90 min/実験

**省コスト変種**: experiments=3 で 3-4h に短縮、初期 ACCEPT 傾向だけ確認しても良い

### Step 7: [Verify] 実機ログから整合性確認

```bash
# smoke モード ログ存在確認 (Step 5 の log で)
grep "smoke correction" /tmp/bonsai-llama/lab-v15-smoke.log | head -10

# fallback 切替の有無 (Step 6 の log で)
grep "\[fallback\]" /tmp/bonsai-llama/lab-v15-core22.log

# ACCEPT gate での補正適用ログ (smoke 時のみ)
grep "full-eval smoke correction" /tmp/bonsai-llama/lab-v15-smoke.log
```

予期される結果:
- Step 5: smoke 時に prescreening + full-eval 両方の補正ログ
- Step 6: BONSAI_LAB_SMOKE 未指定 → 補正ログ 0 件 (raw delta のまま)
- Step 6: MLX primary 安定なら fallback events 0 件

## Key Files

| File | Operation | Description |
|------|-----------|-------------|
| `src/agent/experiment.rs:530-720` (run_experiment_loop) | Modify | Step 2: フル ACCEPT 判定に `apply_smoke_correction_to_delta` 適用 |
| `src/agent/experiment.rs:940-960` (prescreening 分岐) | Refactor | Step 3: inline ロジック → 関数呼出統一 |
| `src/agent/experiment.rs:1602+` (tests) | Add | Step 1: ACCEPT gate smoke 補正テスト 2 件追加 |
| `~/Library/Application Support/bonsai-agent/config.toml` | Append | Step 4: `[fallback_chain]` セクション追加 |

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| R1: ACCEPT 判定変更で既存 Lab 結果と互換性が崩れる | smoke モード時のみ補正適用、非 smoke は raw delta のままで TSV/DB 互換 |
| R2: MLX cold-start 227s で Step 5 socket timeout 発生 | 項目 167 socket timeout 180s/recv → 項目 174 fallback で llama に自動切替で吸収 |
| R3: llama HTTP 400 (Plan 2 未解決) で fallback 連鎖失敗 | Step 5b 代替（MLX 単独 entries）で迂回、Plan 2 完了後に両 entries 復帰 |
| R4: `apply_smoke_correction_to_delta` を ACCEPT gate に挟むと既存 prescreening tests が干渉 | 既存 5 tests (`test_smoke_correction_*`) は関数 unit test なので干渉しない、新規 ACCEPT gate tests は別関数 |
| R5: TSV `delta` カラムに raw を残すか adjusted を残すか論争 | 設計原則「記録は raw、判定は補正」で raw 保持に決定、補正係数の遡及変更で再評価可能性を維持 |
| R6: smoke 補正係数 0.42 が core で過小補正の場合がある (項目 184: smoke +0.0969 → core +0.0405) | 補正は default 0.42 + env override 可能、Lab v15 結果次第で調整 |

## Decision Gate (Step 6 完了後)

- **Zone A (≥0.82)**: Lab v15 core 22 score ≥ Phase 1 (0.8131) + Zone A 突破 → **MLX primary 恒久化、項目 184/185 既存実装の効果を実証完了**
- **Zone B (0.78-0.82)**: 微改善 → 統計的有意性確認のため 1 サイクル追加実行
- **Zone C (<0.78)**: MLX primary が core で安定しない → llama-only に戻し、別アプローチ検討

## Stop Conditions

- Step 5 smoke で fallback chain 枯渇 → MLX entries だけに減らして再試行 (Step 5b)
- Step 6 で 90 min 経過しても baseline 未完了 → MLX cold-start 待機過剰の疑い、socket_timeout 設定見直し
- ACCEPT 判定ログで `full-eval smoke correction` が出ない → Step 2 実装漏れ、cargo test で再確認

## SESSION_ID (for /ccg:execute)

- CODEX_SESSION: (none — Claude direct planning, codex/architect.md は execute 時に dispatch)
- GEMINI_SESSION: (n/a — gemini analyzer.md 不在のため使用せず)
