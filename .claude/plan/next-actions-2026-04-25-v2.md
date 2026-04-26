# Implementation Plan: 次に取れる残作業 v2（Lab v12 並行作業候補、Codex分析統合版）

## Task Type
- [x] Backend (Rust / harness-level)
- [x] Documentation / Configuration
- [ ] Frontend

## 背景（現状: 2026-04-25 18:20）

### 本セッション完了済（v1からの差分）
- **βパス**: 旧プラン削除（fix-linter-revert.md / nat-knowledge-transfer.md / structural-improvements.md）+ commit
- **δパス**:
  - `memory/harness_improvements_v1.md` を 73→162項目・632→909テストに追従更新
  - `src/agent/benchmark.rs` に Phase C 18タスクの `#[ignore]` スケルトン追加（commit `0d01e4f`、TDD Red）
  - cargo test 確認: 18 ignored、CI影響なし

### 現在の状態
- テスト数: **909 → 927**（+18 ignored）
- `agent_loop.rs`: **2661行**（structural-improvements-v2.md記述の2497行から増加、分割計画あり）
- `tool_exec.rs`: 447行
- 未tracked: `.claude/plan/next-actions-2026-04-25.md`（v1メタ計画）と本ファイル（v2）
- Lab v12: ベースライン計測中（release バイナリ、ソース変更影響なし、次回ポーリング 約22分後）

### 残候補
- next-actions v1 由来: Step 3 (SSE境界テスト) / Step 6 (分割検証)
- structural-improvements-v2.md 由来: Step 7 (分割本実装) / Step 8 (依存最適化) / Step 9 (カバレッジ計測)
- Phase C 本実装: Lab v12 完走後（TDD Green）

---

## Technical Solution（Codex 分析統合）

Codex 分析結果（SESSION_ID: `019dc3f1-5ad0-7333-8325-8187ed4f998e`）をベースに、Lab v12 中の安全な作業を優先度別に統合。**「Lab への非干渉 × 低リスク × 高価値」** を3条件として評価。

---

## Implementation Steps（優先度別）

### 🟢 P0: agent_loop.rs 分割検証（推奨パス1、15-20分、リスク 0.5/5）

**Step 1: 分割設計の実コード行数再検証**
- 内容: 2661行の関数境界・LoopState操作箇所・pub依存をマップし、4-5モジュール（mod.rs/core.rs/step.rs/outcome.rs/pipeline.rs）への割当を確定
- 出力: `.claude/plan/agent-loop-split-validated.md`（コード変更なし、読みのみ）
- 期待効果: Lab v12 完走後の Step 7 着手時の手戻り **20-30%削減**
- 工数: 15-20分（次回ポーリングまでに完了可能）
- リスク: ゼロ（read-only分析）

### 🟡 P1: SSE タイムアウト境界テスト追加（推奨パス2、30分、リスク 1.0/5）

**Step 2: `llama_server.rs` の `sse_chunk_timeout_secs` 単体テスト3件追加**
- 現状: `sse_chunk_timeout_secs` は読み込まれて `timeout_recv_body` に流れるが、デフォルト値・MLX 180秒値・境界値の単体テストなし
- 追加内容:
  ```rust
  #[test]
  fn test_sse_timeout_default_is_60() { ... }
  #[test]
  fn test_sse_timeout_mlx_recommended_180() { ... }
  #[test]
  fn test_sse_timeout_zero_disables_or_minclamp() { ... }
  ```
- 期待効果: テスト数 927 → 930、MLX 180秒値の意図文書化、設定退行検出
- 影響: `llama_server.rs` tests 周辺のみ（局所変更）
- リスク: ロジック変更なし、cargo build / Lab v12 への影響ゼロ

### 🟡 P2: 分割実施条件の確定（推奨パス3、60-75分、リスク 0.8/5）

**Step 3: P0 の延長 — 分割後の責務・外部 pub use・commit 単位確定**
- 内容: P0 で確定したマッピングを元に、`pub use` 経路の決定、commit 単位（モジュール毎に分割 or 一括）、clippy 巻戻し対策の commit 直後ルール明文化
- 期待効果: Step 7 本実装の所要時間 **30-40分短縮**
- 工数: 60-75分（次回ポーリング後に着手）

