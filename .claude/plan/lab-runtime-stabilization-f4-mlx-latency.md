# Lab Runtime Stabilization F4 — MLX 2-bit Primary Latency 構造対処 (項目 249 続編)

**状態**: planning-only (2026-05-19 起票、項目 249 Phase 4 G-RT REJECT を受けた次手設計)
**推奨度**: ★★★ (Lab v22 paired (~5-8h) 完走の prerequisite、F1+F2 単独では target ≤35 min/cycle 未達)
**推定工数**: ~2-3h plan + Phase 1-3 impl + smoke G-RT2 (T=0 + MLX-only + F4) ~30 min
**前提**: 項目 249 F1 (`BONSAI_LAB_LONG_SSE=1`、sse_chunk_timeout 60→180) + F2 (`BONSAI_LAB_MLX_ONLY=1`、fallback chain clear + primary を `ServerBackend::MlxLm` + 8000 + `prism-ml/Ternary-Bonsai-8B-mlx-2bit` に切替) は実装済 (commit `5a01a45` + F2 fix commit、項目 249 Phase 1-3 完遂)

---

## §0. 起点・問題定義

### 0.1 項目 249 Phase 4 G-RT 実機計測 (REJECT 確定)

| 軸 | 実測 | target | 判定 |
|---|---|---|---|
| 設定 | F1+F2+T=0+SMOKE=1 (5 task × k=3) | — | — |
| Wall time (1 cycle) | **103m 42s** | ≤ 35 min | **REJECT (3x 超過)** |
| SSE timeout 発火回数 | 5 回 | 0 | F1 180s でも catch しきれず |
| 非ストリーミング fallback | 発生 | 不要 | streaming で完結する想定が外れた |
| 推定 Lab v22 paired (10 cycle) 完走時間 | **~17h** | ≤ 5h | 推定 3.4x 超過 |

### 0.2 根本原因 (項目 249 finding 整理)

- **MLX 2-bit primary は llama-server 1-bit gguf より latency ~2x 高い** (Ternary 精度 +5pt の対価)
- F1 (sse_chunk_timeout 180s) でも MLX 初トークン latency を catch しきれない場面が複数発生
  - MLX cold start (重み load + Metal kernel JIT) が 180s 超える可能性
  - SSE handshake 完了から最初の token chunk まで 180s 以上空く可能性
- F2 (MLX-only) で fallback chain overhead は消えたが、MLX 単独の primary latency が高止まり
- T=0 で 1bit reasoning loop により max_iterations まで token 吐き出すケースが残存 (項目 249 Phase A finding と整合)

### 0.3 制約

- **Ternary 精度 +5pt は維持したい** (項目 184/185 で実証、AgentFloor T6 LongHorizon 強化に必須)
- Lab v22 paired (~5-8h) 完走基準 = 1 cycle ≤ 35 min は確定 hard requirement
- production code は env-gated で touch、default OFF 維持

---

## §1. F4 案比較 (5 軸評価)

### 1.1 評価軸

1. **A1. Lab v22 paired 5h 完走可能性** — 1 cycle ≤ 35 min を達成できるか (◎/○/△/✕)
2. **A2. 実装工数** — Phase 1-3 完遂までの推定時間 (h)
3. **A3. Ternary 精度維持可能性** — +5pt を保てるか (◎/○/△/✕)
4. **A4. リスク** — production 影響、Lab paired stability 影響 (low/mid/high)
5. **A5. ロールバック容易性** — env unset / config 戻しで完全 backward compat か (◎/○/△/✕)

### 1.2 案比較表

| 案 | A1 完走 | A2 工数 | A3 精度 | A4 リスク | A5 rollback | 推奨度 |
|---|---|---|---|---|---|---|
| **A. MLX server pre-warm** | ○ | 1-2h | ◎ | low | ◎ | **★★★ (推奨)** |
| **B. non-streaming default 化 (Lab gate)** | ◎ | 1.5-2h | ◎ | mid | ◎ | **★★ (併用候補)** |
| C. 1-bit gguf primary 回帰 (Ternary 別 A/B) | ◎ | 0.5-1h | ✕ | low | ◎ | ★ (退避案、Ternary 評価不可) |
| D. HTTP/2 multiplex / batching | △ | 4-6h | ◎ | high | △ | (中長期、本 phase 対象外) |
| E. MLX server config tuning (KV cache + early stop) | △ | 1h | ◎ | mid | ○ | ★ (補助、A と組合せ可) |

