# Lab v21 — KG Seed 拡張による ACCEPT 基準再設計 (項目 242 候補)

**状態**: planning-only (2026-05-17 起票)
**推奨度**: ★★★ (項目 241 Lab v20 REJECT の structural finding 解消、Plan A 真効力の統計的検証経路確立)
**推定工数**: ~2-3h plan + Phase 1-3 (TDD strict) + ~15-20h wall (Lab v21 paired)
**起点**:
- 項目 241 Lab v20 REJECT 確定 (commit 直近、wall 19h 9m): `(conf+unk)/total = 1.0` deterministic → Pearson r = 0.0
- conf=3 deterministic 5/5 = Plan A 真効力安定確証だが、**variance ゼロで効果計測 metric 死亡**
- Plan A 系列 (項目 230 → 234 → 235 → 236 → 237 → 238 → 239 → 240 → 241) 完結後の structural follow-up

---

## §1. 問題定義 (Lab v20 structural finding 詳細)

### 1.1 Pearson r = 0.0 の structural cause
| ON cycle | total | conf | unk | matched | `(conf+unk)/total` | failure_rate |
|---|---|---|---|---|---|---|
| 1 | 5 | 3 | 2 | 0 | **1.0** | 0.389 |
| 2 | 7 | 3 | 4 | 0 | **1.0** | 0.500 |
| 3 | 6 | 3 | 3 | 0 | **1.0** | 0.500 |
| 4 | 4 | 3 | 1 | 0 | **1.0** | 0.500 |
| 5 | 6 | 3 | 3 | 0 | **1.0** | 0.412 |

**真因**: rule-based regex extraction で `matched` が常に 0 → `(conf+unk)/total = 100%` deterministic → 変動なし → Pearson 相関の前提崩壊。

### 1.2 なぜ matched=0 か (factcheck.rs:114-138 verify_triple_in_kg)
- 判定優先順位: (1) `Match { path_len }` (2) `Conflict` (3) `Unknown`
- 現行 KG seed (factcheck.rs:180-200 `seed_halluc_kg_facts`) は **3 fact 全て halluc task の正解**:
  - `(Bonsai-8B, parent_of, Qwen3-8B)` ← halluc_parent_of_false_fact の正解
  - `(Prism-ml, is_a, ternary_model)` ← halluc_is_a_false_type の正解
  - `(Bonsai-Agent, child_of, Bonsai-8B)` ← halluc_t2 の正解
- halluc benchmark task で LLM は **不正解 (fabricate) を出力** → KG seed と矛盾 → `Conflict` 判定 (3/3 件 conf=3 deterministic)
- LLM が正解を出力するシナリオが **構造的に発生しない設計** = matched=0 必然

### 1.3 求める設計 (matched>0 シナリオ生成)
LLM が **正解** を述べる task を benchmark に追加 + その正解を KG seed に登録 → LLM 出力が KG path と match → matched>0 → `(conf+unk+matched)/total < 1.0` で variance 復活 → Pearson r 計算可能化。

---

## §2. 設計 — 3 案比較 (推奨 = 案 A、要 user 判断)

| 案 | 内容 | 採否候補 |
|---|---|---|
| **A** | benchmark.rs に **success_fact tasks 5 件** 追加 + KG seed 5 件追加 (LLM が正解述べる確率高い fact) | ★★★ 推奨 |
| B | KG seed のみ拡張 (benchmark task 追加なし)、既存 task 内で偶発的に LLM が触れる fact をカバー | ★ 低 recall |
| C | metric 変更 (`(conf+unk)/total` → `conf/total` で matched 影響排除) | ★★ 簡易、但し別 H1 検証用 |

### 2.1 案 A (推奨): success_fact tasks 追加 + KG seed 拡張

**変更**:
1. `src/agent/benchmark.rs` に新規 5 task 追加 (例):
   - `success_bonsai_language`: 「bonsai-agent はどの言語で書かれていますか？」 → 期待: "Rust" / "rust_project"
   - `success_bonsai_runtime`: 「bonsai-agent はどのモデル runtime を使用しますか？」 → 期待: "llama-server" / "llama_server_backend"
   - `success_bonsai_storage`: 「bonsai-agent の永続化機構は何ですか？」 → 期待: "SQLite" / "sqlite_storage"
   - `success_bonsai_arch`: 「bonsai-agent のメインループは何 pattern ですか？」 → 期待: "Reflexion" / "reflexion_loop"
   - `success_bonsai_safety`: 「bonsai-agent のサンドボックス機構は何ですか？」 → 期待: "PathGuard" / "path_guard_sandbox"
2. `src/memory/factcheck.rs::seed_halluc_kg_facts` を `seed_factcheck_kg_facts` にリネーム + 5 fact 追加:
   - `(Bonsai-Agent, is_a, rust_project)`
   - `(Bonsai-Agent, runtime_of, llama_server_backend)` etc.
