# Drift Monitor (Weekly GC) — Zenn 適用案 Z-3 (Step 8)

## 1. 問題定義

Zenn dragon1208 Step 8 (verbatim): 週次 Codex タスクで drift 監視:
1. デッドコード検出 (ts-prune / knip)
2. ドキュメント整合性確認 (docs/ ↔ 実コード乖離検出)
3. 依存パッケージ更新 (npm outdated + minor 自動 PR)
4. 品質スコア更新 (quality/scores.md 最新 coverage 反映)

bonsai 現状 = analyst 評価で **15% カバー** = **最大の novel gap**。
Lab v17-v22 で 10 連続 REJECT 中、ドリフト蓄積進行中。

### bonsai 既存資産マップ

| 軸 | 現状 | 評価 |
|---|---|---|
| dead code 検出 | (なし、cargo +nightly udeps 未導入) | ❌ |
| docs ↔ code 整合 | (なし、項目 244 KG lint は KG のみ) | ❌ |
| cargo outdated | (なし、手動) | ❌ |
| coverage 更新 | (なし、Lab 結果は CLAUDE.md inline) | ❌ |
| 項目 240 archive automation | claudemd_archive.py = CLAUDE.md 行数 GC のみ | ⚠️ 部分 precedent |
| 項目 246 vault_lint | Vault content 4 軸 (5 軸目で 254 拡張) = docs GC partial 雛形 | ⚠️ 別軸 |

## 2. 設計判断 3 案

### 案 A: 単一 script
`scripts/drift_lint.rs` or `.py` で 4 軸統合。単一実行、結果 1 file。test 困難。

### 案 B: 4 軸独立 module
`tools/drift/{dead_code,docs_sync,outdated,coverage}.rs`。TDD strict 容易、構造複雑。

### 案 C: 段階的導入 (4 phase) ← **推奨**
Phase 1 dead code → Phase 2 docs↔code → Phase 3 outdated → Phase 4 coverage。
段階 commit、各 phase 独立価値、rollback 最大。

## 3. 5 軸比較

| 軸 | A (単一) | B (4 module) | C (4 phase) |
|---|---|---|---|
| Step 8 4 軸 cover | ★★★ | ★★★ | ★★★ |
| 実装工数 | ★★ | ★ | ★★★ |
| TDD strict 適合 | ★ | ★★★ | ★★★ |
| Rollback 容易性 | ★★ | ★★ | ★★★ |
| 段階的価値 delivery | ★ | ★★ | ★★★ |

**案 C = 15/15 ★ 推奨**。

## 4. 推奨案 C: 段階的導入

### 採用理由
1. 段階価値 delivery: Phase 1 即座 dead code 削減効果
2. TDD strict 厳密: 各 phase Red→Green→Refactor
3. Lab 完走 hook trigger 対応: 各 phase 独立 run 可能
4. rollback 最大: phase 別 revert

### 副次設計
- 出力先: `docs/quality/drift-YYYYMMDD.md` (Z-1 統合)
- trigger: 週次 cron (man-driven) + Lab 完走 hook (auto) 両対応
- Rust + Python ハイブリッド:
  - dead code: cargo +nightly udeps
  - docs↔code: Python (markdown parse 容易)
  - outdated: cargo outdated
  - coverage: cargo llvm-cov
- `scripts/drift/` directory 新設

## 5. TDD strict outline (各 phase 独立)

### Phase 1: dead code (~1.5h)
- Red: tests/drift/dead_code.rs で udeps run + unused dep 0 確証
- Green: scripts/drift/dead_code.py (~50 LOC)、nightly 未 install 時 graceful skip
- Refactor: rustdoc + README

### Phase 2: docs↔code (~2h)
- Red: 2 test (CLAUDE.md ↔ memory archive cross-ref + layer rules ↔ structural test WHITELIST)
- Green: scripts/drift/docs_sync.py (~80 LOC) markdown parse + cross-ref
- Refactor: error msg に修正方法 link (記事 Step 4 模倣)

### Phase 3: cargo outdated (~30 min)
- Red: tests/drift/outdated.rs で cargo outdated 実行可能性確認
- Green: scripts/drift/outdated.sh (~30 LOC)、major bump → warning、minor/patch → info
- Refactor: report formatting

### Phase 4: coverage (~1.5h)
- Red: tests/drift/coverage.rs で cargo llvm-cov 実行可能性
- Green: scripts/drift/coverage.sh (~80 LOC) → docs/quality/scores.md upsert
- Refactor: schema 確定 (Z-1 Phase 3 と統合)

