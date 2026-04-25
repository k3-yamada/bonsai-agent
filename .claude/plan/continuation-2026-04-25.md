# Implementation Plan: 続き — Lab v11後継/EventStore統合/ベンチマーク拡張

## Task Type
- [x] Backend (Rust / harness-level)
- [ ] Frontend
- [ ] Fullstack

## 背景（前回セッションからの引き継ぎ）

`session_2026_04_25_handoff.md` 完了分（コミット `1f128bb`, `cffc7a2`）:
- ✅ **項目160**: P0 Step 2（subagent並列実行＋independence判定）— 6テスト追加（898）
- ✅ **項目161**: P1 Step 5（軌跡抽出→`promote_from_trajectory`）— 10テスト追加（908）
- ✅ **Lab v11**: 3 ACCEPT/10（v8-v10の天井突破、累積 +0.0134）

**残課題（優先度順）**:
1. 🔥 **Lab v11 ACCEPT 詳細調査** — `exp_0004`（+0.0108 meta変異）の中身確認と恒久デフォルト化判定
2. 🟡 **P1 Step 5 ランタイム統合** — `run_agent_loop_with_session()` で EventStore イベント emit（実セッションで軌跡が記録されないとスキル昇格が動かない）
3. 🟢 **P2 Step 6** — ベンチマーク 22→40タスク（Lab 標準偏差 ±0.01 → ±0.005）

P2 Step 7（agent_loop.rs 2497行分割）/ P3 Step 8-9（依存関係/カバレッジ）はバックログとして据置。

---

## Technical Solution

### Phase A: Lab v11 ACCEPT 詳細調査（即時、~30分）

`SQLite experiments` テーブルから ACCEPT 3 件のメタデータを取得し、

- **exp_0004（meta_mutation, +0.0108）**: `[ツール使用前に思考を強制] + [マルチステップ計画の強制]`
  - 既存システムプロンプト（`DEFAULT_SYSTEM_PROMPT`）との重複範囲を確認
  - 既デフォルトの「ACCEPT変異1（思考強制）」と「マルチステップ計画」のテキスト統合可否を判定
  - 統合可なら CLAUDE.md 項目162 として恒久ルール化＋プロンプトに静的注入

- **exp_0002（agent_param `max_retries=4`, +0.0003）**: 効果が小さく、AgentConfig デフォルトを 3→4 に上げる根拠が薄い → **観察継続（保留）**

- **exp_0005（prompt_rule oracle insight, +0.0023）**: 「temperature 0.2 劣化」の逆向きルール
  - REJECT 元実験を特定し、insight テキストと現状ルール集の重複を確認
  - 単独効果が中程度のため、Lab v12 で検証してからデフォルト化判定

**成果物**: `.claude/plan/lab-v11-accept-analysis.md` に SQL 抽出結果＋判定マトリクス。

### Phase B: EventStore ランタイム統合（最重要、コア改善）

`extract_successful_trajectories()` は実装済みだが、**ランタイムが Event を emit していないため空回り**。これを解消する。

#### B-1. `run_agent_loop_with_session()` での emit ポイント設計

エントリ: `src/agent/agent_loop.rs:517`

| イベント | emit 位置 | event_data ペイロード |
|----------|-----------|----------------------|
| `SessionStart` | `purge_all_expired` 呼出直後（547行付近） | `{"task_context": "<ユーザー入力先頭200文字>"}` |
| `UserMessage` | 上記直後（task_context 確定後） | `{"content": "<task_context>"}` |
| `ToolCallStart` | `execute_validated_calls` の各呼出ごと | `{"tool": "<name>", "args_preview": "<200文字>"}` |
| `ToolCallEnd` | 同 ToolResult 生成直後 | `{"tool": "<name>", "success": <bool>}` |
| `SessionEnd` | `OutcomeAction::Return` 直前 + ループ最大値到達時 | `{"final_answer_preview": "<200文字>", "iterations": <usize>}` |

#### B-2. 設計方針

- **疎結合**: `EventStore` は `MemoryStore` がある時のみ emit（`store.is_some()`）
- **障害分離**: `append()` 失敗は `log_event(LogLevel::Warn, "event", ...)` で握り潰す（コアループは止めない）
- **AuditLog との重複回避**: 既存 `AuditAction::ToolCall` は粗粒度メトリクス用、Event は **シーケンス保存用**（粒度が違うので両立）
- **ToolCallStart/End の対応付け**: tool 実行は同期なので「Start → execute → End」を `apply_tool_result` 内で連続 append（id 順序で対応保証）

#### B-3. 変更ファイルと差分規模

```
src/agent/agent_loop.rs       +20行 (SessionStart/UserMessage/SessionEnd 3箇所)
src/agent/tool_exec.rs        +15行 (ToolCallStart/End 2箇所、apply_tool_result 内)
src/agent/event_store.rs       0行 (既存 API で十分)
src/agent/agent_loop.rs (helper) +10行 (emit_event ヘルパー: Option<&MemoryStore> + EventType + payload)
```

#### B-4. テスト戦略

