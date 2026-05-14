# Lab v20 — KG-Grounded Hallucination Check Effectiveness (項目 230 Phase 5 / Pearson r ≥ 0.3)

**状態**: planning-only (2026-05-15 起票)、推奨度 ★★★、推定工数: script delivery ~3.25h (Phase 1-3) + Phase 5 実機 ~10-15h wall (別 session、llama-server 排他)
**起点**: Plan A KG fact-check Phase 1-4 wiring 完遂 (本セッション commits `ec29e4d` / `a5fd066` / 直近)、`.claude/plan/kg-grounded-fact-check-impl.md` §3 Phase 5 で Pearson r ≥ 0.3 明示

---

## §1. 背景

### Plan A 到達点
- `src/memory/factcheck.rs` 新規 (Triple/FactCheckResult/FactCheckSummary + 3 fn + run_factcheck_pass + 8 test)
- `src/memory/graph.rs` 拡張 (contains_triple + find_conflicting_edges)
- `src/agent/experiment.rs::run_factcheck_pass_lab` AgentHER 直前 hook 配線 (env-gated `is_factcheck_enabled()`)
- `src/observability/audit.rs::AuditAction::FactCheck` variant
- `[INFO][lab.factcheck] FactCheck post-Lab: total=N matched=N unknown=N conflicting=N mean_path_len=X.XX` emit 確証

### 残課題 (Plan A Phase 5 委譲)
- effectiveness 未検証 = hallucination 検出能力獲得の実機 evidence 未取得
- `conflicting + unknown` と `failed_sessions / (failed_sessions + successful_sessions)` の相関は実機 paired データなしで判定不可

### Lab paired pattern 参照
| Lab | 主題 | 結果 | sample |
|---|---|---|---|
| v17 | ERL Heuristics Pool (項目 215) | **REJECT** (mean Δ=−0.0014 / p=0.5072) | 5 paired = 12 cycle、warm-up 2 必須 |
| v19 | Frontier (項目 229 Phase 5) | 進行中 (本セッション起動 PID 61085) | 5 paired = 10 cycle、warm-up 不要 |
| **v20** | **本 plan KG-FactCheck** | **未起動** | **5 paired = 10 cycle、warm-up 不要** |

Lab v19 paired pattern (`scripts/lab_v19_paired.sh`) template に env knob を `BONSAI_KG_FACTCHECK_ENABLED` 置換、**Pearson r 算出を主指標** + paired Δscore を副次併用。

---

## §2. 設計

### Toggle 機構 (既存、追加実装なし)
Plan A Phase 1-4 で `BONSAI_KG_FACTCHECK_ENABLED` 実装済 (production default OFF)。Lab variant: `BONSAI_KG_FACTCHECK_ENABLED=1 ./target/release/bonsai --lab ...`。

### Warm-up 不要の根拠
- factcheck は pool 蓄積を伴わない (KG は `MemoryStore::save_memory` 経由で別途蓄積、factcheck pass は read-only)
- `lab_start_event_id` で cycle 跨ぎ汚染なし
- 5 paired = 10 cycle、各 ~60-90 min、計 ~10-15h wall

### ACCEPT 基準 (主条件 AND)
| 条件 | 内容 |
|---|---|
| **(a)** | **Pearson r ≥ 0.3** (ON 5 cycle の `(conflict_rate + unknown_rate)` vs `failure_rate` 相関) |
| **(b)** | **ON cycle 全 5 件で `total >= 1`** (fact-check が triple 抽出 + KG 検証で実発火) |

**副次観察 (informational only)**: paired Δscore + paired t-test (Lab v17 同) — factcheck は post-hoc metric で score 寄与なし設計の実機確認

### 判定マトリクス
| 結果 | 判定 | 帰結 |
|---|---|---|
| (a) AND (b) | **ACCEPT** | H_factcheck 採用、Lab v21+ で retry hook 案 A 試行検討 |
| (a) NG (r < 0.3) | **REJECT** | defaults OFF 維持 → 項目 222 pattern で wiring removal 候補 |
| (b) NG (total=0 多発) | **REJECT (extraction 不足)** | LLM-based triple extraction フォールバック plan 起票 (Plan A §3 Phase 2 で延期した案) |
| 両 NG | **REJECT (full)** | Plan A 全機構の段階削除 plan |

---

## §3. 実装 scripts

### `scripts/lab_v20_paired.sh` (新規 ~75 行、Lab v19 template 流用、env knob のみ差し替え)
- ON: `BONSAI_KG_FACTCHECK_ENABLED=1 "$BONSAI_BIN" --lab --lab-experiments 0`
- OFF: `unset BONSAI_KG_FACTCHECK_ENABLED`
- 5 paired = 10 cycle、`BONSAI_BENCH_TIER=core` (22 task)

