# Lab MLX Pre-Warm Timeout Bound (項目 252 critic M2 follow-up)

## 1. 問題定義

### critic adversary review M2 [MAJOR] verbatim

> `lab_mlx_prewarm` has no per-iter or total wall budget. If `backend.generate("ping")`
> hangs (which is precisely the MLX 2-bit cold-start latency problem F4 is designed
> to solve), the loop only exits via `cancel.is_cancelled()`. **A user pressing
> Ctrl+C can interrupt, but absent operator interaction, pre-warm can consume the
> entire Lab cycle wall budget (≤35 min target) and starve the actual benchmark.**
> Required before Smoke G-RT2.

### Lab cycle ≤35 min target との関係

- 項目 249 Phase 4 Smoke G-RT で MLX 2-bit primary は llama-server 1-bit gguf 比 ~2x
  latency = single inference 1-3 min 可能性。
- pre-warm count=3 (default) で 3-9 min、count=10 (上限) で 10-30 min 消費し得る。
- benchmark 自体が 15-25 min 必要なため、pre-warm が 10 min 超だと cycle ≤35 min 不達。
- timeout bound 無しでは Phase 4 Smoke G-RT2 で wall budget 不確定 → ACCEPT 判定不可。

### MLX 2-bit cold-start プロファイル (項目 249 G-RT 観測)

- llama-server 1-bit gguf: 5-30s for first chunk
- MLX 2-bit primary: 15-180s for first chunk (variability 大)
- 項目 249 F1 (BONSAI_LAB_LONG_SSE) で SSE chunk timeout を 60→180s に拡張済
- それでも 180s catch しきれず Phase 4 Smoke G-RT で SSE timeout 5 回発火 + 非ストリーミング fallback

## 2. 案比較

### 案 A: per-iter wall budget (env `BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS`)

**実装方針**:
```rust
let timeout_secs = lab_mlx_warmup_timeout_secs(); // env getter, default 180
for i in 0..count {
    if cancel.is_cancelled() { break; }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|s| {
        s.spawn(|_| {
            let r = backend.generate(...);
            let _ = tx.send(r);
        });
        match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
            Ok(Ok(_)) => success_count += 1,
            Ok(Err(e)) => log_event(Warn, ...),
            Err(_) => log_event(Warn, "timeout after {}s", timeout_secs),
        }
    });
}
```

**特徴**:
- 各 generate に独立 deadline (180s default)
- timeout 発火 = thread 切り離し、loop は次 iter へ進む
- env で 1-600s range で調整可能、default 180s
- 制御粒度: per-iter (loop 全体 wall budget は count × timeout で見積もり可能)

**欠点**:
- thread spawn overhead (毎 iter)
- timeout fire 後の thread は dangling (backend.generate が hang 続行)、accumulated thread leak

### 案 B: 全体 wall budget (loop deadline、env `BONSAI_LAB_MLX_WARMUP_TOTAL_SECS`)

**実装方針**:
```rust
let deadline = Instant::now() + Duration::from_secs(total_timeout);
for i in 0..count {
    if cancel.is_cancelled() || Instant::now() >= deadline { break; }
    let result = backend.generate(...);
    ...
}
```

**特徴**:
- 単一 deadline、loop 開始時刻基準
- 各 iter 開始時に deadline チェック (iter 中の hang は catch 不可)
- thread spawn 不要、シンプル
- env で 1-1800s range、default 600s (10 min)

**欠点**:
- **iter 中 hang は catch 不可**: 1 iter が無限 hang すると total_timeout も無効化
- 案 A と比較し弱い保証

### 案 C: F1 既存 SSE chunk timeout 流用 (新 env なし)

**特徴**: code 変更ゼロ。`BONSAI_LAB_LONG_SSE=1` (F1) で 180s SSE chunk timeout が backend 内部に既に設定済、`backend.generate` が 180s で Err 返却 → 既存 loop が次 iter へ進む。

**欠点**:
- **SSE chunk timeout は chunk 間隔の上限**: 初回 chunk が来た後の subsequent chunk 間 180s 以下なら timeout 発火しない → request 全体 wall budget は無限可能性
- 項目 249 G-RT で F1 単独では cycle ≤35 min 未達確証済 = 既に **棄却された経路**
- M2 finding は「F1 では不十分」を前提とした要求

### 案 D: blocking + tokio::time::timeout 統合 (将来検討)

**特徴**: tokio runtime 導入で async pattern を pre-warm 限定で導入。

**欠点**:
- bonsai-agent は現在 sync-only (項目 100/103/105 系の意図的選択)
- async runtime 導入は scope 過大、本 finding 範囲外

## 3. 5 軸比較

