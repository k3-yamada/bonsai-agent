# Plan: AgentHER Hindsight Relabel — ECHO + HSL 統合実装

> **由来**: arxiv 2603.21357 "AgentHER: Hindsight Experience Replay for LLM Agents" (2026-03)
> 既存 bonsai 項目との統合: 項目 77 (Experience Replay) / 項目 85 (TrialSummary) / 項目 140 (oracle feedback) / 項目 161 (軌跡→スキル昇格) / 項目 162 (EventStore ランタイム統合)
>
> **scope**: 失敗 trajectory も skill promotion 候補に転換、pass^k 改善 (低リスク)
> **scope-out**: LLM-as-judge による subgoal 抽出 (項目 163 と統合の別 plan)
> **non-conflict**: Beyond pass@1 plan / A-RAG plan と並列起草中、API 名空間 (`hindsight_*` / `Hindsight*` / `hsl_*`) で独立保持

---

## Task Type
- [x] Backend (production code 変更、TDD strict 5 phase)
- [x] memory/skill 機能拡張 (既存構造非破壊、additive)

## Background — 動機と理論的根拠

### 既存 pass^k の盲点
現行 `MultiRunTaskScore::from_scores()` (benchmark.rs) と `EventStore::extract_successful_trajectories(min_rate, min_steps)` (event_store.rs:139) は、最終 goal 達成 (final answer 正解 or 全 tool 成功) に基づく厳格評価:

- 22 task k=3 baseline (項目 189) で score=0.7560、つまり **24.4% の trajectory は完全失敗扱い**
- これらの「失敗」trajectory は `extract_successful_trajectories(0.8, 2)` のフィルタで完全除外され、SkillStore に到達しない
- しかし失敗 trajectory 中にも「ファイル作成成功」「git_commit 成功」「コンパイル通過」等の **intermediate sub-achievement** は存在
- 例: FizzBuzz 実装で `cargo new` + `file_write src/main.rs` は成功したが `cargo test` で fail → 最終 score=0、ただし「Rust プロジェクト雛形作成」chain は再利用価値あり

### AgentHER の 2 メカニズム (arxiv 2603.21357)

| 機構 | 概要 | bonsai mapping |
|------|------|----------------|
| **ECHO** (Experience-based Capability Hindsight Optimization) | 失敗 trajectory から alternative subgoal を抽出、達成済 sub-description を scratchpad に維持 | `experience.rs` に `HindsightRelabel` を新設、achieved sub-goals を Lesson 化して `ExperienceStore` に注入 |
| **HSL** (Hindsight Self-Labeling) | trajectory 上で達成した全 goal を mining、本来の goal でなくても「成功」扱いで relabel | 1 trajectory から k 個 (`tool_chain_key` 切片単位) の sub-skill を抽出、`SkillStore::promote_from_hindsight_relabel()` 経由で skill 化 |

### bonsai 既存スタックとの統合 (重複・補完判定)

| 項目 | 機能 | hindsight relabel との関係 |
|------|------|-----------------------------|
| 77 Experience Replay | 類似タスク経験を `<experience-context>` で注入 | **補完**: 失敗 lesson も sub-success と一緒に注入可能、後続 plan のデータソース増 |
| 85 TrialSummary | 失敗試行履歴を Replan 時注入 | **直交**: TrialSummary は同セッション内、本 plan は cross-session 永続化 |
| 140 extract_worst_reasoning | REJECT 実験から worst delta 抽出 | **直交**: Lab cycle の oracle feedback、本 plan は task-level trajectory mining |
| 161 promote_from_trajectory | 成功 trajectory → skill 自動昇格 | **直接拡張**: 本 plan の `promote_from_hindsight_relabel` は既存ラッパー |
| 162 EventStore ランタイム統合 | SessionEnd / ToolCallEnd 等を append-only 記録 | **データソース**: 本 plan の `extract_failed_trajectories` の入力源 |