### `scripts/lab_v20_paired_ttest.py` (新規 ~150 行)
- Lab v17 ttest template + Pearson r 拡張
- 主要新規 regex:
  ```python
  FACTCHECK_RE = re.compile(r"FactCheck post-Lab:\s+total=(\d+)\s+matched=(\d+)\s+unknown=(\d+)\s+conflicting=(\d+)\s+mean_path_len=([0-9.]+)")
  AGENTHER_RE  = re.compile(r"AgentHER post-Lab:\s+failed=(\d+)\s+successful=(\d+)\s+relabels=(\d+)\s+skills=(\d+)\s+insights=(\d+)")
  ```
- 主要新規関数 (scipy 不使用、stdlib math のみ):
  ```python
  def pearson_r(xs, ys):
      n = len(xs)
      if n != len(ys) or n < 2: return 0.0
      mx, my = sum(xs)/n, sum(ys)/n
      cov = sum((x-mx)*(y-my) for x,y in zip(xs,ys))
      vx = sum((x-mx)**2 for x in xs); vy = sum((y-my)**2 for y in ys)
      if vx <= 0 or vy <= 0: return 0.0
      return cov / math.sqrt(vx * vy)
  ```
- 集計フロー: ON 5 cycle で `(conflict+unknown)/total` と `failed/(failed+successful)` 抽出 → `pearson_r()` → ACCEPT/REJECT 判定

CLI: `python3 scripts/lab_v20_paired_ttest.py ./lab-v20-logs [--accept-r 0.3]`

---

## §4. TDD strict 5 phase

本 plan は **production code 変更ゼロ** (Plan A Phase 1-4 で配線済)、script 単体に TDD 適用。

### Phase 1 (Red) — `tests/scripts/test_lab_v20_paired_ttest.py` 5 test
1. `t_pearson_r_perfect_positive`: r([1,2,3], [2,4,6]) == 1.0
2. `t_pearson_r_zero_variance`: zero variance で 0.0
3. `t_pearson_r_negative_correlation`: r([1,2,3], [3,2,1]) == -1.0
4. `t_extract_factcheck_summary_parses_log`: regex 抽出
5. `t_accept_judgment_pearson_above_threshold_and_all_fired`: 判定ロジック

### Phase 2 (Green)
- `scripts/lab_v20_paired_ttest.py` (~150 行) + `scripts/lab_v20_paired.sh` (~75 行)
- 5 test PASS、production code touch なし

### Phase 3 (Refactor)
- docstring に Plan A §3 Phase 5 ACCEPT 基準明記
- `chmod +x scripts/lab_v20_paired.sh`

### Phase 4 (Smoke G-4)
- **G-4a** (env unset = default OFF): 1 cycle 実機で既存挙動互換確証 (`grep "lab.factcheck"` で hit ゼロ)
- **G-4b** (`BONSAI_KG_FACTCHECK_ENABLED=1` + SMOKE): `total >= 1` 確認
- **G-4c** (smoke + hallucination-inducing task): `conflicting + unknown >= 1` 期待、benchmark.rs に false-fact task 追加 (§7 TODO、別 plan)

### Phase 5 (Effectiveness、別 session)
1. `nohup ./scripts/lab_v20_paired.sh ./lab-v20-logs > /tmp/lab_v20_run.log 2>&1 &`
2. ~10-15h 後 `python3 scripts/lab_v20_paired_ttest.py ./lab-v20-logs`
3. ACCEPT/REJECT 判定 → CLAUDE.md 項目 232

---

## §5. risks / mitigations

| # | Risk | Mitigation |
|---|---|---|
| **R1** | regex extract recall ~5% で `total = 0` 多発 | Phase 4 G-4b で `total >= 1` 確認、不足なら LLM extraction フォールバック plan |
| **R2** | n=5 で Pearson r 信頼区間広い | Lab v17 と同 sample size で comparable、必要なら n=10 拡張 |
| **R3** | KG が未網羅で false negative 多発 | `Unknown` 分類で false positive 回避済、外部 KG (Wikidata 等) 連携は別 plan |
| **R4** | ON/OFF 順序効果 (lock 待ち時間) | ACCEPT 主条件は Pearson r、duration 影響は副次観察 |
| **R5** | hallucination task preset 不足で total>=1 不安定 | Phase 4 G-4c 前に hallucination task 1-2 件追加 (§7 TODO) |

---

## §6. 期待効果 + 仮説

- **H1**: bonsai-8B 失敗 trajectory の 30% 以上に `Conflict` triple 含む → 反証: conflicting rate < 0.10 で REJECT
- **H2**: `Unknown` rate は task カテゴリと相関 (一般知識 > tool-use) → 反証: 差 < 0.05
- **H3**: Lab v20 で fact-check ON は failure rate 検出能力 +20% → 反証: r < 0.3

