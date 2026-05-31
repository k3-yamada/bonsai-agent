# CLAUDE.md Curation Rules

> CLAUDE.md (Claude Code session auto-load convention file) の運用ルール集。
> 起源: `.claude/plan/claudemd-size-reduction-item-255-recreate.md` (2026-05-31 起票、Item 255 規模再現後の運用 SSOT 化)。
> 関連 enforcement: `scripts/drift/docs_sync.py` Z-3 第 3 軸 (`check_recent_items_section_count`)、`.claude/plan/claudemd-archive-policy.md` (項目 240、auto-flush ツール設計)。

---

## 1. 全体方針

- **目標サイズ**: ≤ 100 行 / ~14 KB (Zenn dragon1208 Codex Harness Step 1+2 推奨)
- **auto-load コスト意識**: CLAUDE.md は Claude Code session 起動時に毎回 context 展開される。bloat = 全 session の入力 token 増。
- **SSOT**: 詳細は `docs/` 配下と `memory/harness_patterns_archive.md` (project root **外部**) に分離、CLAUDE.md は索引 + 直近項目のみ。

---

## 2. Section 構成 (固定)

| section | 役割 | 削減対象? |
|---------|------|----------|
| プロジェクト概要 | 1bit Qwen3-8B / Mac M2 / test 数 | No (基礎情報) |
| ビルド・テスト・実行 refs | runbook.md link | No (1 行 link only) |
| アーキテクチャ + 主要トレイト refs | overview.md + module-layer-rules.md link | No |
| ハーネスパターン intro | archive 参照前提の宣言文 | No |
| カテゴリ索引 | archive 内項目を category 別に番号 list | 番号 list のみ、説明文禁止 |
| デフォルト化済み変異 | Lab ACCEPT で恒久適用された項目 | 1 行/項目、定量証拠付き |
| **直近 N 項目** | 最新の N 項目を 1 行サマリー | **★FIFO で N 維持必須** |
| Lab/テストパターン refs | docs/quality/ + runbook.md link | No |
| 注意事項 | 不可逆操作禁止ルール (clippy 巻き戻し等) | 重要、削除禁止 |

---

## 3. 「直近 N 項目」section 運用ルール (★最重要)

### 3.1 FIFO 規則

- **N+1 項目目を追加する時は、最古 1 項目を archive に flush**してから書き込む。
- archive = `~/.claude/projects/-Users-keizo-bonsai-agent/memory/harness_patterns_archive.md` (project root 外部)
- 現状 N = 5 (header「### 直近 5 項目」)、必要に応じて 3〜10 の範囲で変更可。

### 3.2 1 行サマリー template

```
- **NNN**: <絵文字> <タイトル句、太字> = <要約 200-400 字、改行禁止、文末に「次手」or「結果」を含む>
```

絵文字凡例:
- 🎉 = 完遂 (TDD strict 全 phase + 検証 ACCEPT)
- 🚨 = 重大 finding (REJECT / 仮説確定 / blocker)
- 🟡 = 進行中 / partial
- 🔵 = info-only / 調査結果
- ⚖️ = 設計判断 (Lab 不要)

例 (2026-05-31 時点の項目 268):
```
- **268**: 🚨 263 ratio tune (BUDGET=1) DEFINITIVE paired evidence REJECT (5/5 paired、mean Δ=-0.0683 / dz=-0.86) + 戦略的 implications (unpaired ACCEPT +9.5% 完全覆 / Phase 6 plan 廃案確定)、env default OFF、Phase 5 axis-priority prune infrastructure は future phase の base として維持
```

### 3.3 改行禁止理由

- 各 entry は **markdown bullet 1 行 = grep でき、line-wise diff で blame しやすい**。
- 多段落の mega-paragraph は archive verbatim が SSOT、CLAUDE.md は 1 行 pointer に徹する。

### 3.4 Section header 同期

- header「### 直近 N 項目」の N と section 内 `**NNN**:` 実数は **常に一致**させる。
- 不一致は `scripts/drift/docs_sync.py` の Z-3 第 3 軸で FAIL 検出 (CI 化候補)。
- N を変更したい時は header 編集 + flush/補充を atomic commit で行う。

---

## 4. カテゴリ索引運用

- 新規項目を archive に追加した時、適切な category 配下に項目番号を append。
- 説明文は CLAUDE.md に書かない (archive verbatim を参照する設計)。
- category 名そのものの新設は ADR (`docs/decisions/`) 起票推奨。

---

## 5. 検出 / Enforcement

| 軸 | 場所 | 動作 |
|----|------|------|
| 100 行 gate | `scripts/drift/docs_sync.py::check_claude_archive_crossref` | CLAUDE.md > 100 行 + 項目 0 件で FAIL (format drift) |
| Archive cross-ref | 同上 | CLAUDE.md 言及項目が archive に欠落で FAIL |
| **N 項目 header 整合** | 同 `check_recent_items_section_count` | header N ↔ 実数 mismatch で FAIL (本 doc §3.4 違反 catch) |
| Auto-flush | `scripts/claudemd_archive.py` (項目 240、Phase 2 Green 完遂・運用中、commit `eba1daf`) | `--mode {check,dry-run,apply}` CLI で N+1 検出 → 最古を archive append + CLAUDE.md rewrite + 任意 git commit |

---

## 6. アンチパターン (避けるべき行動)

| アンチパターン | 害 | 正しい行動 |
|---------------|-----|-----------|
| section 内の mega-paragraph 蓄積 | auto-load token 肥大、Z-3 linter FAIL | 1 行/項目に圧縮、詳細は archive |
| header「直近 5 項目」のまま 6+ 蓄積 | header と実態乖離、運用ルール崩壊 | 6 項目目追加時は最古を archive flush |
| 説明文を CLAUDE.md 側に書く | archive と二重情報、drift 元 | archive verbatim を SSOT に、CLAUDE.md は 1 行参照 |
| 注意事項 section の削除 | 不可逆操作の防御層消失 | 注意事項は **絶対に削除しない**、必要に応じて追加のみ |

---

## 7. 関連ドキュメント

- `.claude/plan/claudemd-size-reduction-item-255-recreate.md` — 今回の slimming 実行手順
- `.claude/plan/claudemd-archive-policy.md` — 自動化ツール設計 (項目 240)
- `scripts/drift/docs_sync.py` — Z-3 mechanical enforcement
- `~/.claude/projects/-Users-keizo-bonsai-agent/memory/harness_patterns_archive.md` — 項目 verbatim SSOT

---

## 8. 変更履歴

| 日付 | 変更 | commit |
|------|------|--------|
| 2026-05-31 | 初版起票 (Item 255 規模再現後の SSOT 化) | — |
