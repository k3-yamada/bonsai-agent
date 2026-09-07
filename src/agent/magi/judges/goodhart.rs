//! Judge A: Goodhart's Law 形骸化監視 Judge
//!
//! `MetricConsistencyChecker` を活用し、指標の形骸化や異常な単調増加を検知する。

use std::sync::{Arc, Mutex};

use crate::agent::magi::panel::{Judge, JudgeVerdict, ResponseContext};
use crate::memory::consistency::{GoodhartRisk, MetricConsistencyChecker};

pub struct GoodhartJudge {
    checker: Arc<Mutex<MetricConsistencyChecker>>,
}

impl GoodhartJudge {
    pub fn new(checker: Arc<Mutex<MetricConsistencyChecker>>) -> Self {
        Self { checker }
    }
}

impl Judge for GoodhartJudge {
    fn name(&self) -> &'static str {
        "Judge A (Goodhart's Law)"
    }

    fn evaluate(&self, _ctx: &ResponseContext) -> JudgeVerdict {
        let risk = if let Ok(c) = self.checker.lock() {
            c.detect_goodhart_pattern()
        } else {
            GoodhartRisk::Acceptable
        };

        match risk {
            GoodhartRisk::Acceptable => JudgeVerdict::Clear,
            GoodhartRisk::Suspicious(reason) => JudgeVerdict::Concern(reason),
            GoodhartRisk::Critical(reason) => JudgeVerdict::Block(reason),
        }
    }
}
