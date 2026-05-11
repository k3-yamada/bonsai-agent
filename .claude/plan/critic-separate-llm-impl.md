# Plan: G1 Critic 別 LLM 分離 — Reflexion 補強用 critic 機構 (Phase 1: 同 backend variation)

> **由来 meta-plan**: `.claude/plan/building-ai-coding-agents-gap-analysis.md` § 4 G1 (★★★ 最高優先)
> **由来項目**: 項目 1 (Reflexion / 同一 LLM 内 self-critique)、項目 17-24/89 (Advisor max_uses=3、verification_prompt)、項目 163 (HttpAdvisorJudge / Phase B1 Judge Gate)、項目 210 (Self-Verification Dilemma 動的 skip、AdvisorConfig 2 フィールド先行)、項目 213 (ERL Heuristics、heuristic_reflection.txt 同居)
>
> **位置付け**: bonsai 既存の Reflexion (`agent/agent_loop/advisor_inject.rs::inject_verification_step` 自己 critique) は **同一 LLM 内完結**。Cursor / Devin / Claude Code 等で実証されている **「critic を別ロール / 別 LLM」分離** パターンを段階的に導入する Phase 1。
>
> **Phase 1 scope (本 plan)**: **同一 backend (llama-server) を流用**しつつ、(a) 別 system prompt + (b) 別 temperature (0.7) で **critic 役を独立呼出**する経路を追加。SeparateBackend (gpt-4 等の真の別 model) は **Phase 2 の派生 plan** で段階導入する設計。
>
> **production code 変更ゼロ前提** — 本 plan merge 時点では plan ファイル 1 件追加のみ。実装は別 session で `/ccg:execute` 経由で起動。

## Task Type
- [ ] Frontend
- [x] Backend (`runtime/model_router.rs::CriticConfig` 新規 / `agent/agent_loop/advisor_inject.rs::inject_critic_review` 新規 / `prompts/critic.txt` 新規 / `observability/audit.rs::AuditAction::CriticCall` 追加 / SCHEMA バンプ不要)
- [ ] Fullstack

## 1. 背景

### 1.1 bonsai 既存実装の到達度 — Reflexion 系の "同一 LLM 完結" 限界

`agent/agent_loop/advisor_inject.rs` には 2 つの自己批評経路が存在:

| 経路 | 関数 | 目的 | LLM | 既存項目 |
|---|---|---|---|---|
| **Reflexion 系** | `inject_verification_step` | iteration > 0 + 複雑タスクで FinalAnswer 直前に self-critique | **executor 同一** (Bonsai-8B) | 項目 1, 17, 18, 89 |
| **再計画系** | `inject_replan_on_stall` | 停滞時に別ロールで再計画 | **executor 同一** | 項目 9 |
| **Lab 評価系** | `judge.rs::HttpAdvisorJudge` | Lab benchmark 終了後の rubric 採点 | 別 LLM 可 (`api_endpoint` 設定時) | 項目 163 |

**観察**: judge.rs は既に **別 LLM 呼出 infrastructure を持つ** (`AdvisorConfig::try_remote_with_prompt` / `try_claude_code_with_prompt`、構造化 system+user prompt 対応)。だが運用は **Lab 評価専用** で、agent loop の **step 中** には呼ばれない。

### 1.2 G1 概念 — Critic 分離の系譜

| Agent | Critic 設計 | bonsai 既存対応物 |
|---|---|---|
| Cursor | inline critic (別 system prompt + 同 model) | (G1 Phase 1 で対応) |
| Devin | verifier (別 model) | (G1 Phase 2 で対応) |
| Claude Code | self-review pass (別 system prompt + 同 model) | (G1 Phase 1 で対応) |
| Aider | code review pass (Architect mode で別 prompt) | (G1 Phase 1 で対応) |

3/4 が **「別 system prompt + 同 model」** で運用 = Phase 1 はこれを bonsai に落とすだけで論文・主要 agent と一致する。

### 1.3 1bit Bonsai-8B 文脈での意義

- Reflexion 単独で self-critique させると **「同じ思考癖」「同じ盲点」** を抱えたまま判断 → 誤りを見逃す
- 別 system prompt (critic 専用 persona) + 別 temperature (0.7、executor は 0.3) で **仮想的に別ロール** を作るだけでも、Reflexion miss の捕捉率が上がる仮説 (論文 G1 主張)
- 1bit モデルは特に self-critique 精度が低い (Lab v17 で Self-Verify dynamic skip が REJECT、Lab 天井 7 連続) → critic 機構自体の追加が **構造変異の next 候補**

### 1.4 動機: Lab 天井 7 連続打開への補助線

Lab v8/v9/v10/v14/v15/v16/v17 全 REJECT。CLAUDE.md 項目 215 副次 finding でも「prompt-level / config-level / context-level 3 軸構造変異枯渇」と確定。本 plan は **「step 中の別 LLM-role 呼出 = 第 4 軸 (multi-role variation)」** を導入することで天井打破の構造変異候補となる。

### 1.5 既存項目との親和性

- **項目 89**: AdvisorConfig 拡張で `verification_prompt` / `replan_prompt` を持つ → 同 pattern で `critic_system_prompt` 追加するだけ
- **項目 210**: Self-Verify dynamic skip で `dynamic_skip_threshold` / `min_samples_for_skip` を `AdvisorConfig` に additive 拡張 = 同じ書式で CriticConfig 系列を別 struct で並列追加可能
- **項目 213**: heuristic_reflection.txt が `prompts/` 下にある = `prompts/critic.txt` も同居で OK

## 2. 目的

1. **critic 独立性確保**
   - executor (Bonsai-8B / 同 backend) の出力を、**別 system prompt + 別 temperature** で再評価する経路を追加
   - Reflexion (`inject_verification_step`) と **直交** に動く (両方 ON 可能、片方のみも可能)
2. **Reflexion 補強 (補完関係)**
   - Reflexion = self-critique (同思考)、Critic = role-shift critique (異視点) として **2 段防衛**
   - Reflexion REJECT (Lab v17 Self-Verify) 経路が dead-code 候補化されつつあるが、本 plan はその経路を **拡張** ではなく **代替候補** として位置付け
3. **別 model gradual transition path 整備**
   - Phase 1: 同 backend + 別 prompt + 別 temperature (本 plan、低リスク、token cost +20-30%)
   - Phase 2 (派生): `CriticBackend::SeparateBackend` 実装 (別 backend、cost +50%、品質期待 +)
   - Phase 1 で `CriticBackend` enum を additive 設計にしておく (後方互換維持)

