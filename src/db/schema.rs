/// 現在のスキーマバージョン
pub const SCHEMA_VERSION: u32 = 1;

/// 全SQLiteスキーマ定義。マイグレーション時に順次適用される。
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    description: "初期スキーマ: セッション、メモリ、経験、タスク、スケジュール、監査ログ",
    sql: SCHEMA_V1,
}];

pub struct Migration {
    pub version: u32,
    pub description: &'static str,
    pub sql: &'static str,
}

const SCHEMA_V1: &str = r#"
-- スキーマバージョン管理
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- セッション
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    summary TEXT
);

-- メッセージ（会話履歴）
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    gist TEXT,
    tool_call_id TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);

-- A-MEM: 知識メモリ
CREATE TABLE IF NOT EXISTS memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'fact',
    tags TEXT NOT NULL DEFAULT '[]',
    source TEXT,
    created_at TEXT NOT NULL,
    accessed_at TEXT NOT NULL,
    access_count INTEGER NOT NULL DEFAULT 0
);

-- メモリ間リンク
CREATE TABLE IF NOT EXISTS memory_links (
    source_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    target_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    relation TEXT NOT NULL,
    PRIMARY KEY (source_id, target_id)
);

-- FTS5全文検索インデックス
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content,
    tags,
    content=memories,
    content_rowid=id
);

-- FTS5同期トリガー
CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content, tags) VALUES (new.id, new.content, new.tags);
END;
CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, tags) VALUES('delete', old.id, old.content, old.tags);
END;
CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, tags) VALUES('delete', old.id, old.content, old.tags);
    INSERT INTO memories_fts(rowid, content, tags) VALUES (new.id, new.content, new.tags);
END;

-- 経験メモリ: 成功/失敗の自動記録
CREATE TABLE IF NOT EXISTS experiences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type TEXT NOT NULL,
    task_context TEXT NOT NULL,
    action TEXT NOT NULL,
    outcome TEXT NOT NULL,
    lesson TEXT,
    tool_name TEXT,
    error_type TEXT,
    error_detail TEXT,
    reuse_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_exp_type ON experiences(type);
CREATE INDEX IF NOT EXISTS idx_exp_tool ON experiences(tool_name);

-- スキル（成功ツールチェーンのテンプレート）
CREATE TABLE IF NOT EXISTS skills (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL,
    tool_chain TEXT NOT NULL,
    trigger_patterns TEXT NOT NULL DEFAULT '[]',
    success_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- ユーザープロファイル
CREATE TABLE IF NOT EXISTS user_profile (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- タスク: 中断・再開可能な状態マシン
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    goal TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    parent_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    step_current INTEGER NOT NULL DEFAULT 0,
    step_log TEXT NOT NULL DEFAULT '[]',
    context TEXT,
    error_info TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_state ON tasks(state);

-- スケジュールタスク
CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id TEXT PRIMARY KEY,
    cron_expr TEXT NOT NULL,
    prompt TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_run_at TEXT,
    created_at TEXT NOT NULL
);

-- 人間待ちキュー（デーモンモード用）
CREATE TABLE IF NOT EXISTS pending_confirmations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    tool_args TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- 監査ログ（append-only）
CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    session_id TEXT,
    step INTEGER,
    action_type TEXT NOT NULL,
    action_data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(timestamp);

-- 推論キャッシュ
CREATE TABLE IF NOT EXISTS inference_cache (
    hash TEXT PRIMARY KEY,
    model_id TEXT NOT NULL,
    response TEXT NOT NULL,
    created_at TEXT NOT NULL,
    access_count INTEGER NOT NULL DEFAULT 0
);

-- WALモードを有効化（クラッシュ耐性向上）
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_version_is_positive() {
        assert!(SCHEMA_VERSION > 0);
    }

    #[test]
    fn test_migrations_exist() {
        assert!(!MIGRATIONS.is_empty());
    }

    #[test]
    fn test_migrations_are_sequential() {
        for (i, migration) in MIGRATIONS.iter().enumerate() {
            assert_eq!(migration.version, (i + 1) as u32);
        }
    }

    #[test]
    fn test_schema_v1_contains_all_tables() {
        let required_tables = [
            "sessions",
            "messages",
            "memories",
            "memory_links",
            "memories_fts",
            "experiences",
            "skills",
            "user_profile",
            "tasks",
            "scheduled_tasks",
            "pending_confirmations",
            "audit_log",
            "inference_cache",
        ];
        for table in required_tables {
            assert!(
                SCHEMA_V1.contains(table),
                "スキーマにテーブル '{table}' が見つかりません"
            );
        }
    }

    #[test]
    fn test_schema_v1_has_wal_mode() {
        assert!(SCHEMA_V1.contains("journal_mode=WAL"));
    }

    #[test]
    fn test_schema_v1_has_foreign_keys() {
        assert!(SCHEMA_V1.contains("foreign_keys=ON"));
    }

    #[test]
    fn test_schema_v1_has_fts5_triggers() {
        assert!(SCHEMA_V1.contains("memories_ai"));
        assert!(SCHEMA_V1.contains("memories_ad"));
        assert!(SCHEMA_V1.contains("memories_au"));
    }
}