### 1.3 各案詳細

#### 案 A: MLX server pre-warm (★★★ 推奨)

**機構**:
- Lab 起動前に MLX server に warm-up request (短 prompt の completion) を 1-3 回投入
- 重み load + Metal kernel JIT を request 計時計算から外す
- pre-warm 完了確認後に Lab cycle 開始

**期待効果**:
- cold start による初トークン latency (推定 60-120s) を Lab 計時から完全に外す
- F1 180s timeout の余裕分を活用、SSE timeout 発火 5 回 → 0 回想定
- 1 cycle wall: 103m → ~25-30 min (cold start 削減 + reasoning loop 余地)

**実装**:
- `scripts/start-mlx-server.sh` に warm-up curl 追加 (短 prompt、k=3 程度) → server 側で重み hot 化
- OR Rust 側で `experiment.rs` の lab gate に pre-warm step 追加 (`BONSAI_LAB_MLX_WARMUP=1` env-gated、default OFF)
- warm-up 失敗は WARN log のみで cycle 続行 (graceful degradation)

**A1 評価**: ○ — cold start 削減で 30 min 目標達成、ただし reasoning loop (項目 249 finding) は別問題で残存可能性
**A2 評価**: 1-2h (env getter + main.rs / experiment.rs 配線 + smoke test)
**A3 評価**: ◎ — MLX 2-bit primary 維持、Ternary +5pt そのまま
**A4 評価**: low — pre-warm 失敗時も Lab cycle 進行可、production code 経路への影響なし
**A5 評価**: ◎ — env unset で完全 backward compat、warm-up logic 自体は no-op 化

#### 案 B: non-streaming default 化 (Lab gate) (★★ 併用候補)

**機構**:
- Lab 中だけ SSE streaming 経路を破棄、completion REST API (non-streaming) を default 化
- `BONSAI_LAB_NONSTREAM=1` env-gated、default OFF
- streaming token callback は no-op、final response を 1 度で受け取る

**期待効果**:
- SSE chunk timeout 機構自体を bypass、F1 180s 超過の MLX 初トークン latency 問題が消滅
- non-streaming は全 token 生成後に response 返却なので「中継 timeout」が発生しない
- 1 cycle wall: 103m → ~30-35 min (streaming overhead 削減 + timeout 排除)

**実装**:
- `src/runtime/llama_server.rs` または backend layer で `is_lab_nonstream()` env check
- ON 時に `/v1/chat/completions` の `stream: false` で投入、token callback は最後に 1 度 emit
- audit_log に `stream_mode: nonstream` を記録 (lab paired 比較で識別可能化)

**A1 評価**: ◎ — SSE timeout 機構を完全 bypass、確実に 30 min 達成
**A2 評価**: 1.5-2h (backend layer 改修 + env getter + smoke)
**A3 評価**: ◎ — モデル切替なし、Ternary +5pt 維持
**A4 評価**: mid — production code (backend) 変更経路、env-gated だが backend layer touch が必要
**A5 評価**: ◎ — env unset で streaming に戻る、設計上 1 行 branch

**注意**: token callback 経由の progress monitor (audit_log AssistantMessage emit、項目 237) が non-streaming 化で挙動変わる可能性 → smoke で確認必須

#### 案 C: 1-bit gguf primary 回帰 (Ternary 別 A/B 専用) (★)

**機構**:
- production config の primary を `llama-server` + `Bonsai-8B` (port 8080) に戻す
- Lab paired は 1-bit gguf で完走優先、Ternary +5pt は別途 A/B test 専用に温存
- `BONSAI_LAB_GGUF_ONLY=1` env-gated で MLX を完全に外す (F2 と類似だが backend 違い)