実機ループと EventStore の統合をブラックボックステストで担保:

1. `MockLlmBackend` で 3ステップの成功シナリオを構築
2. `MemoryStore::in_memory()` を `Some(&store)` で渡して `run_agent_loop_with_session()` 実行
3. 終了後 `EventStore::extract_successful_trajectories(0.8, 2)` を呼出
4. `tool_sequence` の長さ・`task_description`・`tool_success_rate=1.0` を検証
5. 失敗ケース（ToolCallEnd で success=false 混入）でフィルタ動作確認
6. SessionEnd 欠落ケース（max_iterations 到達時）の取扱確認

→ **既存 `agent_loop` 周辺のモックテスト構築コストが高い** ため、まず `tool_exec::execute_validated_calls` レベルで `Option<&EventStore>` を受け取り、emit を分離テストする小ステップ案も検討（B-3 と並行）。

### Phase C: ベンチマーク 22→40 タスク拡張（Lab v12 後）

Phase A で恒久ルール化、Phase B でデータ基盤を整えてから Lab v12 を回す。**Lab v12 結果を見て**から Phase C に着手することで、無駄なタスク追加を避ける（タスク決定的か曖昧かは効果計測してから判断）。

設計詳細は `structural-improvements-v2.md` 既存記述を踏襲。新規 18 タスク候補:
- マルチファイル編集（リネーム3F / シグネチャ変更4F）
- 長期タスク（10/50ステップ）
- ツール連携（RepoMap→Read→Edit→Test 連鎖、Grep→MultiEdit）
- エラー回復（ツール失敗3回後の手段移行、破損ファイル修復）
- MCP 連携（モック）
- セマンティック必須（曖昧要求→具体化）

`tag: "smoke" | "full"` フィールドを `BenchmarkTask` に追加し、smoke=5タスク／full=40タスクで Lab 切替。

---

## Implementation Steps

### Step 1: Lab v11 ACCEPT 抽出（Phase A）
- `~/.config/bonsai-agent/experiments.db` または `.vcsdd/experiments.tsv` を SQL 照会
- `SELECT id, mutation_type, mutation_text, score, score_delta, accepted, notes FROM experiments WHERE accepted = 1 ORDER BY id DESC LIMIT 10;`
- exp_0004 の `mutation_text` 全文を取得 → 既存プロンプト差分確認
- 判定: 統合可 → CLAUDE.md/項目162 ＋ `DEFAULT_SYSTEM_PROMPT` 編集 ／ 保留 → Lab v12 で再検証

### Step 2: EventStore emit ヘルパー追加（Phase B-1）
- `src/agent/agent_loop.rs` 内に private 関数 `fn emit_event(store: Option<&MemoryStore>, session_id: &str, ev: &EventType, data: &str, step: Option<usize>)`
- 内部で `EventStore::new(s.conn()).append(...)` 呼出、失敗は `log_event(Warn, "event", ...)` で握る
- TDD: 単体テストで `Option<None>` パス／成功パス／失敗パスを確認

### Step 3: ループ本体に SessionStart/UserMessage/SessionEnd 注入（Phase B-2a）
- `run_agent_loop_with_session()` の `purge_all_expired` 直後に SessionStart + UserMessage emit
- `OutcomeAction::Return(result)` 返却前にラッパで SessionEnd emit
- ループ最大値到達時の Ok(...) 返却前にも SessionEnd emit
- TDD: モックバックエンドで session_id ごとに `replay()` し 3 イベントが揃うことを確認

### Step 4: ToolCallStart/End 注入（Phase B-2b）
- `tool_exec::apply_tool_result` 直前に ToolCallStart、結果反映直後に ToolCallEnd を emit
- ただし `apply_tool_result` は `&Session` のみ持ち `EventStore` を持たないので、シグネチャに `Option<&EventStore>` を追加するか、上位 `execute_validated_calls` で挟む方式に切り替え
- **推奨**: `execute_validated_calls` 内ループで `apply_tool_result` の前後に emit（`apply_tool_result` のシグネチャを変えない疎結合パス）
- TDD: 既存 `execute_validated_calls` テストを拡張、event 件数で検証

### Step 5: 統合テスト（Phase B-4）
- 新規テスト `test_run_agent_loop_emits_events`:
  - MockLlmBackend で `<tool_call>{"name":"shell",...}</tool_call>` → `[最終回答]` の2ステップシナリオ
  - 終了後 `extract_successful_trajectories(0.8, 1)` で1件取得
  - `tool_sequence == vec!["shell"]` を確認

### Step 6: Lab v12 実行（Phase B 完了後）
- `cargo run -- --lab` バックグラウンド起動
- 比較基準: Lab v11 ベースライン（score=0.7965, pass@k=0.8636）
- 期待効果: meta変異 exp_0004 を恒久化した場合 score +0.005-0.015、または「変化なし」（既デフォルトと等価判定）
- ScheduleWakeup で 20-30分間隔ポーリング

