# Plan A factcheck Trajectory Scope Expansion (Lab v20 起動前提、項目 234 後継)

**状態**: planning-only (2026-05-16 起票、G-4c v1/v2 反証 evidence-backed)
**推奨度**: ★★★ (Lab v20 effectiveness 検証 = Pearson r ≥ 0.3 の前提として必須)
**推定工数**: ~2-3h Phase 1-3 (TDD strict) + Phase 4 smoke ~25-40 min + Phase 5 = Lab v20 ~10-15h wall
**起点**:
- 項目 230 Plan A G-4c Phase 4 Smoke v1+v2 完遂 (wiring 3 度 PASS、extract 0 件)
- `.claude/plan/kg-grounded-fact-check-impl.md` (Plan A 親)
- `.claude/plan/lab-v20-kg-factcheck-effectiveness.md` (Lab v20 plan、本 plan が前提)

---

## §1. G-4c v1/v2 反証 evidence (本 plan の起点 finding)

### 1.1 G-4c v1/v2 観測 data
| run | wall | factcheck row id | total | matched | unknown | conflicting | duration_ms |
|---|---|---|---|---|---|---|---|
| G-4c v1 (旧 entity 名 小文字) | ~41 min | 9455 | 0 | 0 | 0 | 0 | 9 |
| G-4c v2 (大文字始まり化後) | ~50 min | 9534 | 0 | 0 | 0 | 0 | 7 |

### 1.2 大文字始まり化 fix は **無効** だった
- v1 LLM 出力: "The bonsai-agent is the child of bonsai-8B." (小文字、Pattern 1 regex で reject)
- v2 LLM 出力: "Bonsai-8B is the parent of Llama-3.5." (大文字 OK、Pattern 1 match 想定)
- それでも v2 で total=0 = **regex 経路に到達してない**

### 1.3 真因 = `extract_failed_trajectories_since_id` の選定軸 mismatch
`src/agent/event_store.rs:224`:
> 失敗 trajectory を抽出 (success_rate < max_tool_success_rate)

`success_rate` = **tool call の成功率** (task expected_keywords ベースの SUCCESS/FAIL とは別系統)。

| halluc task | tool 使用 | tool success rate | failed_trajectory 入り? |
|---|---|---|---|
| halluc_parent_of_false_fact (T1) | なし | 1.0 (default) | ✗ |
| halluc_is_a_false_type (T1) | なし | 1.0 (default) | ✗ |
| halluc_t2_file_context_misalign (T2) | file_read 1 件 | 1.0 (success) | ✗ |

3 task 全て tool_success_rate >= 0.8 → failed_trajectory に永続的に入らない → factcheck pass 対象テキスト 0 件 → triple 抽出 0 件 → total=0 確定。

これは Plan A §1 設計原則「false positive 回避のため failed_trajectory に絞る」と halluc task の SUCCESS-by-design 性質の **構造的排他**。

---

## §2. 設計 — 4 案比較 (推奨 = 案 A、要 user 判断)

| 案 | 概要 | 採否候補 |
|---|---|---|
| **A** | factcheck pass を全 trajectory (success + failed) に拡張、env-gated | ★★★ 推奨 |
| B | halluc task に tool error 強制 (例: 存在しない file_read) で fail 化 | ★★ 副案 |
| C | sentinel keyword で expected_keywords 不一致 → ただし `extract_failed_trajectories` は keyword じゃなく tool_success_rate 判定なので **無効** | ✗ |
| D | factcheck 専用に独立な trajectory selection (success rate 無視で全 AssistantMessage 集計) | ★ 副案、case insensitive 拡張併用 |

### 2.1 案 A (推奨): 全 trajectory 拡張
**変更**:
- `experiment.rs::run_factcheck_pass_lab` 内で `extract_failed_trajectories_since_id` 呼出を **両方 (failed + successful) 集計**に変更
- env opt-in `BONSAI_FACTCHECK_ALL_TRAJECTORIES=1` で全 trajectory 対象、unset で従来 failed-only (後方互換)
- `event_store.rs::extract_successful_trajectories_since_id` は既存 (line 247 周辺)、両方を chain 集計

