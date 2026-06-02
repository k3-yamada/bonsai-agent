# Vault Ingest/Compile 2-skill 分離 (2do BRAIN 適用案 I-2)

> **状態 (2026-06-02 更新): 据え置き — plan 前提が現コードに不在。**
> 本 plan は `01_raw/` → `_drafts/` → `02_wiki/` の Obsidian 風ディレクトリ構造 +
> frontmatter (`status`/`sources`) + `wiki/index.md`/`log.md` を前提とするが、
> 現 `src/knowledge/vault.rs` の `Vault` は **category-stock 型**
> (`decisions.md`/`facts.md`/... のフラットファイル + `append`/`summary`) で、これらは一切存在しない。
> `vault_lint.rs` の `status`/`unreviewed` も **DB memory entry** に対する SQL 相当で wiki frontmatter ではない。
> agent_loop からの Vault 破壊的 auto-invoke 経路も現状なく、plan の安全動機 (auto-invoke 暴走防止) も適用されない。
> よって本 plan は「小さな bounded タスク」ではなく **何も使っていない greenfield wiki サブシステムの新規構築**
> (~460+ LOC, 6-9h) であり、YAGNI / anti-churn 原則と衝突。ユーザー判断 (2026-06-02) で **据え置き確定**。
> 再着手時は §1〜§2 の前提を現 `Vault` モデルに合わせて全面再設計すること。


## 1. 問題定義

### Qiita 2do BRAIN 記事の core pattern (verbatim)

**`ingest` skill (read-only)**:
> `allowed-tools: [Read, Grep]` の事前承認のみ。手順 2 のドラフト保存 (書き込み) のタイミングで AI がユーザーに実行承認を求めてくる。この一手間が AI の暴走を防ぐ確実な安全弁として機能します。

Workflow: 01_raw 読込 → 主要主張/エンティティ抽出 → `_drafts/YYYY-MM-DD_*.md` 保存。**【厳守】境界の維持: この時点では絶対に 02_wiki/ 本体には書き込まないこと**。

**`compile` skill (destructive、明示的手動起動のみ)**:
> `disable-model-invocation: true` でこのスキルは Claude のコンテキストやサブエージェントにプリロードされない (AI 側からは見えなくなる)。AI が勝手な文脈で自動実行 (Auto-invoke) して Wiki を破壊するのを防ぎ、人間が意図したタイミングで `/compile` と明示的に手動起動させるための堅牢な運用設計です。

Workflow: 「既存ページを更新する場合、再帰的要約劣化を防ぐため、**必ず対応する `01_raw/` の該当箇所を再読込**して事実確認を行う」 → 全主張に `[[01_raw/filename]]` インライン出典 → frontmatter `status: reviewed` → `02_wiki/index.md` + `02_wiki/log.md` 追記。

### bonsai-agent 現状の gap

| 観点 | 記事 | bonsai 現状 | gap |
|---|---|---|---|
| 抽出+書込の分離 | ingest (RO) → compile (RW) で 2 step | `Vault::extract` + `Vault::write` が 1 trajectory 内で連続 | atomic 化、roll-back コスト大 |
| Auto-invoke 禁止機構 | `disable-model-invocation: true` | agent_loop から Vault::write を tool 経由 auto-invoke | manual-only mode 無し |
| Raw 再読込 enforce | 全主張で `[[01_raw/...]]` 必須 + 「Raw 再読込」明文化 | compaction.rs (4 段階) は要約のみ、原典再読込未強制 | 再帰的要約劣化リスク |
| index/log 出典 | plain text `index.md`/`log.md` 自動更新 | DB-backed audit_log のみ (human review 困難) | git diff 可能 trail 不在 |
| status state | `draft` → `reviewed` 状態遷移 | frontmatter `status` field 無し | (案 I-3 で対処) |

## 2. 設計判断 3 案

### 案 A: CLI subcommand 分離 (`--ingest <raw>` / `--compile <draft>`)

**実装方針**:
- `src/main.rs::Cli` に 2 flag 追加:
  - `--ingest <raw_path>`: 01_raw 読込 → `_drafts/YYYY-MM-DD-<hash>.md` 書出のみ
  - `--compile <draft_path>`: draft 読込 → 対応 raw を `Vault::read_raw_sources(&draft.frontmatter.sources)` で強制再読込 → wiki Edit + `wiki/index.md` + `wiki/log.md` 追記
