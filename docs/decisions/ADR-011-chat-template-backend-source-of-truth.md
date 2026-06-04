# ADR-011: Chat Template の Source of Truth = Backend Tokenizer

## Status: Accepted (2026-06-04)

## Context

bonsai-agent の prompt 構築は Rust コード内 (`agent_loop`, `prompt::*`) でハードコードされており、
model ごとに分岐が必要な場合は code 改修が必要な設計になっていた。
現在は 1bit Qwen3-8B の単一モデル運用で問題は未顕在化しているが、
将来の model 切替時 (Gemma E4B / Ternary Bonsai 等) に
**chat template の二重定義リスク** が生じる。

LocalAI (mudler/LocalAI) 設計調査 (2026-06-04、`memory/localai_learnings_2026_06_04.md`) で
`use_tokenizer_template: true` 設計を確認。この設計では chat template の source of truth を
**backend (HuggingFace tokenizer / model card に同梱の jinja) に置き**、
core は role/content のみを保持する。

現在の bonsai-agent は MLX server に `/v1/chat/completions` を渡しており、
MLX-LM 側の jinja chat template が tokenize 時に自動適用されている。
つまり bonsai は **既に "tokenizer template 委譲" 相当の状態** にある。
本 ADR はこの設計選択を明示的に宣言し、将来の model 切替指針とする。

## Decision

bonsai-agent は **prompt formatting (chat template) の source of truth を
backend (MLX server の tokenizer) に委ね**、bonsai 本体は
role (`user` / `assistant` / `system`) と `content` のみを持つ。

具体的な設計選択:

1. bonsai は `messages: Vec<domain::conversation::Message>` を構築し MLX に渡す。
2. MLX-LM は model card に同梱された jinja chat template で tokenize する。
3. bonsai 側に model 固有の template 文字列を **持たない** (現状踏襲・明示宣言化)。
4. 将来 model 切替時は backend が template を自動処理する前提で設計する。
5. tool parser 選択 (hermes / gemma4 / qwen3_xml 等) も backend 委譲方針とする。

## Consequences

### Positive
- 将来 Gemma E4B / Ternary Bonsai 等への切替時に bonsai 側の template 改修不要。
- LocalAI `use_tokenizer_template:true` と同じ安全性保証を暗黙的に得られる。
- production code touch ゼロ (既存設計の明示宣言のみ)。
- Clean Architecture domain 層の `Message` 型 (role/content) の責務が明確化される。

### Negative / Constraints
- template の詳細制御が必要な場合 (custom delimiter 等) は backend 側設定で対応。
- bonsai 側でのprompt level tuning は system message / role content 内で行う。
- bonsai が MLX 以外の backend (e.g. llama.cpp の raw completion) に切替える場合、
  template 委譲前提が崩れるため再検討が必要。

### Neutral
- native tool call 非対応 model (現 1bit Qwen3-8B) では tool parser の選択影響なし。
  将来 native tool call 対応 model への切替時は LocalAI の per-model parser 設計を参照。
- MLX-LM の jinja template 実装バグはベンダー側 fix に依存する。

## F-1 補足 (2026-06-04 調査)

MLX-LM v0.31.3 時点で `response_format` / GBNF grammar は **未サポート** (Issue #1007 open)。
生成中の構造強制は現時点では不可。bonsai の既存 post-hoc JSON extraction (tool-call parser) が
有効な代替手段。Official `response_format` 実装 (Issue #1007) をウォッチする。

## Related

- LocalAI 調査: `memory/localai_learnings_2026_06_04.md` (T-1, F-1 項目)
- ADR-006: バックエンドフォールバックチェーン
- ADR-010: クリーンアーキテクチャ domain 層新設
- `src/domain/conversation.rs`: Message / Role / Session 型定義
