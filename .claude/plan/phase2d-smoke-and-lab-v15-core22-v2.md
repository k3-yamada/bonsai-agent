# Plan v2: F1 + Phase 2d smoke 検証 (MLX-primary + llama-only) + Lab v15 core 22 baseline 実測

> **由来**: v1 (`phase2d-smoke-and-lab-v15-core22.md`) を `/ccg` review (Codex SESSION + Gemini SESSION 並列、本セッション 2026-05-04) で監査し、Codex HIGH 4 件 + MEDIUM 4 件 + LOW 2 件 + Gemini polish 2 件を反映した修正版。v1 は archival として保持。
> **修正核心**: ① Phase D2 を `--lab-experiments 0` に変更 (pure baseline 化、技術バグ修正) / ② score grep を日本語ログ `[lab] ベースライン: score=` に対応 (Gate fail-open 防止) / ③ Phase B に `B1b llama-only smoke` を追加 (F2 isolation 専用 lane) / ④ Gate 閾値を厳格化 (Abort/score/Zone) / ⑤ Phase D 監視を progress marker ベースに / ⑥ bash hardening / ⑦ R13 CachedBackend contamination 追加。

## Task Type

- [ ] Frontend
- [x] Backend (Validation + Measurement、production code 変更ゼロ)
- [ ] Fullstack

## Multi-Model Synthesis 方針

v1 → v2 の修正は本セッション内の `/ccg` (Codex + Gemini 並列 review) 出力を Claude が synthesize したもの (CCG protocol §4 Synthesis 準拠)。v2 適用後の review は本 plan 範囲外、`/ccg:execute` 着手前に user が最終確認。F2 設計の architect SESSION `019def01` は Phase D1 sanity check 用に温存。

## Background

### 現状把握 (本セッション開始時点、v1 と共通)

| 観点 | 値 | 影響 |
|------|-----|------|
| 直前 commit | `88a040a refactor(subagent,middleware,compaction): F2 audit fixes` | F2 = APPROVED |
| `cargo test --release --lib` | 1026 passed | 退行なし |
| llama-server | PID 68103 / `-c 8192` (Thu11PM 起動) | **F1 未実行** |
| config.toml `[model].context_length` | **8192** | F1 未実行 |
| config.toml `[fallback_chain]` | 残存 (MLX `:8000` + llama `:8080`、`max_failures=2`) | sticky: 2 連続失敗後は llama 固定 (model_router.rs:711) |
| MLX-LM `:8000` | (Phase A1 で再確認) | smoke 実行時 warm-up 必要 |
| working tree | `?? fizzbuzz/` (untracked) | F1 直前に user が cleanup 必要 (G1) |
| Origin 差分 | `master ahead 14 commits` | push 未実施、user 判断 |

### v1 → v2 主要変更点サマリ

| # | 種別 | v1 | v2 |
|---|------|----|----|
| C1 | HIGH bug | Phase D2 `--lab-experiments 14` (baseline + 14 mutations) | `--lab-experiments 0` (pure baseline)、14 muts は Phase D2b に分離 |
| C2 | HIGH bug | `grep -E "baseline.*score="` (英語、空 parse) | `grep -E "\[lab\] ベースライン: score=\|score="` + 空 parse で gate fail-closed |
| A1 | HIGH design | MLX-primary smoke 単独 (F2 と backend 効果が混在) | B1a (MLX-primary、existing) + **B1b (llama-only、F2 isolation)** に分離 |
| C3 | HIGH gate | G-B1 `Abort < 2` | G-B1 `Abort ≤ 1`、Phase D 進行は `Abort == 0` |
| C4 | MEDIUM gate | G-B2 `score >= 0.5000` | G-B2 `score >= 0.5376` (前 smoke -0.05 厳格)、0.5000-0.5376 は investigate-only |
| C5 | MEDIUM zone | Zone A `>= 0.82` | A `>= 0.81 (2 consecutive clean)` / B `0.78-0.81` / C `0.75-0.78` / D `< 0.75` |
| A2/A3/C6 | MEDIUM ops | `/v1/models` health のみ | progress marker 監視 (45-60 min なし → abort) + 100 → 20 早期 kill + MLX uptime>2h で restart mandatory |
| C7 | LOW risk | (なし) | R13 CachedBackend contamination 追加 |
| G1 | polish | (なし) | Phase A2 冒頭 git status/stash sanity check |
| G2 | polish | G-B2 FAIL "別 plan 起票" のみ | Safe Exit 手順 (config restore + partial handoff) 明示 |
| #9 | LOW | `mlx_lm.server` | `mlx-openai-server launch` (script 整合) + `/v1/completions` warm-up |

## Implementation Plan

### Phase A: 事前確認 + F1 (user action) — 約 12 min

#### A1: 事前確認 (Claude 実行、約 2 min)

```bash
# 1. llama-server 現状確認
ps -p 68103 -o pid,etime,cmd | head -3 || echo "[PID 68103 already gone]"
curl -fsS http://127.0.0.1:8080/slots | jq '.[0].n_ctx' || echo "[server unreachable]"

# 2. MLX-LM 起動確認 (smoke で primary)
curl -fsS --max-time 5 http://127.0.0.1:8000/v1/models | jq -r '.data[0].id // "DOWN"'

# 3. システムメモリ (M2 16GB、free + inactive + purgeable > 4GB が望ましい)
top -l 1 -n 0 | grep -E "PhysMem" | head -1

# 4. config.toml バックアップ
cp "/Users/keizo/Library/Application Support/bonsai-agent/config.toml" \
   "/Users/keizo/Library/Application Support/bonsai-agent/config.toml.pre-f1-2026-05-05"
```