**期待効果**:
- 1 cycle wall: 103m → ~25-30 min (gguf 経路は項目 184 で 47 min/cycle 実測あり、smoke 15 task で類推 ~25 min)
- 確実な完走、Lab v22 metric redesign (項目 247) 検証が最優先される運用

**A1 評価**: ◎ — gguf 経路は項目 184 等で多数完走実績、smoke 15 task で確実 30 min 内
**A2 評価**: 0.5-1h (env getter + 既存 fallback_chain clear ロジック流用)
**A3 評価**: ✕ — **Ternary +5pt 諦め**、AgentFloor T6 強化が後ろ倒し
**A4 評価**: low — backend 経路は既存 production の常道
**A5 評価**: ◎ — env unset で MLX primary に戻る

**退避案として残す価値**: Lab v22 metric redesign (項目 247) 完遂が Ternary 評価より優先される局面で採用候補

#### 案 D: HTTP/2 multiplex / batching (中長期)

**機構**:
- k=3 同一 task の 3 inference を HTTP/2 multiplex / MLX server batching で同時投入
- latency hide で wall time 圧縮 (理論 3x speedup、現実 1.5-2x)

**期待効果**:
- 1 cycle wall: 103m → ~50-60 min (3x latency hide、ただし MLX server 側 batching 対応必要)

**A1 評価**: △ — 効果は理論的、MLX server / mlx-openai-server の batching 実装依存
**A2 評価**: 4-6h (client 側 multiplex + server 側 batching 確認 + smoke 多周回)
**A3 評価**: ◎ — モデル切替なし
**A4 評価**: high — 同時 inference で memory pressure / Metal kernel contention 等の高度問題、M2 16GB で破綻可能性
**A5 評価**: △ — multiplex client 経路 / batching config の rollback が複雑

**判定**: 本 phase 対象外、中期 watch 案件

#### 案 E: MLX server config tuning (補助)

**機構**:
- `mlx-openai-server` の起動 args 調整 (KV cache size、early stop trigger、max_tokens 削減)
- 例: `--max-tokens 512` (default 2048)、`--early-stop`、KV cache prealloc

**期待効果**:
- reasoning loop で max_iterations まで吐く挙動の抑制
- 1 cycle wall: 103m → ~70-80 min (推定、reasoning loop tail 削減のみ)

**A1 評価**: △ — 単独では target 未達、案 A と併用で効果上乗せ
**A2 評価**: 1h (start-mlx-server.sh の args 拡張 + smoke 確認)
**A3 評価**: ◎
**A4 評価**: mid — max_tokens 強制で long-horizon task 影響可能性
**A5 評価**: ○ — script 戻すだけ

**判定**: 案 A の補助 (Lab gate でのみ tuning args 投入) として採用検討

---

## §2. 推奨案 (A + B 段階導入)

### 2.1 採用判断

**Phase 1: 案 A 単独 (MLX server pre-warm) を最優先実装**

理由:
- A1 完走可能性が ○ で MLX 2-bit primary 維持、Ternary +5pt 維持の Pareto 最適
- A4 リスクが low、production code 影響ゼロ (script + env-gated lab gate のみ)
- A5 rollback ◎ で env unset で完全 backward compat
- A2 工数 1-2h で他案より軽量

**Phase 2 (案 A で target 未達時のみ): 案 B (non-streaming default 化) を追加導入**

理由:
- 案 A で cold start は削減できるが、reasoning loop 中の SSE chunk timeout が残存可能性
- 案 B は SSE timeout 機構を完全 bypass する確実策、案 A と直交で併用可能
- 単独 (案 B のみ) は production code touch が必要なので、まず案 A で十分か検証

**案 C (1-bit gguf 回帰) は退避案として保持** (Lab v22 metric redesign の進捗次第で切替可)

### 2.2 Phase 4 smoke 合格基準

**G-RT2 (案 A 単独)**:
- 設定: F1+F2+F4-A+T=0+SMOKE=1 (5 task × k=3)
- target: **1 cycle wall ≤ 35 min**
- SSE timeout 発火: ≤ 1 回 (案 A で cold start 削減 + F1 180s で catch)
- 非ストリーミング fallback: 0 回 (発生なし)
- pre-warm 所要時間: ≤ 2 min (cycle 計時に含めず、separate measurement)

