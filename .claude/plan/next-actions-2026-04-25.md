# Implementation Plan: 次に取れる残作業（Lab v12 並行作業候補）

## Task Type
- [x] Backend (Rust / harness-level)
- [x] Documentation / Configuration
- [ ] Frontend

## 背景（現状把握: 2026-04-25 17:50）

### 完了済（本セッション）
- 項目162: EventStore ランタイム統合（commit `15f579e`、+1テストで909）
- ファイル衛生: `.DS_Store` un-track、Lab artifact `main.rs` 削除（commit `0c5dc6c`）
- Phase C+リファクタ草案: `.claude/plan/phase-c-and-refactor-draft.md`（commit `6da35ac`）
- A→D→E 調査完了（コード変更なしのレポート）
- パス A 4タスク完了:
  - `~/.config/bonsai-agent/config.toml` に `sse_chunk_timeout_secs = 180`
  - CLAUDE.md 項目112に MLX 注記
  - CLAUDE.md 項目162の数値訂正（3→2 ACCEPT、DB整合）
  - `memory/lab_history_v1_v6.md` 新設 + CLAUDE.md 縮約（330→310行）
  - MEMORY.md にエントリ追加 + Lab v11 数値訂正

### 未コミット
- `CLAUDE.md`（5箇所修正済、ローカル変更）
- `~/.config/bonsai-agent/config.toml`（追記済、git管理外）
- `~/.claude/projects/.../memory/lab_history_v1_v6.md`（新規、git管理外）
- `~/.claude/projects/.../memory/MEMORY.md`（更新済、git管理外）

### Lab v12 状態
- task_id `bm5fh8kse` ベースライン計測中（リリースバイナリ駆動、ソース変更は影響なし）
- 次回ポーリング ScheduleWakeup 約 22分後

### `.claude/plan/` 状況（肥大化）
- 8ファイル、合計約 64KB
- 古い草案: `fix-linter-revert.md`（04-20）、`structural-improvements.md`（04-24 v1）
- 完了タスク残骸: `nat-knowledge-transfer.md`、`structural-improvements-v2.md` の Step 0/2/5 は完了済

---

## Technical Solution

Lab v12 完走（推定残り 1.5–2.5 時間）を待つ間に、コード変更なし or 局所変更のみで完了する作業を優先度別に整理。

---

## Implementation Steps（優先度別、独立実行可）

### 🟢 P0: 即commit（5分、リスクゼロ）

**Step 1: パス A 変更をコミット**
- 範囲: `CLAUDE.md` のみ（リポジトリ管理下）
- メッセージ案:
  ```
  docs: CLAUDE.md整理 + MLXタイムアウト180秒推奨化

  - 項目112にMLXバックエンド180秒推奨を注記
  - 項目162のLab v11 ACCEPT数訂正（3→2、DB整合）
  - Lab v1/v3/v5/v6.2 セクションをmemory/lab_history_v1_v6.mdへ分離（-20行）
  - CLAUDE.md 330→310行（-6%）
  ```
- 期待: Lab v13 起動前に MLX タイムアウト設定が確実に反映

### 🟢 P1: `.claude/plan/` 整理（10分、リスクゼロ）

**Step 2: 完了済プラン草案のアーカイブ or 削除**
- 削除候補:
  - `fix-linter-revert.md` — `scripts/watch_revert.sh` 実装済 → 削除
  - `nat-knowledge-transfer.md` — 項目138-145 全実装済 → 削除
  - `structural-improvements.md` — v2 で代替 → 削除
- 保持候補:
  - `structural-improvements-v2.md` — Step 6/7/8/9 が未着手で価値あり
  - `phase-c-and-refactor-draft.md` — 最新草案
  - `continuation-2026-04-25.md` — 引き継ぎ
  - `lab-v11-accept-analysis.md` — 履歴根拠

### 🟡 P2: SSE タイムアウト境界テスト追加（30分）

**Step 3: `llama_server.rs` の `sse_chunk_timeout_secs` 単体テスト**
- 現状: `sse_chunk_timeout_secs` は読み込まれて `timeout_recv_body` に流れるが、デフォルト値や境界値の単体テストなし
- 追加内容:
  ```rust
  #[test]
  fn test_sse_timeout_default_is_60() { ... }
  #[test]
  fn test_sse_timeout_zero_disables() { ... } // または min clampを確認
  #[test]
  fn test_sse_timeout_mlx_recommended_180() { ... }
  ```
