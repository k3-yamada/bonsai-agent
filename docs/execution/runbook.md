# bonsai-agent Runbook (ビルド・テスト・実行)

> Z-1 Phase 4 で CLAUDE.md から分離 (項目 255)。元の CLAUDE.md「ビルド・テストコマンド」「Rust Edition」「テストパターン」 verbatim 移行。

## ビルド・テストコマンド

```bash
cargo build                    # ビルド
cargo test --lib               # ユニットテスト（1434テスト、2026-06-02 時点）
cargo test --test structural   # Z-4 layer/size/eprintln lint
cargo test -- --ignored        # 統合テスト（llama-server/ネットワーク必要）
cargo clippy -- -D warnings    # リント
cargo fmt -- --check           # フォーマットチェック
cargo run -- --manifest        # ケイパビリティ一覧
cargo run -- --list-tools      # 登録ツール一覧（whitelist 適用後の live registry）
cargo run -- --vault           # ナレッジVault概要
cargo run -- --lab             # 自律的自己改善ループ（pass^k評価）
```

## Rust Edition

Rust **2024 edition**。let chains、div_ceil 等を使用。

## テストパターン

- `MockLlmBackend` — スクリプト化レスポンス (常に Ok を返す、Err-path test には `AlwaysFailBackend` (項目 252 F1) など test-only impl)
- `MemoryStore::in_memory()` — インメモリ SQLite
- `#[ignore]` — 実サーバー/ネットワーク必要なテスト
- `MultiRunTaskScore::from_scores()` — pass^k 指標の単体テスト
- env-gated 機構の test pattern: `pub(crate) static FACTCHECK_ALL_ENV_TEST_LOCK` 等 cross-file Mutex (項目 226/229/233/235)
- `LAB_RUNTIME_ENV_TEST_LOCK` (config.rs:375) — Lab 系 env 全般を serialize する cross-file mutex
- `VAULT_LINT_LAB_ENV_TEST_LOCK` (vault_lint.rs:277) — vault_lint env を serialize

## Lab 起動コマンド (項目 249/252 で env 拡張)

### 基本 Lab
```bash
cargo run -- --lab
```

### Smoke G-RT2 (項目 252 M2 解消後の本番 Smoke)
```bash
cargo build --release  # ~28s
# MLX server 起動 (port 8000、prism-ml/Ternary-Bonsai-8B-mlx-2bit)
BONSAI_LAB_LONG_SSE=1 \              # F1: SSE chunk timeout 60→180s
BONSAI_LAB_MLX_ONLY=1 \              # F2: primary backend を MLX 切替
BONSAI_LAB_MLX_WARMUP=1 \            # F4: MLX server pre-warm 有効化
BONSAI_LAB_MLX_WARMUP_COUNT=3 \      # pre-warm 回数 (default 3、range 1..=10)
BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS=180 \  # M2: per-iter wall budget (default 180s、env=0 sentinel で素朴 loop)
BONSAI_LAB_TEMP=0 \                  # temperature override (deterministic)
BONSAI_LAB_TASK_LIMIT=5 \            # task pool 削減 (smoke 用)
./scripts/lab_v22_aa_test.sh

# ACCEPT 基準: cycle wall ≤ 35 min (Lab v22 paired 5h 完走 prerequisite)
```

### Phase 2 Paired Re-evaluation (G-MCT2 ACCEPT 後)

`lab-v22-paired-metric-mandatory.md` §3 Phase 2 の 3 target を runner script として完備:

```bash
# Phase 1 σ_noise 確立 (A/A test、~5h)
nohup ./scripts/lab_v22_aa_test.sh > /tmp/aa_run.log 2>&1 &
python3 scripts/lab_v22_metric.py ./lab-v22-aa-logs --mode aa

# Phase 2 target #1: 項目 263 BUDGET ratio tune 真効果 (~12h)
nohup ./scripts/g_paired_263_v2.sh > /tmp/p263_run.log 2>&1 &

# Phase 2 target #2: 項目 264 案 D-2 MEMORY_AUG 真効果 (~12h)
nohup ./scripts/g_paired_265_v2.sh > /tmp/p265_run.log 2>&1 &

# Phase 2 target #3: 項目 262 PROMPT_AUGMENT 真効果 (~12h)
nohup ./scripts/g_paired_262_v2.sh > /tmp/p262_run.log 2>&1 &
```

ACCEPT 条件 (各 target 共通): Δ ≥ max(0.010, σ_noise × 2) かつ Wilcoxon p < 0.05 かつ Cohen's dz ≥ 0.3