### Phase 5: 統合 trigger (~30 min)
- scripts/drift/run_all.sh: 4 phase 順次 → drift-YYYYMMDD.md 統合出力
- 週次 cron entry sample 提供
- Lab 完走 hook 統合 (lab_v22_paired.sh から optional call)

## 6. Phase 6 smoke 検証基準

- G-DM-1: 各 phase 独立 run 成功、drift-report 出力
- G-DM-2: dead code phase で 0 件 baseline 確証
- G-DM-3: 試験的 unused dep 注入 → Phase 1 catch → revert
- G-DM-4: docs↔code 不整合注入 → Phase 2 fail → revert
- G-DM-5: 統合 run (run_all.sh) で 4 軸全 success

## 7. Rollback strategy

- 各 phase 独立 commit (4-5 commits)、問題時に該当 commit のみ revert
- scripts/drift/ directory 削除で完全 rollback
- docs/quality/drift-*.md は git-tracked、`git rm` で完全戻し
- production code 変更ゼロ (scripts + test のみ)

## 8. bonsai 既存資産との整合性

### 項目 240 archive automation
claudemd_archive.py precedent、「行数 GC」を「project drift GC」に一般化。

### 項目 244/246/251/254 lint pattern
Phase 2 docs_sync は KG / Vault と同じ「整合性検証」軸。
AuditAction::DriftLint variant 追加で audit_log 統合検討。

### Z-1 plan (docs/quality/scores.md)
Phase 4 で `docs/quality/scores.md` 自動更新、Z-1 Phase 3 雛形と統合。
table schema 確定で双方向参照。

### Z-4 plan (layer linter)
Phase 2 docs_sync で `module-layer-rules.md ↔ tests/structural/layer_rules.rs` cross-ref enforce。双方向 backup。

### Lab 多発との連動
Lab v17-v22 で 10 連続 REJECT、drift 蓄積中。
本案で Lab 完走後 auto trigger → drift report が次 Lab plan の input source。

## 9. 記事との対応関係

| Zenn Step 8 概念 | bonsai 実装 |
|---|---|
| dead code (ts-prune/knip) | Phase 1: cargo +nightly udeps + walk_src grep |
| docs ↔ code 整合 | Phase 2: markdown parse + cross-ref |
| 依存パッケージ更新 (npm outdated) | Phase 3: cargo outdated |
| 品質スコア更新 (quality/scores.md) | Phase 4: cargo llvm-cov → docs/quality/scores.md upsert |
| 週次 Codex タスク | 統合 trigger (cron + Lab hook、Phase 5) |

## 10. 工数見積もり

| Phase | 内容 | 工数 |
|---|---|---|
| 1 dead_code | cargo udeps + script | 1.5h |
| 2 docs_sync | markdown parse + cross-ref | 2h |
| 3 outdated | cargo outdated wrapper | 30 min |
| 4 coverage | llvm-cov → scores.md | 1.5h |
| 5 統合 trigger | run_all.sh + cron sample | 30 min |
| 6 Smoke | G-DM-1..5 実機 | 1h |
| **合計** | | **~7h** |

## 11. Open questions

1. trigger 頻度: 週次 cron vs Lab 完走後 vs PR 毎 (推奨: Lab 完走後)
2. nightly toolchain 依存: graceful skip + warning でカバー
3. docs/quality/scores.md schema: Lab version × module × coverage% の 3 軸 vs date × metric の 2 軸
4. cargo llvm-cov runtime: 全 test run 時間長、CI vs 週次 only
5. report 保存期間: 永続化 vs 直近 N 件のみ

## 12. 次手

1. user 承認後 Phase 1 (dead code、最小工数で最大 baseline insight)
2. Phase 2-4 順次 (~4-5h)
3. Phase 5 統合 trigger
4. Phase 6 smoke

## 13. 関連項目
- 項目 240 (archive automation、precedent)
- 項目 244/246/251/254 (lint pattern、Phase 2 並列)
- 推定項目番号: **項目 257** (Z-1=255 / Z-4=256 と連動)

## 14. 関連 plan / memory
- `memory/zenn_codex_harness_learnings.md` (記事 8 Step、本案 Z-3 = Step 8 最大 gap)
- `.claude/plan/agents-md-docs-knowledge-base.md` (Z-1、scores.md source)
- `.claude/plan/layer-architecture-linter.md` (Z-4、Phase 2 docs_sync cross-ref 対象)
