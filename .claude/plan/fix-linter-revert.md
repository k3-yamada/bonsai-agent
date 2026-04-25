# Implementation Plan: リンタ巻き戻し問題の根本対処

## 根本原因分析

### 確認された原因
1. **Lab実行中の`git stash push --include-untracked`** (`checkpoint.rs:127-132`)
   - `bonsai --lab`がバックグラウンドで実行中
   - 各実験イテレーションでチェックポイント作成時に`git stash push --include-untracked`
   - 新規作成した未追跡ファイル（subagent.rs等）がstashに吸い込まれて消失
   - **最も確度が高い原因**

2. **vcsddプラグインのPostToolUseフック** (`vcsdd-coherence-refresh.js`)
   - Write/Edit/MultiEdit後に`vcsdd-coherence-refresh.js`が実行される
   - Markdownスペックの整合性チェック — Rustソースには影響しないはず
   - auto-commitは`VCSDD_AUTO_COMMIT=true`時のみ — デフォルト無効

3. **omcプラグインのrustfmt定義** (`plugin-patterns/index.js`)
   - `.rs`→`rustfmt`のマッピングが定義されている
   - ただしhooks.jsonが存在せず、例示のみ（auto-format-on-save）の可能性

### 除外された原因
- git hooks: `.git/hooks/`にカスタムフックなし
- rustfmt.toml / clippy.toml: 存在しない
- rust-analyzer-lsp: `false`で無効化済み
- warp: 通知のみ、ファイル変更なし

## Task Type
- [x] Backend (Rust)

## Implementation Steps

### Step 1: checkpoint.rsの`--include-untracked`除去 (最重要)
- `git stash push --include-untracked`から`--include-untracked`を除去
- 追跡済みファイルの変更のみstashする（未追跡ファイルは無視）
- Lab実行中に開発作業しても干渉しなくなる

### Step 2: Lab実行時のgit操作ロック追加
- ファイルロック（`/tmp/bonsai-lab.lock`）でLab中のgit操作を排他制御
- Claude Codeからの新規ファイル追加とLabのstashが競合しない仕組み

### Step 3: テスト追加
- checkpoint.rsのgit stashテスト（--include-untracked除去の確認）
- 並行書き込み時のファイル保全テスト

### Step 4: （任意）vcsddプラグインの無効化検討
- `settings.local.json`にvcsddのPostToolUseフックを無効化する設定追加
- 現状は影響なしと判断するが、再発時の保険

## Key Files

| File | Operation | Description |
|------|-----------|-------------|
| `src/agent/checkpoint.rs:127` | Modify | `--include-untracked`を除去 |
| `src/agent/experiment.rs` | Modify | Lab実行前のロックファイル取得追加 |
| `src/agent/checkpoint.rs` (tests) | Add | stashが未追跡ファイルに影響しないテスト |

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| `--include-untracked`除去で未追跡の設定ファイルがstashされない | 通常の開発フローでは問題なし。Labの実験結果は追跡済みファイルのみ |
| Lab実行中にgit競合 | ロックファイルでLab中のstash操作を排他制御 |
| vcsddフックが再発 | 再発時にsettings.local.jsonで無効化 |

## SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: N/A（Codex使用量制限中）
- GEMINI_SESSION: N/A