**G-RT3 (案 A + B 併用、案 A 未達時のみ実行)**:
- 設定: F1+F2+F4-A+F4-B+T=0+SMOKE=1
- target: **1 cycle wall ≤ 30 min** (案 B で SSE 機構 bypass、余裕分)
- SSE timeout 発火: 0 回 (機構 bypass で原理的にゼロ)
- AssistantMessage event emit (項目 237) の動作確認必須

---

## §3. 実装 (TDD strict 3-phase、案 A 詳細)

### Phase 1 (Red) — 4 failing test

`src/config.rs::tests` または該当 module に追加:

1. `t_lab_mlx_warmup_env_gate_active`: `is_lab_mlx_warmup()` → `BONSAI_LAB_MLX_WARMUP=1` で `true`、unset で `false`
2. `t_lab_mlx_warmup_count_default`: `lab_mlx_warmup_count()` → unset 時 `Some(3)` (default 3 回)
3. `t_lab_mlx_warmup_count_parse`: `BONSAI_LAB_MLX_WARMUP_COUNT=5` で `Some(5)`
4. `t_lab_mlx_warmup_count_out_of_range`: 値 0 or 11 (10 超) で `None` (range 1..=10 guard)

cross-file env mutex は項目 226/229/233/235 の pattern を踏襲 (`pub(crate) static LAB_MLX_WARMUP_ENV_TEST_LOCK: Mutex<()>`)。

### Phase 2 (Green) — config getter + lab gate 配線

#### 2.1 `src/config.rs` 追加

```rust
/// Lab 専用 MLX pre-warm gate (項目 249 F4 案 A)
/// BONSAI_LAB_MLX_WARMUP=1 で有効
pub fn is_lab_mlx_warmup() -> bool {
    std::env::var("BONSAI_LAB_MLX_WARMUP")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// pre-warm 投入回数 (1..=10、default 3)
pub fn lab_mlx_warmup_count() -> Option<usize> {
    let raw = std::env::var("BONSAI_LAB_MLX_WARMUP_COUNT").ok()?;
    let n: usize = raw.parse().ok()?;
    if (1..=10).contains(&n) { Some(n) } else { None }
}
```

#### 2.2 `src/agent/experiment.rs::run_experiment_loop` 配線

```rust
// lab gate の冒頭 (F2 fallback clear の直後あたり)
if config::is_lab_mlx_warmup() {
    let n = config::lab_mlx_warmup_count().unwrap_or(3);
    log_event(LogLevel::Info, "lab",
        &format!("[lab] BONSAI_LAB_MLX_WARMUP=1 → MLX pre-warm {n} 回投入"));
    for i in 0..n {
        // 短 prompt の completion 投入 (token 数最小化)
        let warm_result = backend.generate(
            &[Message::user("ping")],
            &[],
            &mut |_| {},
            &CancellationToken::new(),
        );
        match warm_result {
            Ok(_) => log_event(LogLevel::Info, "lab",
                &format!("[lab] pre-warm {}/{} OK", i+1, n)),
            Err(e) => log_event(LogLevel::Warn, "lab",
                &format!("[lab] pre-warm {}/{} FAIL: {e}", i+1, n)),
        }
    }
    log_event(LogLevel::Info, "lab", "[lab] pre-warm 完了、Lab cycle 開始");
}
```

**注意**: pre-warm の wall time は cycle 計時に含めない (Lab metric には影響しない)。pre-warm 失敗時も cycle 続行 (graceful degradation)。

#### 2.3 `scripts/start-mlx-server.sh` への補助 warm-up 追加 (optional)

```bash
# server 起動完了確認 (既存 health check) の後に追加
echo "[mlx-server] pre-warm 1 回投入..."
curl -sS -X POST http://127.0.0.1:8000/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"prism-ml/Ternary-Bonsai-8B-mlx-2bit","messages":[{"role":"user","content":"ping"}],"max_tokens":1}' \
  > /dev/null && echo "[mlx-server] pre-warm OK"
```

