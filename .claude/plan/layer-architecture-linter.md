# Layer Architecture Linter (Zenn 適用案 Z-4 + Z-3 統合)

## 1. 問題定義

Zenn dragon1208 Step 4 (verbatim): カスタムリンターでレイヤー違反を検出、エラー msg に修正方法 docs link を埋め込んで agent 自律 fix 可能化。

bonsai 現状:
- src/ 95+ source file が 9 module に分散
- Rust の `mod` 階層で自然制約はあるが、明示 enforcement なし
- 項目 244/246/251/254 で他軸の lint 確立済

### Zenn 6 種 linter code (verbatim) と bonsai 適用版

| Zenn コード | Zenn チェック | bonsai 適用版 |
|---|---|---|
| DEP-001 | レイヤー違反 | module layer 順違反 |
| LOG-001 | console.log 使用 | production code で eprintln! 直接 (log_event 経由が原則) |
| SIZE-001 | 500 行超過 | 800 行超過 (CLAUDE.md 慣例) |
| NAME-001 | スキーマ型 | trait 命名 (Tool/Backend/Sandbox は名詞) |
| NAME-002 | サービス関数 | tool function 命名 (動詞_名詞) |
| TYPE-001 | any 使用 | dyn Any / Box<dyn Any> 使用 (CLAUDE.md「No dynamic」) |

## 2. 設計判断 3 案

### 案 A: cargo xtask binary
workspace 化 + `xtask/check-architecture`。Rust ecosystem 統合最大、ただし workspace 化が必要。

### 案 B: tests/structural/ integration test (Z-3 と統合) ← **推奨**
`tests/structural/layer_rules.rs` で `cargo test --test structural` 経由。既存 workflow integration、最小工数。

### 案 C: Python script (scripts/lint_layers.py)
regex ベース、Rust syn AST 活用不可。

## 3. 5 軸比較

| 軸 | A (xtask) | B (test) | C (python) |
|---|---|---|---|
| Rust 統合度 | ★★★ | ★★ | ★ |
| CI 統合容易性 | ★★ | ★★★ | ★★ |
| エラー msg 柔軟性 | ★★★ | ★★ | ★★★ |
| 実装工数 | ★★ | ★★★ | ★★★ |
| 既存資産活用 | ★★ | ★★★ | ★ |

**案 B = 13/15 ★ 推奨** (Z-3 structural test と統合、単一実装で 2 案 cover)。

## 4. 推奨案 B: tests/structural/

### 採用理由
1. Z-3 structural test と統合、単一実装で 2 plan cover
2. 既存 `cargo test --lib` workflow に seamless
3. 項目 254 vault_lint test pattern 踏襲
4. workspace 化不要

### 副次設計
- エラー msg: `[LINT:DEP-001] src/file.rs ... 修正方法: docs/architecture/module-layer-rules.md#dep-001` 形式
- whitelist 機構: 既存超過 file (benchmark.rs 1500+ 行 / experiment.rs 1900+ 行 / compaction.rs 800+ 行) は許可、新 file 超過のみ catch
- 6 軸のうち DEP-001 / SIZE-001 / LOG-001 / meta-test を Phase 1-3、NAME / TYPE は phase 4+ 検討

## 5. TDD strict 3-phase outline

### Phase 1: Red (4 test)

**t1: t_no_new_src_file_over_800_lines** (SIZE-001)
- walk_src() + count_lines > 800、whitelist 適用後 violations.is_empty()
- whitelist: benchmark.rs / experiment.rs / compaction.rs

**t2: t_layer_order_no_upward_dep** (DEP-001)
- layer 順 `db < observability < safety < memory < knowledge < runtime < tools < agent < main`
- 各 src の `use crate::` を grep、上層依存検出

**t3: t_no_eprintln_in_production** (LOG-001、test fixture 除外)
- production code (cfg(test) 除く) の eprintln 直接使用検出
- whitelist: main.rs / vault_lint.rs (operator visibility 用途)

**t4: t_lint_error_messages_include_docs_link** (meta-test)
- panic message に `docs/architecture/` link 含有確証

**Phase 1 Red 検証**: 4 test FAIL (lint logic 未実装 or 現状 violation で fail)。

