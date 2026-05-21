use crate::agent::conversation::{Message, Role};
use crate::memory::store::MemoryStore;
use crate::observability::logger::{LogLevel, log_event};
use crate::runtime::embedder::{Embedder, cosine_similarity};
use std::collections::HashMap;

/// ContextOverflowGuard 派生 budget 比率 (n_ctx の 70% を bonsai 側 budget とする)
const CONTEXT_GUARD_RATIO_NUM: usize = 70;
const CONTEXT_GUARD_RATIO_DEN: usize = 100;

pub struct CompactionConfig {
    pub large_output_threshold: usize,
    pub placeholder_keep_recent: usize,
    pub summary_max_chars: usize,
    pub emergency_keep: usize,
    pub max_context_tokens: usize,
    /// Prune最小閾値（この行数以下のToolメッセージは削除しない、OpenCode知見）
    pub prune_minimum_chars: usize,
    /// Prune保護範囲（直近このトークン数分のメ���セージは削除対象外、OpenCode知見）
    pub prune_protect_tokens: usize,
    /// 項目 248 Phase 4: dynamic budget ratio (Zenn 4 architecture 配分).
    ///
    /// `None` (default) で backward compat = 既存 prune logic.
    /// `Some(_)` で `dynamic_budget_for_compaction` が allocate を返し、`compact_if_needed`
    /// が `[INFO][compaction.budget]` log を emit (将来 4 軸個別 prune の hook).
    pub budget_ratios: Option<BudgetRatios>,
}
impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            large_output_threshold: 5000,
            placeholder_keep_recent: 6,
            summary_max_chars: 200,
            emergency_keep: 4,
            max_context_tokens: 14000,
            prune_minimum_chars: 50,
            prune_protect_tokens: 4000,
            // 項目 248 Phase 4: backward compat — env unset で None
            budget_ratios: None,
        }
    }
}

impl CompactionConfig {
    /// LLM の n_ctx から派生する保守的な圧縮予算 (ratio = 70%)。
    /// `None` または `Some(0)` で legacy default (max_context_tokens=14000) を維持。
    ///
    /// 派生時は `prune_protect_tokens` も新 budget の半分以下にクランプし、
    /// 圧縮が機能する余地を確保する。
    pub fn from_n_ctx_budget(n_ctx_budget: Option<u32>) -> Self {
        let mut config = Self::default();
        let Some(n_ctx) = n_ctx_budget else {
            return config;
        };
        if n_ctx == 0 {
            return config;
        }
        let derived =
            (n_ctx as usize).saturating_mul(CONTEXT_GUARD_RATIO_NUM) / CONTEXT_GUARD_RATIO_DEN;
        config.max_context_tokens = derived.max(config.emergency_keep);
        config.prune_protect_tokens = config
            .prune_protect_tokens
            .min(config.max_context_tokens / 2);
        config
    }

    /// 項目 248 Phase 4: env-driven dynamic budget の wiring factory.
    ///
    /// `BONSAI_DYNAMIC_BUDGET ∈ {"1","true","TRUE","yes","YES"}` のとき `BudgetRatios` を
    /// `budget_ratios` に設定 (`is_dynamic_budget_enabled` matcher と整合):
    /// - `BONSAI_DYNAMIC_BUDGET_RATIOS` が valid (4 要素 + sum 1.0 + 各 ≥ 0.0 finite)
    ///   なら env override 採用
    /// - そうでなければ `BudgetRatios::default()` (40/30/20/10)
    ///
    /// env unset で None 維持 (backward compat). critic F6 follow-up で accept 値 list を明示.
    pub fn with_dynamic_budget_from_env(mut self) -> Self {
        if is_dynamic_budget_enabled() {
            self.budget_ratios = Some(dynamic_budget_ratios_from_env().unwrap_or_default());
        }
        self
    }
}

// ─── Dynamic Budget Ratios (項目 248、plan dynamic-token-budget-compaction.md §3.1) ───
//
// Phase 1 Red: skeleton (全 ratio=0.0、allocate は空、env getter は常に None)。
// Phase 2 Green で full impl、Phase 4 で compact_if_needed 統合 (本 PR scope 外)。

/// メモリ種別ごとの budget 配分 ratio (Zenn 4 architecture 配分の bonsai 適用).
#[derive(Debug, Clone, Copy)]
pub struct BudgetRatios {
    pub recent_buffer: f32,
    pub conversation_summary: f32,
    pub relevant_entities: f32,
    pub knowledge_graph: f32,
}

impl Default for BudgetRatios {
    fn default() -> Self {
        // Phase 2 Green: plan §3.1 base ratio 40/30/20/10
        Self {
            recent_buffer: 0.4,
            conversation_summary: 0.3,
            relevant_entities: 0.2,
            knowledge_graph: 0.1,
        }
    }
}

/// 配分結果 (各種別ごとの絶対 token 数).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocatedBudget {
    pub total: usize,
    pub buffer: usize,
    pub summary: usize,
    pub entities: usize,
    pub kg: usize,
}

/// 1 task ごとの種別 relevance score (0.0..=1.0).
#[derive(Debug, Clone, Copy)]
pub struct MemoryRelevance {
    pub buffer: f32,
    pub summary: f32,
    pub entities: f32,
    pub kg: f32,
}

impl Default for MemoryRelevance {
    fn default() -> Self {
        Self {
            buffer: 1.0,
            summary: 0.5,
            entities: 0.5,
            kg: 0.5,
        }
    }
}

impl BudgetRatios {
    /// 合計が 1.0 ±ε か (Phase 1 Red では全 0 で false).
    ///
    /// critic F4 follow-up: 各 ratio が finite + ≥ 0.0 であることも要件化
    /// (`BONSAI_DYNAMIC_BUDGET_RATIOS="-0.5,0.5,0.5,0.5"` のような sum=1.0 だが負の値を含む
    /// override を reject、Lab paired 起動時の `f32 → usize` cast 飽和による隠れ歪み回避).
    pub fn is_normalized(&self) -> bool {
        let parts = [
            self.recent_buffer,
            self.conversation_summary,
            self.relevant_entities,
            self.knowledge_graph,
        ];
        if !parts.iter().all(|p| p.is_finite() && *p >= 0.0) {
            return false;
        }
        let sum: f32 = parts.iter().sum();
        (sum - 1.0).abs() < 0.001
    }

    /// `total` トークンを 4 軸 ratio で按分、余りは buffer に寄せる.
    pub fn allocate(&self, total: usize) -> AllocatedBudget {
        let t = total as f32;
        let buffer = (t * self.recent_buffer) as usize;
        let summary = (t * self.conversation_summary) as usize;
        let entities = (t * self.relevant_entities) as usize;
        let kg = (t * self.knowledge_graph) as usize;
        let sum = buffer + summary + entities + kg;
        let remainder = total.saturating_sub(sum);
        AllocatedBudget {
            total,
            buffer: buffer + remainder,
            summary,
            entities,
            kg,
        }
    }

    /// relevance score に応じて ratio を動的調整、正規化後返す.
    ///
    /// 計算式: new_ratio = base × (1 + (relevance - 0.5) × α)、α は `dynamic_budget_alpha()`。
    /// 全 ratio 合計が 0 以下になった場合は self を返す (異常入力 safeguard)。
    pub fn adjusted(&self, relevance: &MemoryRelevance) -> BudgetRatios {
        let alpha = dynamic_budget_alpha();
        let adjust = |base: f32, rel: f32| base * (1.0 + (rel - 0.5) * alpha);
        let nb = adjust(self.recent_buffer, relevance.buffer);
        let ns = adjust(self.conversation_summary, relevance.summary);
        let ne = adjust(self.relevant_entities, relevance.entities);
        let nk = adjust(self.knowledge_graph, relevance.kg);
        let sum = nb + ns + ne + nk;
        if sum <= 0.0 {
            return *self;
        }
        BudgetRatios {
            recent_buffer: nb / sum,
            conversation_summary: ns / sum,
            relevant_entities: ne / sum,
            knowledge_graph: nk / sum,
        }
    }
}

/// `BONSAI_DYNAMIC_BUDGET=1` で dynamic budget 経路を有効化.
pub fn is_dynamic_budget_enabled() -> bool {
    matches!(
        std::env::var("BONSAI_DYNAMIC_BUDGET").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// `BONSAI_DYNAMIC_BUDGET_RATIOS="0.4,0.3,0.2,0.1"` で ratio override.
///
/// 4 要素 + 合計が 1.0 ±ε でない場合は `None`、default fallback。
pub fn dynamic_budget_ratios_from_env() -> Option<BudgetRatios> {
    let val = std::env::var("BONSAI_DYNAMIC_BUDGET_RATIOS").ok()?;
    let parts: Vec<f32> = val
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
    if parts.len() != 4 {
        return None;
    }
    let r = BudgetRatios {
        recent_buffer: parts[0],
        conversation_summary: parts[1],
        relevant_entities: parts[2],
        knowledge_graph: parts[3],
    };
    if r.is_normalized() { Some(r) } else { None }
}

/// `BONSAI_DYNAMIC_BUDGET_ALPHA` で adjusted の α 係数 override (default 0.2、範囲 [0.0, 1.0])。
pub fn dynamic_budget_alpha() -> f32 {
    std::env::var("BONSAI_DYNAMIC_BUDGET_ALPHA")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|a| (0.0..=1.0).contains(a))
        .unwrap_or(0.2)
}

/// 項目 248 Phase 4: `CompactionConfig.budget_ratios` が `Some` のとき allocate を返す.
///
/// `compact_if_needed` から呼出され、Some の場合 `[INFO][compaction.budget]` log を emit.
/// 将来 4 軸個別 prune の hook (現状は log emit のみで挙動変化なし、backward compat).
pub fn dynamic_budget_for_compaction(config: &CompactionConfig) -> Option<AllocatedBudget> {
    config
        .budget_ratios
        .as_ref()
        .map(|r| r.allocate(config.max_context_tokens))
}

/// `BONSAI_DYNAMIC_BUDGET_*` env を弄る test 間 cross-file mutex.
#[cfg(test)]
pub(crate) static DYNAMIC_BUDGET_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// メッセージ列の概算トークン数を保守的に算出する。
///
/// ASCII (chars/3) と UTF-8 byte ベース (bytes*0.4) の `max` を取り、
/// 日本語混在テキストで旧実装より実 BPE トークン数を下回りにくい値を返す。
/// 旧実装 `len()/4` は日本語比率高で実値の 50% 程度しか見積らず、
/// llama-server n_ctx を超過する prompt を許してしまっていた (項目 186 H6 CONTEXT_OVERFLOW)。
/// なお code/base64/symbol-heavy 出力では `0.4 token/byte` 仮定が破れ得るため、
/// `from_n_ctx_budget` の 70% ratio がヘッドルームとして機能する。
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| estimate_message_tokens(&m.content))
        .sum()
}

