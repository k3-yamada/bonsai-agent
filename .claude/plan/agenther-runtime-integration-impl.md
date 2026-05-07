# AgentHER Runtime Integration Plan

> handoff 05-07e TODO #1 (★★★ 最優先) — 項目 201 で実装した HSL/ECHO API を `run_experiment_loop` 末尾に配線。
> Effectiveness 観測 (Phase 4 smoke / Phase 5 Lab v16 metric) の前提条件。

## Scope

`src/agent/experiment.rs::run_experiment_loop` の最末尾（既存 `Ok(experiments)` 直前 = L1188）に AgentHER post-Lab pass を追加。Lab 完走後、これまで蓄積された events から失敗 session の hindsight relabel を抽出し SkillStore + ExperienceStore に永続化。**項目 161 の `extract_successful_trajectories` も symmetric に同 hook で配線**（dead-code 解消）。

### Out of scope
- TSV カラム化（hsl_fire_count / hsl_promote_count）→ Phase 5 別 plan
- Failure-inducing benchmark task → Phase 4 別 plan
- LLM-as-judge による subgoal 抽出 → 中期 plan

## Hook 仕様

挿入位置: `src/agent/experiment.rs:1186-1188` の `eprintln!("[lab] 完了: ...")` 直後、`Ok(experiments)` 直前。

```rust
// AgentHER post-Lab hindsight relabel pass (handoff 05-07e TODO #1)
match run_hindsight_pass(store) {
    Ok(s) => log_event(LogLevel::Info, "lab.agenther", &format!(
        "AgentHER: failed={} successful={} relabels={} skills={} insights={}",
        s.failed_sessions, s.successful_sessions, s.relabels,
        s.skills_promoted, s.insights_recorded,
    )),
    Err(e) => log_event(LogLevel::Warn, "lab.agenther", &format!(
        "AgentHER post-Lab pass failed (non-fatal): {e}"
    )),
}
```

新規 private fn:

```rust
#[derive(Default)]
struct HindsightSummary {
    failed_sessions: usize,
    successful_sessions: usize,
    relabels: usize,
    skills_promoted: usize,
    insights_recorded: usize,
}

fn run_hindsight_pass(store: &MemoryStore) -> Result<HindsightSummary> {
    let conn = store.conn();
    let event_store = EventStore::new(conn);
    let skill_store = SkillStore::new(conn);
    let exp_store = ExperienceStore::new(conn);
    let mut summary = HindsightSummary::default();

    // 1. 失敗 session の HSL relabel 抽出 → skill 昇格 + ECHO insight
    let failed = event_store.extract_failed_trajectories(0.8, 2)?;
    summary.failed_sessions = failed.len();
    for candidate in &failed {
        let events = event_store.replay(&candidate.session_id)?;
        let relabels = extract_hindsight_relabels(
            &events,
            SubgoalJudgeMethod::ToolEndSuccessOrSideEffect, // recall 重視 default
        );
        summary.relabels += relabels.len();
        for relabel in &relabels {
            let ids = skill_store.promote_from_hindsight_relabel(relabel, 3)?;
            summary.skills_promoted += ids.iter().filter(|x| x.is_some()).count();
            exp_store.record_hindsight_insight(relabel)?;
            summary.insights_recorded += 1;
        }
    }

    // 2. 成功 session (項目 161) も symmetric に skill 昇格
    let successful = event_store.extract_successful_trajectories(0.8, 2)?;
    summary.successful_sessions = successful.len();
    for candidate in &successful {
        if skill_store.promote_from_trajectory(candidate)?.is_some() {
            summary.skills_promoted += 1;
        }
    }

    Ok(summary)
}
```

### 失敗時の挙動 (non-fatal)
`run_hindsight_pass` の任意エラー → `Warn` log のみ、Lab 結果は通常通り `Ok(experiments)` で返す。HSL は補助機能であり Lab 成果（experiments）を破壊してはならない。

## TDD Strict 3 Phase