これは script-level で server 起動時に 1 回行うもので、Rust 側 pre-warm (env-gated) とは独立。

### Phase 3 (Refactor)

- env getter doc コメント追記 (項目番号、Lab v22 paired 完走目的明示)
- log prefix `[lab] BONSAI_LAB_MLX_WARMUP=1 → ...` の SSOT 化
- clippy/fmt clean、test 1319 → 1323 passed (+4)
- README / handoff には memo 起こさず (本 plan が SSOT)

### Phase 4 (Smoke G-RT2) — 合格基準 §2.2

1. `cargo build --release` (lab 稼働中は禁止、別 session で実行)
2. MLX server 起動確認 (`scripts/start-mlx-server.sh`、port 8000)
3. smoke 起動:
   ```bash
   BONSAI_LAB_LONG_SSE=1 \
   BONSAI_LAB_MLX_ONLY=1 \
   BONSAI_LAB_MLX_WARMUP=1 \
   BONSAI_LAB_TEMP=0 \
   BONSAI_LAB_TASK_LIMIT=5 \
   nohup ./scripts/lab_v22_aa_test.sh ./lab-v22-rt2-logs \
     > /tmp/lab_v22_rt2.log 2>&1 &
   ```
4. cycle 1 完了後に wall time 検証 (≤ 35 min ACCEPT)
5. ACCEPT なら Lab v22 paired full run (15 task × 10 cycle、~5h target) 起動可

---

## §4. 想定 risk / mitigation

| # | Risk | Mitigation |
|---|---|---|
| R1 | pre-warm 自体が cold start を完全に削減しきれない (server 内 KV cache 圧縮等) | pre-warm count を env で 3→5 拡張可能、smoke で count 別 wall 計測 |
| R2 | pre-warm 失敗で MLX server 不安定化 | graceful degradation (WARN log のみ)、cycle 続行 |
| R3 | env-gated だが experiment.rs に backend.generate 直呼出が増えて test 影響 | TDD Phase 1 で env mutex pattern を踏襲、既存 production code path 不変 |
| R4 | Phase 4 smoke で 35 min 未達なら案 B (non-streaming) 追加実装が必要、追加 ~2h | 案 B の outline は §1.3 に保持、Phase 1 で env mutex pattern を準備 |
| R5 | Lab v22 paired full (15 task × 10 cycle ~5h) で 5h 超過 → Lab v22 metric redesign 検証が後退 | smoke G-RT2 で wall 確証後に paired 起動、smoke 段階で REJECT 判定 |
| R6 | pre-warm の token consumption が無駄 (max_tokens=1 でも 数十 token) | M2 16GB / 1bit でコスト negligible、warm-up count guard (1..=10) で上限化 |

---

## §5. 期待効果

### 短期 (本 phase Phase 4 G-RT2 合格時)
- Lab v22 paired smoke (15 task × 10 cycle) wall ~5h で完走可能化
- Ternary 2-bit 精度 +5pt 維持 (AgentFloor T6 LongHorizon 強化の前提保持)
- Lab v22 metric redesign (項目 247 Phase B/C/D) の paired 実機検証が unblock

### 中期 (Lab v23+)
- pre-warm pattern が MLX backend の Lab 運用 SSOT 化
- 案 B (non-streaming) を必要な局面で env-gate 投入可能 (本 plan §1.3 で outline 確保)
- AgentFloor 6-tier × Lab v22 二軸 metric の paired 検証が現実的単位 (1 day = 8 cycle)

### 長期 (Lab v24+、案 D 検討時)
- MLX server batching / HTTP/2 multiplex の実機検証着手 (case 別 plan)
- 1-bit gguf vs Ternary 2-bit の AgentFloor tier 別 ablation (項目 184/185 拡張)

---

## §6. 依存 / 並行性

### 完遂前提
- 項目 249 Phase 1-3 完遂済 (F1+F2 実装、commit `5a01a45` + F2 fix commit)
- 項目 249 Phase 4 G-RT REJECT 確定済 (本 plan の起点)
- production config (`~/Library/Application Support/bonsai-agent/config.toml`) は MLX primary + llama fallback で fixed (memory `fallback_chain_mlx_finding.md`)