| 軸 | 案 A (per-iter) | 案 B (全体 wall) | 案 C (F1 流用) | 案 D (tokio) |
|----|---|---|---|---|
| Lab cycle ≤35 min 完走可能性 | ★★★ 確実 | ★★ iter hang で破綻リスク | ★ G-RT で既に未達確証 | ★★★ 確実 |
| 実装工数 | ★★ thread::scope + mpsc (~80 LOC) | ★★★ Instant deadline (~20 LOC) | ★★★ 0 LOC | ★ tokio 依存追加 (大改修) |
| TDD test 設計容易性 | ★★ SlowBackend (sleep 999s) + tighten timeout で fire 確証 | ★★ 同左 | ★ test 不可 (env のみ) | ★★ 同左 |
| 既存機構との整合性 | ★★★ project 全体 sync 維持 | ★★★ 同左 | ★★★ 同左 | ★ async 導入で 100+ ファイル影響 |
| rollback 容易性 | ★★★ env=0 → 完全 no-op (default 180s 無効化) | ★★★ 同左 | ★★★ env unset で no-op | ★ tokio rip-out 困難 |

**総合**:
- 案 A = ★★ (実装 ★★、確実性 ★★★、保証強い)
- 案 B = ★★ (実装 ★★★、確実性 ★★、保証弱い)
- 案 C = ★ (既に未達確証で棄却)
- 案 D = ★ (scope 過大)

## 4. 推奨案: 案 A (per-iter wall budget)

### 採用理由

1. **Lab cycle ≤35 min 完走の確実性**: per-iter 180s × max 10 = 30 min が上限、benchmark 時間 (15-25 min) と合わせ ≤35 min target に収まる予測可能性。
2. **iter hang catch**: 案 B の構造的弱点 (iter 中 hang は catch 不可) を解決、M2 finding の核心要求を満たす。
3. **env default で安全**: `BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS` unset で default 180s = F1 sse_chunk_timeout と整合。
4. **rollback**: env=0 で完全 no-op (timeout 無効化、素朴 loop 経路) の sentinel 設計。

### default 値の検討

| default | 根拠 | リスク |
|---|---|---|
| 60s | F1 旧値 sse_chunk_timeout default | 短すぎる、MLX 2-bit cold start 取りこぼし |
| **180s** | F1 BONSAI_LAB_LONG_SSE 後の値、整合性高い | **推奨**: F1 とセットで動作確証可能 |
| 300s | MLX 2-bit worst-case 観測 | 長すぎ、count=10 で 50 min |

**推奨 default = 180s** (F1 整合)。

### 副次設計判断

- **count=0**: pre-warm スキップ (既存挙動維持、`BONSAI_LAB_MLX_WARMUP_COUNT=0` 経路)
- **timeout=0**: 無効化 sentinel として「timeout なし、元の挙動」を選べる
- **thread leak 対策**: `thread::scope` で borrow lifetime 担保、scope 終了で thread は detached (backend.generate 内部で cancel polling されれば収束)

## 5. TDD strict 3-phase outline

### Phase 1: Red (4 test、test infrastructure 含む)

**test 1: timeout fire で続行**
```rust
struct SlowBackend(Duration); // sleep(duration) 後 Ok 返却
// env timeout=1s, sleep=10s, count=3 → 3 timeout, succ=0
unsafe { std::env::set_var("BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS", "1"); }
let succ = lab_mlx_prewarm(&SlowBackend(Duration::from_secs(10)), 3, &cancel);
assert_eq!(succ, 0); // 全 timeout
```

**test 2: timeout < latency で early-return + 次 iter 進行**
```rust
// VariableBackend: 1st iter slow (timeout 発火), 2nd iter fast (Ok)
let succ = lab_mlx_prewarm(&VariableBackend::new(vec![10, 0]), 2, &cancel);
assert_eq!(succ, 1); // 1st timeout / 2nd Ok
```

**test 3: env range validation**
```rust
unsafe { std::env::set_var("BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS", "0"); }
assert_eq!(lab_mlx_warmup_timeout_secs(), 0); // sentinel disable
unsafe { std::env::set_var("BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS", "601"); }
assert_eq!(lab_mlx_warmup_timeout_secs(), 180); // out-of-range fallback
unsafe { std::env::set_var("BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS", "60"); }
assert_eq!(lab_mlx_warmup_timeout_secs(), 60);
```

**test 4: timeout=0 で素朴 loop (既存挙動維持)**
```rust
unsafe { std::env::set_var("BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS", "0"); }
let succ = lab_mlx_prewarm(&SlowBackend(Duration::from_secs(2)), 1, &cancel);
assert_eq!(succ, 1); // 待ち合わせて完走
```

**Phase 1 Red 検証**: 4 test 全 fail (timeout 機構未実装)。

### Phase 2: Green (本実装)

