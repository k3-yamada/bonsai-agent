# Plan: Lab v15 core 22 baseline — llama-only (B1b 構成、F2 regression + B1b 再現)

> **由来**: 前 handoff (`session_2026_05_05_handoff.md` ★★★ TODO) の継続。Phase 2d B1b smoke 5 task で `[fallback_chain]` コメントアウト構成 = score **0.7440** (vs B1a MLX-primary + llama fallback = 0.5862、**+27%**) という驚異的結果が出たが、smoke 5 task 由来 variance が排除されておらず、core 22 task k=3 での再現確認が必要。
>
> **副次目的**: F2 ContextOverflowGuard (項目 187) を実装した HEAD で項目 182 (llama core 22 = 0.7571、MCP detach 後、F2 不在) と直接比較し、F2 が baseline に regression を与えていないことを確証する (regression check)。
>
> **前 plan と区別**: `phase2d-smoke-and-lab-v15-core22-v2.md` の Phase D は MLX-primary 構成で `--lab-experiments 0` を想定したが Decision Gate G-B1 FAIL で中止。本 plan は **llama-only (B1b 構成)** で同 Phase D を独立実行する。

## Task Type

- [ ] Frontend
- [x] Backend (Validation + Measurement、production code 変更ゼロ)
- [ ] Fullstack

## Background

### 現状把握 (本セッション開始時、Phase 0 で確認済)

| 観点 | 値 | 備考 |
|------|-----|------|
| llama-server | **PID 63423**、`-c 12288 -ngl 99 --flash-attn on -ctk q8_0 -ctv q8_0` 稼働中 | F1 α (`-c 12288`) 維持 |
| llama-server n_ctx | **12288** | 確認済 (`/slots .[0].n_ctx`) |
| MLX-LM `:8000` | `prism-ml/Ternary-Bonsai-8B-mlx-2bit` 応答中 | 本 plan では呼ばない (B1b 構成) |
| config `[model].backend` | `llama-server` | primary 直結 |
| config `[model].context_length` | 12288 | F1 α |
| config `[fallback_chain]` | **存在** (mlx-lm primary → llama-server fallback) | 本 plan の Phase 1 でコメントアウト |
| PhysMem | 15G used / 258M unused | タイト、smoke 開始前に再確認 |
| 直前 commit | `4df1d76 docs(claude,plan): 項目 188 + Phase 2d smoke v1/v2` | clean working tree |
| Origin 差分 | `master ahead 16 commits` | push 未実施 |

### 比較対象 baseline (期待値判定基準)

| baseline | score | pass@k | duration | 構成 |
|----------|-------|--------|----------|------|
| **B1b smoke 5 task** (前 handoff) | **0.7440** | 0.6667 | 16.1 min | llama-only、F2 あり、`-c 12288` |
| 項目 182 llama core 22 | 0.7571 | 0.8333 | 60.86 min | llama-only、F2 **なし**、MCP detach 後、`-c 8192` (推定) |
| 項目 184 MLX core 22 | 0.7976 | 0.9091 | 63.69 min | MLX-only、F2 なし |
| 項目 185 MLX core 22 再現 | 0.8131 | 0.9394 | 90.33 min | MLX-only、F2 なし、cold-start |

**核心仮説**:
- H1 (本命): F2 は baseline に regression を与えない (`score >= 0.7571 - 0.03 = 0.7271`)
- H2: B1b smoke 0.7440 は core 22 でも維持 (≥ 0.72)、5 task variance ではない実効効果である
- H3 (劣化警戒): F2 圧縮の overhead (項目 188 副次知見 6 で前 smoke 比 +19% duration 観測) が core 22 で蓄積し品質劣化 → score < 0.72

## Implementation Plan

### Phase 1: config 一時切替 (B1b 構成へ、`[fallback_chain]` コメントアウト) — 約 2 min

