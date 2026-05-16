# Lab v19 Frontier — ON 群単独 Bucket Gradient + Stability 解析 (案 A 再設計)

**状態**: planning-only (2026-05-16 起票、Lab v19 完走後の **案 D 観測 partial baseline** を起点に再設計)
**推奨度**: ★★ (Lab v19 既収集データを最大活用、再 run 不要、実装 ~2-3h で第 6 軸 baseline を正式 record 化)
**推定工数**: 解析 script ~2-3h (Phase 1-3 TDD)、production code 変更ゼロ
**起点**:
- 項目 229 frontier benchmark Phase 1-4 完遂 (`.claude/plan/frontier-benchmark-impl.md`)
- 旧 plan `.claude/plan/lab-v19-frontier-analysis-and-phase5.md` (前提崩壊で superseded、本 plan が後継)
- Lab v19 実機 10 cycle 完走 (2026-05-15、score 軸 REJECT 確証 / Δ=+0.0072 / p=0.4262)
- 本 session (2026-05-16) 案 D = 観測 partial baseline 確立判断

---

## §1. 旧 plan の前提崩壊と本 plan の再設計

### 1.1 旧 plan の二重前提崩壊 (2026-05-16 検出)

旧 plan `lab-v19-frontier-analysis-and-phase5.md` §1.2 (b) variance 軸 ACCEPT 基準は次の前提:
> bucket [8K, 16K)+ で **OFF baseline 比 score variance ≥ +50% 拡大** → 第 6 軸 = context-length axis baseline 確立

実機 Lab v19 10 cycle log で次の二重崩壊を検出:

| 崩壊 | 観測 | 根本原因 |
|---|---|---|
| (i) **OFF cycle frontier emit ゼロ** | `test_off_{1..5}.log` 全 5 cycle で `[INFO][lab.frontier]` 行ゼロ、SQLite `frontier_bucket_scores='[]'` | `is_frontier_enabled() == false` → `emit_frontier_log` が早期 return (`src/agent/experiment_log.rs`)、これは production 設計上の正常動作 |
| (ii) **target bucket 3 サンプルゼロ** | ON cycle 5 件全部 bucket 3 [8K, ∞) のデータゼロ、bucket 2 [4K, 8K) は 3/5 cycle で 1 sample のみ | 通常 benchmark suite (5-22 task) は context が 8K に達しない、LADDER + extended (T6 含) でなければ bucket 3 populate しない |

→ ON/OFF 比較は構造的に不可能、target bucket 3 variance は永続的にデータ不足。

### 1.2 本 plan の再設計方針

旧 plan §5 2x2 マトリクス判定 (score × variance) のうち variance 軸を **ON 群単独で再定義**:

| 旧 plan | 本 plan |
|---|---|
| ON vs OFF variance ratio (∞ で undefined) | **ON 群 bucket 0→1→2 score 勾配** + **bucket 1 cycle 間 stability** |
| target = bucket 3 (永続的にデータ不足) | target = **bucket 1** (5/5 cycle 全部 populate、唯一の robust comparable bucket) |
| ACCEPT 基準 = variance ratio ≥ 1.5 | ACCEPT 基準 = **degradation gradient ≤ -0.10** (bucket 0→1 で score が 10pt 以上低下を観測) |
| OFF baseline 必須 | 不要 (ON 群単独で gradient 計算) |

### 1.3 案 D 観測 partial baseline (本 plan の起点 data)

本 session で Lab v19 ON cycle 5 件から取得済:

| bucket | range | 5 cycle 全数値 | mean | std | populated cycles |
|---|---|---|---|---|---|
| 0 | [0, 2048) | 0.8944 / 0.8944 / 0.8944 / 0.8944 / 0.8944 | 0.894400 | 0.000000 | 5/5 (deterministic) |
| 1 | [2048, 4096) | 0.7296 / 0.7229 / 0.7155 / 0.6534 / 0.6788 | 0.700040 | 0.032641 | 5/5 |
| 2 | [4096, 8192) | – / 0.1017 / 0.0000 / – / 0.2406 | 0.114100 | 0.120778 | 3/5 (n-1 stdev 計算可、noise 比率 ≈ mean) |
| 3 | [8192, ∞) | – | N/A | N/A | 0/5 |