### 非目標
- Reflexion (`inject_verification_step`) の削除 — 共存設計
- 別 backend (gpt-4 等) の必須化 — Phase 1 は同 backend + variation のみ
- 既存 `HttpAdvisorJudge` (Lab 評価専用) の振る舞い変更 — judge.rs は read-only
- Lab paired t-test での効果検証 (Phase 5 別 plan、本 plan の delivery 範囲外)
- production default ON 化 — env opt-in default OFF (項目 214/217-219 と同 pattern)

## 3. 既存項目との関係

| 項目 | 関係 | 本 plan での扱い |
|---|---|---|
| **1 (Reflexion)** | 同一 LLM self-critique 既存 | 共存。本 plan は **別** 経路で並列、段階的に独立性を強化 |
| **9 (Replan / Continue Sites)** | 停滞時別ロール re-plan 既存 | 影響なし。`inject_replan_on_stall` は本 plan と独立 |
| **17/18 (Advisor verify max_uses=3)** | Reflexion 上限機構 | critic は **独立カウンタ** (`critic_calls_used`) で並列管理 (advisor max_uses=3 と直交) |
| **89 (verification_prompt 統一文字列)** | AdvisorConfig prompt フィールド | 同 pattern で `CriticConfig::critic_system_prompt` 追加 (デフォルト: `prompts/critic.txt` ロード) |
| **163 (HttpAdvisorJudge / Lab 評価)** | judge.rs に別 LLM call infrastructure 既存 | **直接流用候補**: `AdvisorConfig::try_remote_with_prompt` / `try_claude_code_with_prompt` を Phase 2 SeparateBackend で活用予定。Phase 1 では judge.rs read-only |
| **210 (Self-Verify dynamic skip)** | AdvisorConfig に skip 機構 | 共存。critic も dynamic_skip_threshold で skip 可能化を Phase 2 候補で検討 |
| **211 (Self-Verify Phase 5 Lab variant)** | 動的 skip threshold の Lab 変異機構 | Phase 5 Lab plan で同 pattern (focus filter による critic threshold 変異) 流用候補 |
| **212 (Lab v16 Self-Verify REJECT)** | 同一 LLM 内 skip 機構の効果限界 evidence | 本 plan の動機 = 同一 LLM 完結の限界打破、別ロール導入の必要性 |
| **213 (ERL Heuristics)** | `prompts/heuristic_reflection.txt` 同居先例 | `prompts/critic.txt` を同 dir に配置、`include_str!` pattern 流用 |
| **214 (Lab v17 toggle)** | env opt-in pattern 確立 | `BONSAI_CRITIC_ENABLED=1` で同 pattern (default OFF) |
| **215 (Lab v17 REJECT, 天井 7 連続)** | 構造変異枯渇 evidence | 本 plan は **第 4 軸 (multi-role variation)** で天井打破候補 |

## 4. 設計

### 4.1 `CriticConfig` struct (新規、`runtime/model_router.rs` 追記)

`AdvisorConfig` と並列追加 (同 module、可視性同等)。`AdvisorConfig` に追加するか別 struct にするかで設計判断: **別 struct** で進める (理由: AdvisorConfig は既に 16 フィールドで肥大化、scope creep 回避、Phase 5 REJECT 時の dead-code 化が容易)。

```rust
/// G1 Critic 別 LLM 分離 — step 中の独立 critique 機構
///
/// **Phase 1**: 同 backend (Bonsai-8B llama-server) を流用、別 system prompt + 別 temperature で
/// critic 役を独立呼出。executor の Reflexion と並列に動く。
///
/// **Phase 2 (派生 plan)**: SeparateBackend 経由で真の別 model (gpt-4-class) と接続。
///
/// 由来: building-ai-coding-agents-gap-analysis.md G1
#[derive(Debug, Clone)]
pub struct CriticConfig {
    /// `BONSAI_CRITIC_ENABLED=1` で opt-in。default OFF で観測動作完全互換。
    pub enabled: bool,
    /// critic 呼出モード (Phase 1 は最初の 2 variant のみ実装、SeparateBackend は Phase 2)
    pub mode: CriticMode,
    /// critic 呼出の最大回数 (advisor max_uses と独立)
    pub max_critic_uses: usize,
    /// 現在の呼出回数
    pub critic_calls_used: usize,
    /// critic 専用 system prompt (default: `include_str!("../../prompts/critic.txt")`)
    pub critic_system_prompt: String,
    /// critic 呼出時の temperature override (default 0.7、executor の 0.3 と差別化)
    pub critic_temperature: f32,
    /// critic 応答の最大トークン数 (推奨 400-700、advisor max_advisor_tokens と独立)
    pub max_critic_tokens: usize,
    /// critic 呼出の hook 位置 (Phase 1: AfterStepOutcome のみ実装、BeforeToolCall は Phase 2 候補)
    pub hook: CriticHook,
    /// disagreement 検出時の挙動
    pub on_disagreement: CriticDisagreementAction,
}

/// critic 呼出モード — Phase 1 で 2 variant、Phase 2 で SeparateBackend 追加
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CriticMode {
    /// 同 backend、同 prompt、別 temperature (最低リスク、Phase 1)
    #[default]
    SamePromptDifferentTemperature,
    /// 同 backend、別 system prompt (critic.txt)、別 temperature (Phase 1 中核)
    DifferentSystemPrompt,
    /// 別 backend (gpt-4-class) — **Phase 2 派生 plan で実装**、Phase 1 では `unimplemented!()` panic
    SeparateBackend,
}

/// critic 呼出の hook 位置
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CriticHook {
    /// Phase 1 — step outcome 後に critic 評価 (= Reflexion と同じ位置だが別 role)
    #[default]
    AfterStepOutcome,
    /// Phase 2 候補 — tool 呼出前に「この tool 呼出は妥当か」を critic に確認
    BeforeToolCall,
}

/// critic が executor に disagree した時の挙動
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CriticDisagreementAction {
    /// disagree を session に inject (executor が次 iteration で読む) — Phase 1 default
    #[default]
    InjectAsSystemMessage,
    /// disagree を log only (production 影響なし) — smoke / shadow mode
    LogOnly,
    /// disagree で session.add_message を抑止 + replan を inject (Phase 2 候補)
    ForceReplan,
}
```

### 4.2 `CriticMode` (= meta-plan G1 の `CriticBackend`) enum 詳細