```bash
CONF="/Users/keizo/Library/Application Support/bonsai-agent/config.toml"

# 1. バックアップ (本 plan 完了後に復元する base)
cp "$CONF" "$CONF.pre-llama-only-core22-2026-05-05"

# 2. [fallback_chain] section 全行コメントアウト (Python で安全に)
python3 - <<'PY'
import pathlib
path = pathlib.Path("/Users/keizo/Library/Application Support/bonsai-agent/config.toml")
src = path.read_text()
lines = src.splitlines()
out = []
in_fc = False
for ln in lines:
    stripped = ln.strip()
    if stripped.startswith("[fallback_chain") or stripped.startswith("[[fallback_chain"):
        in_fc = True
    elif stripped.startswith("[") and not stripped.startswith("[[fallback_chain"):
        in_fc = False
    if in_fc and stripped and not ln.lstrip().startswith("#"):
        out.append("# [llama-only-core22 temp] " + ln)
    else:
        out.append(ln)
path.write_text("\n".join(out) + "\n")
PY

# 3. 確認: fallback_chain セクションが全行コメント化
grep -nE "^\[fallback_chain|^\[\[fallback_chain|^# \[llama-only-core22 temp\]" "$CONF" | head -10

# 4. backend は llama-server 直結のまま (変更なし)
grep "^backend" "$CONF"
# 期待: backend = "llama-server"
```

### Phase 2: cargo test regression check — 約 30s

```bash
cd /Users/keizo/bonsai-agent
cargo test --release --lib 2>&1 | tail -5
# 期待: 1026 passed; 0 failed
```

### Phase 3: pure baseline 実行 (`run_in_background=true`) — 約 60-90 min

```bash
LOG=/tmp/bonsai-llama/lab-v15-core22-llama-only-2026-05-05.log
mkdir -p /tmp/bonsai-llama

cd /Users/keizo/bonsai-agent
BONSAI_BENCH_TIER=core cargo run --release -- --lab --lab-experiments 0 \
  2>&1 | tee "$LOG"
```

**Background 監視戦略**:
- 30 min 毎に `grep -cE "task[[:space:]]*[0-9]+/22" "$LOG"` で進捗確認
- 早期 abort 条件:
  - **45 min 連続で task 完了 log 増えない** → MLX hang か llama 異常 → kill
  - **HTTP 400 件数 ≥ 30** で kill (smoke 11 件の 3 倍超 = abnormal regression)
  - **wallclock > 2h** → kill (B1b smoke 16min × 4.4 倍 = 70min 想定の 71% margin)
- 通常終了: `[lab] ベースライン:` ログ出現 + プロセス自然終了

### Phase 4: 結果計測 — 約 5 min

```bash
LOG=/tmp/bonsai-llama/lab-v15-core22-llama-only-2026-05-05.log

echo "=== baseline score (B1b smoke=0.7440 / 項目 182=0.7571) ==="
SCORE=$(grep -E "\[lab\] ベースライン:[[:space:]]*score=" "$LOG" | tail -1 | sed -E 's/.*score=([0-9.]+).*/\1/')
echo "score=${SCORE:-EMPTY_PARSE}"

echo "=== pass@k / pass_consec ==="
grep -E "pass_at_k|pass_consecutive_k|pass@k|pass_consec" "$LOG" | tail -5

echo "=== HTTP 400 件数 (期待: < 30、target ~ 22) ==="
grep -c "http status: 400" "$LOG"

echo "=== Abort 件数 ==="
grep -c "context overflow remains" "$LOG"

echo "=== F2 level3 fire (middleware:context_guard) ==="
grep -c "middleware:context_guard" "$LOG"

echo "=== fallback events (期待: 0、構成切替確認) ==="
grep -cE "fallback.*(used|switched)" "$LOG"

echo "=== duration ==="
grep -E "[Dd]uration|completed in|total.*sec" "$LOG" | tail -5
```

### Phase 5: Decision Gate G-X (Zone 判定)

| Zone | 条件 | 解釈 | 次アクション |
|------|------|------|--------------|
| **Zone A** | score ≥ 0.78 | 項目 182 を上回る、MLX (0.79-0.81) に肉薄 = F2 + llama-only が **新最適解候補** | F2 defaults 化 + llama-only を `[fallback_chain]` 不要化提案 |
| **Zone B** | 0.75 ≤ score < 0.78 | 項目 182 (0.7571) 互換、F2 が neutral regression なし = **健全** | H1 確証、handoff |
| **Zone C** | 0.70 ≤ score < 0.75 | smoke ≥ core 22 ≥ 項目 182 - 0.05、F2 軽度 overhead | H3 部分確認、F2 ratio 調整別 plan |
| **Zone D** | score < 0.70 | 項目 182 比 -0.06 以上の退行、F2 重大 overhead | H3 確証、F2 ratio 60% に下げる plan + production review |

