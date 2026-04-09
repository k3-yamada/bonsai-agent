---
name: rust-local-llm-agent
description: >
  Rustでローカル/エッジLLM（特に1ビット量子化モデル）を使った自律型エージェントを設計・実装するためのガイド。
  llama-cpp-2によるインプロセスFFI統合、Reflexionエージェントループ、A-MEMメモリ、動的ツール選択、
  2段階生成パイプライン、構造化エラーリカバリをカバーする。
  「Rustでエージェント」「ローカルLLM」「llama.cppをRustに組み込む」「1-bit LLM」「エッジ推論」
  「ツール呼び出しエージェント」「自律型エージェント」「ReActパターン」といった文脈で使用すること。
  オフラインで動作するAIエージェントを構築するあらゆる場面でこのスキルを参照すべき。
---

# Rust Local LLM Agent

ローカルLLM（特に1ビット量子化モデル）をRustバイナリに直接組み込み、
外部サーバー不要の自律型エージェントを構築するための設計パターン集。

arxiv論文の知見に基づき、小型モデルでも信頼性の高いツール呼び出しと
自己修正能力を実現する。

## いつこのスキルを使うか

- Rustでローカル/エッジLLMエージェントを構築するとき
- llama.cppやGGUFモデルをRustに統合するとき
- 小型モデル（7B-8B）でツール呼び出しを実装するとき
- 1ビット/低ビット量子化モデルをエージェントに使うとき
- オフライン/プライベートなAIアシスタントを作るとき

---

## 1. 推論バックエンドの統合

### インプロセスFFI（推奨）

`llama-cpp-2`クレートを使い、llama.cppを静的ライブラリとしてRustバイナリにリンクする。
外部サーバープロセスは不要。

```toml
[dependencies]
llama-cpp-2 = { git = "https://github.com/USER/llama-cpp-rs", features = ["metal"] }
```

カスタム量子化フォーマット（Q1_0_g128等）が必要な場合は、llama-cpp-rsをフォークし、
`llama-cpp-sys-2/llama.cpp`のsubmoduleを対象フォークに差し替える。

トレイトで抽象化し、テスト時にモック可能にする:

```rust
pub trait LlmBackend: Send + Sync {
    fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        on_token: &mut dyn FnMut(&str),
    ) -> anyhow::Result<String>;
}
```

テストでは`MockLlmBackend`（`Vec<String>`でスクリプト化されたレスポンスを返す）を使い、
実モデル不要でエージェントループをテストする。実モデルが必要なテストには`#[ignore]`を付ける。

### モデル管理

`hf-hub`クレートでHugging Faceからモデルを自動ダウンロード。
キャッシュは`~/.cache/huggingface/hub/`（Python版と互換）。
`get()`はキャッシュファーストで、既にダウンロード済みならネットワーク不要。

---

## 2. 2段階生成パイプライン

**背景:** 構造化出力（JSON等）を強制するとLLMの推論能力が低下する
（arxiv:2408.02442 "Let Me Speak Freely"）。

**解決策:** 思考と構造化出力を分離する。

1. **思考フェーズ**: `<think>...</think>`タグ内は自由形式でChain-of-Thought推論
2. **出力フェーズ**: `<tool_call>...</tool_call>`タグ内のみGBNF文法でJSON構造を強制

```rust
pub struct ParsedOutput {
    pub thinking: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub text: Option<String>,
}

pub fn parse_assistant_output(raw: &str) -> anyhow::Result<ParsedOutput>;
```

llama.cppのGBNF文法サポートを活用し、ツール呼び出しのJSONスキーマを強制できる。
これにより小型モデルでも構造化出力の信頼性を維持しつつ、推論品質を損なわない。

パーサーのテストは純関数なのでTDDの最適な起点になる。
最低8ケース: プレーンテキスト / `<think>` / 単一・複数`<tool_call>` / 混在 / 不正JSON / ネスト / 空タグ

---

## 3. ツールシステム

### 動的ツール選択

小型モデルでは全ツールをプロンプトに入れると精度が低下する
（arxiv:2409.00608 "TinyAgent", arxiv:2411.15399 "Less is More"）。

ユーザー入力に関連するツールのみを動的に選択してコンテキストに注入する:

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    /// クエリに関連するツールを上位max件まで選択
    pub fn select_relevant(&self, query: &str, max: usize) -> Vec<&dyn Tool>;
}
```

デフォルト上限は5件。キーワードマッチングでスコアリングする。
ツール記述の品質が重要 — 曖昧な記述は実行ステップ増加とトークン浪費を招く
（arxiv:2602.14878 "MCP Tool Descriptions Are Smelly"）。

### 権限モデル

自律エージェントには安全なツール実行のための権限制御が不可欠
（arxiv:2504.11703 "Progent"）:

```rust
pub enum Permission {
    Auto,     // 確認なしで実行（FileRead等）
    Confirm,  // ユーザー確認後に実行（Shell, FileWrite等）
    Deny,     // 実行禁止
}
```

### Toolトレイト

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn permission(&self) -> Permission;
    async fn call(&self, args: serde_json::Value) -> anyhow::Result<String>;
}
```

---

## 4. エージェントループ（Reflexion）