Phase 1 = 最初の 2 variant のみ実装。

| variant | Phase | LLM 数 | token cost | 実装方法 |
|---|---|---|---|---|
| `SamePromptDifferentTemperature` | **1 (default)** | 1 (Bonsai-8B) | +20% | 同 LlmBackend、`generate` 引数で temperature 0.7 override |
| `DifferentSystemPrompt` | **1 (recommended)** | 1 (Bonsai-8B) | +25% | 同 LlmBackend、別 system prompt (critic.txt) + temperature override |
| `SeparateBackend` | **2 (派生)** | 2 (Bonsai-8B + remote) | +50% | `AdvisorConfig::try_remote_with_prompt` を critic call で使用 (judge.rs と同 infrastructure) |

**Phase 1 で SeparateBackend variant 自体は enum に追加** (additive 設計)、**実装は `unimplemented!("Phase 2 派生 plan で実装")`** で Phase 2 移行時に enum 拡張不要 (= 後方互換維持)。

### 4.3 critic prompt template (`prompts/critic.txt` 新規)

`include_str!("../../prompts/critic.txt")` で `CriticConfig::critic_system_prompt` の default にロード (項目 213 `heuristic_reflection.txt` と同 pattern)。

```
あなたは1bitローカルLLMの**critic**です。executor が出した次の応答や行動を、第三者視点で評価してください。

【あなたのロール】
- executor とは別人格として、思考の盲点・前提の誤り・代替手段を探す
- 同意するならその理由、反対するならその根拠を 100 語以内・箇条書き

【評価軸】
1. 事実誤認 — ツール結果と矛盾していないか
2. 論理破綻 — 思考連鎖に飛躍がないか
3. 見落とし — タスク完了条件の一部を欠いていないか
4. 代替案 — 別の手順がより効果的でないか

【出力形式】
- 同意: "AGREE: <1 行根拠>"
- 反対: "DISAGREE: <2-3 行根拠と修正提案>"
- 判断保留: "UNCERTAIN: <不足情報>"

executor の温度は 0.3、あなたの温度は 0.7。**executor の癖と異なる視点** を必ず提示すること。
```

### 4.4 Reflexion との接続経路 — `agent_loop/advisor_inject.rs` の hook 配置

Phase 1 では既存 `inject_verification_step` の **直後** に並列 hook として `inject_critic_review` を追加 (Reflexion と Critic を共存)。

呼出位置 (`agent_loop/core.rs`、loop 内 FinalAnswer 直前付近):
```rust
// 既存 Reflexion 経路 (inject_verification_step が session.add_message 済み)
let reflexion_injected = inject_verification_step(/* ... */);

// 新規: Critic 経路 (Reflexion と独立に発火、env opt-in)
if critic.enabled
    && critic.can_critique()
    && critic.hook == CriticHook::AfterStepOutcome
{
    let critic_result = inject_critic_review(
        session,
        critic,
        backend,                // Phase 1: 同 LlmBackend、Phase 2 で別 backend 追加
        task_context,
        answer,                  // executor の最新応答
        store,
    )?;
    // critic_result が Disagreement なら on_disagreement に従って分岐
}
```

新規関数 signature (advisor_inject.rs):
```rust
pub(super) fn inject_critic_review(
    session: &mut Session,
    critic: &mut CriticConfig,
    backend: &dyn LlmBackend,         // Phase 1 同 backend、Phase 2 で別 backend 追加
    task_context: &str,
    answer: &str,
    store: Option<&MemoryStore>,
) -> anyhow::Result<CriticOutcome>;
```

戻り値:
```rust
pub enum CriticOutcome {
    Agree { raw_response: String },
    Disagree { raw_response: String, suggested_revision: Option<String> },
    Uncertain { raw_response: String },
    Skipped { reason: &'static str },  // can_critique() == false 等
    BackendError { err: String },      // production 影響なし、log + 続行
}
```

### 4.5 env opt-in default OFF (Cerememory 三本柱と同 pattern)

| env | default | 効果 |
|---|---|---|
| `BONSAI_CRITIC_ENABLED` | `0` (OFF) | unset / `0` / `false` で `CriticConfig::enabled = false` → `inject_critic_review` 冒頭 short-circuit |
| `BONSAI_CRITIC_MODE` | `same_temp` | `same_temp` / `different_prompt` / `separate_backend` (Phase 2 で panic) |
| `BONSAI_CRITIC_TEMPERATURE` | `0.7` | `f32::parse`、不正値は default にフォールバック (= legacy 既定) |
| `BONSAI_CRITIC_MAX_USES` | `3` | advisor max_uses と独立 |
| `BONSAI_CRITIC_HOOK` | `after_step` | `after_step` (Phase 1) / `before_tool` (Phase 2 で panic) |
| `BONSAI_CRITIC_DISAGREEMENT` | `inject` | `inject` (default) / `log_only` (shadow mode) / `force_replan` (Phase 2 候補) |

**production default OFF の根拠**:
- 項目 214 (Lab v17 toggle)、項目 217-219 (Cerememory 三本柱) と pattern 統一
- env unset で観測動作完全互換 (1158→1150 passed 維持)
- Phase 5 Lab paired t-test ACCEPT 時のみ defaults 昇格検討 (項目 215 ERL pattern 踏襲)

### 4.6 Lab metric — critic agreement rate / disagreement rate

`MultiRunBenchmarkResult` または `Experiment` 構造体に **informational metric** として追加 (ACCEPT 判定不変、項目 200 Beyond pass@1 と同 pattern):

```rust
pub struct CriticStats {
    pub critic_calls: usize,
    pub agree_count: usize,
    pub disagree_count: usize,
    pub uncertain_count: usize,
    pub skipped_count: usize,
    pub backend_error_count: usize,
}

impl CriticStats {
    pub fn agreement_rate(&self) -> Option<f64> {
        let total = self.agree_count + self.disagree_count + self.uncertain_count;
        if total == 0 { None } else { Some(self.agree_count as f64 / total as f64) }
    }
    pub fn disagreement_rate(&self) -> Option<f64> { /* 同様 */ }
}
```

TSV 列拡張 (informational only): 末尾に `critic_calls / critic_agree / critic_disagree` 3 列追加 (項目 200 で TSV 12→15 列拡張済み、本 plan で 15→18 列)。

**Lab ACCEPT 判定** (Phase 5 別 plan): paired t-test で critic ON vs OFF の `composite_score` Δ で判定。`agreement_rate` 自体は副次指標 (副次 finding 候補: ON で std 縮小なら stability 軸 ACCEPT 検討、項目 215 副次 finding 同パターン)。

