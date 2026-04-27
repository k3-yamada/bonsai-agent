# Multi-File Edit Cycle Detection 実装スケルトン

**Source:** `macos26-agent-learnings-v2.md` ★★ 採用候補 A
**Date:** 2026-04-27
**Status:** 設計（Lab v13 完了後の実装候補）

---

## 動機

**問題**: 既存 `LoopDetector` は同一 `tool_call hash` の連続検出のみ。**複数ファイルを交互に編集する**サイクル（A 編集 → B 編集 → A 編集 → B 編集 ...）を検出できない。

macOS26/Agent の `detectEditCycle` は過去 6 ターンで編集したファイルを window で記録し、2-3 ファイルが交互に出現したら nudge を出す。1bit モデルが「片側を直すと反対側が壊れる」ループに陥るのを防ぐ。

**ゴール**: 既存 `LoopDetector` の補完として、**ファイル間交互編集**を独立に検出。

## 設計

### モジュール配置

```
src/agent/
├── error_recovery.rs    ← 既存、MultiFileEditCycleDetector を追加
└── ...
```

### 公開 API

```rust
// src/agent/error_recovery.rs に追加

use std::collections::{HashMap, VecDeque};

/// 複数ファイル間の編集サイクル検出器（macOS26/Agent ★★ 候補 A）
///
/// 過去 N ターンで編集したファイルパスを window で記録、2-3 ファイルが
/// それぞれ 2 回以上出現すると cycle として検出する。
///
/// 既存 `LoopDetector` は同一 tool_call hash のみ検出するため、
/// 「A 編集 → B 編集 → A 編集 → B 編集」のような交互編集は検出されない。
/// この検出器がそのギャップを埋める。
#[derive(Debug)]
pub struct MultiFileEditCycleDetector {
    /// 過去のファイルパス（古い順）
    recent_paths: VecDeque<String>,
    /// window サイズ（macOS26/Agent と同じデフォルト 6）
    window: usize,
    /// 最小 cycle 検出ファイル数
    min_files: usize,
    /// 最大 cycle 検出ファイル数（広すぎると誤検出）
    max_files: usize,
}

impl MultiFileEditCycleDetector {
    pub fn new(window: usize) -> Self {
        Self {
            recent_paths: VecDeque::with_capacity(window),
            window,
            min_files: 2,
            max_files: 3,
        }
    }

    /// パスを記録し、cycle 検出時 nudge 文字列を返す
    pub fn record_and_check(&mut self, path: &str) -> Option<String> {
        // window 維持
        self.recent_paths.push_back(path.to_string());
        if self.recent_paths.len() > self.window {
            self.recent_paths.pop_front();
        }
        // 最低 4 ターン経過後に判定
        if self.recent_paths.len() < 4 {
            return None;
        }
        // ユニークファイル数チェック
        let mut counts: HashMap<&String, usize> = HashMap::new();
        for p in &self.recent_paths {
            *counts.entry(p).or_default() += 1;
        }
        let unique = counts.len();
        if unique < self.min_files || unique > self.max_files {
            return None;
        }
        // 全てが 2 回以上出現か
        if !counts.values().all(|&c| c >= 2) {
            return None;
        }
        // nudge 構築
        let files: Vec<&String> = counts.keys().copied().collect();
        let names: Vec<String> = files.iter()
            .map(|p| std::path::Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(p)
                .to_string())
            .collect();
        let nudge = format!(
            "🔄 ファイル {} を交互に編集しています — 進捗なし。\n\
             ステップバック: 全ファイルを再読 → 全体計画 → 一括編集してください。\n\
             まだ衝突する場合、より大きな構造変更が必要かもしれません。",
            names.join(", ")
        );
        // 1 回 nudge を出したら window をクリア（連続 nudge 防止）
        self.recent_paths.clear();
        Some(nudge)
    }

    pub fn reset(&mut self) {
        self.recent_paths.clear();
    }
}

impl Default for MultiFileEditCycleDetector {
    fn default() -> Self {
        Self::new(6)
    }
}
```

### 統合ポイント

#### `LoopState` 拡張（既存）

```rust
// src/agent/agent_loop/state.rs

pub struct LoopState<'a> {
    // ... 既存フィールド ...
    pub cycle_detector: MultiFileEditCycleDetector,
}

impl<'a> LoopState<'a> {
    pub fn new(advisor: AdvisorConfig) -> Self {
        Self {
            // ... 既存初期化 ...
            cycle_detector: MultiFileEditCycleDetector::default(),
        }
    }
}
```

#### `outcome::handle_outcome` での呼出

