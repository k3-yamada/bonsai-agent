# Greedy-on-Protocol Decoding for `<tool_call>` Parse Robustness (antirez/ds4 inspired)

**状態**: planning-only (未起票)、推奨度 ★★★ (tool_call parse failure 根本解消)
**推定工数**: ~4h (TDD strict 5 phase、grammar/JSON schema 強制経路で実装)
**起点**: antirez/ds4 server の "protocol syntax greedy / payload sampled" 設計

## §1. 背景 — Bonsai-8B の tool_call parse failure 問題

### ds4 が解いた問題
DeepSeek V4 Flash も含む LLM は tool_call の **protocol structure** (タグ / JSON 括弧 / カンマ) を
正しく出力しないと client 側 parser が落ちる。ds4 server の解決策:

> When the model is emitting stable protocol structure such as DSML tags, parameter
> headers, JSON punctuation, or closing markers, sampling is forced to `temperature=0`
> so the tool call stays parseable. This greedy mode does **not** apply to argument
> payloads.

つまり:
- **Syntax/protocol bytes** (`<tool_call>` open, `{`, `,`, `"key":`, `}`, `</tool_call>`): greedy (temp=0)
- **Payload bytes** (string values, JSON string literal の中身、code/file content): user の sampling 設定

これにより protocol は決定的、payload は creative になる。

### Bonsai-8B の現状 (項目 5 / 11 / 95)
- `src/agent/parse.rs` で `<tool_call>JSON</tool_call>` を regex + manual JSON parse
- `src/agent/validate.rs` で schema validation
- **parse 失敗時の経路** = 項目 95 「Continue Site」段階回復 (retry → replan → safe stop)
- **項目 194 で textual tool_call leak 調査** → parser regression test 2 件追加だが**根本 cause は LLM 出力品質**
- **項目 196 leak fix (a)** = system prompt rule 16 で `<think>` 内 JSON literal 抑止

### ギャップ
- Bonsai は llama-server HTTP API 経由 (項目 167)、推論ごとに temperature を動的変更不可
- 単一 `--temp` setting で whole stream を sample
- **protocol bytes での sampling 失敗** で `<tool_call>{ "name": "Read" ...` の `{` 後に余分な改行 / quotes ずれ等が混入

## §2. 設計 (3 案、推奨 = 案 A)

### 案 A (推奨): GBNF grammar constraint via llama-server
llama-server は `grammar` param で GBNF を渡せる (`-c grammar.gbnf` または request body `"grammar": "..."`)。

```gbnf
root         ::= toolcall-block
toolcall-block ::= "<tool_call>" ws json-obj ws "</tool_call>"
json-obj     ::= "{" ws "\"name\"" ws ":" ws string ws "," ws "\"args\"" ws ":" ws json-value ws "}"
ws           ::= [ \t\n]*
string       ::= "\"" string-char* "\""
string-char  ::= [^"\\] | "\\" .
json-value   ::= ...
```

- ✅ protocol structure を **decode 時に強制**、ds4 と等価な保証
- ✅ payload 中の string-char は normal sampling (creative output 保持)
- ✅ llama-server 既存 feature、Bonsai 側変更は request body へ `grammar` 追加のみ
- ❌ `<think>` 内では grammar 適用不可 (think モード切替で 2 段 generate 必要)
- ❌ GBNF 文法を 1 度書く必要 (~50 行)

### 案 B: 2-pass decoding (think → tool_call で grammar 切替)
think を sampled → `</think>` 検出後に tool_call generate を再開、その段でのみ grammar 適用。
- ✅ think の creative 性を完全保持
- ❌ 2-pass で round-trip latency が増 (~1.5x)
- ❌ HTTP API 2 回呼びの ordering / cancel 制御複雑化

### 案 C: Post-hoc JSON repair (json5 / jq-like fuzzy parse)
parse 失敗時に試行的修復 (trailing comma 削除 / smart quotes 統一 / etc)。
- ✅ 既存 backend 変更ゼロ
- ❌ 根本 cause 未解消、項目 95 Continue Site 既存実装と重複
- ❌ ds4 思想 (decoding-time greedy) と乖離

## §3. TDD strict 5 phase 計画

### Phase 1 (Red) — 失敗 test 6 件
- `t_gbnf_grammar_emit_tool_call_block` (`build_tool_call_grammar()` returns valid GBNF)
- `t_grammar_param_present_in_request_body` (`LlamaServerBackend::generate` が tools 非空時 grammar 付与)
- `t_grammar_param_absent_for_text_only` (tools 空のとき grammar 無し、回答 verbose 性保持)
- `t_grammar_dynamic_toggle_off_via_env` (`BONSAI_GBNF_ENABLED=0` で従来挙動互換)
- `t_grammar_passes_through_think_block` (think 内 JSON literal は grammar 制約対象外)
- `t_grammar_audit_log_emit` (`AuditAction::ToolCallGrammar` variant emit、項目 226 CriticCall 同 pattern)

