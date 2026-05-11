# `.claude/plan/` インデックス

67 ファイル (2026-05-10 時点) の状態と関係を一覧化。新規セッションで最初に参照する起点。

**Last Updated:** 2026-05-10

---

## 🔥 アクティブ（今後の作業の起点）

| ファイル | 役割 | 状態 |
|---|---|---|
| `post-lab-v13-roadmap.md` | **マスタープラン** — Lab v13 完了後の構造改善 v3 全体計画 | 🔥 起点 |
| `lab-v13-config-draft.md` | Lab v13 起動設定（[experiment] セクション） | 🔄 実行中 |
| `structural-improvements-v2.md` | Step 0-9 全体ロードマップ（Step 7 までは ✅ 完了マーク済） | 📊 状態管理 |

## 📐 構造改善 v3 詳細設計（Lab v13 完了後実装）

| ファイル | 候補 | 工数 | 採否ゲート |
|---|---|---|---|
| `diffstore-rust-impl.md` | A: DiffStore (★★★) | 4h | Lab v13 で diff 平均 150+ tok |
| `edit-cycle-detector-impl.md` | B: Edit Cycle (★★) | 1h | Lab v13 で同ファイル交互編集 REJECT |
| `fallback-chain-impl.md` | C: Fallback Chain (★★) | 3h | Lab v13 で MLX 接続断 2+ 回 |
| `step-8-dependency-eval.md` | D: 依存最適化 | 1h | 軽量実施推奨 |
| `step-9-coverage-design.md` | E: テストカバレッジ | 9h | 950→1000 テスト |

## 📚 知見集約・判定ドキュメント

| ファイル | 内容 | 状態 |
|---|---|---|
| `macos26-agent-learnings-v2.md` | macOS26/Agent 8 ファイル分析、3 候補抽出 | 📚 参照 |
| `phase-d-evaluation.md` | ADK Phase D YAGNI 判定 = 見送り | 📚 参照 |
| `phase-b2-judge-gate.md` | ADK Phase B2 設計（実装済） | 📚 履歴 |
| `agent-loop-split-validated.md` | agent_loop 分割設計検証（実装済） | 📚 履歴 |
| `lab-v11-accept-analysis.md` | Lab v11 ACCEPT 詳細（defaults 化見送り） | 📚 履歴 |
| `lab-v12-accept-analysis.md` | Lab v12 ACCEPT 詳細（temperature 0.7 推奨） | 📚 履歴 |

## 🗄 古い計画（参照のみ、新計画でカバー済）

| ファイル | 状態 | 後継 |
|---|---|---|
| `adk-integration.md` | ADK 取込ロードマップ（A-D） | DESIGN_SPEC.md + post-lab-v13-roadmap.md |
| `continuation-2026-04-25.md` | Lab v11 後継作業 | 項目160-163 で対応済 |
| `next-actions-2026-04-25.md` | Lab v12 並行作業候補 v1 | v2 に置換 |
| `next-actions-2026-04-25-v2.md` | Lab v12 並行作業候補 v2 | post-lab-v13-roadmap に統合 |
| `phase-c-and-refactor-draft.md` | Phase C + 分割草稿 | 項目163-164 で完了 |

## 関係マップ

```
post-lab-v13-roadmap.md (★ 起点)
├── Phase 1: lab-v13-config-draft.md → 結果分析
├── Phase 2: 構造改善 v3
│   ├── diffstore-rust-impl.md (★★★)
│   ├── edit-cycle-detector-impl.md (★★)
│   └── fallback-chain-impl.md (★★)
├── Phase 3: 品質強化
│   ├── step-8-dependency-eval.md
│   └── step-9-coverage-design.md
└── Phase 5: 知見継続
    ├── macos26-agent-learnings-v2.md
    └── phase-d-evaluation.md (再評価ゲート)

structural-improvements-v2.md ← 全体俯瞰（Step 0-9 状態管理）
```

## 🆕 外部 OSS 取込み (構造変異 from external repo)

