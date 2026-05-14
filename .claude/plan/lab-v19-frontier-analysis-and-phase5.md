# Lab v19 Frontier — 完走後 2 軸 Evaluation + Bucket Variance Phase 5 確証

**状態**: planning-only (2026-05-15 起票、Lab v19 実機 PID 61085 進行中)
**推奨度**: ★★★ (Lab v19 完走時点で score 軸単独判定だと第 6 軸 baseline 確立を取りこぼす)
**推定工数**: 解析 script 実装 ~2-3h (Phase 1-3)、Lab v19 完走待ち ~12-15h は本 plan 工数外
**起点**: 項目 229 frontier benchmark Phase 1-4 完遂 + 本セッション Lab v19 paired (PID 61085) 起動済
**前提**: `.claude/plan/frontier-benchmark-impl.md` §3 Phase 5 + `scripts/lab_v19_paired.sh` + `scripts/lab_v19_paired_ttest.py` (本セッション作成)

---

## §1. 背景

### 1.1 Lab v19 起動済の現状
- PID 61085 で `scripts/lab_v19_paired.sh ./lab-v19-logs` 起動済 (~12-15h wall、5 paired = 10 cycle)
- `scripts/lab_v19_paired_ttest.py` は **score 軸 Δ paired t-test** のみ対応 (`SCORE_RE` で `[lab] ベースライン: score=...` 抽出)
- 出力 log = `./lab-v19-logs/test_{on,off}_{1..5}.log` (10 file)、各 cycle 内に SQLite db_path emit (項目 229 で `experiments` table に `frontier_bucket_scores` / `frontier_inject_scores` JSON 列 + TSV 23 列の二重 persist)

### 1.2 frontier benchmark plan §3 Phase 5 の ACCEPT 基準 (再掲)
2 経路あり (`scripts/lab_v19_paired.sh` header より):
- **(a) score 軸**: mean Δscore ≥ +0.015 AND p < 0.10 (one-sided) → production default ON 候補
- **(b) variance 軸**: bucket [8K, 16K)+ で OFF baseline 比 score variance ≥ +50% 拡大 → **第 6 軸 = context-length axis baseline 確立**

`lab_v19_paired_ttest.py` は (a) のみ実装、(b) は **未実装** = 本 plan の主担当範囲。

### 1.3 第 6 軸 = context-length axis を分離 metric として確立する意義
Lab 天井 7 連続 (v8-v17 全 REJECT) で打破できなかった理由は score 軸単独判定だったため。frontier benchmark の真価は「長文 context での score 劣化曲線を可視化する」観測 metric にあり、production 採用判定は score 軸と独立して評価すべき。本 plan は (a) REJECT でも (b) ACCEPT なら「第 6 軸 observability 軸として採用」を可能にする。

---

## §2. 設計

### 2.1 新規 script `scripts/lab_v19_bucket_variance.py` (推定 ~180 行)

#### 入力
- 引数 1: `log_dir` (デフォルト `./lab-v19-logs`)
- 引数 2 (optional): `--db-path` (明示的に SQLite 指定、未指定なら log から自動抽出)
- 引数 3 (optional): `--lab-start-epoch` (計測対象 cycle 範囲、未指定なら全 cycle 内最古 timestamp)
- 引数 4 (optional): `--variance-ratio-threshold` (default 1.5、ACCEPT 基準 (b) の +50%)
- 引数 5 (optional): `--target-bucket` (default 3 = [8K, ∞)、frontier_bucket_scores 末尾 unbounded bucket)

