# DiffStore Rust 実装スケルトン詳細設計

**Source:** `macos26-agent-learnings-v2.md` ★★★ 採用候補 C
**Date:** 2026-04-27
**Status:** 設計（Lab v13 完了後の実装候補）

---

## 動機

**問題**: 1bit モデルが MultiEdit/file_write で大きな diff を渡す際、フル diff テキストをエコーバックする必要があり、トークン消費が大きい（推定 200+ トークン/diff）。さらに、複数編集の段階的 undo が git stash 単位（CP）でしか効かない。

**ゴール**:
1. **トークン削減**: `create_diff` が UUID を返却 → `apply_diff(UUID)` で参照 → LLM はフル diff エコー不要
2. **粒度細かい undo**: ファイルごとに apply 履歴を保持 → 直近 N 個の diff を巻き戻し可能
3. **完全 undo**: 初回編集前の original を保持 → 全編集の取消が可能

## 設計

### モジュール配置

```
src/agent/
├── diff_store.rs        ← 新規モジュール
├── tool_exec.rs         (既存、DiffStore 利用)
└── ...

src/tools/
├── file.rs              (既存、apply_diff/undo_diff ツール追加)
└── ...
```

### 公開 API

```rust
// src/agent/diff_store.rs

use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// DiffStore — UUID 経由の diff キャッシュ + 多段 undo
///
/// セッション単位の in-memory ストア。プロセス終了で揮発。
/// 永続化が必要なら git stash CP（既存）を使う。
#[derive(Debug, Default)]
pub struct DiffStore {
    /// UUID → diff 本体（before/after の文字列）
    diffs: HashMap<Uuid, DiffEntry>,
    /// path → 適用済み diff の stack（古い順）
    applied: HashMap<PathBuf, Vec<Uuid>>,
    /// path → 初回編集前の original content（多段 undo 用）
    edit_history: HashMap<PathBuf, String>,
}

#[derive(Debug, Clone)]
pub struct DiffEntry {
    /// SEARCH/REPLACE 形式の生 diff テキスト
    pub source: String,
    /// パース済み: 旧テキスト
    pub old_str: String,
    /// パース済み: 新テキスト
    pub new_str: String,
    /// 作成時刻
    pub created_at: std::time::Instant,
}

impl DiffStore {
    /// 新規 diff を登録、UUID を返却
    pub fn store(&mut self, source: String, old_str: String, new_str: String) -> Uuid {
        let id = Uuid::new_v4();
        self.diffs.insert(id, DiffEntry {
            source, old_str, new_str,
            created_at: std::time::Instant::now(),
        });
        id
    }

    /// UUID から diff を取得
    pub fn get(&self, id: Uuid) -> Option<&DiffEntry> {
        self.diffs.get(&id)
    }

    /// 適用記録（diff_id をパスに紐付け、初回なら original も保存）
    pub fn record_apply(&mut self, id: Uuid, path: PathBuf, original: &str) {
        self.applied.entry(path.clone()).or_default().push(id);
        // 初回編集なら original を保持（以後は上書きしない）
        self.edit_history.entry(path).or_insert_with(|| original.to_string());
    }

    /// 直近 N 個の diff を取消し、何個取消したかを返す
    pub fn undo_last_n(&mut self, path: &PathBuf, n: usize) -> usize {
        let stack = self.applied.entry(path.clone()).or_default();
        let count = stack.len().min(n);
        stack.truncate(stack.len() - count);
        count
    }

    /// 全 undo: original に戻す
    pub fn undo_all(&mut self, path: &PathBuf) -> Option<String> {
        self.applied.remove(path);
        self.edit_history.remove(path)
    }

    /// セッション開始時クリア
    pub fn clear(&mut self) {
        self.diffs.clear();
        self.applied.clear();
        self.edit_history.clear();
    }
}
```

### ツール追加

#### `create_diff` ツール（新規）

```rust
// src/tools/file.rs

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateDiffArgs {
    /// SEARCH/REPLACE 形式の diff テキスト
    pub diff: String,
}

pub struct CreateDiffTool;

impl TypedTool for CreateDiffTool {
    type Args = CreateDiffArgs;

    fn name(&self) -> &str { "create_diff" }
    fn description(&self) -> &str {
        "Diff (SEARCH/REPLACE) をパースして UUID として保存。返却 UUID を apply_diff に渡すと再利用できる（フル diff エコー不要、トークン削減）"
    }

    fn call_typed(&self, args: Self::Args, ctx: &ToolContext) -> Result<ToolResult> {
        let (old_str, new_str) = parse_search_replace(&args.diff)?;
        let id = ctx.diff_store.lock().store(args.diff.clone(), old_str, new_str);
        Ok(ToolResult::success(format!("diff_id={id}")))
    }
}
```

#### `apply_diff` ツール（新規）

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ApplyDiffArgs {
    /// create_diff から返却された UUID
    pub diff_id: String,
    /// 編集対象ファイルパス
    pub file_path: String,
}

pub struct ApplyDiffTool;

impl TypedTool for ApplyDiffTool {
    type Args = ApplyDiffArgs;

    fn name(&self) -> &str { "apply_diff" }

