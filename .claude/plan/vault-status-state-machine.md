# Vault frontmatter status state machine + vault_lint 5 軸目 (2do BRAIN 適用案 I-3)

## 1. 問題定義

### Qiita 2do BRAIN 記事の状態管理 (verbatim)

**Frontmatter `status` state machine**:
```yaml
---
updated_at: YYYY-MM-DD
sources: [01_raw/filename.pdf]
status: reviewed
---
```

**Obsidian Dataview による未レビュー検出**:
```dataview
TABLE updated_at, status
FROM "02_wiki"
WHERE status != "reviewed"
SORT updated_at ASC
```
> 「AI が生成したまま、人間がまだレビューしていないページ一覧」を動的取得・監視できます。

### bonsai-agent 現状の gap

- 項目 246 で vault_lint.rs に 4 軸検出を実装済 (duplicates / stale / orphan / case_variant)
- ただし frontmatter `status` field が存在せず、「未レビュー」「draft 老化」の状態軸 (status state) が欠落
- Obsidian Dataview 等価機能 (動的 CLI 監視) 無し
- 項目 251 で bail branch test を完備、本 plan は **既存 vault_lint 5 軸目追加** の incremental 拡張で価値最大

## 2. 設計判断 3 案

### 案 A: vault_lint.rs に 5 軸目 `unreviewed_too_long_days` 追加 (state machine 無し)

**実装方針**:
- `src/knowledge/vault_lint.rs` に 5 軸目検出 logic 追加
  - `status` frontmatter parse (記事と同じ列挙: `draft | reviewed | stale`)
  - 5 軸目: `status != "reviewed"` かつ `updated_at > N days` のエントリ列挙
- env `BONSAI_VAULT_UNREVIEWED_DAYS` (default 14 日) で閾値設定
- 既存 status 無しエントリは「reviewed として扱う」(backward compat) or 「draft として扱う」(strict mode、env 切替) を選択

**特徴**:
- vault_lint への増分のみ、Vault::write 等の core 機構変更なし
- env で挙動切替可能、既存 entry 影響ゼロ (default behavior preserved)

**欠点**:
- 「state 遷移」を enforce しない (新規 entry が `status: draft` を自動付与する仕組み無し)
- Vault::write 経路で status 付与は別 plan (案 I-2 で対処)

### 案 B: Vault::write 新規 entry に `status: draft` 強制設定、明示 `--compile <path>` で reviewed 遷移

**実装方針**:
- `src/knowledge/vault.rs::Vault::write` 内で新規 entry に frontmatter `status: draft` 強制付与
- `--compile <path>` CLI で `status: reviewed` に状態遷移 (案 I-2 と統合)
- 既存 entry は migration script で `status: reviewed` を付与 (one-shot)

**特徴**:
- 状態遷移を構造的に enforce
- 案 I-2 の compile workflow と直接統合

**欠点**:
- migration 影響大 (既存 vault entry 全件に frontmatter 注入)
- 案 I-2 (Ingest/Compile 分離) が前提、本 plan 単独では実装不能

### 案 C: 案 A + 案 B 統合 (5 軸目検出 + state machine、TDD strict 3-phase で段階的導入)

- Phase 1-3: 案 A 実装 (5 軸目検出のみ、status 無し entry は reviewed として扱う)
- Phase 4 wiring: 既存 entry migration (one-shot script、`status: reviewed` 一括付与)
- Phase 5 案 B 統合: Vault::write での `status: draft` 強制、案 I-2 compile workflow と連動
- 段階的導入で migration cost を分散

## 3. 5 軸比較

| 軸 | 案 A (lint 5 軸目) | 案 B (state machine) | 案 C (統合) |
|---|---|---|---|
| 既存資産活用度 | ★★★ 項目 246/251 vault_lint への増分のみ | ★★ Vault::write 全変更、案 I-2 前提 | ★★★ 案 A の上に段階拡張 |
| 実装工数 | ★★★ 1 軸追加 + env getter (~100 LOC) | ★ Vault::write 改修 + migration script (~300 LOC) | ★★ 案 A + 段階拡張 (~250 LOC) |
| TDD test 設計容易性 | ★★★ vault_lint.rs に test 3-4 件追加 | ★★ Vault::write 既存 test 全件影響 | ★★★ 案 A 部分は ★★★ |
| 既存機構整合性 | ★★★ vault_lint pattern 完全踏襲、項目 251 bail mode と整合 | ★★ Vault::write 経路の全 caller 影響 | ★★★ 段階導入 |
| rollback 容易性 | ★★★ env unset で完全 no-op (5 軸目スキップ) | ★★ migration revert 困難 (frontmatter 削除必要) | ★★★ 案 A 部分は ★★★ |