#### 出力フォーマット (stdout)
```
=== Lab v19 Bucket Variance Analysis (target bucket 3 = [8K, ∞)) ===
SQLite db: /Users/keizo/.../experiments.db
ON  cycles found: 5  (exp_ids: [225, 227, 229, 231, 233])
OFF cycles found: 5  (exp_ids: [226, 228, 230, 232, 234])

=== Per-Cycle Bucket Scores (frontier_bucket_scores) ===
cycle    bucket_0   bucket_1   bucket_2   bucket_3
on_1     0.7820     0.7355     0.6510     0.4220
off_1    0.7905     0.7401     0.6480     0.4115
...

=== Variance Comparison (target bucket 3) ===
ON  scores: [0.4220, 0.4115, ...] var=0.00425
OFF scores: [0.4115, 0.4080, ...] var=0.00190
variance ratio (ON/OFF): 2.24

=== ACCEPT 判定 (variance 軸 = 第 6 軸 baseline 確立) ===
  (b1) ON cycle 数 >= 3: PASS  (5)
  (b2) variance ratio >= 1.5: PASS  (2.24)
  (b3) target bucket samples (sum >= 6 across cycles): PASS  (10)
  → ACCEPT (第 6 軸 = context-length axis baseline 確立)
```

#### コア logic (擬似コード)
1. **db_path 抽出**: `log_dir/test_{on,off}_{1..5}.log` を読み、`db_path=...` 行 (項目 229 `experiment_log.rs` emit) を grep
2. **lab_start_epoch 推定**: 全 log の最古 `[INFO][lab.frontier]` 行 timestamp を取得 (未指定時)
3. **SQLite query**:
   ```sql
   SELECT id, created_at, frontier_bucket_scores, frontier_inject_scores
   FROM experiments
   WHERE created_at > ? AND frontier_bucket_scores != '[]'
   ORDER BY created_at ASC;
   ```
4. **on/off 振分け**: log timestamp と experiment.created_at の隣接マッチ (各 cycle 1 baseline = 1 experiment row)
5. **bucket 集約**: `json.loads(frontier_bucket_scores)` = `[[bucket_idx, score], ...]`、target_bucket (default 3) の score 抽出
6. **variance 計算**: `statistics.variance(on_scores)` / `statistics.variance(off_scores)`
7. **ACCEPT 判定 3 段**: (b1) 各群 >= 3 cycle / (b2) ratio >= threshold / (b3) target bucket に samples 存在
8. **exit code**: 0 = ACCEPT、1 = REJECT、2 = データ不足

### 2.2 設計選択

#### A. SQLite 直接 read vs TSV parse → 採用: SQLite 直接 read
- ✅ JSON 列を `json.loads` で structured 解析可能、TSV は `'-'` placeholder 混在で fragile
- ✅ `created_at` で cycle 順序を厳密保証
- ❌ db_path 抽出に log 依存 (代替: env `BONSAI_EXPERIMENT_DB_PATH` を明示要求)

#### B. on/off 振分け方法 → 採用: log timestamp 隣接マッチ
- ✅ cycle 順序が `lab_v19_paired.sh` で固定 (on_1, off_1, on_2, off_2, ...) のため timestamp 厳密順序対応
- ❌ cycle 内で複数 experiment が emit される場合は baseline のみ採用 (lab cycle の通常動作)

#### C. 集約方式 → 採用: bucket-wise variance 比
- 各 cycle で `frontier_bucket_scores` = `[(0, s0), (1, s1), (2, s2), (3, s3)]` の 4 値を独立 score として保持
- target bucket (default 3 = [8K, ∞)) の N 件 score (on=5, off=5) で variance 比較
- 全 bucket joint variance 案は次元削減で第 6 軸顕在化の趣旨から外れる

---

## §3. 軸 1 (既存 paired_ttest) との整合性

### 3.1 2x2 マトリクス (score 軸 × variance 軸)

| score 軸 | variance 軸 | 判定 | 後続 action |
| -------- | ----------- | ---- | ----------- |
| ACCEPT   | ACCEPT      | **完全 ACCEPT** = frontier production default ON 確定 + 第 6 軸 baseline 確立 | 別 plan で `BONSAI_FRONTIER_ENABLED` default ON 切替 |
| ACCEPT   | REJECT      | score 軸単独 ACCEPT (variance 拡大なし) | production default ON 検討、context-length axis 効果は限定的 |
| REJECT   | **ACCEPT**  | **第 6 軸採用** = score 軸では棄却、variance axis baseline 確立 | observability metric として残置、default OFF 維持、Lab v20+ で variance 拡大を活用した tier-targeted 変異設計 |
| REJECT   | REJECT      | **完全 REJECT** = frontier dead-code 候補化 (Lab v17 ERL と同経路) | production code 削除 plan 起票 (項目 222 sqlite-vec wiring removal pattern) |

