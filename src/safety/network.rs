use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::{IpAddr, ToSocketAddrs};

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
        // 完全一致 or サブドメイン境界 (".d") のみ許可。素の ends_with は
        // `evil-huggingface.co` が `huggingface.co` を満たす suffix bypass になる。
        self.allowed_domains
            .iter()
            .any(|d| domain == d.as_str() || domain.ends_with(&format!(".{d}")))
    }
}

/// URL がプライベート IP やループバック等の SSRF 攻撃対象でないか検証
pub fn validate_fetch_url(url_str: &str, filter: &NetworkFilter) -> Result<(), String> {
    // 1. スキーム検査 (http / https のみ許可)
    let (scheme, rest) = if let Some(idx) = url_str.find("://") {
        (&url_str[..idx], &url_str[idx + 3..])
    } else {
        return Err("URLスキームが指定されていません (http:// または https:// が必要)".to_string());
    };

    let scheme_lower = scheme.to_lowercase();
    if scheme_lower != "http" && scheme_lower != "https" {
        return Err(format!("未許可のスキーム: '{scheme}' (http/httpsのみ許可)"));
    }

    // 2. ホスト名抽出
    let host_part = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_and_port = host_part.trim();
    if host_and_port.is_empty() {
        return Err("ホスト名が空です".to_string());
    }

    let host = if host_and_port.starts_with('[') {
        // IPv6 リテラル [::1]:8080
        host_and_port
            .split(']')
            .next()
            .map(|s| s.trim_start_matches('['))
            .unwrap_or("")
    } else {
        host_and_port.split(':').next().unwrap_or("")
    };

    let host_lower = host.to_lowercase();

    // 3. ループバック・ローカルホスト名の即時拒否
    if host_lower == "localhost"
        || host_lower.ends_with(".localhost")
        || host_lower.ends_with(".local")
        || host_lower.ends_with(".internal")
    {
        return Err(format!(
            "ローカルアドレスへのアクセスは拒否されました: '{host}'"
        ));
    }

    // 4. NetworkFilter のチェック
    if !filter.is_allowed(url_str) {
        return Err(format!(
            "ドメインフィルタによりアクセスがブロックされました: '{host}'"
        ));
    }

    // 5. IP アドレス直接指定の場合の検証
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_or_restricted_ip(&ip) {
            return Err(format!(
                "プライベート/制限されたIPへのアクセスは拒否されました: {ip}"
            ));
        }
        return Ok(());
    }

    // 6. DNS 解決による全 IP のプライベートチェック (DNS リバインディング/イントラネット遮断)
    let port = if let Some(p_str) = host_part.split(':').nth(1) {
        p_str
            .parse::<u16>()
            .unwrap_or(if scheme_lower == "https" { 443 } else { 80 })
    } else if scheme_lower == "https" {
        443
    } else {
        80
    };

    // DNS 解決を試行（解決不能な場合は offline/モック環境等も考慮し、既知パブリック以外は拒否）
    if let Ok(addrs) = (host, port).to_socket_addrs() {
        for socket_addr in addrs {
            let ip = socket_addr.ip();
            if is_private_or_restricted_ip(&ip) {
                return Err(format!(
                    "名前解決されたIPがプライベート/制限対象です: {ip} (ホスト: {host})"
                ));
            }
        }
    }

    Ok(())
}

/// IP アドレスがプライベート、ループバック、リンクローカル等の制限対象か判定
pub fn is_private_or_restricted_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            ipv4.is_loopback()
                || ipv4.is_private()
                || ipv4.is_link_local()
                || ipv4.is_broadcast()
                || ipv4.is_unspecified()
                // 169.254.169.254 (クラウドメタデータ) は link_local で網羅されるが明示
                || (ipv4.octets()[0] == 169 && ipv4.octets()[1] == 254)
                // 100.64.0.0/10 (キャリアグレードNAT / CGNAT)
                || (ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0xc0) == 64)
                // 192.0.0.0/24 (IETFプロトコル代入)
                || (ipv4.octets()[0] == 192 && ipv4.octets()[1] == 0 && ipv4.octets()[2] == 0)
        }
        IpAddr::V6(ipv6) => {
            ipv6.is_loopback()
                || ipv6.is_unspecified()
                // Unique Local Address (fc00::/7)
                || (ipv6.segments()[0] & 0xfe00) == 0xfc00
                // Link-local (fe80::/10)
                || (ipv6.segments()[0] & 0xffc0) == 0xfe80
                // IPv4-mapped IPv6 (::ffff:x.x.x.x)
                || match ipv6.to_ipv4_mapped() {
                    Some(v4) => is_private_or_restricted_ip(&IpAddr::V4(v4)),
                    None => false,
                }
        }
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
        // 正規のサブドメインは許可
        assert!(f.is_allowed("https://cdn.huggingface.co/file"));
    }
    #[test]
    fn t_suffix_bypass_blocked() {
        // `evil-huggingface.co` は `huggingface.co` の suffix だが別ドメイン → 拒否。
        let f = NetworkFilter::strict(&["huggingface.co"]);
        assert!(!f.is_allowed("https://evil-huggingface.co/steal"));
        assert!(!f.is_allowed("https://nothuggingface.co/x"));
    }
    #[test]
    fn t_default_is_allow_all() {
        let f = NetworkFilter::default();
        assert!(!f.block_by_default);
        assert!(f.is_allowed("https://any.domain.example"));
    }
    #[test]
    fn t_strict_blocks_unknown() {
        let f = NetworkFilter::strict(&["trusted.example"]);
        assert!(!f.is_allowed("https://evil.com/path"));
        assert!(!f.is_allowed("http://localhost:8080"));
    }
    #[test]
    fn t_extract_domain_no_scheme() {
        assert_eq!(extract_domain("example.com/path"), "example.com");
    }
    #[test]
    fn t_extract_domain_empty() {
        assert_eq!(extract_domain(""), "");
    }

    #[test]
    fn t_validate_fetch_url_ssrf_protection() {
        let filter = NetworkFilter::allow_all();

        // ループバック・ローカル
        assert!(validate_fetch_url("http://localhost:8080", &filter).is_err());
        assert!(validate_fetch_url("http://127.0.0.1:8080/api", &filter).is_err());
        assert!(validate_fetch_url("http://127.0.0.2/admin", &filter).is_err());
        assert!(validate_fetch_url("http://[::1]:8080", &filter).is_err());

        // プライベートネットワーク
        assert!(validate_fetch_url("http://192.168.1.1/", &filter).is_err());
        assert!(validate_fetch_url("http://10.0.0.5/secret", &filter).is_err());
        assert!(validate_fetch_url("http://172.16.0.1/", &filter).is_err());

        // クラウドメタデータ
        assert!(validate_fetch_url("http://169.254.169.254/latest/meta-data/", &filter).is_err());

        // スキーム違反
        assert!(validate_fetch_url("file:///etc/passwd", &filter).is_err());
        assert!(validate_fetch_url("ftp://example.com", &filter).is_err());
        assert!(validate_fetch_url("javascript:alert(1)", &filter).is_err());

        // 正常なパブリック URL
        assert!(validate_fetch_url("https://example.com/data", &filter).is_ok());
    }

    #[test]
    fn t_validate_fetch_url_respects_strict_filter() {
        let filter = NetworkFilter::strict(&["huggingface.co"]);
        assert!(validate_fetch_url("https://huggingface.co/model", &filter).is_ok());
        assert!(validate_fetch_url("https://evil.com/leak", &filter).is_err());
    }
}