#### A2: F1 実行 (user action、約 8 min) — Gemini G1: working tree sanity check 含む

**user 実施 (Claude は手順表示 + 結果確認のみ)**:

```bash
# 0. working tree sanity check (G1: F1 前に未保存変更を整理)
cd /Users/keizo/bonsai-agent
git status
# 未 commit 変更があれば: git stash push -m "pre-F1 stash 2026-05-05" or git commit
# fizzbuzz/ untracked は本 plan Phase E1 で対処、ここでは触れない

# 1. llama-server を停止 (PID 固定ではなく pattern match で堅牢化、Codex C6)
mkdir -p /tmp/bonsai-llama
pkill -TERM -f "llama-server.*--port 8080" || true

# port :8080 が listen 解放されるまで wait (最大 30s)
for i in $(seq 1 30); do
  if ! lsof -iTCP:8080 -sTCP:LISTEN >/dev/null 2>&1; then
    echo "port 8080 freed after ${i}s"
    break
  fi
  sleep 1
done

# 2. -c 16384 で再起動 (他 flag 維持)
nohup llama-server \
  -m /Users/keizo/Bonsai-demo/models/gguf/8B/Bonsai-8B.gguf \
  --host 127.0.0.1 --port 8080 \
  -c 16384 -ngl 99 --flash-attn on \
  -ctk q8_0 -ctv q8_0 \
  --alias Bonsai-8B \
  > /tmp/bonsai-llama/llama-server-c16384.log 2>&1 &
LLAMA_PID=$!
echo "started PID=$LLAMA_PID"

# 3. health 待機 + ready flag による fail-fast (Codex C6)
ready=0
for i in $(seq 1 30); do
  if curl -fsS --max-time 2 http://127.0.0.1:8080/v1/models 2>/dev/null | grep -q Bonsai-8B; then
    ready=1
    echo "ready after ${i}*10s"
    break
  fi
  sleep 10
done
if [ "$ready" != "1" ]; then
  echo "[ERROR] llama-server not ready in 5min, dumping last 80 lines:"
  tail -80 /tmp/bonsai-llama/llama-server-c16384.log
  echo "[FALLBACK] try -c 12288 or -ctk q4_0 -ctv q4_0"
  exit 1
fi

# 4. n_ctx 確認 (期待: 16384)
test "$(curl -fsS http://127.0.0.1:8080/slots | jq -r '.[0].n_ctx')" = "16384" \
  && echo "n_ctx=16384 OK" || { echo "[ERROR] n_ctx mismatch"; exit 1; }

# 5. config.toml 更新 (sed の whitespace 厳格化、Codex C6)
CONF="/Users/keizo/Library/Application Support/bonsai-agent/config.toml"
grep -nE "^[[:space:]]*context_length[[:space:]]*=[[:space:]]*8192[[:space:]]*$" "$CONF" \
  || { echo "[ERROR] expected line not found, manual edit required"; exit 1; }
sed -i '' -E 's/^([[:space:]]*context_length[[:space:]]*=[[:space:]]*)8192([[:space:]]*)$/\116384\2/' "$CONF"
grep -E "^[[:space:]]*context_length" "$CONF"
# 期待: context_length = 16384
```

**起動失敗時のフォールバック**:

```text
1. -c 12288 で再試行 (KV cache q8_0 維持)
2. -ctk q4_0 -ctv q4_0 (品質トレードオフ、最終手段)
3. F1 見送り → Phase B-1c (F2 単独検証、context_length=8192 のまま smoke 実行) で続行
```

#### A3: F1 完了確認 (Claude 実行、約 1 min)

```bash
# 1. n_ctx 確認
test "$(curl -fsS http://127.0.0.1:8080/slots | jq -r '.[0].n_ctx')" = "16384" \
  && echo "F1 OK" || echo "F1 FAIL"

# 2. config.toml 確認
grep "^context_length" "/Users/keizo/Library/Application Support/bonsai-agent/config.toml"
# 期待: context_length = 16384

# 3. AgentConfig.n_ctx_budget = Some(16384) → max_context_tokens = 11468 (16384*0.7)
# (実機ログでの確認は Phase B1 smoke 起動時に AgentConfig 構築 log を観察)
```

**Decision Gate G-A**: F1 成功 (n_ctx=16384 + config 同期) → Phase B 進行 / 失敗 → フォールバック (`-c 12288` / q4_0 / F1 見送り) を user 判断後 Phase B 進行。

---

### Phase B: Phase 2d smoke 検証 (二重 lane: B1a + B1b) — 約 50 min

#### B0: MLX-LM warm-up + script 整合確認 (Claude 実行、必要時)

