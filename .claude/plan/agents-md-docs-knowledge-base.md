# AGENTS.md + docs/ ナレッジベース整備 (Zenn 適用案 Z-1)

## 1. 問題定義

Zenn dragon1208/66547a030c0236 記事 Step 1+2: AGENTS.md は 100 行以内の目次、詳細は docs/、「絶対ルール」のみ直接記載。

bonsai 現状 gap:
- CLAUDE.md 202 行 (推奨 100 行の 2x)
- docs/INDEX.md 不在、docs/ ほぼ空 (DESIGN_SPEC.md + THIRD_PARTY_LICENSES.md のみ)
- architecture/quality/runbook が CLAUDE.md inline
- ADR 不在 (.claude/plan/ + memory/ 分散)

## 2. 設計判断 3 案

### 案 A: radical (CLAUDE.md 全面分離、~6h)
全詳細 docs/ migrate、CLAUDE.md = 100 行 index のみ。Zenn 完全準拠、ただし大規模 reorganize。

### 案 B: gradual (段階的分離、~3.5h) ← **推奨**
Phase 1-5 で順次分離、各 phase 独立 commit で rollback 容易。

### 案 C: hybrid (AGENTS.md + CLAUDE.md 共存、~4-5h)
両 file 維持で Codex/Claude Code 両対応、ただし Single Source of Truth と矛盾。

## 3. 5 軸比較

| 軸 | A | B | C |
|---|---|---|---|
| Zenn 原則準拠 | ★★★ | ★★ | ★ |
| 実装工数 | ★ | ★★★ | ★★ |
| Rollback 容易性 | ★ | ★★★ | ★★ |
| 既存資産活用 | ★★ | ★★★ | ★★ |
| context 削減 | ★★★ | ★★ | ★ |

**案 B = 14/15 ★ 推奨** (gradual + 段階 commit + memory/ 維持)。

## 4. 推奨案 B: 段階的分離

### 採用理由
1. 各 phase 独立 commit、rollback 最大化
2. 既存 memory/ + .claude/plan/ + CLAUDE.md の関係維持
3. 段階検証で Claude Code 取込挙動を観測可能
4. 項目 240 claudemd_archive.py の section 抽出を再利用

### 副次判断
- AGENTS.md 新設は Phase 6+ で検討 (Codex 実使用予定確認後)
- docs/INDEX.md が事実上の AGENTS.md template 役割
- memory/ は移行対象外 (personal/session 永続 vs production docs)

## 5. TDD strict 5-phase outline

### Phase 1: docs/INDEX.md 新規作成 (~30 min)
- `docs/INDEX.md` 新規、既存 docs + memory/ + .claude/plan/ への navigation
- 「絶対に守るルール」placeholder (Phase 5 で確定)

### Phase 2: docs/architecture/ 分離 (~1h)
- `docs/architecture/overview.md` 新規 (CLAUDE.md「アーキテクチャ」verbatim 移行)
- `docs/architecture/module-layer-rules.md` 新規 (Z-4 layer linter rule source、`db < observability < safety < memory < knowledge < runtime < tools < agent < main`)
- CLAUDE.md「アーキテクチャ」セクション → 1 行 link 化

### Phase 3: docs/quality/lab-history.md 分離 (~1h)
- `docs/quality/lab-history.md` 新規 (CLAUDE.md「Lab 実機テスト結果」table verbatim 移行)
- `docs/quality/scores.md` 雛形 (Z-5 自動生成連動)
- CLAUDE.md → 1 行 link 化

### Phase 4: docs/execution/runbook.md 分離 (~30 min)
- `docs/execution/runbook.md` 新規 (CLAUDE.md「ビルド・テストコマンド」verbatim 移行)
- CLAUDE.md → 1 行 link 化

### Phase 5: CLAUDE.md final reduce (~30 min)
- 「直近 5 項目」維持
- 「注意事項」→「絶対に守るルール」改名、末尾配置
- 目標 100-150 行 index 化、`wc -l CLAUDE.md` で確認

## 6. Phase 6: ADR 整備 (別 plan、本 plan 範囲外)
過去 CLAUDE.md 項目 1-254 から 30-40 件を ADR 化 (~6-8h)。

## 7. Smoke 検証基準

- G-AG-1: Claude Code session で docs/INDEX.md 自動 load 確認
- G-AG-2: cargo test --lib 1348 passed 維持 (production touch ゼロなので退行不可能)
- G-AG-3: Phase 5 完了で `wc -l CLAUDE.md` ≤ 150 行
- G-AG-4: `grep -r "→ docs/" CLAUDE.md` で dead link ゼロ

## 8. Rollback strategy
- 各 Phase 独立 commit (5 commits)、問題時に該当 commit のみ revert
- docs/ 追加は完全 additive、削除は `git rm` で完全戻し

## 9. bonsai 既存資産との整合性

### 項目 240 archive automation
section 抽出ロジック (`scripts/claudemd_archive.py`) を docs migration に再利用。

### 項目 254 vault_lint
docs/ markdown も vault_lint 検査対象拡張検討 (Phase 5+ 別 plan)。

### memory/ との関係
docs/ = production project docs、memory/ = personal/session memory。orthogonal。

### Z-4 layer linter (案 Z-4 plan)
Phase 2 の `docs/architecture/module-layer-rules.md` が Z-4 linter の rule source。

## 10. 記事との対応関係

| Zenn 概念 | bonsai 実装 |
|---|---|
| AGENTS.md (100 行 index) | CLAUDE.md final reduce (Phase 5) |
| docs/INDEX.md | Phase 1 |
| docs/architecture/overview.md | Phase 2 |
| docs/architecture/dependency-rules.md | Phase 2 (Z-4 rule source) |
| docs/quality/scores.md | Phase 3 (Z-5 自動生成統合) |
| docs/execution/runbook.md | Phase 4 |
| docs/decisions/ADR-NNN.md | Phase 6 (別 plan) |
| 絶対ルール明示 | Phase 5 |

## 11. 工数見積もり

| Phase | 内容 | 工数 |
|---|---|---|
| 1 | docs/INDEX.md | 30 min |
| 2 | architecture | 1h |
| 3 | quality | 1h |
| 4 | execution | 30 min |
| 5 | CLAUDE.md reduce | 30 min |
| **合計** | | **~3.5h** |

## 12. Open questions

1. AGENTS.md 新設タイミング: Codex 実使用予定確認後
2. memory/ docs 統合: 維持 (Single Source of Truth は分離運用)
3. 絶対ルール cardinality: 5-10 件に絞れるか
4. ADR 起票範囲: 30-40 件 (Lab 結果除外、設計判断のみ)
5. link integrity 自動 check: Z-4 docs-lint と統合

## 13. 次手

1. user 承認後 Phase 1 着手
2. Phase 2-4 順次 (~3h)
3. Phase 5 で CLAUDE.md 100-150 行 reduce 確認
4. Phase 6 = ADR plan 別途起票

## 14. 関連項目
- 項目 240 (archive automation, activator)
- 項目 246/251/254 (vault_lint extension candidate)
- 推定項目番号: **項目 255**

## 15. 関連 plan / memory
- `memory/zenn_codex_harness_learnings.md` (記事 8 Step + 5 案)
- `.claude/plan/vault-status-state-machine.md` (項目 254、orthogonal)
- `.claude/plan/vault-ingest-compile-separation.md` (案 I-2、orthogonal)
