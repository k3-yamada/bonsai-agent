use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// スキル: 成功したツールチェーンのテンプレート
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub tool_chain: String,       // JSON: ツール呼び出しの順序
    pub trigger_patterns: String, // JSON: 発動パターン
    pub success_count: i64,
    pub created_at: String,
}

/// スキルストア
pub struct SkillStore<'a> {
    conn: &'a Connection,
}

impl<'a> SkillStore<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// スキルを保存/更新
    pub fn save(
        &self,
        name: &str,
        description: &str,
        tool_chain: &str,
        trigger_patterns: &str,
    ) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();

        // 既存スキルがあれば更新
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM skills WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing {
            self.conn.execute(
                "UPDATE skills SET description = ?1, tool_chain = ?2, trigger_patterns = ?3, success_count = success_count + 1, updated_at = ?4 WHERE id = ?5",
                params![description, tool_chain, trigger_patterns, &now, id],
            )?;
            Ok(id)
        } else {
            self.conn.execute(
                "INSERT INTO skills (name, description, tool_chain, trigger_patterns, success_count, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
                params![name, description, tool_chain, trigger_patterns, &now],
            )?;
            Ok(self.conn.last_insert_rowid())
        }
    }

    /// トリガーパターンに一致するスキルを検索
    pub fn find_matching(&self, query: &str, limit: usize) -> Result<Vec<Skill>> {
        // 簡易: スキル名とdescriptionに対するLIKE検索
        let pattern = format!("%{}%", query.split_whitespace().next().unwrap_or(""));
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, tool_chain, trigger_patterns, success_count, created_at
             FROM skills
             WHERE name LIKE ?1 OR description LIKE ?1
             ORDER BY success_count DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![&pattern, limit as i64], |row| {
            Ok(Skill {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                tool_chain: row.get(3)?,
                trigger_patterns: row.get(4)?,
                success_count: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 全スキルを取得
    pub fn list_all(&self) -> Result<Vec<Skill>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, tool_chain, trigger_patterns, success_count, created_at
             FROM skills ORDER BY success_count DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(Skill {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                tool_chain: row.get(3)?,
                trigger_patterns: row.get(4)?,
                success_count: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 全スキルをMarkdown形式にエクスポート
    pub fn export_markdown(&self) -> Result<String> {
        let skills = self.list_all()?;
        let mut md = String::from("# Skills\n\n");

        if skills.is_empty() {
            md.push_str("スキルはありません。\n");
            return Ok(md);
        }

        md.push_str(&format!("総スキル数: {}\n\n", skills.len()));

        for skill in &skills {
            md.push_str(&format!("## {}\n\n", skill.name));
            md.push_str(&format!("{}\n\n", skill.description));
            md.push_str(&format!("- **ツールチェーン**: `{}`\n", skill.tool_chain));
            md.push_str(&format!("- **トリガーパターン**: {}\n", skill.trigger_patterns));
            md.push_str(&format!("- **成功回数**: {}\n", skill.success_count));
            md.push_str(&format!("- **作成日**: {}\n", skill.created_at));
            md.push('\n');
        }

        Ok(md)
    }

    /// Markdownをファイルに書き出し
    pub fn export_to_file(&self, path: &std::path::Path) -> Result<()> {
        let md = self.export_markdown()?;
        std::fs::write(path, md)?;
        Ok(())
    }

    /// 経験からスキルへの昇格チェック（3シグナル重み付きスコアリング）
    /// frequency(0.4) + recency(0.35) + diversity(0.25)
    pub fn promote_from_experiences(
        &self,
        conn: &Connection,
        threshold: usize,
    ) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT action, COUNT(*) as cnt, task_context,
                    COUNT(DISTINCT task_context) as diversity,
                    MAX(created_at) as latest
             FROM experiences
             WHERE type = 'success'
             GROUP BY action
             HAVING cnt >= ?1
             ORDER BY cnt DESC",
        )?;

        let mut promoted = Vec::new();

        let rows = stmt.query_map(params![threshold as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;

        for row in rows {
            let (action, count, context, diversity, latest) = row?;
            let freq_s = (count as f64 / threshold as f64).min(2.0) * 0.4;
            let div_s = (diversity as f64).min(5.0) / 5.0 * 0.25;
            let rec_s = if latest.len() > 10 { 0.35 } else { 0.0 };
            let _score = freq_s + div_s + rec_s;

            // 既にスキルとして存在するかチェック
            let exists: bool = self.conn.query_row(
                "SELECT COUNT(*) > 0 FROM skills WHERE tool_chain = ?1",
                params![&action],
                |row| row.get(0),
            )?;

            if !exists {
                let raw: String = action.chars().take(30).collect();
                let name = format!("auto_{}", raw.replace([' ', ':'], "_"));
                self.save(
                    &name,
                    &format!("自動昇格: {context} で{count}回成功"),
                    &action,
                    "[]",
                )?;
                promoted.push(name);
            }
        }

        Ok(promoted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::experience::{ExperienceStore, ExperienceType, RecordParams};
    use crate::memory::store::MemoryStore;

    fn test_store() -> MemoryStore {
        MemoryStore::in_memory().unwrap()
    }

    #[test]
    fn test_save_new_skill() {
        let store = test_store();
        let skills = SkillStore::new(store.conn());
        let id = skills
            .save("list_files", "ファイル一覧", "shell: ls -la", "[]")
            .unwrap();
        assert!(id > 0);
    }

    #[test]
    fn test_save_updates_existing() {
        let store = test_store();
        let skills = SkillStore::new(store.conn());
        let id1 = skills.save("list_files", "v1", "shell: ls", "[]").unwrap();
        let id2 = skills
            .save("list_files", "v2", "shell: ls -la", "[]")
            .unwrap();
        assert_eq!(id1, id2); // 同じID

        let all = skills.list_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].success_count, 2); // カウント増加
    }

    #[test]
    fn test_find_matching() {
        let store = test_store();
        let skills = SkillStore::new(store.conn());
        skills
            .save("list_files", "ファイル一覧を表示", "shell: ls", "[]")
            .unwrap();
        skills
            .save("read_file", "ファイルを読む", "file_read: path", "[]")
            .unwrap();

        let found = skills.find_matching("list", 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "list_files");
    }

    #[test]
    fn test_list_all() {
        let store = test_store();
        let skills = SkillStore::new(store.conn());
        skills.save("a", "desc", "chain", "[]").unwrap();
        skills.save("b", "desc", "chain", "[]").unwrap();
        assert_eq!(skills.list_all().unwrap().len(), 2);
    }

    #[test]
    fn test_promote_from_experiences() {
        let store = test_store();
        let exp = ExperienceStore::new(store.conn());

        // 同じアクションで3回成功を記録
        for _ in 0..3 {
            exp.record(&RecordParams {
                exp_type: ExperienceType::Success,
                task_context: "file listing",
                action: "shell: ls -la",
                outcome: "OK",
                lesson: None,
                tool_name: Some("shell"),
                error_type: None,
                error_detail: None,
            })
            .unwrap();
        }

        let skills = SkillStore::new(store.conn());
        let promoted = skills.promote_from_experiences(store.conn(), 3).unwrap();
        assert_eq!(promoted.len(), 1);

        // 2回目は重複しない
        let promoted2 = skills.promote_from_experiences(store.conn(), 3).unwrap();
        assert!(promoted2.is_empty());
    }

    #[test]
    fn t_export_markdown_empty() {
        let store = test_store();
        let skills = SkillStore::new(store.conn());
        let md = skills.export_markdown().unwrap();
        assert!(md.contains("# Skills"));
        assert!(md.contains("スキルはありません"));
    }

    #[test]
    fn t_export_markdown_with_skills() {
        let store = test_store();
        let skills = SkillStore::new(store.conn());
        skills.save("list_files", "ファイル一覧を表示", "shell: ls -la", r#"[\"list\", \"files\"]"#).unwrap();
        skills.save("read_file", "ファイルを読む", "file_read: path", "[]").unwrap();
        skills.save("list_files", "ファイル一覧を表示", "shell: ls -la", r#"[\"list\", \"files\"]"#).unwrap();

        let md = skills.export_markdown().unwrap();
        assert!(md.contains("# Skills"));
        assert!(md.contains("## list_files"));
        assert!(md.contains("## read_file"));
        assert!(md.contains("ファイル一覧を表示"));
        assert!(md.contains("shell: ls -la"));
        assert!(md.contains("**成功回数**: 2"));
        assert!(md.contains("**成功回数**: 1"));
    }

    #[test]
    fn t_export_to_file() {
        let store = test_store();
        let skills = SkillStore::new(store.conn());
        skills.save("test_skill", "テスト用", "echo hello", "[]").unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SKILLS.md");
        skills.export_to_file(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Skills"));
        assert!(content.contains("## test_skill"));
    }

    #[test]
    fn test_promote_threshold_not_met() {
        let store = test_store();
        let exp = ExperienceStore::new(store.conn());
        exp.record(&RecordParams {
            exp_type: ExperienceType::Success,
            task_context: "test",
            action: "shell: echo",
            outcome: "OK",
            lesson: None,
            tool_name: Some("shell"),
            error_type: None,
            error_detail: None,
        })
        .unwrap();

        let skills = SkillStore::new(store.conn());
        let promoted = skills.promote_from_experiences(store.conn(), 3).unwrap();
        assert!(promoted.is_empty()); // 1回じゃ足りない
    }
}