### 並行可
- 項目 246 Phase 5 (Vault lint follow-up `vault-lint-bail-branch-test.md`、独立)
- 項目 247 Phase A 再起動 (Lab v22 metric redesign σ_Δ 採取、本 plan ACCEPT 後)
- 項目 248 Phase 4 (Dynamic Budget runtime wiring、別 phase)

### 排他
- 本 plan F4-A (env-gated pre-warm) は `src/agent/experiment.rs` の lab gate に隣接、項目 246/247 の experiment.rs touch と同一区画 → commit 順序整理必要
- F4-B (non-streaming default 化、§1.3 outline) は backend layer touch、本 plan Phase 4 で未達時の追加 phase 化

---

## §7. ロールバック戦略

- F4-A 単独 rollback: `BONSAI_LAB_MLX_WARMUP` unset で完全 backward compat、env getter logic は no-op 化
- 完全 rollback (実装ごと): `git revert <commit>` で 1 commit reversal
- production code (experiment.rs) は env gate 内 branch のみで分岐、unset 時の挙動は項目 249 F2 完了時点と同一

---

## §8. Quick Start

```bash
cd /Users/keizo/bonsai-agent
git log -3 --oneline

# Phase 1 Red: 4 failing test
$EDITOR src/config.rs    # is_lab_mlx_warmup / lab_mlx_warmup_count + 4 test
cargo test --lib lab_mlx_warmup  # 4 失敗確認

# Phase 2 Green: experiment.rs 配線
$EDITOR src/agent/experiment.rs  # lab gate に pre-warm step
cargo test --lib  # 1319 → 1323 passed (+4)

# Phase 3 Refactor
cargo clippy --tests -- -D warnings
cargo fmt
cargo test --lib  # 1323 passed 維持

# Phase 4 Smoke G-RT2
cargo build --release
./scripts/start-mlx-server.sh  # MLX server 8000 起動確認
BONSAI_LAB_LONG_SSE=1 \
BONSAI_LAB_MLX_ONLY=1 \
BONSAI_LAB_MLX_WARMUP=1 \
BONSAI_LAB_TEMP=0 \
BONSAI_LAB_TASK_LIMIT=5 \
nohup ./scripts/lab_v22_aa_test.sh ./lab-v22-rt2-logs > /tmp/lab_v22_rt2.log 2>&1 &
# cycle 1 wall ≤ 35 min なら ACCEPT
```

---

## §9. metadata

- 起点 commits:
  - `5a01a45` (項目 249 Phase 1-3: F1+F2+F3 env-gated 実装)
  - F2 拡張 commit (項目 249 Phase 4 で primary backend 切替 fix、`2f27c9e` 周辺)
- 起点 finding (REJECT 確定):
  - 項目 249 Phase 4 G-RT smoke: wall 103m 42s / SSE timeout 5 回 / 非ストリーミング fallback 発火
- 関連 plan:
  - `.claude/plan/lab-runtime-stabilization.md` (項目 249 親 plan、F1-F3)
  - `.claude/plan/lab-v22-metric-redesign.md` (項目 247、Lab v22 paired 起動 prerequisite)
  - `.claude/plan/fallback-chain-impl.md` (項目 195 系列、fallback 設計の歴史的文脈)
- 関連 memory:
  - `fallback_chain_mlx_finding.md` (production config 実体、MLX primary 残置決定)
  - `ternary_bonsai_paths_2026_05_19.md` (5 経路調査、経路 1 MLX 2-bit 採用根拠)
  - `ternary_bonsai.md` (start-mlx-server.sh 運用 protocol)
- 想定 commit 範囲: 1-2 commit (config.rs getter + experiment.rs 配線)
- 想定 line 範囲: +60 行 / -2 行 (env getter + lab gate pre-warm step)
- 本 plan の項目化: 項目 250 候補 (Phase 1-3 完遂 + Phase 4 G-RT2 ACCEPT 時)
- 案 B (non-streaming default 化) を separate phase 化する場合は本 plan §1.3 outline を別 plan に分離: 候補 plan 名 `lab-runtime-stabilization-f4b-nonstream.md`