### 3.2 Lab v17 REJECT 時の dead-code 化との対比
- Lab v17 (項目 215) では score 軸単独 REJECT で項目 216 (`BONSAI_ERL_ENABLED` default OFF) → 項目 222-pattern wiring removal が将来候補
- 本 plan では score 軸 REJECT でも variance 軸 ACCEPT なら **明示的に第 6 軸 baseline 確立を採用判定で記録** = 観測 metric として残す価値の明確化

---

## §4. Phase 1-3 計画 (TDD strict)

### Phase 1 (Red) — 失敗 test 8 件 (`scripts/test_lab_v19_bucket_variance.py` 新規 ~120 行)
1. `test_extract_db_path_from_log` — log file から `db_path=...` 行抽出
2. `test_load_experiments_after_epoch` — SQLite `WHERE created_at > ?` 動作確認 (fixture DB 経由)
3. `test_parse_bucket_scores_json` — `[[0, 0.7820], [1, 0.7355], ...]` → `dict[int, float]` 変換
4. `test_classify_on_off_by_timestamp` — log timestamp 隣接マッチで on/off 振分け
5. `test_target_bucket_variance_ratio` — variance(on) / variance(off) 計算正確性
6. `test_accept_when_ratio_above_threshold` — ratio=2.24, threshold=1.5 → ACCEPT
7. `test_reject_when_insufficient_samples` — target bucket samples < 3 → REJECT (exit code 2)
8. `test_empty_bucket_scores_excluded` — `frontier_bucket_scores='[]'` の row は skip (default OFF cycle)

Phase 1 はすべて `NotImplementedError` 相当で失敗確証 → commit。

### Phase 2 (Green) — 実装 `scripts/lab_v19_bucket_variance.py` (推定 ~180 行)
- argparse (5 args、§2.1 入力仕様)
- `extract_db_path_from_logs(log_dir: Path) -> Path` (log grep + 全 cycle で一致確認)
- `load_experiments(db: Path, since_epoch: float) -> list[dict]` (SQLite query + JSON decode)
- `classify_on_off(experiments: list, log_timestamps: list) -> tuple[list, list]` (隣接マッチ)
- `compute_bucket_variance(experiments: list, bucket: int) -> tuple[float, float, list, list]` (on_var, off_var, on_scores, off_scores)
- `format_report(on_vals, off_vals, ratio, threshold) -> str` (§2.1 出力フォーマット)
- `main() -> int` (exit code 0/1/2)

### Phase 3 (Refactor + Runnable 化)
- `chmod +x scripts/lab_v19_bucket_variance.py`
- shebang `#!/usr/bin/env python3` + docstring (本 plan 参照リンク)
- type hints (`from __future__ import annotations`)
- error handling: SQLite file 不在 / JSON parse 失敗 / log file 不在 で stderr + exit 2
- 標準ライブラリのみ (sqlite3 / json / statistics / argparse / pathlib / re、scipy 不使用)
- `scripts/lab_v17_paired_ttest.py` の structural mirror (line layout 揃え)

### Phase 4 (Smoke) — 別 plan、本 plan scope 外
Lab v19 完走後 (~12-15h 後) に実機 SQLite で `python3 scripts/lab_v19_bucket_variance.py ./lab-v19-logs` 実行。Phase 4 自体は本 plan に含めず、項目 229 完遂後の analysis session で実施。

### Phase 5 (Phase 4 結果反映)
2x2 マトリクスに基づき follow-up plan 起票:
- ACCEPT/ACCEPT → 別 plan「frontier production default ON 切替」
- REJECT/ACCEPT → 本 plan の成果 = 第 6 軸 baseline 確立記録、CLAUDE.md 項目 231 として shipped
- REJECT/REJECT → 別 plan「frontier wiring removal」(項目 222 pattern)

---

## §5. ACCEPT/REJECT のフローチャート

