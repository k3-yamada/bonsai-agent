pub mod arxiv;
pub mod descriptions;
pub mod file;
pub mod git;
pub mod hooks;
pub mod mcp_client;
pub mod permission;
pub mod plugin;
pub mod repomap;
pub mod sandbox;
pub mod shell;
pub mod typed;
pub mod web;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::runtime::embedder::{Embedder, cosine_similarity, create_embedder};
use crate::tools::permission::Permission;

/// ツールのスキーマ情報（LLMのシステムプロンプトに注入する）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// ツールの実行結果
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub success: bool,
}

/// 全ツールが実装するトレイト
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn permission(&self) -> Permission;

    /// ツールを実行する（同期）
    fn call(&self, args: serde_json::Value) -> Result<ToolResult>;

    /// 読取専用ツールか（並列実行の対象判定用）
    fn is_read_only(&self) -> bool {
        false
    }

    /// スキーマ情報を生成
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}

/// タスク種別 — ツール選択のフィルタリングに使用
/// CrewAI知見: 選択肢が少ないほど1ビットモデルは正確に動作する
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    /// ファイル操作: file_read, file_write, repo_map
    FileOperation,
    /// コード実行: shell, git
    CodeExecution,
    /// リサーチ: web_search, web_fetch, arxiv_search
    Research,
    /// 全ツール使用可能（フィルタなし）
    General,
}

/// モデル能力レベル — バックエンド種別に応じたツール制限（OpenCode知見）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCapability {
    /// 全ツール使用可能（llama-server/mlx-lm標準モデル）
    Full,
    /// 編集系のみ（file_read/file_write/multi_edit/repo_map/shell）
    EditFocused,
    /// 読取+実行のみ（file_read/shell/git/repo_map、編集精度が低いモデル向け）
    ReadExecute,
}

impl ModelCapability {
    /// バックエンド種別からデフォルトのCapabilityを推定
    pub fn from_backend(backend: &str) -> Self {
        match backend {
            "bitnet" => ModelCapability::ReadExecute,
            _ => ModelCapability::Full,
        }
    }

    /// このCapabilityで許可されるツール名を返す（Noneはフィルタなし）
    pub fn allowed_tools(&self) -> Option<&[&str]> {
        match self {
            ModelCapability::Full => None,
            ModelCapability::EditFocused => {
                Some(&["file_read", "file_write", "multi_edit", "repo_map", "shell"])
            }
            ModelCapability::ReadExecute => Some(&["file_read", "repo_map", "shell", "git"]),
        }
    }
}

/// クエリ文字列からタスク種別を推定する
/// 日本語キーワードマッチングで判定（1ビットモデル向けにツール選択肢を絞る）
pub fn detect_task_type(query: &str) -> TaskType {
    let q = query.to_lowercase();

    // ファイル操作キーワード
    if q.contains("ファイル") || q.contains("読") || q.contains("書") || q.contains("編集")
    {
        return TaskType::FileOperation;
    }

    // コード実行キーワード
    if q.contains("実行") || q.contains("ビルド") || q.contains("テスト") || q.contains("コマンド")
    {
        return TaskType::CodeExecution;
    }

    // リサーチキーワード
    if q.contains("検索") || q.contains("調べ") || q.contains("論文") || q.contains("url") {
        return TaskType::Research;
    }

    TaskType::General
}

impl TaskType {
    /// このタスク種別で許可されるツール名プレフィックスを返す
    /// GeneralはNone（フィルタなし）
    fn allowed_prefixes(&self) -> Option<&[&str]> {
        match self {
            TaskType::FileOperation => Some(&["file_read", "file_write", "multi_edit", "repo_map"]),
            TaskType::CodeExecution => Some(&["shell", "git"]),
            TaskType::Research => Some(&["web_search", "web_fetch", "arxiv_search"]),
            TaskType::General => None,
        }
    }
}

/// ツール結果のセッション内キャッシュ — 読取専用ツールの重複I/Oを防止
/// キー: "tool_name:args_json" でツール名+引数のJSON文字列化
pub struct ToolResultCache {
    cache: HashMap<String, ToolResult>,
    hits: usize,
    misses: usize,
}