/// 単一テキストのトークン数推定 — ASCII (chars/3) と UTF-8 byte (bytes*0.4)
/// の `max` で算出。CompactionMiddleware の累積トークン見積りに使用。
pub fn estimate_message_tokens(content: &str) -> usize {
    let by_chars = content.chars().count().div_ceil(3);
    let by_utf8 = (content.len() * 4).div_ceil(10);
    by_chars.max(by_utf8).max(1)
}
/// AI+Toolメッセージペアを検出
pub fn find_ai_tool_pairs(messages: &[Message]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for i in 0..messages.len().saturating_sub(1) {
        if matches!(messages[i].role, Role::Assistant) && matches!(messages[i + 1].role, Role::Tool)
        {
            pairs.push((i, i + 1));
        }
    }
    pairs
}

/// Assistantメッセージ内の<tool_call>からツール名を抽出し、使用回数を集計
///
/// tool_callのJSONから"name"フィールドを正規表現で取得するため、
/// parse.rsへの依存を避けつつ正確なツール名統計を提供する。
pub fn summarize_tool_usage(messages: &[Message]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for msg in messages {
        if matches!(msg.role, Role::Assistant) {
            // <tool_call>ブロック内の"name":"xxx"を抽出
            let mut remaining = msg.content.as_str();
            while let Some(start) = remaining.find("<tool_call>") {
                let after_tag = &remaining[start + 11..];
                if let Some(end) = after_tag.find("</tool_call>") {
                    let block = &after_tag[..end];
                    // "name" : "tool_name" パターンを検索
                    if let Some(name) = extract_name_from_json(block) {
                        *counts.entry(name).or_insert(0) += 1;
                    }
                    remaining = &after_tag[end + 12..];
                } else {
                    break;
                }
            }
        }
    }
    counts
}

/// JSONブロックから"name"フィールドの値を抽出するヘルパー
fn extract_name_from_json(json_str: &str) -> Option<String> {
    // "name" の位置を検索（空白許容）
    let name_key = json_str.find("\"name\"")?;
    let after_key = &json_str[name_key + 6..];
    // コロンを探す
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    // 値の開始引用符
    if !after_colon.starts_with('"') {
        return None;
    }
    let value_start = &after_colon[1..];
    let end_quote = value_start.find('"')?;
    Some(value_start[..end_quote].to_string())
}

/// Assistantメッセージから<think>ブロックの結論部分を抽出（GLM-5.1 Preserved Thinking知見）
///
/// 各thinkブロックの最後の文（結論部分）を最大3件保持し、
/// 推論の連続性を保護する。200文字で切り詰め。
pub fn extract_thinking_summary(messages: &[Message]) -> Vec<String> {
    let mut summaries = Vec::new();
    for msg in messages {
        if !matches!(msg.role, Role::Assistant) {
            continue;
        }
        let mut remaining = msg.content.as_str();
        while let Some(start) = remaining.find("<think>") {
            let after_tag = &remaining[start + 7..];
            if let Some(end) = after_tag.find("</think>") {
                let block = after_tag[..end].trim();
                if !block.is_empty() {
                    let last_sentence = extract_last_sentence(block);
                    if !last_sentence.is_empty() {
                        let truncated: String = last_sentence.chars().take(200).collect();
                        summaries.push(truncated);
                        if summaries.len() >= 3 {
                            return summaries;
                        }
                    }
                }
                remaining = &after_tag[end + 8..];
            } else {
                break;
            }
        }
    }
    summaries
}

/// thinkブロックから最後の文を抽出するヘルパー
fn extract_last_sentence(block: &str) -> String {
    let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
    if let Some(last_line) = lines.last() {
        last_line.trim().to_string()
    } else {
        block.trim().to_string()
    }
}

/// 最後のAssistant/Toolメッセージからタスクの成果を200文字以内で抽出
///
/// 最後のAssistantメッセージの内容を優先し、<think>タグは除外する。
/// Assistantメッセージがない場合は最後のToolメッセージから抽出。
pub fn extract_last_outcome(messages: &[Message]) -> Option<String> {
    // 最後のAssistantメッセージを探す（<think>を除外）
    let last_assistant = messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::Assistant));
    if let Some(msg) = last_assistant {
        let cleaned = strip_think_tags(&msg.content);
        let trimmed = cleaned.trim();
        if !trimmed.is_empty() {
            let outcome: String = trimmed.chars().take(200).collect();
            return Some(outcome);
        }
    }
    // フォールバック: 最後のToolメッセージ
    let last_tool = messages.iter().rev().find(|m| matches!(m.role, Role::Tool));
    if let Some(msg) = last_tool {
        let trimmed = msg.content.trim();
        if !trimmed.is_empty() {
            let outcome: String = trimmed.chars().take(200).collect();
            return Some(outcome);
        }
    }
    None
}

/// <think>...</think> タグとその中身を除去
fn strip_think_tags(s: &str) -> String {
    let mut result = String::new();
    let mut remaining = s;
    while let Some(start) = remaining.find("<think>") {
        result.push_str(&remaining[..start]);
        if let Some(end) = remaining[start..].find("</think>") {
            remaining = &remaining[start + end + 8..];
        } else {
            // 閉じタグなし: <think>以降を全て除去
            return result;
        }
    }
    result.push_str(remaining);
    result
}

/// Tool メッセージが実際のツール実行エラーかを判定（項目 178）
///
/// tool_exec.rs:78 の format `"ツール実行エラー: {e}"` を prefix で照合。
/// 旧実装 `content.contains("エラー")` は file_read で読んだソース内の
/// 「エラーハンドリング」コメント等で偽陽性を起こしていた (項目 175 と同症状)。
/// support.rs:31-42 の `check_invariants` と整合する prefix セットを使用。
fn is_tool_error_message(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with("ツール実行エラー")
        || trimmed.starts_with("Error:")
        || trimmed.starts_with("[Tool error]")
}

/// メッセージの重要度スコアを計算（GLM-5.1 DSA知見）
///
/// トークン重要度による動的注意配分。重要度が低いメッセージから優先的に削除。
pub fn score_message_importance(msg: &Message) -> f64 {
    match msg.role {
        Role::User => 1.0,
        Role::System => {
            if msg.content.contains("<context") || msg.content.contains("<memory-context") {
                0.3
            } else {
                0.9
            }
        }
        Role::Assistant => {
            if msg.content.contains("<tool_call>") {
                0.7
            } else {
                0.4
            }
        }
        Role::Tool => {
            if is_tool_error_message(&msg.content) {
                0.2
            } else {
                0.5
            }
        }
    }
}

/// セマンティック重要度スコアラー（P1 Step 4: embedding-based importance scoring）
///
/// 固定役割スコア（score_message_importance）とタスクコンテキストとの
/// コサイン類似度を 6:4 でブレンドし、動的な重要度を算出する。
/// embedderはBox<dyn Embedder>で所有、task_embeddingは set_task() で一度だけ計算。
pub struct SemanticScorer {
    embedder: Box<dyn Embedder>,
    task_embedding: Vec<f32>,
}

impl SemanticScorer {
    /// タスクコンテキストから初期化。失敗時はErr（呼び出し側でフォールバック判断）。
    pub fn new(embedder: Box<dyn Embedder>, task_context: &str) -> anyhow::Result<Self> {
        let truncated: String = task_context.chars().take(500).collect();
        let mut vecs = embedder.embed(&[truncated.as_str()])?;
        let task_embedding = vecs
            .pop()
            .ok_or_else(|| anyhow::anyhow!("embedder returned empty vector"))?;
        Ok(Self {
            embedder,
            task_embedding,
        })
    }

    /// メッセージの動的重要度スコア（0.0-1.0）
    ///
    /// ブレンド: 0.6 * 固定役割スコア + 0.4 * ((cosine_sim + 1) / 2)
    /// コサイン類似度 [-1, 1] を [0, 1] にマッピング。
    pub fn score(&self, msg: &Message) -> f64 {
        let base = score_message_importance(msg);
        let preview: String = msg.content.chars().take(500).collect();
        if preview.trim().is_empty() {
            return base;
        }
        match self.embedder.embed(&[preview.as_str()]) {
            Ok(mut vecs) => {
                if let Some(v) = vecs.pop() {
                    let sim = cosine_similarity(&v, &self.task_embedding) as f64;
                    let normalized = (sim + 1.0) / 2.0;
                    0.6 * base + 0.4 * normalized
                } else {
                    base
                }
            }
            Err(_) => base,
        }
    }
}

