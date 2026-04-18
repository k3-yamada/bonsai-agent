/// 現在のスキーマバージョン
pub const SCHEMA_VERSION: u32 = 5;

/// 全SQLiteスキーマ定義。マイグレーション時に順次適用される。
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "初期スキーマ: セッション、メモリ、経験、タスク、スケジュール、監査ログ",
        sql: SCHEMA_V1,
    },
    Migration {
        version: 2,
        description: "実験ログ: experiments, experiment_config テーブル",
        sql: SCHEMA_V2,
    },
    Migration {
        version: 3,
        description: "Event Sourcing: 統一イベントストリーム + audit_logインデックス強化",
        sql: SCHEMA_V3,
    },
    Migration {
        version: 4,
        description: "チェックポイント永続化: checkpoints テーブル",
        sql: SCHEMA_V4,
    },
    Migration {
        version: 5,
        description: "グラフ構造連想記憶: knowledge_nodes, knowledge_edges テーブル",
        sql: SCHEMA_V5,
    },
];

pub struct Migration {
    pub version: u32,
    pub description: &'static str,
    pub sql: &'static str,
}

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, summary TEXT
);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL, content TEXT NOT NULL, gist TEXT, tool_call_id TEXT, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
CREATE TABLE IF NOT EXISTS memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content TEXT NOT NULL, category TEXT NOT NULL DEFAULT 'fact',
    tags TEXT NOT NULL DEFAULT '[]', source TEXT,
    created_at TEXT NOT NULL, accessed_at TEXT NOT NULL, access_count INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS memory_links (
    source_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    target_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    relation TEXT NOT NULL, PRIMARY KEY (source_id, target_id)
);
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(content, tags, content=memories, content_rowid=id);
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
CREATE TABLE IF NOT EXISTS experiences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type TEXT NOT NULL, task_context TEXT NOT NULL, action TEXT NOT NULL, outcome TEXT NOT NULL,
    lesson TEXT, tool_name TEXT, error_type TEXT, error_detail TEXT,
    reuse_count INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_exp_type ON experiences(type);
CREATE INDEX IF NOT EXISTS idx_exp_tool ON experiences(tool_name);
CREATE TABLE IF NOT EXISTS skills (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE, description TEXT NOT NULL, tool_chain TEXT NOT NULL,
    trigger_patterns TEXT NOT NULL DEFAULT '[]', success_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS user_profile (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY, goal TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending',
    parent_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    step_current INTEGER NOT NULL DEFAULT 0, step_log TEXT NOT NULL DEFAULT '[]',
    context TEXT, error_info TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_state ON tasks(state);
CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id TEXT PRIMARY KEY, cron_expr TEXT NOT NULL, prompt TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1, last_run_at TEXT, created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS pending_confirmations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL, tool_args TEXT NOT NULL, reason TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL, session_id TEXT, step INTEGER,
    action_type TEXT NOT NULL, action_data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(timestamp);
CREATE TABLE IF NOT EXISTS inference_cache (
    hash TEXT PRIMARY KEY, model_id TEXT NOT NULL, response TEXT NOT NULL,
    created_at TEXT NOT NULL, access_count INTEGER NOT NULL DEFAULT 0
);
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;
"#;

const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS experiments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    experiment_id TEXT NOT NULL UNIQUE,
    mutation_type TEXT NOT NULL, mutation_detail TEXT NOT NULL,
    baseline_score REAL NOT NULL, experiment_score REAL NOT NULL,
    delta REAL NOT NULL, accepted INTEGER NOT NULL,
    duration_secs REAL NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_experiments_accepted ON experiments(accepted);
CREATE INDEX IF NOT EXISTS idx_experiments_created ON experiments(created_at);
CREATE TABLE IF NOT EXISTS experiment_config (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    experiment_id TEXT NOT NULL REFERENCES experiments(experiment_id) ON DELETE CASCADE,
    config_key TEXT NOT NULL, config_value TEXT NOT NULL,
    UNIQUE(experiment_id, config_key)
);
"#;

const SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    event_data TEXT NOT NULL,
    step_index INTEGER,
    parent_event_id INTEGER REFERENCES events(id),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id);
CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type);
CREATE INDEX IF NOT EXISTS idx_events_created ON events(created_at);
CREATE INDEX IF NOT EXISTS idx_audit_session ON audit_log(session_id);
CREATE INDEX IF NOT EXISTS idx_audit_action_type ON audit_log(action_type);
"#;

const SCHEMA_V4: &str = r#"
CREATE TABLE IF NOT EXISTS checkpoints (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT,
    description TEXT NOT NULL,
    git_ref TEXT,
    timestamp TEXT NOT NULL,
    rolled_back_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_checkpoints_session ON checkpoints(session_id);
CREATE INDEX IF NOT EXISTS idx_checkpoints_timestamp ON checkpoints(timestamp);
"#;

const SCHEMA_V5: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_nodes (
    id INTEGER PRIMARY KEY,
    node_type TEXT NOT NULL,
    name TEXT NOT NULL UNIQUE,
    metadata TEXT DEFAULT '{}',
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS knowledge_edges (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES knowledge_nodes(id),
    target_id INTEGER NOT NULL REFERENCES knowledge_nodes(id),
    relation TEXT NOT NULL,
    weight REAL DEFAULT 1.0,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(source_id, target_id, relation)
);
CREATE INDEX IF NOT EXISTS idx_knowledge_edges_source ON knowledge_edges(source_id);
CREATE INDEX IF NOT EXISTS idx_knowledge_edges_target ON knowledge_edges(target_id);
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
        for table in [
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
        ] {
            assert!(
                SCHEMA_V1.contains(table),
                "テーブル '{table}' が見つかりません"
            );
        }
    }

    #[test]
    fn test_schema_v2_contains_experiment_tables() {
        for table in ["experiments", "experiment_config"] {
            assert!(
                SCHEMA_V2.contains(table),
                "V2テーブル '{table}' が見つかりません"
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
    fn test_schema_v3_contains_events_table() {
        assert!(SCHEMA_V3.contains("events"), "V3にeventsテーブルが必要");
        assert!(SCHEMA_V3.contains("idx_events_session"), "V3にセッションインデックスが必要");
    }

    #[test]
    fn test_migrations_count_matches_version() {
        assert_eq!(MIGRATIONS.len(), SCHEMA_VERSION as usize);
    }

    #[test]
    fn test_schema_v1_has_fts5_triggers() {
        assert!(SCHEMA_V1.contains("memories_ai"));
        assert!(SCHEMA_V1.contains("memories_ad"));
        assert!(SCHEMA_V1.contains("memories_au"));
    }

    #[test]
    fn test_schema_v4_contains_checkpoints_table() {
        assert!(SCHEMA_V4.contains("checkpoints"), "V4にcheckpointsテーブルが必要");
        assert!(SCHEMA_V4.contains("idx_checkpoints_session"), "V4にセッションインデックスが必要");
    }

    #[test]
    fn test_schema_v5_contains_knowledge_graph_tables() {
        assert!(SCHEMA_V5.contains("knowledge_nodes"), "V5にknowledge_nodesテーブルが必要");
        assert!(SCHEMA_V5.contains("knowledge_edges"), "V5にknowledge_edgesテーブルが必要");
        assert!(SCHEMA_V5.contains("idx_knowledge_edges_source"), "V5にソースインデックスが必要");
        assert!(SCHEMA_V5.contains("idx_knowledge_edges_target"), "V5にターゲットインデックスが必要");
    }
}