```bash
# 1. MLX 健全性
MLX_STATE=$(curl -fsS --max-time 5 http://127.0.0.1:8000/v1/models 2>/dev/null | jq -r '.data[0].id // "DOWN"')
echo "MLX state: $MLX_STATE"

# 2. DOWN なら user に warm-up 依頼 (script は scripts/setup_mlx_ternary.sh、項目 #9)
#    user 実施: bash scripts/setup_mlx_ternary.sh
#    スクリプト不整合の場合は Codex #9 推奨の mlx-openai-server launch を使用

# 3. UP の場合、warm-up 1 リクエスト (Codex #9: /v1/models だけでは不十分、completion で実機計算を確認)
if [ "$MLX_STATE" != "DOWN" ]; then
  curl -fsS --max-time 60 http://127.0.0.1:8000/v1/completions \
    -H "Content-Type: application/json" \
    -d '{"model":"prism-ml/Ternary-Bonsai-8B-mlx-2bit","prompt":"hello","max_tokens":1}' \
    | jq -r '.choices[0].text' || echo "[WARN] MLX completion warm-up failed"
fi
```

#### B1a: smoke 実行 (MLX-primary + llama fallback、existing config) — 約 22 min

**目的**: 前 handoff 05-04 smoke (baseline=0.5876、26x HTTP 400) と **直接比較** で F1+F2 の operational impact を測る。

**構成**: 現状 config 維持 (MLX `:8000` primary + llama `:8080` fallback、`max_failures=2`)。

```bash
mkdir -p /tmp/bonsai-llama
LOG_A=/tmp/bonsai-llama/lab-v15-smoke-after-f1f2-mlx-primary-2026-05-05.log

cd /Users/keizo/bonsai-agent
BONSAI_LAB_SMOKE=1 cargo run --release -- --lab --lab-experiments 5 \
  2>&1 | tee "$LOG_A"
# 想定 duration: 16-25 min
```

**`run_in_background=true` で実行**。

#### B1b: smoke 実行 (llama-only、F2 isolation lane) — 約 22 min

