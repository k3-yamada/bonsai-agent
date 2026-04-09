use crate::db::schema::{MIGRATIONS, SCHEMA_VERSION};

/// マイグレーション結果
#[derive(Debug)]
pub struct MigrationPlan {
    pub current_version: u32,
    pub target_version: u32,
    pub migrations_to_apply: Vec<u32>,
}

/// 適用すべきマイグレーションを計算する（DB操作なし、純粋なロジック）
pub fn plan_migrations(current_version: u32) -> MigrationPlan {
    let migrations_to_apply: Vec<u32> = MIGRATIONS
        .iter()
        .filter(|m| m.version > current_version)
        .map(|m| m.version)
        .collect();

    MigrationPlan {
        current_version,
        target_version: SCHEMA_VERSION,
        migrations_to_apply,
    }
}

/// 指定バージョンのマイグレーションSQLを取得
pub fn get_migration_sql(version: u32) -> Option<&'static str> {
    MIGRATIONS
        .iter()
        .find(|m| m.version == version)
        .map(|m| m.sql)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fresh_install_needs_all_migrations() {
        let plan = plan_migrations(0);
        assert_eq!(plan.current_version, 0);
        assert_eq!(plan.target_version, SCHEMA_VERSION);
        assert_eq!(plan.migrations_to_apply.len(), MIGRATIONS.len());
    }

    #[test]
    fn test_up_to_date_needs_no_migrations() {
        let plan = plan_migrations(SCHEMA_VERSION);
        assert!(plan.migrations_to_apply.is_empty());
    }

    #[test]
    fn test_partial_migration() {
        if MIGRATIONS.len() > 1 {
            let plan = plan_migrations(1);
            assert_eq!(plan.migrations_to_apply.len(), MIGRATIONS.len() - 1);
        }
    }

    #[test]
    fn test_get_migration_sql_exists() {
        let sql = get_migration_sql(1);
        assert!(sql.is_some());
        assert!(sql.unwrap().contains("CREATE TABLE"));
    }

    #[test]
    fn test_get_migration_sql_not_found() {
        let sql = get_migration_sql(999);
        assert!(sql.is_none());
    }
}
