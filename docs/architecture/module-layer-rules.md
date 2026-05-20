# bonsai-agent Module Layer Rules

> Z-1 Phase 2 で新設 (項目 255)。Z-4 layer linter (`tests/structural/layer_rules.rs`、項目 256 候補) の rule source。

## Layer 順 (提案、Z-4 implementation で確定)

下層 (依存ゼロ) → 上層 (主機能):
1. **db** — SQLite schema、migration
2. **observability** — audit_log、構造化 logger
3. **safety** — secrets filter、boot_guard、sandbox、network policy
4. **memory** — A-MEM store、experience、skill、search、graph、factcheck、decay、review、dreams
5. **knowledge** — extractor、vault、vault_lint
6. **runtime** — inference、llama_server、cache、embedder、model_router
7. **tools** — Tool trait、ToolRegistry、shell/git/web/file/plugin/mcp/hooks/permission/sandbox
8. **agent** — agent_loop、benchmark、experiment、middleware、event_store、compaction、checkpoint、task
9. **main** — CLI entry、bin

各 layer は **下層のみ** を `use crate::<下層>::*` 可能。上層への依存は禁止。

## 依存ルール (DEP-001)

```
db < observability < safety < memory < knowledge < runtime < tools < agent < main
```

### 違反例
- `src/db/migrate.rs` が `use crate::agent::experiment::*` → 違反 (db は最下層、agent に依存禁止)
- `src/memory/store.rs` が `use crate::tools::shell::*` → 違反 (memory < tools、上層依存)

### 例外
- `cancel`、`config` は cross-cutting concern (全 layer から read 可能)
- test fixture (`#[cfg(test)]`) は層制約から除外

## 修正方法

違反検出時:
1. 該当 file を下層へ移動
2. または該当機能を下層に再 implement (上層特有の処理は callback で注入)
3. cross-cutting concern なら `cancel` / `config` に移行検討

## Z-4 layer linter 連動

実装は `tests/structural/layer_rules.rs` (Z-4 plan、項目 256 候補) で:
```rust
const LAYER_ORDER: &[&str] = &["db", "observability", "safety", "memory", "knowledge", "runtime", "tools", "agent", "main"];

#[test]
fn t_layer_order_no_upward_dep() {
    // walk_src() + grep "use crate::<module>::" + layer index 比較
    // 違反検出時 panic message: [LINT:DEP-001] src/X.rs ... 修正方法: docs/architecture/module-layer-rules.md
}
```

## Open questions (Z-4 plan §12)

1. `safety vs memory` の上下確定 (要 import audit)
2. `tools vs agent` の依存方向 (現状: tools が agent から呼ばれる片方向想定、circular 検出時 plan 修正)
3. cross-cutting `cancel`/`config` の正式扱い

## 関連

- docs/architecture/overview.md ← module 一覧
- .claude/plan/layer-architecture-linter.md (Z-4 + Z-3 統合 plan、項目 256 候補)
- 推定項目番号: **項目 256** (Z-4 実装時に正式 ADR 化)