**目的** (Codex HIGH #3 / Gemini Option B): MLX 出力品質変動を排除し、**F2 効果単独**を測定。fallback chain の sticky behavior (`model_router.rs:711` で success が primary に戻らない) による mid-run silent backend switch を回避。

**構成変更** (config 一時切替、Phase B1b 終了後に B1a 構成へ復元):

```bash
# 1. config を一時切替 (バックアップ別名で保存)
CONF="/Users/keizo/Library/Application Support/bonsai-agent/config.toml"
cp "$CONF" "$CONF.b1a-mlx-primary-2026-05-05"

# [fallback_chain] セクション全体 + その下 [[entries]] 全行をコメントアウト
# 安全な方法: Python で行頭 # を付与
python3 - <<'PY'
import pathlib
path = pathlib.Path("/Users/keizo/Library/Application Support/bonsai-agent/config.toml")
src = path.read_text()
lines = src.splitlines()
out = []
in_fc = False
for ln in lines:
    if ln.strip().startswith("[fallback_chain"):
        in_fc = True
    if in_fc and ln.strip() and not ln.lstrip().startswith("#"):
        out.append("# [B1b temp] " + ln)
    else:
        out.append(ln)
path.write_text("\n".join(out) + "\n")
PY

# 2. 確認
grep -E "^\[fallback_chain|^\[\[fallback_chain\." "$CONF" | head -5
# 期待: 全行が "# [B1b temp]" prefix 付き

# 3. smoke 実行
LOG_B=/tmp/bonsai-llama/lab-v15-smoke-after-f1f2-llama-only-2026-05-05.log
cd /Users/keizo/bonsai-agent
BONSAI_LAB_SMOKE=1 cargo run --release -- --lab --lab-experiments 5 \
  2>&1 | tee "$LOG_B"

# 4. config 復元 (B1a 構成へ)
cp "$CONF.b1a-mlx-primary-2026-05-05" "$CONF"
grep -E "^\[fallback_chain|^\[\[fallback_chain\." "$CONF" | head -5
# 期待: コメント無しで [fallback_chain] が再出現
```

**注意**: B1b 実行中は MLX-LM が稼働していても bonsai は呼出さない (config 上 `[fallback_chain]` 無効化済)。

#### B2: 結果計測 (Claude 実行、約 5 min)

```bash
LOG_A=/tmp/bonsai-llama/lab-v15-smoke-after-f1f2-mlx-primary-2026-05-05.log
LOG_B=/tmp/bonsai-llama/lab-v15-smoke-after-f1f2-llama-only-2026-05-05.log

for tag_log in "B1a:$LOG_A" "B1b:$LOG_B"; do
  TAG=${tag_log%%:*}
  LOG=${tag_log#*:}
  echo "===================="
  echo "=== $TAG ($LOG) ==="
  echo "===================="
  test -f "$LOG" || { echo "[ERROR] log missing"; continue; }

  # Codex C2: 日本語ログ対応
  echo "-- baseline score (前 smoke=0.5876) --"
  SCORE=$(grep -E "\[lab\] ベースライン:[[:space:]]*score=" "$LOG" | tail -1 | sed -E 's/.*score=([0-9.]+).*/\1/')
  echo "score=${SCORE:-EMPTY_PARSE}"

  echo "-- HTTP 400 件数 (前 smoke=26、target=0、acceptable<5) --"
  grep -c "http status: 400" "$LOG" || echo 0

  echo "-- 'context overflow remains' Abort (target=0) --"
  grep -c "context overflow remains" "$LOG" || echo 0

  echo "-- F2 level3 fire (middleware:context_guard) --"
  grep -c "middleware:context_guard" "$LOG" || echo 0

  echo "-- ACCEPT (前 smoke=2/5=40%) --"
  grep -cE "\[lab\] (実験.*ACCEPT|adopting variant)" "$LOG" || echo 0

  echo "-- fallback events (前 smoke=27、B1b では 0 期待) --"
  grep -cE "fallback.*(used|switched|failed)" "$LOG" || echo 0

  echo "-- duration --"
  grep -E "[Dd]uration|completed in|total.*sec" "$LOG" | tail -3
done
```

**期待値表 (B1a と B1b 並記)**:

| 指標 | 前 smoke 05-04 (B1a 比較対象) | B1a 期待 (MLX-primary) | B1b 期待 (llama-only) | Gate 判定基準 |
|------|------------------------------|------------------------|------------------------|---------------|
| HTTP 400 | 26 | < 5 (target=0、許容 10) | < 5 (target=0、許容 10) | 両方 < 10 で PASS |
| Abort | n/a | 0 (許容 1) | 0 (許容 1) | 両方 ≤ 1 で PASS (Codex C3) |
| level3 fire | n/a | ≥ 0 (informational) | ≥ 0 (informational) | informational |
| Lab score | 0.5876 | ≥ 0.5376 (-0.05 マージン) | ≥ 0.5000 (llama 単独 baseline 0.5373 ± variance) | B1a ≥ 0.5376 で品質維持 PASS |
| ACCEPT (5 中) | 2 | 1-3 | 1-3 | informational |
| fallback events | 27 | 期待減 (F2 で 400 抑制 → fallback 不発火) | **0** (fallback 無効化済) | B1b で >0 なら config 切替失敗 |
| duration | 966.7s | 900-1200s | 900-1200s | < 1500s で PASS |

**空 parse 時の挙動 (Codex C2)**: `SCORE=EMPTY_PARSE` の場合は **Gate fail-closed** (即時 G-B FAIL 扱い、Phase D 中止)。

#### B3: Decision Gate G-B (Phase 2d 結果評価、Codex C3/C4 厳格化版)

| Gate | 条件 | 判定 |
|------|------|------|
| **G-B1**: F1+F2 効果確認 | B1a + B1b 両方で `400 < 10 AND Abort ≤ 1` | PASS → G-B2 評価 |
| **G-B2 (strict)**: 品質維持 | B1a `score ≥ 0.5376` (前 smoke -0.05) | PASS → Phase D 進行候補 |
| **G-B2 (investigate)**: 中庸退行 | B1a `0.5000 ≤ score < 0.5376` | **investigate-only** (Phase D 中止、原因究明 plan へ) |
| **G-B2 (fail)**: 退行 | B1a `score < 0.5000` | FAIL → Safe Exit (G2) |
| **G-B3**: F2 isolation 効果 | B1b `score >= B1a score - 0.10`、かつ `400 件数 ≤ B1a` | PASS → F2 が backend 非依存に効いている確証 |
| **G-B4**: F2 実機発火 | B1a or B1b で `level3 fire ≥ 1` | informational (PASS 必須ではない) |

**Phase D 進行条件**: G-B1 PASS **AND** G-B2 (strict) PASS **AND** G-B3 informational OK

#### B4: G-B FAIL 時 Safe Exit 手順 (Gemini G2)

```bash
# 1. config 復元 (B1b で切替えた場合)
cp "$CONF.b1a-mlx-primary-2026-05-05" "$CONF" 2>/dev/null || true
grep -E "^\[fallback_chain" "$CONF" || echo "[WARN] fallback_chain section may need manual check"

# 2. F1 巻き戻し判定 (user)
# - n_ctx=16384 で問題発覚なら -c 8192 に戻す user action
# - F2 圧縮で品質劣化が真因なら次セッションで F2 ratio 60% に下げる別 plan

# 3. partial handoff 記録 (CLAUDE.md 項目 188 + handoff session.md に "G-B FAIL by ..." を明記)

# 4. cargo test --release --lib で 1026 passed 維持確認 (regression なきこと)
```

---

### Phase C: 結果分析 + Lab v15 core 22 進行判定 — 約 5 min

#### C1: 結果サマリ (Claude 実行)

Phase B2 結果を B1a / B1b 並記表 + Gate 判定で整理。CLAUDE.md 項目 188 ドラフトを作成。

#### C2: Lab v15 core 22 進行可否判断

| 進行可 | 進行不可 |
|--------|----------|
| G-B1 + G-B2 (strict) 両 PASS | G-B FAIL (any) or G-B2 investigate |
| MLX が応答中 (B0 確認 + B1a 完走) | MLX が DOWN/cold-start |
| user が 6h 以上の連続実行に同意 | user が時間制約 / 別 task 優先 |
| MLX uptime < 2h or 直前 restart 完了 (Codex/Gemini A2) | MLX uptime ≥ 2h かつ restart 拒否 |

**user に決定を仰ぐ**: Phase D を本セッション内で続行 or 別セッションへ繰越し。

---

### Phase D (条件付き、user 同意時): Lab v15 core 22 baseline 実測 — 約 1.5h (pure baseline 化)

#### D1: 事前準備 (Claude 実行、約 5 min) — Codex/Gemini A2: MLX restart mandatory 化

```bash
# 1. MLX uptime チェック (Phase A1 / B0 起動時刻からの経過時間で判定)
# Codex/Gemini A2: uptime ≥ 2h なら restart を mandatory 化

MLX_PID=$(pgrep -f "mlx.*Ternary-Bonsai-8B-mlx-2bit" | head -1)
if [ -n "$MLX_PID" ]; then
  MLX_ETIME=$(ps -p "$MLX_PID" -o etimes= | tr -d ' ')
  if [ "$MLX_ETIME" -gt 7200 ]; then
    echo "[MANDATORY] MLX uptime=${MLX_ETIME}s (>2h)、restart 必要"
    # user に MLX restart 依頼 (script 整合: Codex #9):
    #   pkill -f "mlx.*Ternary-Bonsai-8B"
    #   bash scripts/setup_mlx_ternary.sh
    # 完了後 B0 warm-up を再実行
  else
    echo "[OK] MLX uptime=${MLX_ETIME}s、restart 不要"
  fi
fi

# 2. llama-server (fallback) 健全性
curl -fsS http://127.0.0.1:8080/v1/models | jq -r '.data[0].id'

# 3. config 確認 (B1a 構成に復元済確認)
grep -E "context_length|^\[fallback_chain" \
  "/Users/keizo/Library/Application Support/bonsai-agent/config.toml" | head -10

# 4. (optional) Codex resume sanity check, SESSION 019def01
#    質問: "F2 + n_ctx=16384 で MLX-primary core 22 baseline (--lab-experiments 0) を実行する想定外リスクは?"
```

#### D2: Lab v15 core 22 **pure baseline** 実行 (Claude 実行、`run_in_background=true`) — Codex C1: `--lab-experiments 0`

```bash
LOG_D=/tmp/bonsai-llama/lab-v15-core22-baseline-2026-05-05.log
mkdir -p /tmp/bonsai-llama

cd /Users/keizo/bonsai-agent
# Codex C1: experiments=0 で pure baseline のみ実行 (mutation loop は別 D2b へ分離)
BONSAI_BENCH_TIER=core cargo run --release -- --lab --lab-experiments 0 \
  2>&1 | tee "$LOG_D" &
echo "started PID=$!"
# 想定 duration: ~1.5h (k=3 / 22 tasks、項目 184=63min, 項目 185=90min の中間値想定)
```

**Background 監視戦略 (Codex A3 / Gemini #1)**:

- 進行 marker: `\[lab\] ベースライン.*task[[:space:]]*[0-9]+/22` or `experiment.*complete` を 30 min 毎に grep
- 早期 abort 条件:
  - **45-60 min 連続で task completion log 増加なし** → 生成 hang、kill (Codex A3)
  - **HTTP 400 件数 ≥ 20** で kill (Codex A3、100 → 20 に厳格化)
  - **MLX `/v1/completions` warm-up が 5 連続 timeout** → kill + llama 単独再構成
  - **wallclock > 2.5h** → kill (k=3 / 22 / pure baseline で 1.5h 想定の 67% margin)

#### D2b (将来別 plan): mutation experiments 14 件

本 plan のスコープ外。pure baseline confirmed 後、別 plan `lab-v15-core22-mutations.md` で `--lab-experiments 14` を実行。

#### D3: 結果計測 (Claude 実行、約 5 min) — Codex C2: 日本語 grep 対応

```bash
LOG_D=/tmp/bonsai-llama/lab-v15-core22-baseline-2026-05-05.log

echo "=== baseline score (項目 184=0.7976、項目 185=0.8131、項目 182 llama=0.7571) ==="
SCORE=$(grep -E "\[lab\] ベースライン:[[:space:]]*score=" "$LOG_D" | tail -1 | sed -E 's/.*score=([0-9.]+).*/\1/')
echo "score=${SCORE:-EMPTY_PARSE}"

echo "=== pass@k / pass_consec ==="
grep -E "pass@k|pass_consec|pass_at_k|pass_consecutive_k" "$LOG_D" | tail -5

echo "=== HTTP 400 件数 (期待: 0、許容 < 10) ==="
grep -c "http status: 400" "$LOG_D"

echo "=== Abort 件数 (期待: 0) ==="
grep -c "context overflow remains" "$LOG_D"

echo "=== fallback events (cache hits も観測、Codex C7) ==="
grep -cE "fallback.*(used|switched)" "$LOG_D"
grep -cE "cache.*(hit|stored)" "$LOG_D" || echo 0

echo "=== duration ==="
grep -E "[Dd]uration|completed in|total.*sec" "$LOG_D" | tail -5
```

#### D4: Decision Gate G-D (Codex C5: Zone 再定義)

| Gate | 条件 | 判定 |
|------|------|------|
| **G-D1 (Zone A)** | score ≥ 0.81 **かつ** 2 連続実行で再現 (本 plan は 1 回のみのため "candidate") | F1+F2+MLX-primary を defaults 化候補、別 plan で再現確認後に確定 |
| **G-D2 (Zone B)**: 期待通り | 0.78 ≤ score < 0.81 | 維持、個別検証で defaults 化判断 |
| **G-D3 (Zone C)**: 許容内 | 0.75 ≤ score < 0.78 | 環境劣化 / 構成微調整必要 |
| **G-D4 (Zone D)**: 退行 | score < 0.75 | 真因分析 (項目 173 MLX 劣化再発? F2 過保守? CachedBackend contamination?) |

**Zone 比較対象**:
- 項目 184 MLX core 22 = 0.7976 (1 回目)
- 項目 185 MLX core 22 = 0.8131 (2 回目、許容再現確認)
- 項目 182 llama core 22 = 0.7571 (MCP detach 後)

**追加判定 (Codex C7 R13)**: cache hit 数が異常に高い場合 (例: 全 22 task の半数以上で primary cache hit) → CachedBackend contamination 疑い、score 数値の信頼性に注記。

---

### Phase E: 後処理 + handoff 記録 — 約 18 min

#### E1: working tree cleanup

```bash
# Option A: .gitignore 追加 (推奨)
cd /Users/keizo/bonsai-agent
grep -q "^fizzbuzz/$" .gitignore 2>/dev/null || echo "fizzbuzz/" >> .gitignore
git add .gitignore
git commit -m "chore(gitignore): Lab smoke 副作用 fizzbuzz/ を ignore"

# Option B: 削除 (user 判断)
# rm -rf fizzbuzz/
```

#### E2: CLAUDE.md 項目追記 (Claude 実行、項目 187 numbered-list pattern 準拠、Gemini #7)

- **項目 188**: F1 + Phase 2d smoke 結果 (B1a + B1b 並記、400 件数 / Abort / level3 fire / Lab score / G-B 判定)
- **項目 189** (Phase D 実行時のみ): Lab v15 core 22 baseline (Zone 判定 + Phase 5/項目 184/185 比較 + cache contamination 注記)

例 (項目 188 ドラフト):

```markdown
188. **F1 (llama -c 16384) + Phase 2d smoke 検証 (B1a MLX-primary + B1b llama-only)**: ...
     B1a: score=X (前 smoke 0.5876 比 ±Y)、HTTP 400=N 件 (前 26 から削減)、Abort=M 件、level3 fire=K 回
     B1b (F2 isolation): score=X'、HTTP 400=N' 件、fallback events=0 (config 切替確認)
     G-B1/G-B2/G-B3 判定 = ... 、Phase D 進行 = (Yes/No)
```

#### E3: handoff 記録 (Claude 実行)

`session_2026_05_05_handoff.md` (or `_05b`):

```yaml
---
name: Session Handoff 2026-05-05
description: F1 + Phase 2d smoke (B1a MLX-primary + B1b llama-only F2 isolation) + (条件付き) Lab v15 core 22 baseline、Gate G-B1/G-B2 = X、Zone Y、CachedBackend contamination check 結果 = Z
type: project
---
```

#### E4: MEMORY.md 更新

```markdown
- [Session Handoff 05-05](session_2026_05_05_handoff.md) — F1 + Phase 2d smoke (二重 lane) + (Lab v15 core 22) ...
```

---

## Key Files

| File | Operation | Description |
|------|-----------|-------------|
| `~/Library/Application Support/bonsai-agent/config.toml` | Modify (F1) | `context_length = 8192 → 16384` (sed whitespace-strict) |
| llama-server PID 68103 | User restart (F1) | `-c 16384` で再起動、新 PID |
| `~/Library/Application Support/bonsai-agent/config.toml.pre-f1-2026-05-05` | Write (A1) | F1 適用前 backup |
| `~/Library/Application Support/bonsai-agent/config.toml.b1a-mlx-primary-2026-05-05` | Write (B1b) | B1b 切替前の MLX-primary 構成 backup、B1b 完了後に復元 |
| `/tmp/bonsai-llama/llama-server-c16384.log` | Write (F1) | 再起動後 stdout/stderr |
| `/tmp/bonsai-llama/lab-v15-smoke-after-f1f2-mlx-primary-2026-05-05.log` | Write (B1a) | MLX-primary smoke 出力 |
| `/tmp/bonsai-llama/lab-v15-smoke-after-f1f2-llama-only-2026-05-05.log` | Write (B1b) | llama-only smoke 出力 (F2 isolation) |
| `/tmp/bonsai-llama/lab-v15-core22-baseline-2026-05-05.log` | Write (D2、条件付き) | Lab core 22 pure baseline (`--lab-experiments 0`) |
| `.gitignore` | Modify (E1、user 判断) | `fizzbuzz/` 追加 |
| `/Users/keizo/bonsai-agent/CLAUDE.md` | Modify (E2) | 項目 188 (+ 189) 追記 |
| `~/.claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_05_handoff.md` | Write (E3) | handoff |
| `~/.claude/projects/-Users-keizo-bonsai-agent/memory/MEMORY.md` | Modify (E4) | 1 行追加 |
| `.claude/plan/phase2d-smoke-and-lab-v15-core22.md` (v1) | (no change) | archival 保持 |

**production code (`src/**`)**: **変更ゼロ**。

## Risks and Mitigation (v1 12 件 + v2 追加 2 件 = 14 件)

| # | Risk | 確率 | Mitigation |
|---|------|------|------------|
| **R1** | F1 `-c 16384` で VRAM 不足 (M2 16GB) | 中 | `-c 12288` → `-ctk q4_0 -ctv q4_0` → F1 見送り |
| **R2** | smoke 中 MLX cold-start で初回 timeout 多発 | 中 | B0 warm-up `/v1/completions` 1 リクエスト、項目 167 socket timeout 180s、fallback chain |
| **R3** | 400 が 10+ 件残存 | 低 | `CONTEXT_GUARD_RATIO_NUM=70 → 60` 別 plan |
| **R4** | Abort 多発 | 低 | emergency_keep 増 / compact_level3 改善別 plan |
| **R5** | Phase D 中 MLX hang/degrade | 中 | 30 min health check + progress marker (45-60 min なし → kill) + fallback chain |
| **R6** | Phase D wallclock 超過 | 中 | 2.5h kill (pure baseline 1.5h 想定の 67% margin)、partial 結果で handoff |
| **R7** | working tree `fizzbuzz/` の git 汚染 | 低 | E1 で .gitignore (Option A) |
| **R8** | Codex SESSION 失効 | 低 | resume 失敗で skip、Claude direct |
| **R9** | F2 圧縮で重要 thinking/handoff 破壊 | 低 | smoke で品質確認 (G-B2)、項目 47-58 で handoff summary 品質保証 |
| **R10** | llama 再起動の port 一時不在 | 低 | A2 lsof port wait + ready flag (Codex C6) |
| **R11** | smoke + core 22 連続で MLX cumulative degrade | 中 | D1 で uptime > 2h なら restart **mandatory** (Codex/Gemini A2) |
| **R12** | `[fallback_chain]` 残存で純粋 F2 効果が判別不能 | **解消** | **B1b llama-only smoke 追加で対処済 (Codex HIGH #3)** |
| **R13** (新規) | **CachedBackend contamination**: synthetic model id で primary/fallback 切替が cache に隠蔽、measurement に影響 | 中 | D3 で `cache.*hit` log を grep し、異常高頻度なら score に注記。disable 機構が必要なら別 plan で `CachedBackend::disable_for_measurement()` 検討 (Codex C7 LOW #10) |
| **R14** (新規) | **score grep 空 parse による Gate fail-open** | **解消** | **EMPTY_PARSE 時に Gate fail-closed (Codex C2)** |

## Decision Gates 一覧 (v2)

| Gate | Phase | 条件 | True | False |
|------|-------|------|------|-------|
| **G-A** | A3 | n_ctx=16384 + config 同期 | Phase B 進行 | フォールバック (`-c 12288` / q4_0) / F2 単独検証 |
| **G-B1** | B3 | B1a + B1b 両方で `400 < 10 AND Abort ≤ 1` | G-B2 評価 | G-B FAIL Safe Exit (B4) |
| **G-B2 (strict)** | B3 | B1a `score ≥ 0.5376` (前 smoke -0.05) | Phase D 候補 | G-B2 (investigate) 評価 |
| **G-B2 (investigate)** | B3 | B1a `0.5000 ≤ score < 0.5376` | Phase D 中止、原因究明別 plan | G-B2 (fail) 評価 |
| **G-B2 (fail)** | B3 | B1a `score < 0.5000` | Safe Exit (B4) + handoff partial | n/a |
| **G-B3** | B3 | B1b `score ≥ B1a-0.10 AND 400 ≤ B1a` | F2 isolation 効果確証 | F2 と backend 効果が分離不能と handoff 注記 |
| **G-B4** | B3 | level3 fire ≥ 1 (B1a or B1b) | informational PASS | informational FAIL (production 影響なし) |
| **G-C** | C2 | user 同意 + MLX uptime < 2h or restart 完了 | Phase D 進行 | 別セッション繰越 |
| **G-D1-4** | D4 | Zone A `≥0.81 (candidate)` / B `0.78-0.81` / C `0.75-0.78` / D `<0.75` | Zone 別 handoff 記録 | n/a |

## Test Strategy

| 区分 | 検証 | 方法 |
|------|------|------|
| Production code | 1026 passed 退行ゼロ | `cargo test --release --lib` (Phase A1 で 1 度実行) |
| F2 実機発火 | level3 fire 観測 | smoke log の `middleware:context_guard` grep |
| F2 graceful Abort | Abort 動作確認 | smoke log の `context overflow remains` grep (期待: 0) |
| F2 効果 | 400 削減 | B1a smoke 26 → < 10 で PASS |
| F2 isolation | backend 非依存 | B1b llama-only smoke で 400/score 計測、B1a と差分比較 |
| 品質維持 | Lab score | B1a `score ≥ 0.5376` (前 smoke -0.05 strict) |
| Lab v15 core 22 (条件付き) | Zone 判定 | 項目 184/185/182 比較、cache contamination check |
| Gate fail-closed | EMPTY_PARSE 時の挙動 | B2 で `SCORE=EMPTY_PARSE` を即時 G-B FAIL 扱い |

## YAGNI / 見送り

| 案 | 判定 | 理由 |
|----|------|------|
| Plan 分割 (Gemini Option A: smoke / core22) | 本 plan 内修正で対応 | combined plan のコンテキスト連続性のメリット維持、core22 = pure baseline 化で複雑度削減済 |
| Phase D2b (mutation experiments 14 件) | 別 plan へ切出し | core22 baseline 確認後の next step、本 plan のスコープ外 (Codex C1) |
| Lab v15 extended tier | 別 plan | core tier 優先 (項目 173 で extended=0.341 baseline 確定) |
| Gemini reviewer wrapper exit 1 調査 | 本 plan 対象外 | optional |
| CachedBackend trait 動的化 RFC | 別 plan | R13 観測のみで本 plan 範囲、disable 機構は別途 |
| F3 (400 → 自動 retry) | 不要 | F2 で 400 自体が発生しないため (handoff 05-04b §YAGNI) |
| F4 (1bit retry 閾値調整) | 不要 | 同上 |
| `accept_threshold` field | 別 plan | smoke 補正の真の false-accept 防止、本 plan の YAGNI fence 外 |
| `AuditAction::ContextGuard` SQLite log | 見送り | smoke log grep で代替 |
| BPE tokenizer (tiktoken) 導入 | 見送り | 70% ratio + hybrid estimator で十分 |
| MLX restart between B/D | mandatory 化 (Codex/Gemini A2) | uptime ≥ 2h で必須化、それ以下なら不要 |

## SESSION_ID (for /ccg:execute or sanity check)

- **CODEX_SESSION**: `019def01-d210-7e00-a909-d43525f735fd` (F2 architect、Phase D1 sanity check で resume 候補、optional)
- **GEMINI_SESSION**: 本セッション内 `/ccg` review で生成 (artifact: `.omc/artifacts/ask/gemini-review-...-2026-05-04T04-07-35-139Z.md`)、再 spawn は新規

## 完了基準

### Phase 2d (B + C) 完了 (★★★ 必達)

1. F1 完了: `curl :8080/slots | jq .[0].n_ctx` = 16384
2. config.toml `context_length = 16384`
3. B1a 実行完了: `lab-v15-smoke-after-f1f2-mlx-primary-2026-05-05.log` 取得
4. B1b 実行完了: `lab-v15-smoke-after-f1f2-llama-only-2026-05-05.log` 取得
5. config 復元 (B1b 後): `[fallback_chain]` セクションがコメント無しで再現
6. G-B1 PASS: B1a + B1b 両方で `400 < 10 AND Abort ≤ 1`
7. G-B2 (strict) PASS: B1a `score ≥ 0.5376`
8. G-B3 informational: B1b score 比較
9. handoff 記録 + MEMORY.md 1 行追加 + CLAUDE.md 項目 188 追記

### Phase D 完了 (条件付き、★★)

10. Lab v15 core 22 **pure baseline** 実行完了 (`--lab-experiments 0`、Codex C1)
11. Zone 判定 + cache contamination check 結果 (R13/Codex C7) を handoff 記録
12. CLAUDE.md 項目 189 追記

### 全フェーズ完了

13. working tree clean (E1)
14. cargo test --release --lib 1026 passed 維持確認

## 想定 commit

| Phase | commit | 内容 |
|-------|--------|------|
| E1 | `chore(gitignore): Lab smoke 副作用 fizzbuzz/ を ignore` | 1 file |
| E2 | `docs(claude.md): 項目 188 — F1 + Phase 2d smoke (B1a + B1b) 結果` | 1 file |
| E2 (条件付き) | `docs(claude.md): 項目 189 — Lab v15 core 22 pure baseline (Zone X)` | 1 file |

production code commit ゼロ。

## 推定所要時間 (v2)

| Phase | 所要 | 実行者 | v1 比 |
|-------|------|--------|-------|
| A1 | 2 min | Claude | 同 |
| A2 (sanity check + F1) | 8 min | user | +3 min (Gemini G1 git status) |
| A3 | 1 min | Claude | 同 |
| B0 | 1 min (DOWN なら +30 min) | Claude/user | 同 |
| **B1a** | 22 min | Claude (background) | 同 |
| **B1b** (新規) | 22 min | Claude (background) | **+22 min** (F2 isolation) |
| B2-B3 (二重 lane 計測) | 5 min | Claude | +0 |
| C | 5 min | Claude + user | 同 |
| **小計 (Phase 2d まで)** | **約 1h 5min** | (warm-up 不要時) | **+25 min vs v1** |
| D1 (uptime check + restart 判定) | 5-10 min | Claude (+ user if restart) | +5 min (mandatory) |
| **D2 (pure baseline)** | **~1.5h** | Claude (background) | **-4.5h vs v1** (`--lab-experiments 0` で時間短縮) |
| D3-D4 | 10 min | Claude | 同 |
| **小計 (Phase D)** | **約 2h** | | **-4.5h vs v1** |
| E1-E4 | 18 min | Claude + user | +3 min (項目 188 詳細化) |
| **総計 (Phase D 含む)** | **約 3h 25min** | | **-3.5h vs v1** |
| **総計 (Phase D 別セッション繰越)** | **約 1h 30min** | | +30 min (B1b 追加) |

**主要 trade-off**: B1b で +22 min、D2 で -4.5h (pure baseline)。**正味で v1 比 -3.5h かつ品質向上**。
