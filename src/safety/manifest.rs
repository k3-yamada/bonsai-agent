pub struct Capability {
    pub name: &'static str,
    pub enabled: bool,
    pub description: &'static str,
}
pub fn build_manifest() -> Vec<Capability> {
    vec![
        Capability {
            name: "shell",
            enabled: true,
            description: "シェルコマンド実行",
        },
        Capability {
            name: "file_read",
            enabled: true,
            description: "ファイル読み取り",
        },
        Capability {
            name: "file_write",
            enabled: true,
            description: "ファイル書き込み",
        },
        Capability {
            name: "git",
            enabled: true,
            description: "Git操作",
        },
        Capability {
            name: "web_search",
            enabled: true,
            description: "Web検索",
        },
        Capability {
            name: "web_fetch",
            enabled: true,
            description: "URL取得",
        },
        Capability {
            name: "repo_map",
            enabled: true,
            description: "コード構造マップ",
        },
        Capability {
            name: "plugin",
            enabled: true,
            description: "TOMLプラグイン",
        },
        Capability {
            name: "mcp",
            enabled: true,
            description: "MCPクライアント",
        },
        Capability {
            name: "embeddings",
            enabled: cfg!(feature = "embeddings"),
            description: "ベクトル埋め込み(fastembed)",
        },
    ]
}
pub fn format_manifest() -> String {
    let caps = build_manifest();
    let mut out = String::from("bonsai-agent capabilities:\n");
    for c in &caps {
        out.push_str(&format!(
            "  [{}] {} — {}\n",
            if c.enabled { "✓" } else { "✗" },
            c.name,
            c.description
        ));
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn t_manifest() {
        let m = build_manifest();
        assert!(m.len() >= 8);
        assert!(m.iter().any(|c| c.name == "shell" && c.enabled));
    }
    #[test]
    fn t_format() {
        let s = format_manifest();
        assert!(s.contains("shell"));
        assert!(s.contains("✓"));
    }
}
