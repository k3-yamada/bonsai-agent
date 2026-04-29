---
plan_name: phase5-baseline-execution
created: 2026-04-29
supersedes_phase5_of: .omc/plans/optimal-plan-2026-04-29.md (Phase 5 のみ刷新)
based_on:
  - CLAUDE.md 項目 172（BONSAI_BENCH_TIER 実装、992 tests）
  - .claude/plan/lab-v14-result.md（baseline=0.5192 の確定事実）
  - Codex audit SESSION_ID 019dd7c0-67e0-7121-aa64-ca77b14532fa（前 plan 6 件指摘）
  - 実 TSV `~/Library/Application Support/bonsai-agent/experiments.tsv` の baseline 履歴
status: ready-to-execute（ユーザー承認後に /ccg:execute）
estimated_effort: 2.0–2.5h（preflight 10 min + core 30–40 min + extended 30–40 min + 解析 10 min）
risk_level: LOW
session_links:
  CODEX_SESSION: 019dd7c0-67e0-7121-aa64-ca77b14532fa
  GEMINI_SESSION: skipped（wrapper silent fail、handoff 注意事項 5）
---

# Phase 5 — 実測 Baseline 計測（Tier Core / Extended、変異ループなし）

## 1. 目的

Lab v14 baseline = **0.5192**（v9/v10 baseline 0.79–0.81 から **−35%** 退行）の主因を 1 セッションで確定する。

| 仮説 | 内容 | 棄却条件 | 確認条件 |
|------|------|---------|---------|
| **X** | Bench 拡張 22→40 が主因 | core baseline < 0.65 | core ≥ 0.70 かつ extended < core |
| **Y** | MLX 環境劣化（SSE timeout fallback、サーバー不安定）が主因 | core baseline ≥ 0.70 | core < 0.65 かつ extended < 0.30 |

両者が中間値（core 0.65–0.70）の場合は **再計測 1 回**で MLX instability か bench expansion 寄与かを切り分ける。

## 2. 実装前提（既マージ済 = 項目 172、no code change in Phase 5）

- `BenchmarkSuite::core_tasks()` = 22、`extended_tasks()` = 18
- `BenchmarkTask.tier: TaskTier { Core, Extended }`
- `BONSAI_BENCH_TIER=core|extended` env（case-insensitive、未対応値で warn fallback）
- `BONSAI_LAB_SMOKE=1` は独立フラグ（**Phase 5 では明示的に `=0` で leak 防止**）
- `MultiRunBenchmarkResult.{core_avg_score, extended_avg_score}: Option<f64>` は **TSV/JSON には出力されない**（in-memory のみ）

## 3. Codex audit が暴いた前 plan の致命的 6 flaw

| # | 前 plan の主張 | 実際 |
|---|---------------|------|
| 1 | `cargo run -- --lab` で `max_experiments=0` を config 経由で渡す | `cli.lab_experiments` (default=10) が CLI 引数で **config を上書きする**。`--lab --lab-experiments 0` が正解。`--lab 0` は invalid syntax |
| 2 | `core_avg_score`/`extended_avg_score` を TSV/JSON で観測 | **これらは TSV にもログにも出ない**（in-memory only）。tier 単独実行時は `composite_score()` がそのまま該当 tier 平均と等価 |
| 3 | TSV から baseline 抽出 | `--lab-experiments 0` だと **TSV 行は 0 件**（experiment 行のみ append される設計）。stdout の `[lab] ベースライン:` 行が唯一の取得源 |
| 4 | `pkill -f "cargo run.*--lab"` でガード | wrong process kill / 実行中タスクのデータ損失リスク。**`SIGINT` (ctrlc_handler 経由)** で graceful cancel |
| 5 | config.toml `max_experiments=0` 一時編集 | CLI 引数で十分、revert 忘れリスク不要 |
| 6 | Lab v14 TSV から事前推定不可 | TSV の最近 20 件 baseline は `[0.5192, 0.5250, 0.7750, 0.8043, 0.8063, 0.8080, 0.8099, 0.8110]`。**0.7750 は同じ 40 タスク構成で観測済**（v13 期）→ 仮説 Y（環境劣化）寄りの prior |