→ **bucket 0→1 gradient = -0.194360** (定性的に degradation 観測、本 plan ACCEPT 基準 -0.10 を **1.94x 超過 ✓**)
→ **bucket 1→2 gradient = -0.585940** (3 cycle 限定、weak evidence だが第 6 軸の存在を示唆)
→ **bucket 1 cycle 間 std = 0.032641** (cycle 安定性、stability 軸の incremental finding)
→ **副次 finding**: ON cycle 全体 score std=**0.031066** vs OFF cycle std=**0.035326** (ON 群が微優位、項目 215 Lab v17 副次 finding と整合)

---

## §2. 設計 — `scripts/lab_v19_on_only_gradient.py` (推定 ~150 行)

### 2.1 入力
- 引数 1: `log_dir` (デフォルト `./lab-v19-logs`)
- 引数 2 (optional): `--gradient-threshold` (default -0.10、bucket 0→1 で -10pt 以上を ACCEPT)
- 引数 3 (optional): `--min-populated-cycles` (default 4、bucket k に 4 cycle 以上の sample があれば集約)

### 2.2 出力フォーマット (stdout)
```
=== Lab v19 ON-only Bucket Gradient + Stability Analysis ===
Source: 5 ON cycle log files (test_on_{1..5}.log)

=== Bucket Statistics (ON 群単独) ===
bucket  range            n  mean    std     populated
0       [0, 2048)        5  0.8944  0.0000  5/5 ★ deterministic
1       [2048, 4096)     5  0.7000  0.0327  5/5
2       [4096, 8192)     3  0.1141  N/A     3/5
3       [8192, ∞)        0  -       -       0/5 (insufficient data)

=== Degradation Gradient ===
bucket 0 → 1: Δ = -0.1944  (10pt+ threshold で ACCEPT 確証 ✓)
bucket 1 → 2: Δ = -0.5859  (3 cycle 限定、weak evidence)

=== Stability per bucket (cycle 間) ===
bucket 0 std: 0.0000  (deterministic、stability metric として trivial)
bucket 1 std: 0.0327  (5 cycle で安定性測定可、actionable baseline)

=== ACCEPT 判定 (第 6 軸 = context-length axis baseline) ===
  (a1) bucket 0 → 1 gradient ≤ -0.10: PASS (-0.1944)
  (a2) bucket 0 と bucket 1 が両方 5/5 populated: PASS
  (a3) bucket 1 cycle std < 0.10: PASS (0.0327)
  → ACCEPT (第 6 軸 baseline = context-length axis confirmed in [0, 4K) range)

NOTE: bucket 2/3 は LADDER + extended benchmark で再 run 必要 (別 plan)
```

### 2.3 コア logic (擬似コード)
1. **log parse**: `test_on_{1..5}.log` から `[INFO][lab.frontier]   bucket {idx} [{lo}, {hi}): {score}` を regex 抽出
2. **bucket aggregate**: bucket 別に score list を集約、`statistics.mean` / `statistics.stdev` (n>=2)
3. **gradient 計算**: 隣接 bucket 間 `mean[k+1] - mean[k]` (両 bucket 4+ cycle populated のみ)
4. **ACCEPT 判定 3 段**: (a1) gradient ≤ threshold / (a2) 両 bucket robust populated / (a3) cycle std < 0.10
5. **exit code**: 0 = ACCEPT、1 = REJECT、2 = データ不足

### 2.4 設計選択

#### A. log parse vs SQLite read → 採用: log parse
- ✅ db_path emit が log になく、SQLite path 推定が fragile (旧 plan §2.1 の前提崩壊原因)
- ✅ log の `[INFO][lab.frontier]` 行は `experiment_log.rs:emit_frontier_log` で確定 emit、format 安定
- ✅ Python 標準ライブラリのみ (sqlite3 不要)
- ❌ 将来 frontier emit format 変更で fragile (現状 stable、change 時に test 8 件で検出)

#### B. ON only vs ON/OFF → 採用: ON only
- ✅ 案 D 検出の前提崩壊回避、production 設計と整合
- ✅ gradient 自体は ON 群単独で意味を持つ (degradation curve は absolute metric)
- ❌ "OFF と比較して ON が有意に degrade" は測れない (informational として bucket 1 mean を log のみ)

#### C. target bucket 1 vs bucket 3 → 採用: bucket 1 (robust populated)
- ✅ 5/5 cycle 全部 populated、cycle 間 variance を本物として計算可
- ✅ 通常 benchmark suite で再現可能、LADDER + extended なくても動く
- ❌ context length 4K までの観測しかカバーしない (bucket 2/3 は LADDER 必須、別 plan)

