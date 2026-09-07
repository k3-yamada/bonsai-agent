//! DMN (Default Mode Network) 自発思考ループモジュール
//!
//! 会話ターンを待たずに手持ちの記憶を自発的に反芻し、
//! 黄金比ジッタースケジューラと三段階ゲートにより、
//! 新規性・重要度の高い洞察を統合記憶に還元する。

pub mod generator;
pub mod outcome;
pub mod scheduler;
pub mod worker;

pub use generator::DmnGenerator;
pub use outcome::DmnOutcome;
pub use scheduler::DmnScheduler;
pub use worker::{DmnRunner, DmnWorker};