- 期待: 設定ミス検出 + MLX 180秒値の意図文書化
- 影響: テスト+3、ロジック変更なし

### 🟡 P3: harness_improvements_v1.md 追従更新（20分）

**Step 4: 73項目 → 162項目への追従**
- 現状: memory/harness_improvements_v1.md は 632テスト時点で停滞（73項目）
- 更新内容: CLAUDE.md 最新項目 162 までのサマリー反映、テスト数 909
- 構造案: 「項目1-50 / 51-100 / 101-150 / 151-162 + 主要マイルストーン」
- 期待: memory整合性回復、新規セッション起動時の文脈精度向上

### 🔵 P4: Phase C テストスケルトン先行（30分）

**Step 5: ベンチマーク 18タスクの `#[ignore]` テスト追加**
- 草案: `.claude/plan/phase-c-and-refactor-draft.md` Part 1
- 内容: `benchmark.rs` に新規 18 タスク（カテゴリ別）の `BenchmarkTask` 構造体宣言のみ追加（実行はまだしない）
- TDD Red フェーズ: `#[test] #[ignore]` で 18 タスクのスケルトン
- 期待: Lab v12 完走後の本実装で TDD Green フェーズへ即移行可能

### 🔵 P5: agent_loop.rs 分割の実コード行数検証（20分）

**Step 6: 分割設計の精査**
- 草案: `.claude/plan/phase-c-and-refactor-draft.md` Part 2
- 検証内容: 各分割モジュール（mod.rs / config.rs / core.rs / outcome.rs / injection.rs）の想定行数 vs 実コード再カウント
- 出力: `.claude/plan/agent-loop-split-validated.md`（コード変更なし）

### 🟣 P6: Lab v12 完走後タスク（待機）

**Step 7: Lab v12 結果分析 + Phase C 本実装 + agent_loop.rs 分割**
- 推定残り 1.5-2.5時間（ベースライン計測中）
- 完走後フロー: 結果記録 → handoff更新 → Phase C 実装（Step 5 のスケルトンを Green に） → 分割実行

---

## 推奨パス

| パス | 内容 | 工数 | 価値 |
|---|---|---|---|
| **α**: 最小工数 | Step 1（commit）| 5分 | 高（Lab v13 設定反映保証）|
| **β**: 整頓重視 | Step 1 → 2（plan整理）| 15分 | 高（既存草案ノイズ除去）|
| **γ**: テスト先行 | Step 1 → 2 → 3（SSE単体テスト）| 45分 | 中（境界条件可視化）|
| **δ**: 最大進捗 | Step 1 → 2 → 4 → 5（memory + Phase C スケルトン）| 65分 | 高（Lab v12完走後即実装可）|
| **ε**: 設計精査 | Step 1 → 2 → 6（分割設計検証）| 35分 | 中（実装前の精度向上）|

**推奨: β または δ**
- β は安全・短時間・整頓効果大
- δ は Lab v12 完走後の本実装速度を最大化

---

## Key Files

| File | Operation | Step |
|------|-----------|------|
| (git) | Commit | 1 |
| .claude/plan/{fix-linter-revert,nat-knowledge-transfer,structural-improvements}.md | Delete | 2 |
| src/runtime/llama_server.rs | Modify (tests追加) | 3 |
| ~/.claude/projects/.../memory/harness_improvements_v1.md | Rewrite | 4 |
| src/agent/benchmark.rs | Modify (`#[ignore]` テスト追加) | 5 |
| .claude/plan/agent-loop-split-validated.md | Create | 6 |

---

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| Step 1 commit でリンター巻き戻しが起きる | CLAUDE.md は md ファイル、clippy 対象外。リスクほぼゼロ |
| Step 2 削除した plan を後から参照したくなる | git 履歴から復元可能。削除前に commit 推奨 |
| Step 3 テスト追加で `cargo test` が遅くなる | `#[test]` のみ、実行時間は無視できる |
| Step 5 のスケルトンが将来仕様と齟齬 | `#[ignore]` 付きで未実装明示、CI 影響なし |
| Lab v12 完走中に main ファイル編集 | リリースバイナリで動作中、ソース変更は影響しない（既確認）|

---

## SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: (not invoked — メタ計画のため外部分析省略)
- GEMINI_SESSION: (not invoked)

---

## 備考

本プランは 2026-04-25 17:50 時点の状態（Lab v12 baseline 計測中、CLAUDE.md ローカル変更あり、テスト909）を前提とする。Lab v12 完走後は別プランで Phase C 本実装と agent_loop.rs 分割に移行する。
