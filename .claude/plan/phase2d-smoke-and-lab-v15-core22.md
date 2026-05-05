# Plan: F1 + Phase 2d smoke 検証 + Lab v15 core 22 baseline 実測

> **由来**: 前 handoff (`session_2026_05_04b_handoff.md`、CLAUDE.md 項目 187) で完遂した F2 ContextOverflowGuard (1020→1026 passed、4 commits) の **実機効果計測** と、F1+F2 完成後の **Lab v15 core 22 baseline 実測** を統合する次フェーズ plan。F1 (llama-server `-c 16384` + config.toml `context_length=16384`) は user action として未実行のまま継承。

## Task Type

- [ ] Frontend
- [x] Backend (Validation + Measurement、production code 変更ゼロを基本姿勢)
- [ ] Fullstack

## Multi-Model Synthesis 方針

本 plan は backend pure validation で、`/multi-plan` Phase 2 の analyzer prompt 不在は前 2 セッション (`session_2026_05_04` / `session_2026_05_04b`) の慣例通り。F2 設計議論は Codex SESSION `019def01-d210-7e00-a909-d43525f735fd` で既に深掘り済 (architect 検証 + reviewer audit CONDITIONAL APPROVAL → APPROVED)。本 plan は実機計測 + decision gate を主体とし、Codex resume は **Phase D (Lab v15 core 22) 着手前の sanity check** で必要時のみ 1 spawn する設計。

## Background

### 現状把握 (本セッション開始時点)

| 観点 | 値 | 影響 |
|------|-----|------|
| 直前 commit | `88a040a refactor(subagent,middleware,compaction): F2 audit fixes` | F2 = APPROVED |
| `cargo test --release --lib` | 1026 passed (handoff 05-04b 確定) | 退行なし |
| llama-server プロセス | PID 68103 / `-c 8192` (Thu11PM 起動、約 4 日稼働) | **F1 未実行** |
| config.toml `[model].context_length` | **8192** | F1 未実行と一致 |
| config.toml `[fallback_chain]` セクション | **残存** (MLX primary `:8000` + llama fallback `:8080`、`max_failures=2`) | 前 handoff 05-04 Plan 1 A-side smoke 配備版が継続 |
| MLX-LM `:8000` プロセス | (要確認、本 plan Phase A1 で再確認) | smoke 実行時に warm-up 必要 |
| working tree | `?? fizzbuzz/` (untracked、前 handoff carryover) | smoke 中に再生成される副作用、.gitignore 追加候補 |
| Origin 差分 | `master ahead 14 commits` | push 未実施、user 判断 |

### F2 完成度の確認 (handoff 05-04b より)

- `CompactionConfig::from_n_ctx_budget(Some(n_ctx))` で `n_ctx * 0.7` を budget 派生
- `estimate_tokens` を hybrid `max(chars/3, bytes*0.4, 1)` で日本語混在に保守化
- `CompactionMiddleware::before_step` (LLM 呼出前) に強制 `compact_level3` 追加、超過残存で `MiddlewareSignal::Abort`
- `SubAgentConfig::from_parent` で `n_ctx_budget` 引き継ぎ (Codex MEDIUM finding fix)
- 既存 1020 tests 退行ゼロ (`t_tok` のみ期待値 3→5 仕様改善)

### Phase 2d 検証が未実行の根拠

handoff 05-04b 「次セッション 推奨 TODO」表 ★★★:

> **F1**: llama-server `-c 16384` + config.toml `context_length=16384` (5 min, user)
> **Phase 2d**: smoke 5 task で 400 削減効果測定 (20 min, next session)

→ 本 plan が「next session」に該当。

## Implementation Plan

### Phase A: 事前確認 + F1 (user action) — 約 10 min

#### A1: 事前確認 (Claude 実行、約 2 min)

```bash
# 1. llama-server 現状確認
ps -p 68103 -o pid,etime,cmd | head -3
curl -s http://127.0.0.1:8080/slots | jq '.[0].n_ctx'  # 期待: 8192 (F1 前)

# 2. MLX-LM の起動状況確認 (smoke で primary に使用)
curl -s --max-time 5 http://127.0.0.1:8000/v1/models | jq -r '.data[0].id // "DOWN"'
# DOWN なら Phase B0 で MLX warm-up が必要

# 3. システムメモリ確認 (M2 16GB)
top -l 1 -n 0 | grep -E "PhysMem|MemRegions" | head -2
# free + inactive + purgeable が >4GB なら -c 16384 安全

# 4. config.toml バックアップ (F1 適用前)
cp "/Users/keizo/Library/Application Support/bonsai-agent/config.toml" \
   "/Users/keizo/Library/Application Support/bonsai-agent/config.toml.pre-f1-2026-05-05"
```

