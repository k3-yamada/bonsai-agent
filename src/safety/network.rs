use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkFilter {
    pub allowed_domains: HashSet<String>,
    pub block_by_default: bool,
}
impl NetworkFilter {
    pub fn allow_all() -> Self {
        Self {
            allowed_domains: HashSet::new(),
            block_by_default: false,
        }
    }
    pub fn strict(domains: &[&str]) -> Self {
        Self {
            allowed_domains: domains.iter().map(|d| d.to_string()).collect(),
            block_by_default: true,
        }
    }
    pub fn is_allowed(&self, url: &str) -> bool {
        if !self.block_by_default {
            return true;
        }
        let domain = extract_domain(url);
        self.allowed_domains
            .iter()
            .any(|d| domain.ends_with(d.as_str()))
    }
}
fn extract_domain(url: &str) -> String {
    url.split("//")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase()
}
impl Default for NetworkFilter {
    fn default() -> Self {
        Self::allow_all()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn t_allow_all() {
        let f = NetworkFilter::allow_all();
        assert!(f.is_allowed("https://evil.com"));
    }
    #[test]
    fn t_strict() {
        let f = NetworkFilter::strict(&["api.duckduckgo.com", "huggingface.co"]);
        assert!(f.is_allowed("https://api.duckduckgo.com/query"));
        assert!(!f.is_allowed("https://evil.com/steal"));
    }
    #[test]
    fn t_domain() {
        assert_eq!(
            extract_domain("https://example.com:8080/path"),
            "example.com"
        );
    }
    #[test]
    fn t_subdomain() {
        let f = NetworkFilter::strict(&["huggingface.co"]);
        assert!(f.is_allowed("https://huggingface.co/api"));
    }
}