    fn call_typed(&self, args: Self::Args, ctx: &ToolContext) -> Result<ToolResult> {
        let id = Uuid::parse_str(&args.diff_id)?;
        let entry = ctx.diff_store.lock().get(id).cloned()
            .ok_or_else(|| anyhow::anyhow!("diff_id {id} not found"))?;
        let path = PathBuf::from(&args.file_path);
        let original = std::fs::read_to_string(&path)?;
        let modified = original.replacen(&entry.old_str, &entry.new_str, 1);
        if modified == original {
            anyhow::bail!("diff did not match");
        }
        std::fs::write(&path, &modified)?;
        ctx.diff_store.lock().record_apply(id, path, &original);
        Ok(ToolResult::success(format!("applied {id} to {}", args.file_path)))
    }
}
```

#### `undo_diff` ツール（新規）

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UndoDiffArgs {
    pub file_path: String,
    /// undo 個数（None なら 1、"all" なら全 undo）
    pub count: Option<UndoCount>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum UndoCount {
    N(usize),
    All(String),  // "all" のみ受け入れ
}

pub struct UndoDiffTool;
// (実装略 — DiffStore::undo_last_n / undo_all を呼び出す)
```

### 統合ポイント

#### `LoopState` への追加

```rust
// src/agent/agent_loop/state.rs

pub struct LoopState<'a> {
    // ... 既存フィールド ...
    pub diff_store: Arc<Mutex<DiffStore>>,
}

impl<'a> LoopState<'a> {
    pub fn new(advisor: AdvisorConfig) -> Self {
        Self {
            // ... 既存初期化 ...
            diff_store: Arc::new(Mutex::new(DiffStore::default())),
        }
    }
}
```

#### `ToolContext` への追加

```rust
// src/tools/mod.rs (or context module)

pub struct ToolContext<'a> {
    // ... 既存 ...
    pub diff_store: Arc<Mutex<DiffStore>>,
}
```

### TDD 計画（推定 6-8 テスト）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_store_returns_unique_uuid() { /* ... */ }

    #[test]
    fn t_get_returns_stored_entry() { /* ... */ }

    #[test]
    fn t_record_apply_pushes_to_stack() { /* ... */ }

    #[test]
    fn t_record_apply_preserves_first_original() { /* ... */ }

    #[test]
    fn t_undo_last_n_truncates_stack() { /* ... */ }

    #[test]
    fn t_undo_all_returns_original() { /* ... */ }

    #[test]
    fn t_clear_resets_all() { /* ... */ }

    #[test]
    fn t_create_diff_tool_returns_uuid_string() { /* integration */ }

    #[test]
    fn t_apply_diff_tool_modifies_file() { /* integration */ }
}
```

## トークン削減効果見積もり

**前提**: 平均 diff サイズ 200 トークン、Lab v13 で MultiEdit 呼出が 30 回/セッション

**変更前**:
- LLM が SEARCH/REPLACE フル diff を毎回エコーバック
- 30 回 × 200 = 6000 トークン

**変更後**:
- 1 回目: create_diff で 200 トークン → UUID 取得 (~40 文字 = 12 トークン)
- 2 回目以降: apply_diff(UUID) のみで 12 トークン
- 30 回 ≈ 200 + 29 × 12 = 548 トークン

**削減率**: 6000 → 548 = **約 91% 削減**（理論最大、同一 diff 再利用率による）

実測必要: 同一 diff の再利用率は Lab で測定し、1bit モデルが UUID 経由運用を学習可能か確認。

## 実装手順（TDD、推定 4h）

| Step | 内容 | テスト数 | 工数 |
|------|------|---------|------|
| 1 | `diff_store.rs` 新規 + Red テスト 7 件 | +7 失敗 | 30分 |
| 2 | DiffStore Green 実装（store/get/record_apply/undo） | 950→957 pass | 60分 |
| 3 | `LoopState` / `ToolContext` 拡張 | 957 維持 | 30分 |
| 4 | `CreateDiffTool` / `ApplyDiffTool` / `UndoDiffTool` 追加 | +3 統合テスト | 60分 |
| 5 | descriptions.rs にツール説明追加 | — | 10分 |
| 6 | CLAUDE.md 項目追記（167 番） | — | 10分 |
| 7 | clippy 通過確認 + commit | — | 20分 |

合計 220 分（3.7h）、目標 4h 内に収まる。

## リスク

| リスク | 影響 | 緩和策 |
|--------|------|--------|
| 1bit モデルが UUID 概念を理解できない | トークン削減効果なし | descriptions.rs に明示例を多数記載 + Lab で計測 |
| diff_id が応答間で漏れて参照エラー | apply_diff 失敗 | エラーメッセージで「create_diff から再取得」を促す |
| Mutex 競合（並列ツール実行時） | ロック待ちで遅延 | `parking_lot::Mutex` で軽量化、または RwLock 検討 |
| セッション再開時に diff_id が無効 | 再開後の参照失敗 | セッション再開時 `clear()` を呼び、明示リセット |

## 採否判定ゲート（再掲）

実装着手前に確認:

- [ ] **Lab v13 結果から 1bit モデルの平均 diff サイズが 150+ トークンであること**
- [ ] 同一セッション内で MultiEdit/file_write の連続呼出が 5+ 回観測されること
- [ ] Lab v13 ベースラインが安定 (variance < 0.01) であること

該当なしなら見送り。

## 関連

- 親計画: `.claude/plan/macos26-agent-learnings-v2.md`
- 既存類似機能: 項目111 MultiEdit、checkpoint manager（git stash CP）
- 後続候補: A. Multi-File Edit Cycle Detection、D. Model Fallback Chain
