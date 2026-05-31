# CLAUDE.md Size Reduction Plan — Item 255 規模再現 (案 A)

**起票日**: 2026-05-31
**起票背景**: CLAUDE.md が項目 255 直後 (88 行 / 13.5 KB) → 現在 116 行 / 53 KB へ約 4 倍肥大、Z-3 drift linter 100 行 gate 違反状態
**選定案**: 案 A (Item 255 規模再現 + 直近 5 項目 section の真の 5 項目化)
**Trade-off 認識**: 案 B (50 行 aggressive) は採用せず — カテゴリ索引/注意事項の即時可視性を維持
**関連既存 plan**: `claudemd-archive-policy.md` (項目 240、自動化ツール設計、~3-4h dev) — **本 plan は補完 (今回固有の一発 trim 実行手順、~30 min)**。両者は重複ではなく、本 plan = 即時実行、既存 plan = 再発防止ツール構築。

---

## 1. 現状診断

| 指標 | 現在 | 目標 | Δ |
|------|------|------|---|
| 行数 | 116 | ~85 | -27% |
| バイト数 | 53,156 | ~14,000 | -74% |
| 「直近 5 項目」section 実数 | 21 項目 (247-268) | 5 項目 (264-268) | -16 |
| Z-3 drift linter 100 行 gate | **FAIL** | PASS | — |
| auto-load token 概算 | ~13K | ~3.5K | -73% |

### 1.1 肥大部位 (line-level breakdown)

| section | 行範囲 | 行数 | 状態 |
|---------|--------|------|------|
| プロジェクト概要 | 1-11 | 11 | 維持 |
| ビルド/テスト/アーキ refs | 12-19 | 8 | 維持 |
| カテゴリ索引 | 21-52 | 32 | 維持 |
| デフォルト化済み変異 | 54-59 | 6 | 維持 |
| **直近 5 項目 (実 21 項目)** | **61-98** | **38** | **★削減対象** |
| Lab/テスト refs | 100-107 | 8 | 維持 |
| 注意事項 | 109-116 | 8 | 維持 |

### 1.2 Archive 状態 (Phase 1 不要の確証)

`harness_patterns_archive.md` grep 結果: **項目 247-268 全 22 項目 verbatim 既収載**。
→ archive 移行作業は **既完了**、本計画は CLAUDE.md trim のみで完結。

---

## 2. Phase 構成

### Phase 1: archive sync (SKIP)

理由: archive grep で 247-268 全 verbatim 確認済 (項目 259/267/268 等の archive sync 作業で完了済)。

### Phase 2: CLAUDE.md trim (本体作業)

**作業**: 「直近 5 項目」section (現 61-98 行、21 項目) を真の 5 項目 (264-268) のみ + 各 1 行サマリーに圧縮。

**圧縮後 entry 例** (目安各 200-400 字):
- **264**: 🚨 T6 案 D-2 (MEMORY_AUG) Phase 2a 完遂 + 重大 finding (AUGMENT×BUDGET destructive interaction -19.1%、H-A4 noise / hidden binary diff 仮説確定、全 smoke で実 prune 不発火)
- **265**: 🎉 max_context_tokens 縮小 (smoke/env override) Phase 1-3 完遂 + G-MCT2 構造的 finding 確定 (smoke k=3 baseline は独立 session で context reset、level1 never fires)
- **266**: 🚨 D-2 (MEMORY_AUG) paired evidence DEFINITIVE REJECT (Cohen's dz=-10.60、4 paired 全 B<A by 0.12-0.15、env default OFF)
- **267**: 🎉 D-2 case B 削除完遂 (~-466 LOC、1378→1372 passed、paired-evidence-driven cleanup pattern 確立)
- **268**: 🚨 263 ratio tune (BUDGET=1) paired evidence REJECT (Cohen's dz=-0.86、mean Δ=-0.0683、unpaired ACCEPT +9.5% 完全覆、env default OFF)

**header 文言**: 「### 直近 5 項目 (詳細は archive 参照)」はそのまま (header 文言と実態整合)。

**運用ルール追記** (1-2 行):
> 6 項目目追加時は最古 1 件を archive 移行 (FIFO)、本 section は常に直近 5 項目に保つ。

### Phase 3: 検証

- `wc -l CLAUDE.md` → ≤ 100 行確認
- `scripts/drift/run_lint.sh` → All PASS (100 行 gate + archive cross-ref Phase 2)
- production code touch ゼロ → cargo test 退行不可能 (実行省略可、ただし念のため `cargo test --lib --quiet` で 1372 retention 確証推奨)
- diff レビュー: 削除のみ・追加なし (mega-paragraph → 1 行) を視覚確認

### Phase 4: コミット

単発 commit (atomic):
```
docs(claudemd): 直近 5 項目 section を 264-268 1 行サマリーに圧縮 (Item 255 規模再現 / 案 A)

- 116→~85 行 (-27%) / 53KB→~14KB (-74%)
- 247-263 mega-paragraph 17 件は harness_patterns_archive.md verbatim 既収載 (移行作業不要)
- Z-3 drift linter 100 行 gate クリア
- 「直近 5 項目」header と実態 (21 項目蓄積) の乖離解消
- 運用ルール: 6 項目目追加時は FIFO で最古を archive flush
```

---

## 3. ACCEPT 条件

- [ ] `wc -l CLAUDE.md` ≤ 100 (目標 ~85)
- [ ] `scripts/drift/run_lint.sh` All PASS
- [ ] 「直近 5 項目」section が真の 5 項目 (264-268) のみ
- [ ] 各 entry が 1 行 (改行なし、200-400 字)
- [ ] カテゴリ索引・デフォルト化済み変異・注意事項は完全維持
- [ ] archive verbatim entry の verbatim 性は touch せず (削除のみ)

---

## 4. リスク + Rollback

| リスク | 対策 |
|--------|------|
| 圧縮しすぎで重要情報損失 | archive verbatim 完備 (247-268 全件)、参照 hop は 1 段増だが情報損失ゼロ |
| 1 行サマリー精度不足 | commit 後 user 確認、不足あれば追加 commit で fine-tune |
| drift linter regression | Phase 3 で run_lint.sh 実行確認、FAIL 時 rollback |
| 後続 session で再肥大 | 運用ルール明文化 + `claudemd-archive-policy.md` (項目 240) ツール完成で機械的 enforcement |

**Rollback**: `git revert <commit>` 1 コマンド (single atomic commit のため)。

---

## 5. 後続 follow-up (本計画 scope 外)

- (任意) `claudemd-archive-policy.md` (項目 240) の `scripts/claudemd_archive.py` 完成 → 6 項目目 detect 時 auto-flush 化
- (任意) drift linter に「直近 5 項目」section 実数 vs header 数の整合 check 軸を追加 (Z-3 拡張軸)
- (任意) `docs/maintenance/claudemd-curation.md` 新設で運用ルール詳細化 (FIFO 規則 + 1 行サマリー template + section header 同期 rule)

---

## 6. 工数見積

| Phase | 工数 |
|-------|------|
| Phase 1 (archive sync) | **0h** (既完了) |
| Phase 2 (CLAUDE.md trim) | ~20 min (5 entry × 4 min for 1 行サマリー作成) |
| Phase 3 (検証) | ~5 min |
| Phase 4 (commit) | ~2 min |
| **合計** | **~30 min** |