```
Lab v19 完走 (PID 61085 終了、~12-15h wall)
   │
   ├── ① python3 scripts/lab_v19_paired_ttest.py ./lab-v19-logs
   │      → score 軸判定 (mean Δscore ≥ +0.015 AND p < 0.10)
   │
   ├── ② python3 scripts/lab_v19_bucket_variance.py ./lab-v19-logs
   │      → variance 軸判定 (target bucket variance_ratio ≥ 1.5)
   │
   ▼
   2x2 マトリクス判定:
   ┌─ score ACCEPT + variance ACCEPT
   │     → 完全 ACCEPT → frontier production default ON 切替 plan
   ├─ score ACCEPT + variance REJECT
   │     → 部分 ACCEPT → frontier production default ON 候補
   ├─ score REJECT + variance ACCEPT
   │     → 第 6 軸採用 → observability metric として残置
   └─ score REJECT + variance REJECT
         → 完全 REJECT → frontier wiring removal plan
```

---

## §6. 期待効果 + 仮説

### 仮説
- **H1**: ON cycle の `frontier_bucket_scores` で bucket 3 ([8K, ∞)) variance が OFF cycle 比 +50% 以上拡大
  - 反証条件: variance ratio < 1.5 → variance 軸 REJECT
- **H2**: bucket [8K, ∞)+ で OFF baseline 比 score mean が低下 (informational only、本 plan 判定外)
  - 検証条件: `mean(on, bucket 3) < mean(on, bucket 0) - 0.05`
- **H3**: 2 軸両方 ACCEPT で項目 231 = frontier production default ON 切替 plan 起票候補

### 期待効果
1. **第 6 軸 = context-length axis の baseline 確立**: Lab 天井 7 連続打破に向けた discrete frontier 軸の measurable な定義
2. **dead-code 化判断の data 化**: score 軸単独 REJECT でも variance 軸で救済できる場合の明示的判定経路
3. **後続 plan のための data**: Lab v20+ で tier-targeted 変異 (T6-LongHorizon 向け context compaction 強化等) の根拠 data

---

## §7. 起票候補項目

- **項目 231** = 本 plan の Phase 1-3 完遂 (analysis script 実装) + Lab v19 完走後の 2 軸 evaluation 実行
- **項目 232 (将来、条件付き)** = 2x2 マトリクス判定結果に基づく follow-up:
  - (ACCEPT/ACCEPT) → frontier production default ON 切替
  - (REJECT/REJECT) → frontier wiring removal (項目 222 pattern)
  - (REJECT/ACCEPT) → CLAUDE.md「Lab 天井 8 連続 + 第 6 軸 baseline 確立」記録

---

## §8. 依存

### 完遂前提
- 項目 229 (frontier benchmark Phase 1-4 完遂) ✅
- Lab v19 paired 完走 (PID 61085、~12-15h 残り) **必須**
- SQLite `experiments` table の `frontier_bucket_scores` / `frontier_inject_scores` 列 (V16 migration、項目 229 で完遂済) ✅

### 直交 plan (本 plan と並行可)
- 項目 230 KG-Grounded Hallucination Check (`.claude/plan/kg-grounded-fact-check-impl.md`) — Lab v19 と直交、並行実装可
- AgentFloor LADDER mode 配線 follow-up (項目 224 副次 finding (b))
- Pre-screen REJECT carry-over fix (項目 229 副次 finding (a))

### 不要転用 (rejected)
- scipy 依存追加 — Lab v17/v19 paired_ttest.py の標準ライブラリ縛りと整合維持
- numpy 依存 — `statistics.variance` (Python 3.4+) で十分
- Lab v17 reuse — Lab v17 は ERL pool 蓄積依存だが本 plan は cycle 内独立 (frontier metric 自体に pool 蓄積なし)

---

## §9. ロールバック戦略

### 本 plan の risk profile = 極小
- analysis script のみ追加 (`scripts/lab_v19_bucket_variance.py` + `scripts/test_lab_v19_bucket_variance.py`)
- production code 変更ゼロ (Read/Glob/Grep 経由のみ、項目 229 frontier 実装は本 plan scope 外で既に shipped)
- Lab v19 実機 run 自体は本 plan と独立 (現在進行中)

