# 草稿: Phase C ベンチマーク拡張 + agent_loop.rs リファクタ設計

> **位置づけ**: continuation-2026-04-25.md の Step 7 (Phase C) と structural-improvements-v2.md の P2 Step 7 (agent_loop 分割) を実装前にまとめた設計草稿。
> Lab v12 結果待ちの並行作業として作成、実装着手は Lab v12 結果分析後に判断。

---

## Part 1: Phase C — ベンチマーク 22→40タスク拡張

### 現状（src/agent/benchmark.rs）

```rust
pub enum TaskCategory {
    ToolUse, Reasoning, MultiStep, ErrorRecovery,
    ToolSelection, CodeGeneration, Summarization,
}

pub struct BenchmarkTask {
    pub id: String,
    pub name: String,
    pub input: String,
    pub expected_tools: Vec<String>,
    pub expected_keywords: Vec<String>,
    pub max_iterations: usize,
    pub category: TaskCategory,
}

pub fn default_tasks() -> Self { ... }  // 22タスク, L298
```

### 設計差分

#### 1. `TaskTag` enum 追加

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskTag {
    Smoke,  // 5タスク - 開発時短縮用
    Full,   // 40タスク - Lab/CI/夜間
}

// BenchmarkTask に追加
pub struct BenchmarkTask {
    // ... 既存フィールド ...
    pub tag: TaskTag,
}

impl BenchmarkSuite {
    pub fn smoke_tasks() -> Self {
        Self { tasks: Self::default_tasks().tasks.into_iter()
            .filter(|t| t.tag == TaskTag::Smoke).collect() }
    }
    pub fn full_tasks() -> Self { Self::default_tasks() }
}
```

#### 2. 既存22タスクへの `tag` 付与方針

- **Smoke (5タスク)**: 各カテゴリから代表 1 件
  - ToolUse: `read_readme`
  - MultiStep: `git_status_explain`
  - ErrorRecovery: `nonexistent_file_read`
  - ToolSelection: `repo_map_count_files`
  - CodeGeneration: `fizzbuzz_implement`
- **Full (残り17タスク)**: 既存タスクは Full 扱い

#### 3. 新規追加18タスク（Full 専用）

| カテゴリ | タスク名 | input例 | expected_tools |
|---------|---------|--------|---------------|
| MultiFileEdit | rename_var_3files | "src/foo.rs/bar.rs/baz.rs の変数 `old_name` を `new_name` にリネーム" | repo_map, multi_edit×3 |
| MultiFileEdit | sig_change_4files | "関数 `foo(a: i32)` を `foo(a: i32, b: bool)` に変更し全呼出元更新" | grep, multi_edit×4 |
| LongRun | tool_chain_10steps | "RepoMap→各ファイルに `// audited` コメント追加→git diff確認" | repomap, file_read×N, multi_edit×N, git |
| LongRun | implement_50steps | "FizzBuzz 拡張: 7→`Bazz`, 11→`Lazz` 含めた仕様、テスト追加、cargo test 確認" | file_write, shell |
| ToolChain | repomap_read_edit_test | "ファイル A.rs の `parse` 関数を `parse_v2` に改名し依存ファイルのテスト追加" | repomap, file_read, multi_edit, file_write |
| ToolChain | grep_multiedit | "`anyhow::Result` を `Result<T, MyError>` 型に置換（grep でヒット箇所特定→multi_edit）" | grep, multi_edit |
| ErrorRecovery | tool_fail_pivot | "存在しないコマンドを 3 回試行後、別ツールで代替（fakecmd → shell ls）" | shell |
| ErrorRecovery | corrupt_file_repair | "壊れた JSON ファイル `/tmp/broken.json` を読込→修復→書出" | file_read, file_write |
| McpInteg | mcp_filesystem_list | "filesystem MCP で `/tmp` ディレクトリ一覧取得" | filesystem:list_directory |
| McpInteg | mcp_search_replace | "filesystem MCP で `/tmp/test.txt` の `foo` を `bar` に置換" | filesystem:read_file, filesystem:write_file |
| Semantic | vague_log_improve | "ログを改善して" (具体化を求める) | （明示的なツール期待なし、検証で具体的提案を確認） |
| Semantic | refactor_intent | "このコードもっと綺麗にして" (任意ファイル選択+リファクタ提案) | repomap, file_read |
| Reasoning | nested_logic | "x=3, y=5 のとき `if x > y && (x + y) % 2 == 0` の真偽" | （推論のみ、ツール不要） |
| Reasoning | ambiguous_calc | "2の8乗を3で割った余りは" | （推論のみ） |
| Summarization | multi_file_summary | "src/agent/agent_loop.rs と tool_exec.rs の役割の違いを200字で" | file_read×2 |
| Summarization | git_log_summary | "直近5コミットを「変更概要+影響範囲」表で要約" | git |
| Verification | self_check_arithmetic | "(17 × 23) を計算して、間違いがないか自分で検算してから答えて" | （推論+自己検証） |
| Verification | tool_fact_check | "現在のディレクトリにファイル `Cargo.toml` が存在するか確認して" | file_read or shell ls |

