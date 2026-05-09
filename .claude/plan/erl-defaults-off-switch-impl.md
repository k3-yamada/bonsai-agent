# ERL Heuristics Pool defaults OFF 切替 — TDD strict 5 phase 実装 plan

**作成日**: 2026-05-09
**plan id**: `erl-defaults-off-switch`
**親**: 項目 213 (ERL Phase 2 Green) / 項目 214 (toggle 機構) / 項目 215 (Lab v17 REJECT)
**推定**: ~3h (plan 起票 ~30 min / TDD 5 phase ~2h / smoke ~30 min)
**production 動作**: default ON → default OFF (heuristics 注入 + post-Lab pass 全 skip)

---

## 1. 背景

### 1.1 Lab v17 effectiveness 検証 結果 (項目 215、2026-05-09)
- 12 cycle 完走 (warmup 2 + test 5 paired)、core 22 / k=3、15h 37min wall
- 5 paired Δscore (ON−OFF): −0.0255 / −0.0022 / +0.0590 / +0.0179 / −0.0562
- **paired t-test**: ON mean=0.7007 / OFF mean=0.7021 / Δ mean=**−0.0014** / t=**−0.0718** (df=4) / one-sided p=**0.5072**
- **ACCEPT 基準** (a) Δ≥+0.015 NG / (b) p<0.1 NG → **両条件未達で REJECT 確定**
- 天井 7 連続確定 (v8/v9/v10/v14/v15/v16/v17、prompt+config+context level の構造変異全失敗)

### 1.2 副次 finding (項目 215 §副次)
- ON pair 1-4 variance std≈0.010 vs OFF std≈0.034 で **stability 軸 ON 顕著優位**
- ただし ON pair 5 急落 (0.6518) で **pool 成熟による陳腐化** = Cerememory ADR-011 命題の実機実証
- → Plan B (ReviewState V12 freshness gate) で別軸対応予定

### 1.3 H_ERL 仮説棄却の帰結
- arxiv 2603.24639 ERL は GPT-4 級モデル + Reflexion 由来で +7.8% over ReAct を主張
- Bonsai-8B 1bit では **translate しない** (天井 7 連続の延長線上)
- production default ON は意義喪失、計算 overhead (LLM reflection call + SQLite writes) のみ残置 → **default OFF 切替**
- 機構自体は temporary opt-in で復活可能性を残す (副次 stability 軸 finding の将来活用、別 model 移植時の baseline 比較)

---

## 2. 設計選択

### 2.1 アプローチ比較

| 案 | 内容 | Pros | Cons |
|---|---|---|---|
| **A. env default 反転** | `is_erl_disabled()` 維持、env unset で true | 最小変更 | 二重否定 (`BONSAI_ERL_DISABLED=0` で ON は混乱) |
| **B. env rename + 意味論統一** | `is_erl_enabled()` rename、env `BONSAI_ERL_ENABLED`、default false | semantics 明確、self-documenting | callsite 4 件 + test 5 件反転 |
| C. hard-disable + dead-code | `inject_heuristics` を完全 no-op | 最終形態 | 復活困難、副次 finding 活用機会喪失 |

**採用: B** — 意味論統一の長期保守利益 >> 変更コスト 30 分。C は時期尚早 (副次 finding 残置)。

### 2.2 後方互換

- `BONSAI_ERL_DISABLED` env は **完全廃止** (Lab v17 plan / handoff の historical record として CLAUDE.md と plan 内のみ残存)
- 旧 env を設定しても黙殺 (warning も出さない、cold path)
- 復活手順は CLAUDE.md 項目 216 に明記: `BONSAI_ERL_ENABLED=1 ./target/release/bonsai --lab ...`

---

## 3. スコープ

### 3.1 Production code (3 ファイル)
- `src/memory/heuristics.rs`
  - `pub(crate) fn is_erl_disabled() -> bool` → `pub(crate) fn is_erl_enabled() -> bool`
  - env name `BONSAI_ERL_DISABLED` → `BONSAI_ERL_ENABLED`
  - default: env unset で `false` (= disabled = OFF)
  - docstring 全面更新 (Lab v17 REJECT を default OFF 根拠として記載)
- `src/agent/context_inject.rs:11, 212-227` (`inject_heuristics`)
  - import 修正 + `if is_erl_disabled() { return Vec::new(); }` → `if !is_erl_enabled() { return Vec::new(); }`
  - 上部コメント更新
- `src/agent/experiment.rs:16, 1405-1422` (`run_heuristics_pass`)
  - import 修正 + 同上の条件反転 + コメント更新

