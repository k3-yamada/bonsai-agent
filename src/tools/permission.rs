use serde::{Deserialize, Serialize};

/// ツール実行の権限レベル
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// 確認なしで実行
    Auto,
    /// ユーザー確認後に実行
    Confirm,
    /// 実行禁止
    Deny,
}

/// デーモンモード時の権限ポリシー
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonPolicy {
    /// Auto権限のツールのみ実行
    AutoOnly,
    /// Confirm要求をDBキューに溜める
    QueueForHuman,
}

/// 権限チェックの結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// 実行OK
    Allow,
    /// ユーザー確認が必要
    NeedConfirmation,
    /// キューに溜める（デーモンモード）
    QueueForLater,
    /// 実行拒否
    Denied,
}

/// 権限チェックを実行する
pub fn check_permission(
    permission: Permission,
    is_daemon: bool,
    daemon_policy: DaemonPolicy,
) -> PermissionDecision {
    match permission {
        Permission::Auto => PermissionDecision::Allow,
        Permission::Confirm => {
            if is_daemon {
                match daemon_policy {
                    DaemonPolicy::AutoOnly => PermissionDecision::Denied,
                    DaemonPolicy::QueueForHuman => PermissionDecision::QueueForLater,
                }
            } else {
                PermissionDecision::NeedConfirmation
            }
        }
        Permission::Deny => PermissionDecision::Denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 対話モード: Auto → Allow
    #[test]
    fn test_interactive_auto() {
        let result = check_permission(Permission::Auto, false, DaemonPolicy::AutoOnly);
        assert_eq!(result, PermissionDecision::Allow);
    }

    // 対話モード: Confirm → NeedConfirmation
    #[test]
    fn test_interactive_confirm() {
        let result = check_permission(Permission::Confirm, false, DaemonPolicy::AutoOnly);
        assert_eq!(result, PermissionDecision::NeedConfirmation);
    }

    // 対話モード: Deny → Denied
    #[test]
    fn test_interactive_deny() {
        let result = check_permission(Permission::Deny, false, DaemonPolicy::AutoOnly);
        assert_eq!(result, PermissionDecision::Denied);
    }

    // デーモン + AutoOnly: Auto → Allow
    #[test]
    fn test_daemon_auto_only_auto() {
        let result = check_permission(Permission::Auto, true, DaemonPolicy::AutoOnly);
        assert_eq!(result, PermissionDecision::Allow);
    }

    // デーモン + AutoOnly: Confirm → Denied（人間不在なのでブロック）
    #[test]
    fn test_daemon_auto_only_confirm() {
        let result = check_permission(Permission::Confirm, true, DaemonPolicy::AutoOnly);
        assert_eq!(result, PermissionDecision::Denied);
    }

    // デーモン + QueueForHuman: Confirm → QueueForLater
    #[test]
    fn test_daemon_queue_confirm() {
        let result = check_permission(Permission::Confirm, true, DaemonPolicy::QueueForHuman);
        assert_eq!(result, PermissionDecision::QueueForLater);
    }

    // デーモン: Deny → Denied（ポリシーに関係なく常に拒否）
    #[test]
    fn test_daemon_deny() {
        let result = check_permission(Permission::Deny, true, DaemonPolicy::QueueForHuman);
        assert_eq!(result, PermissionDecision::Denied);
    }

    // シリアライズ
    #[test]
    fn test_permission_serialization() {
        let json = serde_json::to_string(&Permission::Confirm).unwrap();
        assert_eq!(json, "\"confirm\"");
        let p: Permission = serde_json::from_str("\"auto\"").unwrap();
        assert_eq!(p, Permission::Auto);
    }

    #[test]
    fn test_daemon_policy_serialization() {
        let json = serde_json::to_string(&DaemonPolicy::QueueForHuman).unwrap();
        assert_eq!(json, "\"queue_for_human\"");
    }
}
