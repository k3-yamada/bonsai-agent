# bonsai-agent docs/ INDEX

このディレクトリは bonsai-agent project の知識ベース (Zenn Codex Harness Engineering 適用案 Z-1、項目 255)。
Single Source of Truth として CLAUDE.md (Claude Code エントリ) + memory/ (personal/session memory) + .claude/plan/ (TDD outline) と協調動作。

## ナビゲーション

### アーキテクチャ
- [docs/architecture/overview.md](architecture/overview.md) — module 階層 + 主要 trait + 設計原則 (Z-1 Phase 2 で CLAUDE.md から分離)
- [docs/architecture/module-layer-rules.md](architecture/module-layer-rules.md) — module layer 順 (Z-4 layer linter の rule source)

### 品質
- [docs/quality/lab-history.md](quality/lab-history.md) — Lab 実機テスト結果 (v1-v22 履歴、Z-1 Phase 3 で CLAUDE.md から分離)
- [docs/quality/scores.md](quality/scores.md) — 定量 quality scores (coverage / clippy / Lab、Z-3 drift monitor Phase 4 で自動更新候補)

### 実行
- [docs/execution/runbook.md](execution/runbook.md) — ビルド・テストコマンド + Lab 起動手順 (Z-1 Phase 4 で CLAUDE.md から分離)

### 設計判断 (ADR)
- [docs/decisions/](decisions/) — ADR (Architecture Decision Records)、Z-1 Phase 6 で起票予定

### 既存 docs
- [docs/DESIGN_SPEC.md](DESIGN_SPEC.md) — Bonsai-8B agent 設計仕様 (オリジナル設計書)
- [docs/THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md) — 第三者ライブラリのライセンス

## 外部 reference (project root の memory/ + .claude/plan/)

### memory/ (personal/session memory、project root の `/Users/keizo/.claude/projects/-Users-keizo-bonsai-agent/memory/`)
- `MEMORY.md` — 全 memory ファイルのインデックス
- `harness_patterns_archive.md` — ハーネスパターン項目 1-252 verbatim アーカイブ
- `qiita_2do_brain_learnings.md` — Qiita YushiYamamoto 記事 (2do BRAIN) 深掘り
- `zenn_codex_harness_learnings.md` — Zenn dragon1208 記事 (Codex Harness Engineering) 深掘り
- `session_2026_*_handoff.md` — session 引継ぎ documents

### .claude/plan/ (project root の `.claude/plan/`)
- TDD strict 3-phase plan documents (現在 25 件)
- 実装完遂 plan は `docs/decisions/ADR-NNN.md` 化候補 (Z-1 Phase 6)

## 絶対に守るルール (placeholder、Z-1 Phase 5 で確定)

詳細は CLAUDE.md 末尾「注意事項」セクション参照。Phase 5 で本 INDEX に統合予定。

## 関連項目

- 項目 255: Z-1 = docs/ 整備 (本 INDEX.md は Phase 1 成果物)
- 項目 256: Z-4 = layer linter (docs/architecture/module-layer-rules.md と連動)
- 項目 257: Z-3 = drift monitor (docs/quality/ + docs/ 整合性検証と連動)
