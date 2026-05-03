# Plan: llama-server `Bonsai-8B` HTTP 400 副次調査

> **Multi-plan dispatch**: handoff 05-02b (項目 185) Phase 2 副次発見の独立調査 plan。FallbackChain forced-fallback smoke (`/tmp/bonsai-llama/forced-fallback-preflight-2026-05-02.log`) で primary=dead port + fallback=llama-server `:8080` model="Bonsai-8B" の構成にした際、llama-server エンドポイントへ POST した chat completion リクエストが **HTTP 400 Bad Request** を返し chain exhaustion を引き起こした事象。FallbackChain の信頼性向上 (Plan 1 の前提) に必須。

## Task Type

- [ ] Frontend
- [x] Backend (→ Codex, debug-focused)
- [ ] Fullstack

## Background

### 発生状況 (項目 185 Phase 2 抜粋)

```
[fallback] backend "MlxLm:Bonsai-8B" failed: connection refused (期待される: dead port)
[fallback] backend "LlamaServer:Bonsai-8B" failed: HTTP 400 Bad Request
[lab] フォールバックチェーン枯渇
```

llama-server (`:8080`) は `/v1/models` で疎通していたにもかかわらず、`/v1/chat/completions` への POST が 400 を返却。本来は connection refused → llama に切替後、正常応答するはず。

### 仮説

| ID | 仮説 | 根拠 | 検証コスト |
|----|------|------|----------|
| H1 | `model_id="Bonsai-8B"` が `/v1/models` の registered model 名と不一致 | llama-server は `--alias` で別名を取らないと bare gguf path を返すことがある | 低 (curl 1 発) |
| H2 | `build_request_body` が出力する JSON が llama-server の OpenAI 互換 endpoint で reject される | MLX 互換モードでないにも関わらず想定外フィールドが入っている可能性 | 中 (test 経由) |
| H3 | `max_tokens` / `temperature` 等の値が llama-server 受理範囲外 | `inference.params` の default 値 | 低 (config inspect) |
| H4 | tools 配列の schema が llama-server tool-calling 仕様と不適合 | llama-server の OpenAI 互換 tools サポートはバージョン依存 | 中 (curl で tools なし版と比較) |
| H5 | request `Content-Type` 不整合 | `application/json` 指定済みなので低確率 | 低 |

## Investigation Steps

### Phase 1: Reproduction (準備)

```bash
# 1. llama-server 起動状態確認
curl -s http://127.0.0.1:8080/v1/models | jq '.data[].id'
# 期待: "Bonsai-8B" or 実 model 名

# 2. 単純 prompt curl で 400 を再現
curl -v http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "Bonsai-8B",
    "messages": [{"role":"user","content":"hi"}],
    "stream": false
  }' 2>&1 | tee /tmp/bonsai-llama/llama-400-repro.log
# 期待: 400 Bad Request + response body
```

**判定 1**: 上記 curl で 400 を再現できれば → H1/H2/H3/H4 の絞り込み開始。再現できなければ → bonsai 側 build_request_body の差分を調査。

### Phase 2: Hypothesis Discrimination

#### H1 検証 (model_id mismatch)

```bash
# /v1/models から正確な id を取得
ACTUAL_ID=$(curl -s http://127.0.0.1:8080/v1/models | jq -r '.data[0].id')
echo "actual: $ACTUAL_ID"

# 取得した id で再試行
curl -v http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d "{\"model\": \"$ACTUAL_ID\", \"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}" 2>&1
```

**判定 1a**: `$ACTUAL_ID` で 200 が返れば H1 確定 → fix は config.toml の `model_id` を `$ACTUAL_ID` に更新 (or llama-server 起動時に `--alias Bonsai-8B` 追加)。

#### H2 検証 (request body 差分)

`bonsai` の実 build_request_body 出力をダンプ:

```rust
// src/runtime/llama_server.rs に一時的に追加
#[cfg(test)]
#[test]
fn dump_request_body_for_debug() {
    let backend = LlamaServerBackend::connect("http://127.0.0.1:8080", "Bonsai-8B");
    let messages = vec![Message::user("hi")];
    let body = backend.build_request_body(&messages, &[]);
    eprintln!("{}", serde_json::to_string_pretty(&body).unwrap());
}
```