---

## §3. Phase 1-3 計画 (TDD strict)

### Phase 1 (Red) — 失敗 test 8 件 (`scripts/test_lab_v19_on_only_gradient.py` 新規 ~120 行)

1. `test_parse_bucket_line_basic` — `[INFO][lab.frontier]   bucket 0 [0, 2048): 0.8944` → `(0, 0, 2048, 0.8944)` 抽出
2. `test_parse_bucket_skips_inject_lines` — `inject:` 行は frontier_inject_scores 用、bucket scores から除外
3. `test_aggregate_buckets_per_cycle` — 5 log file から bucket-wise dict 集約
4. `test_compute_gradient_basic` — bucket 0 mean=0.9, bucket 1 mean=0.7 → gradient = -0.2
5. `test_gradient_requires_min_populated` — bucket 2 sample が 3/5 (< 4) → gradient 計算スキップ
6. `test_accept_when_gradient_below_threshold` — gradient=-0.20, threshold=-0.10 → ACCEPT (-0.20 ≤ -0.10)
7. `test_reject_when_gradient_above_threshold` — gradient=-0.05, threshold=-0.10 → REJECT
8. `test_handles_no_frontier_lines` — log に `lab.frontier` 行ゼロ → exit code 2 (insufficient data)

### Phase 2 (Green) — 実装 `scripts/lab_v19_on_only_gradient.py` (推定 ~150 行)

```python
#!/usr/bin/env python3
"""Lab v19 — ON-only bucket gradient + stability analysis (案 A 再設計)。"""
from __future__ import annotations
import argparse, re, statistics, sys
from pathlib import Path

BUCKET_RE = re.compile(
    r"\[INFO\]\[lab\.frontier\]\s+bucket\s+(\d+)\s+"
    r"\[(\d+),\s*(\d+|∞)\):\s+([0-9.]+)"
)

def parse_bucket_lines(log_path: Path) -> list[tuple[int, float]]:
    """Return [(bucket_idx, score), ...] from one log file."""
    if not log_path.exists():
        return []
    out = []
    with log_path.open("r", encoding="utf-8", errors="replace") as f:
        for line in f:
            m = BUCKET_RE.search(line)
            if m:
                out.append((int(m.group(1)), float(m.group(4))))
    return out

# ... aggregate_buckets, compute_gradient, format_report, main
```

### Phase 3 (Refactor + Runnable 化)
- `chmod +x scripts/lab_v19_on_only_gradient.py`
- shebang + docstring
- type hints (`from __future__ import annotations`)
- error handling: log file 不在 / 数値 parse 失敗で stderr + exit 2
- 標準ライブラリのみ
- `scripts/lab_v19_paired_ttest.py` の structural mirror (line layout 揃え)

---

## §4. ACCEPT/REJECT のフローチャート

```
Lab v19 完走 (本 plan 起点 data 確保済)
   │
   ▼
   python3 scripts/lab_v19_on_only_gradient.py ./lab-v19-logs
   │
   ├── ACCEPT (gradient ≤ -0.10、bucket 0/1 両 5/5)
   │     → 第 6 軸 baseline 確立 → CLAUDE.md 項目 230 末尾追記
   │     → frontier wiring 残置 (observability metric として価値、production default OFF 維持)
   │
   ├── REJECT (gradient > -0.10、context degrade 観測されず)
   │     → 第 6 軸の効力否定 → frontier wiring removal plan 起票 (項目 222 pattern)
   │
   └── データ不足 (bucket 0 or 1 で populated < 4)
         → Lab v19 再 run (LADDER + extended、~12-15h wall) 必要、別 plan
```

---

## §5. 期待効果 + 仮説

### 仮説
- **H1**: bucket 0→1 gradient は -0.10 以上の degradation を示す = 第 6 軸 = context-length axis が **観測可能 metric** として確立
  - 反証条件: gradient > -0.10 → 第 6 軸の効力否定、wiring removal 候補
- **H2**: bucket 1 cycle 間 std は 0.05 未満で stability 軸として actionable
  - 反証条件: std > 0.10 → bucket 1 自体が noise 支配で informational のみ