/// Toolメッセージからエラー（未解決事項）を検出
///
/// 項目 178: 実エラー prefix のみで照合 (`is_tool_error_message`)、
/// ソース内コメントの「エラー」を偽陽性として拾わない。
fn collect_unresolved(messages: &[Message], boundary: usize) -> Vec<String> {
    let mut errors = Vec::new();
    // 圧縮対象の末尾付近のエラーを優先的に収集
    for msg in messages[..boundary].iter().rev().take(boundary) {
        if matches!(msg.role, Role::Tool) && is_tool_error_message(&msg.content) {
            let preview: String = msg.content.chars().take(100).collect();
            if !errors.contains(&preview) {
                errors.push(preview);
            }
            if errors.len() >= 3 {
                break;
            }
        }
    }
    errors.reverse();
    errors
}

pub fn compact_level0(messages: &mut [Message], config: &CompactionConfig) -> Vec<String> {
    let mut off = Vec::new();
    for msg in messages.iter_mut() {
        if matches!(msg.role, Role::Tool) && msg.content.len() > config.large_output_threshold {
            let h = {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut ha = DefaultHasher::new();
                msg.content.hash(&mut ha);
                format!("{:x}", ha.finish())
            };
            let p = format!("/tmp/bonsai-out-{h}.txt");
            if std::fs::write(&p, &msg.content).is_ok() {
                let pv: String = msg.content.chars().take(200).collect();
                let l = msg.content.len();
                msg.content = format!("{pv}...\n[saved:{p}({l}c)]");
                off.push(p);
            }
        }
    }
    off
}
/// セマンティック版 level1: SemanticScorerで動的重要度スコアを算出
///
/// compact_level1と同じ保護ルール（最初/最後のUser、AI+Toolペア、直近トークン保護）を適用し、
/// 固定スコアの代わりにscorer.score()で削除候補を順位付けする。
pub fn compact_level1_with_scorer(
    messages: &mut [Message],
    config: &CompactionConfig,
    scorer: &SemanticScorer,
) {
    let t = messages.len();
    if t <= config.placeholder_keep_recent {
        return;
    }
    let keep_by_count = config.placeholder_keep_recent;
    let keep_by_tokens = {
        let mut acc = 0usize;
        let mut keep = 0usize;
        for msg in messages.iter().rev() {
            acc += msg.content.len().div_ceil(4);
            if acc > config.prune_protect_tokens {
                break;
            }
            keep += 1;
        }
        keep
    };
    let boundary = t.saturating_sub(keep_by_count.max(keep_by_tokens));
    if boundary == 0 {
        return;
    }
    let pairs = find_ai_tool_pairs(messages);
    let protected: std::collections::HashSet<usize> = pairs
        .iter()
        .flat_map(|&(a, b)| {
            if a >= boundary || b >= boundary {
                vec![a, b]
            } else {
                vec![]
            }
        })
        .collect();
    let first_user_idx = messages[..boundary]
        .iter()
        .position(|m| matches!(m.role, Role::User));
    let last_user_idx = messages[..boundary]
        .iter()
        .rposition(|m| matches!(m.role, Role::User));
    let mut candidates: Vec<(usize, f64)> = (0..boundary)
        .filter(|&i| {
            !protected.contains(&i) && Some(i) != first_user_idx && Some(i) != last_user_idx
        })
        .map(|i| (i, scorer.score(&messages[i])))
        .collect();
    candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    for (i, _score) in &candidates {
        let msg = &mut messages[*i];
        if matches!(msg.role, Role::Tool) && msg.content.len() > config.prune_minimum_chars {
            let id = msg.tool_call_id.as_deref().unwrap_or("?");
            msg.content = format!("[prev:{id}]");
        }
    }
}

pub fn compact_level1(messages: &mut [Message], config: &CompactionConfig) {
    compact_level1_with_budget(messages, config, None);
}
pub fn compact_level2(messages: &mut [Message], config: &CompactionConfig) {
    compact_level2_with_budget(messages, config, None);
}
pub fn compact_level3(messages: &mut Vec<Message>, config: &CompactionConfig) {
    if messages.len() <= config.emergency_keep + 1 {
        return;
    }
    let sys: Vec<Message> = messages
        .iter()
        .filter(|m| matches!(m.role, Role::System))
        .cloned()
        .collect();
    // Handoff framing: 圧縮前の要約を「引継ぎ」として挿入（hermes-agent知見）
    let handoff = build_handoff_summary(messages, config);
    let rec: Vec<Message> = messages
        .iter()
        .rev()
        .take(config.emergency_keep)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    messages.clear();
    messages.extend(sys);
    if let Some(h) = handoff {
        messages.push(h);
    }
    messages.extend(rec);
}

/// Handoff framing: 圧縮対象から要約を構築（hermes-agent/macOS26パターン）
///
/// 「別のアシスタントが引き継ぎ」として、解決済み/未解決を整理。
/// ツール使用統計・最終成果・未解決事項を含む高品質な引継ぎサマリーを生成。
/// 1bitモデルが指示と混同しないよう「Remaining Work」命名を使用。
fn build_handoff_summary(messages: &[Message], config: &CompactionConfig) -> Option<Message> {
    let boundary = messages.len().saturating_sub(config.emergency_keep);
    if boundary < 2 {
        return None;
    }
    let compressed = &messages[..boundary];

    // 圧縮対象のAssistantメッセージから解決済みタスクの要約を構築
    let mut resolved = Vec::new();
    for msg in compressed {
        if matches!(msg.role, Role::Assistant) && msg.content.len() > 20 {
            let preview: String = msg.content.chars().take(80).collect();
            resolved.push(preview);
        }
    }
    if resolved.is_empty() {
        return None;
    }

    let resolved_text = resolved
        .iter()
        .take(3)
        .map(|r| format!("- {r}"))
        .collect::<Vec<_>>()
        .join("\n");

    // ツール使用統計: どのツールを何回使ったか
    let tool_stats = summarize_tool_usage(compressed);
    let tool_stats_text = if tool_stats.is_empty() {
        String::new()
    } else {
        let mut entries: Vec<_> = tool_stats.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1));
        let stats_str = entries
            .iter()
            .take(8)
            .map(|(name, count)| format!("{name}:{count}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("\nTool stats: {stats_str}")
    };

    // 最終成果の要約
    let outcome_text = match extract_last_outcome(compressed) {
        Some(outcome) => format!("\nLast outcome: {outcome}"),
        None => String::new(),
    };

    // 未解決事項（エラー）の検出
    let unresolved = collect_unresolved(messages, boundary);
    let unresolved_text = if unresolved.is_empty() {
        String::new()
    } else {
        let items = unresolved
            .iter()
            .map(|e| format!("- {e}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\nUnresolved issues:\n{items}")
    };

    Some(Message::system(format!(
        "[Context handoff] 前のアシスタントの作業を引き継ぎます。\n\
         Resolved:\n{resolved_text}{tool_stats_text}{outcome_text}{unresolved_text}\n\
         Remaining Work: 直近のメッセージに基づいて作業を続行してください。"
    )))
}

/// コンパクション前のメモリフラッシュ: 削除対象のAssistant発言を要約してMemoryStoreに退避
pub fn flush_before_compaction(messages: &[Message], store: Option<&MemoryStore>) {
    let Some(store) = store else { return };
    let boundary = messages.len().saturating_sub(6);
    let mut flushed = Vec::new();
    for msg in &messages[..boundary] {
        if matches!(msg.role, Role::Assistant) && msg.content.len() > 100 {
            let summary: String = msg.content.chars().take(200).collect();
            flushed.push(summary);
        }
    }
    if flushed.is_empty() {
        return;
    }
    let combined = flushed.join("\n---\n");
    if let Err(e) = store.save_memory(&combined, "context_flush", &["compaction".to_string()]) {
        eprintln!("[flush] メモリ保存失敗: {e}");
    }
}
#[allow(clippy::possible_missing_else)]
pub fn compact_if_needed(
    messages: &mut Vec<Message>,
    config: &CompactionConfig,
) -> (u8, Vec<String>) {
    // 項目 248 Phase 4 wiring: budget_ratios=Some のとき計測 log を emit.
    // 項目 248 Phase 5 (rust-reviewer H-1 fix): allocated を compact_level{1,2}_with_budget に
    // 伝播し、axis-priority prune を実 production 経路で有効化。env unset = allocated None で
    // 既存 wrapper 経路と semantic 同等、env=1 で overflow 軸優先 prune が発火。
    let allocated = dynamic_budget_for_compaction(config);
    if let Some(a) = allocated.as_ref() {
        log_event(
            LogLevel::Info,
            "compaction.budget",
            &format!(
                "buffer={} summary={} entities={} kg={} total={}",
                a.buffer, a.summary, a.entities, a.kg, a.total,
            ),
        );
    }
    let off = compact_level0(messages, config);
    let mut lv = 0u8;
    if estimate_tokens(messages) > config.max_context_tokens * 3 / 4 {
        compact_level1_with_budget(messages, config, allocated.as_ref());
        lv = 1;
    }
    if estimate_tokens(messages) > config.max_context_tokens * 9 / 10 {
        compact_level2_with_budget(messages, config, allocated.as_ref());
        lv = 2;
    }
    if estimate_tokens(messages) > config.max_context_tokens {
        compact_level3(messages, config);
        lv = 3;
    }
    (lv, off)
}

// ========== Phase 5 — 4 軸個別 prune (項目 248 Phase 5、plan §3 Phase 2 Green) ==========

/// メモリ種別タグ (prefix-based 判別、Phase 5 plan §1.2)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryKind {
    Buffer,
    Summary,
    Entities,
    Kg,
    Unclassified,
}

