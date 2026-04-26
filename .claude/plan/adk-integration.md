# Implementation Plan: Google ADK 知見の取り込み

## Task Type
- [x] Backend (Rust / harness-level)
- [x] Documentation / Configuration
- [ ] Frontend

## 背景

`memory/google_adk_learnings.md` で整理した Google ADK / agents-cli の 5 取込候補（P0–P4）を、Lab v12 完走（推定 6–12h 残）前後で実装するための段階的計画。

**外部モデル状況**: 本セッションでは Codex（gpt-5.5 アカウント未対応エラー）と Gemini（silent failure）が共に不通。Claude 単独で蓄積メモリ + Phase 1 ソース読込結果（HttpAdvisor / BenchmarkSuite / MiddlewareChain / SubAgentExecutor の signature を確認済）を元に最終プランを合成。

**Lab v12 制約**:
- 進行中: ベースライン score=0.8043, exp_2 ACCEPT (+0.0020), exp_3 開始済
- release バイナリ稼働 → ソース編集 / `cargo test --lib` は **非干渉**
- 但し **release ビルド禁止** — Lab 中の `cargo build --release` は競合リスク

---

## Technical Solution（要約）

| 候補 | スコープ | 既存資産 | コスト |
|---|---|---|---|
| **P0 LLM-as-judge** | Rubric 評価メトリクスを benchmark に追加 | `HttpAdvisor::try_remote_advice` / `try_claude_code_advice` | M（200–300行） |
| **P1 Generator-Critic** | `inject_verification_step` を SubAgent 化 | `SubAgentExecutor`, `inject_verification_step` | M（150–250行） |
| **P2 Workflow primitive** | Sequential/Parallel/Loop trait | `MiddlewareChain`, `MiddlewareSignal::Abort` | L（400–600行、リスク高） |
| **P3 DESIGN_SPEC.md** | `.claude/plan/` 散在を統合 | （なし） | S（doc only） |
| **P4 OTel exporter** | AuditLog/EventStore → OTel スパン変換 | `AuditLog`, `EventStore` | L（要 crate 追加） |

---

## Implementation Steps（フェーズ別）

### 🟢 Phase A: Lab v12 idle window（即着手可、ソース編集 + `cargo test --lib` のみ）

#### Step A1: P0 — LLM-as-judge TDD Red（45–60分、+5–8 tests）

**目的**: Lab 中に書ける範囲で型と失敗テストだけ先に commit（巻き戻し対策）。

**追加箇所**:
- `src/agent/benchmark.rs` 末尾に `RubricScore` 構造体
- `src/agent/judge.rs` 新規（HttpAdvisor 流用、~150行想定）
- `src/agent/mod.rs` に `pub mod judge;`

**疑似コード**:
```rust
// benchmark.rs（追加）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RubricScore {
    pub completeness: f64,    // 0.0–1.0: 要求の網羅性
    pub correctness: f64,      // 0.0–1.0: 事実誤りの有無
    pub reasoning_quality: f64,// 0.0–1.0: 推論の妥当性
    pub raw_judge_response: String, // 監査用
}

impl RubricScore {
    pub fn composite(&self) -> f64 {
        0.4 * self.completeness + 0.4 * self.correctness + 0.2 * self.reasoning_quality
    }
}

// judge.rs（新規）
pub trait LlmJudge {
    fn evaluate(&mut self, task: &BenchmarkTask, response: &str, trajectory: &[String])
        -> anyhow::Result<RubricScore>;
}

pub struct HttpAdvisorJudge<'a> {
    advisor: &'a mut AdvisorConfig,
    rubric_template: String, // タスクカテゴリ別 rubric
}

impl<'a> LlmJudge for HttpAdvisorJudge<'a> {
    fn evaluate(...) -> Result<RubricScore> {
        let prompt = format!(
            "Rubric: {}\nTask: {}\nResponse: {}\nTrajectory: {:?}\n\
             回答 JSON: {{\"completeness\": ..., \"correctness\": ..., \"reasoning_quality\": ...}}",
            self.rubric_template, task.description, response, trajectory
        );
        let raw = self.advisor.try_remote_advice(AdvisorRole::Verify, &prompt)?
                    .or_else(|| self.advisor.try_claude_code_advice(AdvisorRole::Verify, &prompt).ok().flatten())
                    .ok_or_else(|| anyhow::anyhow!("judge unavailable"))?;
        Self::parse_judge_response(&raw)
    }
}
```