## 4. Pre-bias signal（実 TSV から抽出済、Phase 5 結果解釈の参照点）

```
最近 20 件 baseline 履歴:
  v9/v10 (22 タスク): 0.8043, 0.8063, 0.8080, 0.8099, 0.8110
  v13   (40 タスク): 0.7750
  v14   (40 タスク): 0.5192, 0.5250  ← 退行
```

→ **同じ 40 タスク構成で 0.7750 は出ている**。Bench 拡張だけで 0.5192 まで沈むわけではない。Y（MLX/環境）寄与が混入している可能性が高い。

→ Phase 5 で core ≥ 0.78 が出れば「v14 baseline 0.5192 は MLX 不調が主因」確定。

## 5. 実行手順（コピペ可、シェル全コマンド済）

### 5.1 Preflight（≤10 min）

```bash
cd /Users/keizo/bonsai-agent && \
git status --short && \
echo "---" && \
cargo test --lib 2>&1 | tail -3 && \
echo "---" && \
cargo build --release 2>&1 | tail -3 && \
echo "--- MLX health ---" && \
curl -fsS -m 5 http://localhost:8000/v1/models | head -3
```

期待: `working tree clean` / `992 passed` / `Finished` / JSON が返る。

### 5.2 Tier 切替 smoke 検証（≤2 min、変異 0 で開始～baseline 完了直前まで早期 abort）

実装の env 切替が動くことだけを確認。本番 baseline ではない。

```bash
cd /Users/keizo/bonsai-agent && \
BONSAI_LAB_SMOKE=1 BONSAI_BENCH_TIER=core \
  cargo run --release -- --lab --lab-experiments 0 2>&1 | \
  tee /tmp/phase5-preflight-tier-2026-04-29.log | \
  grep -E "BONSAI_BENCH_TIER|smoke_tasks|core_tasks|extended_tasks" | head -5
```

**期待ログ行（どちらか一方）**:
- `BONSAI_LAB_SMOKE=1 → smoke_tasks() 使用（5 タスク）` （SMOKE が優先される設計確認）
- または `BONSAI_BENCH_TIER=core → core_tasks() 使用（22 タスク）`

`SMOKE=1` は `BONSAI_BENCH_TIER` より優先される（`experiment.rs:672` の if 構造）ので、smoke 経由で env 配線の生死だけ確認。本番では `SMOKE=0` 明示。

### 5.3 Core baseline（22 タスク × k=3 = 66 ラン、~30–40 min、SIGINT ガード付き）

```bash
cd /Users/keizo/bonsai-agent && \
rm -f /tmp/phase5-core-2026-04-29.log && \
BONSAI_LAB_SMOKE=0 BONSAI_BENCH_TIER=core \
  cargo run --release -- --lab --lab-experiments 0 \
  > /tmp/phase5-core-2026-04-29.log 2>&1 &
pid=$! ; \
echo "[main] core pid=$pid" ; \
( sleep 5400 ; \
  if kill -0 "$pid" 2>/dev/null ; then \
    echo "[guard] timeout 5400s, sending SIGINT to $pid" >> /tmp/phase5-core-2026-04-29.log ; \
    kill -INT "$pid" ; sleep 90 ; \
    kill -TERM "$pid" 2>/dev/null ; \
    kill -KILL "$pid" 2>/dev/null ; \
  fi ) &
guardpid=$! ; \
wait "$pid" ; status=$? ; \
kill "$guardpid" 2>/dev/null ; \
echo "[done] core exit=$status"
```

**進捗の途中観測**は別シェルで `tail -f /tmp/phase5-core-2026-04-29.log`。

### 5.4 Extended baseline（18 タスク × k=3 = 54 ラン、~30–40 min）

Core 完了後に MLX が劣化していないか軽く確認 → extended 起動。