### 🔵 P3: 調査のみ（推奨パス4、30-45分、リスク 1.5/5）

**Step 4: 依存最適化・カバレッジ計測の前提調査**
- 内容:
  - `cargo machete` / `cargo-udeps` で未使用依存リスト化
  - `ureq`/`reqwest` の重複箇所マッピング（grep）
  - `tree-sitter-*` 言語パーサ依存グラフ確認（`cargo tree -p tree-sitter`）
  - `tarpaulin` 実行条件確認（既存設定の有無、CI連携）
- 出力: `.claude/plan/deps-and-coverage-research.md`
- リスク: 調査のみで変更なし
- 直接価値は薄い（Step 1/2 より優先度低）

### 🟣 P4: Lab v12 完走後タスク（待機、本実施保留）

**Step 5: Phase C 本実装 → agent_loop.rs 分割 → 依存最適化**
- Phase C: 18タスクの `#[ignore]` を解除し default_tasks() に追加（2-3時間、リスク 3/5）
- agent_loop.rs 分割: P0/P2 で確定した設計に従い実施（90-150分、リスク 4/5）
- 依存最適化本実施: HTTP クライアント統合・tree-sitter整理（60-120分、リスク 3.5/5）

---

## 推奨パス

| パス | 内容 | 工数 | リスク | 期待効果 |
|---|---|---|---|---|
| **α**: 25分窓最大活用 | Step 1（分割検証） | 15-20分 | 0.5/5 | 次回ポーリングまでに完了、分割手戻り20-30%削減 |
| **β**: 最有力 | Step 1 → Step 2（SSE境界テスト） | 45-55分 | 1.0/5 | テスト+3、MLX設定退行防止、Lab非干渉 |
| **γ**: 設計固定重視 | Step 1 → Step 2 → Step 3（分割条件確定） | 105-130分 | 0.8/5 | Lab後 Step 7 を30-40分短縮 |
| **δ**: 調査拡張 | Step 1 → Step 2 → Step 4（依存/カバレッジ調査） | 75-100分 | 1.5/5 | 後続フェーズの前提整理 |

**Codex推奨: β（最有力）** — 低リスク・高価値・Lab 非干渉の3条件最適。

**今は非推奨**: Step 7（分割本実施、リスク 4/5）／Step 8（依存最適化本実施、リスク 3.5/5）／Step 9（tarpaulin 実測、重い）／Phase C 本実装（Lab v12 完走後）

---

## Key Files

| File | Operation | Step |
|------|-----------|------|
| `.claude/plan/agent-loop-split-validated.md` | Create | 1 |
| `src/runtime/llama_server.rs` | Modify (tests追加) | 2 |
| `.claude/plan/deps-and-coverage-research.md` | Create | 4 |

---

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| Step 2 のテスト追加で `cargo test` 遅延 | テスト3件のみ、ms オーダー |
| Step 1/3 の設計が将来仕様と齟齬 | コード変更なし、Lab後の検証で再評価 |
| Step 4 の調査結果が陳腐化 | `.claude/plan/` の date suffix 付きで管理 |
| Lab v12 完走時刻と作業重複 | β（45-55分）はポーリング1回内で完了、γ/δはポーリング跨ぎ前提 |
| agent_loop.rs 直接編集 | 本プランでは P0/P2 とも read-only、コード変更なし |

---

## SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: `019dc3f1-5ad0-7333-8325-8187ed4f998e`
- GEMINI_SESSION: (failed silently — log file empty, retry可)

---

## 備考

- Gemini analyzer は本セッションで起動失敗（exit code 1、ログ未生成）。Codex 単独分析だが、過去 next-actions v1 と structural-improvements-v2.md の制約を統合してプラン化したため、視点欠落リスクは限定的
- Lab v12 ポーリング: 18:43 予定（25分間隔継続）
- 残候補（次回プラン v3 に持ち越し可）: ダッシュボード機能改善 / README.md テスト数927反映 / Lab v12 完走後分析の雛形作成