- `src/knowledge/vault.rs` に 2 method 追加:
  - `Vault::ingest_to_draft(raw_path) -> Result<DraftPath>` (read-only on wiki/、_drafts/ のみ書出)
  - `Vault::compile_to_wiki(draft_path) -> Result<()>` (RW)
- agent_loop から自動呼出される `Vault::write` は **`ingest_to_draft` のみ** に変更 (compile 経路は CLI 専用)

**特徴**:
- CLI subcommand で完全分離 = 構造的に「automation vs manual」境界が明確
- 既存 `--vault` (RO summary) は維持、新規 `--ingest`/`--compile` は additive
- agent_loop からの auto-invoke は ingest までで停止 → 明示 compile が unhuman approval gate

**欠点**:
- Vault::write の caller 変更で agent_loop 経路の existing test 軽微影響
- `_drafts/` の lifecycle (TTL/gc) は別途設計必要

### 案 B: middleware で gate (`CompileGuardMiddleware`、env-gated)

**実装方針**:
- `src/agent/middleware.rs` に `CompileGuardMiddleware` 追加
- env `BONSAI_VAULT_COMPILE_REQUIRE_MANUAL=1` で agent_loop からの `Vault::compile_to_wiki` 呼出を block (Err 返却)
- 明示 CLI (`--compile`) からは permit (env 無視)
- Vault method は案 A と同じ (`ingest_to_draft` / `compile_to_wiki`)

**特徴**:
- middleware chain 既存 pattern を流用 (Compaction/Critic 等と並列)
- env で gate 可能、test 容易

**欠点**:
- middleware は CompactionMiddleware と並列の per-step hook で、Vault tool 呼出時の dispatch には別途配線必要
- middleware は agent_loop 内 step ごとに走る設計、Vault write は infrequent operation で middleware overhead が割高

### 案 C: 案 A + 案 B 統合 (CLI 分離 + middleware gate)

- 案 A の CLI subcommand 分離をベースに、案 B の middleware gate を **将来オプション**として残す
- Phase 1-3: 案 A 実装 (CLI 分離 + Vault 2 method)
- Phase 4: case B middleware の opt-in 配線 (env unset で完全 no-op)
- 段階的導入で migration cost を分散

## 3. 5 軸比較

| 軸 | 案 A (CLI) | 案 B (middleware) | 案 C (統合) |
|---|---|---|---|
| 既存資産活用度 | ★★★ `--vault`/`--lab` 等の CLI pattern 流用 | ★★ Compaction/Critic middleware pattern 流用 | ★★★ 案 A の上に case B 加算 |
| 実装工数 | ★★ Vault 2 method + CLI 2 flag (~250 LOC) | ★★ middleware impl + 配線 (~200 LOC) | ★ 工数加算 (~450 LOC) |
| TDD test 設計容易性 | ★★★ Vault::ingest_to_draft / compile_to_wiki の unit test 独立 | ★★ middleware chain と Vault tool dispatch を mock 化必要 | ★★ 案 A 部分は ★★★、case B は ★★ |
| 既存機構整合性 | ★★★ CLI 分離は既存 main.rs::handle_*_mode pattern 完全踏襲 | ★★ middleware は infrequent op に対し overhead | ★★ 段階導入で良いが complexity 増 |
| rollback 容易性 | ★★★ CLI flag 追加は additive、既存 `--vault` 不変 | ★★★ env unset で完全 no-op | ★★★ 両方 |

**総合**:
- 案 A = 14/15 ★ ← **推奨**
- 案 B = 11/15 ★
- 案 C = 12/15 ★ (将来移行)

## 4. 推奨案: 案 A (CLI subcommand 分離)

### 採用理由

1. **構造的シンプル**: CLI flag 分離は既存 `--vault`/`--lab` と同 pattern、reader/operator が一目で挙動把握可能。
2. **agent_loop auto-invoke 路の clean cut**: agent_loop は `Vault::ingest_to_draft` のみ呼出可能、`compile_to_wiki` は CLI 専用 = 「破壊的操作の human-in-the-loop 強制」が code structure で表現。
3. **既存 test pattern 流用**: `Vault` の既存 unit test に 2 method 追加だけ、middleware test の mock complexity 回避。
4. **rollback 容易**: `--ingest`/`--compile` flag を CLI から削除すれば旧挙動復活、Vault 2 method は内部 helper として残しても backward compat。

### 副次設計判断