```bash
curl -fsS -m 5 http://localhost:8000/v1/models | head -1 && \
cd /Users/keizo/bonsai-agent && \
rm -f /tmp/phase5-extended-2026-04-29.log && \
BONSAI_LAB_SMOKE=0 BONSAI_BENCH_TIER=extended \
  cargo run --release -- --lab --lab-experiments 0 \
  > /tmp/phase5-extended-2026-04-29.log 2>&1 &
pid=$! ; \
echo "[main] extended pid=$pid" ; \
( sleep 5400 ; \
  if kill -0 "$pid" 2>/dev/null ; then \
    echo "[guard] timeout 5400s, sending SIGINT to $pid" >> /tmp/phase5-extended-2026-04-29.log ; \
    kill -INT "$pid" ; sleep 90 ; \
    kill -TERM "$pid" 2>/dev/null ; \
    kill -KILL "$pid" 2>/dev/null ; \
  fi ) &
guardpid=$! ; \
wait "$pid" ; status=$? ; \
kill "$guardpid" 2>/dev/null ; \
echo "[done] extended exit=$status"
```

### 5.5 結果抽出（≤5 min）

```bash
echo "=== core ===" ; \
grep -E "BONSAI_BENCH_TIER|ベースライン|score=|pass.*=|tier" /tmp/phase5-core-2026-04-29.log | head -30 ; \
echo "=== extended ===" ; \
grep -E "BONSAI_BENCH_TIER|ベースライン|score=|pass.*=|tier" /tmp/phase5-extended-2026-04-29.log | head -30
```

期待行（experiment.rs:744-750 の `log_event` 出力）:
- core 側: `BONSAI_BENCH_TIER=core → core_tasks() 使用（22 タスク）`
- core 側: `[lab] ベースライン: score=X.XXXX, pass@k=X.XXXX, pass_consec=X.XXXX, ...`
- extended 側: `BONSAI_BENCH_TIER=extended → extended_tasks() 使用（18 タスク）`
- extended 側: `[lab] ベースライン: ...`

各 `score=` 値が **当該 tier の `composite_score()`** であり、tier 単独実行時は `core_avg_score`（または `extended_avg_score`）と等価。

## 6. 結果記録ファイル（実行者が埋める）

`.claude/plan/baseline-tier-2026-04-29.md` を新規作成し以下フォーマットで記録:

```markdown
# Baseline Tier 2026-04-29 結果

## 実行情報
- 日時: 2026-04-29 HH:MM JST
- Backend: mlx-lm
- Model: ternary-bonsai-8b (prism-ml/Ternary-Bonsai-8B-mlx-2bit)
- SSE timeout: 180s
- k: 3
- 変異ループ: 無効（--lab-experiments 0）

## 結果サマリー

| Tier | Tasks | Runs | score | pass@k | pass_consec | duration | log path | exit |
|------|-------|------|-------|--------|-------------|----------|----------|------|
| core | 22 | 66 | ?.???? | ?.???? | ?.???? | ?s | /tmp/phase5-core-2026-04-29.log | 0 |
| extended | 18 | 54 | ?.???? | ?.???? | ?.???? | ?s | /tmp/phase5-extended-2026-04-29.log | 0 |

## 仮説判定

- 仮説 X (Bench 拡張): [confirm/reject/ambiguous]
- 仮説 Y (MLX 環境劣化): [confirm/reject/ambiguous]
- 主因確定: [X / Y / both / inconclusive]

## 次の意思決定（自動派生）

[判定マトリクス §7 から導出した次セッション action を 1 行で]
```

## 7. 判定マトリクス（自動意思決定）

| core | extended | 判定 | 次セッション |
|------|----------|------|---------|
| ≥ 0.78 | ≥ 0.50 | **X 確定**: Phase C タスク自体が難しすぎる、Bench 拡張が主因 | P3 Lab v15（LLM 提案変異）or task weighting |
| ≥ 0.78 | < 0.30 | X 強確定 + Phase C 過剰 | Phase C タスク見直しを最優先 |
| 0.65–0.78 | any | **Ambiguous Zone** | core 1 回再計測 → MLX restart 後に判定 |
| < 0.65 | any | **Y 確定**: MLX 環境劣化が主因 | P2 FallbackChain で llama-server 試験 |
| any | invalid (no `[lab] ベースライン:` 行) | 無効ラン | 該当 tier を再計測 |