#### A2: F1 実行 (user action、約 5 min)

**user 実施 (Claude は手順表示 + 結果確認のみ)**:

```bash
# 1. 既存 llama-server を停止 (PID 68103 を SIGTERM)
kill 68103
# 30s 待ち、SIGKILL fallback (必要時)
sleep 30 && (kill -9 68103 2>/dev/null || true)

# 2. -c 16384 で再起動 (他 flag は維持)
nohup llama-server \
  -m /Users/keizo/Bonsai-demo/models/gguf/8B/Bonsai-8B.gguf \
  --host 127.0.0.1 --port 8080 \
  -c 16384 -ngl 99 --flash-attn on \
  -ctk q8_0 -ctv q8_0 \
  --alias Bonsai-8B \
  > /tmp/bonsai-llama/llama-server-c16384.log 2>&1 &
echo "started PID=$!"

# 3. health 待機 (5 min 上限)
for i in $(seq 1 30); do
  if curl -s --max-time 2 http://127.0.0.1:8080/v1/models | grep -q Bonsai-8B; then
    echo "ready after ${i}*10s"; break
  fi
  sleep 10
done

# 4. n_ctx 確認 (期待: 16384)
curl -s http://127.0.0.1:8080/slots | jq '.[0].n_ctx'

# 5. config.toml 更新 (1 行)
sed -i '' 's/^context_length = 8192$/context_length = 16384/' \
  "/Users/keizo/Library/Application Support/bonsai-agent/config.toml"
grep "^context_length" "/Users/keizo/Library/Application Support/bonsai-agent/config.toml"
# 期待: context_length = 16384
```

**起動失敗時のフォールバック**:

```bash
# `-c 16384` で OOM や VRAM 不足の場合
# A: -c 12288 で再試行
# B: -ctk q4_0 -ctv q4_0 に下げる (品質トレードオフ)
# C: F1 を見送り、F2 単独で検証続行 (項目 187 で any backend graceful 動作は獲得済み)
```

#### A3: F1 完了確認 (Claude 実行、約 1 min)

```bash
# 1. n_ctx 確認
test "$(curl -s http://127.0.0.1:8080/slots | jq -r '.[0].n_ctx')" = "16384" \
  && echo "F1 OK" || echo "F1 FAIL"

# 2. config.toml の context_length 確認
grep "^context_length" "/Users/keizo/Library/Application Support/bonsai-agent/config.toml"
# 期待: context_length = 16384

# 3. bonsai 起動時の budget 計算値ログ確認 (任意)
# AgentConfig.n_ctx_budget = Some(16384) → max_context_tokens = 11468 (16384*0.7)
```

**Decision Gate G-A**: F1 成功 (n_ctx=16384 + config 同期) → Phase B 進行 / 失敗 → フォールバックで `-c 12288` 試行 or F2 単独検証 (Phase B を `-c 8192` のまま実行、F2 効果のみ計測) のいずれか user 判断。

---

### Phase B: Phase 2d smoke 検証 — 約 25 min

#### B0: MLX-LM warm-up 確認 (Claude 実行、必要時)

```bash
# Phase A1 で MLX が DOWN なら warm-up 必要 (20-30 min cold-start 履歴あり、項目 183/184/185)
curl -s --max-time 5 http://127.0.0.1:8000/v1/models | jq -r '.data[0].id // "DOWN"'
# DOWN の場合、user に MLX-LM 起動を依頼 (本 plan のスコープ外、別ターミナル):
#   bash scripts/setup_mlx_ternary.sh
#   または mlx_lm.server --model prism-ml/Ternary-Bonsai-8B-mlx-2bit --port 8000
```

#### B1: smoke 実行 (Claude 実行、約 20 min)

**構成**: 現状の config 維持 (MLX primary `:8000` + llama fallback `:8080` with `max_failures=2`)。前 handoff 05-04 smoke (baseline=0.5876、26x HTTP 400) と **直接比較** 可能。