**追加判定**:
- HTTP 400 ≥ 30 → smoke (11 件) からの異常増加 = F2 が本 task で逆効果可能性、調査必須
- HTTP 400 < 5 → F2 が core 22 規模で効いている確証 = G-B4 の遅延 PASS
- fallback events > 0 → config 切替失敗 (B1b 構成になっていない)、結果無効

### Phase 6: 後処理 — 約 15 min

#### 6.1: config 復元

```bash
CONF="/Users/keizo/Library/Application Support/bonsai-agent/config.toml"
cp "$CONF.pre-llama-only-core22-2026-05-05" "$CONF"
grep -nE "^\[fallback_chain|^\[\[fallback_chain" "$CONF" | head -5
# 期待: コメント無しで再出現
```

#### 6.2: CLAUDE.md 項目 189 追記

```markdown
189. **Lab v15 core 22 llama-only baseline + F2 regression check (★★★ TODO 消化)**:
     前 handoff (項目 188) の B1b smoke 0.7440 を core 22 task k=3 で再現確認 + F2 regression check。構成: llama-server `-c 12288` 直結、`[fallback_chain]` コメントアウト、F2 ContextOverflowGuard (項目 187) 有効。結果: score=**X** / pass@k=**Y** / duration=**Z** min / HTTP 400=**N** 件 / level3 fire=**M** / Abort=0。Zone **判定**。比較: B1b smoke (0.7440 / +Δ)、項目 182 (0.7571 / +Δ)、項目 184/185 MLX (0.79-0.81)。F2 regression = **判定**。次=...
```

#### 6.3: handoff 記録 + MEMORY.md 更新

`session_2026_05_05b_handoff.md` (前 handoff の続き番号) を作成。

#### 6.4: commit

```bash
cd /Users/keizo/bonsai-agent
git add CLAUDE.md .claude/plan/lab-v15-core22-llama-only.md
git commit -m "docs(claude,plan): 項目 189 — Lab v15 core 22 llama-only baseline (Zone X)"
```

production code 変更ゼロのため commit は docs のみ。

## Key Files

| File | Operation | Description |
|------|-----------|-------------|
| `~/Library/Application Support/bonsai-agent/config.toml` | Modify (Phase 1, restore Phase 6.1) | `[fallback_chain]` セクションコメントアウト → 復元 |
| `~/Library/Application Support/bonsai-agent/config.toml.pre-llama-only-core22-2026-05-05` | Write (Phase 1) | 切替前 backup |
| `/tmp/bonsai-llama/lab-v15-core22-llama-only-2026-05-05.log` | Write (Phase 3) | core 22 baseline 出力 |
| `/Users/keizo/bonsai-agent/CLAUDE.md` | Modify (Phase 6.2) | 項目 189 追記 |
| `~/.claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_05b_handoff.md` | Write (Phase 6.3) | handoff |
| `~/.claude/projects/-Users-keizo-bonsai-agent/memory/MEMORY.md` | Modify (Phase 6.3) | 1 行追加 |
| `.claude/plan/lab-v15-core22-llama-only.md` | (本 plan) | 本ファイル |

## Risks and Mitigation

| # | Risk | 確率 | Mitigation |
|---|------|------|------------|
| R1 | 60-90 min 超過 / hang | 中 | 2h wallclock kill (Phase 3 早期 abort 条件) |
| R2 | PhysMem 258M unused でメモリ圧迫 | 中 | smoke 開始前に再確認、必要なら他 process kill |
| R3 | HTTP 400 が core 22 で smoke 比異常増加 | 中 | Phase 3 early kill (≥ 30 件)、Phase 4 で記録、F2 ratio 調整別 plan |
| R4 | MLX-LM が config 切替後も背景に残る | 低 | `[fallback_chain]` コメントアウトで bonsai は MLX 不呼出、影響なし (リソース消費のみ) |
| R5 | config 切替の Python script で構文ミス | 低 | Phase 1 末尾で grep 確認、ミスあれば即 backup から復元 |
| R6 | task hang で MLX 経路に切替わらない | 低 | B1b 構成で fallback_chain 無効、llama 単独で完走判定 |
| R7 | F2 level3 fire 0 (smoke と同) | 中 | informational、production 影響なし、F2 設計通り (累積保護) |
| R8 | 本 plan 6h を超過 | 低 | 本 plan 想定総計 1.5-2h、6h は B1b 比 4-5 倍 margin |