### 4. experiment.rs 連携

```rust
// experiment.rs - 既存の run_experiment_loop に切替パラメータ追加
pub struct ExperimentConfig {
    // ... 既存 ...
    pub benchmark_tag: TaskTag,  // Smoke (デフォルト, 開発) or Full (Lab/CI)
}

// CLI: cargo run -- --lab --full でフル40タスク（既定 smoke=5タスク, 別途 full=40）
// または: --lab を従来通り full のまま、新規 --lab-smoke で smoke
```

→ 互換性維持: `--lab` は従来通り Full（変更なし）、新規 `--lab-smoke` を追加。Lab v12 既存実行と衝突しない。

### 5. テスト戦略

```rust
#[test]
fn test_smoke_tag_filter() {
    let smoke = BenchmarkSuite::smoke_tasks();
    assert_eq!(smoke.tasks.len(), 5);
    assert!(smoke.tasks.iter().all(|t| t.tag == TaskTag::Smoke));
}

#[test]
fn test_full_includes_all_18_new() {
    let full = BenchmarkSuite::full_tasks();
    assert_eq!(full.tasks.len(), 40);
    let new_ids = ["rename_var_3files", "sig_change_4files", /* ... */];
    for id in new_ids {
        assert!(full.tasks.iter().any(|t| t.id == id), "missing {id}");
    }
}

#[test]
fn test_task_determinism() {
    // 同 seed で 2 回実行 → スコア一致（既存パターン）
}
```

### 6. リスクと対策

| リスク | 対策 |
|--------|------|
| 40タスクで Lab 時間が ~倍（4-8時間） | smoke タグで開発時短縮（5タスク = ~30分） |
| 新規タスクの「正解」が曖昧 | expected_tools/keywords を明示、Verification 系は recursion_check で対応 |
| MCP タスクが filesystem 権限で失敗 | 設定を `/tmp/bonsai-bench-sandbox/` に固定、テストフィクスチャで作成 |
| 既存ベースライン（Lab v11 以前）と非互換 | smoke を既定にせず、`--lab` は full のまま固定（互換性維持） |

### 実装規模見積

- benchmark.rs: 1454 → ~1850行（+400行 = 18タスク × ~22行）
- experiment.rs: +20行（ExperimentConfig 拡張）
- main.rs (CLI): +5行（`--lab-smoke` フラグ）
- テスト: +6件
- **テスト合計**: 909 → 915

---

## Part 2: agent_loop.rs リファクタ設計（P2 Step 7）

### 現状規模

```
src/agent/agent_loop.rs   2661行 total
├── 本体              1252行 (L1-1252)
└── tests            1408行 (L1253-2661)
```

**項目160-162追加で 2497→2661 (+164行)**。テスト膨張が顕著。

### 関数マップ（本体 1252 行）

| 行範囲 | 関数/型 | 役割 |
|--------|---------|------|
| L31-154 | AgentConfig + Default + inference_for_task | 設定 |
| L155-176 | AgentLoopResult, StepOutcome | 結果型 |
| L177-345 | LoopState, TokenBudgetTracker, OutcomeAction, StallDetector, StepContext | ループ状態 |
| L347-507 | execute_step | LLM 1回呼出+パース |
| L508-738 | run_agent_loop, run_agent_loop_with_session | メインループ |
| L739-883 | create_task_start_checkpoint, handle_outcome | アウトカム処理 |
| L884-996 | detect_task_complexity, AdvisorResolution, resolve_advisor_prompt, log_advisor_call | Advisor補助 |
| L997-1152 | inject_replan_on_stall, inject_verification_step, inject_planning_step, compute_output_hash | コンテキスト注入 |
| L1153-1227 | check_invariants, record_success, record_abort | 検証/記録 |
| L1228-1252 | build_answer, clean_response | 応答整形 |

### 分割設計（4モジュール）

```
src/agent/agent_loop/
├── mod.rs              (~150行)
│   ├── pub use config::*; types::*; core::*;
│   └── 公開API + emit_event ヘルパー（項目162）
├── config.rs           (~250行)
│   ├── AgentConfig + Default + inference_for_task
│   ├── AgentLoopResult, StepOutcome, OutcomeAction
│   └── LoopState, TokenBudgetTracker, StallDetector, StepContext
├── core.rs             (~400行)
│   ├── run_agent_loop / run_agent_loop_with_session
│   ├── execute_step
│   └── create_task_start_checkpoint
├── outcome.rs          (~150行)
│   └── handle_outcome（OutcomeAction ディスパッチ）
└── injection.rs        (~300行)
    ├── detect_task_complexity, resolve_advisor_prompt, log_advisor_call
    ├── inject_replan_on_stall, inject_verification_step, inject_planning_step
    ├── check_invariants, record_success, record_abort
    ├── build_answer, clean_response, compute_output_hash
    └── AdvisorResolution
```