### 3.2 Tests (5 件反転)
- `src/memory/heuristics.rs` (4 件):
  - `t_is_erl_disabled_default_unset` → `t_is_erl_enabled_default_unset` (default で `is_erl_enabled() == false`)
  - `t_is_erl_disabled_explicit_1` → `t_is_erl_enabled_explicit_1` (env=`1` で `true`)
  - `t_is_erl_disabled_case_insensitive_true` → `t_is_erl_enabled_case_insensitive_true` (`true`/`TRUE` 等で `true`)
  - `t_is_erl_disabled_other_values_treated_as_false` → `t_is_erl_enabled_other_values_treated_as_false` (`0`/`false`/空文字 で `false`)
  - module-local `ERL_TEST_LOCK` Mutex は維持 (env mutation race 回避)
- `src/agent/context_inject.rs:582-635` (1 件):
  - inject_heuristics short-circuit 統合 test の env 設定/期待値反転

### 3.3 Docs
- CLAUDE.md 項目 216: 「Lab v17 REJECT 受けて ERL defaults OFF 切替完遂」
  - env rename + default 反転の根拠 (semantics 統一)
  - opt-in 復活手順 (`BONSAI_ERL_ENABLED=1`)
  - 副次 stability 軸 finding は Plan B で別軸対応予定

### 3.4 スコープ外
- ERL 機構自体の dead-code 化 / 削除 (将来別 plan、副次 finding と Plan B 完了後に再評価)
- heuristics SCHEMA_V10 テーブル / インデックス (DB スキーマは無変更、空 pool で no-op)
- HeuristicStore 6 method の API 変更
- Lab v15/v16/v17 のリトライ実機検証 (REJECT 確定で不要)

---

## 4. TDD strict 5 phase

### Phase 1 Red (~20 min)

**目的**: rename + 反転後の semantics を test で先行記述、build red 確証

**手順**:
1. `src/memory/heuristics.rs` の 4 test 関数名 + body を反転
   - 関数名: `t_is_erl_disabled_*` → `t_is_erl_enabled_*`
   - 期待値反転: `assert!(!is_erl_disabled(), ...)` → `assert!(!is_erl_enabled(), ...)` (default false)
   - env name: `BONSAI_ERL_DISABLED` → `BONSAI_ERL_ENABLED`
2. `src/agent/context_inject.rs` の short-circuit 統合 test も同様反転
3. `cargo build` で **未定義 `is_erl_enabled` / 未削除 `is_erl_disabled` callsite で compile error** = Red 確証
4. (本 phase では production code は未変更)

**成功基準**: build red、test 反転完了

### Phase 2 Green (~40 min)

**目的**: production code を test に追従させる

**手順**:
1. `src/memory/heuristics.rs::is_erl_disabled` → `is_erl_enabled`
   - 関数 rename
   - env name `BONSAI_ERL_DISABLED` → `BONSAI_ERL_ENABLED`
   - body は `unwrap_or_default()` の semantics そのまま (env unset で空文字 → false)
   - docstring 全面更新 (Lab v17 REJECT 言及、opt-in 手順、副次 finding 残置理由)
2. `src/agent/context_inject.rs:11` の import 修正 (`is_erl_disabled` → `is_erl_enabled`)
3. `src/agent/context_inject.rs:212-227` の short-circuit 反転
   - `if is_erl_disabled() { return Vec::new(); }` → `if !is_erl_enabled() { return Vec::new(); }`
   - 上部コメント更新
4. `src/agent/experiment.rs:16` の import 修正
5. `src/agent/experiment.rs:1405-1422` の short-circuit 反転 + コメント更新
6. `cargo build` green 確認
7. `cargo test --lib` で 1104 passed 維持確認
8. `cargo clippy -- -D warnings` 0 warning
9. `cargo fmt --check` clean

**成功基準**: 1104 passed (件数変動なし、test 5 件は rename のみ)、clippy 0 / fmt clean

### Phase 3 Refactor (~10 min)

**目的**: docstring + コメントの整合性確認、不要表現削除

**手順**:
1. `is_erl_enabled` docstring の用語統一 (「OFF cycle」「ON variant」表現を Lab v17 REJECT 文脈に合わせて再確認)
2. `inject_heuristics` / `run_heuristics_pass` の上部コメントを「default OFF」前提に揃える
3. `cargo test --lib` 再走で退行ゼロ確認
4. `cargo fmt` 適用

**成功基準**: 1104 passed 維持、docstring と code semantics の整合

### Phase 4 Smoke (~30 min)

**目的**: release build + heuristics inject 0 件動作確認