**`src/config.rs`**: env getter `lab_mlx_warmup_timeout_secs() -> u64` (default 180、range 0..=600、0=sentinel)。

**`src/agent/experiment.rs::lab_mlx_prewarm`**: signature 不変、本体に thread::scope + mpsc::channel + recv_timeout 導入。test-only `SlowBackend` / `VariableBackend` 追加。

**Phase 2 Green 検証**: 4 test 全 PASS + 既存 4 test 全 PASS = 8 test PASS (退行ゼロ)。

### Phase 3: Refactor

- rustdoc 拡充 (M2 critic finding 引用 + timeout 値選択根拠)
- log_event の timeout 発火時 message を `[lab] pre-warm i/N TIMEOUT after Ns` 形式に統一
- clippy clean / fmt clean

## 6. Phase 4 Smoke 検証基準

### G-PWT1: env unset (後方互換)
- env `BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS` unset、`BONSAI_LAB_MLX_WARMUP=1` のみ
- 期待: default 180s で動作、既存 G-RT2 動作と差分 0 (実機 MLX で OK path のみ)
- ACCEPT: cargo test --lib lab_mlx_prewarm 8 passed、stderr に timeout log 0 件

### G-PWT2: env=60 で timeout 発火 catch (合成 hang)
- env `BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS=60`、MLX server を **port 8001 (存在しない)** に向ける
- 期待: 各 iter 60s で connect 失敗 → succ=0、stderr に項目 252 F3 警告
- ACCEPT: total wall ≤ 60s × 3 + overhead ≤ 200s

### G-PWT3: env=180、MLX 正常起動
- 環境変数全部入り (本番想定): `BONSAI_LAB_LONG_SSE=1 BONSAI_LAB_MLX_ONLY=1 BONSAI_LAB_MLX_WARMUP=1 BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS=180`
- 期待: pre-warm 3/3 OK、stderr に timeout log 0 件
- ACCEPT: pre-warm wall ≤ 9 min

### G-RT2 (M2 解消後の本番 Smoke、Lab v22 paired prerequisite)
- 環境変数全部入り: `BONSAI_LAB_LONG_SSE=1 BONSAI_LAB_MLX_ONLY=1 BONSAI_LAB_MLX_WARMUP=1 BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS=180 BONSAI_LAB_TEMP=0 BONSAI_LAB_TASK_LIMIT=5 ./scripts/lab_v22_aa_test.sh`
- ACCEPT: cycle wall ≤ 35 min (Lab v22 paired 5h 完走 prerequisite)
- REJECT 時: timeout 値 60s に調整 or 案 B 切替検討

## 7. Rollback strategy

### 完全 rollback (env=0 sentinel)

env `BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS=0` で「timeout 無し、元の挙動」経路:
```rust
let timeout = lab_mlx_warmup_timeout_secs();
if timeout == 0 {
    // 案 A 前の素朴 loop (timeout 無し、既存挙動 100% 互換)
    let result = backend.generate(...);
    ...
} else {
    // 案 A の thread::scope パス
}
```

### git revert 影響範囲

- 単一 commit (Phase 2 Green) で本体実装、Phase 1 Red と Phase 3 Refactor は分離
- revert 対象: `src/config.rs` (env getter)、`src/agent/experiment.rs` (lab_mlx_prewarm 本体 + test)
- main.rs は変更なし (caller は signature 同じ、内部実装変更のみ)

## 8. Open questions

1. **thread leak 監視**: timeout 発火後の dangling thread が backend 内部で完走するまで resource を保持。`std::thread::scope` で scope 終了時に detach されるが、accumulated thread leak が long-running Lab cycle で問題化するか? → Phase 4 G-PWT2 で 10 サイクル連続実行し RSS 観測。
2. **MLX server 側 connection limit**: timeout で abandon された request が server 側で queue 溜まる可能性。HTTP keep-alive 切断は backend 側 (ureq) で行われるか要確認。
3. **default 値の Lab v22 paired 影響**: 180s × 3 = 9 min pre-warm + 25 min benchmark = 34 min/cycle、10 cycle = 5.7h wall。target 5h を 14% 超過。**60s × 3 = 3 min pre-warm + 25 min benchmark = 28 min/cycle、10 cycle = 4.7h** のほうが ≤5h target に確実だが、60s で MLX cold start 取りこぼしのリスク = G-PWT3 で実測判定。

## 9. 関連 plan / memory

- `.claude/plan/lab-runtime-stabilization-f4-mlx-latency.md` (413 行) — F4 全体設計、本 plan は M2 詳細
- `CLAUDE.md` 項目 249 / 252 entry — 経緯
- `memory/session_2026_05_20_handoff.md` — 本 plan 起票時の作業 context
