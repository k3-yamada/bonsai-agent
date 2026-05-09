/// 現在のスキーマバージョン
pub const SCHEMA_VERSION: u32 = 13;

/// 全SQLiteスキーマ定義。マイグレーション時に順次適用される。
///
/// V10 = ERL heuristics table (項目 213)。
/// V11 = decay stability column (項目 217、Cerememory ADR-005 port)。
/// V12 = ReviewState 9 columns (項目 218 候補、Cerememory ADR-011 port)。
/// V13 = sqlite-vec vec0 virtual table (plan T-1.4)。embeddings feature ON で
///       `vec_memories(memory_id, embedding float[256])` を作成、OFF では空 SQL
///       (no-op、version のみ記録) で MIGRATIONS.len() invariant 維持。
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
    Migration {
        version: 6,
        description: "TTL: memories, experiences, skills に expires_at カラム追加",
        sql: SCHEMA_V6,
    },
    Migration {
        version: 7,
        description: "プリスクリーニング: experiments に prescreened カラム追加",
        sql: SCHEMA_V7,
    },
    Migration {
        version: 8,
        description: "実験インデックス: accepted+mutation_detail複合インデックス",
        sql: SCHEMA_V8,
    },
    Migration {
        version: 9,
        description: "Beyond pass@1 信頼性メトリクス: rdc/vaf/gds/stability_delta カラム追加 (項目 200)",
        sql: SCHEMA_V9,
    },
    Migration {
        version: 10,
        description: "ERL heuristics pool: 自然言語助言の第 4 メモリ層 (項目 213)",
        sql: SCHEMA_V10,
    },
    Migration {
        version: 11,
        description: "Cerememory decay port: heuristics に stability 列追加 (項目 217)",
        sql: SCHEMA_V11,
    },
    Migration {
        version: 12,
        description: "Cerememory ADR-011 ReviewState port: heuristics に Freshness 軸 9 列追加 (項目 218 候補)",
        sql: SCHEMA_V12,
    },
    Migration {
        version: 13,
        description: "sqlite-vec vec0 virtual table: vec_memories(memory_id, embedding float[256]) (plan T-1.4)",
        sql: SCHEMA_V13,
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

const SCHEMA_V6: &str = r#"
ALTER TABLE memories ADD COLUMN expires_at TEXT;
ALTER TABLE experiences ADD COLUMN expires_at TEXT;
ALTER TABLE skills ADD COLUMN expires_at TEXT;
CREATE INDEX IF NOT EXISTS idx_memories_expires ON memories(expires_at);
CREATE INDEX IF NOT EXISTS idx_experiences_expires ON experiences(expires_at);
"#;

const SCHEMA_V7: &str = r#"
ALTER TABLE experiments ADD COLUMN prescreened INTEGER NOT NULL DEFAULT 0;
"#;

const SCHEMA_V8: &str = r#"
CREATE INDEX IF NOT EXISTS idx_experiments_accepted_detail ON experiments(accepted, mutation_detail);
"#;

/// 項目 200 (Beyond pass@1): 信頼性メトリクス 4 カラム追加。
/// すべて REAL NULL で旧データは NULL のまま (後方互換)。
const SCHEMA_V9: &str = r#"
ALTER TABLE experiments ADD COLUMN reliability_decay REAL;
ALTER TABLE experiments ADD COLUMN variance_amplification REAL;
ALTER TABLE experiments ADD COLUMN graceful_degradation REAL;
ALTER TABLE experiments ADD COLUMN stability_delta REAL;
"#;

/// 項目 213 (ERL Heuristics Pool): SkillStore (tool_chain) / ExperienceStore /
/// Vault (rules) と並ぶ第 4 メモリ層。`fingerprint` UNIQUE で deterministic dedup
/// (項目 206 同方針、advice 先頭 80 chars + trigger_hash)。
const SCHEMA_V10: &str = r#"
CREATE TABLE IF NOT EXISTS heuristics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    advice TEXT NOT NULL,
    trigger_patterns TEXT NOT NULL DEFAULT '[]',
    source_session_id TEXT,
    source_task TEXT NOT NULL DEFAULT '',
    category TEXT NOT NULL DEFAULT 'efficiency',
    score REAL NOT NULL DEFAULT 0.5,
    used_count INTEGER NOT NULL DEFAULT 0,
    success_after_use INTEGER NOT NULL DEFAULT 0,
    fingerprint TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_used_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_heuristics_category ON heuristics(category);
"#;

/// 項目 217 (Cerememory power-law decay port、MIT、Copyright 2026 CORe Inc.):
/// `heuristics` テーブルに `stability REAL NOT NULL DEFAULT 1.0` 列を追加。
/// `crate::memory::decay::compute_stability_boost` で `record_outcome` 時に boost 適用、
/// `compute_fidelity` で `prune` 時に decay-adjusted 削除順序を計算。
/// production default OFF (`BONSAI_DECAY_ENABLED` env unset) で既存挙動 100% 維持。
const SCHEMA_V11: &str = r#"
ALTER TABLE heuristics ADD COLUMN stability REAL NOT NULL DEFAULT 1.0;
"#;

/// 項目 218 候補 (Cerememory ADR-011 ReviewState port、MIT、Copyright 2026 CORe Inc.):
/// `heuristics` テーブルに **Freshness 軸**を構成する 9 列を追加 (Strength と分離)。
/// `crate::memory::review::ReviewState` 構造体に対応:
/// - `review_status` TEXT: ReviewStatus enum SQLite 表現 (default 'unknown')
/// - `importance` / `volatility` / `freshness` REAL [0.0..=1.0]
/// - `source_confidence` REAL NULL
/// - `last_reviewed_at` / `next_review_at` TEXT NULL (RFC3339)
/// - `review_count` / `stale_count` INTEGER (default 0)
/// - `idx_heuristics_next_review` index で `review_tick` SELECT を最適化
///
/// production default OFF (`BONSAI_REVIEW_ENABLED` env unset) で既存挙動 100% 維持。
const SCHEMA_V12: &str = r#"
ALTER TABLE heuristics ADD COLUMN review_status TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE heuristics ADD COLUMN importance REAL NOT NULL DEFAULT 0.5;
ALTER TABLE heuristics ADD COLUMN volatility REAL NOT NULL DEFAULT 0.5;
ALTER TABLE heuristics ADD COLUMN freshness REAL NOT NULL DEFAULT 1.0;
ALTER TABLE heuristics ADD COLUMN source_confidence REAL;
ALTER TABLE heuristics ADD COLUMN last_reviewed_at TEXT;
ALTER TABLE heuristics ADD COLUMN next_review_at TEXT;
ALTER TABLE heuristics ADD COLUMN review_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE heuristics ADD COLUMN stale_count INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_heuristics_next_review ON heuristics(next_review_at);
"#;

/// V13: sqlite-vec vec0 virtual table (plan T-1.4)。
/// embeddings feature ON で vec_memories(memory_id, embedding float[256]) を
/// 作成。OFF では空 SQL (no-op) で MIGRATIONS.len()==SCHEMA_VERSION invariant
/// を維持しつつ vec0 module 未ロード環境での migration 失敗を回避。
#[cfg(feature = "embeddings")]
const SCHEMA_V13: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS vec_memories USING vec0(
    memory_id INTEGER PRIMARY KEY,
    embedding float[256]
);
"#;

#[cfg(not(feature = "embeddings"))]
const SCHEMA_V13: &str = "";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_v6_ttl_migration() {
        assert!(SCHEMA_V6.contains("expires_at"));
        assert!(SCHEMA_V6.contains("ALTER TABLE memories"));
        assert!(SCHEMA_V6.contains("ALTER TABLE experiences"));
        assert!(SCHEMA_V6.contains("ALTER TABLE skills"));
    }

    #[test]
    fn test_schema_version_is_positive() {
        const { assert!(SCHEMA_VERSION > 0) };
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
        assert!(
            SCHEMA_V3.contains("idx_events_session"),
            "V3にセッションインデックスが必要"
        );
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
        assert!(
            SCHEMA_V4.contains("checkpoints"),
            "V4にcheckpointsテーブルが必要"
        );
        assert!(
            SCHEMA_V4.contains("idx_checkpoints_session"),
            "V4にセッションインデックスが必要"
        );
    }

    #[test]
    fn test_schema_v5_contains_knowledge_graph_tables() {
        assert!(
            SCHEMA_V5.contains("knowledge_nodes"),
            "V5にknowledge_nodesテーブルが必要"
        );
        assert!(
            SCHEMA_V5.contains("knowledge_edges"),
            "V5にknowledge_edgesテーブルが必要"
        );
        assert!(
            SCHEMA_V5.contains("idx_knowledge_edges_source"),
            "V5にソースインデックスが必要"
        );
        assert!(
            SCHEMA_V5.contains("idx_knowledge_edges_target"),
            "V5にターゲットインデックスが必要"
        );
    }

    #[test]
    fn test_schema_v7_contains_prescreened_column() {
        assert!(
            SCHEMA_V7.contains("prescreened"),
            "V7にprescreenedカラムが必要"
        );
        assert!(
            SCHEMA_V7.contains("ALTER TABLE experiments"),
            "V7はexperimentsテーブルのALTER"
        );
    }

    #[test]
    fn test_schema_v8_contains_accepted_detail_index() {
        assert!(
            SCHEMA_V8.contains("idx_experiments_accepted_detail"),
            "V8にaccepted+mutation_detail複合インデックスが必要"
        );
        assert!(
            SCHEMA_V8.contains("experiments(accepted, mutation_detail)"),
            "V8はexperimentsテーブルの複合インデックス"
        );
    }

    #[test]
    fn test_schema_v10_contains_heuristics_table() {
        assert!(
            SCHEMA_V10.contains("CREATE TABLE IF NOT EXISTS heuristics"),
            "V10にheuristicsテーブルが必要 (項目 213)"
        );
        assert!(SCHEMA_V10.contains("fingerprint TEXT NOT NULL UNIQUE"));
        assert!(SCHEMA_V10.contains("idx_heuristics_category"));
    }

    #[test]
    fn test_schema_v12_contains_review_state_columns() {
        // 項目 218 候補: Cerememory ADR-011 ReviewState port
        for col in [
            "review_status",
            "importance",
            "volatility",
            "freshness",
            "source_confidence",
            "last_reviewed_at",
            "next_review_at",
            "review_count",
            "stale_count",
        ] {
            assert!(
                SCHEMA_V12.contains(col),
                "V12 に {col} 列追加が必要 (Plan B §4.2)"
            );
        }
        assert!(
            SCHEMA_V12.contains("idx_heuristics_next_review"),
            "V12 に next_review_at index 必要 (review_tick SELECT 最適化)"
        );
    }

    #[cfg(feature = "embeddings")]
    #[test]
    fn test_schema_v13_contains_vec_memories() {
        // plan T-1.4: sqlite-vec vec0 virtual table
        assert!(
            SCHEMA_V13.contains("vec_memories"),
            "V13 に vec_memories 仮想テーブル定義必要"
        );
        assert!(SCHEMA_V13.contains("USING vec0"), "vec0 module 利用が必要");
        assert!(
            SCHEMA_V13.contains("float[256]"),
            "256d 固定 (DEFAULT_EMBEDDING_DIM 整合)"
        );
    }

    #[cfg(not(feature = "embeddings"))]
    #[test]
    fn test_schema_v13_is_empty_when_no_embeddings() {
        assert!(
            SCHEMA_V13.is_empty(),
            "embeddings OFF では V13 SQL 空 (no-op で migration 失敗回避)"
        );
    }
}