**TDD Red テスト**:
1. `test_rubric_score_composite_weights` — 重み 0.4/0.4/0.2 検証
2. `test_judge_response_parses_json` — Mock JSON で構造体生成
3. `test_judge_response_handles_malformed_json` — 不正 JSON でも値ゼロ＋raw 保持
4. `test_judge_failure_falls_back_to_zero_score` — Advisor エラー時にゼロ返却＋warn ログ
5. `test_rubric_template_per_category` — TaskCategory 別テンプレ選択

**Lab 非干渉保証**:
- `cargo test --lib` のみ（バイナリ非再ビルド）
- 既存 `MultiRunTaskScore` 改変なし（フィールド追加なし、別 trait 追加のみ）
- `experiment.rs` から呼ばれる経路には未組込（次フェーズ）

#### Step A2: P3 — DESIGN_SPEC.md 雛形（15–20分、doc only）

`docs/DESIGN_SPEC.md` 新規。`.claude/plan/` の active プラン（agent-loop-split-validated.md, next-actions-2026-04-25-v2.md, adk-integration.md）の見出しレベル 1 だけを章別に集約。`.claude/plan/` 各ファイル **削除しない**（agents-cli 流の集約 index）。

---

### 🟡 Phase B: Lab v12 完走後（release 再ビルド可、~30–60分）

#### Step B1: P0 — LLM-as-judge Green（90–120分、Phase A の test を pass）

**変更点**:
- `judge.rs` の `evaluate()` を実装（Phase A の Mock 化を解除）
- `MultiRunBenchmarkResult` に `mean_rubric_score: Option<f64>` 追加（後方互換: Option）
- `BenchmarkSuite::run_k` に `judge: Option<&mut dyn LlmJudge>` 追加（呼出側未指定なら従来動作）
- 設定: `config.toml [benchmark] enable_llm_judge = false` デフォルト

**Lab v13 移行条件**: `enable_llm_judge = true` で Lab 起動 → judge 評価が pass^k と相補的に効くか実測。

#### Step B2: P0 — Lab v13 連携（30–45分）

`experiment.rs` の `run_experiment_loop` に judge 結果を ACCEPT/REJECT 判定の **追加シグナル** として導入（pass^k と AND 条件）。閾値: `judge_threshold = 0.7`（rubric composite）。

---

### 🟡 Phase C: Generator-Critic 分離（Phase B 完了後）

#### Step C1: P1 — `inject_verification_step` を Critic SubAgent 化（90–150分、+8–10 tests）

**変更点**:
- `src/agent/critic_agent.rs` 新規 — SubAgent として動作する Critic（`SubAgentExecutor::execute_sequential` 呼出）
- `inject_verification_step` を `delegate_to_critic_subagent` にリネーム + 内部実装差替
- 既存 verification prompt は Critic SubAgent の system prompt に移動

**escalate=True 採用**:
- `MiddlewareSignal::Abort` を `MiddlewareSignal::Escalate` にリネーム（API 互換維持のため alias 残す）
- Critic SubAgent が修正不要判定で `Escalate` を発火 → 親ループが verification step をスキップ

---

### 🔵 Phase D: Workflow primitive 形式化（Phase C 完了後、要再評価）

#### Step D1: P2 — Sequential/Parallel/Loop trait 抽出（120–180分、+10–15 tests）

**慎重判断**: Phase C 完了時点で本当にプリミティブ化が必要か再評価する（YAGNI 原則）。`run_agent_loop` のハードコードが具体的にどの拡張を阻害しているかを実例で示せない場合は **見送り**。

着手するなら:
- `src/agent/workflow.rs` 新規 — `trait Workflow { fn execute(&mut self) -> WorkflowResult; }`
- 既存 `run_agent_loop_with_session` を `LoopWorkflow` 実装に分離
- `MiddlewareChain` を `Workflow` の前/後フックとして再配置

---

### ⚫ 後回し: P4 OTel exporter

`opentelemetry` crate 依存追加 + `AuditLog::log()` を OTel span に変換。**ローカル完結ポリシーと衝突**するため、外部 collector 構築計画が立つまで保留。

---

## Key Files