### bonsai 適用効果の定量予測 (項目 189 baseline ベース)
- core 22 task k=3 = 66 run のうち failure ≈ 16 run (success_rate 0.7560 推定の補集合)
- 各 failure trajectory が平均 2 件 sub-success を持つと仮定 → 32 件の hindsight relabel 候補
- `promote_from_trajectory` 既存 dedup で重複排除後、純増 skill 数 ~5-10 件 (Lab v15 + 既存 SkillStore に対する増分)
- pass^k 改善は **間接** (skill が次 session で trigger されたとき効く)、smoke では fire 観測のみ、core 22 で初めて score 影響計測可能 → Phase 4-5 で多段検証

---

## Design — 責務分離と既存構造の非破壊性

```
┌──────────────────────────────────────────────────────────────────┐
│ EventStore (項目 162、append-only)                                │
│   - SessionEnd / ToolCallStart / ToolCallEnd                     │
│   - 既存 extract_successful_trajectories は無変更                 │
└──────────────────┬───────────────────────────────────────────────┘
                   │ 並列パス (重複読みは Phase 3 で dedup)
            ┌──────┴──────┐
            ▼             ▼
   既存 (success path)   新規 (failure path)
   extract_successful   extract_failed_trajectories
   _trajectories        ↓
   ↓                    extract_hindsight_relabels (HSL)
   TrajectoryCandidate  ↓
   ↓                    Vec<HindsightRelabel>
   promote_from_        ↓
   trajectory           promote_from_hindsight_relabel
   ↓                    ↓ (内部で promote_from_trajectory ラップ + ECHO で Experience に lesson 注入)
   SkillStore           SkillStore (同テーブル、name prefix 'hsl_' で区別)
                        + ExperienceStore (lesson type='insight')
```

**非破壊原則**:
- `EventStore` / `TrajectoryCandidate` / `SkillStore::promote_from_trajectory` / `ExperienceStore::record` は無変更
- 新規 API のみ追加、既存 1032 tests に regression ゼロ
- Phase 3 で `extract_successful_trajectories` と `extract_failed_trajectories` の event_data 読みは内部 helper で共有 (1 セッション 1 read)

---

## API Spec (案)

### 1. `src/memory/experience.rs` への追加

```rust
/// Hindsight relabel 結果: 失敗 trajectory から達成済 subgoal を抽出した記録
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HindsightRelabel {
    /// 元の goal (UserMessage の content)
    pub original_goal: String,
    /// 達成済 sub-goal の記述 (heuristic 由来、HSL で生成)
    pub achieved_subgoals: Vec<String>,
    /// trajectory の tool chain (生 sequence、prefix slice 用に順序保持)
    pub trajectory: Vec<String>,
    /// この trajectory 全体の tool 成功率 (0.0〜1.0)
    pub tool_success_rate: f64,
    /// 元 session_id (原因追跡用)
    pub session_id: String,
    /// trajectory 全体長 (相対 prefix 比率計算用)
    pub total_steps: usize,
}

impl HindsightRelabel {
    /// achieved_subgoals 末尾を「主目標」と再ラベルした疑似 TrajectoryCandidate を生成
    /// (`SkillStore::promote_from_trajectory` への adapter)
    pub fn into_relabeled_candidate(&self, subgoal_index: usize)
        -> Option<crate::agent::event_store::TrajectoryCandidate> { /* ... */ }
}

/// subgoal 達成判定方式
#[derive(Debug, Clone, Copy)]
pub enum SubgoalJudgeMethod {
    /// 案 A: ToolCallEnd の success=true 信号 (deterministic、低 cost、項目 162 と整合)
    ToolEndSuccess,
    /// 案 B: 副作用ある tool 種別の成功のみ (file_write / multi_edit / git_commit 等)
    SideEffectOnly,
    /// 案 A + 案 B: 両方を OR で評価 (デフォルト推奨)
    ToolEndSuccessOrSideEffect,
}

/// Event 列から HindsightRelabel を mining (HSL: hindsight self-labeling)
pub fn extract_hindsight_relabels(
    events: &[crate::agent::event_store::Event],
    method: SubgoalJudgeMethod,
) -> Vec<HindsightRelabel>;
```

