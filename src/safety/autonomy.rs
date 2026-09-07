use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel {
    ReadOnly,
    #[default]
    Supervised,
    Full,
}
impl AutonomyLevel {
    pub fn can_write(&self) -> bool {
        !matches!(self, Self::ReadOnly)
    }
    pub fn needs_confirmation(&self, is_destructive: bool) -> bool {
        match self {
            Self::ReadOnly => true,
            Self::Supervised => is_destructive,
            Self::Full => false,
        }
    }
}

impl std::str::FromStr for AutonomyLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "readonly" | "read_only" | "read-only" => Ok(Self::ReadOnly),
            "supervised" => Ok(Self::Supervised),
            "full" => Ok(Self::Full),
            other => Err(format!(
                "Unknown autonomy level: '{other}'. Valid options: readonly, supervised, full"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn t_ro() {
        let l = AutonomyLevel::ReadOnly;
        assert!(!l.can_write());
        assert!(l.needs_confirmation(false));
    }
    #[test]
    fn t_sv() {
        let l = AutonomyLevel::Supervised;
        assert!(l.can_write());
        assert!(l.needs_confirmation(true));
        assert!(!l.needs_confirmation(false));
    }
    #[test]
    fn t_full() {
        let l = AutonomyLevel::Full;
        assert!(l.can_write());
        assert!(!l.needs_confirmation(true));
    }
    #[test]
    fn t_default() {
        assert_eq!(AutonomyLevel::default(), AutonomyLevel::Supervised);
    }
    #[test]
    fn t_serde() {
        let j = serde_json::to_string(&AutonomyLevel::Full).unwrap();
        assert_eq!(j, "\"full\"");
    }
    #[test]
    fn t_from_str() {
        assert_eq!(
            AutonomyLevel::from_str("readonly").unwrap(),
            AutonomyLevel::ReadOnly
        );
        assert_eq!(
            AutonomyLevel::from_str("read_only").unwrap(),
            AutonomyLevel::ReadOnly
        );
        assert_eq!(
            AutonomyLevel::from_str("READ-ONLY").unwrap(),
            AutonomyLevel::ReadOnly
        );
        assert_eq!(
            AutonomyLevel::from_str("supervised").unwrap(),
            AutonomyLevel::Supervised
        );
        assert_eq!(
            AutonomyLevel::from_str("SUPERVISED").unwrap(),
            AutonomyLevel::Supervised
        );
        assert_eq!(
            AutonomyLevel::from_str("full").unwrap(),
            AutonomyLevel::Full
        );
        assert_eq!(
            AutonomyLevel::from_str("FULL").unwrap(),
            AutonomyLevel::Full
        );
        assert!(AutonomyLevel::from_str("unknown").is_err());
    }
}