### 4.7 audit log 拡張

```rust
// observability/audit.rs (既存 enum に variant 追加)
pub enum AuditAction {
    // ... 既存
    CriticCall {
        mode: String,            // "same_temp" | "different_prompt"
        outcome: String,         // "agree" | "disagree" | "uncertain" | "skipped" | "error"
        prompt_len: usize,
        response_len: usize,
        duration_ms: u64,
    },
}
```

`as_str()` 関数の match arm に `Self::CriticCall { .. } => "critic_call"` 追加 (audit.rs:107 付近、既存 `AdvisorSkip` の隣)。SQLite テーブルは既存 schema で受け切れる (action_type TEXT + payload JSON)、**migration 不要**。

## 5. TDD strict 5 phase

### Phase 1 — Red (test ≥ 6 件、本 plan は 8-10 件で着地)

新規 test ファイル: `src/agent/agent_loop/tests.rs` 末尾に追加 (既存 tests.rs パターン踏襲) または `src/agent/agent_loop/tests_critic.rs` 新規分離 (size 抑制)。判断は実装時。

| # | test 名 | 期待 (Red 時) |
|---|---|---|
| 1 | `t_critic_config_default_disabled` | `CriticConfig::default().enabled == false`、`mode == SamePromptDifferentTemperature` |
| 2 | `t_critic_config_env_enabled_parse` | `BONSAI_CRITIC_ENABLED=1` 環境変数で `enabled == true`、`mode` も env から override |
| 3 | `t_critic_short_circuit_when_disabled` | `enabled=false` 時 `inject_critic_review` が `Skipped { reason: "disabled" }` 即 return、backend 呼出ゼロ (MockLlmBackend.calls() で確認) |
| 4 | `t_critic_invokes_backend_with_critic_prompt` | `enabled=true, mode=DifferentSystemPrompt` で MockLlmBackend に **critic_system_prompt の文字列** が渡されること (mock spy で確認) |
| 5 | `t_critic_invokes_with_temperature_override` | temperature=0.7 が `generate` 引数の GenerateOptions / temperature override に反映されること |
| 6 | `t_critic_parses_agree_response` | mock 応答 `"AGREE: looks correct"` で `CriticOutcome::Agree { raw_response }` 返却 |
| 7 | `t_critic_parses_disagree_with_revision` | mock 応答 `"DISAGREE: missing X\n修正案: Y"` で `Disagree { suggested_revision: Some("Y") }` |
| 8 | `t_critic_max_uses_enforced` | `max_critic_uses=2` で 3 回目呼出が `Skipped { reason: "max_uses" }` |
| 9 | `t_critic_audit_log_emitted` | critic 呼出後に `AuditLog` に `CriticCall` action が 1 件追加されること |
| 10 | `t_critic_separate_backend_phase1_panic` | `mode=SeparateBackend` で Phase 1 は `unimplemented!()` panic (Phase 2 で実装の意図表明、`#[should_panic]` 適用) |

期待: `cargo test --lib critic` で **全 8-10 件 fail / compile error** で Red 確証。

commit `test(critic): Phase 1 Red — CriticConfig + inject_critic_review 8-10 test`

### Phase 2 — Green

実装ファイル:
1. `src/runtime/model_router.rs` — `CriticConfig` / `CriticMode` / `CriticHook` / `CriticDisagreementAction` / `CriticOutcome` 追加 + Default impl
2. `prompts/critic.txt` 新規 (~25 行、§ 4.3 内容)
3. `src/agent/agent_loop/advisor_inject.rs` — `inject_critic_review` 関数追加 (~80 行)、`parse_critic_response` private helper 追加 (AGREE/DISAGREE/UNCERTAIN 接頭辞 deterministic 判定、`once_cell::sync::Lazy` で正規表現キャッシュ)
4. `src/agent/agent_loop/core.rs` — Reflexion (`inject_verification_step`) 直後の hook 追加 (5-10 行、`if critic.enabled && ...`)
5. `src/observability/audit.rs` — `AuditAction::CriticCall` variant 追加 + `as_str()` match arm
6. `src/agent/benchmark.rs` — `CriticStats` 構造体 + `MultiRunBenchmarkResult` への `Option<CriticStats>` field 追加 + `run_k` 内集計 (Phase 4 smoke で空集計から始め、本 plan delivery 範囲は struct 定義 + 集計 hook までで Lab metric 完全配線は Phase 4)
7. env パーサ helper: `runtime/model_router.rs::CriticConfig::from_env()` 関数 (`std::env::var` ベース、項目 214 / 217-219 と pattern 統一)

期待:
- `cargo test --lib` で **既存 1150 passed + 新規 8-10 test = 1158-1160 passed**
- `cargo clippy --lib --tests -- -D warnings` clean
- `cargo fmt --check` clean
- env unset (default) で既存全 test 退行ゼロ (BONSAI_CRITIC_ENABLED 未設定 = `enabled=false` short-circuit)

commit `feat(critic): Phase 2 Green — CriticConfig + inject_critic_review + audit + critic.txt`

### Phase 3 — Refactor

- `parse_critic_response` の正規表現キャッシュ (`once_cell::sync::Lazy<Regex>` × 3 = AGREE/DISAGREE/UNCERTAIN)
- `inject_critic_review` 内の `match critic.mode` を early-return ladder に整理 (collapsible_if 警告対策)
- docstring 整備 — 各 pub 構造体に「由来 plan: critic-separate-llm-impl.md / G1 / 項目候補 223」明記
- env パース失敗時の挙動を `default + log warn` に統一 (項目 214 toggle と pattern 統一)
- test mutex (env mutation race 回避、項目 214 と同 pattern):
  ```rust
  static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
  fn with_critic_env(env: &[(&str, &str)], f: impl FnOnce()) { /* ... */ }
  ```

commit `refactor(critic): Phase 3 — regex cache + early return + docstring + env mutex`

### Phase 4 — Smoke (3 段)

#### G-4a: 既存経路後方互換 (env unset)
```bash
cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
```
期待: critic call 0 回、score / pass@k / duration が項目 207 baseline (0.7812) ± variance、TSV `critic_calls=0`