**判定 heuristic (案 A + 案 B、高精度・低 false-positive)**:
- 案 A: `ToolCallEnd.event_data.success == true` を sub-achievement とみなす
- 案 B: tool_name が `["file_write", "multi_edit", "git_commit"]` (副作用既知ホワイトリスト) かつ ToolCallEnd.success
- 案 C (本 plan scope 外): LlmJudge (項目 163) で自然言語 sub-goal を抽出 — 別 plan へ defer
- 推奨デフォルト: `ToolEndSuccessOrSideEffect` (案 A + 案 B、誤検出許容より recall 重視)

### 2. `src/agent/event_store.rs` への追加 (既存 extract_successful_trajectories と並列)

```rust
impl<'a> EventStore<'a> {
    /// 失敗 trajectory を抽出 (success_rate < max_rate)
    /// SessionEnd 必須、`min_steps` 以上、`max_tool_success_rate` 未満の trajectory を返す
    /// 戻り値は既存と同じ TrajectoryCandidate (互換性最優先、success/failure の判別は呼出側責務)
    pub fn extract_failed_trajectories(
        &self,
        max_tool_success_rate: f64,  // 例: 0.8 — これ未満は失敗扱い
        min_steps: usize,             // 例: 2 — 短すぎる trajectory は除外
    ) -> Result<Vec<TrajectoryCandidate>>;
}
```

### 3. `src/memory/skill.rs` への追加

```rust
impl<'a> SkillStore<'a> {
    /// HindsightRelabel から skill を昇格 (HSL ラッパー)
    /// 既存 promote_from_trajectory を内部で使い、name prefix を 'hsl_' に変える
    /// achieved_subgoals 各要素に対して別個 candidate を生成し、最大 max_promote 件まで昇格
    pub fn promote_from_hindsight_relabel(
        &self,
        relabel: &crate::memory::experience::HindsightRelabel,
        max_promote: usize,  // デフォルト 3 件 (skill 爆発防止)
    ) -> Result<Vec<i64>>;
}
```

**重複判定**: 既存 `promote_from_trajectory` の tool_chain ベース dedup に完全依存 (項目 161 安定ハッシュサフィックス)、本 plan で追加実装不要。

### 4. ECHO 部分: ExperienceStore への lesson 注入

```rust
impl<'a> ExperienceStore<'a> {
    /// HindsightRelabel から「失敗だが部分達成した経験」を Insight として記録
    /// ECHO 相当 (alternative subgoal description を scratchpad に維持)
    pub fn record_hindsight_insight(
        &self,
        relabel: &crate::memory::experience::HindsightRelabel,
    ) -> Result<i64>;
}
```

実装は `ExperienceType::Insight` で 1 trajectory につき 1 レコード:
- task_context = `original_goal`
- action = `trajectory.join(" -> ")`
- outcome = `部分達成: {achieved_subgoals.join(", ")}`
- lesson = `Some("失敗 trajectory から hindsight 抽出: 主目標未達だが {N} 個の subgoal は達成、再利用候補")`
- tool_name = trajectory 末尾 tool

---

## TDD Strict 5 Phase

### Phase 1: Red — failing tests 10 件追加 (production stub のみ、全 fail 確認)

**1.1 `extract_hindsight_relabels` (5 件)**
1. `t_extract_hsl_basic_success_subgoal` — file_write 成功 + 失敗 cargo_test の trajectory で subgoal 1 件抽出
2. `t_extract_hsl_filters_all_failures` — 全 ToolCallEnd success=false なら relabel 0 件 (false-positive 防止)
3. `t_extract_hsl_side_effect_method` — `SideEffectOnly` で shell の exit 0 を除外 (file_write のみ拾う)
4. `t_extract_hsl_session_end_required` — SessionEnd 不在 trajectory は除外 (項目 162 整合)
5. `t_extract_hsl_min_steps_filter` — total_steps < 2 の trajectory は除外

