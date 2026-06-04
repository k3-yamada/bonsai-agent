//! ドメイン層 — 最下層のエンティティ/値オブジェクト群。
//! cross-cutting concern (`cancel` / `config`) を除き他層に依存しない純粋な型・port のみを
//! 置く (Clean Architecture の Entities + Ports)。具象実装は上層に残す (DIP)。
//! layer 順: domain < db < observability < ... (DEP-001、module-layer-rules.md)。

pub mod conversation;
pub mod embedder;
pub mod event;
pub mod llm;
pub mod tool_schema;