**Pros**:
- Plan A §1 設計原則「false positive 回避」を `Unknown` 分類 + KG seed で防御 (現行設計上有効)
- halluc task SUCCESS でも factcheck 検証可
- env opt-in で既存挙動 100% 互換

**Cons**:
- false positive 増の可能性 (success task の自然出力で偶然 KG fact と矛盾)
- ただし Plan A §1.3 の `Unknown` 分類が KG 未収載を救う = 実害低い

### 2.2 案 B (副案): halluc task tool fail 強制
**変更**:
- halluc 3 task を全て T2 化、`expected_tools=["file_read"]` + 存在しない path で失敗させる
- 例: `Read /tmp/NON_EXISTENT_BONSAI_HALLUC_TARGET_FILE.txt and answer...`
- file_read 失敗 = tool_success_rate < 1.0 → failed_trajectory 入り

**Pros**:
- Plan A 設計変更ゼロ
- benchmark.rs の input + expected_tools 修正のみ (~30 min)

**Cons**:
- halluc task の本来の意図 (LLM の捏造観察) が tool fail に置き換わる = 設計違反
- LLM が tool fail で諦めて回答出力しないかもしれない (factcheck 対象テキスト無し)

### 2.3 案 C (棄却)
- expected_keywords は SUCCESS/FAIL 判定軸じゃない → factcheck 経路と無関係 → 効果ゼロ

### 2.4 案 D (副案): factcheck 専用 trajectory selection
**変更**:
- `event_store.rs` に `extract_all_assistant_messages_since_id(since_event_id)` 新規追加 (success_rate 無視)
- `experiment.rs::run_factcheck_pass_lab` で当該関数を呼出
- + factcheck regex の Pattern 1 を case-insensitive 化 (補助)

**Pros**:
- success_rate 軸と独立な API = factcheck 機構が trajectory selection の implementation detail に依存しない設計
- Lab v20 で sample size が最大化 = Pearson r 計算精度向上

**Cons**:
- event_store.rs の API 表面が拡大
- 既存 `extract_failed_trajectories_since_id` との関係性 docstring で明示必要

---

## §3. TDD strict 5 phase (案 A 採用想定)

### Phase 1 (Red) — 4 failing test
1. `t_factcheck_all_trajectories_env_opt_in` — env=1 で success_rate=1.0 trajectory も対象
2. `t_factcheck_default_failed_only_backwards_compat` — env unset で failed-only 維持
3. `t_factcheck_extracts_from_success_assistant_message` — SUCCESS task の AssistantMessage が triple 抽出される
4. `t_factcheck_unknown_classification_dominates_in_all_trajectories_mode` — false positive 増抑制 (`Unknown` 比率高)

### Phase 2 (Green)
- `experiment.rs::run_factcheck_pass_lab` で env-gated branch:
  - `BONSAI_FACTCHECK_ALL_TRAJECTORIES=1` → `extract_failed_trajectories_since_id` + `extract_successful_trajectories_since_id` を chain
  - unset → 従来 failed-only
- 1267 → 1271 passed (+4) 期待

### Phase 3 (Refactor)
- comment 更新 (Plan A §1 設計原則の env opt-in 拡張記述)
- clippy/fmt clean

### Phase 4 (Smoke G-5a/b/c)
| Gate | env | 期待 |
|------|-----|------|
| G-5a | unset | 1271 pass、audit_log[factcheck].total=0 (後方互換) |
| G-5b | `BONSAI_FACTCHECK_ALL_TRAJECTORIES=1` | total >= 1 (halluc task が SUCCESS でも triple 抽出) |
| G-5c | 上記 + KG seed pre-populate | matched + unknown + conflicting >= 1 (factcheck 完全動作) |

### Phase 5 (Lab v20 effectiveness)
本 plan G-5b/c PASS で Lab v20 起動、Pearson r ≥ 0.3 paired t-test (~10-15h wall、別 session)。

---