**1.2 `promote_from_hindsight_relabel` (3 件)**
6. `t_promote_hsl_basic` — relabel 1 件、achieved_subgoals 2 件、`max_promote=2` で 2 skills 昇格、name prefix `hsl_*`
7. `t_promote_hsl_dedup` — 同一 tool_chain は既存 promote_from_trajectory dedup で skip
8. `t_promote_hsl_max_promote_clamps` — `max_promote=1` で 2 候補あっても 1 件のみ

**1.3 fallthrough / regression (2 件)**
9. `t_existing_promote_from_trajectory_unaffected` — 既存 success path が無変更 (1032 baseline 退行ゼロ確認)
10. `t_record_hindsight_insight_creates_experience` — ExperienceStore に type=insight で 1 レコード追加

**Red 状態の判定**:
- production code: `pub fn extract_hindsight_relabels(...) -> Vec<HindsightRelabel> { todo!() }` 等 stub のみ
- `cargo test --release --lib` で **1032 既存 + 10 新規 → 1032 passed / 10 failed** を期待
- 1032 既存 test の退行ゼロを確認 (Red phase でも regression は許さない)

### Phase 2: Green — 最小実装で 10 tests 全 PASS

**実装手順**:
1. `extract_failed_trajectories` を `event_store.rs` に追加 (`extract_successful_trajectories` の対称、`max_tool_success_rate` 指定)
2. `HindsightRelabel` struct を `experience.rs` に追加
3. `extract_hindsight_relabels` 実装 — events を線形走査、ToolCallStart/End ペアごとに heuristic 適用、achieved_subgoals に push
4. `into_relabeled_candidate` 実装 — `trajectory[..=subgoal_index]` を tool_sequence とする TrajectoryCandidate を生成、tool_success_rate=1.0 (この prefix までは成功) で再ラベル
5. `promote_from_hindsight_relabel` 実装 — achieved_subgoals 各要素を `into_relabeled_candidate(i)` で TrajectoryCandidate 化 → `promote_from_trajectory` 呼出 → name prefix を `hsl_` に置換 (既存 `traj_` 生成と同形 ASCII 操作)
6. `record_hindsight_insight` 実装 — `RecordParams { exp_type: Insight, ... }` で `record()` 呼出
7. `cargo test --release --lib` で **1042 passed / 0 failed** (G-2)

**SubgoalJudgeMethod の semantics**:

```rust
fn is_subgoal_achieved(
    method: SubgoalJudgeMethod,
    tool_name: &str,
    success: bool,
) -> bool {
    if !success { return false; }
    match method {
        SubgoalJudgeMethod::ToolEndSuccess => true,
        SubgoalJudgeMethod::SideEffectOnly => {
            matches!(tool_name, "file_write" | "multi_edit" | "git_commit")
        }
        SubgoalJudgeMethod::ToolEndSuccessOrSideEffect => true,  // 案 A 包含で実質 OR
    }
}
```

**name 生成 (重複対策)**:
- prefix `hsl_` + 既存 `traj_` ハッシュサフィックス機構を踏襲 (skill.rs:181-193)
- 例: `hsl_FizzBuzz実装_a3f4` (既存 traj_ と name 衝突なし、tool_chain 単位の dedup は SQLite UNIQUE で担保)

### Phase 3: 既存 Experience Replay 経路と非衝突確認

**3.1 Skill 重複判定**:
- 既存 `promote_from_trajectory` (skill.rs:172-179): `WHERE tool_chain = ?` UNIQUE check
- HSL prefix の subgoal 切片が既存 success trajectory の prefix と一致した場合 → 既存 dedup で skip
- これは設計通り (HSL は「既存 success が捉えそこねた失敗 prefix」を補う、success が拾えるなら HSL は必要ない)