**総合**:
- 案 A = 15/15 ★ ← **推奨**
- 案 B = 9/15 ★ (案 I-2 完了後に検討)
- 案 C = 14/15 ★ (将来移行、案 I-2 完成後)

## 4. 推奨案: 案 A (vault_lint 5 軸目検出)

### 採用理由

1. **即効性最大**: 項目 246/251 で確立済 vault_lint pattern への 1 軸追加で、既存 4 軸と並列に動作 (LintReport.unreviewed_aged: Vec<...> field 追加のみ)。
2. **既存 entry 影響ゼロ**: status 無し entry は `reviewed` として扱う backward-compat default、env opt-in (`BONSAI_VAULT_UNREVIEWED_DAYS=14`) で active 化。
3. **TDD strict 3-phase の test 設計が容易**: vault_lint.rs::tests 既存パターン (status field 注入 fixture 3-4 件) を流用。
4. **案 I-2 と orthogonal**: I-2 (Ingest/Compile 分離) が完成後に Phase 5 で案 C 統合可能 (本 plan は I-2 を前提としない)。

### 副次設計判断

- **status field 列挙**: `draft | reviewed | stale` (記事と同じ)。未知値は parse error → strict mode で bail
- **default 閾値**: 14 日 (項目 246 stale_days default と同じ)、env で 1-90 日範囲調整
- **未 frontmatter entry の扱い**: backward compat default = `status: reviewed` として扱う (= 未レビュー検出から除外)、env `BONSAI_VAULT_STRICT_STATUS=1` で `status: draft` として扱う (strict mode)
- **5 軸目の出力先**: 既存 LintReport struct に `pub unreviewed_aged: Vec<UnreviewedEntry>` field 追加、`UnreviewedEntry { path, status, updated_at, age_days }`

## 5. TDD strict 3-phase outline

### Phase 1: Red (4 test)

1. `t_status_reviewed_recent_no_unreviewed_detect` (status: reviewed + 最新 → 検出ゼロ)
2. `t_status_draft_aged_30d_detects_unreviewed` (status: draft + 30 日経過 → 検出 1 件)
3. `t_vault_unreviewed_days_env_range_validation` (env getter range 1-90 fallback)
4. `t_strict_status_mode_treats_no_frontmatter_as_draft` (strict mode で status 無し entry を draft 扱い)

**Phase 1 Red 検証**: 4 test 全 fail (5 軸目未実装、env getter 未実装)。

### Phase 2: Green (本実装)

**`src/config.rs`** (~20 LOC):
- `pub fn vault_unreviewed_days() -> i64` (range 1..=90、default 14)
- `pub fn is_vault_strict_status_enabled() -> bool` (env-gated default OFF)

**`src/knowledge/vault_lint.rs`** (~80 LOC):
- `LintReport` struct に `unreviewed_aged: Vec<UnreviewedEntry>` field 追加
- `UnreviewedEntry { path, status, updated_at, age_days }` struct 新規
- frontmatter parse 拡張 (既存 stale_days 検出のための `updated_at` parse を流用、`status` field 追加)
- 5 軸目検出 logic 追加

**Phase 2 Green 検証**: 4 test 全 PASS + 既存 vault_lint test 全 PASS = 1340 → 1344 (+4) (退行ゼロ)。

### Phase 3: Refactor

- rustdoc 拡充 (5 軸目の意味、env パラメータ説明)
- frontmatter parse helper の共通化 (項目 246 既存 stale 検出と統合)
- clippy clean / fmt clean

## 6. Phase 4 wiring 設計

