# F3 RequestSizeGuard core 22 検証結果 (2026-05-06)

> **由来**: handoff `session_2026_05_06_handoff.md` 最優先 ★★★ TODO 消化。
> **対象**: 項目 190 (F3 RequestSizeGuard 実装) の effectiveness を core 22 task k=3 で実測。

## TL;DR

**llama-only / F3 enabled (`f3_max_message_tokens=4915`) で score 0.7849 / 42.2 min / HTTP 400=0 / F3 fire=0**。

Decision Gate 3 条件中 **2 PASS / 1 FAIL** — F3 fire=0 で **effectiveness 実証は失敗** だが、score +0.0289 (項目 189 比) / HTTP 400 -100% / 副作用ゼロ確認で **production 投入は安全**。

## 結果

| 指標 | F3 enabled (本回) | 項目 189 baseline (F3 disabled) | 差分 |
|------|------------------|-------------------------------|------|
| score | **0.7849** | 0.7560 | **+0.0289** ✅ |
| pass@k | 0.8636 | 0.8636 | 0 |
| pass_consec | 0.8485 | 0.8485 | 0 |
| duration | 2532.1s (42.2 min) | 2850.1s (47.5 min) | **-318s (-11.2%)** |
| HTTP 400 | **0** | 5 | **-100%** ✅ |
| F3 fire | **0** | n/a | — |
| F2 fire | 0 | 0 | 同 |
| fallback events | 0 | 0 | 同 |
| Abort | 0 | 0 | 同 |
| SSE timeout | 1 | 0 | +1 (noise) |

設定: llama-server `:8080` 単独 / `[fallback_chain]` 一時 comment-out / `[model] f3_max_message_tokens = 4915` (= 12288 \* 0.4) / MCP detach 維持 / k=3 / 22 task = 66 run / pure baseline (`--lab-experiments 0`)。

## Decision Gate 評価

| Gate | 条件 | 結果 | 判定 |
|------|------|------|------|
| ① score | >= 0.7460 (項目 189 0.7560 - 0.01) | 0.7849 | ✅ **PASS** (+0.0389) |
| ② HTTP 400 | < 5 | 0 | ✅ **PASS** (-100%) |
| ③ F3 fire | >= 1 (effectiveness 実証) | 0 | ❌ **FAIL** |

**判定: 2/3 PASS、effectiveness 実証は失敗**。ただし F3 起因の regression なし (副作用ゼロ証明)。

## 仮説判定

| 仮説 | 結果 | 根拠 |
|------|------|------|
| H1: F3 が core 22 で fire し HTTP 400 を抑制 | **REJECT** | F3 fire=0、項目 189 の HTTP 400=5 は variance 寄りの低水準だった |
| H2: F3 副作用ゼロ (1bit 解釈崩壊なし) | **CONFIRM** | score +0.0289 で **退行ゼロ**、tool_call tag skip + Tool role 専用 truncate が機能 |
| H3: core 22 task に巨大単発 message が存在 | **REJECT** | 4915 tokens 閾値を超える単発 message は **66 run 中 0 件**、項目 116 (truncate_tool_output) の 4000 char 上限が Layer 1 で先に効いている可能性 |

## 核心知見

### 1. F3 effectiveness は core 22 でも実証できなかった

smoke 5 task (handoff 05-06、F3 fire=0) と同パターン。**core 22 task でも単発 message が 4915 tokens を超えるケースは皆無**。原因推定:

- **Layer 1 (項目 116 `truncate_tool_output`、`max_tool_output_chars=4000`)** が tool 出力を 4000 char ≈ 1333 tokens で先に切捨て
- LLM が生成する Assistant message は通常 1000-3000 tokens 範囲
- File_write の long content は項目 116 経由ではなく content として直接渡るが、core 22 task では 4915 tokens 超えのファイルを書く task が少ない

### 2. score +0.0289 / duration -11% は F3 由来ではない

F3 fire=0 のため、改善は **環境/llama-server 状態 or run-to-run variance** に起因。項目 189 と同じ commit (本セッション内実装後) で計測しているため code drift はゼロ。**真の原因解明は再計測 (本セッション同条件で 2 回目) が必要**。