**3.2 EventStore 二重 read オーバーヘッド**:
- `extract_successful_trajectories` と `extract_failed_trajectories` を **同一 EventStore で 1 回 list_sessions → 各 session で 1 回 replay** する内部 helper (`build_trajectory`、event_store.rs:158) を共有
- private helper `pub(crate) fn build_trajectory_with_classification(...) -> (TrajectoryCandidate, bool /*is_failure*/)` を導入する選択肢あり、Phase 2 で必要なら実装、不要なら deferred

**3.3 ExperienceStore type=insight の検索衝突**:
- 既存 `find_similar` (experience.rs:104) は LIKE 検索で type 指定なし → hsl insight も hit する
- これは設計通り (hindsight insight は context 注入で読まれる前提)
- 必要なら type filter を別 plan で追加、本 plan scope 外

### Phase 4: smoke 検証 (失敗 task を含む設計)

**4.1 既存 smoke 5 task の失敗誘発度**:
- 項目 192 観測: file_write 3 件 / 0 件失敗 → smoke では HSL 発動不可
- 解決策: smoke 用 task に **意図的に失敗誘発する task を追加** (時限拡張、本 plan 内で benchmark.rs に追加)
  - 例: 「存在しない URL を curl して内容を要約せよ」 (web tool 失敗 + file_write 成功 prefix を残す)
  - 例: 「`tests/missing.rs` を読んで修正せよ」 (file_read 失敗だが他 tool 部分成功)

**4.2 Smoke 実行手順**:
```bash
# config.toml backup
cp ~/Library/Application\ Support/bonsai-agent/config.toml{,.pre-hsl-smoke}

# llama-only 構成で smoke 実行 (HSL 発動を観測)
BONSAI_LAB_SMOKE=1 cargo run --release -- --lab --lab-experiments 0

# 検証ポイント:
# 1. EventStore に SessionEnd 全件記録
# 2. extract_failed_trajectories(0.8, 2) >= 1 件
# 3. extract_hindsight_relabels >= 1 件
# 4. promote_from_hindsight_relabel で `hsl_*` skill が 1 件以上 SkillStore に追加
# 5. 既存 traj_* skill 数が減らない (G-3)
```

**4.3 Smoke 期待結果** (パターン例):
- baseline score: variance 範囲 (項目 193 smoke 0.7253 ± 0.03)
- hindsight relabel fire 数: ≥ 1 件 (G-2)
- 新規 hsl_* skill 数: 1-3 件
- 既存 traj_* skill 数: 不変 (G-3、退行検知)

### Phase 5: 初期データ収集 (Lab 連動、optional・別セッション可)

**5.1 metric 計測**:
- Lab v16 baseline run で `event_store.list_sessions()` 全走査 → `extract_failed_trajectories` 件数取得
- 1 cycle あたり HSL fire 数 / promotion 増加率を `experiment_log` に記録 (TSV カラム追加は別 plan、本 plan は記録のみ)

**5.2 初期受入基準**:
- core 22 task k=3 で hindsight relabel mining 数 ≥ 5 件
- promotion 増加率 ≥ 1 (1 Lab cycle で skill 数が +1 以上)
- pass^k への影響は次 cycle で観測 (本 plan は仕込みのみ、効果実証は Lab v17 以降)

---

## 判定 Gate