```bash
mkdir -p /tmp/bonsai-llama
LOG=/tmp/bonsai-llama/lab-v15-smoke-after-f1f2-2026-05-05.log

cd /Users/keizo/bonsai-agent
BONSAI_LAB_SMOKE=1 cargo run --release -- --lab --lab-experiments 5 \
  2>&1 | tee "$LOG"
# 想定 duration: 16-25 min (前 smoke と同等、F2 圧縮分のオーバーヘッド微量)
```

**`run_in_background=true` で実行** (long operation、20 min)。

#### B2: 結果計測 (Claude 実行、約 3 min)

```bash
LOG=/tmp/bonsai-llama/lab-v15-smoke-after-f1f2-2026-05-05.log

echo "=== HTTP 400 件数 (前 smoke=26、target=0、acceptable<5) ==="
grep -c "http status: 400" "$LOG" || echo 0

echo "=== Abort 'context overflow remains' 件数 (target=0) ==="
grep -c "context overflow remains" "$LOG" || echo 0

echo "=== F2 level3 fire 回数 (middleware:context_guard) ==="
grep -c "middleware:context_guard" "$LOG" || echo 0

echo "=== Lab baseline score (前 smoke=0.5876) ==="
grep -E "baseline.*score=" "$LOG" | tail -1

echo "=== ACCEPT 件数 (前 smoke=2/5=40%) ==="
grep -cE "ACCEPT|adopting variant" "$LOG" || echo 0

echo "=== fallback events (前 smoke=27) ==="
grep -cE "fallback.*(used|switched|failed)" "$LOG" || echo 0

echo "=== duration (前 smoke=966.7s) ==="
grep -E "total.*duration|completed in" "$LOG" | tail -3
```

**期待値表**:

| 指標 | 前 smoke (05-04) | Phase 2d 期待 | 許容下限 | Gate-1 判定 |
|------|------------------|---------------|----------|-------------|
| HTTP 400 件数 | 26 | **< 5 (target=0)** | 10 | < 10 で PASS |
| Abort 件数 | n/a (F2 不在) | **0** | 1 | 1 以下で PASS (頻発なら Gate-3 で別 plan) |
| level3 fire 回数 | n/a | **>= 1** (実機発火確認) | 0 | 0 でも production OK (短 prompt のみだった可能性) |
| Lab baseline score | 0.5876 | **>= 0.5376** (-0.05 マージン) | 0.5000 | >= 0.5000 で品質維持 |
| ACCEPT 件数 (5 中) | 2 | 1-3 (variance 内) | 0 | variance 範囲内なら PASS |
| duration | 966.7s | 900-1200s (圧縮分微増) | 1500s | < 1500s で PASS |

#### B3: Decision Gate G-B (Phase 2d 結果評価)

| Gate | 条件 | 判定 |
|------|------|------|
| **G-B1**: F1+F2 効果確認 | 400 件数 < 10 **AND** Abort < 2 | PASS → Phase C 進行 |
| **G-B2**: 品質維持 | Lab score >= 0.5000 | PASS → Phase D 候補 |
| **G-B3**: F2 実機発火 | level3 fire >= 1 (informational) | n/a (PASS 必須ではない) |

**G-B1 FAIL** (400 が 10+ 件残存): F2 estimate_tokens 閾値が緩すぎる疑い → debug log で `level3 applied` を確認、`CONTEXT_GUARD_RATIO_NUM=70 → 60` に下げる別 plan を起票。本 plan の Phase D は中止、handoff へ記録。

**G-B2 FAIL** (Lab score < 0.5000): F2 圧縮で重要 message が消失している疑い → emergency_keep を増やす or compact_level3 の handoff summary 改善を別 plan へ。本 plan の Phase D は中止。

---

### Phase C: 結果分析 + Lab v15 core 22 進行判定 — 約 5 min

#### C1: 結果サマリ作成 (Claude 実行)

Phase B2 結果を表で整理し、Decision Gate 判定。

#### C2: Lab v15 core 22 進行可否判断

| 進行可 | 進行不可 |
|--------|----------|
| G-B1 + G-B2 両 PASS | G-B1 or G-B2 FAIL |
| MLX-LM が応答中 (Phase A1 / B0 で確認) | MLX が DOWN または cold-start 進行中 |
| user が 6h 以上の連続実行に同意 | user が時間制約 / 別 task 優先 |

