# Vault 知識ストック 品質ゲート (旧「Ingest/Compile 2-skill 分離」再設計版)

> **状態 (2026-06-02 再設計): 旧 plan の raw→draft→wiki greenfield 構想を却下し、実在フローに接地して再設計。**
> 旧版 (2do BRAIN 案 I-2 そのまま移植) は `01_raw/`→`_drafts/`→`02_wiki/` の Obsidian 風
> ディレクトリ + frontmatter + 手動 compile を前提としたが、現 `Vault` は category-stock 型で
> 該当構造は皆無。ユーザー判断 (2026-06-02) で旧構想は **据え置き → 本 doc で実フローに再設計**。
> 本再設計は「ingest/compile *分離* というディレクトリ機構」ではなく、その**精神=「無審査の自動書込で
> 知識ストックを汚染しない」**を、実在する `extract_stock → append_all` 経路の **品質ゲート**として実装する。

**起票日**: 2026-05-20 / **再設計**: 2026-06-02
**起源**: Qiita 2do BRAIN (案 I-2) + Zenn tsurubee LLM Wiki (Ingest/Query/Lint の Ingest 軸)
**優先度**: ★★ (evidence-gated。下記 §8 の採否ゲート未充足なら据え置き継続)
**production code touch**: あり (`agent/context_inject.rs` + `knowledge/extractor.rs` + `knowledge/vault.rs`、env-gated)

---

## 1. 実フロー (2026-06-02 コード確認、grounded)

Vault は vestigial ではなく **production 書込経路が正確に 1 つ**存在する:

```
agent/context_inject.rs::inject_contextual_memories(session, task_context, store)
  └─ Vault::new(~/Library/Application Support/bonsai-agent/vault)      # 6 category .md を ensure
  └─ extractor::extract_stock(task_context, &session.id) -> Vec<StockEntry>
  └─ Vault::append_all(&stocks)                                        # ★無条件 append-only
  └─ inject_vault_knowledge(session, task_context, v)                  # category 読戻し → context 注入
```

- `extract_stock` (extractor.rs:129): task メッセージが decision/todo/insight/preference/fact の
  **正規表現に1つでも一致すると `content = message.to_string()` (メッセージ全文)** を該当カテゴリに push。
  カテゴリ毎1件、計最大5件。
- `Vault::append` (vault.rs:32): category .md に追記。dedup は **先頭50字 prefix 一致のみ**。
- `--vault` CLI = `handle_vault_mode` = `summary()` 読取。
- `vault_lint.rs` (項目246/251/254) は **DB memory entry** を lint (本 .md 群とは別系統)。

### 実フローの弱点 (= 真の gap)

| # | 弱点 | 影響 |
|---|------|------|
| G1 | **無審査の自動 append**: regex 一致で全タスクが書込まれる | 低品質 entry 累積 → `inject_vault_knowledge` 経由で context 汚染 (1bit に有害) |
| G2 | **claim ではなく message 全文を保存** | category .md が肥大、要点不明、summary() 希釈 |
| G3 | **dedup が先頭50字 prefix のみ** | 前置きが同じ別内容は重複保存、僅差の言い換えは検出漏れ |
| G4 | **review 状態なし** (append-only) | 古い・誤った stock を剪定する術がない (vault_lint は DB 側のみ) |

旧 plan が持ち出した「再帰的要約劣化」「auto-invoke wiki 破壊」「model-invocation 制御」は
本フロー (append-only flat file・regex 抽出・非 model 起動) には**該当しない**。よって旧構想は不適。

---

## 2. 再設計の方針

2do BRAIN「ingest (RO) / compile (deliberate)」の**精神のみ**を採り、ディレクトリ機構は持ち込まない:

- **ingest 相当** = `extract_stock` を「メッセージ全文 dump」から「**claim 抽出 + 信頼度**」へ格上げ (G2)。
- **compile 相当 (deliberate write)** = append を**品質ゲート通過分のみ**に絞る (G1)。閾値 + 正規化 dedup (G3)。
- **review 相当** = StockEntry に最小限の `confidence`/`source` provenance を持たせ、将来の剪定 (G4) に備える。