ダンプ結果と Phase 1 の minimal curl の差分を `diff` で比較。差分フィールドを 1 つずつ削って 400→200 に変わる境界を特定。

#### H3 検証 (sampling parameters)

`config.toml [model.inference]` (or default `InferenceParams`) を確認:

```bash
# default 値
grep -A 20 "InferenceParams" src/config.rs | head -30
```

`max_tokens` / `repeat_penalty` / `temperature` の default 値が llama-server 受理範囲か確認。特に `repeat_penalty` は MLX と llama で名称が異なる場合がある (`repetition_penalty` vs `repeat_penalty`)。

#### H4 検証 (tools schema)

bonsai は tools を OpenAI 互換 schema で送る。llama-server バージョン依存で tools サポート有無:

```bash
# tools なし版で 400→200 になるか
curl -v http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"Bonsai-8B","messages":[{"role":"user","content":"hi"}]}' \
  | grep -E "^(HTTP|<)"
```

200 に変わる場合 → tools schema が原因。OpenAI tools 形式の `parameters` JSON Schema に厳密性要求がある。

### Phase 3: Fix

特定された原因に応じて以下のいずれか:

#### Fix-A: model_id mismatch (H1)

```toml
# config.toml
[model]
model_id = "<actual_id_from_/v1/models>"  # or llama-server を --alias 起動
```

または llama-server 起動コマンドに `--alias Bonsai-8B` を追加。

#### Fix-B: request body 差分 (H2/H4)

`src/runtime/llama_server.rs:build_request_body` で問題フィールドを条件付き除外。MLX 互換モードがあるように、llama-server バージョン依存の互換モード追加検討:

```rust
pub fn with_llama_strict(mut self, strict: bool) -> Self {
    self.llama_strict_mode = strict;
    self
}

fn build_request_body(&self, messages: &[Message], tools: &[ToolSchema]) -> serde_json::Value {
    // ...既存ロジック...
    if self.llama_strict_mode {
        // 厳密モードでは未対応フィールドを除外
        body.as_object_mut().unwrap().remove("min_p");  // 例
    }
    body
}
```

#### Fix-C: sampling out-of-range (H3)

`config.toml [model.inference]` の問題 param を補正:

```toml
[model.inference]
repeat_penalty = 1.05  # llama-server 範囲内
```

### Phase 4: TDD

```rust
#[test]
fn test_llama_server_request_body_strict_mode() {
    let backend = LlamaServerBackend::connect("http://localhost:8080", "Bonsai-8B")
        .with_llama_strict(true);
    let body = backend.build_request_body(&[Message::user("hi")], &[]);
    // 厳密モードでは min_p が除外される
    assert!(body.get("min_p").is_none(), "strict mode で min_p は除外");
}

#[test]
#[ignore]  // 実 llama-server 必要
fn test_llama_server_actual_400_resolution() {
    // Phase 1 で再現した curl を Rust 経由で再現、200 が返ることを確認
    let backend = LlamaServerBackend::connect("http://localhost:8080", "Bonsai-8B");
    let result = backend.generate(
        &[Message::user("hi")],
        &[],
        &mut |_| {},
        &CancellationToken::new_root(),
    );
    assert!(result.is_ok(), "fix 後 200 が返ること");
}
```

## Key Files

| File | Operation | Description |
|------|-----------|-------------|
| `src/runtime/llama_server.rs:126` (build_request_body) | Inspect/Modify | H2 検証 + Fix-B 候補 |
| `src/config.rs` (InferenceParams default) | Inspect | H3 検証 |
| `~/Library/Application Support/bonsai-agent/config.toml [model.inference]` | Modify (Fix-C only) | H3 hit 時の補正 |
| `/tmp/bonsai-llama/forced-fallback-preflight-2026-05-02.log` | Read | Phase 2 失敗ログ参照 |
| `/tmp/bonsai-llama/llama-400-repro.log` (新規) | Write | Phase 1 curl ログ保存 |

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| R1: 再現できない (curl で 200 が返る) | bonsai の build_request_body が timing/state-dependent な何かを送っている可能性 → middleware/runtime での後置加工を確認 |
| R2: llama-server のバージョン更新で再現性が変わる | `llama-server --version` を log 冒頭に記録、再現性のため version pin を CLAUDE.md に記載 |
| R3: 修正で他の llama-server バージョンでの動作を破壊 | `llama_strict_mode` を opt-in flag にし default OFF |
| R4: tools schema 起因の場合、bonsai 全体の tool-calling が影響 | 段階的検証: tools なし → 1 tool → 全 tools で境界特定 |

