//! Test 用 mock 実装群 (Clean Architecture Repository pattern、項目 209)。
//!
//! Production binary 込み (size 影響軽微 ~150 行)、ERL / Self-Verify 等の
//! 別モジュール test からも `bonsai_agent::memory::mocks::*` で利用可。

pub mod event_repository_mock;

pub use event_repository_mock::MockEventRepository;