**user に決定を仰ぐ**: Phase D (Lab v15 core 22) を本セッション内で続行するか、別セッションへ繰越すか。

---

### Phase D (条件付き、user 同意時): Lab v15 core 22 baseline 実測 — 約 6h

#### D1: 事前準備 (Claude 実行、約 5 min)

```bash
# 1. MLX uptime 確認 (項目 173/184 経験: 1-2h 連続稼働で degrade 兆候)
curl -s http://127.0.0.1:8000/v1/models | jq '.'
# (uptime 直接取得不可、Phase A1 / B0 から経過時間を逆算)

# 2. llama-server (fallback) 健全性
curl -s http://127.0.0.1:8080/v1/models | jq '.'

# 3. config 確認 (F1 完了後の状態)
grep -E "context_length|backend|fallback_chain" \
  "/Users/keizo/Library/Application Support/bonsai-agent/config.toml" | head -10

# 4. Codex resume sanity check (optional、1 spawn)
# - SESSION 019def01 (F2 architect) を resume
# - 質問: "F2 ContextOverflowGuard 配備 + n_ctx=16384 で Lab v15 core 22 (k=3, 22 tasks) を MLX primary で実行する際、想定外リスクは?"
# - 回答待ち、必要時 plan 修正
```

#### D2: Lab v15 core 22 実行 (Claude 実行、約 6h、`run_in_background=true`)

```bash
LOG=/tmp/bonsai-llama/lab-v15-core22-2026-05-05.log
mkdir -p /tmp/bonsai-llama

cd /Users/keizo/bonsai-agent
BONSAI_BENCH_TIER=core cargo run --release -- --lab --lab-experiments 14 \
  2>&1 | tee "$LOG" &
echo "started PID=$!"
```

**Background 実行戦略**:
- `run_in_background=true` で投入、`Monitor` で 30 min 毎に health check
- 進行ログ: `tail -f $LOG | grep -E "experiment.*complete|score|fallback"`
- 早期アボート条件 (Claude 監視):
  - MLX が 30 min 以内に hang (curl `:8000/v1/models` 5 連続 timeout) → kill + llama 単独再構成
  - 80 min wallclock で 1 実験完了未達 → kill (1bit / k=3 / 22 tasks の現実値超過)
  - HTTP 400 件数 が 100+ で fallback exhaustion 兆候 → kill

#### D3: 結果計測 (Claude 実行、約 5 min)

```bash
LOG=/tmp/bonsai-llama/lab-v15-core22-2026-05-05.log

echo "=== baseline score (項目 184=0.7976、項目 185=0.8131、項目 182 llama=0.7571) ==="
grep -E "baseline.*score=|core_avg_score" "$LOG" | tail -3

echo "=== pass@k / pass_consec ==="
grep -E "pass@k|pass_consec" "$LOG" | tail -5

echo "=== ACCEPT/REJECT 内訳 ==="
grep -cE "ACCEPT|adopting" "$LOG"
grep -cE "REJECT|rejecting" "$LOG"

echo "=== HTTP 400 件数 (期待: 0 or <10) ==="
grep -c "http status: 400" "$LOG"

echo "=== Abort 件数 (期待: 0) ==="
grep -c "context overflow remains" "$LOG"

echo "=== fallback events ==="
grep -cE "fallback.*(used|switched)" "$LOG"

echo "=== duration ==="
grep -E "total.*duration|completed in" "$LOG" | tail -3
```

#### D4: Decision Gate G-D (Lab v15 core 22 評価)

| Gate | 条件 (zone) | 判定 |
|------|-------------|------|
| **G-D1 (Zone A)**: 卓越 | score >= 0.82 | F1+F2+MLX primary を defaults 化候補 |
| **G-D2 (Zone B)**: 期待通り | 0.78 <= score < 0.82 | 維持、個別検証で defaults 化判断 |
| **G-D3 (Zone C)**: 許容内 | 0.75 <= score < 0.78 | 環境劣化 / 構成微調整必要 |
| **G-D4 (Zone D)**: 退行 | score < 0.75 | 真因分析 (項目 173 MLX 劣化再発? F2 過保守?) |

**Zone 比較対象**:
- 項目 184 MLX core 22 baseline = 0.7976 (1 セッション目)
- 項目 185 MLX core 22 再現 = 0.8131 (2 セッション目、許容再現確認)
- 項目 182 llama core 22 = 0.7571 (MCP detach 後)
- 期待: F1+F2 で MLX primary が安定運用 → Zone A or B