#### G-4b: critic ON / different_prompt / log_only (shadow mode)
```bash
BONSAI_CRITIC_ENABLED=1 \
  BONSAI_CRITIC_MODE=different_prompt \
  BONSAI_CRITIC_DISAGREEMENT=log_only \
  BONSAI_LAB_SMOKE=1 \
  ./target/release/bonsai --lab --lab-experiments 0
```
期待: critic call ≥ 1 (smoke task 7 件のうち 2-step 以上で発火)、`AuditAction::CriticCall` log emit、production 動作影響ゼロ (DisagreementAction=LogOnly)、score ± variance

#### G-4c: critic ON / different_prompt / inject (production-like)
```bash
BONSAI_CRITIC_ENABLED=1 \
  BONSAI_CRITIC_MODE=different_prompt \
  BONSAI_CRITIC_DISAGREEMENT=inject \
  BONSAI_LAB_SMOKE=1 \
  ./target/release/bonsai --lab --lab-experiments 0
```
期待: critic disagree 時に session.add_message が発火、score Δ ∈ [-0.05, +∞)、duration +20-30% (token cost 増加分)

判定:
- ✅ G-4a: 既存挙動完全互換 (1158-1160 passed 維持)
- ✅ G-4b: critic 呼出経路 wiring 確認、production 影響ゼロ
- ✅ G-4c: critic inject 経路 wiring 確認、score lenient gate (Δ ≥ -0.05)

### Phase 5 — Lab effectiveness (別 plan、本 plan delivery 範囲外)

- 別 plan ファイル `lab-v18-critic-effectiveness.md` を起票
- core 22 / k=3 / paired t-test (env=enabled vs env=unset) で `composite_score` Δ を計測
- ACCEPT 判定: (a) Δ mean ≥ +0.015 AND (b) one-sided p < 0.1 (項目 215 Lab v17 と同基準)
- ACCEPT → defaults 昇格検討 (env default ON、本 plan 7 章 R3 mitigation 参照)
- REJECT → dead-code 候補化 (項目 215 ERL pattern、項目 222 sqlite-vec wiring 削除 pattern 踏襲)

## 6. API 影響

### 6.1 公開 API (新規)

| modulo path | 種別 | 備考 |
|---|---|---|
| `runtime::model_router::CriticConfig` | pub struct | 9 field、Default impl 完備 |
| `runtime::model_router::CriticMode` | pub enum | 3 variant (Phase 1 で 2 variant 実装、SeparateBackend は Phase 2 で `unimplemented!()`) |
| `runtime::model_router::CriticHook` | pub enum | 2 variant (AfterStepOutcome / BeforeToolCall、後者 Phase 2 で `unimplemented!()`) |
| `runtime::model_router::CriticDisagreementAction` | pub enum | 3 variant (Inject / LogOnly / ForceReplan、後者 Phase 2) |
| `runtime::model_router::CriticConfig::from_env` | pub fn | env から構築 |
| `runtime::model_router::CriticConfig::can_critique` | pub fn | `enabled && critic_calls_used < max_critic_uses` |
| `runtime::model_router::CriticConfig::record_call` | pub fn | counter +1 |
| `agent::benchmark::CriticStats` | pub struct | informational metric |
| `observability::audit::AuditAction::CriticCall` | enum variant | 既存 enum に additive |

### 6.2 内部 API (新規 pub(super) / pub(crate))

| API | 種別 | 配置 |
|---|---|---|
| `inject_critic_review` | pub(super) fn | `agent_loop/advisor_inject.rs` |
| `parse_critic_response` | private fn | 同上 |
| `CriticOutcome` | pub(super) enum | `agent_loop/advisor_inject.rs` または `runtime/model_router.rs` (実装時に判断) |

### 6.3 公開 API 破壊的変更

**ゼロ**:
- `AdvisorConfig` 既存フィールド unchanged
- `inject_verification_step` signature unchanged
- `BenchmarkSuite::run_k` signature unchanged (`MultiRunBenchmarkResult` への `Option<CriticStats>` 追加は serde default + skip_if_none で additive)
- `MultiRunBenchmarkResult` 既存フィールド unchanged

env unset で既存挙動 100% 維持 = 1150 passed 退行ゼロ。

## 7. Risks / Mitigations