**`src/main.rs::handle_lab_mode` の vault sanity gate 強化** (項目 246/251 で導入済):
- 既存: vault lint 4 軸 strict bail (`run_vault_sanity_gate`)
- 追加: 5 軸目 (unreviewed_aged) 評価
  - env `BONSAI_VAULT_UNREVIEWED_LAB=1` で 5 軸目を Lab pre-gate に統合 (default OFF、backward compat 100%)
  - env `BONSAI_VAULT_UNREVIEWED_STRICT=1` で 5 軸目検出時 strict bail (項目 251 bail pattern 流用)

## 7. Phase 5 smoke 検証基準

### G-VS-1: env unset で 5 軸目 no-op (後方互換)
- 期待: `--vault` mode で既存 4 軸のみ表示、unreviewed_aged 計算スキップ
- ACCEPT: cargo test --lib 1344 passed (+4 from baseline)

### G-VS-2: env=1 + dirty case (status: draft + 30 日経過)
- 期待: `--vault` mode で 5 軸目 1 件検出、stderr に WARN 出力
- ACCEPT: `grep "unreviewed" vault_lint_output.txt` 1 件

### G-VS-3: env=1 + strict case (5 軸目検出 + bail)
- 期待: Lab cycle 起動前に anyhow::bail で abort、audit_log VaultLint variant に記録
- ACCEPT: `bonsai --lab` exit code != 0、audit_log row 追加

### G-VS-4: clean case (env=1 で全 entry reviewed)
- 期待: 5 軸目検出ゼロ、Lab cycle 通常起動
- ACCEPT: `bonsai --lab` 正常起動、stderr に WARN なし

### G-VS-5: 案 I-2 の compile workflow との連動 (将来案 C 拡張時)
- 期待: compile で `status: draft → reviewed` 遷移、5 軸目検出から除外
- ACCEPT: 案 I-2 Phase 4 完成後の smoke で確証

## 8. Rollback strategy

### 完全 rollback (env unset)
- env `BONSAI_VAULT_UNREVIEWED_LAB` unset で 5 軸目検出スキップ (Phase 4 wiring の if 分岐で no-op)
- vault_lint.rs の 5 軸目検出 logic は `unreviewed_aged: Vec<...>` field 経由、cargo build には影響なし

### git revert 影響範囲
- 案 A: src/config.rs (+20 LOC) + src/knowledge/vault_lint.rs (+80 LOC) + tests (+50 LOC) = 単一 phase で revert 容易
- Vault::write 等の他 module 変更ゼロ (案 A の利点)

## 9. bonsai 既存資産との整合性

### 項目 244 LLM Wiki Lint パターン (KG lint pass)
- vault_lint と KG lint は orthogonal (vault = file system 上の wiki、KG = SQLite 上の graph)
- Phase 4 wiring で両 lint を順次実行可能 (handle_lab_mode 内で vault → KG の順)

### 項目 246 Vault Lint Phase 1-4 (4 軸検出)
- 本 plan は 5 軸目追加、既存 4 軸 (duplicates / stale / orphan / case_variant) と完全 orthogonal
- LintReport struct に 1 field 追加するだけ、既存 caller の API 影響ゼロ

### 項目 251 Vault Lint bail branch test
- 本 plan の Phase 4 strict bail は項目 251 で確立した pattern (`is_clean = (axes.is_empty() && unreviewed_aged.is_empty())`) を踏襲
- bail branch test を 5 軸目にも追加 (test 5: dirty unreviewed + strict → Err)

### 項目 230/237 Plan A factcheck (KG-FactCheck)
- 本 plan と orthogonal (vault entry の status vs KG triple の matched/conflicting)
- compile 時に factcheck 強制呼出は案 I-2 で対処

### 項目 248 Dynamic Token Budget Phase 5 (axis prune)
- 本 plan と orthogonal (compaction の 4 軸 vs vault_lint の 5 軸)
- 名称 collision 注意: vault_lint の「軸」と compaction の「軸」は別概念

### 項目 110 audit_log
- `AuditAction::VaultLint` variant は項目 246 で追加済、本 plan の 5 軸目検出も同 variant で記録 (LintReport summary に 5 軸目 count 含める)

## 10. 記事との対応関係マトリクス