---

### Phase E: 後処理 + handoff 記録 — 約 15 min

#### E1: working tree cleanup (user 判断)

```bash
# Option A: .gitignore に追加 (Lab smoke 副作用を許容)
cd /Users/keizo/bonsai-agent
echo "fizzbuzz/" >> .gitignore
git add .gitignore
git commit -m "chore(gitignore): Lab smoke 副作用 fizzbuzz/ を ignore"

# Option B: 削除 (user 判断)
rm -rf fizzbuzz/
```

#### E2: CLAUDE.md 項目追記 (Claude 実行)

- **項目 188**: F1 + Phase 2d smoke 結果 (400 件数 / Abort / level3 fire / Lab score)
- **項目 189** (Phase D 実行時のみ): Lab v15 core 22 baseline 結果 (Zone 判定 + Phase 5 比較)

#### E3: handoff 記録 (Claude 実行)

`session_2026_05_05_handoff.md` (or 連続 2 セッション目なら `_05b`):

```markdown
---
name: Session Handoff 2026-05-05
description: F1 (llama -c 16384) + Phase 2d smoke 検証完遂、F2 ContextOverflowGuard 実機効果確認 (HTTP 400 = N 件 / Abort = M 件 / score = X)、(条件付き) Lab v15 core 22 baseline = Y / Zone Z
type: project
---

# 完遂サマリー
| Phase | 内容 | 結果 |
| ... | ... | ... |

# 次セッション 推奨 TODO
| 優先 | TODO | 規模 |
| ... | ... | ... |
```

#### E4: MEMORY.md 更新 (Claude 実行)

```markdown
- [Session Handoff 05-05](session_2026_05_05_handoff.md) — F1 + Phase 2d smoke + (Lab v15 core 22) ...
```

---

## Key Files

| File | Operation | Description |
|------|-----------|-------------|
| `~/Library/Application Support/bonsai-agent/config.toml` | Modify (F1) | `context_length = 8192 → 16384` (sed 1 行) |
| (実機) llama-server プロセス PID 68103 | User restart (F1) | `-c 16384` で再起動、PID 変更 |
| `~/Library/Application Support/bonsai-agent/config.toml.pre-f1-2026-05-05` | Write (Phase A1) | F1 適用前 backup |
| `/tmp/bonsai-llama/llama-server-c16384.log` | Write (F1) | 再起動後の llama-server stdout/stderr |
| `/tmp/bonsai-llama/lab-v15-smoke-after-f1f2-2026-05-05.log` | Write (Phase B1) | smoke 出力 |
| `/tmp/bonsai-llama/lab-v15-core22-2026-05-05.log` | Write (Phase D2、条件付き) | Lab core 22 出力 |
| `.gitignore` | Modify (Phase E1、user 判断) | `fizzbuzz/` 追加 |
| `/Users/keizo/bonsai-agent/CLAUDE.md` | Modify (Phase E2) | 項目 188 (+ 189) 追記 |
| `~/.claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_05_handoff.md` | Write (Phase E3) | handoff |
| `~/.claude/projects/-Users-keizo-bonsai-agent/memory/MEMORY.md` | Modify (Phase E4) | 1 行追加 |

**production code (`src/**`)**: **変更ゼロ** (本 plan は validation + measurement のみ)。F2 完成版を実機で計測する立ち位置。

## Risks and Mitigation

