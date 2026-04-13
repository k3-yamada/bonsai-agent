use std::path::PathBuf;
pub struct BootGuard { failure_count: u32, threshold: u32, state_file: PathBuf }
impl BootGuard {
    pub fn new(threshold: u32) -> Self {
        let state_file = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("bonsai-agent").join("boot_state.json");
        let failure_count = Self::load_count(&state_file);
        Self { failure_count, threshold, state_file }
    }
    pub fn with_state_file(threshold: u32, state_file: PathBuf) -> Self {
        let failure_count = Self::load_count(&state_file);
        Self { failure_count, threshold, state_file }
    }
    pub fn should_safe_mode(&self) -> bool { self.failure_count >= self.threshold }
    pub fn record_success(&mut self) { self.failure_count = 0; self.save(); }
    pub fn record_failure(&mut self) { self.failure_count += 1; self.save(); }
    pub fn failure_count(&self) -> u32 { self.failure_count }
    fn save(&self) { let _ = std::fs::create_dir_all(self.state_file.parent().unwrap_or(&PathBuf::from("."))); let _ = std::fs::write(&self.state_file, format!("{{\"failures\":{}}}", self.failure_count)); }
    fn load_count(path: &PathBuf) -> u32 { std::fs::read_to_string(path).ok().and_then(|s| { let v: serde_json::Value = serde_json::from_str(&s).ok()?; v.get("failures")?.as_u64().map(|n| n as u32) }).unwrap_or(0) }
}
impl Default for BootGuard { fn default() -> Self { Self::new(3) } }
#[cfg(test)] mod tests { use super::*; use tempfile::TempDir;
    fn test_guard(threshold: u32) -> (BootGuard, TempDir) {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("boot_state.json");
        (BootGuard::with_state_file(threshold, file), dir)
    }
#[test] fn t_new() { let (g, _d) = test_guard(3); assert!(!g.should_safe_mode()); }
#[test] fn t_trip() { let (mut g, _d) = test_guard(2); g.record_failure(); g.record_failure(); assert!(g.should_safe_mode()); }
#[test] fn t_reset() { let (mut g, _d) = test_guard(2); g.record_failure(); g.record_failure(); g.record_success(); assert!(!g.should_safe_mode()); } }
