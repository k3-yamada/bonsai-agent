# macOS26/Agent 知見抽出 v2（2026-04-27）

**Source:** https://github.com/macOS26/Agent (push 2026-04-25)
**前回取込:** CLAUDE.md 項目 41-44 (StuckGuard / Anti-Hallucination / TokenBudgetTracker / health monitoring)
**ステータス:** 設計ドキュメント（Lab v13 完了後の実装判断用）

---

## 抽出対象 8 ファイル

| ファイル | 概要 | 取込済 |
|---|---|---|
| `TabTask/StuckGuard.swift` | 連続失敗 nudge / give-up | ✅ 項目42 |
| `TaskExecution/ToolBatch.swift` | 読取並列 / 書込逐次 | ✅ 項目55 |
| `TaskExecution/Guards.swift` | overnight + edit cycle | 一部（cycle 未取込） |
| `TabTask/TT-Triage.swift` | 軽量 triage | ❌ |
| `Services/FallbackChainService.swift` | model fallback chain | ❌ |
| `Services/FileChangeJournal.swift` | JSONL ファイル変更ログ | ❌ |
| `Services/DiffStore.swift` | UUID 経由 diff 適用 + undo | ❌ |
| `TaskExecution/AboutSelf.swift` | LLM 自己問合ツール | ❌ |

---

## ★★★ 採用推奨候補

### **C. DiffStore — UUID-based diff cache + multi-step undo**

**Source:** `Agent/Services/DiffStore.swift`

**現状**:
- `create_diff` が UUID を返却 → `apply_diff(diff_id)` で参照
- LLM はフル diff テキストをエコーバック不要 → **トークン削減**
- ファイルごとに `appliedDiffs: [String: [UUID]]` で stack 管理
- `editHistory: [String: String]` で初回編集前の original を保持 → 完全 undo 可能

**bonsai-agent 適用案**:
- `src/agent/diff_store.rs` 新設
  ```rust
  pub struct DiffStore {
      diffs: HashMap<Uuid, DiffEntry>,
      applied: HashMap<PathBuf, Vec<Uuid>>,
      edit_history: HashMap<PathBuf, String>,  // 初回 original
  }
  ```
- `FileWriteTool` / `MultiEditTool` (項目111) と統合
- `undo_last_n(path, n)` API で粒度細かいロールバック（git stash CP より軽量）
- 効果見込み: **1bit モデルの長 diff トークン消費を 50%+ 削減**（実測必要）

**実装工数**: 中（4h、TDD で 6-8 テスト）
**優先度**: ★★★

### **A. Multi-File Edit Cycle Detection**

**Source:** `Agent/AgentViewModel/TaskExecution/Guards.swift::detectEditCycle`

**現状**:
- 過去 6 ターンで編集したファイルパスを window で記録
- 2-3 ファイルが交互編集（全部が 2 回以上出現） → cycle 検出
- nudge メッセージで "ファイルを再読してから一括計画" を促す

**bonsai-agent 適用案**:
- `src/agent/error_recovery.rs::LoopDetector` に拡張
  ```rust
  pub struct MultiFileEditCycleDetector {
      recent_paths: VecDeque<String>,
      window: usize,  // = 6
  }
  impl MultiFileEditCycleDetector {
      pub fn record_and_check(&mut self, path: &str) -> Option<String> {
          // returns Some(nudge_msg) if cycle detected
      }
  }
  ```
- 既存 `LoopDetector` は同一 tool_call hash のみ検出（ファイル間交互は未検出）
- 統合先: `outcome.rs::handle_outcome` の Continue 分岐内

**実装工数**: 低（1h、3-4 テスト）
**優先度**: ★★

### **D. Model Fallback Chain**

**Source:** `Agent/Services/FallbackChainService.swift`

**現状**:
- ユーザー設定可能な (provider, model) の順序リスト
- 連続失敗 N 回（デフォルト 2）で次へ自動切替
- 「夜間放置運転」を想定

**bonsai-agent 適用案**:
- `src/runtime/model_router.rs::ModelRouter` を拡張
  ```rust
  pub struct FallbackChain {
      entries: Vec<(BackendType, ModelId)>,
      current_idx: i32,  // -1 = primary
      consecutive_failures: usize,
      max_failures_before_fallback: usize,  // = 2
  }
  ```
- 既存 Advisor の `backend = "claude-code"` フォールバックは粒度違い（advice 専用）
- メイン推論にも適用：例 `mlx-lm 失敗 2 回 → llama-server に切替 → BitNet に切替`
- config.toml `[fallback_chain]` セクション

**実装工数**: 中（3h、5-6 テスト）
**優先度**: ★★

---

## ★ 低優先（既存機能で代替）

### B. FileChangeJournal
- JSONL 日次ログで before/after hash を記録
- bonsai の `EventStore` (項目161/162) で同等の追跡可能（schema 拡張で対応）
- **採用見送り**: EventStore に `FileChange` イベントタイプを追加で代替可

### E. AboutSelf tool
- LLM が「自分のツール能力」を問い合わせるメタツール
- `format_schemas_progressive` (項目72) + Tool tag 開示で代替済
- **採用見送り**

### F. TabTaskTriage
- 軽量タスク → Apple Intelligence、重いタスク → クラウド LLM
- bonsai は 1bit モデル特化のため triage 効果が薄い（モデル選択の余地が小さい）
- **採用見送り**

### G. runOvernightCodingGuards
- 夜間長時間運転用の error budget 管理
- bonsai の `CircuitBreaker` + `TokenBudgetTracker` + `task_timeout` で同等以上
- **採用見送り**

---

## 提案実施順

Lab v13 完了後の構造改善 v3 (Step 10-12) として:

| Step | 候補 | 工数 | リスク | 効果実測方法 |
|------|------|------|--------|-------------|
| **10** | C: DiffStore | 4h | 中（API 追加） | Lab で diff 経由ツール使用率 + トークン消費比較 |
| **11** | A: Edit Cycle | 1h | 低（局所追加） | Lab でループ検出回数の delta |
| **12** | D: Fallback Chain | 3h | 中（runtime 改修） | 夜間 Lab の停止率改善 |

合計 8h、すべて opt-in（既存挙動を破壊しない）。

## 採否判断ゲート

実装着手前に以下を確認:

- [ ] **C**: 1bit モデルの平均 diff サイズが 200+ トークンであること（current 実測）
- [ ] **A**: Lab v13 で「同じファイルを交互編集する REJECT 変異」が観測されること
- [ ] **D**: Lab v13 で MLX 接続断絶 / API 失敗が 2 回以上発生すること

該当なしなら見送り（YAGNI）。

---

## 関連

- 前回知見: 項目 41-44（CLAUDE.md）
- bonsai 全体方針: `Scaffolding > Model`（CLAUDE.md 設計原則）
- 既存類似機能:
  - 項目 5: 2層 LoopDetector
  - 項目 33-34: LoopState / handle_outcome
  - 項目 42: FileStuckGuard
  - 項目 95: 不変条件チェック
  - 項目 111: MultiEdit
  - 項目 161/162: EventStore