**手順**:
1. `cargo build --release` green
2. `cargo run --release -- --lab --lab-experiments 0` を **env unset** で起動 (`BONSAI_LAB_SMOKE=1` で 1 cycle)
3. log で以下を確認:
   - `inject_heuristics` 呼び出しが no-op で抜けること (debug log なし、heuristic injection IDs = empty)
   - `run_heuristics_pass` が `HeuristicSummary::default()` を返すこと (post-Lab log なし)
   - SQLite `heuristics` テーブルへの新規 insert ゼロ (既存 134 件は維持)
4. `BONSAI_ERL_ENABLED=1 cargo run --release -- --lab --lab-experiments 0` で従来動作 (項目 213) が復活することを確認 (heuristics injection が動く)
5. score / pass@k は本 plan の検証範囲外 (REJECT 既に確定、smoke G-4 は wiring 確証のみ)

**成功基準**:
- env unset で post-Lab heuristics pass log が出現しない
- env=1 で項目 213 動作が復活
- 退行ゼロ (1104 passed 維持)

**Smoke 省略可条件**: llama-server 未起動の場合は build + unit test のみで PASS とする (production code 変更は default OFF 短絡で既存挙動完全変更、unit test の semantics 反転で機構保証済み)

### Phase 5 Commit + Docs (~30 min)

**手順**:
1. commit (Phase 1 Red、Phase 2 Green、Phase 3 Refactor を 1 commit に統合可、本 plan は最小変更で論理単位 1 つ)
2. CLAUDE.md 項目 216 追記
3. MEMORY.md handoff 更新 (`session_2026_05_09b_handoff.md` 新規作成、本 plan を root として参照)

**Commit message** (案):
```
feat(erl): defaults OFF 切替 — Lab v17 REJECT 反映 (項目 216)

- is_erl_disabled() → is_erl_enabled() rename
- env BONSAI_ERL_DISABLED → BONSAI_ERL_ENABLED
- default false (= OFF) 切替
- inject_heuristics + run_heuristics_pass short-circuit 反転
- test 5 件反転 (1104 passed 維持)
- docstring 全面更新 (Lab v17 REJECT 根拠 + opt-in 復活手順)

副次 finding (stability 軸 ON 優位) は Plan B (ReviewState V12) で別軸対応予定。
ERL 機構自体は env=1 で復活可能、dead-code 化は将来別 plan。
```

---

## 5. リスク

| ID | 内容 | 影響 | 軽減 |
|---|---|---|---|
| R1 | rename 漏れ (callsite/test) | compile error | grep で全件確認、Phase 1 Red で検出保証 |
| R2 | env race condition (test) | flaky test | module-local `ERL_TEST_LOCK` Mutex 維持 (handoff 5-08i pattern) |
| R3 | production code 動作変更 (heuristics 蓄積停止) | Lab cycle で skill promotion 経路の 1 つ消失 | 副次 finding 活用は Plan B で別軸、現状 Lab REJECT 受けて意義消失で受容 |
| R4 | smoke G-4 で llama-server 必要 | smoke skip 必要 | smoke 省略可条件を Phase 4 に明記、unit test で機構保証 |
| R5 | 旧 env `BONSAI_ERL_DISABLED` を見ている外部 script | 黙殺で機能不全に気付かない | CLAUDE.md 項目 216 で env rename を明記、`scripts/lab_v17_paired.sh` は historical record で動作不要 |

---

## 6. ACCEPT 判定

### Gate 1 (TDD strict)
- ✅ Phase 1 Red で build red 確証
- ✅ Phase 2 Green で 1104 passed 維持
- ✅ Phase 3 Refactor で docstring 整合
- ✅ clippy 0 / fmt clean

### Gate 2 (semantics 確証)
- ✅ env unset で `is_erl_enabled() == false`
- ✅ env=`1`/`true`/`TRUE` で `is_erl_enabled() == true`
- ✅ env=`0`/`false`/空文字 で `is_erl_enabled() == false`
- ✅ inject_heuristics + run_heuristics_pass が default で no-op

### Gate 3 (smoke、optional)
- ✅ release build green
- ✅ env unset で post-Lab pass log 不在
- ✅ env=1 で項目 213 動作復活 (heuristics injection 動作)

**全 Gate PASS で完遂宣言**、CLAUDE.md 項目 216 + handoff 更新で commit。

---

## 7. 後続 plan 候補

- **Plan A**: Cerememory power-law decay port (`cerememory-decay-port-impl.md`、~0.5 day)
- **Plan B**: Cerememory ReviewState V12 (`cerememory-review-state-v12-impl.md`、~1.5 day、本 plan 副次 finding を直接活用)
- **Phase G**: Working memory cap 7±2 (Cerememory roadmap、~0.5 day)
- **(将来)**: ERL 機構 dead-code 化 plan (Plan B 完遂後、副次 finding が再現しなければ着手)