### ロールバック手順
1. script 2 ファイル削除のみ
2. Lab v19 完走後の判定は `lab_v19_paired_ttest.py` 単独 (score 軸 only) に fallback 可
3. 副作用ゼロ (SQLite read-only、production code 不変)

### 失敗時の degradation path
- script 動作不良 → manual SQL query で代替 (`sqlite3 experiments.db "SELECT frontier_bucket_scores FROM experiments WHERE ..."`)
- 第 6 軸 baseline 取得失敗 → Lab v19 (a) score 軸単独判定で fallback、Lab v20 で再試行

---

## §10. Quick Start

### 10.1 Lab v19 完走待ち (実行中)
```bash
# 進捗確認 (PID 61085)
./scripts/lab_v19_monitor.sh
tail -f /tmp/lab_v19_run.log

# 完走判定 (期待: "=== ALL CYCLES COMPLETE ===")
grep "ALL CYCLES COMPLETE" /tmp/lab_v19_run.log
```

### 10.2 解析 script 実装 (Phase 1-3、~2-3h)
```bash
# Phase 1 Red (~30 min): 失敗 test 8 件
touch scripts/test_lab_v19_bucket_variance.py
# edit ...
python3 -m pytest scripts/test_lab_v19_bucket_variance.py  # all FAIL (NotImplementedError)
git commit -m "test(lab-v19): Phase 1 Red - bucket variance analysis 8 tests"

# Phase 2 Green (~90 min): script 実装
touch scripts/lab_v19_bucket_variance.py
# edit ...
python3 -m pytest scripts/test_lab_v19_bucket_variance.py  # all PASS
git commit -m "feat(lab-v19): Phase 2 Green - bucket variance script (案 C variance 軸)"

# Phase 3 Refactor + Runnable (~30 min)
chmod +x scripts/lab_v19_bucket_variance.py
ruff check scripts/lab_v19_bucket_variance.py
git commit -m "refactor(lab-v19): Phase 3 - bucket variance script polish"
```

### 10.3 Lab v19 完走後の 2 軸 evaluation 実行
```bash
# 軸 1: score 軸 paired t-test (既存)
python3 scripts/lab_v19_paired_ttest.py ./lab-v19-logs

# 軸 2: bucket variance (本 plan)
python3 scripts/lab_v19_bucket_variance.py ./lab-v19-logs

# 2x2 マトリクス判定 (§5 flowchart 参照)
```

### 10.4 expected wall time
- Phase 1 Red: ~30 min
- Phase 2 Green: ~90 min
- Phase 3 Refactor: ~30 min
- (Lab v19 完走待ち: ~12-15h 別計算、本 plan 工数外)
- 2 軸 evaluation: ~5 min (Lab v19 完走後)
- **本 plan 合計実装時間: ~2-3h**

---

## §11. References

- `.claude/plan/frontier-benchmark-impl.md` — 項目 229 親 plan、§3 Phase 5 ACCEPT 基準 (a)(b) 定義元
- `scripts/lab_v19_paired.sh` — Lab v19 runner、header に 2 軸 ACCEPT 基準明記
- `scripts/lab_v19_paired_ttest.py` — score 軸 t-test (本 plan の bucket_variance.py の structural mirror 元)
- `scripts/lab_v19_monitor.sh` — 進捗 snapshot utility (本セッション作成)
- `scripts/lab_v17_paired_ttest.py` — Lab v17 effectiveness、df=4 t-table interp 流儀
- `src/agent/frontier.rs` — `frontier_bucket_for` + `parse_frontier_buckets_env` + `is_frontier_enabled` 純関数群
- `src/agent/experiment.rs` — `frontier_bucket_scores` / `frontier_inject_scores` Vec field
- `src/db/schema.rs` — V16 ALTER TABLE 2 TEXT 列 JSON encoded
- 項目 229 (CLAUDE.md) — frontier benchmark Phase 1-4 完遂記録
- 項目 215 (Lab v17 REJECT) — paired t-test pattern の先例 (`p=0.5072`, 天井 7 連続)
- 項目 222 (sqlite-vec wiring removal) — REJECT 後の dead-code 化 pattern