`pre-bias signal §4` に従い、**core ≥ 0.78 の事前確率はそれなりにある**（v13 で 0.7750 既往）。

## 8. リスク → 緩和（Codex 指摘反映済）

| Risk | Mitigation |
|------|-----------|
| `--lab 0` syntax invalid | `--lab --lab-experiments 0` 固定 |
| `BONSAI_LAB_SMOKE=1` がシェル環境から leak | `BONSAI_LAB_SMOKE=0` を必ず明示 |
| `BONSAI_BENCH_TIER` typo で silent fallback | log の `core_tasks() 使用（22 タスク）` 行を必ず確認、なければ無効ラン扱い |
| `pkill -f` で意図せぬ別プロセス kill | **`SIGINT` only**（ctrlc_handler 経由 graceful）→ 90s 待ち → `SIGTERM` → `SIGKILL` の段階的フォールバック |
| Guard 5400s 経過時の partial result | partial は無効扱い、final `[lab] ベースライン:` 行があるかが唯一の有効性判定 |
| Core 完走後に MLX が劣化 | extended 起動前に `curl /v1/models` 再 health check |
| TSV からベースライン抽出 | **不可**（baseline-only 実行では TSV 0 行）。stdout/log のみが source-of-truth |
| 結果ばらつきによる Y 誤検出 | core が 0.65–0.78 zone なら 1 回再計測（§7 Ambiguous Zone） |
| Cargo cache 影響なし確認 | `cargo build --release` を preflight で実施し、本番 cargo run --release は再ビルドなし即起動 |

## 9. Definition of Done

| 項目 | 判定基準 |
|------|---------|
| Preflight | working tree clean / 992 passed / build success / MLX up |
| Tier env 動作 | preflight smoke で `BONSAI_LAB_SMOKE=1 → smoke_tasks()` または `core_tasks() 使用` を確認 |
| Core baseline 取得 | `/tmp/phase5-core-2026-04-29.log` に `BONSAI_BENCH_TIER=core → core_tasks() 使用（22 タスク）` と最終 `[lab] ベースライン:` の両行 |
| Extended baseline 取得 | `/tmp/phase5-extended-2026-04-29.log` に同等 2 行 |
| 結果ファイル | `.claude/plan/baseline-tier-2026-04-29.md` に §6 フォーマットで記録 |
| 仮説判定 | §7 マトリクスから 1 行 next-action 抽出 |
| 巻き戻し対象 | `agent_loop.rs` / `error_recovery.rs` / `tool_exec.rs` / `benchmark.rs` 差分 0（Phase 5 はコード変更なし） |
| commits | Phase 5 はコード変更なしのため commit 0（結果記録のみ memory に追加可） |

## 10. なぜこの順序か（1 行サマリ）

**v14 baseline 0.5192 と v13 baseline 0.7750 が同じ 40 タスク構成で観測されている事実**は、Bench 拡張だけでは退行を説明できないことを意味する。tier 別 baseline で「core 22 のみで 0.78 復帰するか」を 30 分で判定すれば、X / Y のどちらが主因かが定量的に確定する — それ以外の安価な実験はない。

## 11. 次セッションへのバトン

判定確定後の action（§7 マトリクス）に従い、以下のいずれかを起動:
- **Y 確定 → P2 FallbackChain**: `.claude/plan/fallback-chain-impl.md` 既存、llama-server バイナリと GGUF 入手から
- **X 確定 → P3 Lab v15 or task redesign**: 既存 LLM 提案変異の設計を `.claude/plan/post-lab-v13-roadmap.md` から再活用
- **Ambiguous → core 再計測** 1 回追加、MLX restart 後

## SESSION_ID（/ccg:execute resume 用）

- CODEX_SESSION: `019dd7c0-67e0-7121-aa64-ca77b14532fa`（前 plan 6 件指摘の architect critique セッション）
- GEMINI_SESSION: skipped（wrapper silent fail、frontend 観点不要）
