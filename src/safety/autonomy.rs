use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel { ReadOnly, #[default]
Supervised, Full }
impl AutonomyLevel {
    pub fn can_write(&self) -> bool { !matches!(self, Self::ReadOnly) }
    pub fn needs_confirmation(&self, is_destructive: bool) -> bool {
        match self { Self::ReadOnly => true, Self::Supervised => is_destructive, Self::Full => false }
    }
}
#[cfg(test)] mod tests { use super::*;
#[test] fn t_ro() { let l = AutonomyLevel::ReadOnly; assert!(!l.can_write()); assert!(l.needs_confirmation(false)); }
#[test] fn t_sv() { let l = AutonomyLevel::Supervised; assert!(l.can_write()); assert!(l.needs_confirmation(true)); assert!(!l.needs_confirmation(false)); }
#[test] fn t_full() { let l = AutonomyLevel::Full; assert!(l.can_write()); assert!(!l.needs_confirmation(true)); }
#[test] fn t_default() { assert_eq!(AutonomyLevel::default(), AutonomyLevel::Supervised); }
#[test] fn t_serde() { let j = serde_json::to_string(&AutonomyLevel::Full).unwrap(); assert_eq!(j, "\"full\""); } }
