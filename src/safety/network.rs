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
    /// 環境変数 `BONSAI_ALLOWED_DOMAINS`（カンマ区切り）からフィルタを生成。
    /// 設定されていれば strict、未設定なら default（全許可、SSRF防護は常時有効）
    pub fn from_env() -> Self {
        if let Ok(domains_str) = std::env::var("BONSAI_ALLOWED_DOMAINS") {
            let domains: Vec<&str> = domains_str
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if !domains.is_empty() {
                return Self::strict(&domains);
            }
        }
        Self::default()
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
///
/// 名前解決には OS の `ToSocketAddrs` を使用し、DNS 失敗時は fail-closed (Err) となる。
pub fn validate_fetch_url(url_str: &str, filter: &NetworkFilter) -> Result<(), String> {
    validate_fetch_url_with_resolver(url_str, filter, |host, port| {
        (host, port)
            .to_socket_addrs()
            .map(|iter| iter.map(|s| s.ip()).collect())
    })
}

/// リゾルバ関数を注入可能な URL 検証関数（テストおよびカスタム DNS 検査用）
pub fn validate_fetch_url_with_resolver<F>(
    url_str: &str,
    filter: &NetworkFilter,
    resolve: F,
) -> Result<(), String>
where
    F: Fn(&str, u16) -> std::io::Result<Vec<IpAddr>>,
{
    // 1. WHATWG 準拠の URL パース (user:pass@host 等の userinfo を正しく分離)
    let parsed = reqwest::Url::parse(url_str).map_err(|e| format!("無効なURL形式です: {e}"))?;

    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("未許可のスキーム: '{scheme}' (http/httpsのみ許可)"));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| "ホスト名が空です".to_string())?;
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });

    // 2. ループバック・ローカルホスト名の即時拒否
    let host_lower = host.to_lowercase();
    if host_lower == "localhost"
        || host_lower.ends_with(".localhost")
        || host_lower.ends_with(".local")
        || host_lower.ends_with(".internal")
    {
        return Err(format!(
            "ローカルアドレスへのアクセスは拒否されました: '{host}'"
        ));
    }

    // 3. NetworkFilter のチェック
    if !filter.is_allowed(url_str) {
        return Err(format!(
            "ドメインフィルタによりアクセスがブロックされました: '{host}'"
        ));
    }

    // 4. IP アドレス直接指定の場合の検証 (IPv6 の角括弧を除去してパース)
    let clean_ip_str = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = clean_ip_str.parse::<IpAddr>() {
        if is_private_or_restricted_ip(&ip) {
            return Err(format!(
                "プライベート/制限されたIPへのアクセスは拒否されました: {ip}"
            ));
        }
        return Ok(());
    }

    // 5. DNS 解決による全 IP のプライベートチェック (DNS リバインディング/イントラネット遮断)
    // DNS 解決失敗時は fail-closed (SSRF 穴防止)
    let ips = resolve(host, port)
        .map_err(|e| format!("ホスト '{host}' の名前解決に失敗しました (fail-closed): {e}"))?;

    if ips.is_empty() {
        return Err(format!(
            "ホスト '{host}' の名前解決結果が空です (fail-closed)"
        ));
    }

    for ip in ips {
        if is_private_or_restricted_ip(&ip) {
            return Err(format!(
                "名前解決されたIPがプライベート/制限対象です: {ip} (ホスト: {host})"
            ));
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

        // 正常なパブリック IP URL (DNS 不要)
        assert!(validate_fetch_url("https://93.184.216.34/data", &filter).is_ok());
        assert!(validate_fetch_url("https://1.1.1.1/dns-query", &filter).is_ok());

        // userinfo (user:pass@host) 形式でのパストラバーサル・SSRF 試行
        assert!(validate_fetch_url("http://user:pass@127.0.0.1:8080/admin", &filter).is_err());
        assert!(validate_fetch_url("http://admin:secret@192.168.1.1/cfg", &filter).is_err());
        assert!(validate_fetch_url("http://user:pass@localhost:3000/", &filter).is_err());
        assert!(validate_fetch_url("http://user:pass@93.184.216.34/data", &filter).is_ok());

        // DNS 解決失敗時は fail-closed (Err)
        assert!(
            validate_fetch_url("https://nonexistent-nxdomain-test-domain.invalid/", &filter)
                .is_err()
        );
    }

    #[test]
    fn t_validate_fetch_url_resolver_mock() {
        let filter = NetworkFilter::allow_all();

        // 1. パブリック IP に解決される正常ケース
        let res = validate_fetch_url_with_resolver("https://example.com/api", &filter, |h, _| {
            assert_eq!(h, "example.com");
            Ok(vec!["93.184.216.34".parse().unwrap()])
        });
        assert!(res.is_ok());

        // 2. プライベート IP に解決されるケース (DNS リバインディング等) -> 遮断
        let res =
            validate_fetch_url_with_resolver("https://rebinding.evil.com/leak", &filter, |_, _| {
                Ok(vec!["127.0.0.1".parse().unwrap()])
            });
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("プライベート/制限対象"));

        // 3. 複数 A/AAAA レコードのうち 1 つでもプライベート IP が混入している場合 -> 遮断
        let res =
            validate_fetch_url_with_resolver("https://dual-ip.evil.com/leak", &filter, |_, _| {
                Ok(vec![
                    "93.184.216.34".parse().unwrap(),
                    "10.0.0.1".parse().unwrap(),
                ])
            });
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("プライベート/制限対象"));

        // 4. 名前解決エラー時の fail-closed 検証
        let res =
            validate_fetch_url_with_resolver("https://failed-dns.example.com/", &filter, |_, _| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "NXDOMAIN",
                ))
            });
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .contains("名前解決に失敗しました (fail-closed)")
        );

        // 5. userinfo 付きでもホストが正しくリゾルバに渡されること
        let res = validate_fetch_url_with_resolver(
            "https://user:password@target.example.org:8443/resource",
            &filter,
            |h, port| {
                assert_eq!(h, "target.example.org");
                assert_eq!(port, 8443);
                Ok(vec!["93.184.216.34".parse().unwrap()])
            },
        );
        assert!(res.is_ok());
    }

    #[test]
    fn t_validate_fetch_url_respects_strict_filter() {
        let filter = NetworkFilter::strict(&["huggingface.co"]);
        let res_ok =
            validate_fetch_url_with_resolver("https://huggingface.co/model", &filter, |_, _| {
                Ok(vec!["93.184.216.34".parse().unwrap()])
            });
        assert!(res_ok.is_ok());

        let res_err = validate_fetch_url_with_resolver("https://evil.com/leak", &filter, |_, _| {
            Ok(vec!["93.184.216.34".parse().unwrap()])
        });
        assert!(res_err.is_err());
    }
}
