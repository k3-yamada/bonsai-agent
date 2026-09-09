# ADR-013: ModelProfile レジストリ導入と既定モデルの MiniCPM5-2B 化

## Status: Accepted (2026-09-08)

## Context

モデル identity（model_id、GGUF/MLX の Hugging Face repo、context 長、推論パラメータ既定値）は
`src/config.rs`、`src/main.rs`、`src/cli_args.rs`、複数の `scripts/*.sh`、README/CLAUDE.md 等の
docs に個別のハードコードとして散在していた。差し替え時はこれらすべてを手作業で追わないと
値が食い違う（例: context 長が config.rs では 16384、start-server.sh では別値）リスクを抱えていた。

同時に、既定モデルを 1-bit 量子化の Bonsai-8B（1.28GB、Qwen3-8B ベース、PrismML fork 依存）から
2B dense モデルへ切り替える判断があった。候補の MiniCPM5-2B は標準の `LlamaForCausalLM`
アーキテクチャ（42 layers、GQA 16/2）で、素の llama.cpp / mlx-lm でそのまま動く
（PrismML fork 不要）。GGUF Q4_K_M で 1.56GB、native context 131072、Apache-2.0。

## Decision

モデル identity の single source of truth を Rust 側 (`src/domain/model_profile.rs`) と
shell 側 (`scripts/model.env`) の2箇所に集約する。

- `src/domain/model_profile.rs` に `ModelProfile` 型と `PROFILES` 静的配列を置く
  （依存ゼロの domain 層、DEP-001 準拠）。`ModelConfig::default()` はここから導出する
  （`ModelConfig::from_profile()`）。
- `scripts/model.env` に対応する env（`BONSAI_MODEL_ID` 等）を置き、各 `scripts/*.sh` は
  これを source して使う。`BONSAI_MODEL_ID` の値で分岐する `case` 節が4件の preset
  （Rust 側の `ModelProfile` と 1:1）を切り替えるため、`BONSAI_MODEL_ID` を変えるだけで
  GGUF/MLX repo、context 長、推論パラメータがまとめて切り替わる。
- thinking（`<think>` 出力）を有効にするかどうかは shell 側の `BONSAI_ENABLE_THINKING` のみで
  制御する。`ModelProfile` に `enable_thinking` フィールドは持たせない。
- 登録プロファイルは4件: `minicpm5-2b`（既定）、`minicpm5-1b`、`bonsai-8b`（legacy）、
  `ternary-bonsai-8b`（legacy）。legacy 2件は削除せず、後方互換の切替先として残す。
- 既定モデルを MiniCPM5-2B に変更する。GGUF は Q4_K_M、context_length は 32768、
  推論パラメータは temperature 0.6 / top_p 0.95 / top_k 20 / min_p 0.05 / max_tokens 2048 /
  repeat_penalty 1.05 を初期値とする。
- chat template は ADR-011 の方針（backend tokenizer が source of truth）をそのまま維持する。
  MiniCPM5-2B の jinja テンプレートは ChatML + `<think>` + `enable_thinking` kwarg に対応しており、
  既存の `--chat-template-kwargs '{"enable_thinking": false}'` と `src/agent/parse.rs` の
  `<think>` パーサはどちらも変更不要。

## Consequences

### Positive
- モデル差し替えが3手順（config.toml / env / `ModelProfile` への追加登録、
  [docs/execution/model-switching.md](../execution/model-switching.md) 参照）に収まる。
- 標準アーキテクチャの採用により、PrismML fork のビルド・保守負担なしに動作する。
- 既存のハーネス資産（LoopDetector、Compaction、Advisor 等、CLAUDE.md の「Scaffolding > Model」
  設計原則が支える機構群）はモデル非依存の設計のため温存される。

### Negative
- `docs/quality/lab-history.md` 等に蓄積された定量スコアは Bonsai-8B（1-bit、Qwen3-8B ベース）
  前提で測定されたものであり、MiniCPM5-2B では前提が異なるため再測定が必要になる。
- MiniCPM5 系の native tool call（`<function name=..><param ..>` の XML 形式）は未活用のまま
  残る。bonsai は引き続き system prompt 経由の `<tool_call>` JSON 方式を使うため、
  native 形式への移行は別途の follow-up 課題とする。
- 上記の推論パラメータ初期値は Lab paired evidence（ADR-003）による検証を経ていない。

## Related

- [ADR-003](ADR-003-paired-evidence-over-unpaired.md): 推論パラメータの検証規律
- [ADR-011](ADR-011-chat-template-backend-source-of-truth.md): chat template backend 委譲
- [docs/execution/model-switching.md](../execution/model-switching.md): 差し替え手順
- `src/domain/model_profile.rs`: `ModelProfile` / `PROFILES` / `resolve_model_id()`
- `scripts/model.env`: shell 側の対応する env