| File | Operation | Phase | Step |
|------|-----------|-------|------|
| `src/agent/judge.rs` | Create (~150 LoC) | A | A1 |
| `src/agent/benchmark.rs:35-160` | Modify (RubricScore 追加, MultiRunBenchmarkResult.mean_rubric_score) | A/B | A1, B1 |
| `src/agent/mod.rs` | Modify (`pub mod judge;`) | A | A1 |
| `docs/DESIGN_SPEC.md` | Create | A | A2 |
| `src/runtime/model_router.rs:283-380` | Read-only 流用 (try_remote_advice/try_claude_code_advice) | B | B1 |
| `src/agent/experiment.rs` | Modify (judge シグナル統合) | B | B2 |
| `src/agent/critic_agent.rs` | Create | C | C1 |
| `src/agent/agent_loop.rs:1065 inject_verification_step` | Modify → delegate_to_critic_subagent | C | C1 |
| `src/agent/middleware.rs:25 MiddlewareSignal` | Modify (Abort → Escalate alias) | C | C1 |
| `src/agent/workflow.rs` | Create (条件付き) | D | D1 |
| `Cargo.toml` | Modify (opentelemetry 追加、保留) | — | — |

---

## Risks and Mitigation

| Risk | 確率 | 影響 | Mitigation |
|------|------|------|------------|
| Lab v12 中の `cargo build --release` 競合 | 中 | 高（Lab 中断） | Phase A は `cargo test --lib` のみ、release ビルド禁止 |
| Edit/Write 後の clippy 巻き戻し（CLAUDE.md 注意事項） | 高 | 中 | judge.rs は新規 + 別 commit、agent_loop.rs 直接編集を Phase C まで遅延 |
| LLM-as-judge の rubric drift（指標が変質） | 中 | 高 | 初期は read-only で並走、judge 結果はログのみ → 1サイクル後に ACCEPT/REJECT 統合 |
| claude-code subprocess の latency 累積 | 高 | 中 | judge を pass^k k=3 中の代表 1 ラン目のみ呼出（コスト 1/3） |
| Critic SubAgent 分離で 1bit モデル精度低下 | 中 | 高 | Phase C 着手前に Phase B の judge 実測で pass^k 改善を確認、改善なしなら Phase C 中止 |
| Workflow primitive の YAGNI（過剰抽象化） | 高 | 中 | Phase D 着手前に再評価ゲート設置（具体的拡張ニーズなしなら見送り） |
| MiddlewareSignal::Abort → Escalate リネームで既存コード破壊 | 中 | 中 | alias で API 互換維持、deprecation warning のみ |

---

## 推奨パス

| パス | 内容 | 工数 | リスク | 期待効果 |
|---|---|---|---|---|
| **α** Phase A only | A1（judge TDD Red）+ A2（DESIGN_SPEC） | 60–90分 | 0.5/5 | Lab 中安全、+5–8 tests、Phase B/C の前提整備 |
| **β** A → B（推奨） | judge を Lab v13 に統合 | +120–180分 (post-Lab) | 1.5/5 | LLM-as-judge 実測、ACCEPT 判定の精度向上 |
| **γ** A → B → C | Critic SubAgent 化まで | +250–400分 | 2.5/5 | Generator-Critic パターン採用、検証ステップの責務分離 |
| **δ** A → B → C → D | Workflow primitive 含む全工程 | +500–800分 | 4/5 | 構造改善 P0 Step 7 と相乗効果、但し YAGNI リスク |

**Claude 推奨**: **β** — Lab v12 完走時刻が直近 6–12h なので、α を Lab 中に消化し、β を完走後即着手。γ/δ は β の効果実測後に判断（pass^k 改善が見えなければ γ 以降は中止）。

---

## SESSION_ID（外部モデル不通のため不在）

- CODEX_SESSION: （`gpt-5.5` 未対応で起動失敗、PID 85225）
- GEMINI_SESSION: （silent failure、ログ未生成）

`/ccg:execute` 実行時は新規セッションで開始し、本ファイル全文を context に含めること。

---

## 備考

- 本プランは Lab v12 並行作業安全性を最優先で設計
- Phase A1 完了後 Phase A2 と並行で Lab ポーリング継続可能（25分間隔の余裕で書ける）
- Phase B 着手判断は Lab v12 ACCEPT 件数 + delta 累積を見て決定（v12 で大幅 ACCEPT が出れば Phase B の判定基盤強化が急務、出なければ judge 単独効果検証）