| # | Risk | severity | Mitigation |
|---|---|---|---|
| **R1** | critic 自体が hallucinate して executor を誤誘導 | **HIGH** | (a) Phase 1 default `DisagreementAction::LogOnly` 推奨運用 (Phase 4 G-4b で shadow 検証) (b) 強制力高い `ForceReplan` は Phase 2 派生 (c) Phase 5 Lab paired t-test で REJECT 時 dead-code 化 (項目 222 pattern) |
| **R2** | token cost 倍増で Lab cycle 時間 +50% (90 min → 135 min) | MEDIUM | (a) Phase 1 default `max_critic_tokens=400` で advisor max_advisor_tokens=700 より小 (b) `max_critic_uses=3` で advisor max_uses=3 と独立、合計呼出回数上限管理 (c) Lab smoke で duration +30% 以下 gate (G-4c) |
| **R3** | env default OFF のままでは production 価値ゼロ | MEDIUM | Phase 5 Lab ACCEPT 時 defaults 昇格 (項目 215 ERL pattern)、ACCEPT 基準 = paired t-test Δ ≥ +0.015 + p < 0.1 (Lab v17 と同基準) |
| **R4** | advisor max_uses=3 制約と独立で「reflexion 3 + critic 3 = 6 回 LLM call/step」が暴走 | MEDIUM | (a) `critic_calls_used` を `advisor.calls_used` と独立カウント (b) `inject_critic_review` 冒頭で `critic.can_critique()` gate (c) Phase 4 G-4b smoke で実 call 回数を audit log で確認 (d) `max_critic_uses` default 3 で advisor max_uses=3 と対称、合計上限 6 を超えない |
| **R5** | parse_critic_response が AGREE/DISAGREE 接頭辞を 1bit モデルが守らず always Uncertain 化 | HIGH | (a) `prompts/critic.txt` で出力形式を厳格指定 (b) parse_critic_response の正規表現を case-insensitive、複数行容認 (c) Uncertain 多発時は `Skipped { reason: "parse_failed" }` 扱いで production 影響ゼロ (d) Phase 4 G-4b で Uncertain 比率 ≤ 50% gate |
| **R6** | `BONSAI_CRITIC_ENABLED` 未設定でも `from_env` 内 partial 設定 (mode のみ等) で誤動作 | LOW | `enabled=false` 時は他フィールドを完全無視する short-circuit を `inject_critic_review` 冒頭で確証 (test #3 でカバー) |
| **R7** | Phase 5 Lab 効果測定で「Reflexion vs Critic」のどちらが効いたか分離不能 | MEDIUM | Phase 5 Lab variant 設計時に 4 cell (Reflexion ON/OFF × Critic ON/OFF) factorial 推奨、本 plan delivery 範囲外 (Lab plan で扱う) |
| **R8** | `AuditAction::CriticCall` payload JSON 肥大化で audit テーブル inflation | LOW | prompt_len / response_len のみ記録、本文は session message に含まれるため重複保存しない |
| **R9** | env mutation race で Phase 1 test の決定論性損失 | MEDIUM | test mutex 導入 (項目 214 と同 pattern)、Phase 3 Refactor で対応 |
| **R10** | critic.txt 改変で Lab 結果が再現不可能 | LOW | `include_str!` で binary 内に埋込、git 履歴で改変追跡可。Phase 5 Lab plan で「critic.txt git hash」を Experiment metadata に記録 |
| **R11** | Phase 2 SeparateBackend variant の `unimplemented!()` がうっかり production code path で呼ばれる | LOW | `from_env` で `BONSAI_CRITIC_MODE=separate_backend` 設定時は warn log + default (`SamePromptDifferentTemperature`) に置換、`unimplemented!()` 到達 = test の `#[should_panic]` のみ |

## 8. Quality Gates

### G-1 Phase 1 Red
- 8-10 新規 test 全 fail or compile error (`cargo test --lib critic`)
- 既存 1150 passed 維持

### G-2 Phase 2 Green
- 既存 1150 + 新規 8-10 test = **1158-1160 passed**
- `cargo clippy --lib --tests -- -D warnings` clean (新規 warning ゼロ)
- `cargo fmt --check` clean
- env unset (default) で既存全 test 退行ゼロ

### G-3 Phase 3 Refactor
- regex cache (once_cell) 導入確認
- 全 pub 構造体 docstring (G1 由来 + 項目候補 223 cross-ref)
- test mutex 導入 (env race 回避)
- 1158-1160 passed 維持

### G-4 Phase 4 Smoke (3 段)
- **G-4a (env unset)**: 既存挙動完全互換、`critic_calls=0`、score / duration が項目 207 baseline ± variance
- **G-4b (different_prompt + log_only)**: critic call ≥ 1、AuditAction::CriticCall emit、production 動作影響ゼロ、Uncertain 比率 ≤ 50% (R5 gate)
- **G-4c (different_prompt + inject)**: critic inject 経路 wiring 確認、score Δ ≥ -0.05 (lenient)、duration +30% 以下 (R2 gate)

### G-5 Lab effectiveness (別 plan、本 plan delivery 範囲外)
- core 22 / k=3 / paired t-test
- ACCEPT 基準: (a) Δ mean ≥ +0.015 AND (b) one-sided p < 0.1
- 副次 finding (項目 215 同パターン): stability 軸 (std 縮小) で informational ACCEPT 検討

### G-6 Final
- CLAUDE.md 項目 223 (or 次番) 候補追記
- handoff 起票 (`session_2026_05_XX_handoff.md`)
- INDEX.md 「Building AI Coding Agents 派生」section に G1 リンク追加 (meta-plan 由来明示)

G-1 〜 G-4 PASS で本 plan delivery 完了 (Phase 5 / G-5 は別 plan)。

## 9. 完了条件

1. ✅ `CriticConfig` / `CriticMode` / `CriticHook` / `CriticDisagreementAction` / `CriticOutcome` 追加 (`runtime/model_router.rs`)
2. ✅ `prompts/critic.txt` 新規 (§ 4.3 内容、~25 行)
3. ✅ `inject_critic_review` 関数追加 (`agent_loop/advisor_inject.rs`)
4. ✅ `core.rs` の Reflexion 直後に critic hook 配置 (`if critic.enabled` short-circuit)
5. ✅ `AuditAction::CriticCall` variant 追加 (`observability/audit.rs`)
6. ✅ `BONSAI_CRITIC_ENABLED` env opt-in、default OFF (項目 214 / 217-219 と pattern 統一)
7. ✅ `CriticStats` informational metric (`benchmark.rs`、Phase 4 smoke で集計確認)
8. ✅ TDD strict 5 phase 全消化 (Phase 1 Red → Phase 2 Green → Phase 3 Refactor → Phase 4 Smoke 3 段 → Phase 5 別 plan 起票確証)
9. ✅ 既存 1150 passed 維持、新規 8-10 test 追加で 1158-1160 passed
10. ✅ clippy 0 / fmt clean / API 完全 additive (signature 変更ゼロ)
11. ✅ smoke G-4a/b/c 全 PASS
12. ✅ CLAUDE.md 項目 223 候補追記 + handoff 起票 + INDEX.md G1 リンク

## 10. 見積もり

| Phase | 内容 | 時間 |
|---|---|---|
| **P0 (調査)** | judge.rs / advisor_inject.rs / model_router.rs 既読、env opt-in pattern 確認 | 0.3h |
| **P1 (Red)** | test 8-10 件追加、cargo test 全 fail 確認 | 1.0h |
| **P2 (Green)** | CriticConfig + enums + inject_critic_review + parse + AuditAction + critic.txt + core.rs hook + CriticStats hook | 4.0h |
| **P3 (Refactor)** | regex cache + early return + docstring + env mutex | 1.0h |
| **P4 (Smoke 3 段)** | G-4a env unset (5 min) + G-4b log_only smoke (15 min) + G-4c inject smoke (20 min) + 解析 + 修正 buffer | 3.0h (実機 wall ~40 min) |
| **P6 (commit + handoff + CLAUDE.md)** | 5 commits + handoff 起票 + CLAUDE.md 項目 223 + INDEX.md | 0.7h |
| **計** | | **~10h ≈ 1.25-1.5 day** |

Phase 5 (Lab v18 critic effectiveness paired t-test) は別 plan ~6h、本 plan delivery 範囲外。

派生 plan 候補 (本 plan ACCEPT 後):
- `critic-separate-backend-phase2-impl.md` (gpt-4 等の真の別 model 接続、~1.5 day)
- `lab-v18-critic-effectiveness.md` (paired t-test、~6h)
- `critic-before-tool-call-phase2-impl.md` (BeforeToolCall hook、~1 day)

## 11. Quick Start

```bash
# 0. 既存実装確認 (production code 変更ゼロ確証)
rtk grep -n "CriticConfig\|inject_critic_review\|CriticMode\|critic.txt" src/  # 期待 0 件
rtk grep -n "BONSAI_CRITIC" src/                                                # 期待 0 件
ls /Users/keizo/bonsai-agent/prompts/                                           # heuristic_reflection.txt のみ

# 1. Phase 1 Red — test 追加
$EDITOR src/agent/agent_loop/tests.rs  # または tests_critic.rs 新規
rtk cargo test --lib critic  # 全 8-10 件 fail / compile error 確認
git commit -m "test(critic): Phase 1 Red — CriticConfig + inject_critic_review 8-10 test"

# 2. Phase 2 Green — 実装
$EDITOR src/runtime/model_router.rs              # CriticConfig + enums + from_env
$EDITOR prompts/critic.txt                        # 新規 (§ 4.3 内容)
$EDITOR src/agent/agent_loop/advisor_inject.rs   # inject_critic_review + parse_critic_response
$EDITOR src/agent/agent_loop/core.rs             # Reflexion 直後 hook
$EDITOR src/observability/audit.rs               # CriticCall variant
$EDITOR src/agent/benchmark.rs                   # CriticStats + run_k 集計 hook
rtk cargo test --lib   # 1158-1160 passed
rtk cargo clippy --lib --tests -- -D warnings
rtk cargo fmt --check
git commit -m "feat(critic): Phase 2 Green — CriticConfig + inject_critic_review + audit + critic.txt"

# 3. Phase 3 Refactor
$EDITOR src/agent/agent_loop/advisor_inject.rs   # regex cache (once_cell) + early return
$EDITOR src/runtime/model_router.rs              # docstring 整備 + env mutex
git commit -m "refactor(critic): Phase 3 — regex cache + early return + docstring + env mutex"

# 4. Phase 4 Smoke 3 段
rtk cargo build --release

# G-4a: 既存経路後方互換
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/critic_g4a.log
grep "critic_calls" /tmp/critic_g4a.log  # 期待 0

# G-4b: log_only shadow mode
BONSAI_CRITIC_ENABLED=1 BONSAI_CRITIC_MODE=different_prompt BONSAI_CRITIC_DISAGREEMENT=log_only \
  BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/critic_g4b.log
grep "critic_call\|CriticCall" /tmp/critic_g4b.log  # 期待 ≥ 1

# G-4c: inject mode
BONSAI_CRITIC_ENABLED=1 BONSAI_CRITIC_MODE=different_prompt BONSAI_CRITIC_DISAGREEMENT=inject \
  BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | tee /tmp/critic_g4c.log

# 5. commit + handoff + CLAUDE.md
$EDITOR CLAUDE.md       # 項目 223 候補追記
$EDITOR .claude/projects/-Users-keizo-bonsai-agent/memory/session_2026_05_XX_handoff.md
$EDITOR .claude/plan/INDEX.md  # G1 リンク追加
git commit -m "docs(critic): G-1〜G-4 PASS + 項目 223 候補 + handoff"

# 6. Phase 5 別 plan 起票 (本 plan delivery 範囲外)
$EDITOR .claude/plan/lab-v18-critic-effectiveness.md
```

## 12. 参考

### 由来 meta-plan
- **`.claude/plan/building-ai-coding-agents-gap-analysis.md`** § 4 G1 (★★★ 高優先、~1.5 day 推定の起票元)

### bonsai 既存 plan (品質基準・TDD strict 5 phase 手本)
- **`.claude/plan/agentfloor-tier-eval-impl.md`** — TDD strict 5 phase + Quality Gates G-1〜G-5 構造手本
- **`.claude/plan/self-verify-dilemma-impl.md`** — AdvisorConfig 拡張先例、`dynamic_skip_threshold` 実装パターン (項目 210)
- **`.claude/plan/cerememory-decay-port-impl.md`** — env opt-in default OFF + license attribution + SCHEMA migration pattern
- **`.claude/plan/erl-heuristics-pool-impl-v2.md`** — `prompts/heuristic_reflection.txt` 同居先例 (項目 213)
- **`.claude/plan/lab-v17-erl-effectiveness.md`** — Phase 5 Lab paired t-test 別 plan の構造手本 (項目 214-215)
- **`.claude/plan/event-repository-trait-impl.md`** — Mock + parity test pattern (項目 209、Phase 1 test の参考)

### bonsai 既存項目 (本 plan で reference する CLAUDE.md 項目)
- **項目 1**: Reflexion (同一 LLM self-critique) — 共存対象
- 項目 9: 停滞時 Replan / Continue Sites
- 項目 17/18: Advisor verify / max_uses=3
- **項目 89**: AdvisorConfig prompt フィールド先例
- **項目 163**: HttpAdvisorJudge / Phase B1 Judge Gate / `try_remote_with_prompt` infrastructure
- **項目 210**: Self-Verification Dilemma 動的 skip (`dynamic_skip_threshold` / `min_samples_for_skip` 先例)
- **項目 211**: Self-Verify Phase 5 Lab variant 機構 (Lab focus filter pattern 流用候補)
- **項目 212**: Lab v16 REJECT (同一 LLM 内 skip 機構の効果限界 evidence、本 plan の動機)
- 項目 213: ERL Heuristics Pool / `prompts/heuristic_reflection.txt`
- 項目 214: Lab v17 toggle 機構 (env opt-in pattern)
- **項目 215**: Lab v17 REJECT (天井 7 連続、構造変異枯渇 evidence、ACCEPT 基準先例)
- 項目 217-219: Cerememory 三本柱 (env opt-in default OFF pattern)
- 項目 222: sqlite-vec wiring 削除 (REJECT 後 dead-code 化 pattern、Phase 5 REJECT 時参考)

### bonsai source files (本 plan で grep / 参照、production code 変更ゼロで Phase 2 着手時の対象)
- `src/runtime/model_router.rs` (`AdvisorConfig`、本 plan で `CriticConfig` 追記先)
- `src/agent/agent_loop/advisor_inject.rs` (Reflexion injection 経路、本 plan で `inject_critic_review` 追加先)
- `src/agent/agent_loop/core.rs` (loop 本体、本 plan で critic hook 配置先)
- `src/agent/judge.rs` (HttpAdvisorJudge、本 plan は judge.rs を **read-only** で infrastructure 流用候補確認のみ)
- `src/observability/audit.rs` (`AuditAction` enum、本 plan で `CriticCall` variant 追加)
- `src/agent/benchmark.rs` (`MultiRunBenchmarkResult`、本 plan で `CriticStats` 追加)
- `prompts/heuristic_reflection.txt` (項目 213 同居先例、本 plan は `prompts/critic.txt` を同 dir 配置)

### 論文・survey
- **arxiv 2603.05344** — Building AI Coding Agents for the Terminal (G1 由来論文、本 plan の主軸)
- arxiv 2602.03485 — Self-Verification Dilemma (項目 210 由来、Reflexion 過剰発動の課題、本 plan の動機補強)
- arxiv 2603.21357 — AgentHER ECHO + HSL (項目 201、hindsight relabel との相性検証候補 Phase 5)

### CODEX_SESSION (for `/ccg:execute` use)
- 新規取得推奨 (本 plan は G1 derivative 起票で項目 213/214/210 の既存 session 不適)
- 既存 session 流用検討時: `019e064a-334c-7692-9735-c5d95231ebf1` (項目 213 ERL plan v2 起票時の session、env opt-in pattern context が近い)

### 失敗時 (Phase 5 effectiveness REJECT) handling
Lab v18 paired t-test で critic-on の Δscore < +0.015:
1. `BONSAI_CRITIC_ENABLED` default 未設定維持 (= legacy 既定、本 plan default と同じ、構造変更不要)
2. `CriticConfig` / `inject_critic_review` を **dead-code 候補化** (項目 215 ERL pattern、項目 222 sqlite-vec wiring 削除 pattern 踏襲)
3. CLAUDE.md に negative finding 記録 (副次知見: critic agreement_rate / disagreement_rate / std 縮小有無を項目 215 pattern で報告)
4. 後続 plan 検討:
   - Phase 2 派生 `critic-separate-backend-phase2-impl.md` で **真の別 model** (gpt-4-class) で再測定 → ACCEPT 可能性が残る
   - 副次 finding (stability 軸 ON 顕著優位) があれば項目 200 RDC/VAF re-eval 候補
   - dead-code 削除 plan は別 session (項目 222 pattern: sqlite-vec wiring 削除と同経路)

## 13. session 05-11b gap analysis 補注 (★ minor、次 session 着手時に注意)

> **補注由来**: handoff `session_2026_05_11b_handoff.md` 完了後、本 plan を deep-read で gap 検査した結果。major blocking gap なし、minor 4 件を以下に集中記録。**次 session 着手 (Phase 2 Green) 時に本 §13 を必ず参照** し、§4.6 / §5 Phase 1-2 / §10 の outdated 数値を実装直前に更新。

### G-1: TSV 列拡張見積もりの outdated (★ low、§4.6)
- plan §4.6 「TSV 12→15 列 (項目 200 で拡張済)、本 plan で **15→18 列**」と記載
- **訂正**: 本 session (commit `ec4bd73` AgentFloor Phase 4 Green) で TSV を **15→21 列に拡張済** (tier_t1..t6 追加)、本 plan の critic 3 列追加は正確には **21→24 列**
- 影響: Phase 2 Green step 6 (`MultiRunBenchmarkResult` への `Option<CriticStats>` field 追加) で TSV 列追加実装時に正確な列番号 22/23/24 を使う

### G-2: 期待 test count の outdated (★ low、§5 Phase 2 + §10)
- plan §5 Phase 2 「**既存 1150 passed + 新規 8-10 test = 1158-1160 passed**」と記載
- **訂正**: 本 session (commit `a52edc6` 項目 224 pre-screen tier fix) で test 1162→**1165 passed** に増、本 plan の正確な期待値は **1165 + 8-10 = 1173-1175 passed**
- 影響: Quality Gate G-2 と完了条件 #6 の数値修正必要、判定基準として再計算

### G-3: `LlmBackend::generate` signature と test #5 期待の不整合 (★ medium、§5 Phase 1)
- plan §5 Phase 1 Red test #5 `t_critic_invokes_with_temperature_override` で「temperature=0.7 が `generate` 引数の **GenerateOptions / temperature override** に反映」と記載
- **訂正**: bonsai 現行 `LlmBackend::generate(messages, tools, on_token, cancel) -> GenerateResult` signature には `GenerateOptions` 引数なし、temperature 制御は `crate::config::InferenceParams` または `AgentConfig.base_inference` 経由の暗黙設定が現状
- **対応必須**: Phase 2 Green 着手時に以下のいずれかを設計判断:
  - **Option A**: `LlmBackend` trait に `generate_with_options(messages, tools, options: &GenerateOptions, on_token, cancel)` 新 method 追加 (additive、既存 `generate` 不変、Mock + LlamaServer 両 impl 必要)
  - **Option B**: critic 呼出時に専用 `AgentConfig` clone + `base_inference.temperature = 0.7` で executor と分離 (signature 変更ゼロ、ただし AgentConfig 全 clone コスト)
  - **Option C**: `LlmBackend::generate` signature を破壊変更 (project-wide 影響大、却下推奨)
- 推奨 = **Option B** (signature 変更ゼロ、本 plan §6 「signature 変更ゼロ」 commitment 維持)、ただし AgentConfig clone は per-call なため performance 影響軽微検証 (Phase 4 G-4b smoke で duration 計測)
- 影響: Phase 1 Red test #5 を Option B 採用前提で書き直し (`AgentConfig.base_inference.temperature` が critic 呼出時のみ 0.7 になることを assert)

### G-4: 既存項目親和性 list 重複 (informational、§1.5 + §3)
- §1.5 「既存項目との親和性」と §3 「既存項目との関係 表」で項目 89/210/213 が両方 list 化されている
- 影響: 認知負荷軽微、実装影響なし、informational only
- 対応 (任意): Phase 5 commit 時に §1.5 を §3 に統合する整理も可、ただし scope creep 回避なら現状維持で OK

### gap analysis サマリー
- **major blocking gap**: 0 件
- **minor (実装影響あり)**: G-3 (LlmBackend signature) — Phase 2 Green 着手前に Option B 採用判断が必要
- **minor (数値 outdated)**: G-1 (TSV 列), G-2 (test count) — Phase 2 / Quality Gate 直前に値更新
- **informational**: G-4 (docs 重複) — 対応任意

### Phase 0 追加 (本 plan §5 に追加、次 session 着手時)
Phase 0a: 本 §13 G-3 の Option B vs A 判断を `/ccg:plan` agent で 30 min discussion (LlmBackend trait 影響 + 全 impl 確認)
Phase 0b: G-1 / G-2 の outdated 数値を §4.6 / §5 Phase 2 / §10 で実装直前に置換