impl ToolResultCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            hits: 0,
            misses: 0,
        }
    }

    /// キャッシュキーを生成（ツール名+引数JSON）
    fn make_key(tool_name: &str, args: &serde_json::Value) -> String {
        format!("{}:{}", tool_name, args)
    }

    /// キャッシュからツール結果を取得（ヒット時にhitsカウント加算）
    pub fn get(&mut self, tool_name: &str, args: &serde_json::Value) -> Option<&ToolResult> {
        let key = Self::make_key(tool_name, args);
        if self.cache.contains_key(&key) {
            self.hits += 1;
            self.cache.get(&key)
        } else {
            self.misses += 1;
            None
        }
    }

    /// ツール結果をキャッシュに保存
    pub fn put(&mut self, tool_name: &str, args: &serde_json::Value, result: ToolResult) {
        let key = Self::make_key(tool_name, args);
        self.cache.insert(key, result);
    }

    /// ヒット/ミス統計を返す
    pub fn stats(&self) -> (usize, usize) {
        (self.hits, self.misses)
    }

    /// 特定ツール名に関連するキャッシュエントリを無効化
    /// 書き込みツール実行後に呼ぶ（file_write→file_read/repo_mapクリア等）
    pub fn invalidate(&mut self, tool_name: &str) {
        self.cache
            .retain(|key, _| !key.starts_with(&format!("{}:", tool_name)));
    }

    /// 全キャッシュをクリア
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// キャッシュ済みエントリ数
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

impl Default for ToolResultCache {
    fn default() -> Self {
        Self::new()
    }
}

/// セマンティック選択用の遅延初期化キャッシュ
/// 初回呼び出し時にembedderとツール説明のembedding行列を構築
struct SemanticCache {
    embedder: Box<dyn Embedder>,
    /// ツール名 → 説明文のembedding
    tool_embeddings: HashMap<String, Vec<f32>>,
}