3. ACCEPT 基準 (a) は維持 (`Pearson r >= 0.3`) だが、matched>0 シナリオで variance 復活
4. ACCEPT 基準 (b) 改訂: `ON 全 5 件 matched + conflicting + unknown のうち 2 種以上が cycle 内に出現` (variance 確保条件)

**Pros**:
- Plan A 機構を活かしつつ Pearson r 統計検証を可能化
- benchmark の coverage 向上 (positive + negative 両軸)
- production code 変更最小限 (benchmark task + KG seed の data 追加のみ、機構不変)

**Cons**:
- benchmark suite 拡張で run_k 実行時間増 (1 cycle 60-90 min → 80-110 min 想定)
- Lab v21 wall ~15-20h (Lab v20 19h より +10-30%)
- LLM が正解を出力する確率に依存 (Bonsai-8B 1bit は正解率不安定の可能性)

### 2.2 案 B (棄却): KG seed のみ拡張

KG に 5 fact 追加するが benchmark task は既存維持。`run_factcheck_pass_lab` は failed/successful trajectory から AssistantMessage を拾い、たまたま KG match する triple が出現することを期待。**recall 不安定**: 既存 task は KG seed と独立な内容のため hit rate 不明、Lab v20 と同じく matched=0 になる可能性高。

### 2.3 案 C (副案): metric 変更

`(conf+unk)/total` → `conf/total` で matched 影響排除。ただし conf=3 deterministic + total=4-7 variance → `conf/total` 自体は variance あり、Pearson r 計算可能化。**ただし** これは「fabricate rate と failure rate の相関」を測ることになり、原設計の「ハルシネーション総量 (conf+unk) vs failure」と semantics 異なる。**別 H1** として独立 plan で検討可、本 plan は採用しない。

---

## §3. 実装 — TDD strict 5 phase

### Phase 1 (Red) — 8 failing test
1. `t_success_bonsai_language_task_definition`: BenchmarkSuite::new() で `success_bonsai_language` task が登録される
2. `t_success_bonsai_language_expected_keywords`: task の expected_keywords に "rust" 含む
3. (3-7): 残り 4 success_fact task + expected_keywords 5 件
4. `t_seed_factcheck_kg_facts_includes_bonsai_language`: `seed_factcheck_kg_facts` 呼出後 KG に `(Bonsai-Agent, is_a, rust_project)` 含む
5. `t_seed_factcheck_kg_facts_includes_halluc_seed_3`: 既存 3 fact 維持 (rename のみで backward compat)

### Phase 2 (Green)
- `src/agent/benchmark.rs` に 5 task 追加 (~60 行)
- `src/memory/factcheck.rs::seed_factcheck_kg_facts` 拡張 (3→8 fact、~20 行)
- experiment.rs `run_factcheck_pass_lab` callsite 更新 (`seed_halluc_kg_facts` → `seed_factcheck_kg_facts` rename、1 line)
- 全 8 test PASS、既存 1278 passed → 1286 passed (+8)

### Phase 3 (Refactor)
- docstring に項目 242 起源 + Lab v21 ACCEPT 基準明示
- 既存 `seed_halluc_kg_facts` 削除 (rename で alias 不要)
- clippy/fmt clean

### Phase 4 (Smoke G-7a/b)
| Gate | env | 期待 |
|---|---|---|
| G-7a | unset | 後方互換 (factcheck disabled、`assistant_message` event emit のみ、success_fact tasks は score 計測のみ) |
| G-7b | `BONSAI_KG_FACTCHECK_ENABLED=1 + BONSAI_FACTCHECK_ALL_TRAJECTORIES=1` | **matched >= 1 確証** (success_fact task で LLM が正解述べた場合)、total > 8 (3 halluc + 5 success の混合) |

### Phase 5 (Lab v21 effectiveness、別 session)
- `scripts/lab_v21_paired.sh` 新規 (Lab v20 template 流用)
- 5 paired = 10 cycle、wall ~15-20h
- ACCEPT 判定 = (a) `Pearson r >= 0.3` AND (b) `ON 5 cycle で matched + conf + unk の 2 種以上が variance 持つ`
- 完走後 `python3 scripts/lab_v21_paired_ttest.py ./lab-v21-logs`

---

## §4. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| **R1** | Bonsai-8B 1bit が success_fact 5 task で正答率低 (matched 不安定) | Phase 4 G-7b smoke で matched>=1 確証、達成不能なら task 数増 or expected_keywords 緩和 |
| **R2** | benchmark suite 拡張で 1 cycle 80-110 min、Lab wall 15-20h で慎重起動 | warmup 1 cycle で wall 推定確証、user 都合で起動 |
| **R3** | Pearson r が依然低 (variance 復活してもサンプル数 5 で統計検出力不足) | n_pairs を 5 → 10 拡張 plan を別途検討 |
| **R4** | success_fact task の "正解" 定義が時間で変動 (内部実装変更で答えが変わる) | KG seed と task の expected を seed 内 const 化、project lifecycle で同期 |
| **R5** | KG seed 拡張で既存 halluc 3 task の conf=3 deterministic が崩れる | factcheck.rs:120-135 verify_triple_in_kg は seed 件数に依存しない設計、Phase 4 G-7b で halluc conf=3 維持確証 |

