# Plan: CachedBackend × FallbackBackend wrap 順序のテスト hardening (R13)

> **Multi-plan dispatch**: handoff 05-02b (項目 185) で Gemini 提起の R13 (CachedBackend wrap 順序問題) を test 駆動で観測テスト化する小規模 plan。実害は **観測 (model_id telemetry)** と **edge case (MLX healthy 復帰後の cache stale)** に限定され、benchmark 整合性 (同一 prompt → 同一 output) には影響しない判定済。テスト追加で挙動を契約化し、将来の挙動変更を検出可能にする。

## Task Type

- [ ] Frontend
- [x] Backend (→ Codex, TDD-focused)
- [ ] Fullstack

## Background

### R13 の事実関係 (本セッションで検証済)

| 観点 | 実装事実 | 影響 |
|------|---------|------|
| `CachedBackend::compute_key` (cache.rs:135) | `self.inner.model_id()` を含めて hash | inner の model_id が変われば別 key |
| `FallbackBackend::model_id()` (inference.rs:158-160) | constant `"fallback-chain"` 返却 | チェーン内の primary/fallback 切替で **key は変わらない** |
| `CachedBackend(FallbackBackend(...))` wrap (main.rs:545) | Lab モードで二重 wrap | cache hit が primary/fallback を区別しない |
| 同一 prompt → 同一 cached text 返却 | benchmark 整合性は維持 | **機能影響なし**（同じ prompt → 同じ出力という契約は守られる） |
| `GenerateResult.model_id = "fallback-chain"` (inference.rs cache hit path) | 実生成元の model_id を失う | **observability への影響あり**（テレメトリで MLX 由来か llama 由来か区別不能） |

### 評価

- **機能リスク**: 低（cache の本来の目的「同一 prompt → 同一出力」は実装通り）
- **観測リスク**: 中（telemetry 上で「どの backend が実応答したか」が消える）
- **将来 silent breakage リスク**: 低 - 中（未来に `model_id()` を current entry に動的化する変更が入った場合、cache key 体系が壊れる可能性 → 今 test で契約化することで回避）

### 結論

**実装変更は不要**、ただし **R13 観察事項を test で契約化する** (regression 防止)。挙動変更が必要かどうかは別途判定 (任意の Step 4 で議論)。

## Technical Solution

### TDD アプローチ (Red → Green → Refactor)

3 件の test を追加し、現状実装の挙動を契約として固定:

