//! ドメイン層 — 最下層のエンティティ/値オブジェクト群。
//! 他層に依存しない純粋な型のみを置く (Clean Architecture の Entities)。
//! layer 順: domain < db < observability < ... (DEP-001、module-layer-rules.md)。

pub mod conversation;
pub mod tool_schema;