全て **env-gated** とし、unset で現挙動 100% 維持 (backward compat、既存 1432 test 不変)。

---

## 3. 案比較 (3 案 × 5 軸)

| 軸 | 案 A (extract_stock 品質ゲート、最小) | 案 B (session-end deferred compile) | 案 C (旧 raw→wiki greenfield) |
|----|---------------------------------------|--------------------------------------|-------------------------------|
| 実フロー接地 | ★★★ 既存 1 経路を直接改善 | ★★ buffer 機構を context_inject に追加 | ★ 存在しない構造の新規構築 |
| 工数 | ★★ (~120 LOC: extractor 1 fn + append gate + env) | ★ (~250 LOC: buffer + flush + CLI) | ✗ (~460+ LOC, 6-9h) |
| backward compat | ★★★ env unset で不変 | ★★ buffer の session lifecycle 影響 | ★ CLI 衝突 (`--ingest` 既使用) |
| 1bit 効用 | ★★★ context 汚染源を直接低減 | ★★ 汚染低減は同等、即時性低下 | ? 未検証 |
| YAGNI 整合 | ★★★ 既存機能の質改善のみ | ★★ 新規 buffer = 追加状態 | ✗ 不使用機構の新設 |
| **総合** | **15/15 ★ 推奨** | 11/15 | 却下 |

### 推奨 = **案 A (extract_stock 品質ゲート)**

最小・実フロー直結・YAGNI 整合。案 B (deferred compile) は案 A で G1/G2 を解消後も
「自動 vs 明示」分離が必要と evidence が出た時の follow-up に格下げ。案 C は §状態ヘッダ通り却下。

---

## 4. 案 A 詳細設計

### 4.1 extractor 改善 (G2 + claim 抽出)
- `extract_stock` に「マッチした正規表現の**キャプチャ部 (claim)** を content とする」path を追加。
  正規表現に capture group を持たせ、無ければ従来通り全文 fallback。
- `StockEntry` に `confidence: f32` 追加 (pattern 種別由来の固定値: decision=0.8 / fact=0.7 /
  insight=0.6 / preference=0.6 / todo=0.5 等、要 tune)。`source` は既存維持。

### 4.2 append 品質ゲート (G1 + G3)
- `Vault::append_all` の前段に env-gated フィルタ:
  - `BONSAI_VAULT_MIN_CONFIDENCE` (default unset = ゲート無効 = 現挙動)。set 時は
    `entry.confidence >= threshold` のみ通過。
  - dedup を「先頭50字 prefix」から「**正規化 (trim + 連続空白圧縮 + lowercase) 全文一致**」へ強化
    (G3、安全側に `BONSAI_VAULT_STRICT_DEDUP` env で opt-in)。

### 4.3 backward compat
- env 全 unset で `extract_stock` は従来出力、`append_all` は従来 dedup → **挙動完全不変**。

---

## 5. ACCEPT 条件

### 5.1 unit (Phase 1 Red → Phase 2 Green)
- (a) `extract_stock` claim 抽出: capture group 持ち pattern で content がメッセージ全文でなく claim 部になる test
- (b) `confidence` 付与: category 別 confidence 値の test
- (c) append ゲート: `BONSAI_VAULT_MIN_CONFIDENCE=0.75` で fact(0.7) が除外/decision(0.8) が通過する test
- (d) 正規化 dedup: 「  Foo  bar 」と「foo bar」が同一視される test (env on 時)
- (e) backward compat: env 全 unset で従来 5 件抽出 + prefix dedup が不変の test
- (f) cargo test --lib 1432 → 1438+ retention、clippy / fmt clean

### 5.2 integration
- (g) `context_inject` 経路で env-gated filter が効く wiring test (in-memory Vault)

