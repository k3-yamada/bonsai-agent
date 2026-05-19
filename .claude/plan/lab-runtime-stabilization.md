# Lab Runtime Stabilization — cycle 80 min → 30 min 圧縮 (項目 249 候補)

**状態**: planning-only (2026-05-19 起票、CCG synthesis 経由)
**推奨度**: ★★★ (Lab v22 Phase A 実機 80 min/cycle → 30 min target で 2.6x 圧縮必須、Lab v22+ 再起動 prerequisite)
**推定工数**: ~2-3h plan + Phase 1-3 impl + smoke G-PR (T=0 + MLX-only) ~30 min
**起点**:
- Lab v22 Phase A 実機 (2026-05-19 09:38 起動、12:37 kill): cycle 1 = 78 min、cycle 2 = 81 min wall (想定 30 min の 2.6x)
- CCG synthesis (Codex 統計実装 + Gemini LLM eval workflow):
  - Codex root cause: SSE timeout 60s default 短すぎ → non-stream retry → fallback chain で 13 件 timeout 蓄積
  - Gemini root cause: 1bit reasoning loop で T=0 deterministic に max_iterations まで token 吐き続ける + Memory Bandwidth bottleneck で sampling layer 差 <1%
- Phase A データは fallback 汚染で待つ価値なし、両 advisor が kill 推奨で kill 実施 (12:37)

---

## §1. 問題定義

### 1.1 Lab v22 Phase A 実機計測 (kill 前)
- 起動: 2026-05-19 09:38 (BONSAI_LAB_TEMP=0、smoke 15 task、MLX primary + llama fallback)
- cycle 1 (test_on_1): 78 min、SSE timeout 78 件
- cycle 2 (test_off_1): 81 min、SSE timeout 53 件
- cycle 3 (test_on_2): 12:18 開始、18 min 経過で kill (33 timeout in progress)

### 1.2 過去 Lab 実測との比較
| Lab | task | k | T | cycle wall |
|---|---|---|---|---|
| v20 (core 32) | 32 | 3 | 0.5 | 114 min |
| v21 smoke | 15 | 3 | 0.5 | 57 min |
| v22 Phase A | 15 | 3 | 0 | **~80 min (+40% vs v21)** |

→ **T=0 は遅くなった** (sampling layer の wall 差 <1% per Gemini、reasoning loop は max_iterations まで)。

### 1.3 root cause hypothesis (CCG 統合)
1. **SSE timeout 60s default が MLX 初トークン遅延を catch** (Codex)
   - llama_server.rs:38 で `sse_chunk_timeout_secs: 60` default
   - MLX server の cold start / 1bit decoding latency が 60s 超
   - timeout → non-streaming retry → 同 backend 内 retry + fallback chain trigger
2. **fallback chain (MLX → llama-server) の retry コスト固定化** (Codex)
   - max_failures=2、recover_after_n_success=10 で「一度 llama 側に寄ると戻りにくい」
   - Phase A の noise floor 測定が「MLX」ではなく「MLX/llama timeout 混合系」
3. **T=0 で reasoning loop に陥る** (Gemini)
   - 1bit モデルのエントロピー低、deterministic で「同じ思考のループ」
   - 早期終了せず max_iterations / task_timeout 限界まで token 吐き出す
   - T=0 が逆に wall を延ばす方向に作用する可能性

---

## §2. 設計 (Phase 1-3、Codex 案 A 採用)

### 2.1 ACCEPT (Lab cycle ≤ 35 min)

技術修正の優先度 (Codex 推奨):

| # | 修正 | 期待効果 | risk |
|---|---|---|---|
| F1 | `sse_chunk_timeout_secs` default 60 → 180 | SSE timeout 60s 関連 retry を消滅 | low、production code 1 行変更 |
| F2 | Lab 専用 MLX-only mode (fallback chain 無効化 env gate) | retry chain による 2nd backend 経由 timeout 消滅 | low、env-gated default OFF |
| F3 | task pool 縮小 (`BONSAI_LAB_TASK_LIMIT=N` env)、smoke 15 → 5 | 単純線形短縮 (45 task → 15 task で 1/3)、smoke triage 速化 | medium、estimand 変わる (5 task の noise floor は別計算系) |

組み合わせ効果想定:
- F1 単独: 80 → ~50 min (SSE timeout 消滅で fallback chain 経路スキップ)
- F1+F2: 80 → ~35 min (MLX-only で純粋計測、fallback overhead ゼロ)
- F1+F2+F3 (5 task): ~12 min/cycle (target 30 min を大幅下回り、smoke triage 用)

### 2.2 ACCEPT 後の Lab 運用 protocol
- F1+F2 で **30 min/cycle target 達成**
- Lab v22 Phase A 再起動 (5 task lightweight 版 OR 15 task 通常版で wall 5h)
- Lab v22 Phase D pilot (smoke 10 cycle = 5h with F1+F2) 起動

---

## §3. 実装 (TDD strict 3 phase)

### Phase 1 (Red) — 4 failing test

1. `t_sse_chunk_timeout_default_180`: `InferenceParams::default().sse_chunk_timeout_secs == 180`
2. `t_lab_mlx_only_env_gate_active`: `is_lab_mlx_only()` → BONSAI_LAB_MLX_ONLY=1 で true
3. `t_lab_task_limit_env_parse`: `lab_task_limit()` → BONSAI_LAB_TASK_LIMIT=5 で `Some(5)`
4. `t_lab_task_limit_env_out_of_range`: `lab_task_limit()` → 値 0 or 16 (15 超) で None

### Phase 2 (Green)