- **`_drafts/` lifecycle**: ephemeral、TTL 7 日で自動 gc (vault_lint.rs に 6 軸目 `stale_drafts` 追加で監視可能、案 I-3 と連動)
- **frontmatter `sources: [01_raw/...]` 必須**: compile 時に `Vault::read_raw_sources` で raw 再読込 enforce、ソース無し draft は compile reject
- **`wiki/index.md` フォーマット**: `| [link] | 1-line summary | YYYY-MM-DD |` table、compile 末尾で auto append
- **`wiki/log.md` フォーマット**: `## [YYYY-MM-DD] Compile | <draft_basename>` (記事と同形式)

## 5. TDD strict 3-phase outline

### Phase 1: Red (7 test)

1. `t_ingest_to_draft_creates_drafts_only` (wiki/ 不変確証)
2. `t_compile_rejects_draft_without_sources` (sources frontmatter required)
3. `t_compile_forces_raw_reread` (raw 書換後に compile、新内容反映)
4. `t_compile_updates_status_to_reviewed`
5. `t_compile_appends_to_wiki_index_md`
6. `t_compile_appends_to_wiki_log_md`
7. `t_ingest_assigns_status_draft`

**Phase 1 Red 検証**: 7 test 全 fail。

### Phase 2: Green (本実装)

**`src/knowledge/vault.rs`** (~200 LOC):
- `pub fn ingest_to_draft(&mut self, raw_path: &Path) -> Result<DraftHandle>`
- `pub fn compile_to_wiki(&mut self, draft_path: &Path) -> Result<()>`

**`src/main.rs`** (~30 LOC):
- `Cli` に `--ingest` / `--compile` flag
- `handle_ingest_mode` / `handle_compile_mode` dispatch

**Phase 2 Green 検証**: 7 test PASS + 既存退行ゼロ (1340 → 1347)。

### Phase 3: Refactor

- rustdoc 拡充 (境界 contract 明示)
- `Vault::write` の deprecation 検討
- clippy clean / fmt clean

## 6. Phase 4 wiring 設計

**`src/main.rs::main`** dispatch 追加:
```rust
if let Some(path) = cli.ingest { return handle_ingest_mode(path); }
if let Some(path) = cli.compile { return handle_compile_mode(path); }
```

**env gate (将来オプション、案 C への部分実装)**: env `BONSAI_VAULT_AUTO_COMPILE_BLOCK=1` で agent_loop 経路からの compile 呼出を block (現状 no-op、将来案 C 拡張時に有効化)。

## 7. Phase 5 smoke 検証基準

### G-IC-1: env unset で既存挙動 100% 互換
- 期待: vault/raw/ + vault/wiki/ + vault/_drafts/ 構造で動作、既存 entry 不変
- ACCEPT: cargo test --lib 1347 passed

### G-IC-2: `--ingest 01_raw/test.md` で `_drafts/` only 書出
- 期待: `vault/_drafts/2026-05-20-test.md` 作成、`vault/wiki/` 不変
- ACCEPT: `_drafts/test.md` 内容に `status: draft` 含む

### G-IC-3: `--compile <draft>` で raw 再読込 + index/log 更新
- 期待: `vault/wiki/test.md` 作成 (`status: reviewed`)、`wiki/index.md` table 行追加、`wiki/log.md` 履歴行追加
- ACCEPT: `grep "Compile | test" vault/wiki/log.md` 1 件

### G-IC-4: 将来案 C 拡張時 (本 plan 範囲外)
### G-IC-5: 項目 246/251 vault_lint との連動
- compile 後の wiki に対し既存 4 軸検出が正常動作

## 8. Rollback strategy

- 完全 rollback: CLI flag `--ingest`/`--compile` 削除で旧挙動復活
- git revert 影響範囲: src/knowledge/vault.rs (+200 LOC) + src/main.rs (+30 LOC) + tests (+150 LOC)
- main.rs の dispatch 分岐は他 mode と独立、衝突無し

## 9. bonsai 既存資産との整合性

### 項目 244 LLM Wiki Lint パターン (KG lint pass)
- compile_to_wiki の最後で KG lint pass を呼出 (orphan link 検出)
- 既存 `seed_kg_for_factcheck_lab` の seed 拡張で compile 後の KG 状態を test 内で reset 可能

### 項目 246/251 Vault Lint (4 軸検出)
- compile 後の wiki 全体に対し既存 vault_lint 4 軸を再走 (compile が duplicates 増産しないか確証)
- vault_lint.rs に 6 軸目 `stale_drafts` (案 I-3 で 5 軸目 status と連動) 追加検討