単純なReActループ（Think→Act→Observe）を超え、自己反省による学習を組み込む
（arxiv:2303.11366 "Reflexion"）。

### フロー

```
入力 → メモリ検索 → ツール選択(5件) → プロンプト構築 → LLM(インプロセス)
  ↑                                                         ↓
  │                                                   2段階パース
  │                                                         ↓
  │                                         ┌─ ツール呼び出しあり
  │                                         │   権限チェック → 実行
  │                                         │   ├ 成功 → 要旨圧縮 → LLMへ戻る
  │                                         │   └ 失敗 → Reflexion → リトライ(3回)
  │                                         │
  └── セッション永続化 ← メモリ保存 ←──── └─ テキスト回答 → 出力
```

### 構造化エラーリカバリ

失敗モードを分類し、モード別にリカバリ戦略を適用する
（arxiv:2503.13657 "Why Multi-Agent Fail", arxiv:2509.25370 "Where Agents Fail"）:

```rust
pub enum FailureMode {
    ParseError,      // → プロンプト修正してリトライ
    ToolExecError,   // → エラー情報をコンテキストに追加してリトライ
    ReasoningError,  // → 別アプローチを促すプロンプト
    LoopDetected,    // → 即座に打ち切り（同じ操作の繰り返し）
}
```

Reflexionのポイント: 単純リトライではなく、失敗情報をコンテキストに追加して
LLMに「何が間違っていたか」を反省させてから再試行する。

---

## 5. メモリ設計（A-MEM式）

フラットなkey-valueストアではなく、Zettelkasten方式の原子的ノート＋動的リンクを使う。
MemGPT比で85-93%のトークン使用量を削減できる（arxiv:2502.12110 "A-MEM"）。

### SQLiteスキーマ

```sql
CREATE TABLE memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content TEXT NOT NULL,
    tags TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    accessed_at TEXT NOT NULL
);
CREATE TABLE memory_links (
    source_id INTEGER REFERENCES memories(id),
    target_id INTEGER REFERENCES memories(id),
    relation TEXT NOT NULL,  -- "related_to", "derived_from", "contradicts"
    PRIMARY KEY (source_id, target_id)
);
CREATE VIRTUAL TABLE memories_fts USING fts5(content, tags);
```

エージェント自身がメモリを操作するツール（`MemorySearchTool`, `MemorySaveTool`）を提供する。

### 階層的コンテキスト管理

ツール実行結果をそのまま保持するとコンテキストが爆発する。
ReadAgent方式（arxiv:2402.09727）で要旨圧縮を行う:

- 直近メッセージ: フル内容
- 古いメッセージ: 要旨（gist）に圧縮
- コンテキスト上限到達で自動要約
- `messages`テーブルに`gist`列を持ち、オリジナルは必要時に再取得

---

## 6. 推奨プロジェクト構造

```
src/
├── main.rs                    # CLI（対話/デーモン/単発）
├── lib.rs
├── runtime/
│   ├── model.rs               # hf-hub モデル自動DL
│   └── inference.rs           # LlmBackend トレイト + 実装
├── agent/
│   ├── loop.rs                # Reflexion エージェントループ
│   ├── parse.rs               # 2段階パーサー
│   ├── conversation.rs        # Message, Session
│   ├── context.rs             # コンテキスト圧縮
│   └── error_recovery.rs      # エラー分類 + リカバリ
├── tools/
│   ├── mod.rs                 # Tool トレイト + Registry + 動的選択
│   ├── permission.rs          # 権限制御
│   └── ...                    # 各ツール実装
├── memory/
│   ├── store.rs               # SQLite A-MEM ストア
│   └── search.rs              # FTS5 + タグ探索
└── scheduler/
    └── cron.rs                # 定期タスク実行（デーモンモード用）
```

---

## 7. 実装順序（TDD）

テスト容易性と依存関係から、この順序が最適:

1. **型 + パーサー** — 純関数、外部依存なし。TDDの最適な起点
2. **ツールシステム** — トレイト定義とレジストリ。まだLLM不要
3. **推論ランタイム** — LlmBackendトレイト + 実装。モック設計もここで
4. **メモリ** — SQLite。インメモリDBでテスト可能
5. **エージェントループ** — 全てを統合。MockLlmBackendでテスト
6. **スケジューラ** — デーモンモード用
7. **ストリーミング + UX** — 最後に磨く

---

## 参考論文

詳細は `references/arxiv-papers.md` を参照。

| 設計要素 | 論文 | ID |
|---------|------|-----|
| 2段階生成 | Let Me Speak Freely / DCCD | 2408.02442 / 2603.03305 |
| 動的ツール選択 | TinyAgent / Less is More | 2409.00608 / 2411.15399 |
| Reflexion | Reflexion / Agent-R | 2303.11366 / 2501.11425 |
| A-MEM | A-MEM | 2502.12110 |
| コンテキスト圧縮 | ReadAgent / ACON | 2402.09727 / 2510.00615 |
| ツール権限 | Progent | 2504.11703 |
| エラーリカバリ | Why Fail / Where Fail | 2503.13657 / 2509.25370 |
| 小型モデルエージェント | SLM Survey | 2510.03847 |
| 1-bitエージェント | ACBench / BitVLA | 2505.19433 / 2506.07530 |