```rust
// src/agent/agent_loop/outcome.rs

use super::super::error_recovery::MultiFileEditCycleDetector;

pub(super) fn handle_outcome(
    outcome: StepOutcome,
    session: &mut Session,
    state: &mut LoopState,
    // ...
) -> OutcomeAction {
    match outcome {
        StepOutcome::Continue(step_tools) => {
            // ... 既存処理 ...

            // ファイル編集ツールが呼ばれた場合、cycle 検出
            for tool_name in &step_tools {
                if matches!(tool_name.as_str(),
                    "file_write" | "file_edit" | "multi_edit") {
                    // 直近のツール実行結果から path を抽出
                    if let Some(path) = extract_path_from_last_tool_result(session) {
                        if let Some(nudge) = state.cycle_detector.record_and_check(&path) {
                            session.add_message(Message::system(&nudge));
                            log_event(LogLevel::Warn, "edit_cycle", "Multi-file cycle detected");
                        }
                    }
                }
            }

            OutcomeAction::Continue
        }
        // ...
    }
}

fn extract_path_from_last_tool_result(session: &Session) -> Option<String> {
    // 直近 Tool ロールメッセージの content から path 抽出
    // 既存 file.rs の出力フォーマットに合わせる: "file_write: /path/to/file"
    session.messages.iter().rev()
        .find(|m| m.role == Role::Tool)
        .and_then(|m| {
            // 簡易抽出: ":" の後の最初のパス文字列
            m.content.split(':').nth(1).map(|s| s.trim().to_string())
        })
}
```

### TDD 計画（推定 4 テスト）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_cycle_detector_no_cycle_under_4_steps() {
        let mut det = MultiFileEditCycleDetector::new(6);
        assert!(det.record_and_check("a.rs").is_none());
        assert!(det.record_and_check("b.rs").is_none());
        assert!(det.record_and_check("a.rs").is_none()); // 3 turn
    }

    #[test]
    fn t_cycle_detector_detects_alternating() {
        let mut det = MultiFileEditCycleDetector::new(6);
        det.record_and_check("a.rs");
        det.record_and_check("b.rs");
        det.record_and_check("a.rs");
        let nudge = det.record_and_check("b.rs"); // 4 turn, A=2 B=2
        assert!(nudge.is_some());
        assert!(nudge.unwrap().contains("a.rs"));
    }

    #[test]
    fn t_cycle_detector_skips_too_many_files() {
        let mut det = MultiFileEditCycleDetector::new(6);
        det.record_and_check("a.rs");
        det.record_and_check("b.rs");
        det.record_and_check("c.rs");
        det.record_and_check("d.rs");  // 4 unique > max_files=3
        assert!(det.record_and_check("a.rs").is_none());
    }

    #[test]
    fn t_cycle_detector_resets_after_nudge() {
        let mut det = MultiFileEditCycleDetector::new(6);
        det.record_and_check("a.rs");
        det.record_and_check("b.rs");
        det.record_and_check("a.rs");
        let _ = det.record_and_check("b.rs");
        // window cleared、次の判定は 4 ターン後まで None
        assert!(det.record_and_check("a.rs").is_none());
    }
}
```

## 実装手順（推定 1h）

| Step | 内容 | テスト | 工数 |
|---|---|---|---|
| 1 | `error_recovery.rs` に MultiFileEditCycleDetector 追加 + 4 テスト Red | +4 失敗 | 15分 |
| 2 | Green 実装 | 950→954 pass | 20分 |
| 3 | `LoopState` 統合 | 954 維持 | 10分 |
| 4 | `outcome.rs::handle_outcome` で cycle 検出呼出 | 954 維持 | 10分 |
| 5 | CLAUDE.md 項目追記 + commit | — | 5分 |

合計 60 分。

## リスク

| リスク | 対策 |
|---|---|
| 短い回答多発で false positive | min_files=2 制約 + 4 ターン待機 |
| nudge 後すぐ再 cycle | window クリアで連続 nudge 防止 |
| パス抽出失敗 | `extract_path_from_last_tool_result` で None 時 skip |

## 採否判定ゲート

実装前に確認:

- [ ] Lab v13 で「同ファイル交互編集 REJECT 変異」が観測されること
- [ ] 既存 `LoopDetector` で十分でないことを実例で確認

該当なしなら見送り（YAGNI）。

## 関連
- 親計画: `.claude/plan/macos26-agent-learnings-v2.md`
- 既存類似機能: 項目5 LoopDetector、項目42 FileStuckGuard
- 並行: `.claude/plan/diffstore-rust-impl.md`（候補 C）