### 項目 230/237 Plan A factcheck (KG-FactCheck)
- compile 時に `factcheck::run_factcheck_pass_lab(...)` を opt-in 強制呼出 (env `BONSAI_VAULT_COMPILE_FACTCHECK=1`)

### 項目 110 audit_log
- `AuditAction::VaultIngest { draft_path }` / `AuditAction::VaultCompile { draft_path, wiki_path }` variant 追加

## 10. 記事との対応関係マトリクス

| 記事概念 | bonsai 実装箇所 |
|---|---|
| `ingest` skill (RO) | `bonsai --ingest` CLI + `Vault::ingest_to_draft` |
| `compile` skill (RW) | `bonsai --compile` CLI + `Vault::compile_to_wiki` |
| `disable-model-invocation: true` | CLI flag 設計で構造的に達成 (agent_loop 経路に compile 無し) |
| `paths: [01_raw/*]` | `Vault::ingest_to_draft` の引数 path validation |
| 「【厳守】境界の維持」 | `Vault::ingest_to_draft` の signature でのみ `_drafts/` 書出許可 |
| 「Raw 再読込」 | `Vault::compile_to_wiki` 内で `read_raw_sources(&frontmatter.sources)` 強制呼出 |
| Frontmatter `status: draft → reviewed` | ingest が `status: draft` 書込、compile が `status: reviewed` 書換 |
| `02_wiki/index.md` カタログ | `Vault::compile_to_wiki` が table row 追記 |
| `02_wiki/log.md` 履歴 | `Vault::compile_to_wiki` が `## [YYYY-MM-DD] Compile | <basename>` 追記 |
| `[[01_raw/filename]]` inline citation | compile が raw merge 時に link 自動付与 |
| `Bash(git add 02_wiki/*)` scope | (案 I-4 で対処、本 plan 範囲外) |
| Obsidian Dataview | `--vault` mode + vault_lint 5 軸目で代替 (案 I-3) |

## 11. 工数見積もり

| Phase | 内容 | LOC | 工数 |
|---|---|---|---|
| 1 Red | 7 test 作成 | ~150 | 1-2h |
| 2 Green | Vault 2 method + CLI 2 flag + frontmatter parse | ~230 | 3-4h |
| 3 Refactor | rustdoc + clippy/fmt + Vault::write deprecation 検討 | ~50 | 1h |
| 4 wiring | main.rs dispatch + env gate | ~30 | 30min |
| 5 smoke | G-IC-1/2/3/5 実機検証 | - | 1h |
| **合計** | | **~460** | **6-9h** |

## 12. Open questions

1. **`_drafts/` の TTL gc**: 7 日 auto-gc を vault_lint.rs に統合 (案 I-3 と連動)?
2. **`AuditAction::VaultIngest`/`VaultCompile` variant 分割**: 既存粒度との整合性?
3. **Claims 抽出方式**: 既存 `extractor.rs::Extractor` の 6 カテゴリ抽出と統合 or ingest 専用 subset?
4. **`index.md` 形式**: Obsidian table syntax (Dataview 連動) or plain markdown list?
5. **`Vault::write` の deprecation**: 既存 caller (agent_loop tool 経由) の置換戦略?

## 13. 次手

1. user 承認後 Phase 1 Red 着手 (7 test 作成 + 全 fail 確証)
2. Phase 2 Green 実装 (Vault 2 method + CLI 2 flag)
3. Phase 3 Refactor (rustdoc + deprecation 検討)
4. Phase 4 wiring (main.rs dispatch)
5. Phase 5 smoke G-IC-1/2/3/5 (~1h 実機検証)

## 14. 関連項目

- 項目 76/77/80: Karpathy LLM Wiki 由来の知識 Vault 設計原則
- 項目 244: LLM Wiki Lint パターン
- 項目 246/251: Vault Lint
- 項目 230/237: Plan A KG-FactCheck
- 項目 110: audit_log
- 推定項目番号: **項目 253** (本 plan 実装後)

## 15. 関連 plan / memory

- `memory/qiita_2do_brain_learnings.md` — 記事 9 概念 + bonsai 適用 5 案
- `.claude/plan/vault-status-state-machine.md` — 案 I-3 (frontmatter status + vault_lint 5 軸目、並行 plan)
- `.claude/plan/vault-lint-bail-branch-test.md` — 項目 246 critic F1 (項目 251 で実装済)
- `.claude/plan/dynamic-token-budget-phase5-axis-prune.md` — 項目 248 Phase 5 (orthogonal)