| Gate | 内容 | 判定基準 | 失敗時の挙動 |
|------|------|----------|--------------|
| **G-1** | 既存 1032 tests 全 PASS 維持 | `cargo test --release --lib` で `1032 + N` passed (N = 新規追加 test 数 = 10) | Phase 1 Red phase でも既存 test 退行は許さない、即時修正 |
| **G-2** | smoke で hindsight relabel 発動 ≥ 1 件 | smoke ログに `extract_hindsight_relabels: N=X (X>=1)` または `hsl_* skill promoted` | smoke 失敗 task が不足 → 4.1 で追加 task 設計 |
| **G-3** | 既存 promote_from_trajectory のスキル数が減らない | smoke 前後で `SELECT COUNT(*) FROM skills WHERE name LIKE 'traj_%'` 不変 | 既存経路への副作用 → Phase 2 実装 review |
| **G-4** (informational) | core 22 baseline で `hsl_*` skill 数 ≥ 1 件 | Phase 5 計測 | 失敗誘発 task が core 22 に少ない可能性、benchmark 追加 plan へ |

---

## Risk 表 (5+ 項目)

| ID | リスク | 影響度 | 軽減策 |
|----|--------|--------|--------|
| R1 | **false-positive promotion** — 副作用ない tool 成功も sub-achievement 扱い | HIGH | デフォルトを `ToolEndSuccessOrSideEffect` に、副作用既知 tool ホワイトリスト (file_write/multi_edit/git_commit) で案 B 補強、shell は案 B から除外 (exit 0 だけでは副作用不明) |
| R2 | **SkillStore 容量肥大** — 失敗 trajectory も全 mining → 重複 skill 大量 | MEDIUM | `max_promote=3` clamp + 既存 `tool_chain` UNIQUE dedup (項目 161)、purge_expired (skill TTL、未実装だが Phase 5 で追加候補) |
| R3 | **経験 promotion 質劣化で context 汚染** — hsl_* skill が `<context type="skills">` で誤注入 | MEDIUM | name prefix `hsl_` で区別、context_inject 側で skill type filter を別 plan で実装 (本 plan は記録のみ、注入経路は既存 unchanged で副作用ゼロ) |
| R4 | **subgoal heuristic の偏向** — ToolCallEnd success だけで判定 → 本物の意味的 sub-goal 取り逃し | MEDIUM | `SubgoalJudgeMethod` enum で切替可能、案 C (LlmJudge) は別 plan、recall 重視デフォルトで偏向影響小 |
| R5 | **EventStore double-read オーバーヘッド** — extract_successful + extract_failed の 2 回 list_sessions | LOW | Phase 3.2 で内部 helper 共有 (1 read で is_failure 分類)、現状でも O(N session) 線形なので overhead 軽微 |
| R6 | **smoke で HSL 発動しない** — 失敗 task 不足で G-2 達成不能 | MEDIUM | Phase 4.1 で意図的失敗誘発 task を smoke benchmark に時限追加 (本 plan 内で実装、merge 後 revert か `#[cfg(test)]` 限定で残置) |
| R7 | **既存 trajectory テストへの retroactive 影響** — 重複 candidate 注入で既存 dedup がより厳密化 | LOW | Phase 1 Red で `t_existing_promote_from_trajectory_unaffected` を必須テスト化、退行検出 |

---

## YAGNI 境界 (本 plan のスコープ外)

| 機能 | 本 plan に含めるか | 理由 |
|------|-----------------|------|
| LLM-as-judge による subgoal 抽出 (案 C) | NO | 計算コスト高、項目 163 LlmJudge の二次利用、別 plan で着手 |
| 失敗 trajectory content の memory_blocks 直接注入 | NO | 項目 179 memory_blocks の責務範囲外、本 plan は SkillStore + ExperienceStore 経由のみ |
| SkillStore 重複判定の強化 | NO | 項目 161 安定ハッシュサフィックスで対応済、追加対応不要 |
| skill TTL purge 機構 | NO | 既存 `purge_expired` は skills テーブル対応済 (skill.rs:149)、本 plan は新規 TTL 設定不要 |
| HSL fire 数の experiment_log TSV カラム化 | NO | 本 plan は記録 (`record_hindsight_insight`) まで、TSV 統合は Phase 5 別 plan |
| context_inject 側で skill type filter | NO | 注入経路に手を入れない、副作用ゼロを保つために defer |
| ECHO の scratchpad リアルタイム維持 (in-flight session) | NO | 本 plan は post-session mining のみ、in-flight 維持は次世代設計 |