| ファイル | 由来 | 工数 | 採否ゲート |
|---|---|---|---|
| `cerememory-decay-port-impl.md` | `co-r-e/cerememory` ADR-005 (commit b08d201、MIT) | 0.5 day | Lab v18 paired t-test (decay ON/OFF) で Δscore ≥ +0.015 |
| `cerememory-review-state-v12-impl.md` | `co-r-e/cerememory` ADR-011 (Strength/Freshness 分離) | 1.5 day | Lab v19 paired t-test (freshness gate ON/OFF) で Δscore ≥ +0.015 |
| `cerememory-extension-roadmap-d-g.md` | Cerememory 5-store + 周辺機構の bonsai 取込み master roadmap (Phase D Emotional / E MCP / F Audit hashchain / G Working memory cap) | planning-only (1.5h、各 Phase 個別 plan は採否ゲート後展開) | Lab v17/v18/v19 結果に応じ Phase D-G 優先順動的決定 |
| `ds4-insights-port-impl.md` | `antirez/ds4` (DeepSeek V4 Flash inference engine、5,036 stars、MIT) | Stage 1: 1 day / 全 Stage: ~3 day | Stage 1 paired smoke で duration −10% AND score ±0.02 |
| `ds4-rax-skill-index-impl.md` | ds4 同梱 `rax.c` (Redis Adaptive Radix tree、antirez 単独著作、Redis 由来 2017-2018) | 1.5 day | Lab paired smoke で latency −50% 以上 + score ±0.02 (REJECT 時は項目 222 と同 dead-code 削除経路) |
| `gbrain-insights-port-impl.md` | Zenn 記事 「gbrain Knowledge Graph 設計」 (Y Combinator CEO 開発、TypeScript + PostgreSQL/PGLite + MCP) | Stage 1: 0.5 day / 全 Stage: ~3 day | Stage 1 backlink boost paired smoke で score variance 範囲内 + Lab v22 で paired 5 cycle ACCEPT |

6 plan は production default OFF (env opt-in、項目 214 toggle pattern と一貫)。ds4 plan は Stage 1 (KV cache wiring) のみ本 plan で完結、Stage 2 (rax skill index、`ds4-rax-skill-index-impl.md` 起票済) は独立着手可、Stage 3 (tool_id replay map) は Stage 1 ACCEPT 後に別 plan 起票。gbrain plan は Stage 1 (Backlink Boost) のみ本 plan で完結、Stage 2 (Edge Provenance) は派生 plan で別 session 起票、Stage 3 (記憶層分離 validation) は Cerememory 三本柱で完遂済 = port 不要。Lab v17 完了後着手必須 (cerememory 3 plan)、ds4 Stage 1/2 + gbrain Stage 1/2 は独立着手可。

## 🔬 arxiv 2026-05 由来 plan (research_arxiv_2026_05_07.md ★★★ 高優先 10 件)

| ファイル | 由来論文 | 種別 | 工数 |
|---|---|---|---|
| `beyond-pass1-rdc-vaf-impl.md` | arxiv 2603.29231 Beyond pass@1 | 実装済 (項目 200) | — |
| `agenther-runtime-integration-impl.md` | arxiv 2603.21357 AgentHER (HSL relabel) | 実装済 (項目 201-205) | — |
| `arag-hierarchical-retrieval-docs.md` | arxiv 2602.03442 A-RAG | docs (項目 199) | — |
| `erl-heuristics-pool-impl-v2.md` | arxiv ERL Heuristics | 実装済 (項目 213-216、Lab v17 REJECT) | — |
| `self-verify-dilemma-impl.md` | arxiv Self-Verification Dilemma | 実装済 (項目 210-212、Lab v16 REJECT) | — |
| `agentfloor-tier-eval-impl.md` | arxiv 2605.00334 AgentFloor 6-tier | ✓ 完遂 (項目 223、5 commits 2b63441→6be9b67、1162 passed)、副次=run_k tier populate fix `572a9a4` | — |
| `agentfloor-prescreen-tier-fix.md` | G-4c v3 PARTIAL PASS で発覚 (項目 223 wiring 最終 fix) | Phase 1-3 完遂 (commit a52edc6+fd30398、1162→1165 passed)、Phase 4 G-4c v4 background 検証中 | ~6h |
| `experiment-from-results-deletion-impl.md` | session 05-11b §11 wiring gap 全網羅検査の副次発見 (legacy ctor + BenchmarkResult dead-code chain) | 起票済 (237 行)、未実装、★ 機能影響なし cleanup、項目 222 sqlite-vec wiring 削除と同 pattern | ~3.5h ≈ 0.5 day |
| `pass-k-t-metric-impl.md` | arxiv 2604.14877 PASS@(k,T) | 起票済 (907 行)、未実装 | ~5h |
| `vllm-mlx-backend-impl.md` | arxiv 2601.19139 vllm-mlx | 起票済 (734 行)、未実装 | ~17h |
| `mcp-bench-integration-impl.md` | arxiv MCP-Bench | 起票済 (540 行)、未実装 | ~12h |
| `building-ai-coding-agents-gap-analysis.md` | arxiv survey 系 | meta-plan、4 派生候補 (G1-G4) 起票済、未着手 | ~0.5 day + 6 day |