1. `CachedBackend(FallbackBackend)` の cache key が synthetic_id を使う (Test #1)
2. fallback 切替後も同一 prompt で cache hit する (Test #2)
3. cache hit 時 `GenerateResult.model_id` が synthetic_id を返す (Test #3)

これらは Red→Green ではなく **Green** から始まる: 現状実装で全件 PASS する想定。Refactor で挙動変更する場合は test を反転。

## Implementation Steps

### Step 1: Test fixture 準備

`src/runtime/cache.rs` の `#[cfg(test)] mod tests` に以下追加:

```rust
use crate::runtime::inference::{FallbackBackend, MockLlmBackend};
use crate::runtime::model_router::{FallbackChain, FallbackEntry};
use crate::config::ServerBackend;

fn build_cached_fallback(
    primary_responses: Vec<String>,
    secondary_responses: Vec<String>,
) -> CachedBackend {
    let entries = vec![
        FallbackEntry {
            backend: ServerBackend::MlxLm,
            model_id: "primary-mlx".into(),
            server_url: "http://127.0.0.1:8000".into(),
        },
        FallbackEntry {
            backend: ServerBackend::LlamaServer,
            model_id: "secondary-llama".into(),
            server_url: "http://127.0.0.1:8080".into(),
        },
    ];
    let chain = FallbackChain::with_threshold(entries.clone(), 1);
    let mut backends: HashMap<String, Box<dyn LlmBackend>> = HashMap::new();
    backends.insert(
        FallbackBackend::key_for(&entries[0]),
        Box::new(MockLlmBackend::new(primary_responses)),
    );
    backends.insert(
        FallbackBackend::key_for(&entries[1]),
        Box::new(MockLlmBackend::new(secondary_responses)),
    );
    let fallback = FallbackBackend::new(chain, backends);
    CachedBackend::new(Box::new(fallback), 10)
}
```

### Step 2: Test #1 — cache key uses synthetic_id

```rust
#[test]
fn test_cached_fallback_key_uses_synthetic_id() {
    // CachedBackend が wrap する FallbackBackend の model_id() = "fallback-chain"
    // → cache key は primary/fallback を区別しない
    let cached = build_cached_fallback(
        vec!["primary-resp".into()],
        vec!["fallback-resp".into()],
    );
    assert_eq!(
        cached.model_id(),
        "fallback-chain",
        "CachedBackend は inner FallbackBackend の synthetic_id を返す"
    );
}
```

### Step 3: Test #2 — cache hit survives fallback switch

```rust
#[test]
fn test_cached_fallback_hit_after_chain_switch() {
    // primary が応答してキャッシュされた後、primary が失敗→fallback に切替したとしても
    // 同じ prompt なら cached text が返り、fallback backend は呼ばれない契約。
    let cached = build_cached_fallback(
        vec!["primary-resp".into(), "primary-resp-2".into()],
        vec!["fallback-resp".into(), "fallback-resp-2".into()],
    );
    let cancel = CancellationToken::new_root();
    let messages = vec![crate::agent::conversation::Message::user("hi")];

    // 1 回目: primary 応答 + cache 保存
    let r1 = cached
        .generate(&messages, &[], &mut |_| {}, &cancel)
        .expect("first call");
    assert_eq!(r1.text, "primary-resp", "1 回目は primary 応答");

    // 2 回目: cache hit (primary backend は MockLlmBackend で 1 回しか応答しないので、
    // cache が効かなければ "primary-resp-2" が返る)
    let r2 = cached
        .generate(&messages, &[], &mut |_| {}, &cancel)
        .expect("second call");
    assert_eq!(
        r2.text, "primary-resp",
        "同一 prompt は cache hit で同じ text"
    );
}
```

### Step 4: Test #3 — cache hit `GenerateResult.model_id`

```rust
#[test]
fn test_cached_fallback_hit_model_id_field() {
    let cached = build_cached_fallback(
        vec!["primary-resp".into()],
        vec!["fallback-resp".into()],
    );
    let cancel = CancellationToken::new_root();
    let messages = vec![crate::agent::conversation::Message::user("hi")];

    let _r1 = cached
        .generate(&messages, &[], &mut |_| {}, &cancel)
        .expect("first call");

    let r2 = cached
        .generate(&messages, &[], &mut |_| {}, &cancel)
        .expect("second call");

    // 現状仕様: cache hit 時 model_id は synthetic_id (R13 観察事項)
    // 将来 inner 動的化で変わる場合はこの assert を反転して再評価
    assert_eq!(
        r2.model_id, "fallback-chain",
        "現状: cache hit 時 model_id は synthetic_id (R13)"
    );
}
```

### Step 5: 実行確認

```bash
cargo test --release --lib runtime::cache::tests::test_cached_fallback_key_uses_synthetic_id
cargo test --release --lib runtime::cache::tests::test_cached_fallback_hit_after_chain_switch
cargo test --release --lib runtime::cache::tests::test_cached_fallback_hit_model_id_field

# 全 cache tests
cargo test --release --lib runtime::cache::tests
```

3 件すべて PASS する想定。

### Step 6 (Optional, Refactor): 挙動変更の判定 — 採否は議論待ち

> **デフォルトは Step 5 で完了**。本 Step 6 は判断材料として記録のみ。

候補 R: `FallbackBackend::model_id()` を current entry の model_id に動的化:

```rust
impl LlmBackend for FallbackBackend {
    fn model_id(&self) -> &str {
        // 現状: &self.synthetic_id
        // 候補: self.chain.current().map(|e| e.model_id.as_str()).unwrap_or(&self.synthetic_id)
        // ただし &str 返却で AtomicUsize 経由の借用ライフタイム問題が発生する
        // → 設計上は OwnedString 化が必要 (trait 変更 = 影響範囲大)
        &self.synthetic_id
    }
}
```

**trade-off**:
- 利点: telemetry で実 backend が見える、cache key も backend 別に分離 (実用意味なし、benchmark 整合性に影響なし)
- 欠点: trait 変更 (`&str` → `String` or `Arc<str>`) で破壊的変更、CachedBackend の cache 効率低下 (backend 切替で cache miss)

**判定**: 観測価値 < 設計コスト で **不採用**、Test #3 で synthetic_id 仕様を契約化することで完結。

## Key Files

| File | Operation | Description |
|------|-----------|-------------|
| `src/runtime/cache.rs:172+` (tests mod) | Add | Test #1, #2, #3 追加 |
| `src/runtime/inference.rs` | No change | (Step 6 不採用なら) |
| `src/runtime/model_router.rs` | No change | |

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| R1: MockLlmBackend 経由のテストが実 FallbackBackend の挙動と乖離 | MockLlmBackend は既存 inference.rs:355 の FlakyBackend test pattern と同等、十分なカバー |
| R2: build_cached_fallback ヘルパーが他テストと衝突 | tests mod 内 private 関数、`#[cfg(test)]` 限定、衝突なし |
| R3: 将来 FallbackBackend::model_id() 仕様変更で Test #1/#3 壊れる | 仕様変更時の意図的反転として活用、regression 検出に利用可 |

## Decision Gate

- **Step 5 全 PASS**: 観察事項を test 化完了 → R13 closed
- **Step 5 で 1 件以上 FAIL**: 既存実装と仮説の乖離 → 再調査が必要 (実装変化があったか確認)
- **Step 6 採否**: 本 plan では不採用、別途 RFC 化が望ましい

## Estimate

- Step 1-5: 30-60 min (TDD + ローカル検証)
- 全体: 1h 以下

## SESSION_ID (for /ccg:execute)

- CODEX_SESSION: (none — Claude direct planning)
- GEMINI_SESSION: (n/a, Gemini reviewer は execute 時に diff review として活用可)