### F11 反証経路
- r < 0.3 by extraction 不足 (total=0 多発) → LLM extraction フォールバック plan (~6-8h)
- r < 0.3 by 真因独立 (total>=1 だが相関なし) → Plan A 機構の段階削除 plan (項目 222 pattern、~3h)

---

## §7. 起票候補項目 + TODO

- **項目 231** = 本 plan の Phase 1-3 完遂 + Phase 4 G-4a/b Smoke (script delivery)
- **項目 232** (将来) = Lab v20 paired Pearson r 判定 ACCEPT/REJECT

### TODO (本 plan §4 Phase 4 G-4c 前提)
- **hallucination-inducing task 追加**: Plan A §3 Phase 4 G-4c で defer された task (Bonsai-8B vs GPT-5 系 false fact prompt) を benchmark.rs に追加。~1h、別 plan or 本 plan G-4c。

---

## §8. 依存 / 並行性

### 完遂前提
- Plan A Phase 1-4 全完遂 (commits `ec29e4d` / `a5fd066` / 直近) ✅
- 項目 215 Lab v17 paired pattern (template) ✅
- 項目 229 Lab v19 frontier paired pattern (warm-up 不要先行例) ✅

### 排他あり (llama-server 単独排他)
- Lab v18 (G1 Critic) / Lab v19 (frontier 進行中) / Lab v20 (本 plan) は逐次実行

### 排他なし (並行可)
- production code touch ゼロのため code 変更系 plan (AgentFloor LADDER wiring / hallucination task 追加) と並行可

---

## §9. ロールバック戦略

- production default OFF (`BONSAI_KG_FACTCHECK_ENABLED` 未設定で no-op、Plan A Phase 1-4 で確証済)
- 本 plan は production code 変更ゼロ = `git revert <script commit>` で完全 rollback
- Phase 5 REJECT 確定時 = Plan A 機構を env opt-in のまま放置 (項目 213 同 pattern)
- REJECT + Lab v21+ で dead-code 判定なら別 plan で wiring removal (項目 216/222 pattern)

---

## §10. Quick Start

```bash
cd /Users/keizo/bonsai-agent
git log -3 --oneline
cargo test --lib factcheck --release 2>&1 | tail -5

# Phase 1 Red
mkdir -p tests/scripts && $EDITOR tests/scripts/test_lab_v20_paired_ttest.py
pytest tests/scripts/test_lab_v20_paired_ttest.py 2>&1 | tail -10

# Phase 2 Green
$EDITOR scripts/lab_v20_paired_ttest.py && $EDITOR scripts/lab_v20_paired.sh
pytest tests/scripts/test_lab_v20_paired_ttest.py

# Phase 3 Refactor
chmod +x scripts/lab_v20_paired.sh

# Phase 4 Smoke G-4a/b (release build 必須、Lab v19 完走後)
cargo build --release
BONSAI_KG_FACTCHECK_ENABLED=1 BONSAI_LAB_SMOKE=1 \
  ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/lab_v20_smoke.log
grep "lab.factcheck" /tmp/lab_v20_smoke.log

# Commit
git add -A && git commit -m "feat(lab): Lab v20 KG-FactCheck effectiveness harness (項目 231 / scripts only)"

# Phase 5 (別 session、10-15h)
nohup ./scripts/lab_v20_paired.sh ./lab-v20-logs > /tmp/lab_v20_run.log 2>&1 &
python3 scripts/lab_v20_paired_ttest.py ./lab-v20-logs
```

---

## §11. 参考

- `.claude/plan/kg-grounded-fact-check-impl.md` (Plan A 前提、§3 Phase 5 起源)
- `.claude/plan/lab-v17-erl-effectiveness.md` (paired pattern template)
- `scripts/lab_v17_paired_ttest.py` (Pearson r 拡張流用源)
- `scripts/lab_v19_paired.sh` (warm-up 不要 pattern 先行例、本セッション作成)
- `src/memory/factcheck.rs` (Plan A 実装、`FactCheckSummary` 5 field)
- `src/agent/experiment.rs:1456-1477` (factcheck post-Lab hook 配線)
- 項目 215 Lab v17 REJECT (`mean Δ=−0.0014 / p=0.5072`)
- 項目 229 Lab v19 frontier (進行中、PID 61085)
- arxiv 2603.03303 HumanLM (EidoGraph confidence/weight 二軸起源)
- Zenn 井本 賢「LLM の隣にファクトチェック係を置く」(2026-05-06、usecase #7)
- Zenn edom18「EidoGraph」(2026-05-09、confidence/weight 二軸分離)