### 5.3 Phase 4 Smoke (要 MLX、optional)
- (h) `BONSAI_VAULT_MIN_CONFIDENCE=0.7 ./scripts/g_mct2_smoke.sh` で vault .md の entry 数が baseline 比減少、score 退行なし

---

## 6. TDD strict outline (3 phase)

- **Phase 1 Red**: §5.1 の 5 test を先行追加 (extractor.rs + vault.rs tests)、未実装 API で fail。
- **Phase 2 Green**: extractor capture path + StockEntry.confidence + Vault append gate (env getter は
  `config.rs` or `knowledge/vault.rs` ローカル、layer 注意: knowledge 層は db/observability/safety/memory のみ参照可)。
- **Phase 3 Refactor**: rustdoc、runbook env table に 2 env 追記、clippy/fmt。

---

## 7. Rollback

- env 全 unset で現挙動。code revert は extractor capture path + append gate の 2 箇所削除。

---

## 8. 採否ゲート (honest ROI 評価)

本再設計は **evidence-gated で据え置き継続が default**。着手判断は以下のいずれか充足時:
- (i) 実 vault .md を点検し、低品質/重複 entry が実害レベルで累積している事を確認 (要 production vault dump 確認)。
- (ii) Lab smoke で `inject_vault_knowledge` 注入が score を有意に下げている疑いが出た時。
- いずれも未確認なら、現 append-only は低リスク (項目 G1-G4 は理論上の gap) のため **着手しない**判断が妥当。

→ **次手: production vault (`~/Library/Application Support/bonsai-agent/vault/*.md`) の entry 品質を
read-only 点検し、(i) の実害有無を確定**してから Phase 1 着手可否を決める。

### 8.1 点検結果 (2026-06-02 実施、read-only)

| file | entries | uniq(norm) | dup | avg_len |
|------|--------:|-----------:|----:|--------:|
| decisions.md | 1 | 1 | 0 | 39 |
| facts.md | 5 | 5 | 0 | 106 |
| insights.md | 0 | 0 | 0 | 0 |
| patterns.md | 0 | 0 | 0 | 0 |
| preferences.md | 4 | 4 | 0 | 22 |
| todos.md | 3 | 3 | 0 | 23 |
| **計** | **13** | **13** | **0** | — |

- **G1/G3 (累積・重複の実害) = ゼロ** (総 13 entry / dup 0)。production で Vault 書込頻度が極小
  (多くの run は `BONSAI_DB_PATH` 隔離 or agent 非起動)。
- **G2 (全文 dump 由来 noise) は実在するが軽微**: facts.md に Lab task prompt が "fact" として誤捕捉
  (例: `"Describe what prism-ml is. Use the format 'prism-ml is a X'"`)。ただし 5 entry で実害レベル未満。

**判定: 採否ゲート (i) NOT MET → 案 A も据え置き継続が妥当。** 13 entry / dup 0 の現状で品質ゲートを
実装するのは non-problem への premature optimization = YAGNI 違反。再設計 (案 A) は「実装可能な
grounded 設計」として本 doc に保全し、**volume が実害レベル (例: 単一 category 100+ entry or dup 顕在化)
に達した時点で Phase 1 着手**する。それまで code touch しない。

---

## 9. 旧設計却下の記録 (再着手防止)

旧 §1-§2 (raw→draft→wiki + frontmatter status/sources + wiki/index.md/log.md + `--vault-ingest`/
`--vault-compile` CLI) は **現 category-stock Vault に対し過剰**。`--ingest` は memory ingest で既使用、
auto-invoke 破壊経路も不在。Obsidian 由来の機構移植は YAGNI 違反として恒久却下。

## 10. 関連
- 実ファイル: `src/agent/context_inject.rs:252-268` / `src/knowledge/extractor.rs:129` / `src/knowledge/vault.rs:32,79`
- 項目 244/246/251/254 (lint pattern) / 項目 76/77/80 (LLM Wiki 原則)
- `memory/qiita_2do_brain_learnings.md` / `memory/zenn_tsurubee_llm_wiki_learnings.md`