**テストの扱い**:
- 既存 1408 行を `core.rs` 末尾に集約 → 既存 cargo test --lib で全動作維持
- 段階的に分割テストに整理（後続作業）

### 公開API（外部 import）

```rust
// 現状の他モジュールからの import パターン:
// - benchmark.rs: AgentConfig, AgentLoopResult, run_agent_loop
// - experiment.rs: AgentConfig, AgentLoopResult, run_agent_loop
// - subagent.rs: AgentConfig, run_agent_loop_with_session

// 分割後 mod.rs で再エクスポート:
pub use config::{AgentConfig, AgentLoopResult, StepOutcome, OutcomeAction, ...};
pub use core::{run_agent_loop, run_agent_loop_with_session};
```

外部 import 互換性100%維持（`crate::agent::agent_loop::AgentConfig` のまま動作）。

### 分割手順（巻戻しリスク対策付き）

```bash
# Step 1: ディレクトリ作成 + mod.rs 雛形
mkdir -p src/agent/agent_loop
git mv src/agent/agent_loop.rs src/agent/agent_loop/_legacy.rs

# Step 2: config.rs 切出（types のみ、関数なし）
# テスト実行 → cargo build OK
git commit -m "refactor: agent_loop/ ディレクトリ化 + config.rs 分離"

# Step 3: outcome.rs 切出（handle_outcome のみ）
git commit -m "refactor: handle_outcome を outcome.rs に分離"

# Step 4: injection.rs 切出（注入系+検証系）
git commit -m "refactor: 注入/検証ヘルパーを injection.rs に分離"

# Step 5: _legacy.rs → core.rs 改名 + 残コード集約
# テスト全Pass → 完了
git commit -m "refactor: agent_loop.rs を 4モジュールに分割（mod/config/core/outcome/injection）"
```

各 Step で **必ず即commit**（CLAUDE.md「巻戻し禁止」原則）。clippy auto-fix が走る前にコミット。

### リスクと対策

| リスク | 対策 |
|--------|------|
| Lab v12 実行中の binary は変更不要だが、再実行時にビルド失敗 | 各 Step で cargo build 確認、Lab 再起動前に必ず動作確認 |
| 公開 API 互換性破綻 | mod.rs で `pub use` 全関数/型を再エクスポート、benchmark.rs/experiment.rs/subagent.rs の import 維持 |
| clippy auto-fix で巻戻し | 4 段階 commit 戦略で各時点を保護 |
| テストの import パス変更 | tests を core.rs 末尾に維持（移動なし）→ 後続別作業で整理 |
| pub(crate) 関数の可視性 | 分割後 `pub(super)` または `pub(crate)` に明示変更 |

### 実装規模見積

- agent_loop.rs (旧) 削除 → 5モジュール作成
- 行数: 2661 → 約 mod 150 + config 250 + core 1700 (本体400 + tests 1300) + outcome 150 + injection 300 = ~2550行
- 実質変更（テスト含めず）: 1252 → 1250行（-2行、ほぼ移動のみ）
- テスト: 全909件維持（移動による壊れなし）

### 実施判断基準

**今は実装しない**。理由:
1. Lab v12 が release バイナリを実行中（再ビルド必要なら lab 中断リスク）
2. 項目162 の EventStore 統合直後で agent_loop.rs に追加変更が入ったばかり（ホットコード）
3. P2 Step 7 は plan 上「バックログとして据置」

**実施タイミング**:
- Lab v12 完了後 → 結果分析 → handoff 更新 → 次セッション冒頭で実施候補
- 先に Phase C（ベンチマーク拡張）の方が Lab 評価精度向上に直結するため優先度高

---

## 推奨実施順序（Lab v12 完了後）

```
Lab v12 完了
  ↓
Phase A: Lab v12 結果分析 (15分)
  - score / pass@k 集計
  - ACCEPT 変異の中身確認
  - meta 変異恒久化判定
  ↓
Phase B: handoff 更新 (15分)
  - MEMORY.md / session_2026_04_26_handoff.md 作成
  ↓
Phase C: ベンチマーク 18タスク追加 (2-3時間)
  - 本ドキュメント Part 1 を実装
  - cargo test --lib 通過確認
  - commit "feat: ベンチマーク40タスク化（22→40, smoke/fullタグ、項目163）"
  ↓
Phase D (Optional): agent_loop.rs 分割 (1-2時間)
  - 本ドキュメント Part 2 を実装
  - 4段階 commit 戦略で巻戻し防止
  - commit "refactor: agent_loop.rs を 4モジュールに分割（項目164）"
  ↓
Lab v13: 40タスク full ベンチマーク (3-5時間バックグラウンド)
  - 標準偏差 ±0.005 検証
```

---

## SESSION_ID
- CODEX_SESSION: (not invoked)
- GEMINI_SESSION: (not invoked)

## 備考
本草稿は Lab v12 並行作業として作成。実装は Lab v12 結果次第で順序調整あり。