派生 plan 候補 (`building-ai-coding-agents-gap-analysis.md` 由来):

| ファイル | 派生 ID | 優先度 | 工数 | 状態 |
|---|---|---|---|---|
| `critic-separate-llm-impl.md` | G1 Critic 別 LLM | ★★★ | ~1.25-1.5 day | 起票済 (640 行)、未実装 |
| `task-aware-system-prompt-impl.md` | G4 Task-Aware Prompt | ★★ | ~0.7-1 day | 起票済 (519 行)、未実装 |
| `agent-side-tdd-enforcement-impl.md` | G2 Agent-Side TDD | ★★ | ~2 day | 起票済 (770 行)、G1 dependency + 独立着手可、未実装 |
| `parallel-subagent-roles-impl.md` | G3 並列 Sub-Agent | ★ | ~1.5 day | 起票済 (819 行)、std::thread::scope 経路踏襲 (tokio 移行は Phase 2 派生)、未実装 |

推奨着手順序 = **AgentFloor pre-screen tier fix (本 session 着手中、~6h)** → G1 Critic → G4 Task-Aware → PASS@(k,T) → vllm-mlx → MCP-Bench → G2 → G3。派生 plan 4 件全て起票済 (合計 2748 行)、production code 変更ゼロ。AgentFloor 本体 (`agentfloor-tier-eval-impl.md`) は項目 223 で完遂、最終 wiring fix のみ残 (`agentfloor-prescreen-tier-fix.md` 起票済)。

## 📊 Lab effectiveness paired t-test plan (G1-G4 実装 ACCEPT 後の Phase 5 検証)

| ファイル | 対応 | ACCEPT 基準 | 工数 | 状態 |
|---|---|---|---|---|
| `lab-v17-erl-effectiveness.md` | ERL Heuristics (項目 213) | Δ≥+0.015 + p<0.1 | 15h 37min | ✅ 完走 / **REJECT** (項目 215、Δ=−0.0014, p=0.5072) |
| `lab-v18-critic-effectiveness.md` | G1 Critic 別 LLM (`critic-separate-llm-impl.md`) | Δ≥+0.015 + p<0.1 (Lab v17 同形) | ~18-22h | 起票済 (539 行)、G1 実装 ACCEPT 後 |
| (未起票) | G2 Agent-Side TDD (`agent-side-tdd-enforcement-impl.md`) | 同左 | ~18-22h | Lab v19、G2 実装 ACCEPT 後 |
| (未起票) | G3 並列 Sub-Agent (`parallel-subagent-roles-impl.md`) | 同左 | ~18-22h | Lab v20、G3 実装 ACCEPT 後 |
| (未起票) | G4 Task-Aware Prompt (`task-aware-system-prompt-impl.md`) | 同左 | ~12-18h | Lab v21、G4 実装 ACCEPT 後 |

REJECT 時は項目 222 (sqlite-vec wiring 削除) と同 pattern で dead-code 削除経路 (各 plan §6.2 / §14 で明記)。Lab 天井 7 連続 (項目 215) 打開仮説の falsifiable hypothesis 検証経路。

## メンテナンス方針

- 新規 plan 作成時、本 INDEX に行を追加
- 完了/置換時、状態を 🔥/🔄/📚/🗄 で更新
- 90 日以上未参照の 🗄 ファイルは memory に集約検討

## 関連

- 上位: `docs/DESIGN_SPEC.md`（章 7 で本ディレクトリ参照）
- メモリ: `memory/MEMORY.md`（個別知見ファイル索引）