### Phase 2: Green (~150 LOC)

**tests/structural/ directory 新設**:
- `mod.rs`: walk_src / count_lines / module_of helpers
- `file_size.rs`: SIZE-001
- `layer_rules.rs`: DEP-001
- `no_eprintln.rs`: LOG-001
- `error_msg_format.rs`: meta-test

**Cargo.toml**: dev-dependencies に walkdir 追加。

**Phase 2 Green 検証**: 4 test PASS、1348→1352 passed (+4)。

### Phase 3: Refactor
- `docs/architecture/module-layer-rules.md` (Z-1 Phase 2 と連動)
- `docs/architecture/conventions.md` (SIZE/LOG/NAME/TYPE rules)
- clippy clean / fmt clean

## 6. Phase 4 CI 統合

`.github/workflows/ci.yml`:
```yaml
- run: cargo test --test structural -- --test-threads=1
```

## 7. Phase 5 smoke 検証基準

- **G-LL-1**: 既存 src/ 全件 PASS (whitelist 適用後)、`cargo test --test structural` 4 PASS
- **G-LL-2**: 試験的 violation 注入で fail 確証 → revert
- **G-LL-3**: 800 行超過 new file 注入で SIZE-001 fail
- **G-LL-4**: 既存 whitelist file の更なる増行は PASS

## 8. Rollback strategy
- `tests/structural/` directory 削除で完全 rollback
- whitelist test-only、production code 影響ゼロ

## 9. bonsai 既存資産

### 項目 244/246/251/254 lint pattern
4 件目の lint axis、AuditAction variant 追加検討 (phase 4+)。

### Z-1 plan (AGENTS.md + docs/)
`docs/architecture/module-layer-rules.md` (Z-1 Phase 2) が本案 rule source。

### Z-3 structural test との重複
本案 Phase 2 で Z-3 スコープ cover、Z-3 別 plan 不要 (本 plan に統合済)。

## 10. 記事との対応関係

| Zenn 概念 | bonsai 実装 |
|---|---|
| 6 種 linter (DEP/LOG/SIZE/NAME×2/TYPE) | tests/structural/ の 4 軸 (DEP/SIZE/LOG/meta)、NAME/TYPE は phase 6+ |
| エラー msg に修正方法 docs link | `[LINT:CODE] ... 修正方法: docs/...` panic message |
| custom linter (Node.js 例) | Rust integration test |
| レイヤー順違反検出 | DEP-001 (use crate:: 上層依存) |

## 11. 工数見積もり

| Phase | 内容 | 工数 |
|---|---|---|
| 1 Red | 4 test 作成 | 1h |
| 2 Green | tests/structural/ 実装 (~150 LOC) | 2-3h |
| 3 Refactor | docs/architecture/ 整備 + rustdoc | 1h |
| 4 CI 統合 | ci.yml 追加 | 30 min |
| 5 Smoke | G-LL-1..4 実機 | 30 min |
| **合計** | | **~5h** |

## 12. Open questions

1. layer 順確定: `safety vs memory` / `tools vs agent` 上下 (要 import audit)
2. whitelist 永続化 vs file 分割 (項目 248 Phase 5 と連動)
3. walkdir crate 追加 (test-only dev-dependencies)
4. NAME/TYPE 実装: Rust では generics/trait object、TS の any 等価機構不在 = scope limit
5. AuditAction::ArchitectureLint variant 追加検討 (phase 4+)

## 13. 次手

1. user 承認後 Phase 1 着手
2. Phase 2-3 順次 (~4h)
3. Phase 4 CI 統合
4. Phase 5 smoke G-LL-1..4

## 14. 関連項目
- 項目 244 (LLM Wiki Lint pattern)
- 項目 246/251/254 (vault_lint pattern)
- 推定項目番号: **項目 256** (Z-1 = 項目 255 と連動)

## 15. 関連 plan / memory
- `memory/zenn_codex_harness_learnings.md` (記事 8 Step + 5 案、本案 Z-3 + Z-4 統合)
- `.claude/plan/agents-md-docs-knowledge-base.md` (Z-1、docs/architecture/ source)
- `.claude/plan/vault-status-state-machine.md` (項目 254、lint pattern 雛形)