| # | Risk | 確率 | Mitigation |
|---|------|------|------------|
| **R1** | F1 再起動で `-c 16384` が VRAM 不足 (M2 16GB) | 中 | フォールバック `-c 12288` → `-ctk q4_0 -ctv q4_0` (品質低下、最終手段) → F1 見送り (F2 単独検証) |
| **R2** | Phase B1 smoke 中に MLX が cold-start で初回 timeout 多発 | 中 | Phase B0 で MLX warm-up 1 リクエスト先行、項目 167 socket timeout 180s で耐性、fallback chain で llama 切替 |
| **R3** | Phase B 結果で 400 が減らない (10+ 件残存) | 低 | F2 estimate_tokens 閾値再調整 (`CONTEXT_GUARD_RATIO_NUM=70 → 60`) を別 plan へ。本 plan は handoff 記録で完了 |
| **R4** | Phase B で Abort 多発 (level3 後も超過残存) | 低 | emergency_keep 増 or compact_level3 handoff summary 改善を別 plan へ。Phase D 中止 |
| **R5** | Phase D 中に MLX が hang/degrade (項目 173 再現) | 中 | 30 min 毎の health check + 早期 abort、fallback chain で llama 単独継続、score は劣化覚悟 |
| **R6** | Phase D wallclock 6h 超過 (k=3, 22 tasks の見積誤差) | 中 | 80 min/実験 で kill、partial 結果で handoff 記録、次セッションへ |
| **R7** | working tree `fizzbuzz/` が smoke で再生成され git status を汚染 | 低 | Phase E1 で .gitignore 追加 (Option A) で恒久解決 |
| **R8** | Codex resume (Phase D1) で SESSION `019def01` 失効 | 低 | resume 失敗で skip、Claude direct で進行。SESSION 失効でも plan 進行は不変 |
| **R9** | F2 level3 圧縮が重要 thinking/handoff を破壊し品質劣化 | 低 | smoke で品質維持確認 (G-B2)、handoff summary は項目 47-58 で品質保証済 |
| **R10** | llama-server `-c 16384` 再起動で port :8080 が一時不在、bonsai が startup fail | 低 | health 5 min 待機ループ、user に `pkill→start→health 待ち→smoke` 順序を厳守させる |
| **R11** | smoke と core 22 の連続実行で MLX cumulative degrade | 中 | Phase C-D 間に MLX restart オプションを user 提示、Phase D D1 で uptime 確認 |
| **R12** | `[fallback_chain]` 残存で smoke 純粋 F2 効果が判別不能 (MLX 出力品質と F2 効果が混在) | 中 | 直接比較対象は前 handoff 05-04 同構成 smoke (baseline=0.5876、26x 400)、構成変更せず差分のみで F2 効果を判別 |

## Decision Gates 一覧

| Gate | Phase | 条件 | True 時 | False 時 |
|------|-------|------|---------|----------|
| **G-A** | A3 | F1 成功 (n_ctx=16384 + config 同期) | Phase B 進行 | フォールバック -c 12288 / F2 単独検証 (Phase B `-c 8192` で実行) / F1 見送り |
| **G-B1** | B3 | 400 件数 < 10 AND Abort < 2 | G-B2 評価 | F2 閾値再調整別 plan、Phase D 中止 |
| **G-B2** | B3 | Lab score >= 0.5000 | Phase C 進行 | F2 emergency_keep 改善別 plan、Phase D 中止 |
| **G-B3** | B3 | level3 fire >= 1 | Informational PASS | informational FAIL (production には影響なし) |
| **G-C** | C2 | user 同意 (Phase D 6h 続行) | Phase D 進行 | 本セッション完了、Phase D は別セッションへ |
| **G-D1-4** | D4 | score Zone (>=0.82 / 0.78-0.82 / 0.75-0.78 / <0.75) | Zone 別判定 (handoff 記録) | n/a (zone 判定で完結) |

## Test Strategy

| 区分 | 検証 | 方法 |
|------|------|------|
| **Production code** | 1026 passed 退行ゼロ | `cargo test --release --lib` (Phase A1 で 1 度実行、本 plan で production code 変更なし) |
| **F2 実機発火** | level3 fire 観測 | smoke log の `middleware:context_guard` grep |
| **F2 graceful Abort** | Abort 動作確認 | smoke log の `context overflow remains` grep (期待: 0) |
| **F2 効果** | 400 削減 | smoke 26 → < 10 で PASS |
| **品質維持** | Lab score | smoke 0.5876 → >= 0.5376 で PASS |
| **Lab v15 core 22** (条件付き) | Zone 判定 | 項目 184/185/182 比較 |

## YAGNI / 見送り