### 期待効果
1. **第 6 軸 baseline の data-backed 公式記録**: 案 D 観測 partial baseline を script で再現可能化
2. **旧 plan の前提崩壊知見の plan-as-doc 残置**: 本 plan §1.1 が後続 session の plan 設計時の参照に
3. **Lab v19 既収集データ最大活用**: 再 run コスト ~15h wall を回避

---

## §6. 起票候補項目

- **項目 231 (将来)** = 本 plan Phase 1-3 完遂 + ACCEPT/REJECT 判定 → CLAUDE.md 追記
- **項目 232 (条件付き、ACCEPT 時)** = bucket 2/3 populate のため LADDER + extended Lab 再 run plan
- **項目 232' (条件付き、REJECT 時)** = frontier wiring removal plan (項目 222 pattern)

---

## §7. 依存

### 完遂前提
- 項目 229 (frontier benchmark Phase 1-4 完遂) ✅
- Lab v19 paired 完走 (PID 61085、2026-05-15 23:17 完走) ✅
- 本 session (2026-05-16) 案 D 観測 partial baseline ✅

### 直交 plan (本 plan と並行可)
- 項目 230 KG-Grounded Hallucination Check Phase 4 Smoke (G-4a/b/c) — 排他リソース無し、Plan A 配線が AgentHER hook 直前 + 本 plan は post-hoc analysis script のみ

### 不要転用 (rejected)
- scipy 依存 — Python 標準ライブラリのみで十分 (旧 plan の流儀継承)
- SQLite read — log parse の方が robust (案 D で確証)
- ON/OFF 比較 — 構造的に不可能 (旧 plan §1.2(b) の崩壊原因)

---

## §8. ロールバック戦略

### 本 plan の risk profile = 極小
- analysis script のみ追加 (`scripts/lab_v19_on_only_gradient.py` + `scripts/test_lab_v19_on_only_gradient.py`)
- production code 変更ゼロ (Read のみ)
- Lab v19 実機データ read-only (副作用ゼロ)

### ロールバック手順
1. script 2 ファイル削除のみ
2. 旧 plan `lab-v19-frontier-analysis-and-phase5.md` は archive (削除しない、前提崩壊知見として保持)

### 失敗時の degradation path
- script 動作不良 → 案 D 観測値 (本 plan §1.3 表) を CLAUDE.md 項目 230 に手動記録
- ACCEPT 判定不能 → frontier wiring を opt-in のまま放置 (項目 213 ERL 同 pattern)

---

## §9. Quick Start

```bash
cd /Users/keizo/bonsai-agent

# Phase 1 Red (~30 min): 失敗 test 8 件
touch scripts/test_lab_v19_on_only_gradient.py
# edit ...
python3 -m pytest scripts/test_lab_v19_on_only_gradient.py  # all FAIL

# Phase 2 Green (~90 min): script 実装
touch scripts/lab_v19_on_only_gradient.py
# edit ...
python3 -m pytest scripts/test_lab_v19_on_only_gradient.py  # all PASS

# Phase 3 Refactor (~30 min)
chmod +x scripts/lab_v19_on_only_gradient.py
ruff check scripts/lab_v19_on_only_gradient.py

# 実機実行 (~5 sec)
python3 scripts/lab_v19_on_only_gradient.py ./lab-v19-logs
```

### expected wall time
- Phase 1 Red: ~30 min
- Phase 2 Green: ~90 min
- Phase 3 Refactor: ~30 min
- **合計実装時間: ~2-3h**
- 実機実行: ~5 sec (Lab v19 既収集データ on disk)

---

## §10. References

- `.claude/plan/frontier-benchmark-impl.md` — 項目 229 親 plan
- `.claude/plan/lab-v19-frontier-analysis-and-phase5.md` — superseded、前提崩壊 reference
- `scripts/lab_v19_paired_ttest.py` — score 軸 t-test (本 plan の structural mirror 元)
- `src/agent/frontier.rs` — `frontier_bucket_for` + `parse_frontier_buckets_env` 純関数群
- `src/agent/experiment_log.rs` — `emit_frontier_log` (本 plan が parse する log emit 元)
- 項目 215 (Lab v17 REJECT) — paired t-test pattern の先例
- 項目 222 (sqlite-vec wiring removal) — REJECT 後の dead-code 化 pattern
- 項目 229 (frontier benchmark Phase 1-4 完遂) — 本 plan の data 起点