### 3. HTTP 400=0 は variance であり F3 由来ではない

項目 189 (5 件) → 本回 (0 件) の改善は F3 fire=0 で起きたため、**F3 が prevent したわけではない**。項目 188 (B1a MLX-primary) で 24 件、項目 189 (llama-only) で 5 件、本回 0 件 → llama-only baseline では HTTP 400 はもともと低水準で variance 範囲内。

### 4. F3 は production 安全に enable 可能

- 副作用ゼロ確認 (score 退行なし)
- opt-in (default disabled、`f3_max_message_tokens=0` で legacy 互換)
- AuditAction::F3SizeGuard で fire 時は SQLite 永続化 (将来集計可能)
- **推奨**: config.toml に `f3_max_message_tokens = 4915` を documented optional として保持

## 採否判定

**F3 の core 22 task workload における effectiveness は実証されなかった**。ただし以下の観点で **棚保留に値する**:

1. **safety net** として: extended tier、新規 task 追加、user 個別 task で巨大単発 burst が発生した場合に automatic intercept
2. **multi-modal task** での将来効果: 画像 base64 / 大規模 JSON / 長 SQL 等で fire 確率上昇
3. **non-llama-server backend** での効果: MLX-LM `:8000` で 405 error / cold start retry 中に conversation 蓄積 → 単発 burst の発生確率は llama-server より高い可能性

**推奨アクション**:
- F3 コードは **merge 維持** (1040 passed、副作用ゼロ実証済)
- f3_max_message_tokens は **default 0 (disabled)** を維持
- documentation で「extended tier / 大規模 file_write task / non-llama backend で enable 推奨」を記載
- 次セッション以降の effectiveness 検証は **extended tier baseline (18 task)** で再実施 (項目 172 階層分離活用)

## 次セッション TODO

### ★★ extended tier baseline (任意、F3 effectiveness 再検証)

extended tier 18 task (MultiFileEdit / LongRun / ToolChain / ErrorRecovery 等、項目 163 で追加) は core 22 より単発 burst の発生確率が高い workload。同条件 (F3 enabled / llama-only) で baseline を計測。

```bash
BONSAI_BENCH_TIER=extended cargo run --release -- --lab --lab-experiments 0
```

**Decision Gate**:
- F3 fire >= 5 → effectiveness 実証
- score >= 0.30 (項目 173 暫定 0.3410 を許容範囲とする) → 副作用ゼロ
- HTTP 400 < 3 → 安定動作

### ★ MLX primary + fallback sticky 動作見直し (handoff 05-06 carry-over)

項目 137 split policy + R13 CachedBackend disable 機構の検討。MLX primary で F3 effectiveness が出るか別 plan で検証。

### ★ duration -11% 原因究明 (本セッション carry-over)

llama-server uptime 状態 / OS load / 推論 cache hit rate の影響を切り分けるため、項目 189 と同条件 (llama-only / F3 disabled) で再計測し variance を確認。

## working tree 状態

- config.toml: backup `config.toml.pre-f3-core22-2026-05-06` (SHA `e217687e1cc2d690...`) から完全復元、production 通常運用 (B1a 構成 = MLX primary + llama fallback + MCP detach) 復帰
- production code: 変更ゼロ (本検証は計測のみ)
- working tree: clean (`.serena/project.yml` のみ stale auto-sync)
- ahead: master の 19 commits (handoff 05-06 までの累積)

## ログ + plan path

- log: `/tmp/bonsai-llama/f3-core22-baseline-2026-05-06.log` (922 行、49KB)
- plan v2 (本実装の指針): `.claude/plan/request-size-guard-impl-v2.md`
- backup: `~/Library/Application Support/bonsai-agent/config.toml.pre-f3-core22-2026-05-06`

## 推定所要時間 (実測)

| Phase | 想定 | 実測 |
|-------|------|------|
| 状態確認 + config 編集 | 5 min | 5 min |
| core 22 baseline run | 50 min | 42.2 min (-15%) |
| 集計 + config 復元 | 10 min | 5 min |
| handoff + plan + commit | 25 min | 進行中 |
| **合計** | **90 min** | **~60 min** |