## §4. 期待効果

### Lab v20 effectiveness 検証経路確立
- 現状 `total=0` 一上限で空回り → 本 plan 後 `total >= 1` 確証 → Pearson r 算出可
- ACCEPT 基準 (b) 「ON cycle 全 5 件で `total >= 1`」が達成可能に

### Plan A §1 設計原則の維持
- env opt-in で既存挙動 100% 互換 = production default (failed-only) 不変
- Lab 専用拡張 = production agent_loop 副作用ゼロ

---

## §5. risks / mitigations
| # | Risk | Mitigation |
|---|------|-----------|
| R1 | success trajectory での false positive 増 | `Unknown` 分類が KG 未収載を救う、Plan A §1.3 設計上対応済 |
| R2 | sample size 増で audit_log 肥大 | `BONSAI_FACTCHECK_ALL_TRAJECTORIES=1` opt-in で Lab 限定 |
| R3 | halluc task が SUCCESS で「正解」を吐く可能性 (Bonsai-8B is the parent of Qwen3-8B → KG match) | `Match` も Lab v20 ACCEPT 基準には寄与 = 害なし |
| R4 | regex 改善 (case-insensitive) は本 plan scope 外 | 別 plan で検討、本 plan は trajectory 軸のみ修正 |

---

## §6. ロールバック戦略
- env opt-in default OFF (env unset で従来挙動 = 100% 後方互換)
- git revert 1 commit (Phase 2 Green) で clean rollback
- 新規 test 4 件削除でも production 影響ゼロ

---

## §7. 起票候補項目
- **項目 235** = 完遂時 (factcheck-trajectory-scope-expansion、test 1267→1271、G-5a/b/c PASS)
- **項目 236** = Lab v20 起動 (項目 230 Phase 5 effectiveness paired t-test)

---

## §8. Quick Start
```bash
# 前提確認
git log --oneline -3   # 本 plan 起点 = 5/16 session 終端
sqlite3 "/Users/keizo/Library/Application Support/bonsai-agent/bonsai.db" \
  "SELECT COUNT(*) FROM audit_log WHERE action_type='factcheck';"  # baseline=3

# Phase 1 Red — 4 failing test
$EDITOR src/agent/experiment.rs   # 新 test 4 追加
cargo test --lib t_factcheck_all_trajectories 2>&1 | tail -10  # 4 FAIL

# Phase 2 Green
$EDITOR src/agent/experiment.rs   # env-gated branch + chain extract
cargo test --lib                  # 1271 passed

# Phase 3 Refactor + clippy/fmt

# Phase 4 G-5a/b/c smoke (要 llama-server)
cargo build --release
BONSAI_LAB_SMOKE=1 BONSAI_KG_FACTCHECK_ENABLED=1 BONSAI_FACTCHECK_ALL_TRAJECTORIES=1 \
  ./target/release/bonsai --lab --lab-experiments 0 | tee /tmp/g5b.log
grep "FactCheck post-Lab" /tmp/g5b.log   # total >= 1 確証

# Phase 5 = Lab v20 起動 (別 session、~10-15h wall)
```

---

## §9. 不要転用 (rejected)
- 案 C (sentinel keyword): expected_keywords は trajectory selection と無関係、効果ゼロ
- factcheck regex case-insensitive (本 plan scope 外、別 plan)
- Plan A §1 設計原則「failed-only」全廃止 (既存挙動破壊、後方互換性なし)

---

## §10. 参考
- 項目 230 Plan A G-4c Phase 4 Smoke v1+v2 完遂 (wiring 3 度確証)
- 項目 234 候補 = 本 plan 起点 finding 記録
- `src/agent/event_store.rs:224` (`extract_failed_trajectories` docstring)
- `src/agent/experiment.rs:1597` (`run_factcheck_pass_lab` 内 caller)
- `.claude/plan/kg-grounded-fact-check-impl.md` (Plan A 親)
- `.claude/plan/lab-v20-kg-factcheck-effectiveness.md` (Lab v20 = 本 plan の Phase 5)