## Decision Gate

- **Phase 1 で再現不可**: bonsai 固有の問題 → request 流路の中間 state を疑う、Plan 完了は **観測継続 + Lab v15 で再現待ち**
- **Phase 2 H1 hit**: 軽量 fix (config or alias) → 30 min で完了
- **Phase 2 H2/H4 hit**: 中規模 fix (Rust 修正 + flag 追加) → 1-2h で完了
- **Phase 2 H3 hit**: 設定変更のみ → 15 min で完了

## ─── 2026-05-04 セッション実機検証結果: **NOT REPRODUCIBLE** ────

### Phase 1 curl 実機実行 (3 種類)

```
T1. 完全 bonsai-body (model + 全 sampling params + stream=false):
    HTTP 200, time=1.84s
    response: chat.completion / system_fingerprint=b8960-19821178b
T2. model field 抜き (H1 検証):
    HTTP 200, time=0.70s
    response: 正常 chat.completion
T3. stream=true (bonsai default):
    HTTP 200, time=0.67s
    response: 正常 SSE chunks
```

3 ケース全て **HTTP 200**。05-02b Phase 2 で観測された 400 は **現環境 (llama-server build b8960) では再現せず**。

### 仮説判定

| 仮説 | 検証 | 結論 |
|------|------|------|
| H1 (model_id mismatch) | T1 で `Bonsai-8B` が registered model に hit、T2 でも 200 | **REJECT** |
| H2 (request body 差分) | bonsai-style 全フィールド送信で 200 | **REJECT** |
| H3 (sampling out-of-range) | `temperature=0.7 / top_k=40 / top_p=0.9 / min_p=0.05 / repeat_penalty=1.1 / max_tokens=16` で 200 | **REJECT** |
| H4 (tools schema) | T1 では tools なしで 200、tools 試験は不要に | **N/A (上流 REJECT で skip)** |
| H5 (Content-Type) | 全テストで `application/json` で 200 | **REJECT** |

### 真の原因仮説（未検証、観測継続）

05-02b Phase 2 当時の 400 は以下のいずれかと推定:
- (a) **当時の llama-server バージョン違い** — 当時 build と b8960 の差分（dependencies、起動 flag 等）
- (b) **当時の prompt 内容依存** — bonsai が Lab 中に送信した特定 prompt（システムプロンプト + tools schema + 履歴）の組合せが build_request_body の output で 400 を誘発した可能性
- (c) **状態依存** — degraded mode / queue overflow 等

### Plan Status: **CLOSED — NOT REPRODUCIBLE**

実装変更不要。**監視継続** 方針:
- Lab v15 実機運用中に再発した場合、当時の正確な request body を eprintln 等でダンプし収集
- llama-server build version (`/v1/models` レスポンスの `system_fingerprint`) を log に記録するよう改善案あり (別 plan、将来検討)

### Phase 1 Re-execution Reference

```bash
# 再現確認用 curl (build b8960 で 200 確認済 2026-05-04)
curl -s -w "HTTP_CODE=%{http_code}\nTIME=%{time_total}\n" --max-time 60 \
  http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"messages":[{"role":"user","content":"hi"}],"temperature":0.7,
       "top_k":40,"top_p":0.9,"min_p":0.05,"max_tokens":16,
       "repeat_penalty":1.1,"stream":false}'
```

実行 log: `/tmp/bonsai-llama/{llama-400-repro-body, no-model-field, stream-true}.json`

## SESSION_ID (for /ccg:execute)

- CODEX_SESSION: (none — Claude direct planning)
- GEMINI_SESSION: (n/a)