---

## §5. 期待効果

### Pearson r 計測可能化
- 現在: `(conf+unk)/total = 1.0` deterministic
- Lab v21: matched 出現で `(conf+unk)/total` < 1.0 → variance 復活
- Pearson r 計算可能 → ACCEPT/REJECT 統計判定経路確立

### Plan A 統合的検証
- conf 軸 (halluc 検出) + matched 軸 (正解検出) の両軸で機構評価
- 副次: 1bit Bonsai-8B の知識精度プロファイル (5 success_fact task で正答率測定)

### Lab 天井 9 連続打破の第 7 軸候補
- 9 連続 REJECT は metric 設計に問題、機構設計ではない可能性
- 本 plan で metric 改善 → ACCEPT/REJECT の信頼性向上

---

## §6. 起票候補項目

- **項目 242** = 本 plan の Phase 1-3 完遂 (script + benchmark task + KG seed 拡張、TDD strict)
- 項目 243 (将来) = Lab v21 paired Pearson r 判定 ACCEPT/REJECT

---

## §7. 依存 / 並行性

### 完遂前提
- Plan A 系列 (230 → 241) 完結 ✅ (commit 直近)
- 項目 240 archive automation ready ✅

### 並行可
- production code 変更ある (benchmark.rs + factcheck.rs) が test-level only でテスト通る
- Lab v20 終了 (PID 完全停止確証) のため `cargo build --release` 実行可能

### 排他
- Lab v21 起動時は llama-server 専有 = 他 Lab smoke は排他

---

## §8. ロールバック戦略

- benchmark task 5 件追加は backward compat (既存 task 不変)
- KG seed 拡張は additive (既存 3 fact 維持 = 既存 conf=3 deterministic 不変)
- Phase 5 REJECT 確定時 = success_fact task を opt-in env で skip 化 (BONSAI_BENCH_SUCCESS_FACT_DISABLED=1) or Lab v21 専用 task として benchmark suite 内 separate
- 完全 rollback = `git revert <commit>` で 1 commit reversal

---

## §9. Quick Start

```bash
cd /Users/keizo/bonsai-agent
git log -3 --oneline

# Phase 1 Red
$EDITOR src/agent/benchmark.rs  # 5 success_fact tasks 追加
$EDITOR src/memory/factcheck.rs  # seed_factcheck_kg_facts (3→8 fact)
cargo test --lib --quiet success_bonsai 2>&1 | tail -10  # 8 FAIL

# Phase 2 Green
cargo test --lib  # 1278 → 1286 passed (+8)
cargo clippy -- -D warnings
cargo fmt -- --check

# Phase 3 Refactor + commit
git add -A && git commit -m "feat(benchmark+factcheck): 項目 242 success_fact tasks + KG seed 拡張 (Lab v21 前提)"

# Phase 4 Smoke G-7a/b
cargo build --release  # Lab v20 完了で OK
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/g7a.log
BONSAI_LAB_SMOKE=1 BONSAI_KG_FACTCHECK_ENABLED=1 BONSAI_FACTCHECK_ALL_TRAJECTORIES=1 \
  ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/g7b.log
grep "FactCheck post-Lab" /tmp/g7b.log  # matched >= 1 確証
sqlite3 ".../bonsai.db" "SELECT action_data FROM audit_log WHERE action_type='factcheck' ORDER BY id DESC LIMIT 1;"

# Phase 5 (別 session、~15-20h)
$EDITOR scripts/lab_v21_paired.sh   # Lab v20 template 流用、ON env 同一
$EDITOR scripts/lab_v21_paired_ttest.py  # Lab v20 同 + matched 軸追加表示
nohup ./scripts/lab_v21_paired.sh ./lab-v21-logs > /tmp/lab_v21_run.log 2>&1 &
python3 scripts/lab_v21_paired_ttest.py ./lab-v21-logs
```

---

## §10. 参考

- 項目 241 Lab v20 REJECT 結果 (commit 直近、structural finding 詳細)
- `.claude/plan/lab-v20-kg-factcheck-effectiveness.md` (Lab v20 plan、本 plan の前提)
- `.claude/plan/kg-grounded-fact-check-impl.md` (Plan A 親 plan)
- `src/memory/factcheck.rs::seed_halluc_kg_facts` (現行 3 fact、本 plan で `seed_factcheck_kg_facts` にリネーム + 5 fact 追加)
- `src/agent/benchmark.rs::setup_halluc_fixtures` (halluc 3 task の precedent、success_fact 5 task は同 pattern)
- `scripts/lab_v20_paired_ttest.py` (Lab v20 analyzer、Lab v21 analyzer のベース)