### Step 7: ベンチマーク拡張（Phase C、Lab v12 後判断）
- `BenchmarkTask` に `tag: TaskTag` フィールド追加（enum: Smoke / Full）
- 18 新タスク追加 → smoke=既存5タスク維持、full=22+18=40タスク
- TDD: タスク決定性テスト（同 seed 再実行でスコア一致）、タグフィルタ動作

### Step 8: コミット＆セッション handoff 更新
- 各 Phase 完了ごとに `feat:` または `refactor:` で個別コミット
- 完了後 `MEMORY.md` に新規 handoff エントリ追加

---

## Key Files

| File | Operation | Lines | Phase |
|------|-----------|-------|-------|
| `~/.config/bonsai-agent/experiments.db` | Read (SQL) | — | A |
| `.claude/plan/lab-v11-accept-analysis.md` | Create | new | A |
| `src/agent/agent_loop.rs:517` | Modify (emit_event ヘルパー + 3 emit) | +30 | B-2a |
| `src/agent/tool_exec.rs:153` | Modify (Tool emit 2箇所) | +15 | B-2b |
| `src/agent/event_store.rs` | (既存 API 流用、変更なし) | 0 | B |
| `src/agent/benchmark.rs` | Extend (+18タスク, TaskTag) | +400 | C |
| `src/agent/experiment.rs` | Modify (smoke/full 切替) | +20 | C |
| `CLAUDE.md` | Append (項目162-163) | +10 | A/B |

---

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| EventStore emit がループ性能を劣化させる | append は INSERT 1発、トランザクション不使用。実機 latency 計測（`step_duration_ms` 比較） |
| AuditLog と Event の意味重複 | 役割分離を CLAUDE.md に明記: AuditLog=メトリクス集計用 / Event=軌跡シーケンス保存用 |
| Tool 並列実行で Event 順序が乱れる | `execute_read_batch_parallel` 後に **結果順** に emit すれば順序保証可（現実装も apply_tool_result が逐次） |
| meta変異統合でベースライン低下 | Lab v12 で必ず比較計測。劣化したら revert（git diff で1コミット粒度に保つ） |
| SessionEnd emit 漏れ（panic / 例外パス） | `defer!` 風マクロは Rust 標準にないため、Drop 実装で `EventStore` ラッパに保証させる選択肢を検討（過剰設計気味なので最初は手動で全 return パス確認） |
| ベンチマーク40タスクで Lab 実行時間が倍 | smoke タグデフォルトで開発時短縮、full は CI/夜間専用 |
| `EventStore::append` が SQLite write で他処理をブロック | サブエージェント並列実行下では同時 INSERT 競合の可能性 → 現実的には1セッション内シングルスレッド emit なので大丈夫だが、サブエージェント並列時は専用 session_id でテスト |

---

## 実施順序

```
今すぐ:    Step 1 (Lab v11 ACCEPT 抽出) — 30分
           ↓
Phase B:   Step 2-5 (EventStore 統合 + テスト) — 2-3時間
           ↓ コミット
Lab v12:   Step 6 (バックグラウンド) — 2-4時間
           ↓ 結果分析
判断:      meta変異恒久化 vs 保留
           ↓
Phase C:   Step 7 (ベンチマーク 40タスク) — Lab v12 で標準偏差不足が確認できた場合のみ
           ↓
Step 8:    コミット + handoff 更新
```

---

## Success Metrics

- **Phase A**: ACCEPT 3件の中身が SQL 照会で取得され、判定マトリクスが文書化されている
- **Phase B**: 実セッション完走後 `extract_successful_trajectories()` が ≥1件返す。テスト 908 → 916（+8 程度）
- **Lab v12**: ベースライン更新（meta変異統合の純効果が定量化される）
- **Phase C**（実施した場合）: Lab 標準偏差 ±0.005 以下、smoke=5タスクで開発時短縮確認

---

## 注意事項（CLAUDE.md / 過去 handoff から継承）

- **Edit/Write 後の clippy 警告を理由とした巻戻し禁止** — 特に `agent_loop.rs` / `tool_exec.rs` への変更は即 commit
- **Fact-Forcing Gate** が CLAUDE.md/event_store.rs 等への複数 Bash/Write をブロックする可能性 → 4ファクト提示で解除（過去 handoff 参照）
- **TDD 原則**: 各 Step で Red → Green → Refactor、テスト先行
- **Lab 実行は非ブロッキング**: `run_in_background: true` 起動後、TaskOutput でポーリング、kill しない
- **大量変更時は Python subprocess + 即 git commit** 手法（過去確立済）

---

## SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: (not invoked — analyzer.md 役割プロンプト未確認のため、既存コードベース直接調査ベース)
- GEMINI_SESSION: (not invoked)

---

## 備考

本プランは前回セッション handoff（2026-04-25）の「次セッション最優先タスク」3 件を技術設計まで掘り下げて構造化したもの。dual-model（Codex/Gemini）分析は役割プロンプト未配置で省略し、Read/Grep ベースの直接調査で代替。Phase A→B→Lab v12→C の順序は **計測ファースト（Phase A）→ 基盤整備（Phase B）→ 効果検証（Lab v12）→ 拡張（Phase C）** の依存関係を反映。