/// 4 軸 token 消費集計
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AxisUsage {
    pub buffer: usize,
    pub summary: usize,
    pub entities: usize,
    pub kg: usize,
    pub unclassified: usize,
}

/// 単一 Message の種別判別 (prefix + role + tool_call_id ベース、O(1))
///
/// 判定優先順位:
/// 1. 末尾 keep_recent 件の User/Assistant → Buffer
/// 2. Assistant で [summarized] / [Preserved Thinking] prefix → Summary
///    (注: [Context handoff] は System role で emit されるため Assistant 分岐では届かない、
///     System 軸での classify は将来 phase で検討)
/// 3. Tool の tool_call_id prefix → Entities (agenther_) / Kg (memory_search/kg_query/graph_search)
/// 4. Tool の content prefix `[entities:` → Entities
/// 5. それ以外 → Unclassified
pub(crate) fn classify_memory_kind(
    msg: &Message,
    idx: usize,
    total: usize,
    keep_recent: usize,
) -> MemoryKind {
    // (1) 末尾 keep_recent 件の User/Assistant は Buffer
    if idx >= total.saturating_sub(keep_recent) && matches!(msg.role, Role::User | Role::Assistant)
    {
        return MemoryKind::Buffer;
    }
    // (2) Assistant の summary prefix 判別
    // rust-reviewer L-1 fix: [Handoff Summary] は System role で emit されるため Assistant
    // 分岐では届かず dead branch だったので削除。残 2 marker のみで Summary 判定。
    if matches!(msg.role, Role::Assistant) {
        let c = &msg.content;
        if c.contains("[Preserved Thinking]") || c.contains("...[summarized]") {
            return MemoryKind::Summary;
        }
    }
    // (3) Tool の tool_call_id prefix 判別
    if matches!(msg.role, Role::Tool) {
        if let Some(id) = &msg.tool_call_id {
            if id.starts_with("agenther_") {
                return MemoryKind::Entities;
            }
            if id.starts_with("memory_search")
                || id.starts_with("kg_query")
                || id.starts_with("graph_search")
            {
                return MemoryKind::Kg;
            }
        }
        // (3b) content prefix で entities 補助判別
        if msg.content.starts_with("[entities:") {
            return MemoryKind::Entities;
        }
    }
    MemoryKind::Unclassified
}

/// messages 全体の 4 軸 token 消費を集計
///
/// estimate_tokens と整合する 1 token ≈ 4 chars 推定 (`len().div_ceil(4)`) を使用。
pub(crate) fn measure_axis_usage(messages: &[Message], keep_recent: usize) -> AxisUsage {
    let mut u = AxisUsage::default();
    let total = messages.len();
    for (idx, msg) in messages.iter().enumerate() {
        let kind = classify_memory_kind(msg, idx, total, keep_recent);
        let tok = msg.content.len().div_ceil(4);
        match kind {
            MemoryKind::Buffer => u.buffer += tok,
            MemoryKind::Summary => u.summary += tok,
            MemoryKind::Entities => u.entities += tok,
            MemoryKind::Kg => u.kg += tok,
            MemoryKind::Unclassified => u.unclassified += tok,
        }
    }
    u
}

/// allocated との差分で overflow 軸を返す (超過量降順)
///
/// usage >= allocated の軸を overflow と見なす。
/// usage == allocated の場合は overflow 量 0 として記録し、prune 優先軸として扱う。
/// これにより allocate の float→usize 切捨による 1 token 誤差を吸収する。
pub(crate) fn overflow_axes(
    usage: &AxisUsage,
    allocated: &AllocatedBudget,
) -> Vec<(MemoryKind, usize)> {
    let mut result: Vec<(MemoryKind, usize)> = Vec::new();
    if usage.buffer >= allocated.buffer {
        result.push((
            MemoryKind::Buffer,
            usage.buffer.saturating_sub(allocated.buffer),
        ));
    }
    if usage.summary >= allocated.summary {
        result.push((
            MemoryKind::Summary,
            usage.summary.saturating_sub(allocated.summary),
        ));
    }
    if usage.entities >= allocated.entities {
        result.push((
            MemoryKind::Entities,
            usage.entities.saturating_sub(allocated.entities),
        ));
    }
    if usage.kg >= allocated.kg {
        result.push((MemoryKind::Kg, usage.kg.saturating_sub(allocated.kg)));
    }
    // 超過量降順
    result.sort_by(|a, b| b.1.cmp(&a.1));
    result
}

/// compact_level1 + budget 軸統合版
///
/// `allocated=None` のとき既存 `compact_level1` と完全同一の動作 (backward compat)。
/// `allocated=Some(a)` のとき overflow 軸のメッセージを優先 prune する。
pub fn compact_level1_with_budget(
    messages: &mut [Message],
    config: &CompactionConfig,
    allocated: Option<&AllocatedBudget>,
) {
    let t = messages.len();
    if t <= config.placeholder_keep_recent {
        return;
    }
    let keep_by_count = config.placeholder_keep_recent;
    let keep_by_tokens = {
        let mut acc = 0usize;
        let mut keep = 0usize;
        for msg in messages.iter().rev() {
            acc += msg.content.len().div_ceil(4);
            if acc > config.prune_protect_tokens {
                break;
            }
            keep += 1;
        }
        keep
    };
    let boundary = t.saturating_sub(keep_by_count.max(keep_by_tokens));
    if boundary == 0 {
        return;
    }
    let pairs = find_ai_tool_pairs(messages);
    let protected: std::collections::HashSet<usize> = pairs
        .iter()
        .flat_map(|&(a, b)| {
            if a >= boundary || b >= boundary {
                vec![a, b]
            } else {
                vec![]
            }
        })
        .collect();

    let first_user_idx = messages[..boundary]
        .iter()
        .position(|m| matches!(m.role, Role::User));
    let last_user_idx = messages[..boundary]
        .iter()
        .rposition(|m| matches!(m.role, Role::User));

    // Phase 5: overflow 軸計算 (allocated=None のとき空 Vec = backward compat)
    let overflow_kinds: Vec<MemoryKind> = match allocated {
        Some(a) => {
            let usage = measure_axis_usage(&messages[..boundary], config.placeholder_keep_recent);
            overflow_axes(&usage, a)
                .into_iter()
                .map(|(k, _)| k)
                .collect()
        }
        None => Vec::new(),
    };

    // candidates: (idx, score, is_overflow)
    // overflow=true のものを先頭に、次に score 低位順
    let mut candidates: Vec<(usize, f64, bool)> = (0..boundary)
        .filter(|&i| {
            !protected.contains(&i) && Some(i) != first_user_idx && Some(i) != last_user_idx
        })
        .map(|i| {
            let kind = classify_memory_kind(&messages[i], i, t, config.placeholder_keep_recent);
            let is_overflow = overflow_kinds.contains(&kind);
            let score = score_message_importance(&messages[i]);
            (i, score, is_overflow)
        })
        .collect();

    // overflow=true が先 (true > false)、次に score 低位順
    candidates.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });

    for (i, _score, _) in &candidates {
        let msg = &mut messages[*i];
        if matches!(msg.role, Role::Tool) && msg.content.len() > config.prune_minimum_chars {
            let id = msg.tool_call_id.as_deref().unwrap_or("?");
            msg.content = format!("[prev:{id}]");
        }
    }
}

/// compact_level2 + budget 軸統合版
///
/// `allocated=None` のとき既存 `compact_level2` と完全同一の動作 (backward compat)。
/// `allocated=Some(a)` で summary 軸 overflow のとき切詰量を 0.7x 増強。
pub fn compact_level2_with_budget(
    messages: &mut [Message],
    config: &CompactionConfig,
    allocated: Option<&AllocatedBudget>,
) {
    let t = messages.len();
    if t <= config.placeholder_keep_recent {
        return;
    }
    let boundary = t - config.placeholder_keep_recent;

    // summary 軸 overflow チェック (allocated=None のとき overflow なし)
    // rust-reviewer M-2 fix: overflow_axes と同一 `>=` 判定で contract 統一
    // (float→usize 切捨 1 token 誤差吸収)
    let summary_overflow = match allocated {
        Some(a) => {
            let usage = measure_axis_usage(messages, config.placeholder_keep_recent);
            usage.summary >= a.summary
        }
        None => false,
    };
    let summary_max = if summary_overflow {
        // 30% さらに圧縮、rust-reviewer M-3 fix: usize 切捨で 0 になる corner case を防ぐため `.max(1)`
        ((config.summary_max_chars as f64 * 0.7) as usize).max(1)
    } else {
        config.summary_max_chars
    };

    // Preserved Thinking: 削除対象から思考サマリーを抽出してから要約
    let thinking_summaries = extract_thinking_summary(&messages[..boundary]);
    for msg in messages[..boundary].iter_mut() {
        if matches!(msg.role, Role::Assistant) && msg.content.len() > summary_max {
            let s: String = msg.content.chars().take(summary_max).collect();
            msg.content = format!("{s}...[summarized]");
        }
    }
    // 思考サマリーを最後の要約済みAssistantメッセージに追加
    if !thinking_summaries.is_empty() {
        let thinking_text = thinking_summaries
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(last_assistant) = messages[..boundary]
            .iter_mut()
            .rev()
            .find(|m| matches!(m.role, Role::Assistant))
        {
            last_assistant
                .content
                .push_str(&format!("\n[Preserved Thinking]\n{thinking_text}"));
        }
    }
}

