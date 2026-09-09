# bonsai-agent サブエージェント運用ガイドライン

本ドキュメントは、`bonsai-agent` プロジェクトにおけるサブエージェント体制の設計原則、役割定義、およびワークフローを定めた公式ガイドです。

---

## 1. 2 つのサブエージェント・レイヤー

`bonsai-agent` では、サブエージェントという概念を以下の 2 つのレイヤーで明確に区別し運用します。

```
┌────────────────────────────────────────────────────────────────────────┐
│ 【Track A】 開発支援サブエージェント体制 (Dev / Meta Subagents)         │
│  Antigravity / Claude Code / Codex 等で本リポジトリを改修する AI チーム │
│  ・DEP-001 レイヤー守護 / 完全同期アーキテクチャ維持                   │
│  ・Clippy 巻き戻し絶対禁止 / Lab release ビルド保護                     │
│  ・1,480+ TDD 回帰ラチェット / ADR-003 Paired Evidence 統計検証        │
│  ・docs/VALUES.md (V1〜V7) & Goodhart's Law 指標形骸化監査              │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ 開発・実装
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ 【Track B】 bonsai-agent 内部の実行時サブエージェント (Runtime Scaffolding)│
│  Bonsai-8B (1-bit) の能力限界を外部から補強する推論・実行機構           │
│  ・src/agent/subagent.rs のロール特化（Explorer / Builder / Verifier） │
│  ・タスク独立性ヒューリスティック & 並列/順次自動切替                  │
│  ・コンテキスト予算 (n_ctx_budget) の伝播 & MAGI 3-panel 監視連動      │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 2. 開発支援サブエージェント（Track A）

### ① ロール定義マトリクス

| エージェント名 | 責務と専門領域 | 主な入力と出力 | 検証コマンド |
| :--- | :--- | :--- | :--- |
| **`bonsai-architect`** | Clean Architecture (DEP-001) の厳格維持、port trait 設計、完全同期保証、ADR 整合性管理 | 入力: 課題/要件<br>出力: 設計仕様・port 定義 | `cargo test --test structural` |
| **`bonsai-rust-implementer`** | Rust 2024 実装、**Clippy 巻き戻し絶対禁止**、**Lab release ビルド禁止**、完全同期維持 | 入力: 設計書<br>出力: 最小限のプロダクションコード | `cargo clippy -- -D warnings`<br>`cargo fmt -- --check` |
| **`bonsai-tdd-verifier`** | 1,480+ 件の単体テスト資産保護、RED-GREEN-REFACTOR、回帰ラチェット、インメモリテスト維持 | 入力: コード差分<br>出力: テストコード・合否判定 | `cargo test --lib`<br>`cargo test --test structural` |
| **`bonsai-lab-evaluator`** | **ADR-003 Paired Evidence 規律** の執行、Cohen's dz / Wilcoxon 統計検証、Lab ログ検証 | 入力: 実機ログ・メトリクス<br>出力: 統計判定 (ACCEPT/REJECT) | `scripts/lab_v22_metric.py` |
| **`bonsai-values-auditor`** | `docs/VALUES.md` (V1〜V7) 整合性監査、Goodhart's Law 形骸化検知、MAGI 3-panel 視座 | 入力: 設計・機能方針<br>出力: 価値観適合性監査レポート | VALUES.md チェック |

### ② 関心の分離（Separation of Concerns）と受入れフロー

1. **オーケストレーター（Planner / Synthesizer）**:
   - 課題を分析し、適切なサブエージェントへタスクを委任する。
2. **自己承認の禁止（No Self-Approval）**:
   - 実装担当（`bonsai-rust-implementer`）は決して自分のコードを承認してはならない。
   - 必ず `bonsai-tdd-verifier` または独立したレビュアーの客観的検証を経てマージ・コミットする。
3. **Lab 実験の昇格ゲート（Paired Evidence Gate）**:
   - 新機能や変異を default ON に昇格させる場合、必ず `bonsai-lab-evaluator` が Paired Evidence（A/B 比較、Cohen's dz > 0、Wilcoxon p < 0.05）を確認する。

---

## 3. ランタイム・サブエージェント（Track B）

`src/agent/subagent.rs` は、1-bit Bonsai-8B モデル向けに特化した Scaffolding 装置です。

1. **深度制限**: `MAX_DEPTH = 2`（無限再帰や注意発散の防止）。
2. **タスク独立性判定 (`check_independence`)**:
   - 日本語/英語の依存マーカー（"前の", "上記", "then", "previous" 等）を検出し、独立サブタスクは `std::thread::scope` で並列実行、依存サブタスクは順次実行に自動切り替え。
3. **コンテキスト予算の伝播 (`n_ctx_budget`)**:
   - 親エージェントのトークン予算をサブエージェントへ伝播し、Bonsai-8B のコンテキスト溢れを防止。
4. **ロール特化 Scaffolding**:
   - `Explorer`: ファイル・コードの探索・読取専用。親へは 3 行以内の要約のみ返却。
   - `Builder`: コード変更・差分生成専用。
   - `Verifier`: コンパイル・テスト・リント検証専用。

---

## 4. ドキュメント更新責任（RACI）と SSOT 同期規約

コードや方針の変更に伴うドキュメント同期漏れ・ドリフトを防ぐため、サブエージェントごとに更新責任を明確化します。

### ① サブエージェント別ドキュメント責任
- **`bonsai-architect`**: ADR（`docs/decisions/ADR-*.md`）起票、アーキテクチャ文書（`docs/architecture/*`）、`docs/INDEX.md`、レイヤールール（DEP-001）同期。
- **`bonsai-rust-implementer`**: コード内インライン doc（`///`）、新環境変数導入時の `docs/execution/runbook.md` 下書き（アーキテクチャ文書の単独確定は禁止）。
- **`bonsai-tdd-verifier`**: ドキュメント記載のビルド・テストコマンドが完全に通るかの検証 (`cargo test --test structural`)。
- **`bonsai-lab-evaluator`**: 実機実験ログ、`docs/quality/lab-history.md`、`scores.md`、変異採否の反映。
- **`shipper` / `orchestrator`**: `CHANGELOG.md` (`## Unreleased`) への記録およびコミット前の全ドキュメント同期確認。

### ② ポインタ参照原則（Pointer Over Copy）
`AGENTS.md`、`GEMINI.md`、`CLAUDE.md` はエージェント用ガイドレールであり、正本（SSOT）ではありません。詳細な仕様や全履歴を直書きせず、「数行の要約 ＋ `[docs/...]` へのリンク」に留めることで、情報の乖離とコンテキスト浪費を防ぎます。

### ③ 原子的コミット（Atomic Commit with Code）
コード変更とドキュメント同期を分離せず、必ず同一 PR / 同一コミットで同期を完結させます。