### Phase 1 Red
新規 test (`experiment.rs` tests モジュール末尾):
- `t_hindsight_pass_no_events_returns_zero_summary` — events 空で全フィールド 0
- `t_hindsight_pass_extracts_subgoals_from_failed_session` — 失敗 session 1 件 + subgoal → relabels >= 1, insights >= 1
- `t_hindsight_pass_promotes_successful_trajectory_symmetric` — 成功 session のみで successful_sessions >= 1, skills_promoted >= 1
- `t_hindsight_pass_max_promote_caps_skill_explosion` — 5 subgoals → skills_promoted ≤ 3 per relabel (内部 max_promote=3 確認)

これらは `run_hindsight_pass` / `HindsightSummary` 不在で compilation 失敗（E0425/E0412）。

### Phase 2 Green
- `HindsightSummary` struct + `Default` derive
- `run_hindsight_pass(&MemoryStore) -> Result<HindsightSummary>` private fn 追加
- `run_experiment_loop` 末尾に call を挿入
- 全 test PASS、cargo build 0 warning、clippy 0 warning

### Phase 3 Refactor
- clippy 警告対応（`derivable_impls`/`needless_collect` 等が出れば fix）
- 必要なら `make_event_seq()` test helper を experiment.rs tests に追加
- cargo fmt --check clean

## Risk

| ID | 内容 | 緩和策 |
|----|------|--------|
| R1 | EventStore に events 0 件で extract_failed_trajectories panic | 既存実装で SessionEnd 必須フィルタ済、空 Vec 返却（event_store tests で確証） |
| R2 | promote 中の SQLite UNIQUE conflict | 既存 dedup 機構で `Option<i64>` 返却、None=skip 仕様 |
| R3 | benchmark.rs::run_k 経由で events 実 emit されているか | 項目 162 で run_agent_loop_with_session が SessionStart/UserMessage/ToolCallStart/End/SessionEnd を emit、benchmark は同 path 経由のため自動 emit |
| R4 | post-Lab pass で大量 promote → SkillStore 肥大化 | max_promote=3 + tool_chain UNIQUE dedup で実効上限 < 10 skills/Lab cycle |
| R5 | Lab 中の任意 panic で run_experiment_loop 全体が stop | hindsight_pass は `Ok(experiments)` 直前で Result エラーは Warn log のみで握り潰す（non-fatal） |
| R6 | smoke baseline で events が新規 session のみ集まり既存 events と分離されない | event_store は累積 SQLite、Lab 1 cycle で promote 数の上限は max_promote 制約で問題なし |

## Verification Gates

- G-1: cargo test --release --lib **1051 → 1055+ passed**（+4 minimum、退行ゼロ）
- G-2: cargo clippy --release --lib --tests -- -D warnings → 0 warning
- G-3: cargo fmt --check → clean
- G-4: cargo build --release → 0 warning

## Estimated Effort

- Phase 1 Red: 30 min (test helper + 4 failing tests)
- Phase 2 Green: 45 min (HindsightSummary + run_hindsight_pass + run_experiment_loop hook)
- Phase 3 Refactor + verification: 30 min
- 合計: ~1.75h（handoff 想定 ~2h と整合）

## 期待効果

- 項目 201 (HSL API) + 項目 161 (success trajectory API) の **dead-code 解消**
- Lab v16 以降で Lab 1 cycle あたり typical 0-5 skills + 0-3 insights 純増（実観測は次セッション Phase 5）
- Phase 4 smoke + Phase 5 Lab v16 metric の前提条件達成

## Symmetric promotion 採用根拠

handoff 05-07e の TODO #1 は HSL のみ言及だが、`extract_successful_trajectories` (項目 161) も同様に dead-code 状態であることが調査で判明した。同 hook で symmetric に配線する利点:
- 1 commit で trajectory → skill パイプライン全体が活性化
- 「成功は traj_ prefix / 失敗の subgoal は hsl_ prefix」で SkillStore 内で起源が観測可能
- 項目 161 の本来意図（成功軌跡からの skill 自動昇格）が Lab cycle 末尾で初めて発動