- `src/config.rs::InferenceParams::default` で `sse_chunk_timeout_secs: 60 → 180` (default のみ、既存 toml override は維持)
- `src/config.rs` に `is_lab_mlx_only()` / `lab_task_limit()` env getter
- `src/main.rs` の lab gate (lab_temperature_override の隣) で:
  - `is_lab_mlx_only()` 時 fallback_chain を消去 (`app_config.fallback_chain.entries.clear()`)
- `src/agent/experiment.rs::run_experiment_loop` で:
  - `lab_task_limit()` が Some(n) なら `suite.tasks().take(n)` で truncate

### Phase 3 (Refactor)

- env getter SSOT、log prefix `[lab] BONSAI_LAB_MLX_ONLY=1 → fallback chain 無効化` 等
- clippy/fmt clean、test 1312 → 1316 passed (+4)

### Phase 4 (Smoke G-RT)

1. Phase A 再起動 (lab_v22_aa_test.sh、BONSAI_LAB_MLX_ONLY=1 BONSAI_LAB_TEMP=0)
2. cycle 1 wall を実測、F1+F2 で ≤ 35 min なら ACCEPT
3. F3 (task_limit=5) 追加で ≤ 12 min 確認

---

## §4. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | sse_chunk_timeout 180 で MLX 完全 hang のとき agent loop が長時間 stuck | task_timeout_secs (現 300) で global limit、agent 側で recover |
| R2 | MLX-only で MLX 不安定なら task fail 増加 | env-gated default OFF、Lab paired のみ ON |
| R3 | task_limit で smoke の estimand 変わる、過去 Lab v21/v22 結果と直接比較不可 | metadata に lab_task_limit を記録、別系列扱い |
| R4 | sse_chunk_timeout 増で existing TOML override (180 等の明示) が priority 取れているか | test で TOML override path も確認 |
| R5 | fallback chain 無効化で production 影響 (CLI 通常使用) | env gate で Lab only、production 路線は無変化 |

---

## §5. 期待効果

### 短期 (Lab v22 再起動)
- Phase A cycle 30 min target 達成、smoke 10 cycle = 5h で完走
- Phase D pilot 5h 内、Phase E full lab 10-13h (本来 plan §3.3 power table と整合)

### 中期 (Lab v23+)
- 1 cycle 30 min は development loop の現実的単位、daily smoke が可能
- Pearson r / Wilcoxon の検出力評価が cycle 数追加で改善 (現状 n=5 限界打破)

### 長期 (CCG Gemini Iteration Velocity philosophy)
- 「精度より勾配の向き」優先で 30 min/cycle = 1 day 8 cycle 可能
- weekly decision lab (40 cycle) / monthly strict lab (108 cycle) の cadence が成立

---

## §6. 依存 / 並行性

### 完遂前提
- Phase A kill 済 (本 plan の起点)、Lab cycle 中の cargo build --release 制約解除済

### 並行可
- 項目 246 Phase 4 wiring (experiment.rs lint 呼出追加、本 plan F2 env gate と隣接コード)
- 項目 248 Phase 4 wiring (compaction.rs 統合、本 plan F3 task_limit と独立)

### 排他
- 本 plan F1 (sse default 変更) は production code 変更で test 影響、TDD strict で safety 確保
- F2 (fallback chain 無効化) は main.rs lab gate、項目 247 Phase C wiring と同一区画

---

## §7. ロールバック戦略

- F1 (sse 180): toml override で従来 60 に戻す可能、または revert 1 行
- F2 (MLX-only env): env unset で完全 backward compat
- F3 (task_limit env): env unset で smoke 15 task 維持
- 完全 rollback = `git revert <commit>` で 1-2 commit reversal

---

## §8. Quick Start

```bash
cd /Users/keizo/bonsai-agent
git log -3 --oneline

# Phase 1 Red: 4 failing test
$EDITOR src/config.rs   # InferenceParams.sse default 60→180、is_lab_mlx_only、lab_task_limit
$EDITOR src/main.rs     # cli.lab で is_lab_mlx_only 時 fallback chain clear
$EDITOR src/agent/experiment.rs  # lab_task_limit で suite truncate
cargo test --lib  # 1312 → 1316 passed (+4)

# Phase 4 Smoke G-RT
cargo build --release
BONSAI_LAB_MLX_ONLY=1 nohup ./scripts/lab_v22_aa_test.sh ./lab-v22-aa-logs-rt > /tmp/lab_v22_rt.log 2>&1 &
# cycle 1 = ≤ 35 min なら F1+F2 ACCEPT

# Lab v22 再起動 (clean run)
nohup ./scripts/lab_v22_aa_test.sh ./lab-v22-aa-logs > /tmp/lab_v22_aa.log 2>&1 &
# ~5h 後
python3 scripts/lab_v22_metric.py ./lab-v22-aa-logs --mode aa  # σ_Δ 出力
```

---

## §9. metadata

- 起点 commits: `5109219` (項目 248 Phase 2+3)、その前の Phase A kill
- 起点 CCG artifacts:
  - codex: `.omc/artifacts/ask/codex-bonsai-agent-rust-1bit-bonsai-8b-lab-paired-evaluation-phase-2026-05-19T03-51-42-933Z.md`
  - gemini: `.omc/artifacts/ask/gemini-bonsai-agent-lab-v22-phase-a-a-a-test-2-6x-slow-down-cycle-8-2026-05-19T03-43-25-339Z.md`
- 関連 plan: `lab-v22-metric-redesign.md` (項目 247、Phase A 起動 plan)
- 関連 memory: `fallback_chain_mlx_finding.md` (項目 245 で記録の Lab v21 paired slowdown root cause)
- 想定 commit 範囲: 2-3 commit (config.rs + main.rs + experiment.rs 配線)
- 想定 line 範囲: +80 行 / -5 行 (env getter + lab gate)
- **本 plan の項目化**: 項目 249 候補 (Phase 1-3 完遂時)