| 案 | 判定 | 理由 |
|----|------|------|
| **llama 単独 smoke (clean F2 効果計測)** | 本 plan 対象外 | 直接比較対象 (前 handoff 05-04 smoke) と構成揃えるため `[fallback_chain]` 維持。pure F2 効果は別 plan で測定 (必要時) |
| **Lab v15 extended tier (Plan 4)** | 本 plan 対象外 | core tier 優先、extended は項目 173 で 0.341 baseline 確定済、別 plan で改善検討 |
| **Gemini reviewer wrapper exit 1 調査** | 本 plan 対象外 | optional、Codex single authority で audit 完了済 (handoff 05-04b) |
| **CachedBackend trait 動的化 RFC** | 本 plan 対象外 | 観測価値 < 設計コスト で見送り判定済 (handoff 05-04) |
| **F3 (400 → 自動 retry)** | 不要 | F2 で 400 自体が発生しないため (handoff 05-04b plan §YAGNI) |
| **F4 (1bit retry 閾値調整)** | 不要 | 同上 |
| **`accept_threshold` field 導入** | 別 plan | smoke 補正の真の false-accept 防止、本 plan の YAGNI fence 外 (handoff 05-04 で確定) |
| **`AuditAction::ContextGuard` SQLite log** | 見送り | smoke log grep で代替十分 (handoff 05-04b §YAGNI) |
| **estimate_tokens を BPE tokenizer (tiktoken) 化** | 見送り | 70% ratio + hybrid estimator で十分 (handoff 05-04b §YAGNI) |
| **MLX restart between Phase B/D** | optional | Phase D D1 で uptime 確認後に user 判断、本 plan で必須化しない |

## SESSION_ID (for /ccg:execute or sanity check)

- **CODEX_SESSION**: `019def01-d210-7e00-a909-d43525f735fd` (F2 architect、Phase D1 sanity check で resume 候補、optional)
- **GEMINI_SESSION**: n/a (analyzer prompt 不在 + 前 handoff 05-04b で wrapper exit 1 観測、本 plan では skip)

## 完了基準

### Phase 2d (B + C) 完了 (★★★ 必達)

1. F1 完了: `curl :8080/slots | jq .[0].n_ctx` = 16384
2. config.toml `context_length = 16384`
3. smoke 実行完了: `lab-v15-smoke-after-f1f2-2026-05-05.log` 取得
4. G-B1 PASS: `http status: 400` 件数 < 10
5. G-B2 PASS: Lab score >= 0.5000
6. handoff 記録 (`session_2026_05_05_handoff.md`) + MEMORY.md 1 行追加
7. CLAUDE.md 項目 188 追記

### Phase D 完了 (条件付き、★★)

8. Lab v15 core 22 実行完了 (or 早期 abort で partial 結果)
9. Zone 判定 (G-D1〜G-D4) を handoff へ記録
10. CLAUDE.md 項目 189 追記

### 全フェーズ完了

11. working tree clean (Phase E1 .gitignore 追加 or `fizzbuzz/` 削除)
12. cargo test --release --lib (1026 passed 維持確認、production code 変更なしのため事前自明)

## 想定 commit (Phase 完了時)

| Phase | commit | 内容 |
|-------|--------|------|
| E1 | `chore(gitignore): Lab smoke 副作用 fizzbuzz/ を ignore` | (user 判断時) 1 file |
| E2 | `docs(claude.md): 項目 188 — F1 + Phase 2d smoke 結果` | 1 file |
| E2 (条件付き) | `docs(claude.md): 項目 189 — Lab v15 core 22 baseline (Zone X)` | 1 file |

production code commit はゼロ。本 plan は validation + measurement の純粋 plan。

## 推定所要時間 (Claude + user)

| Phase | 所要 | 実行者 |
|-------|------|--------|
| A1 (事前確認) | 2 min | Claude |
| A2 (F1) | 5 min | user |
| A3 (F1 確認) | 1 min | Claude |
| B0 (MLX warm-up 確認) | 1 min | Claude (DOWN なら +30 min user warm-up) |
| B1 (smoke) | 20 min | Claude (background) |
| B2-B3 (計測 + Gate) | 5 min | Claude |
| C (分析 + 判定) | 5 min | Claude + user |
| **小計 (Phase 2d まで)** | **約 40 min** | (MLX warm-up 不要時) |
| D1 (準備) | 5 min | Claude |
| D2 (Lab core 22) | 6h | Claude (background) |
| D3 (計測) | 5 min | Claude |
| D4 (Gate) | 5 min | Claude |
| **小計 (Phase D)** | **約 6h 15min** | |
| E1-E4 (後処理 + handoff) | 15 min | Claude + user |
| **総計 (Phase D 含む)** | **約 7h** | |
| **総計 (Phase D 別セッション繰越)** | **約 1h** | (本 plan で Phase 2d まで完遂) |
