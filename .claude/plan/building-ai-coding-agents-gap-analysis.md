# Plan (meta): Building AI Coding Agents — Gap Analysis & 派生 plan 起票候補抽出

> **由来**: arxiv 2603.05344 "Building AI Coding Agents for the Terminal: Scaffolding, Harness, Context Engineering" (2026-03)。現代 coding agent (Cursor / Devin / Claude Code / Aider) の architecture survey で、**bonsai-agent の設計原則「Scaffolding > Model」と完全一致** する経験論まとめ。本 plan は survey 知見を bonsai 既存実装と照合し、不足要素 (gap) を派生 plan 候補として起票するための **meta-plan**。
>
> **位置付け (重要)**: 本 plan **自体は実装しない** (production code 変更ゼロ)。gap matrix 完成 + 派生 plan 候補 ≥ 3 件抽出 + docs 整合性確保で完了。各 gap の実装は別 plan / 別 session で起票する設計。
>
> **由来 plan / handoff**: `research_arxiv_2026_05_07.md` 領域 2 (★★★ #10) / CLAUDE.md 項目 198+ / 過去 plan `agentfloor-tier-eval-impl.md` (構造手本) `erl-heuristics-pool-impl-v2.md` (構造変異 plan の手本) `arag-hierarchical-retrieval-docs.md` (docs PR scope の手本)。

## Task Type
- [ ] Frontend
- [ ] Backend
- [ ] Fullstack
- [x] Docs (CLAUDE.md 項目候補 + 派生 plan 起票元、本 plan 自体 production code 変更ゼロ)

## 1. 背景

### 1.1 論文要旨 (arxiv 2603.05344)
"Building AI Coding Agents for the Terminal: Scaffolding, Harness, Context Engineering" は **Cursor / Devin / Claude Code / Aider 等の主要 coding agent** に共通する architecture pattern を survey した経験論まとめ。著者らの主張は明示的に **「model 改善より harness/scaffolding/context engineering の方が ROI が高い」** であり、bonsai-agent の CLAUDE.md 巻頭設計原則 **「Scaffolding > Model」** (1bit モデルの改善余地は限定的、ハーネス側で底上げする) と完全一致する。

### 1.2 現代 coding agent の共通 design pattern (本 plan の照合軸)
論文 (および類縁 survey) から抽出した **10 pattern**:

| # | Pattern | 短い定義 | bonsai 関連既存層 |
|---|---|---|---|
| P1 | Repository indexing | RepoMap / AST / embedding でコードベース構造を pre-compute | `tools/repomap.rs` (PageRank)、`memory/search.rs` (FTS5+vector) |
| P2 | Multi-step planning | Plan → Execute → Verify を強制、計画なしの即実行を抑制 | 項目 10 (計画強制ルール、Lab v6.2 唯一 ACCEPT) |
| P3 | Self-correction | Reflexion + Critic loop で自己評価 → 修正 | 項目 1 (Reflexion)、`agent/agent_loop/core.rs`、`agent/judge.rs` (rubric judge) |
| P4 | Tool selection | 動的 schema、deferred load で context 節約 | 項目 7 (Deferred Schema 80% 節約)、項目 70 (タスク種別ツール制限) |
| P5 | Memory hierarchies | working / episodic / semantic の 3 層 (or それ以上) | 項目 76 (Vault)、項目 77 (Experience)、項目 161 (Skill)、項目 219 (Working Memory Cap) |
| P6 | Test-driven verification | TDD enforcement、test 起点の計画立案 | (なし — gap 候補 G2) |
| P7 | Sub-agent delegation | 子 agent に深度制限付きで委任、エラー境界 | 項目 120/160 (`agent/subagent.rs` SubAgentExecutor、深度 2) |
| P8 | Context management | compaction、token budget、AI+Tool ペア保護 | 項目 6 (4 段 compaction)、項目 187 (ContextOverflowGuard) |
| P9 | Safety guards | sandbox、permission、secrets フィルタ | 項目 42-44 (`safety/secrets.rs`、`safety/sandbox.rs`、`safety/network.rs`) |
| P10 | Adaptive prompting | task-aware system prompt、persona 注入 | 項目 8 (SOUL.md 3 段検索)、項目 80 (contextual memory injection)、項目 179 (MemoryBlock) |

### 1.3 bonsai 既存実装の到達度
本 plan 着手時点で bonsai-agent は **1158→1150 passed** (項目 222 sqlite-vec wiring 削除直後)、69 ソースファイル、886+ テスト基盤、Lab 天井 7 連続 (v8/v9/v10/v14/v15/v16/v17 全 REJECT)。P1/P2/P3/P4/P5/P7/P8/P9/P10 の 9 pattern は何らかの形で実装済、唯一 **P6 TDD enforcement** は agent harness レベルでは未実装 (リポジトリ運用としての TDD は CLAUDE.md で人間向けに明文化されているが、agent 自身が test-first で動くわけではない)。

### 1.4 「Scaffolding > Model」原則と整合
論文の主張 = bonsai の設計原則であり、本 plan の **第一の価値は external validation** (CLAUDE.md 巻頭引用候補)。第二の価値は **gap 特定**: 9/10 pattern を実装済とはいえ、各 pattern の実装深度は様々で、論文知見と比較して「より深く攻めるべき領域」が見える。

### 1.5 動機: Lab 天井 7 連続打破への補助線
Lab v17 まで paired t-test で 7 連続 REJECT、prompt-level / config-level / context-level の 3 軸構造変異が枯渇 (CLAUDE.md 項目 207-215)。本 plan は「論文知見との比較で実装深度を上げる方向」を示すことで、HypothesisGenerator が次に何を試すべきか (= どの派生 plan を起票すべきか) のメタ指針を提供する。

## 2. 目的

1. **bonsai 既存実装の external validation**
   - 9/10 pattern を実装済であることを論文知見と対応付けて記録
   - 「Scaffolding > Model」原則の外部裏付けを CLAUDE.md に組み込む
2. **gap 特定 (gap matrix 作成)**
   - 各 pattern の実装深度 (✅ 完全 / 🟡 部分 / ❌ 未実装) を判定
   - 真の gap (= 論文に存在し bonsai に存在しない or 部分実装) を抽出
3. **派生 plan 起票 (meta-plan としての deliverable)**
   - gap ごとに派生 plan 候補を ≥ 3 件抽出 (優先度・推定工数付き)
   - 各候補は本 plan と独立に起票・実装可能 (本 plan は起票元 root)

### 非目標
- 本 plan 自体での実装 (production code 変更ゼロ、gap matrix と派生 plan 候補のみ)
- 派生 plan の詳細設計 (各 plan は別 session で起票、本 plan は「候補」止まり)
- AgentFloor / ERL / Cerememory 等、既存 plan で扱う pattern の再 plan 化 (重複回避)
- Lab paired t-test での効果検証 (各派生 plan の責務)

## 3. gap matrix (最重要セクション)

### 3.1 判定基準
- ✅ **完全**: 論文の核要素を bonsai が実装済、Lab で実機運用、項目化済
- 🟡 **部分**: 概念は実装、深度や運用範囲が論文比で限定的
- ❌ **未実装**: 概念自体が bonsai に存在しない、または無関係化済 (REJECT 後の dead code 除く)

### 3.2 10 pattern × bonsai 実装の対応表

| # | Pattern | 実装状況 | 既存項目・file | gap 概要 | 派生 plan 候補 | 優先度 |
|---|---|---|---|---|---|---|
| **P1** | Repository indexing | ✅ 完全 | 項目 30 (RepoMap v2) / 74 / 100 / 119 / 148; `tools/repomap.rs` (PageRank); `memory/search.rs` (FTS5+vector RRF); 項目 71 (KnowledgeGraph BFS) | RepoMap が PageRank ベース、AST シンボル抽出は未活用 (現状はファイル単位)。**項目 220-222 で sqlite-vec brute-force KNN を入れたが G-4.2 paired smoke で REJECT、項目 222 で wiring 削除済 = 「ベクトル化未統合」状態に意図的に戻した**。 | (派生なし — 項目 221 REJECT 後経路として decided、再着手は別根拠が出てから) | — |
| **P2** | Multi-step planning | ✅ 完全 | 項目 10 (計画強制ルール、Lab v6.2 唯一 ACCEPT、デフォルト化済); `agent/agent_loop/core.rs` | 短期 multi-step は強制済。**T6 Long-Horizon (5+ step)** で精度大幅劣化、AgentFloor plan で対応中。 | `agentfloor-tier-eval-impl.md` (既存)、本 plan からの新派生なし | — |
| **P3** | Self-correction | 🟡 部分 | 項目 1 (Reflexion); `agent/agent_loop/core.rs` (Self-Reflection step); `agent/judge.rs` (HttpAdvisorJudge rubric); 項目 17/18 (advisor verify、最大 3 回); 項目 210 (Self-Verify dynamic skip) | **Reflexion は同一 LLM 内で完結**。論文知見では Critic を **別 LLM (or 別ロール)** で動かす場面が多い (Devin の verifier、Cursor の inline critic)。bonsai は Critic を別 LLM 化していない (advisor は提案系、judge.rs は最終 rubric)。 | **G1: Critic 別 LLM 分離 plan** (~1.5 day) | ★★★ |
| **P4** | Tool selection | ✅ 完全 | 項目 7 (Deferred Schema 80% 節約、`tools/mod.rs::format_schemas_compact`); 項目 70 (タスク種別ツール制限); 項目 101 (上限ガード); 項目 137 (split policy); 項目 132-135 (MCP 動的) | 動的 schema は実装済、entropy-guided branching (arxiv 2604.12126) は未着手。 | (低優先 — 8 ツール上限の bonsai では効果限定) | ★ |
| **P5** | Memory hierarchies | ✅ 完全 | 項目 76 (Vault rules); 項目 77 (Experience); 項目 161 (Skill tool_chain); 項目 71 (KnowledgeGraph); 項目 179 (MemoryBlock); 項目 217 (decay); 項目 218 (ReviewState V12); 項目 219 (Working Memory Cap 7±2) | Cerememory 三本柱完成 (項目 217-219)。Phase D-G roadmap (`cerememory-extension-roadmap-d-g.md`) で段階拡張中。論文比 over-engineered な部分あり。 | (既存 roadmap で十分、本 plan からの新派生なし) | — |
| **P6** | Test-driven verification | ❌ 未実装 | (該当なし — CLAUDE.md 巻頭の TDD は人間向け、agent 自身は test-first で動かない) | **agent harness レベルでの TDD enforcement が完全に欠落**。論文では「test を先に書かせ、実装を test PASS で検証」が安定 pattern として紹介。bonsai では計画強制 (項目 10) はあるが test-first 強制はない。 | **G2: Agent-Side TDD Enforcement plan** (~2 day) | ★★ |
| **P7** | Sub-agent delegation | ✅ 完全 | 項目 120/160 (`agent/subagent.rs` SubAgentExecutor); 深度制限 2; エラー境界; 順次委任 | 順次委任は実装済、**並列委任** (W&D arxiv 2602.07359、MASS-RAG 等の役割分業) は未着手。項目 55 (読取ツール並列) で部分カバー。 | **G3: 並列 Sub-Agent 役割分業 plan** (~1.5 day) | ★ |
| **P8** | Context management | ✅ 完全 | 項目 6 (4 段 compaction); 項目 12 / 41 / 46 / 78 / 81 / 82 / 158 / 159 / 178 / 187 (ContextOverflowGuard) | F2 ContextOverflowGuard で n_ctx burst 防御、AI+Tool ペア保護も項目 6 で実装。論文比で十分。 | (新派生なし) | — |
| **P9** | Safety guards | ✅ 完全 | 項目 42-44 (`safety/secrets.rs`); 項目 47 (autonomy); 項目 50 (network); `safety/sandbox.rs` (`tools/sandbox.rs`); permission system (`tools/permission.rs`) | 論文比で十分。1bit ローカル実行なので外部 API 流出リスクは web_fetch のみ。 | (新派生なし) | — |
| **P10** | Adaptive prompting | 🟡 部分 | 項目 8 (SOUL.md 3 段検索); 項目 80 (contextual memory injection、`agent/context_inject.rs`); 項目 179 (MemoryBlock); 項目 213 (heuristics injection) | 静的 SOUL.md + heuristics 注入はあるが、**task complexity に応じた system prompt 動的生成** (Cursor の `.cursorrules` 階層、Aider の repo-specific config) は未実装。`classify_task_complexity` (項目 210 由来) は存在するが prompt 生成側に未連携。 | **G4: Task-Complexity-Aware System Prompt 動的生成 plan** (~1 day) | ★★ |

### 3.3 真の gap (= 派生 plan 候補) 集約
本 plan 起票の主要 deliverable:

- **G1: Critic 別 LLM 分離** (P3 深掘り) — ★★★ 高優先
- **G2: Agent-Side TDD Enforcement** (P6 新規) — ★★ 中優先
- **G3: 並列 Sub-Agent 役割分業** (P7 拡張) — ★ 低-中優先
- **G4: Task-Complexity-Aware System Prompt** (P10 深掘り) — ★★ 中優先

3 件以上 (要件 ≥ 3 達成、4 件抽出)。

## 4. gap 派生 plan 候補 (詳細)

### G1: Critic 別 LLM 分離 plan ★★★
- **背景**: 現状 Reflexion は同一 LLM 内 self-correction (`agent/agent_loop/core.rs`) で完結、`agent/judge.rs::HttpAdvisorJudge` は AdvisorConfig 経由で別 LLM 呼出可能 = **infrastructure は既にある**。論文知見の Critic はこの judge を **step 中** に呼ぶ pattern に近いが、現状 judge は Lab 評価専用。
- **動機**: 1bit Bonsai-8B は self-correction に弱い可能性 (項目 184 で MLX 環境劣化 REJECT 等の symptoms)。別 LLM (claude-code or 強力な remote) で critic を担うことで Reflexion 単独では捕捉できない誤りを catch。
- **設計提案**:
  - `AdvisorConfig` を `agent_loop/advisor_inject.rs` の verification step で活用 (既存 P210 dynamic skip と協調)
  - Critic mode: `before_tool_call` / `after_step_outcome` の 2 hook 追加
  - Lab v18 paired t-test で `--enable-critic on/off` 比較
  - production default OFF、env opt-in (項目 214 toggle pattern 踏襲)
- **既存項目との関係**: 項目 1 (Reflexion) / 17-24 (Advisor) / 89 / 210 (Self-Verify dynamic skip) / 163 (Judge Gate)
- **推定工数**: ~1.5 day (TDD strict 5 phase、Phase 4 smoke + Phase 5 Lab v18 別 plan)
- **falsifiable hypothesis**: Critic ON で paired Δscore ≥ +0.015 かつ p < 0.1 → ACCEPT、未達 → dead-code 候補化 (項目 215 ERL pattern 踏襲)
- **リスク**: critic LLM call cost で duration +20-30% 増、advisor max_uses=3 制約と整合化が必要

### G2: Agent-Side TDD Enforcement plan ★★
- **背景**: P6 完全 gap。CLAUDE.md 巻頭で人間向け TDD は明文化されているが、agent 自身は test を先に書かない (計画強制 = 項目 10 はあるが test-first ではない)。論文では「failing test → 実装 → green → refactor」を agent に強制すると安定性が大幅向上、と報告されている。
- **動機**: Bonsai-8B の検証ステップ多用問題 (項目 17-24 で max_uses=3 制限導入済) を test-first で **構造的に解決** する補助線。test を agent 自身が書けば、verification 自動化 (test 実行 = pass/fail) が決定論的になり、Self-Verification Dilemma (項目 210) も緩和される可能性。
- **設計提案**:
  - 新規 task category `TestDriven` を `benchmark.rs::TaskCategory` に追加
  - system prompt extension: `<test_first_directive>` ブロック (env `BONSAI_TDD_DIRECTIVE_ENABLED=1` opt-in)
  - Phase 4 smoke で smoke_tdd_red_green task 追加 (失敗 test → file_write 実装 → shell test 再実行で green の trajectory を期待)
  - HSL relabel (項目 201-205) との相性: test green を「達成済 subgoal」として hindsight 抽出可能
- **既存項目との関係**: 項目 10 (計画強制) / 17-24 (verify 過剰) / 201-205 (AgentHER) / 209 (EventRepository)
- **推定工数**: ~2 day (TDD strict 5 phase、smoke task 設計に追加 0.5 day)
- **falsifiable hypothesis**: TDD directive ON で T5 ErrorRecovery / T6 LongHorizon の paired Δ ≥ +0.02 (AgentFloor plan の tier 別計測と組合せ)
- **リスク**: 1bit モデルが test-first 指示を無視するリスク高 (Lab v15 で #47 思考強制再生成、自然言語 directive の効果限界)、effective でなければ dead-code 化

### G3: 並列 Sub-Agent 役割分業 plan ★
- **背景**: 項目 120/160 で `SubAgentExecutor` 順次委任は実装済、深度制限 2、エラー境界あり。論文知見の MASS-RAG (arxiv 2604.18509) や W&D (arxiv 2602.07359) では役割分業 (evidence summarization / extraction / reasoning 等) を **並列** 実行することで efficient deep research を実現。
- **動機**: 項目 55 (読取ツール 2+ 連続自動並列) は tool レベル並列化のみで、agent レベル並列化ではない。並列 sub-agent で「複数視点を同時に得る」ことが Lab tier 別評価 (AgentFloor T4-T5) で効くか検証。
- **設計提案**:
  - `SubAgentExecutor::run_parallel(tasks: Vec<SubTask>) -> Vec<SubResult>` 新規 API
  - tokio::join! ベース、深度制限 2 を継承、共有 MemoryStore は read-only mode
  - production default OFF、env opt-in
  - Phase 4 smoke で agentfloor T4 multi_file_summary task で並列化 ON/OFF 比較
- **既存項目との関係**: 項目 55 (tool 並列) / 120 (SubAgent 順次) / 160 (深度制限) / AgentFloor T4
- **推定工数**: ~1.5 day (TDD strict、tokio runtime 統合に追加 0.3 day)
- **falsifiable hypothesis**: 並列化で T4 paired Δscore ≥ +0.015 かつ duration -20% 以下 (cost neutral 以上)
- **リスク**: M2 16GB で 2 並列 LLM call は OOM 危険、1bit モデル context 短さで子 agent context budget 競合

### G4: Task-Complexity-Aware System Prompt 動的生成 plan ★★
- **背景**: 項目 8 SOUL.md は静的、項目 80 contextual memory injection は task_context 検索ベースで「task complexity 軸での切替」がない。`classify_task_complexity` (項目 210 由来) で T1-T6 tier 推定はあるが、推定結果を system prompt 生成側に未連携。
- **動機**: AgentFloor plan で tier 別計測が入る (Lab v18+ 候補) → tier ごとに最適な system prompt が異なるはず (T1 短い指示は冗長 prompt が逆効果 / T6 long-horizon は計画 directive 強化が必要)。論文知見の Cursor `.cursorrules` 階層、Aider repo-specific config はこの動的化の延長。
- **設計提案**:
  - `agent/context_inject.rs::inject_adaptive_directive(session, task_complexity)` 新規
  - 6 tier × 3 directive variant = 18 prompt template (`prompts/directive_*.txt`)
  - production default = static (current behavior)、env opt-in `BONSAI_ADAPTIVE_DIRECTIVE_ENABLED=1`
  - AgentFloor 6 tier capability_tier と連携 (依存: AgentFloor plan merge 済前提)
- **既存項目との関係**: 項目 8 / 80 / 179 / 210 / AgentFloor plan
- **推定工数**: ~1 day (directive template 作成に 0.4 day、TDD strict 5 phase に 0.6 day)
- **依存**: AgentFloor 6-tier merge 必須 (capability_tier classifier 流用)
- **falsifiable hypothesis**: Adaptive directive ON で 6 tier 別 paired Δ の中で 1 tier 以上で +0.02 以上、他 tier で -0.01 以下 (副作用なし)
- **リスク**: tier classifier の誤判定で間違った directive 注入、prompt template 18 種の保守負担

### 4.5 派生 plan 候補一覧 (要約表)

| ID | plan 名 | 優先度 | 推定工数 | 依存 | falsifiable hypothesis (要旨) |
|---|---|---|---|---|---|
| G1 | Critic 別 LLM 分離 | ★★★ | 1.5 day | judge.rs / AdvisorConfig | paired Δscore ≥ +0.015, p < 0.1 |
| G2 | Agent-Side TDD Enforcement | ★★ | 2 day | benchmark.rs / 項目 201-205 | T5/T6 で paired Δ ≥ +0.02 |
| G3 | 並列 Sub-Agent 役割分業 | ★ | 1.5 day | subagent.rs / tokio | T4 paired Δ ≥ +0.015 かつ duration -20% |
| G4 | Task-Complexity-Aware System Prompt | ★★ | 1 day | AgentFloor plan merge | 1 tier ≥ +0.02、他 tier ≥ -0.01 |

## 5. 既存項目との関係

本 plan は CLAUDE.md 項目 198+ (項目番号は本 plan merge 時点で次項を取る、現時点 222 まで埋まっている) に新規記録される。各派生 plan が承認・実装された場合、別途項目を取得。

| 既存項目 | 関係 | 本 plan での扱い |
|---|---|---|
| 1 (Reflexion) | P3 既存実装 | gap matrix で「同一 LLM 内 self-correction」と明記、G1 で Critic 別 LLM 分離 |
| 6 (4 段 compaction) | P8 既存実装 | gap なしと判定 |
| 7 (Deferred Schema) | P4 既存実装 | gap なしと判定 |
| 8 (SOUL.md 3 段) | P10 部分実装 | G4 で動的化方向を提示 |
| 10 (計画強制) | P2 既存実装 | gap matrix で完全実装と評価 |
| 17-24 (Advisor verify) | P3 既存実装 | G1 と協調 (max_uses=3 制約継承) |
| 30 (RepoMap v2) | P1 既存実装 | gap matrix で完全実装と評価 |
| 76/77/161 (Memory) | P5 既存実装 | Cerememory 三本柱で十分と評価 |
| 120/160 (SubAgent) | P7 既存実装 | G3 で並列化方向を提示 |
| 187 (ContextOverflowGuard) | P8 既存実装 | gap なしと判定 |
| 199 (A-RAG alignment) | P5 docs PR 先行例 | 本 plan の docs PR scope の手本 |
| 207-215 (Lab v15-v17 REJECT) | 動機 | 構造変異枯渇への補助線として位置付け |
| 210 (Self-Verify dynamic skip) | P3 既存実装 | G1/G2 と協調 |
| 217-219 (Cerememory 三本柱) | P5 既存実装 | gap なしと判定 |
| 220-222 (sqlite-vec 経路) | P1 関連 | G-4.2 REJECT 後は wiring 削除済、再着手 plan は別根拠が出てから |

## 6. 派生 plan 起票手順 (本 plan = meta-plan)

本 plan 自体は実装しない。各 gap (G1-G4) ごとに **個別の plan ファイル** を別 session で起票する。順序と手順:

### 6.1 起票順序の推奨
1. **G1 (Critic 別 LLM 分離)** ★★★ — 最高優先、infrastructure (judge.rs / AdvisorConfig) 既存で着手障壁低い
2. **G4 (Adaptive Directive)** ★★ — AgentFloor merge 後 (依存)、tier 別計測との直結性高い
3. **G2 (Agent-Side TDD)** ★★ — 1bit モデル directive 効果検証、HSL relabel との相性検証
4. **G3 (並列 Sub-Agent)** ★ — M2 16GB OOM リスク、後回し

### 6.2 各派生 plan の必須セクション (本 plan のひな形踏襲)
- Task Type / 由来 / 関連項目 (本 plan reference 必須)
- 1. 背景 (論文 + bonsai 既存実装)
- 2. 目的 / 非目標
- 3. 設計 (struct / API / 注入経路)
- 4. TDD strict 5 phase
- 5. API 影響 (新規 public 一覧)
- 6. risks / mitigations
- 7. quality gates G-1 〜 G-5
- 8. 見積もり
- 9. Quick Start
- 10. 参考 (本 plan、論文、既存項目)

### 6.3 派生 plan 起票 = 別 session 必須
各派生 plan は本 plan の knowledge を継承して別 session で起票 (本 plan を「参考」セクションで参照)。本 plan merge 時点で派生 plan ファイルは作成しない (= meta-plan の deliverable は gap matrix と候補一覧で完結)。

### 6.4 派生 plan 採否基準
各派生 plan の採否は本 plan ではなく、各 plan の Lab paired t-test (G-6 effectiveness gate) で判定する。本 plan は「候補抽出」のみ、優劣判定は実機データで行う。

## 7. Risks / Mitigations

| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| **R1** | Survey 論文の解釈幅大、抽出した 10 pattern が著者意図と乖離 | gap 判定誤り、派生 plan 方向性ズレ | (i) 論文 abstract + 既知 coding agent (Cursor / Devin / Aider / Claude Code) の docs/blog で 2 ソース突合 (ii) 各 pattern の bonsai 関連層 ≥ 1 を必須記述 |
| **R2** | bonsai 文脈不一致 (1bit ローカル実行 vs 論文の cloud-scale agent) | 派生 plan が小規模モデルで効果なし | falsifiable hypothesis を各派生 plan で必須化 (項目 215 ERL Lab v17 REJECT pattern 踏襲)、未達は dead-code 化 |
| **R3** | gap matrix の判定主観性 (✅/🟡/❌ の境界) | 真 gap の見落とし or 過剰生成 | (i) 既存 CLAUDE.md 項目 reference を必須化、項目なし = ❌ 判定 (ii) 🟡 部分実装は「論文比で何が足りないか」を 1 行で明文化 |
| **R4** | 既存 plan (AgentFloor / ERL / Cerememory) との重複 | 派生 plan が先行 plan と衝突 | gap matrix で各 pattern に既存 plan reference を明記、新派生は既存未カバー領域に限定 (G1-G4 すべて新規領域) |
| **R5** | meta-plan の慣習 bonsai 不在で merge 後の運用イメージ希薄 | docs として死蔵 | CLAUDE.md 項目で「派生 plan 起票元 = 本 plan」と明記、派生 plan が起票されたら本 plan の派生候補表にリンク追加 (将来 update) |

## 8. Quality Gates

| Gate | 内容 | 検証 | 必須 |
|---|---|---|---|
| **G-1 gap matrix 完成** | 10 pattern × bonsai 実装の対応表が完成、各 row に既存項目 reference ≥ 1 (❌ 未実装は除く) | 表 3.2 の row 数 = 10 / reference 列空欄なし (P6 ❌ を除く) | 必須 |
| **G-2 派生 plan 候補 ≥ 3** | gap (🟡 / ❌) ごとに派生 plan 候補抽出、合計 ≥ 3 件、各候補に優先度 + 推定工数 + falsifiable hypothesis | 表 4.5 row 数 ≥ 3 (本 plan は G1-G4 = 4 件) | 必須 |
| **G-3 docs 整合性** | 既存 CLAUDE.md 項目番号の引用が正確 (項目 30 = RepoMap、項目 7 = Deferred Schema 等)、誤参照ゼロ | grep で項目番号引用を全件突合、過去 plan `arag-hierarchical-retrieval-docs.md` の G-3 修正手順踏襲 | 必須 |
| **G-4 production code 変更ゼロ** | `git status` で src/ 配下に変更なし、plan ファイル 1 個のみ追加 | `git diff --stat src/` 空、`git status` で `.claude/plan/building-ai-coding-agents-gap-analysis.md` のみ untracked | 必須 |
| **G-5 派生 plan 重複なし** | 既存 plan (`agentfloor-tier-eval-impl.md` / `erl-heuristics-pool-impl-v2.md` / `arag-hierarchical-retrieval-docs.md` / cerememory 系) と G1-G4 が重複しないこと確認 | grep で plan dir 内の plan 名と概念対応を全件突合 | 必須 |

G-1〜G-5 PASS で merge 可能。各派生 plan の Lab effectiveness gate (G-6) は別 plan の責務。

## 9. 完了条件

1. ✅ gap matrix (表 3.2) が 10 pattern 全 row 完成、判定 (✅/🟡/❌) 明示
2. ✅ 派生 plan 候補 ≥ 3 件抽出 (本 plan は G1-G4 = 4 件)
3. ✅ 各派生 plan 候補に優先度 (★/★★/★★★) + 推定工数 + falsifiable hypothesis 付記
4. ✅ 既存項目 (CLAUDE.md 項目 1-222) との関係を § 5 に明記
5. ✅ G-1 〜 G-5 quality gate 全 PASS
6. ✅ production code 変更ゼロ、plan ファイル 1 個のみ追加
7. ✅ 既存 plan (AgentFloor / ERL / Cerememory) との非重複確認

## 10. 見積もり

本 plan **自体** の実装 (= plan ファイル 1 個の起票):

| Phase | 内容 | 時間 |
|---|---|---|
| **P1 (調査)** | 論文 (arxiv 2603.05344) abstract + 関連 survey の cross-check | 0.5h |
| **P2 (gap matrix)** | bonsai 既存実装の確認 (`grep` で 10 pattern × CLAUDE.md 項目突合) + 表 3.2 起票 | 1.5h |
| **P3 (派生候補抽出)** | G1-G4 詳細 (背景 / 設計提案 / falsifiable hypothesis / 推定工数) | 1.5h |
| **P4 (整合性チェック)** | G-1 〜 G-5 quality gate self-review、CLAUDE.md 項目 reference 全件突合 | 0.5h |
| **P5 (commit + handoff)** | 1 commit (`docs(plan): building-ai-coding-agents-gap-analysis 起票`) | 0.3h |
| **計** | | **~4.3h ≈ 0.5 day** |

派生 plan (G1-G4) の実装は **本 plan の見積もりに含まない** (各 plan で別途見積もり、表 4.5 参照)。

派生 plan 全実装合計 (参考):
- G1 (1.5 day) + G2 (2 day) + G3 (1.5 day) + G4 (1 day) = **6 day** (各々別 session、Lab effectiveness 検証は別 session × 4 で +4 × 0.5 = 2 day = 計 8 day)

## 11. Quick Start

本 plan 起票後の派生 plan 起票への進み方:

```bash
# 0. 本 plan を最初に確認
$EDITOR .claude/plan/building-ai-coding-agents-gap-analysis.md

# 1. 推奨順序: G1 から起票 (★★★ 最優先)
# 別 session で:
#   prompt 例: "Critic 別 LLM 分離 plan を起票してほしい。
#               由来: building-ai-coding-agents-gap-analysis.md G1
#               既存 infra: src/agent/judge.rs HttpAdvisorJudge / AdvisorConfig
#               TDD strict 5 phase、production default OFF、env BONSAI_CRITIC_ENABLED 形式"
$EDITOR .claude/plan/critic-separate-llm-impl.md

# 2. AgentFloor plan merge 確認後、G4 起票
grep -n "agentfloor-tier-eval-impl" .claude/plan/INDEX.md
$EDITOR .claude/plan/adaptive-directive-impl.md

# 3. G2 (Agent-Side TDD) — HSL relabel との相性検証準備
$EDITOR .claude/plan/agent-side-tdd-enforcement-impl.md

# 4. G3 (並列 Sub-Agent) — 最後 (M2 16GB OOM リスク評価必要)
$EDITOR .claude/plan/parallel-subagent-roles-impl.md

# 5. INDEX.md 更新
$EDITOR .claude/plan/INDEX.md   # 「🆕 Building AI Coding Agents 派生」セクション追加
```

派生 plan 起票時の必須 reference:
- 本 plan (`.claude/plan/building-ai-coding-agents-gap-analysis.md` § 4 該当 G)
- arxiv 2603.05344 (Building AI Coding Agents)
- bonsai CLAUDE.md 該当既存項目 (本 plan § 5 表)

## 12. 参考

### 論文・survey
- **arxiv 2603.05344** — Building AI Coding Agents for the Terminal: Scaffolding, Harness, Context Engineering (2026-03、本 plan 主軸)
- arxiv 2604.12126 — Long-Horizon Plan Execution in Large Tool Spaces through Entropy-Guided Branching (P4 関連)
- arxiv 2602.07359 — W&D: Scaling Parallel Tool Calling for Efficient Deep Research (P7 関連)
- arxiv 2604.18509 — MASS-RAG: Multi-Agent Synthesis (P7 関連)
- arxiv 2603.22862 — Evolution of Tool Use in LLM Agents Survey (P4 関連)

### bonsai 既存 plan (本 plan 参照)
- `.claude/plan/agentfloor-tier-eval-impl.md` (構造手本、tier 別評価設計、G4 依存)
- `.claude/plan/erl-heuristics-pool-impl-v2.md` (構造変異 plan の手本、TDD strict 5 phase + Codex audit pattern)
- `.claude/plan/arag-hierarchical-retrieval-docs.md` (docs PR scope の手本)
- `.claude/plan/cerememory-extension-roadmap-d-g.md` (Phase D-G roadmap、P5 補強)
- `.claude/plan/event-repository-trait-impl.md` (項目 209 dividend、G1/G2 で活用候補)
- `.claude/plan/INDEX.md` (本 plan を「🆕 Building AI Coding Agents 派生」として追記候補)

### bonsai 既存項目 (本 plan で reference する CLAUDE.md 項目)
- 項目 1 (Reflexion)
- 項目 6 (4 段 compaction)
- 項目 7 (Deferred Schema)
- 項目 8 (SOUL.md 3 段検索)
- 項目 10 (計画強制ルール、Lab v6.2 唯一 ACCEPT)
- 項目 17-24 (Advisor verify、max_uses=3)
- 項目 30 (RepoMap v2)
- 項目 42-44 (safety/secrets, sandbox, network)
- 項目 70 (タスク種別ツール制限)
- 項目 76/77/161 (Memory 三層)
- 項目 80 (contextual memory injection)
- 項目 89 (Advisor 拡張)
- 項目 101 (上限ガード)
- 項目 120 / 160 (SubAgentExecutor)
- 項目 132-135 (MCP 動的)
- 項目 137 (split policy)
- 項目 161 (Skill 軌跡昇格)
- 項目 163 (Judge Gate)
- 項目 179 (MemoryBlock)
- 項目 187 (ContextOverflowGuard)
- 項目 199 (A-RAG alignment、docs PR 先行例)
- 項目 201-205 (AgentHER hindsight relabel)
- 項目 207-215 (Lab v15-v17 paired t-test、構造変異枯渇 evidence)
- 項目 209 (EventRepository trait)
- 項目 210 (Self-Verify dynamic skip)
- 項目 213 (ERL Heuristics Pool)
- 項目 217-219 (Cerememory 三本柱: decay / ReviewState V12 / Working Memory Cap)
- 項目 220-222 (sqlite-vec G-4.2 REJECT 経路)

### bonsai source files (本 plan で grep / 参照)
- `src/agent/agent_loop/core.rs` (Reflexion 注入経路)
- `src/agent/agent_loop/advisor_inject.rs` (advisor verify、Critic 拡張候補)
- `src/agent/judge.rs` (HttpAdvisorJudge、G1 base infrastructure)
- `src/agent/error_recovery.rs` (Continue Sites / LoopDetector / FailureMode)
- `src/agent/middleware.rs` (5 段 middleware: Audit / ToolTrack / Stall / Compact / TokenBudget)
- `src/agent/subagent.rs` (SubAgentExecutor、G3 base)
- `src/agent/context_inject.rs` (inject_memory_blocks / inject_heuristics / inject_contextual_memories、G4 base)
- `src/tools/mod.rs::format_schemas_compact` (P4 Deferred Schema)
- `src/tools/repomap.rs` (P1 Repository indexing)
- `src/safety/*.rs` (P9 Safety guards)

### CODEX_SESSION
- (取得不要 — meta-plan 起票のみ、実装なし)