/// ツールレジストリ — 登録・検索・動的選択を管理
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    /// セマンティックツール選択用キャッシュ（初回使用時に遅延構築、register時に無効化）
    semantic_cache: Mutex<Option<SemanticCache>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            semantic_cache: Mutex::new(None),
        }
    }

    /// ツールを登録（セマンティックキャッシュは次回使用時に再構築される）
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
        // キャッシュ無効化: 新ツール登録時は次回select_relevant_split_semanticで再構築
        if let Ok(mut cache) = self.semantic_cache.lock() {
            *cache = None;
        }
    }

    /// 名前でツールを取得
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// モデルCapabilityに基づくツールフィルタリング（OpenCode知見）
    pub fn select_for_capability(&self, capability: ModelCapability) -> Vec<&dyn Tool> {
        match capability.allowed_tools() {
            Some(allowed) => self
                .tools
                .values()
                .filter(|t| allowed.contains(&t.name()))
                .map(|t| t.as_ref())
                .collect(),
            None => self.tools.values().map(|t| t.as_ref()).collect(),
        }
    }

    /// 登録済みツール名の一覧
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// 登録済みツール数
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 登録ツール数が上限を超えている場合に警告ログ
    pub fn warn_if_exceeded(&self, limit: usize) {
        if self.tools.len() > limit {
            crate::observability::logger::log_event(
                crate::observability::logger::LogLevel::Warn,
                "tools",
                &format!(
                    "登録ツール数({})が上限({})を超過。1bitモデルの精度低下リスク",
                    self.tools.len(),
                    limit
                ),
            );
        }
    }

    /// クエリに関連するツールを動的に選択（上位max件）。
    /// キーワードマッチングでスコアリングし、スコアの高い順に返す。
    pub fn select_relevant(&self, query: &str, max: usize) -> Vec<&dyn Tool> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();
        let task_boost = Self::detect_task_boost(&query_lower);

        let mut scored: Vec<(&dyn Tool, usize)> = self
            .tools
            .values()
            .map(|tool| {
                let name = tool.name().to_lowercase();
                let desc = tool.description().to_lowercase();
                let mut score = query_words
                    .iter()
                    .filter(|w| name.contains(*w) || desc.contains(*w))
                    .count();
                if task_boost.iter().any(|b| name.contains(b)) {
                    score += 2;
                }
                (tool.as_ref(), score)
            })
            .collect();

        // スコア降順でソート（同スコアはツール名のアルファベット順）
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name().cmp(b.0.name())));

        scored.into_iter().take(max).map(|(t, _)| t).collect()
    }

    /// ビルトイン/MCP分離選択: ビルトインは上位builtin_max件、MCPは別枠mcp_max件
    /// MCPツールは名前に':'を含む（"server:tool"形式）
    pub fn select_relevant_split(
        &self,
        query: &str,
        builtin_max: usize,
        mcp_max: usize,
    ) -> Vec<&dyn Tool> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();
        let task_boost = Self::detect_task_boost(&query_lower);

        let score_tool = |tool: &dyn Tool| -> usize {
            let name = tool.name().to_lowercase();
            let desc = tool.description().to_lowercase();
            let mut score = query_words
                .iter()
                .filter(|w| name.contains(*w) || desc.contains(*w))
                .count();
            if task_boost.iter().any(|b| name.contains(b)) {
                score += 2;
            }
            score
        };

        let mut builtin: Vec<(&dyn Tool, usize)> = Vec::new();
        let mut mcp: Vec<(&dyn Tool, usize)> = Vec::new();
        for tool in self.tools.values() {
            let s = score_tool(tool.as_ref());
            if tool.name().contains(':') {
                mcp.push((tool.as_ref(), s));
            } else {
                builtin.push((tool.as_ref(), s));
            }
        }

        let sort_fn = |a: &(&dyn Tool, usize), b: &(&dyn Tool, usize)| {
            b.1.cmp(&a.1).then_with(|| a.0.name().cmp(b.0.name()))
        };
        builtin.sort_by(sort_fn);
        mcp.sort_by(sort_fn);

        let mut result: Vec<&dyn Tool> = builtin
            .into_iter()
            .take(builtin_max)
            .map(|(t, _)| t)
            .collect();
        result.extend(mcp.into_iter().take(mcp_max).map(|(t, _)| t));
        result
    }

    /// セマンティック類似度ベースのビルトイン/MCP分離選択
    ///
    /// ローカルONNX埋め込みモデル（FastEmbedder/AllMiniLML6V2）でツール説明とクエリを
    /// ベクトル化し、コサイン類似度+キーワードスコアのハイブリッドで上位を選択。
    /// embedder初期化失敗時は`select_relevant_split`（キーワードマッチ）にフォールバック。
    ///
    /// キャッシュ: 初回呼び出しで embedder + ツール embedding行列を構築し、register時のみ無効化。
    pub fn select_relevant_split_semantic(
        &self,
        query: &str,
        builtin_max: usize,
        mcp_max: usize,
    ) -> Vec<&dyn Tool> {
        // 1. キャッシュを初期化または検証
        let query_embedding = {
            let mut cache_guard = match self.semantic_cache.lock() {
                Ok(g) => g,
                Err(_) => {
                    // Mutex poisoned → キーワードフォールバック
                    return self.select_relevant_split(query, builtin_max, mcp_max);
                }
            };

            // 初回またはinvalidate後: 遅延構築
            if cache_guard.is_none() {
                let embedder = create_embedder();
                let tool_names: Vec<String> = self.tools.keys().cloned().collect();
                let tool_descs: Vec<&str> = tool_names
                    .iter()
                    .filter_map(|n| self.tools.get(n).map(|t| t.description()))
                    .collect();
                match embedder.embed(&tool_descs) {
                    Ok(vecs) if vecs.len() == tool_names.len() => {
                        let mut map = HashMap::with_capacity(tool_names.len());
                        for (name, v) in tool_names.into_iter().zip(vecs.into_iter()) {
                            map.insert(name, v);
                        }
                        *cache_guard = Some(SemanticCache {
                            embedder,
                            tool_embeddings: map,
                        });
                    }
                    _ => {
                        // embedding失敗 → キーワードフォールバック
                        return self.select_relevant_split(query, builtin_max, mcp_max);
                    }
                }
            }

            // クエリをembedding（Mutex内でembedder借用）
            let cache = match cache_guard.as_ref() {
                Some(c) => c,
                None => return self.select_relevant_split(query, builtin_max, mcp_max),
            };
            match cache.embedder.embed(&[query]) {
                Ok(mut vs) if !vs.is_empty() => vs.swap_remove(0),
                _ => return self.select_relevant_split(query, builtin_max, mcp_max),
            }
        };

        // 2. キーワードスコア（既存ロジック、補助シグナル）
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();
        let task_boost = Self::detect_task_boost(&query_lower);

        // 3. ハイブリッドスコアリング: 0.7 * cosine + 0.3 * 正規化キーワードスコア
        let cache_guard = match self.semantic_cache.lock() {
            Ok(g) => g,
            Err(_) => return self.select_relevant_split(query, builtin_max, mcp_max),
        };
        let cache = match cache_guard.as_ref() {
            Some(c) => c,
            None => return self.select_relevant_split(query, builtin_max, mcp_max),
        };

        let max_keyword_score = query_words.len().max(1) as f32 + 2.0; // task_boost最大+2
        let mut builtin: Vec<(&dyn Tool, f32)> = Vec::new();
        let mut mcp: Vec<(&dyn Tool, f32)> = Vec::new();

        for (name, tool) in &self.tools {
            let sem_score = cache
                .tool_embeddings
                .get(name)
                .map(|v| cosine_similarity(&query_embedding, v))
                .unwrap_or(0.0)
                .max(0.0); // 負の類似度は0にクリップ

            let name_l = tool.name().to_lowercase();
            let desc_l = tool.description().to_lowercase();
            let mut kw_raw = query_words
                .iter()
                .filter(|w| name_l.contains(*w) || desc_l.contains(*w))
                .count() as f32;
            if task_boost.iter().any(|b| name_l.contains(b)) {
                kw_raw += 2.0;
            }
            let kw_score = (kw_raw / max_keyword_score).clamp(0.0, 1.0);

            let hybrid = 0.7 * sem_score + 0.3 * kw_score;
            if tool.name().contains(':') {
                mcp.push((tool.as_ref(), hybrid));
            } else {
                builtin.push((tool.as_ref(), hybrid));
            }
        }

        let sort_fn = |a: &(&dyn Tool, f32), b: &(&dyn Tool, f32)| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.name().cmp(b.0.name()))
        };
        builtin.sort_by(sort_fn);
        mcp.sort_by(sort_fn);

        let mut result: Vec<&dyn Tool> = builtin
            .into_iter()
            .take(builtin_max)
            .map(|(t, _)| t)
            .collect();
        result.extend(mcp.into_iter().take(mcp_max).map(|(t, _)| t));
        result
    }

    /// タスク種別からブーストするツール名プレフィックスを検出
    fn detect_task_boost(query: &str) -> Vec<&'static str> {
        let mut b = Vec::new();
        if query.contains("ファイル") || query.contains("読") || query.contains("書") {
            b.push("file");
        }
        if query.contains("コマンド") || query.contains("実行") || query.contains("ビルド")
        {
            b.push("shell");
        }
        if query.contains("git") || query.contains("コミット") {
            b.push("git");
        }
        if query.contains("検索") || query.contains("探") {
            b.push("web");
            b.push("file");
        }
        b
    }

    /// 選択されたツールのスキーマをシステムプロンプト用にフォーマット
    pub fn format_schemas(&self, tools: &[&dyn Tool]) -> String {
        if tools.is_empty() {
            return String::new();
        }

        let mut output = String::from("# 使用可能なツール\n\n");
        for tool in tools {
            output.push_str(&format!("## {}\n", tool.name()));
            output.push_str(&format!("{}\n", tool.description()));
            output.push_str(&format!(
                "パラメータ: {}\n\n",
                serde_json::to_string_pretty(&tool.parameters_schema())
                    .unwrap_or_else(|_| "{}".to_string())
            ));
        }
        output
    }

    /// 名前+説明のみのコンパクト形式（パラメータスキーマ省略でトークン節約）
    pub fn format_schemas_compact(&self, tools: &[&dyn Tool]) -> String {
        if tools.is_empty() {
            return String::new();
        }
        let mut output = String::from("# 使用可能なツール\n\n");
        for tool in tools {
            output.push_str(&format!("- **{}**: {}\n", tool.name(), tool.description()));
        }
        output
    }
    /// タスク種別に基づいてツールをフィルタリングしてから選択する
    /// GeneralはフィルタなしでIALを維持（既存動作と同一）
    pub fn select_relevant_with_type(&self, query: &str, max: usize) -> Vec<&dyn Tool> {
        let task_type = detect_task_type(query);
        let allowed = task_type.allowed_prefixes();

        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();
        let task_boost = Self::detect_task_boost(&query_lower);

        let mut scored: Vec<(&dyn Tool, usize)> = self
            .tools
            .values()
            .filter(|tool| {
                // Generalならフィルタなし、それ以外は許可リストでフィルタ
                match allowed {
                    None => true,
                    Some(prefixes) => prefixes.iter().any(|p| tool.name() == *p),
                }
            })
            .map(|tool| {
                let name = tool.name().to_lowercase();
                let desc = tool.description().to_lowercase();
                let mut score = query_words
                    .iter()
                    .filter(|w| name.contains(*w) || desc.contains(*w))
                    .count();
                if task_boost.iter().any(|b| name.contains(b)) {
                    score += 2;
                }
                (tool.as_ref(), score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name().cmp(b.0.name())));
        scored.into_iter().take(max).map(|(t, _)| t).collect()
    }

    /// 段階的開示: 第1段階は名前+summaryのみ（超軽量）、第2段階は選択ツールの全スキーマ展開
    ///
    /// compactとの違い: compactは名前+description、progressiveは名前+summary（より短い1行）
    pub fn format_schemas_progressive(
        &self,
        tools: &[&dyn Tool],
        expanded_names: &[&str],
    ) -> String {
        if tools.is_empty() {
            return String::new();
        }

        let mut output = String::from(
            "# 使用可能なツール

",
        );

        for tool in tools {
            if expanded_names.contains(&tool.name()) {
                // 第2段階: 全スキーマ展開
                output.push_str(&format!(
                    "## {}
",
                    tool.name()
                ));
                output.push_str(&format!(
                    "{}
",
                    tool.description()
                ));
                output.push_str(&format!(
                    "パラメータ: {}

",
                    serde_json::to_string_pretty(&tool.parameters_schema())
                        .unwrap_or_else(|_| "{}".to_string())
                ));
            } else {
                // 第1段階: 名前+summaryのみ（descriptionの先頭40文字をsummary代替）
                let desc = tool.description();
                let summary = if desc.chars().count() <= 40 {
                    desc.to_string()
                } else {
                    match desc.char_indices().nth(40) {
                        Some((idx, _)) => format!("{}…", &desc[..idx]),
                        None => desc.to_string(),
                    }
                };
                output.push_str(&format!(
                    "- **{}**: {}
",
                    tool.name(),
                    summary
                ));
            }
        }
        output
    }

    /// 登録済みツール名のHashSetを返す（バリデーション用）
    pub fn known_names(&self) -> std::collections::HashSet<String> {
        self.tools.keys().cloned().collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用のダミーツール
    struct DummyTool {
        name: String,
        description: String,
    }

    impl DummyTool {
        fn new(name: &str, desc: &str) -> Self {
            Self {
                name: name.to_string(),
                description: desc.to_string(),
            }
        }
    }

    impl Tool for DummyTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            &self.description
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn permission(&self) -> Permission {
            Permission::Auto
        }
        fn call(&self, _args: serde_json::Value) -> Result<ToolResult> {
            Ok(ToolResult {
                output: "ok".to_string(),
                success: true,
            })
        }
    }

    fn build_registry() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(DummyTool::new(
            "shell",
            "シェルコマンドを実行する",
        )));
        reg.register(Box::new(DummyTool::new("file_read", "ファイルを読み込む")));
        reg.register(Box::new(DummyTool::new("file_write", "ファイルに書き込む")));
        reg.register(Box::new(DummyTool::new(
            "memory_search",
            "メモリを検索する",
        )));
        reg.register(Box::new(DummyTool::new("git", "Gitリポジトリを操作する")));
        reg.register(Box::new(DummyTool::new("web_search", "Webを検索する")));
        reg
    }

    #[test]
    fn test_register_and_get() {
        let reg = build_registry();
        assert_eq!(reg.len(), 6);
        assert!(reg.get("shell").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn test_names() {
        let reg = build_registry();
        let names = reg.names();
        assert_eq!(names.len(), 6);
        assert!(names.contains(&"shell"));
    }

    #[test]
    fn test_select_relevant_keyword_match() {
        let reg = build_registry();
        // 「ファイル」で検索 → file_read, file_write がマッチ
        let selected = reg.select_relevant("ファイルを読みたい", 5);
        assert!(!selected.is_empty());
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"file_read"));
    }

    #[test]
    fn test_select_relevant_max_limit() {
        let reg = build_registry();
        let selected = reg.select_relevant("操作 実行 検索", 2);
        assert!(selected.len() <= 2);
    }

    #[test]
    fn test_select_relevant_no_match() {
        let reg = build_registry();
        // マッチしないクエリでも全ツール（スコア0）が返る
        let selected = reg.select_relevant("xyz123", 3);
        assert_eq!(selected.len(), 3);
    }

    #[test]
    fn test_format_schemas() {
        let reg = build_registry();
        let tools: Vec<&dyn Tool> = vec![reg.get("shell").unwrap()];
        let formatted = reg.format_schemas(&tools);
        assert!(formatted.contains("# 使用可能なツール"));
        assert!(formatted.contains("## shell"));
        assert!(formatted.contains("シェルコマンドを実行する"));
    }

    #[test]
    fn test_format_schemas_empty() {
        let reg = ToolRegistry::new();
        let formatted = reg.format_schemas(&[]);
        assert!(formatted.is_empty());
    }

    #[test]
    fn test_known_names() {
        let reg = build_registry();
        let known = reg.known_names();
        assert!(known.contains("shell"));
        assert!(!known.contains("unknown"));
    }

    #[test]
    fn test_tool_call() {
        let reg = build_registry();
        let tool = reg.get("shell").unwrap();
        let result = tool.call(serde_json::json!({})).unwrap();
        assert!(result.success);
    }

    #[test]
    fn test_tool_schema() {
        let reg = build_registry();
        let tool = reg.get("shell").unwrap();
        let schema = tool.schema();
        assert_eq!(schema.name, "shell");
        assert!(!schema.description.is_empty());
    }

    #[test]
    fn test_format_schemas_compact() {
        let reg = build_registry();
        let all: Vec<&dyn Tool> = reg.tools.values().map(|t| t.as_ref()).collect();
        let compact = reg.format_schemas_compact(&all);
        let full = reg.format_schemas(&all);
        // compact版はフル版より短い
        assert!(compact.len() < full.len());
        // 名前と説明は含まれる
        assert!(compact.contains("file_read"));
        assert!(compact.contains("shell"));
    }

    #[test]
    fn test_format_schemas_compact_empty() {
        let reg = build_registry();
        let compact = reg.format_schemas_compact(&[]);
        assert!(compact.is_empty());
    }

    #[test]
    fn test_format_schemas_compact_has_description() {
        let reg = build_registry();
        let tool = reg.get("file_read").unwrap();
        let compact = reg.format_schemas_compact(&[tool]);
        assert!(compact.contains("ファイル"));
    }

    #[test]
    fn test_format_schemas_compact_no_params() {
        let reg = build_registry();
        let tool = reg.get("shell").unwrap();
        let compact = reg.format_schemas_compact(&[tool]);
        // JSONスキーマが含まれない
        assert!(!compact.contains("properties"));
    }

    #[test]
    fn test_task_boost_file_query() {
        let reg = build_registry();
        let selected = reg.select_relevant("ファイルを読みたい", 3);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        // file系ツールがブーストされて上位に来る
        assert_eq!(names[0], "file_read");
    }

    #[test]
    fn test_task_boost_git_query() {
        let reg = build_registry();
        let selected = reg.select_relevant("gitのコミット履歴を見たい", 3);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"git"));
    }

    #[test]
    fn test_is_read_only_default_false() {
        // Tool トレイトの is_read_only() デフォルト値は false
        let tool = DummyTool::new("test", "test tool");
        assert!(
            !tool.is_read_only(),
            "デフォルトのis_read_onlyはfalseであるべき"
        );
    }

    /// is_read_only を true にオーバーライドするテスト用ツール
    struct ReadOnlyTool;

    impl Tool for ReadOnlyTool {
        fn name(&self) -> &str {
            "read_only_tool"
        }
        fn description(&self) -> &str {
            "読取専用テストツール"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn permission(&self) -> Permission {
            Permission::Auto
        }
        fn call(&self, _args: serde_json::Value) -> Result<ToolResult> {
            Ok(ToolResult {
                output: "ok".to_string(),
                success: true,
            })
        }
        fn is_read_only(&self) -> bool {
            true
        }
    }

    #[test]
    fn test_is_read_only_override_true() {
        // is_read_only() をオーバーライドして true にできる
        let tool = ReadOnlyTool;
        assert!(tool.is_read_only(), "オーバーライドでtrueになるべき");
    }
    #[test]
    fn test_task_boost_no_boost() {
        let reg = build_registry();
        // ブーストキーワードなしのクエリ
        let selected = reg.select_relevant("天気を教えて", 3);
        assert_eq!(selected.len(), 3);
    }

    /// Tool トレイトが Send+Sync を要求していることのコンパイル時保証
    fn _assert_tool_send_sync<T: Tool>() {}

    #[test]
    fn test_tool_send_sync_compile_time() {
        _assert_tool_send_sync::<DummyTool>();
    }

    #[test]
    fn test_tool_parallel_call_via_thread_scope() {
        let reg = build_registry();
        let tool = reg.get("shell").unwrap();

        std::thread::scope(|s| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    s.spawn(|| {
                        let result = tool.call(serde_json::json!({})).unwrap();
                        assert!(result.success);
                        result.output.clone()
                    })
                })
                .collect();
            let results: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert_eq!(results.len(), 4);
            assert!(results.iter().all(|r| r == "ok"));
        });
    }

    #[test]
    fn test_registry_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<ToolRegistry>();
    }

    #[test]
    fn test_warn_if_exceeded_no_panic() {
        let reg = ToolRegistry::new();
        reg.warn_if_exceeded(8); // 空 → 警告なし
        // パニックしないことのみ確認
    }

    #[test]
    fn test_max_tools_in_context_default() {
        let settings = crate::config::AgentSettings::default();
        assert_eq!(settings.max_tools_in_context, 8);
    }

    #[test]
    fn test_detect_task_type_file_operation() {
        assert_eq!(
            detect_task_type("ファイルを読みたい"),
            TaskType::FileOperation
        );
        assert_eq!(detect_task_type("設定を書き込む"), TaskType::FileOperation);
        assert_eq!(
            detect_task_type("コードを編集する"),
            TaskType::FileOperation
        );
    }

    #[test]
    fn test_detect_task_type_code_execution() {
        assert_eq!(
            detect_task_type("コマンドを実行する"),
            TaskType::CodeExecution
        );
        assert_eq!(
            detect_task_type("プロジェクトをビルドしたい"),
            TaskType::CodeExecution
        );
        assert_eq!(
            detect_task_type("テストを走らせる"),
            TaskType::CodeExecution
        );
    }

    #[test]
    fn test_detect_task_type_research() {
        assert_eq!(detect_task_type("Webで検索して"), TaskType::Research);
        assert_eq!(detect_task_type("この問題を調べて"), TaskType::Research);
        assert_eq!(detect_task_type("最新の論文を探す"), TaskType::Research);
        assert_eq!(detect_task_type("このURLを開いて"), TaskType::Research);
    }

    #[test]
    fn test_detect_task_type_general() {
        assert_eq!(detect_task_type("天気を教えて"), TaskType::General);
        assert_eq!(detect_task_type("こんにちは"), TaskType::General);
    }

    #[test]
    fn test_select_relevant_with_type_file_operation() {
        let reg = build_registry();
        // ファイル操作クエリ → file_read, file_writeのみに絞られる
        let selected = reg.select_relevant_with_type("ファイルを読みたい", 10);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"file_write"));
        // shell, git, web_search等は含まれない
        assert!(!names.contains(&"shell"));
        assert!(!names.contains(&"git"));
        assert!(!names.contains(&"web_search"));
    }

    #[test]
    fn test_select_relevant_with_type_code_execution() {
        let reg = build_registry();
        let selected = reg.select_relevant_with_type("コマンドを実行する", 10);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"git"));
        assert!(!names.contains(&"file_read"));
    }

    #[test]
    fn test_select_relevant_with_type_research() {
        let reg = build_registry();
        let selected = reg.select_relevant_with_type("Webで検索して", 10);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"web_search"));
        assert!(!names.contains(&"shell"));
    }

    #[test]
    fn test_select_relevant_with_type_general_no_filter() {
        let reg = build_registry();
        // Generalは全ツールが候補（既存動作と同一）
        let selected = reg.select_relevant_with_type("天気を教えて", 10);
        assert_eq!(selected.len(), 6); // build_registry()は6ツール登録
    }

    #[test]
    fn test_select_relevant_with_type_respects_max() {
        let reg = build_registry();
        let selected = reg.select_relevant_with_type("天気を教えて", 2);
        assert!(selected.len() <= 2);
    }

    #[test]
    fn test_select_relevant_unchanged() {
        // 既存のselect_relevantが変更されていないことを確認
        let reg = build_registry();
        let selected = reg.select_relevant("ファイルを読みたい", 5);
        // select_relevantはフィルタなし → 5件返る
        assert_eq!(selected.len(), 5);
    }

    #[test]
    fn test_task_type_allowed_prefixes() {
        assert!(TaskType::FileOperation.allowed_prefixes().is_some());
        assert!(TaskType::CodeExecution.allowed_prefixes().is_some());
        assert!(TaskType::Research.allowed_prefixes().is_some());
        assert!(TaskType::General.allowed_prefixes().is_none());
    }

    #[test]
    fn test_format_schemas_progressive_collapsed() {
        // 展開対象なし — 全ツールがsummary形式（第1段階）
        let reg = build_registry();
        let all: Vec<&dyn Tool> = reg.tools.values().map(|t| t.as_ref()).collect();
        let progressive = reg.format_schemas_progressive(&all, &[]);
        // パラメータスキーマは含まれない
        assert!(!progressive.contains("properties"));
        // 名前は含まれる
        assert!(progressive.contains("shell"));
        assert!(progressive.contains("file_read"));
    }

    #[test]
    fn test_format_schemas_progressive_expanded() {
        // shellのみ展開 — 他はsummary形式
        let reg = build_registry();
        let all: Vec<&dyn Tool> = reg.tools.values().map(|t| t.as_ref()).collect();
        let progressive = reg.format_schemas_progressive(&all, &["shell"]);
        // shellはフルスキーマ展開（##ヘッダ + パラメータ）
        assert!(progressive.contains("## shell"));
        // 他のツールはsummary形式（- **name**: ...）
        assert!(progressive.contains("- **file_read**"));
    }

    #[test]
    fn test_format_schemas_progressive_empty() {
        let reg = build_registry();
        let progressive = reg.format_schemas_progressive(&[], &[]);
        assert!(progressive.is_empty());
    }

    #[test]
    fn test_format_schemas_progressive_shorter_than_full() {
        // progressive（第1段階）はフル展開より短い
        let reg = build_registry();
        let all: Vec<&dyn Tool> = reg.tools.values().map(|t| t.as_ref()).collect();
        let progressive = reg.format_schemas_progressive(&all, &[]);
        let full = reg.format_schemas(&all);
        assert!(progressive.len() < full.len());
    }

    #[test]
    fn test_format_schemas_progressive_all_expanded() {
        // 全ツール展開 — format_schemasと同等の情報量
        let reg = build_registry();
        let all: Vec<&dyn Tool> = reg.tools.values().map(|t| t.as_ref()).collect();
        let names: Vec<&str> = all.iter().map(|t| t.name()).collect();
        let progressive = reg.format_schemas_progressive(&all, &names);
        // 全ツールが ## ヘッダで展開
        assert!(progressive.contains("## shell"));
        assert!(progressive.contains("## file_read"));
        // summary形式（- **name**）は含まれない
        assert!(!progressive.contains("- **shell**"));
    }

    #[test]
    fn t_cache_hit() {
        // 同じ引数で2回目はキャッシュから返る
        let mut cache = ToolResultCache::new();
        let args = serde_json::json!({"path": "src/main.rs"});
        let result = ToolResult {
            output: "fn main()".to_string(),
            success: true,
        };
        cache.put("file_read", &args, result);

        let cached = cache.get("file_read", &args);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().output, "fn main()");
        assert!(cached.unwrap().success);
    }

    #[test]
    fn t_cache_miss() {
        // 異なる引数はミス
        let mut cache = ToolResultCache::new();
        let args1 = serde_json::json!({"path": "src/main.rs"});
        let args2 = serde_json::json!({"path": "src/lib.rs"});
        let result = ToolResult {
            output: "content".to_string(),
            success: true,
        };
        cache.put("file_read", &args1, result);

        let cached = cache.get("file_read", &args2);
        assert!(cached.is_none());
    }

    #[test]
    fn t_cache_invalidate() {
        // 書き込み後にキャッシュクリアされる
        let mut cache = ToolResultCache::new();
        let args = serde_json::json!({"path": "src/main.rs"});
        cache.put(
            "file_read",
            &args,
            ToolResult {
                output: "old".to_string(),
                success: true,
            },
        );
        cache.put(
            "repo_map",
            &serde_json::json!({}),
            ToolResult {
                output: "map".to_string(),
                success: true,
            },
        );
        assert_eq!(cache.len(), 2);

        // file_readのキャッシュだけ無効化
        cache.invalidate("file_read");
        assert_eq!(cache.len(), 1);
        assert!(cache.get("file_read", &args).is_none());

        // repo_mapは残っている
        let map_cached = cache.get("repo_map", &serde_json::json!({}));
        assert!(map_cached.is_some());
    }

    #[test]
    fn t_cache_stats() {
        // ヒット/ミス統計が正しい
        let mut cache = ToolResultCache::new();
        let args = serde_json::json!({"path": "test.rs"});
        cache.put(
            "file_read",
            &args,
            ToolResult {
                output: "ok".to_string(),
                success: true,
            },
        );

        // 1回ヒット
        let _ = cache.get("file_read", &args);
        // 1回ミス
        let _ = cache.get("file_read", &serde_json::json!({"path": "other.rs"}));
        // もう1回ヒット
        let _ = cache.get("file_read", &args);

        let (hits, misses) = cache.stats();
        assert_eq!(hits, 2);
        assert_eq!(misses, 1);
    }

    #[test]
    fn t_cache_read_only_only() {
        // 書き込みツールはキャッシュしないことを示すテスト
        // ToolResultCache自体はツールのis_read_only()を知らないので、
        // 呼び出し側がis_read_only()チェック後にのみputすることを期待する。
        // ここではclearの動作を確認する。
        let mut cache = ToolResultCache::new();
        let args = serde_json::json!({"command": "ls"});
        // 仮にshell結果をputしても…
        cache.put(
            "shell",
            &args,
            ToolResult {
                output: "files".to_string(),
                success: true,
            },
        );
        assert_eq!(cache.len(), 1);
        // clearで全消去される
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    fn build_registry_with_mcp() -> ToolRegistry {
        let mut reg = build_registry(); // ビルトイン6ツール
        // MCPツール（コロン付き名前）を追加
        reg.register(Box::new(DummyTool::new("fs:read_file", "ファイルを読む")));
        reg.register(Box::new(DummyTool::new("fs:write_file", "ファイルに書く")));
        reg.register(Box::new(DummyTool::new(
            "fs:list_dir",
            "ディレクトリを一覧",
        )));
        reg.register(Box::new(DummyTool::new("fs:search", "ファイルを検索")));
        reg.register(Box::new(DummyTool::new("db:query", "DBクエリ実行")));
        reg
    }

    #[test]
    fn test_select_relevant_split_separates_builtin_and_mcp() {
        let reg = build_registry_with_mcp();
        // ビルトイン最大8、MCP最大3
        let selected = reg.select_relevant_split("ファイルを読みたい", 8, 3);
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        // ビルトイン6ツール全部入る（8枠）
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"file_read"));
        // MCPも最大3件入る
        let mcp_count = names.iter().filter(|n| n.contains(':')).count();
        assert!(mcp_count <= 3);
        assert!(mcp_count > 0);
    }

    #[test]
    fn test_select_relevant_split_mcp_limit() {
        let reg = build_registry_with_mcp();
        // MCP枠を1に制限
        let selected = reg.select_relevant_split("ファイルを読みたい", 8, 1);
        let mcp_count = selected.iter().filter(|t| t.name().contains(':')).count();
        assert_eq!(mcp_count, 1);
    }

    #[test]
    fn test_select_relevant_split_no_mcp() {
        let reg = build_registry(); // MCPなし
        let selected = reg.select_relevant_split("ファイルを読みたい", 8, 3);
        // ビルトインのみ
        assert_eq!(selected.len(), 6);
        assert!(selected.iter().all(|t| !t.name().contains(':')));
    }

    #[test]
    fn test_select_relevant_split_zero_mcp() {
        let reg = build_registry_with_mcp();
        // MCP枠を0にすればMCPは選ばれない
        let selected = reg.select_relevant_split("ファイルを読みたい", 8, 0);
        assert!(selected.iter().all(|t| !t.name().contains(':')));
    }

    #[test]
    fn test_max_mcp_tools_in_context_default() {
        let settings = crate::config::AgentSettings::default();
        assert_eq!(settings.max_mcp_tools_in_context, 3);
    }
}