### Smoke G-MCT2 (項目 265 max_context_tokens reduction 効果検証)
```bash
cargo build --release  # ~30s (Phase 1-3 反映後の binary 必須)
# MLX server 起動 (port 8000、prism-ml/Ternary-Bonsai-8B-mlx-2bit)
./scripts/start-mlx-server.sh &

mkdir -p lab-265-smoke-logs
BONSAI_LAB_LONG_SSE=1 \
BONSAI_LAB_MLX_ONLY=1 \
BONSAI_LAB_MLX_WARMUP=1 \
BONSAI_LAB_TEMP=0 \
BONSAI_LAB_TASK_LIMIT=5 \
BONSAI_LAB_SMOKE=1 \              # 項目 265: 自動 max_context=6000 → level1=4500 強制発火
BONSAI_T6_PROMPT_AUGMENT=1 \      # 項目 262 stack
BONSAI_DYNAMIC_BUDGET=1 \         # 項目 263 + 261 Phase 5 axis-priority prune
./target/release/bonsai --lab --lab-experiments 0 \
  > lab-265-smoke-logs/g_mct2_smoke.log 2>&1

# ACCEPT 条件:
# (a) [prev: marker count >= 5 (15 run 中、80%+ 発火率)
grep -c "\[prev:" lab-265-smoke-logs/g_mct2_smoke.log
# (b) compaction.budget emit と prune marker の time window 整合
grep -E "compaction.budget|\[prev:" lab-265-smoke-logs/g_mct2_smoke.log | head -20
# (c) 既存 cargo test --lib 1377 passed retention
cargo test --lib 2>&1 | tail -3
```

## env 一覧 (項目 246/249/252/254)

| Env | Default | 範囲 | 効果 |
|---|---|---|---|
| `BONSAI_VAULT_LINT_LAB` | OFF | bool | Lab 起動前の Vault sanity gate (項目 246) |
| `BONSAI_VAULT_LINT_STRICT` | OFF | bool | not_clean で abort (項目 251 bail) |
| `BONSAI_VAULT_LINT_STALE_DAYS` | 90 | 1..=365 | Vault stale 軸閾値 |
| `BONSAI_VAULT_UNREVIEWED_DAYS` | 14 | 1..=90 | Vault unreviewed_aged 5 軸目閾値 (項目 254) |
| `BONSAI_LAB_LONG_SSE` | OFF | bool | SSE chunk timeout 60→180s (項目 249 F1) |
| `BONSAI_LAB_MLX_ONLY` | OFF | bool | primary backend を MLX 切替 (項目 249 F2) |
| `BONSAI_LAB_TASK_LIMIT` | None | int | task pool 削減 (項目 249 F3) |
| `BONSAI_LAB_MLX_WARMUP` | OFF | bool | MLX server pre-warm 有効化 (項目 252 F4 案 A) |
| `BONSAI_LAB_MLX_WARMUP_COUNT` | 3 | 1..=10 | pre-warm 回数 (項目 252) |
| `BONSAI_LAB_MLX_WARMUP_TIMEOUT_SECS` | 180 | 1..=600、0=sentinel | per-iter wall budget (項目 252 M2) |
| `BONSAI_LAB_TEMP` | model default | float | temperature override (項目 247) |
| `BONSAI_FACTCHECK_ALL_TRAJECTORIES` | OFF | bool | factcheck scope 拡張 (項目 235) |
| `BONSAI_DYNAMIC_BUDGET` | OFF | bool | Compaction dynamic budget (項目 248) |
| `BONSAI_DYNAMIC_BUDGET_RATIOS` | 30/30/15/25 | 4 要素 sum=1.0 | 4 軸配分 (default 項目 263) |
| `BONSAI_DYNAMIC_BUDGET_ALPHA` | 0.2 | 0.0..=1.0 | relevance 反映係数 |
| `BONSAI_LAB_SMOKE` | OFF | bool | smoke task pool (5 件) 使用 + 項目 265 max_context 自動縮小 (14000→6000) + readonly tool whitelist 自動適用 (Z-NEW-E) |
| `BONSAI_LAB_MAX_CTX` | None | 1..=14000 | max_context_tokens 明示 override (項目 265、smoke より優先) |
| `BONSAI_T6_PROMPT_AUGMENT` | OFF | bool | T6 LongHorizonPlanning system prompt augment (項目 262、+14.4% strong ACCEPT、paired re-eval 待ち) |
| `BONSAI_ENABLED_TOOLS` | None | comma-list | deny-by-default tool whitelist (Z-NEW-E)。列挙 tool のみ active、未設定で全 tool。smoke より優先 |
| `BONSAI_MLX_IDLE_TIMEOUT_SEC` | 0 (OFF) | int | B-1: N 秒 idle で MLX server 自動 kill + 次 request で lazy respawn。0 で lifecycle supervisor 全体無効 (既存挙動保持) |
| `BONSAI_MLX_SPAWN_PROGRAM` | `~/.venvs/bonsai-mlx/bin/mlx-openai-server` | path | B-1: MLX server 起動プログラム (lazy respawn 用)。idle timeout>0 時のみ使用 |
| `BONSAI_MLX_AUTO_CLAMP` | OFF | bool | B-3: 起動時 MLX `/props` の n_ctx で context_length を `min(configured, n_ctx)` にクランプ。server 未応答時 no-op。LocalAI fit_params 思想 |