### Phase 2 (Green) — 実装
- `src/runtime/llama_server.rs`: `build_tool_call_grammar(tools: &[ToolSchema]) -> String` 純関数
- `src/runtime/llama_server.rs`: `LlamaServerBackend::generate_with_params` で tools 非空 + env 有効時に request body へ `grammar: build_tool_call_grammar(...)` 追加
- `src/runtime/llama_server.rs`: `CachedBackend::generate_with_params` も同 override (項目 226 HIGH fix と同 pattern)
- `src/observability/audit.rs`: `AuditAction::ToolCallGrammar { tools_count, grammar_bytes }` variant
- env: `BONSAI_GBNF_ENABLED` (default ON / production default、Bonsai-8B 1bit の parse 失敗多発を考慮した攻めの default)
- env: `BONSAI_GBNF_GRAMMAR_FILE` (default = embedded constant、外部 GBNF 上書き経路)

### Phase 3 (Refactor)
- `prompts/tool_call_grammar.gbnf` を新規作成 (50 行)、`include_str!` で embed
- `build_tool_call_grammar` を `Cow<'static, str>` 返却で alloc 削減
- think 内 JSON literal 抑止は項目 196 既存 rule 16 で済み (本 plan で重複しない)

### Phase 4 (Smoke)
- G-4a (env unset = ON、本 plan の default): wall ≈ baseline、tool_call parse failure rate -50% 期待 (Bonsai core 22)
- G-4b (env disabled): 従来 parse failure rate 維持、後方互換確証
- G-4c (think + tool_call mixed task): think 中の JSON literal が grammar で誤検出されないこと確認 (項目 196 整合性)

### Phase 5 (Effectiveness — 別 plan)
- Lab v20 paired t-test で `BONSAI_GBNF_ENABLED` ON/OFF 5 paired cycle
- ACCEPT 基準: tool_call parse failure rate Δ ≥ -30% AND mean score Δ ≥ +0.01
- 期待効果 = parse failure 経由の wasted step 削減 → effective pass@k 向上

## §4. ds4 直接転用しない判断

### Tool ID radix tree replay map は転用しない
ds4 は server 内 in-memory map で `tool_id → exact DSML byte` を保持し、後続 request の
re-rendering で exact-byte 一致を保証する。これは:
- ✅ Stateless server で KV prefix mismatch 回避に必須
- ❌ Bonsai は session-local in-process loop でこの問題が存在しない (項目 25 checkpoint で local 完結)
→ 本 plan では転用しない。

### Greedy decoding 思想は **grammar constraint** で代替
ds4 は temperature を動的切替するが、llama-server backend では困難。
GBNF grammar による decoding-time constraint は **意味的に等価** (decoder の選択肢を制約):
- ds4 greedy = 確率分布から argmax (top-1)
- GBNF grammar = 確率分布を grammar 許可トークンに制限 → 制限内で argmax 同等が選ばれる傾向
両者は理論上、protocol bytes での parse 保証という同じ end goal を達成する。

## §5. 期待効果 (仮説、Phase 5 で検証)

| 仮説 | 反証条件 |
|---|---|
| H1: GBNF grammar で tool_call parse failure rate -30% 以上削減 | Lab v20 で parse failure delta < -10% |
| H2: parse failure 削減で effective pass@k +0.02 以上 | Lab v20 で mean score delta < +0.005 |
| H3: GBNF latency overhead ≤ +5% | smoke wall time delta > +10% |

H1+H2 成立なら本 plan ACCEPT → production default ON 維持。
H1 単独成立 (H2 失敗) なら parse は治るが score 向上に寄与しない = 別経路の改善が必要。
H3 失敗 (overhead > +10%) なら think 経路のみ grammar OFF にする 2-pass 移行を別 plan で検討。

## §6. 起票候補項目

- **項目 230** = 本 plan の Phase 1-3 完遂 + Phase 4 G-4a/b/c smoke
- **項目 231** (将来) = Lab v20 paired t-test ACCEPT/REJECT 判定

## §7. 依存 / 順序

- 項目 167 LlamaServerBackend HTTP API (済) — generate_with_params 経路の前提
- 項目 226 CriticConfig (済) — env opt-in pattern + AuditAction variant の先例
- 項目 196 think 内 JSON 抑止 rule 16 (済) — 本 plan は重複せず補完

## §8. リスク

| Risk | Mitigation |
|---|---|
| GBNF 文法バグで legitimate 出力が拒否される | Phase 1 test 6 件 + Phase 4 G-4c で think 内 JSON 通過確認 |
| llama-server version 依存で grammar param 仕様変動 | `prompts/tool_call_grammar.gbnf` を外部 file 上書き可、env `BONSAI_GBNF_GRAMMAR_FILE` で escape |
| think → tool_call 切替の grammar reset 漏れ | Phase 2 で request 単位で grammar 付与/未付与を判定 (response 跨ぎ state ナシ) |