---

## 実装ファイル一覧 (production 変更見込み)

| ファイル | 変更種別 | 推定行数 |
|----------|----------|----------|
| `src/memory/experience.rs` | additive (`HindsightRelabel` struct + `extract_hindsight_relabels` + `record_hindsight_insight` + `SubgoalJudgeMethod` enum) | +120 / -0 |
| `src/memory/skill.rs` | additive (`promote_from_hindsight_relabel`) | +50 / -0 |
| `src/agent/event_store.rs` | additive (`extract_failed_trajectories` + Phase 3.2 内部 helper) | +60 / -0 |
| `src/agent/benchmark.rs` (Phase 4 限定) | conditional (smoke 失敗誘発 task 2 件追加) | +30 / -0 (合意取れなければ scope 外) |
| 既存テストファイル | unchanged (regression 検証のみ) | 0 / 0 |

**新規 test ファイル**: なし (既存 `experience.rs::tests` / `skill.rs::tests` / `event_store.rs::tests` 各モジュールに追記)

---

## 見積もり

| Phase | 作業内容 | 推定時間 |
|-------|----------|----------|
| Phase 1 (Red) | 10 件 failing tests 追加 + production stub | 1.5h |
| Phase 2 (Green) | 実装 + 1042 passed 達成 | 2.5h |
| Phase 3 (非衝突確認) | 既存経路の regression 検証 + helper 共有判断 | 1h |
| Phase 4 (smoke) | smoke 失敗誘発 task 設計 + 実機 fire 観測 | 1h |
| Phase 5 (初期データ) | 計測のみ、別セッション可 | 0.5h (本 plan セッションでは optional) |
| **合計** | | **~6h (Phase 1-4)、計 1 day** |

---

## 期待効果と次 plan へのつなぎ

### 短期 (本 plan 完了時)
- 失敗 trajectory も skill promotion 候補に転換、SkillStore 充実度 +5-10 件
- ExperienceStore に hindsight insight 記録、後続 plan のデータソース確立
- production code 変更は additive のみ、退行ゼロ

### 中期 (次 plan)
- LLM-as-judge による意味的 subgoal 抽出 (案 C、項目 163 統合)
- 項目 138 構造化 feedback retry に hindsight insight を注入
- Beyond pass@1 plan (RDC/VAF) と統合: 失敗 trajectory の partial reliability metric 化

### 長期
- 項目 77 Experience Replay の context_inject に hsl_* skill を選別注入
- in-flight ECHO (session 中の動的 scratchpad 更新)、項目 179 memory_blocks 派生

---

## 参考

- arxiv 2603.21357 — AgentHER: Hindsight Experience Replay for LLM Agents (2026-03)
- 既存項目: 77 / 85 / 138 / 140 / 161 / 162 / 163
- 既存 plan: `request-size-guard-impl-v2.md` (TDD strict 5 phase の構造踏襲)
- 既存ファイル参照:
  - `src/memory/experience.rs:26-50` (Experience / RecordParams 構造)
  - `src/memory/skill.rs:162-206` (promote_from_trajectory 既存実装)
  - `src/agent/event_store.rs:139-156` (extract_successful_trajectories 既存実装)
  - `src/agent/event_store.rs:228-242` (TrajectoryCandidate 構造)
  - `src/agent/error_recovery.rs:264 (TrialSummary)` / `src/agent/experiment.rs:744 (extract_worst_reasoning)`
- 並列起草中の独立 plan: Beyond pass@1 (`MultiRunTaskScore` 拡張) / A-RAG (3 検索 tool 整理) — 名前空間衝突なし (`HindsightRelabel` / `extract_hindsight_relabels` / `promote_from_hindsight_relabel` / `hsl_*`)