## YAGNI / 見送り

| 案 | 判定 | 理由 |
|----|------|------|
| `--lab-experiments 14` で mutation も同時計測 | 別 plan | pure baseline 確認が本 plan の必達、mutation は次 step |
| F1 を `-c 16384` に再挑戦 | 別 plan | PhysMem 15G/16G で OOM 確実、F1 α (`-c 12288`) で十分 baseline 取得可 |
| MLX 単独 core 22 再々測 (項目 184/185 後の差) | 別 plan | 項目 185 で許容再現確認済 |
| `accept_threshold` field 導入 | 別 plan | 本 plan は pure baseline、ACCEPT 評価不要 |
| F2 ratio 調整 (70 → 60) 検証 | 条件付き別 plan | Phase 5 G-X で Zone D 判定時のみ起票 |
| MLX-LM プロセス kill | 不要 | 本 plan で呼ばないがリソース大、user 判断 |
| BPE tokenizer 導入 | 見送り | 項目 187 hybrid estimator で十分 |
| Lab v15 extended tier | 別 plan | core tier 優先 |

## Decision Gates 一覧

| Gate | Phase | 条件 | True | False |
|------|-------|------|------|-------|
| **G-1** | Phase 2 | cargo test 1026 passed | Phase 3 進行 | regression 修正 (本 plan 中止) |
| **G-2** | Phase 3 | wallclock < 2h AND HTTP 400 < 30 AND task 完了 marker 増加 | Phase 4 進行 | early abort + Phase 6 cleanup |
| **G-X (Zone A-D)** | Phase 5 | score 範囲 | Zone 別アクション (上記表) | n/a |

## Test Strategy

| 区分 | 検証 | 方法 |
|------|------|------|
| Production code regression | 1026 passed 維持 | `cargo test --release --lib` (Phase 2) |
| F2 regression | baseline ≥ 0.7271 (項目 182 - 0.03) | Phase 4 score 比較 |
| B1b 再現 | core 22 score が smoke 0.7440 ± 0.05 | Phase 4 score 比較 |
| config 切替正当性 | fallback events == 0 | Phase 4 grep |
| Empty parse fail-closed | EMPTY_PARSE → 即 G-X 中断 | Phase 4 SCORE check |

## 完了基準

1. config 切替確認 (Phase 1): fallback_chain 全行コメント化
2. cargo test 1026 passed (Phase 2)
3. Lab v15 core 22 baseline 完走 (Phase 3): `lab-v15-core22-llama-only-2026-05-05.log` 取得
4. Phase 4 結果計測 + Zone 判定 (Phase 5)
5. config 復元 (Phase 6.1)
6. CLAUDE.md 項目 189 追記 (Phase 6.2)
7. handoff + MEMORY.md 更新 (Phase 6.3)
8. commit (Phase 6.4)

## 推定所要時間

| Phase | 所要 | 実行者 |
|-------|------|--------|
| Phase 1 (config 切替) | 2 min | Claude |
| Phase 2 (cargo test) | 30s | Claude |
| Phase 3 (pure baseline) | **60-90 min** | Claude (background) |
| Phase 4 (計測) | 5 min | Claude |
| Phase 5 (Zone 判定) | 5 min | Claude (+ user 同意) |
| Phase 6 (cleanup + docs + commit) | 15 min | Claude + user 確認 |
| **総計** | **約 1.5-2h** | |

## SESSION_ID

- 本 plan は CCG review 不要 (前 plan v2 が CCG 通過済の派生、scope 限定)
- 必要時の sanity check: Codex `019def01-d210-7e00-a909-d43525f735fd` (F2 architect) 温存