/// 直近 messages から MemoryRelevance を粗推定
///
/// 各軸の token 比率から relevance を粗推定。buffer は常に 1.0 (最優先保護)。
pub fn memory_relevance_from_messages(messages: &[Message], keep_recent: usize) -> MemoryRelevance {
    let usage = measure_axis_usage(messages, keep_recent);
    let total = (usage.buffer + usage.summary + usage.entities + usage.kg).max(1) as f32;
    MemoryRelevance {
        buffer: 1.0,
        summary: (usage.summary as f32 / total).clamp(0.0, 1.0),
        entities: (usage.entities as f32 / total).clamp(0.0, 1.0),
        kg: (usage.kg as f32 / total).clamp(0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mk(n: usize, sz: usize) -> Vec<Message> {
        let mut v = vec![Message::system("s")];
        for i in 0..n {
            v.push(Message::user(format!("q{i}")));
            v.push(Message::assistant("x".repeat(sz)));
            v.push(Message::tool("y".repeat(sz), format!("t{i}")));
        }
        v
    }

    /// ツール呼び出しを含むAssistantメッセージを持つテスト用メッセージ列を生成
    fn mk_with_tool_calls(calls: &[(&str, &str)]) -> Vec<Message> {
        let mut v = vec![Message::system("s")];
        for (tool_name, result) in calls {
            v.push(Message::user("q"));
            v.push(Message::assistant(format!(
                "<think>plan</think>\n<tool_call>{{\"name\":\"{tool_name}\",\"arguments\":{{}}}}</tool_call>"
            )));
            v.push(Message::tool(
                result.to_string(),
                format!("call_{tool_name}"),
            ));
        }
        v
    }

    #[test]
    fn t_tok() {
        // Phase 2b Green: 新 estimator は max(chars/3, bytes*0.4)
        // "hello world" = 11 chars / 11 bytes → max(11/3=4, 11*4/10=5) = 5
        assert_eq!(estimate_tokens(&[Message::user("hello world")]), 5);
    }

    /// Phase 2a Red: 旧 estimator (`len()/4`) は "hello world" (11 bytes) → 3 を返すので
    /// `5` 期待で fail する。Green で hybrid estimator `max(chars/3, bytes*0.4)` に置換すると pass。
    #[test]
    fn t_estimate_tokens_is_japanese_aware() {
        assert_eq!(estimate_tokens(&[Message::user("hello world")]), 5);
        assert_eq!(estimate_tokens(&[Message::user("こんにちは世界")]), 9);
    }

    /// Phase 2a Red: stub `from_n_ctx_budget` は常に default を返すため `Some(8192)` でも 14000、
    /// 期待値 5734 (8192 * 70 / 100) と乖離して fail。Green で派生実装に置換すると pass。
    #[test]
    fn t_compaction_config_derives_from_n_ctx_budget() {
        let derived = CompactionConfig::from_n_ctx_budget(Some(8192));
        assert_eq!(derived.max_context_tokens, 5734);

        let none = CompactionConfig::from_n_ctx_budget(None);
        assert_eq!(none.max_context_tokens, 14000);

        let zero = CompactionConfig::from_n_ctx_budget(Some(0));
        assert_eq!(zero.max_context_tokens, 14000);
    }

    #[test]
    fn t_l0() {
        let mut m = vec![Message::tool("x".repeat(10000), "b")];
        let o = compact_level0(
            &mut m,
            &CompactionConfig {
                large_output_threshold: 5000,
                ..Default::default()
            },
        );
        assert_eq!(o.len(), 1);
        for p in &o {
            std::fs::remove_file(p).ok();
        }
    }
    #[test]
    fn t_l1() {
        let mut m = mk(10, 100);
        compact_level1(
            &mut m,
            &CompactionConfig {
                placeholder_keep_recent: 4,
                prune_protect_tokens: 0,
                ..Default::default()
            },
        );
        assert!(m.iter().any(|x| x.content.contains("[prev:")));
    }
    #[test]
    fn t_l2() {
        let mut m = mk(10, 500);
        compact_level2(
            &mut m,
            &CompactionConfig {
                placeholder_keep_recent: 4,
                summary_max_chars: 100,
                ..Default::default()
            },
        );
        assert!(m.iter().any(|x| x.content.contains("[summarized]")));
    }
    #[test]
    fn t_l3() {
        let mut m = mk(20, 100);
        compact_level3(
            &mut m,
            &CompactionConfig {
                emergency_keep: 4,
                ..Default::default()
            },
        );
        assert!(m.len() <= 6, "system+handoff+keep4=最大6");
    }
    #[test]
    fn t_noop() {
        let mut m = vec![Message::user("hi")];
        let (lv, _) = compact_if_needed(&mut m, &CompactionConfig::default());
        assert_eq!(lv, 0);
    }

    #[test]
    fn t_find_pairs() {
        let m = vec![
            Message::system("s"),
            Message::user("q"),
            Message::assistant("a"),
            Message::tool("r", "t1"),
        ];
        assert_eq!(find_ai_tool_pairs(&m), vec![(2, 3)]);
    }
    #[test]
    fn t_pair_multiple() {
        let m = vec![
            Message::assistant("a1"),
            Message::tool("r1", "t1"),
            Message::assistant("a2"),
            Message::tool("r2", "t2"),
        ];
        assert_eq!(find_ai_tool_pairs(&m).len(), 2);
    }
    #[test]
    fn t_pair_none() {
        let m = vec![
            Message::user("q"),
            Message::assistant("a"),
            Message::user("q2"),
        ];
        assert!(find_ai_tool_pairs(&m).is_empty());
    }
    #[test]
    fn t_l1_no_orphan() {
        let mut m = vec![
            Message::system("s"),
            Message::user("q0"),
            Message::assistant("assistant output here"),
            Message::tool(
                "tool result with enough content to compress and it must be over fifty characters long for testing",
                "t0",
            ),
            Message::user("q1"),
            Message::assistant("a1"),
            Message::tool("r1 short", "t1"),
        ];
        compact_level1(
            &mut m,
            &CompactionConfig {
                placeholder_keep_recent: 3,
                prune_protect_tokens: 0,
                ..Default::default()
            },
        );
        // idx3はペア(2,3)の一部だが、境界=4なのでidx3<4→圧縮対象
        assert!(m[3].content.contains("[prev:"));
    }
    #[test]
    fn t_l1_protect_boundary_pair() {
        let mut m = vec![
            Message::system("s"),
            Message::user("q0"),
            Message::assistant("old assistant content long"),
            Message::tool("old tool content long enough", "old"),
            Message::assistant("boundary assistant"),
            Message::tool("boundary tool content long enough", "bnd"),
        ];
        // keep_recent=2 → boundary=4, pair(4,5) both >= 4 → protected
        compact_level1(
            &mut m,
            &CompactionConfig {
                placeholder_keep_recent: 2,
                ..Default::default()
            },
        );
        assert!(!m[5].content.contains("[prev:"));
    }

    #[test]
    fn t_flush_saves_to_store() {
        let store = crate::memory::store::MemoryStore::in_memory().unwrap();
        let mut msgs = vec![Message::system("s")];
        for i in 0..10 {
            msgs.push(Message::user(format!("q{i}")));
            msgs.push(Message::assistant("important context data ".repeat(8)));
        }
        flush_before_compaction(&msgs, Some(&store));
        let results = store.search_memories("important", 10).unwrap();
        assert!(
            !results.is_empty(),
            "フラッシュされたメモリが検索可能であること"
        );
    }
    #[test]
    fn t_flush_no_store() {
        let msgs = vec![Message::assistant("x".repeat(200))];
        flush_before_compaction(&msgs, None);
        // パニックしないことを確認
    }

    #[test]
    fn t_handoff_summary() {
        let mut msgs = mk(5, 200);
        let config = CompactionConfig {
            max_context_tokens: 100,
            emergency_keep: 4,
            ..Default::default()
        };
        compact_level3(&mut msgs, &config);
        // systemメッセージ + handoff + 直近4件
        let has_handoff = msgs.iter().any(|m| m.content.contains("handoff"));
        assert!(has_handoff, "Handoff summary が挿入されるべき");
    }

    #[test]
    fn t_handoff_short_session_skipped() {
        let mut msgs = vec![
            Message::system("s"),
            Message::user("q"),
            Message::assistant("a"),
        ];
        let config = CompactionConfig {
            emergency_keep: 4,
            ..Default::default()
        };
        compact_level3(&mut msgs, &config);
        // 短すぎてhandoff不要
        let has_handoff = msgs.iter().any(|m| m.content.contains("handoff"));
        assert!(!has_handoff);
    }

    // --- 新規テスト: summarize_tool_usage ---

    #[test]
    fn t_summarize_tool_usage_basic() {
        let msgs = mk_with_tool_calls(&[
            ("shell", "ok"),
            ("file_read", "content"),
            ("shell", "done"),
            ("file_write", "written"),
        ]);
        let stats = summarize_tool_usage(&msgs);
        assert_eq!(stats.get("shell"), Some(&2), "shellは2回使用");
        assert_eq!(stats.get("file_read"), Some(&1), "file_readは1回使用");
        assert_eq!(stats.get("file_write"), Some(&1), "file_writeは1回使用");
    }

    #[test]
    fn t_summarize_tool_usage_empty() {
        let msgs = vec![
            Message::system("s"),
            Message::user("q"),
            Message::assistant("no tools"),
        ];
        let stats = summarize_tool_usage(&msgs);
        assert!(stats.is_empty(), "ツール呼び出しがなければ空");
    }

    #[test]
    fn t_summarize_tool_usage_multiple_in_one_message() {
        let msgs = vec![Message::assistant(
            "<tool_call>{\"name\":\"shell\",\"arguments\":{}}</tool_call>\n\
                 <tool_call>{\"name\":\"git\",\"arguments\":{}}</tool_call>"
                .to_string(),
        )];
        let stats = summarize_tool_usage(&msgs);
        assert_eq!(stats.get("shell"), Some(&1));
        assert_eq!(stats.get("git"), Some(&1));
    }

    // --- 新規テスト: extract_last_outcome ---

    #[test]
    fn t_extract_last_outcome_assistant() {
        let msgs = vec![
            Message::system("s"),
            Message::user("q"),
            Message::assistant("ファイルの修正が完了しました。テストも全件パスしています。"),
        ];
        let outcome = extract_last_outcome(&msgs);
        assert!(outcome.is_some());
        assert!(outcome.unwrap().contains("修正が完了"));
    }

    #[test]
    fn t_extract_last_outcome_strips_think() {
        let msgs = vec![Message::assistant(
            "<think>内部思考</think>タスク完了: 3ファイル修正済み",
        )];
        let outcome = extract_last_outcome(&msgs).unwrap();
        assert!(!outcome.contains("内部思考"), "thinkタグの中身は除外");
        assert!(outcome.contains("タスク完了"), "thinkタグ外の内容は保持");
    }

    #[test]
    fn t_extract_last_outcome_fallback_to_tool() {
        let msgs = vec![
            Message::system("s"),
            Message::assistant(""), // 空のAssistantメッセージ
            Message::tool("ビルド成功: 0 errors, 0 warnings", "build"),
        ];
        let outcome = extract_last_outcome(&msgs).unwrap();
        assert!(
            outcome.contains("ビルド成功"),
            "Toolメッセージにフォールバック"
        );
    }

    #[test]
    fn t_extract_last_outcome_truncates() {
        let long_msg = "a".repeat(300);
        let msgs = vec![Message::assistant(long_msg)];
        let outcome = extract_last_outcome(&msgs).unwrap();
        assert_eq!(outcome.chars().count(), 200, "200文字に切り詰め");
    }

    #[test]
    fn t_extract_last_outcome_empty() {
        let msgs = vec![Message::system("s"), Message::user("q")];
        assert!(extract_last_outcome(&msgs).is_none());
    }

    // --- 新規テスト: level3にツール統計と成果が含まれる ---

    #[test]
    fn t_l3_handoff_includes_tool_stats() {
        let mut msgs = mk_with_tool_calls(&[
            ("shell", "ok"),
            ("shell", "ok"),
            ("file_read", "content"),
            ("shell", "ok"),
            ("file_write", "done"),
            ("git", "committed"),
            ("shell", "final"),
        ]);
        let config = CompactionConfig {
            emergency_keep: 3,
            ..Default::default()
        };
        compact_level3(&mut msgs, &config);
        let handoff = msgs.iter().find(|m| m.content.contains("handoff")).unwrap();
        assert!(
            handoff.content.contains("Tool stats:"),
            "ツール統計が含まれるべき"
        );
        assert!(
            handoff.content.contains("shell:"),
            "shellの統計が含まれるべき"
        );
    }

    #[test]
    fn t_l3_handoff_includes_outcome() {
        let mut msgs = vec![Message::system("s")];
        for i in 0..8 {
            msgs.push(Message::user(format!("q{i}")));
            msgs.push(Message::assistant(format!(
                "作業ステップ{i}を完了しました。次に進みます。"
            )));
            msgs.push(Message::tool(format!("result{i}"), format!("t{i}")));
        }
        let config = CompactionConfig {
            emergency_keep: 3,
            ..Default::default()
        };
        compact_level3(&mut msgs, &config);
        let handoff = msgs.iter().find(|m| m.content.contains("handoff")).unwrap();
        assert!(
            handoff.content.contains("Last outcome:"),
            "最終成果が含まれるべき"
        );
    }

    #[test]
    fn t_l3_handoff_includes_unresolved() {
        let mut msgs = vec![Message::system("s")];
        for i in 0..8 {
            msgs.push(Message::user(format!("q{i}")));
            msgs.push(Message::assistant(format!(
                "ステップ{i}を実行します。長い文章にするため追加テキスト。"
            )));
            if i == 3 {
                msgs.push(Message::tool(
                    "ツール実行エラー: ファイルが見つかりません".to_string(),
                    format!("t{i}"),
                ));
            } else {
                msgs.push(Message::tool(format!("ok{i}"), format!("t{i}")));
            }
        }
        let config = CompactionConfig {
            emergency_keep: 3,
            ..Default::default()
        };
        compact_level3(&mut msgs, &config);
        let handoff = msgs.iter().find(|m| m.content.contains("handoff")).unwrap();
        assert!(
            handoff.content.contains("Unresolved"),
            "未解決事項が含まれるべき"
        );
        assert!(
            handoff.content.contains("エラー"),
            "エラー内容が含まれるべき"
        );
    }

    // --- Preserved Thinking テスト ---

    #[test]
    fn t_extract_thinking_summary() {
        let msgs = vec![
            Message::assistant(
                "<think>まずファイル構造を確認する。\n次にテストを書く。\n結論: TDDアプローチで進める。</think>実装開始"
                    .to_string(),
            ),
            Message::assistant(
                "<think>エラーの原因を分析。\n借用チェッカーが問題。\n解決策: Cloneを導入する。</think>修正完了"
                    .to_string(),
            ),
        ];
        let summaries = extract_thinking_summary(&msgs);
        assert_eq!(summaries.len(), 2, "2つのthinkブロックからサマリー抽出");
        assert!(summaries[0].contains("TDD"), "最後の文（結論）が抽出される");
        assert!(summaries[1].contains("Clone"), "2つ目の結論も抽出される");
    }

    #[test]
    fn t_extract_thinking_empty() {
        let msgs = vec![
            Message::assistant("ツール呼び出しのみ".to_string()),
            Message::user("質問"),
        ];
        let summaries = extract_thinking_summary(&msgs);
        assert!(summaries.is_empty(), "thinkブロックがなければ空");
    }

    #[test]
    fn t_score_importance_user() {
        let msg = Message::user("タスクの定義");
        assert_eq!(
            score_message_importance(&msg),
            1.0,
            "Userメッセージは最高スコア"
        );
    }

    #[test]
    fn t_score_importance_error() {
        // 項目 178: 実エラー format (tool_exec.rs:78 prefix) のみ低スコア扱い
        let msg = Message::tool("ツール実行エラー: file not found", "t1");
        assert_eq!(score_message_importance(&msg), 0.2, "エラーToolは低スコア");
    }

    #[test]
    fn t_level2_preserves_thinking() {
        let mut msgs = vec![Message::system("s")];
        for i in 0..6 {
            msgs.push(Message::user(format!("q{i}")));
            msgs.push(Message::assistant(format!(
                "<think>ステップ{i}の分析。{}結論: 方針{i}で進める。</think>{}",
                "x".repeat(300),
                "y".repeat(100),
            )));
            msgs.push(Message::tool(format!("result{i}"), format!("t{i}")));
        }
        let config = CompactionConfig {
            placeholder_keep_recent: 3,
            summary_max_chars: 50,
            ..Default::default()
        };
        compact_level2(&mut msgs, &config);
        let has_preserved = msgs
            .iter()
            .any(|m| m.content.contains("[Preserved Thinking]"));
        assert!(has_preserved, "level2後に思考サマリーが残るべき");
    }

    // --- 重要度スコア追加テスト ---

    #[test]
    fn t_score_importance_tool_call() {
        let msg = Message::assistant(r#"<tool_call>{"name":"shell"}</tool_call>"#);
        assert_eq!(
            score_message_importance(&msg),
            0.7,
            "tool_call含むAssistantは0.7"
        );
    }

    #[test]
    fn t_score_importance_system_context() {
        let msg = Message::system("<context>injected</context>");
        assert_eq!(
            score_message_importance(&msg),
            0.3,
            "注入コンテキストSystemは0.3"
        );
    }

    #[test]
    fn t_score_importance_system_normal() {
        let msg = Message::system("通常のシステムプロンプト");
        assert_eq!(score_message_importance(&msg), 0.9, "通常Systemは0.9");
    }

    // --- SemanticScorer テスト (P1 Step 4) ---

    #[test]
    fn t_semantic_scorer_new() {
        use crate::runtime::embedder::SimpleEmbedder;
        let embedder = Box::new(SimpleEmbedder::default());
        let scorer = SemanticScorer::new(embedder, "rust programming task").unwrap();
        assert_eq!(scorer.task_embedding.len(), 256, "埋め込み次元は256");
    }

    #[test]
    fn t_semantic_scorer_blends_role_and_similarity() {
        use crate::runtime::embedder::SimpleEmbedder;
        let embedder = Box::new(SimpleEmbedder::default());
        let scorer = SemanticScorer::new(embedder, "rust async programming").unwrap();

        // User基本スコア=1.0、タスクと同じ内容 → 高スコア期待
        let relevant_user = Message::user("rust async programming");
        let score = scorer.score(&relevant_user);
        // 0.6 * 1.0 + 0.4 * ((1.0 + 1.0) / 2.0) = 0.6 + 0.4 = 1.0（完全一致）
        assert!(score > 0.7, "タスク関連Userは高スコア ({score})");
    }

    #[test]
    fn t_semantic_scorer_empty_content() {
        use crate::runtime::embedder::SimpleEmbedder;
        let embedder = Box::new(SimpleEmbedder::default());
        let scorer = SemanticScorer::new(embedder, "task").unwrap();
        let empty_msg = Message::assistant("");
        // 空コンテンツは固定スコアにフォールバック
        let score = scorer.score(&empty_msg);
        assert!((score - 0.4).abs() < 1e-6, "空はAssistant固定0.4 ({score})");
    }

    #[test]
    fn t_semantic_scorer_error_tool_low() {
        use crate::runtime::embedder::SimpleEmbedder;
        let embedder = Box::new(SimpleEmbedder::default());
        let scorer = SemanticScorer::new(embedder, "write unit tests").unwrap();
        // 項目 178: 実エラー format (tool_exec.rs:78 prefix) を使用
        let error_tool = Message::tool("ツール実行エラー: compile failed", "t1");
        let ok_tool = Message::tool("write unit tests completed successfully", "t2");
        // エラーToolはベース0.2、成功Toolはベース0.5 → 成功の方が高いはず
        assert!(
            scorer.score(&ok_tool) > scorer.score(&error_tool),
            "成功Toolはエラーより高スコア"
        );
    }

    #[test]
    fn t_compact_level1_with_scorer_prunes() {
        use crate::runtime::embedder::SimpleEmbedder;
        let embedder = Box::new(SimpleEmbedder::default());
        let scorer = SemanticScorer::new(embedder, "task").unwrap();
        let mut m = mk(10, 100);
        compact_level1_with_scorer(
            &mut m,
            &CompactionConfig {
                placeholder_keep_recent: 4,
                prune_protect_tokens: 0,
                ..Default::default()
            },
            &scorer,
        );
        assert!(
            m.iter().any(|x| x.content.contains("[prev:")),
            "scorer版でも圧縮される"
        );
    }

    #[test]
    fn t_compact_level1_with_scorer_no_op_short() {
        use crate::runtime::embedder::SimpleEmbedder;
        let embedder = Box::new(SimpleEmbedder::default());
        let scorer = SemanticScorer::new(embedder, "task").unwrap();
        let mut m = vec![Message::user("q"), Message::assistant("a")];
        let orig_len = m.len();
        compact_level1_with_scorer(
            &mut m,
            &CompactionConfig {
                placeholder_keep_recent: 10,
                ..Default::default()
            },
            &scorer,
        );
        assert_eq!(m.len(), orig_len, "短いセッションは圧縮されない");
    }

    // --- 項目 178: compaction.rs 偽陽性除去テスト群 ---
    // file_read で読んだソース内コメントの「エラー」を実エラー扱いせず、
    // tool_exec.rs:78 形式 prefix のみを真のエラーとして扱う (support.rs:31-42 と整合)。

    #[test]
    fn t_score_importance_no_false_positive_on_source_code_error_word() {
        let msg = Message::tool(
            "fn handle() {\n    // エラーハンドリング: 失敗時のフォールバック\n    Ok(())\n}",
            "t1",
        );
        assert_eq!(
            score_message_importance(&msg),
            0.5,
            "ソース内コメントの「エラー」は失敗扱いしない"
        );
    }

    #[test]
    fn t_score_importance_real_tool_error_prefix() {
        let msg = Message::tool("ツール実行エラー: file not found", "t1");
        assert_eq!(
            score_message_importance(&msg),
            0.2,
            "実エラー format (ツール実行エラー: prefix) は低スコア"
        );
    }

    #[test]
    fn t_score_importance_real_error_capital_prefix() {
        let msg = Message::tool("Error: connection refused", "t1");
        assert_eq!(
            score_message_importance(&msg),
            0.2,
            "Error: 大文字 prefix も実エラー扱い"
        );
    }

    #[test]
    fn t_collect_unresolved_no_false_positive_on_source_code_error_word() {
        let messages = vec![Message::tool(
            "fn handle() { // エラーハンドリング\n    Ok(()) }",
            "t1",
        )];
        let unresolved = collect_unresolved(&messages, 1);
        assert!(
            unresolved.is_empty(),
            "ソース内コメントの「エラー」は未解決事項として収集しない"
        );
    }

    #[test]
    fn t_collect_unresolved_collects_real_tool_error() {
        let messages = vec![Message::tool("ツール実行エラー: file not found", "t1")];
        let unresolved = collect_unresolved(&messages, 1);
        assert_eq!(unresolved.len(), 1, "実エラーは未解決事項として収集する");
        assert!(unresolved[0].contains("ツール実行エラー"));
    }

    // ─── Dynamic Budget Ratios (項目 248) tests ───────────────────────────
    //
    // Phase 1 Red 期待: skeleton で 4 件 FAIL + 1 件 PASS (env unset sanity gate)。
    // Phase 2 Green で 5 件 PASS に。

    #[test]
    fn t_budget_ratios_default_sums_to_one() {
        let r = BudgetRatios::default();
        assert!(
            r.is_normalized(),
            "default ratio の合計 == 1.0 ±ε (Phase 1 Red 期待 FAIL、Phase 2 Green PASS)"
        );
    }

    #[test]
    fn t_allocate_distributes_total() {
        let r = BudgetRatios::default();
        let alloc = r.allocate(10000);
        // Phase 2 Green: buffer=4000, summary=3000, entities=2000, kg=1000
        assert_eq!(
            alloc.buffer + alloc.summary + alloc.entities + alloc.kg,
            10000,
            "Phase 2 Green: total 全消費 (allocate sum == 10000)"
        );
        assert_eq!(alloc.total, 10000);
    }

    #[test]
    fn t_allocate_handles_remainder() {
        let r = BudgetRatios::default();
        let alloc = r.allocate(10003);
        // 余り 3 は buffer に寄る
        let sum = alloc.buffer + alloc.summary + alloc.entities + alloc.kg;
        assert_eq!(sum, 10003, "Phase 2 Green: 余り含めて total 一致");
    }

    #[test]
    fn t_adjusted_increases_high_relevance() {
        let r = BudgetRatios::default();
        let relevance = MemoryRelevance {
            buffer: 1.0,
            summary: 0.3,
            entities: 0.8, // > 0.5 で entities ratio 増加
            kg: 0.2,
        };
        let adjusted = r.adjusted(&relevance);
        // Phase 2 Green: entities が base 0.2 より大きくなる
        assert!(
            adjusted.relevant_entities > 0.2,
            "Phase 2 Green: high relevance で entities ratio 増 (Phase 1 Red FAIL 期待)"
        );
    }

    #[test]
    fn t_dynamic_budget_env_unset_returns_none() {
        let _g = DYNAMIC_BUDGET_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("BONSAI_DYNAMIC_BUDGET_RATIOS") };
        assert!(
            dynamic_budget_ratios_from_env().is_none(),
            "env unset で None 戻り (Phase 1 でも PASS、sanity gate)"
        );
    }

    // ── 項目 248 Phase 4 Red: CompactionConfig.budget_ratios + with_dynamic_budget_from_env ──
    //
    // Phase 1 Red: field 追加 + 2 stub method (no-op) で t3/t4 が FAIL (logical).
    // Phase 2 Green: env-driven Some 設定 + dynamic_budget_for_compaction allocate 返却.
    // Phase 3 Refactor: log emit を compact_if_needed に wire、middleware.rs から factory chain.

    /// Phase 1 Red sanity: default の budget_ratios は None (backward compat).
    #[test]
    fn t_compaction_config_default_budget_ratios_none() {
        let c = CompactionConfig::default();
        assert!(
            c.budget_ratios.is_none(),
            "Default は backward compat = None (env unset で従来挙動)"
        );
    }

    /// Phase 1 Red sanity: env unset + with_dynamic_budget_from_env → None.
    #[test]
    fn t_with_dynamic_budget_from_env_unset_returns_none() {
        let _g = DYNAMIC_BUDGET_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var("BONSAI_DYNAMIC_BUDGET") };
        let c = CompactionConfig::default().with_dynamic_budget_from_env();
        assert!(
            c.budget_ratios.is_none(),
            "env unset で backward compat = None 維持"
        );
    }

    /// Phase 1 Red 核心 1: env=1 + with_dynamic_budget_from_env → Some(default 40/30/20/10).
    /// stub は self 返却 → budget_ratios None のまま → FAIL 期待.
    #[test]
    fn t_with_dynamic_budget_from_env_set_returns_some() {
        let _g = DYNAMIC_BUDGET_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("BONSAI_DYNAMIC_BUDGET", "1") };
        let c = CompactionConfig::default().with_dynamic_budget_from_env();
        unsafe { std::env::remove_var("BONSAI_DYNAMIC_BUDGET") };
        assert!(
            c.budget_ratios.is_some(),
            "env=1 で BudgetRatios (40/30/20/10) を budget_ratios に設定 (Phase 2 Green PASS 期待)"
        );
    }

    /// Phase 1 Red 核心 2: budget_ratios=Some で dynamic_budget_for_compaction が Some(allocated) 返却.
    /// stub は常に None → FAIL 期待.
    #[test]
    fn t_dynamic_budget_for_compaction_returns_some_when_set() {
        let c = CompactionConfig {
            budget_ratios: Some(BudgetRatios::default()),
            ..Default::default()
        };
        let allocated = dynamic_budget_for_compaction(&c);
        assert!(
            allocated.is_some(),
            "budget_ratios=Some で allocate を返す (Phase 2 Green PASS 期待)"
        );
        let a = allocated.expect("Some 確認済");
        assert_eq!(
            a.total, c.max_context_tokens,
            "total が max_context_tokens (allocate sum 一致)"
        );
    }

    // ── 項目 248 Phase 4 critic F2 follow-up: env override 動作確証 2 件 ──
    //
    // critic 指摘: 「BONSAI_DYNAMIC_BUDGET_RATIOS env override の有効/無効分岐 test が 0 件」。
    // 本 test 2 件で valid override + invalid fallback の両端を捕捉.

    /// env override valid → ratio 反映確証.
    #[test]
    fn t_with_dynamic_budget_from_env_override_applied() {
        let _g = DYNAMIC_BUDGET_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("BONSAI_DYNAMIC_BUDGET", "1") };
        unsafe { std::env::set_var("BONSAI_DYNAMIC_BUDGET_RATIOS", "0.5,0.3,0.2,0.0") };
        let c = CompactionConfig::default().with_dynamic_budget_from_env();
        unsafe { std::env::remove_var("BONSAI_DYNAMIC_BUDGET") };
        unsafe { std::env::remove_var("BONSAI_DYNAMIC_BUDGET_RATIOS") };
        let r = c.budget_ratios.expect("env=1 で Some");
        assert!(
            (r.recent_buffer - 0.5).abs() < 1e-6,
            "override `0.5,0.3,0.2,0.0` の buffer ratio=0.5 反映"
        );
        assert!(
            (r.knowledge_graph - 0.0).abs() < 1e-6,
            "override の kg ratio=0.0 反映 (Lab paired で 4 軸調整可能化)"
        );
    }

    /// env override invalid (sum != 1.0) → default 40/30/20/10 fallback 確証.
    #[test]
    fn t_with_dynamic_budget_from_env_invalid_falls_back_to_default() {
        let _g = DYNAMIC_BUDGET_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("BONSAI_DYNAMIC_BUDGET", "1") };
        unsafe { std::env::set_var("BONSAI_DYNAMIC_BUDGET_RATIOS", "0.1,0.1,0.1,0.1") }; // sum=0.4 invalid
        let c = CompactionConfig::default().with_dynamic_budget_from_env();
        unsafe { std::env::remove_var("BONSAI_DYNAMIC_BUDGET") };
        unsafe { std::env::remove_var("BONSAI_DYNAMIC_BUDGET_RATIOS") };
        let r = c
            .budget_ratios
            .expect("env=1 で Some (override 不正でも default fallback)");
        assert!(
            (r.recent_buffer - 0.4).abs() < 1e-6,
            "invalid override → BudgetRatios::default 40/30/20/10 fallback (buffer=0.4)"
        );
        assert!(
            (r.knowledge_graph - 0.1).abs() < 1e-6,
            "invalid override → kg=0.1 default"
        );
    }

    // ========== Phase 5 — 4 軸個別 prune (項目 248 Phase 5、plan §3 Phase 1 Red) ==========

    #[test]
    fn t_classify_buffer_role() {
        // 末尾 keep_recent 件は Buffer 判定
        let msg = Message::user("recent");
        let kind = classify_memory_kind(&msg, 5, 6, 2); // idx=5, total=6, keep_recent=2 → 末尾 2 件
        assert_eq!(kind, MemoryKind::Buffer, "末尾 keep_recent 件は Buffer");
    }

    #[test]
    fn t_classify_summary_prefix() {
        let msg = Message::assistant("...[summarized] content");
        let kind = classify_memory_kind(&msg, 0, 10, 2);
        assert_eq!(
            kind,
            MemoryKind::Summary,
            "[summarized] prefix の Assistant は Summary"
        );
    }

    #[test]
    fn t_classify_entities_tool_call_id() {
        let msg = Message::tool("entity content", "agenther_xyz");
        let kind = classify_memory_kind(&msg, 0, 10, 2);
        assert_eq!(
            kind,
            MemoryKind::Entities,
            "agenther_ prefix tool_call_id は Entities"
        );
    }

    #[test]
    fn t_classify_kg_tool_call_id() {
        let msg = Message::tool("kg content", "memory_search_1");
        let kind = classify_memory_kind(&msg, 0, 10, 2);
        assert_eq!(
            kind,
            MemoryKind::Kg,
            "memory_search_ prefix tool_call_id は Kg"
        );
    }

    #[test]
    fn t_measure_axis_usage_sums_correctly() {
        let messages = vec![
            Message::user("q"),
            Message::assistant("a"),
            Message::tool("entity short", "agenther_1"),
        ];
        let usage = measure_axis_usage(&messages, 1);
        let total_axis =
            usage.buffer + usage.summary + usage.entities + usage.kg + usage.unclassified;
        // 4 軸 + unclassified の合計 == 全 message の token 合計に等しい
        let expected_total: usize = messages.iter().map(|m| m.content.len().div_ceil(4)).sum();
        assert_eq!(total_axis, expected_total, "4 軸合計 == 全 token");
    }

    #[test]
    fn t_overflow_axes_descending() {
        let usage = AxisUsage {
            buffer: 50,
            summary: 10,
            entities: 5,
            kg: 100,
            unclassified: 0,
        };
        let allocated = AllocatedBudget {
            total: 100,
            buffer: 40,
            summary: 30,
            entities: 20,
            kg: 10,
        };
        let result = overflow_axes(&usage, &allocated);
        // kg: 100-10=90 overflow / buffer: 50-40=10 overflow / summary/entities は overflow なし
        assert_eq!(result.len(), 2, "kg + buffer の 2 軸 overflow");
        assert_eq!(result[0].0, MemoryKind::Kg, "kg が overflow 量最大");
        assert_eq!(result[1].0, MemoryKind::Buffer, "buffer が次点");
    }

    #[test]
    fn t_compact_level1_with_budget_prunes_overflow_axis_first() {
        // Phase 2 Green で実装: overflow_axes が KG overflow を検出し、KG tool を優先 prune。
        // rust-reviewer H-2 fix: helper 単体検証 + compact_level1_with_budget の実 prune 挙動を
        // 同一 test で双方確証 (axis-priority prune が実 production 経路で発火する事を保証)。
        let msgs = vec![
            Message::user("q"),
            Message::assistant("a"),
            Message::tool("kg result content", "memory_search_1"),
        ];
        let usage = measure_axis_usage(&msgs, 1);
        // Phase 2 Green では kg > 0 になるが stub では kg == 0
        assert!(
            usage.kg > 0,
            "KG tool の token が kg 軸に集計される (stub: 0 で FAIL)"
        );

        let allocated = AllocatedBudget {
            total: 100,
            buffer: 40,
            summary: 30,
            entities: 20,
            kg: 5,
        };
        // usage.kg > allocated.kg なら overflow
        let overflows = overflow_axes(&usage, &allocated);
        assert!(
            overflows.iter().any(|(k, _)| *k == MemoryKind::Kg),
            "KG overflow が overflow_axes で検出される (stub: empty で FAIL)"
        );

        // rust-reviewer H-2 fix: 実 prune 挙動を assert
        // KG tool message が overflow 軸として優先 prune されることを直接確証
        let mut prune_msgs = vec![
            Message::system("s"),
            Message::user("first user query"),
            Message::assistant("plan response"),
            Message::tool(
                "kg long content that should be pruned first when KG overflow occurs",
                "memory_search_1",
            ),
            Message::tool("entity short data", "agenther_x"),
            Message::user("recent user"),
            Message::assistant("recent assistant"),
        ];
        let config = CompactionConfig {
            max_context_tokens: 100,
            placeholder_keep_recent: 2,
            prune_protect_tokens: 10,
            prune_minimum_chars: 5,
            ..CompactionConfig::default()
        };
        compact_level1_with_budget(&mut prune_msgs, &config, Some(&allocated));
        // KG tool が `[prev:memory_search_1]` または `[prev:idx]` に prune されている事
        let kg_pruned = prune_msgs.iter().any(|m| {
            matches!(m.role, Role::Tool)
                && m.tool_call_id.as_deref() == Some("memory_search_1")
                && m.content.starts_with("[prev:")
        });
        assert!(
            kg_pruned,
            "KG overflow 時に KG tool が prune される (実 axis-priority prune 確証)"
        );
    }

    #[test]
    fn t_compact_if_needed_backward_compat_when_env_unset() {
        // env unset (None) でも measure_axis_usage は全 token を unclassified で返す
        // (stub は Default = 全 0 を返す) → unclassified == 0 の assert で FAIL になる
        let messages = vec![
            Message::user("hello world"),
            Message::assistant("response text"),
        ];
        let usage = measure_axis_usage(&messages, 1);
        let expected_total: usize = messages.iter().map(|m| m.content.len().div_ceil(4)).sum();
        // stub では全 0 → total_axis == 0 で FAIL
        // Phase 2 Green では unclassified に全 token が集約されるため PASS
        let total_axis =
            usage.buffer + usage.summary + usage.entities + usage.kg + usage.unclassified;
        assert_eq!(
            total_axis, expected_total,
            "measure_axis_usage: 全 token が 4 軸 + unclassified に集約される (stub: 0 で FAIL)"
        );
    }
}
