# bonsai-agent Module Layer Rules

> Z-1 Phase 2 で新設 (項目 255)。Z-4 layer linter (`tests/structural.rs`、DEP-001) の rule source。
> 2026-06-04: クリーンアーキテクチャ準拠リファクタリングで `domain` 層を新設し
> WHITELIST_DEP を完全に空 (0 件) 化 (ADR-010)。

## Layer 順 (確定、DEP-001 linter で強制)

下層 (依存ゼロ) → 上層 (主機能):
1. **domain** — エンティティ/値オブジェクト/port trait。他層に依存しない純粋型のみ。
   conversation (Message/Role/Session/ToolCall/Attachment) / tool_schema (ToolSchema) /
   embedder (Embedder trait + SimpleEmbedder/FastEmbedder + cosine_similarity) /
   event (Event/EventType/TrajectoryCandidate/EventRepository trait + 純粋 event ロジック) /
   llm (LlmBackend trait + GenerateResult/TokenUsage + MockLlmBackend)
2. **db** — SQLite schema、migration (apply_all)
3. **observability** — audit_log、構造化 logger
4. **safety** — secrets filter、boot_guard、sandbox、network policy
5. **memory** — A-MEM store、experience、skill、search、graph、factcheck、decay、review、dreams
6. **knowledge** — extractor、vault、vault_lint
7. **runtime** — inference (FallbackBackend)、llama_server、cache、model_router
8. **tools** — Tool trait、ToolRegistry、shell/git/web/file/plugin/mcp/hooks/permission/sandbox
9. **agent** — agent_loop、benchmark、experiment、middleware、event_store (具象 EventStore)、compaction、checkpoint、task
10. **main** — CLI entry、bin

各 layer は **下層のみ** を `use crate::<下層>::*` 可能。上層への依存は禁止。

## 依存ルール (DEP-001)

```
domain < db < observability < safety < memory < knowledge < runtime < tools < agent < main
```

### 違反例
- `src/db/migrate.rs` が `use crate::agent::experiment::*` → 違反 (db は最下層に近く、agent に依存禁止)
- `src/memory/store.rs` が `use crate::tools::shell::*` → 違反 (memory < tools、上層依存)

### 例外
- `cancel`、`config` は cross-cutting concern (全 layer から read 可能、LAYER_ORDER 外)
- **test コードも DEP-001 の対象** (重要): `#[cfg(test)]` 配下の `use crate::<上層>` も違反として検出される。
  test fixture は層制約から除外**されない**。テストで上層の具象が必要な場合は、その型/trait を
  下層へ移すか、port trait + 下層モックで wire する (例: domain::llm::MockLlmBackend、
  memory::mocks::MockEventRepository)。
  ※ LOG-001 (eprintln 検出) のみ test ブロックを除外する。DEP-001 とは挙動が異なる点に注意。

## 修正方法

違反検出時:
1. 該当する型/trait (エンティティ・port) を下層 (多くは `domain`) へ移動
2. または該当機能を下層に再 implement (上層特有の処理は callback / DI で注入)
3. 具象 (SQLite/model_router 依存等) は上層に残し、port trait のみ下層へ (DIP)
4. cross-cutting concern なら `cancel` / `config` に移行検討

## Z-4 layer linter 連動

実装は `tests/structural.rs` の `t_layer_order_no_upward_dep` で:
```rust
const LAYER_ORDER: &[&str] = &[
    "domain", "db", "observability", "safety", "memory",
    "knowledge", "runtime", "tools", "agent", "main",
];

#[test]
fn t_layer_order_no_upward_dep() {
    // walk_src() + "use crate::<module>::" 抽出 + layer index 比較
    // 違反検出時 panic message: [LINT:DEP-001] src/X.rs ... 修正方法: docs/architecture/module-layer-rules.md
}
```

`WHITELIST_DEP` は **空 (0 件)**。新たな上向き依存を入れると即 FAIL する (regression gate)。
やむを得ず一時許容する場合のみ `(path, current_layer, imported_layer)` tuple を追加し、
follow-up で解消する。

## 解消済み (旧 Open questions)

ADR-010 のリファクタリングで以下が確定・解消:
1. `domain` 層を最下層に新設 (エンティティ + port を集約)。
2. 具象 (EventStore=SQLite / FallbackBackend=model_router) は上層残置、port (EventRepository /
   LlmBackend) は domain へ = DIP を徹底。
3. `safety vs memory` / `tools vs agent` の上下は現 LAYER_ORDER で確定 (上向き依存ゼロで成立)。
4. cross-cutting `cancel`/`config` は LAYER_ORDER 外として正式に扱う。

## 関連

- docs/architecture/overview.md ← module 一覧
- docs/decisions/ADR-010-clean-architecture-domain-layer.md ← 本リファクタリングの判断記録
- tests/structural.rs ← DEP-001 / SIZE-001 / LOG-001 linter 実装
