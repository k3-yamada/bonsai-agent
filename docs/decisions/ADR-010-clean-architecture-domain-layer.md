# ADR-010: クリーンアーキテクチャ準拠 — domain 層新設と DEP-001 違反ゼロ化

## Status: Accepted (2026-06-04)

## Context

DEP-001 layer linter (`tests/structural.rs`、項目 256) は層順
`db < observability < safety < memory < knowledge < runtime < tools < agent < main`
を強制するが、導入時点で 16 件の上向き依存違反が `WHITELIST_DEP` で許容 (grandfather)
されていた (項目 1488 audit で「全件 legitimate」と確認済だが、本質的には未解消の技術的負債)。

違反の主因は **エンティティ/port trait が上位層 (agent/runtime/tools) に置かれ、
下位層 (memory/observability) がそれを参照していた** こと (Entity Leakage):

1. memory→agent: `agent::conversation::{Message,Session,Role,ToolCall}`
2. runtime→agent: 同上 Message
3. runtime→tools: `tools::ToolSchema` (純粋 DTO)
4. memory→runtime: `runtime::embedder::*`
5. memory→agent: `agent::event_store::{Event,EventType,EventRepository,TrajectoryCandidate,...}`
6. memory→runtime: `runtime::inference::LlmBackend` (推論 port)
7. observability→memory: audit.rs test の `MemoryStore::in_memory`

外部レビュー (ccg = Codex + Gemini) で「この規模 (95 ファイル/1372 テスト) で
最下層 `domain` を新設してエンティティ + port を集約するのは over-engineering ではなく
適切」「具象は上層残置・port のみ下層へ (DIP)」との合意を得た。

## Decision

### 最下層 `domain` を新設

層順を `domain < db < observability < ...` に変更。`domain` は**他層に一切依存しない
純粋な型・port trait のみ**を持つ (Clean Architecture の Entities + Ports)。
`cancel` / `config` は従来どおり cross-cutting (LAYER_ORDER 外、全層参照可)。

移動した内容 (6 phase、各 phase 独立コミット):
- **conversation** → `domain::conversation` (Message/Role/Session/ToolCall/Attachment)
- **tool_schema** → `domain::tool_schema` (ToolSchema DTO。Tool trait=振る舞いは tools 残置)
- **embedder** → `domain::embedder` (Embedder trait + SimpleEmbedder/FastEmbedder + cosine。
  crate 依存ゼロの foundational service)
- **event** → `domain::event` (Event/EventType/TrajectoryCandidate/EventRepository trait +
  純粋 event 走査ロジック。具象 EventStore<'a>=SQLite は agent 残置)
- **llm** → `domain::llm` (LlmBackend trait + GenerateResult/TokenUsage + MockLlmBackend。
  具象 FallbackBackend=model_router 依存は runtime 残置)

### DIP の徹底 — 具象は上層、port は下層

- `EventRepository` (port) を domain へ、`EventStore<'a>` (SQLite 具象) は agent に残す。
  agent→domain は下向きで合法。memory は domain の port/型のみ参照。
- `LlmBackend` (port) + `MockLlmBackend` (参照モック) を domain へ、`FallbackBackend`
  (model_router 依存の具象) は runtime に残す。

### test も DEP-001 の対象

audit.rs の observability→memory 違反は **production ではなく test 専用**
(`MemoryStore::in_memory()` で Connection 準備) だった。DEP-001 は test コードも
検出する (LOG-001 のみ test 除外)。db 層に `migrate::apply_all(&Connection)` を新設し、
test が memory を経由せず fresh DB を構築できるようにして解消。
module-layer-rules.md の「test fixture は層制約から除外」という旧記述は
**実装と乖離していた誤り**であり、本 ADR で訂正した (test も対象が正)。

### WHITELIST_DEP を空に

全 16 違反を解消し `const WHITELIST_DEP: &[(&str,&str,&str)] = &[];` (0 件) とした。
以降は新たな上向き依存が即 FAIL する regression gate として機能する。

## Consequences

**Positive**:
- レイヤー違反が物理的にゼロ。エンティティ + port が `domain` に集約され、依存方向が
  一方向 (下向き) に厳格化。将来 crate 分割する場合 `bonsai-domain` が自然な抽出単位。
- DIP により memory/observability が具象 (SQLite/model_router) を知らずに済む。
- `WHITELIST_DEP = &[]` で「気付かぬ層侵食」を CI (structural test) が即検出。
- doc/impl の乖離 (test 除外の誤記) を解消し high-fidelity 化。

**Negative / Trade-off**:
- `domain` に trait + 参照モック (MockLlmBackend) という「振る舞い」も置いた。純粋 DTO
  限定という理想からは緩い。ただし port の参照実装を core が持つのは DIP の標準形であり、
  memory 等の下位層がモックを下向き参照できる利点が上回る。
- 機械的 import 更新が広範 (約 60 ファイル touch、6 commit)。各 phase で
  lib 1434 test + structural + clippy + fmt 全 green を確認し退行ゼロを担保。
- 1bit モデルの推論挙動・production binary は一切不変 (型の所在変更のみ、ADR-002
  「Scaffolding > Model」と整合、ADR-003 paired evidence 対象外の no-op refactor)。

## Related

- ADR-002 (Scaffolding > Model — production 挙動不変の構造改善)
- docs/architecture/module-layer-rules.md (層順・DEP-001 ルール本体、本 ADR で更新)
- tests/structural.rs (DEP-001 linter、WHITELIST_DEP = &[])
- 検証: ccg (Codex + Gemini) によるアーキテクチャ戦略レビュー
- commit: conversation/ToolSchema/embedder/event_store/audit/LlmBackend の 6 refactor commit