| 記事概念 | bonsai 実装箇所 | 備考 |
|---|---|---|
| Frontmatter `status: reviewed` | vault entry frontmatter (案 I-2 の compile が書込) | 本 plan は parse + 検出のみ |
| Frontmatter `status: draft` | vault entry frontmatter (案 I-2 の ingest が書込) | 本 plan は parse + 検出のみ |
| Frontmatter `status: stale` (記事未明示) | vault entry frontmatter (operator が手動付与) | 「廃止予定」マーク |
| Obsidian Dataview `WHERE status != "reviewed"` | vault_lint.rs 5 軸目検出 logic | CLI dashboard 代替 |
| Dataview `SORT updated_at ASC` | `unreviewed_aged` を `age_days` desc でソート | 古い順表示 |
| 「未レビュー監視」 | `bonsai --vault` mode で 5 軸目 summary 表示 | Lab pre-gate でも warn 出力 |
| `updated_at` field | 既存 vault_lint stale 検出で parse 済、本 plan で 5 軸目にも使用 | 統一 |
| `sources: [01_raw/...]` | (案 I-2 で扱う、本 plan 範囲外) | |

## 11. 工数見積もり

| Phase | 内容 | LOC | 工数 |
|---|---|---|---|
| 1 Red | 4 test 作成 | ~80 | 1h |
| 2 Green | env getter 2 + LintReport field + 検出 logic | ~100 | 2-3h |
| 3 Refactor | rustdoc + frontmatter parse helper 共通化 | ~30 | 30min |
| 4 wiring | handle_lab_mode 統合 + env gate | ~30 | 30min |
| 5 smoke | G-VS-1/2/3/4 実機検証 | - | 30min |
| **合計** | | **~240** | **4-5h** |

## 12. Open questions

1. **`status: stale` の処理**: 記事には未明示。本 plan で導入する場合、5 軸目検出から除外 (operator が明示的に廃止予定マーク = warn 不要) or 含める (廃止予定でも長期未対処は問題)?
2. **未 frontmatter entry の default**: backward compat の reviewed 扱い vs strict mode の draft 扱い、どちらを default にすべきか? 提案: backward compat default、strict mode は env opt-in。
3. **Vault::write 経路での自動 status 付与**: 本 plan は parse + 検出のみ、status 付与は案 I-2 で対処。案 I-2 未完成時に operator が手動 frontmatter 編集する手間あり。
4. **Migration**: 既存 vault entry 全件への `status: reviewed` 一括付与は migration script 必要? backward compat default なら不要。
5. **vault_lint.rs の 6 軸目** (`stale_drafts`): 案 I-2 で `_drafts/` directory が登場後、TTL gc のため必要? 本 plan で予約だけしておく?

## 13. 次手

1. user 承認後 Phase 1 Red 着手 (4 test 作成 + 全 fail 確証)
2. Phase 2 Green 実装 (env getter + LintReport field + 検出 logic)
3. Phase 3 Refactor (frontmatter parse helper 共通化)
4. Phase 4 wiring (handle_lab_mode 統合)
5. Phase 5 smoke G-VS-1/2/3/4 (~30 min 実機検証)

## 14. 関連項目

- 項目 76/77/80: Karpathy LLM Wiki 由来の知識 Vault 設計
- 項目 244: LLM Wiki Lint パターン (KG lint への適用済)
- 項目 246: Vault Lint Phase 1-4 (4 軸検出、本 plan の 5 軸目の基盤)
- 項目 251: Vault Lint bail branch test (本 plan の Phase 4 strict bail pattern の基盤)
- 項目 248: Dynamic Token Budget Phase 5 (orthogonal、軸 collision 注意)
- 推定項目番号: **項目 254** (本 plan 実装後、案 I-2 = 項目 253 と連動)

## 15. 関連 plan / memory

- `memory/qiita_2do_brain_learnings.md` — 記事 9 概念 + bonsai 適用 5 案
- `.claude/plan/vault-ingest-compile-separation.md` — 案 I-2 (本 plan と orthogonal、将来案 C で統合可能)
- `.claude/plan/vault-lint-bail-branch-test.md` — 項目 246 critic F1 (項目 251 で実装済、bail pattern の reference)
- `.claude/plan/vault-lint-coverage-check.md` — 項目 246 coverage 関連