### Phase 2 メモリ最適化 sidecar (`scripts/start-mlx-sidecar.sh`)

cubist `mlx-openai-server` の drop-in 代替 (`scripts/mlx_server/server.py`)。OpenAI 互換 (`/v1/models` + `/v1/chat/completions` SSE) のまま、MLX メモリ最適化を解禁する。**bonsai 側は `server_url` (port 8888) 経由の純粋 consumer で Rust 変更不要**。以下 env は sidecar 専用 (cubist は未対応)。

| env | default | 型 | 説明 |
| --- | --- | --- | --- |
| `BONSAI_MLX_CACHE_LIMIT_GB` | None | float | sidecar: `mx.set_cache_limit`。MLX バッファ上限で swap 阻止 (99% ディスク環境で致命的な swap を回避) |
| `BONSAI_MLX_WIRED_LIMIT_GB` | None | float | sidecar: `mx.set_wired_limit` |
| `BONSAI_MLX_KV_BITS` | None | 4 or 8 | sidecar: KV cache 量子化。**実測: resident KV cache 0.926→0.267GB (-71%、kv4@6417tok)**。長文 sustained メモリの本命。8=保守/4=積極 |
| `BONSAI_MLX_QUANTIZED_KV_START` | 0 | int | sidecar: 先頭 N tok を fp16 保持し量子化変換トランジェント/精度劣化を緩和 |
| `BONSAI_MLX_KV_GROUP_SIZE` | 64 | int | sidecar: KV 量子化 group size |
| `BONSAI_MLX_MAX_KV_SIZE` | None | int | sidecar: 回転 KV 上限 (長文でのメモリ暴走防止) |
| `BONSAI_MLX_MODEL` / `BONSAI_MLX_PORT` | ternary 2bit / 8888 | str/int | sidecar: model id / port |

- **使い方**: `start-mlx-server.sh` (cubist) の代わりに `start-mlx-sidecar.sh` を起動するだけで bonsai は memory-optimized server を使う。
- **B-1 watchdog 併用**: `BONSAI_MLX_SPAWN_PROGRAM=<repo>/scripts/start-mlx-sidecar.sh` で idle respawn 対象を sidecar に。
- **注意 (codex)**: KV量子化は長文 recall / tool-call 安定性を劣化させ得る → 長文 paired smoke で確認必須 (短 smoke では見逃す)。`peak_gb` でなく長文 sustained の resident KV で評価する。
- 計測: `python scripts/mlx_server/measure_kv_memory.py --ctx-words N` で `/mem` の peak/cache を取得。

## 注意事項 (Phase 5 で「絶対に守るルール」化)

詳細は CLAUDE.md「注意事項」セクション参照 (Phase 5 で本 runbook に再配置候補)。

主要ルール:
- **Edit/Write 後の巻き戻し禁止** (error_recovery.rs / benchmark.rs / agent_loop.rs で clippy auto-fix 巻き戻し頻発)
- **Lab 稼働中の `cargo build --release` 禁止** (target/release/bonsai 置換で 10-cycle 一貫性破壊)
- 大量変更時は Python subprocess + 即 git commit で原子的に行う
- ureq v3 の HTTPS → web_fetch は reqwest::blocking (native-tls) を使用
- llama-server の `--flash-attn` は値 `on` 必要 (`--flash-attn on`)

## 関連

- CLAUDE.md (Claude Code エントリ) ← 本 file の link source
- docs/INDEX.md (Z-1 Phase 1) ← ナビゲーション
- docs/quality/lab-history.md ← Lab 結果詳細
