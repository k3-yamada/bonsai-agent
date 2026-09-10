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
# MLX server 起動 (port 8000、既定は openbmb/MiniCPM5-2B-MLX)。
# Ternary Bonsai-8B での再現は `BONSAI_MODEL_ID=ternary-bonsai-8b ./scripts/start-mlx-server.sh` +
# config `model_id = "ternary-bonsai-8b"` を使う。
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
# MLX server 起動 (port 8000、既定は openbmb/MiniCPM5-2B-MLX)。
# Ternary Bonsai-8B での再現は `BONSAI_MODEL_ID=ternary-bonsai-8b ./scripts/start-mlx-server.sh` +
# config `model_id = "ternary-bonsai-8b"` を使う。
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
| `BONSAI_LAB_MLX_ONLY` | OFF | bool | primary backend を MLX 切替 (項目 249 F2)。既知 profile で MLX ビルドなし (例 `bonsai-8b`) → エラー、未知 model_id (Unsloth 置換後の `Aratako/...` や独自 repo) → エラー、`BONSAI_LAB_MLX_ALLOW_UNKNOWN=1` で opt-in 許容 (#14-2, #17) |
| `BONSAI_LAB_MLX_ALLOW_UNKNOWN` | OFF | bool | `BONSAI_LAB_MLX_ONLY=1` で未知 model_id (独自 MLX repo) を許容。未設定なら既知 profile 以外は起動時エラー (#17) |
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
| `BONSAI_EMBED_URL` | None | url | `HttpEmbedder` 有効化。設定時 `create_embedder()` が `{url}/v1/embeddings` (OpenAI 互換) 経由でローカル埋め込みを取得 (MLX sidecar / Ruri sidecar 等)。**`embeddings` feature 非依存** = ort バイナリDLなしのオフライン/Linux ビルドでも実埋め込み。例: MLX `http://localhost:8888`、Ruri `http://127.0.0.1:8787`。未設定で従来挙動 (fastembed→SimpleEmbedder) |
| `BONSAI_EMBED_MODEL` | `bonsai-embed` | str | `HttpEmbedder` が送る model 名。Ruri 利用時は `cl-nagoya/ruri-v3-30m`（ADR-015）。リモート失敗時は hash 埋め込みに graceful fallback (dim=256 維持) |
| `BONSAI_MODEL` | None | str | `resolve_model_id()` 経由の model_id override。優先順位は `--model` CLI > `BONSAI_MODEL` > `BONSAI_MODEL_ID` (fallback alias (BONSAI_MODEL 優先)) > `UNSLOTH_MODEL` (後方互換) > config の `model_id` (ADR-013)。Unsloth backend への旧既定 `bonsai-8b` 置換特例は現行既定 `minicpm5-2b` には適用されない |
| `BONSAI_TOOL_SPILL` | ON (未設定=有効) | bool | ツール出力スピルオーバー一時ファイル (`{TMPDIR}/bonsai-agent/spill-{pid}/`) の書き出し有効化。`0`/`false`/`no` で完全無効化 (ファイルを1つも作らない、Issue #22 B3-1) |
| `BONSAI_TOOL_SPILL_MAX_FILES` | 64 | 1..=4096 | spill ディレクトリ内の保持ファイル数上限。超過時は最古 (mtime昇順) から削除。範囲外/非数値は default に巻き戻し |
| `BONSAI_TOOL_SPILL_MAX_BYTES` | 67108864 (64MiB) | 1MiB..=4GiB | spill ディレクトリ内の合計バイト数上限。超過時は最古から削除。範囲外/非数値は default に巻き戻し |

`--init` は API key を config.toml に書き出さない（`ModelConfig.api_key` / `AdvisorSettings.api_key` は
`#[serde(skip_serializing)]`、ファイルは 0600 で作成、Issue #15）。`UNSLOTH_API_KEY` / `BONSAI_API_KEY` /
Advisor 用 `OPENAI_API_KEY` 等は env のまま管理し、config.toml には書かない。以前の `--init` で
`api_key = ...` が config.toml に平文で書かれている場合は削除して env に移すこと（deserialize は
従来どおり動作するため config.toml 側に残っていても読めてしまう点に注意）。既存の config.toml がある場合
`--init` はファイルに触れない (早期 return) ため、権限は自動修正されない。以前の `--init` で作成した 644
のファイルは `chmod 600` すること。

### モデル選択 (scripts、`scripts/model.env`)

Rust 側の env (`BONSAI_MODEL`) とは別に、shell スクリプト (`start-server.sh` 等) はこちらを読む。
`BONSAI_MODEL_ID` を変えると、`scripts/model.env` の `case` 節が repo/file/context/推論パラメータを
まとめて preset に切り替える（Rust 側 `ModelProfile` レジストリと 1:1 対応。drift は
`tests/model_env_sync.rs` が検出する）。`BONSAI_MODEL_ID` 未設定時は `BONSAI_MODEL` (Rust 側 env) を
フォールバック既定値として読む。個々の値は env で個別上書きもできる。
詳細は [docs/execution/model-switching.md](model-switching.md)。

| Env | Default (`minicpm5-2b`) | 効果 |
|---|---|---|
| `BONSAI_MODEL_ID` | `${BONSAI_MODEL:-minicpm5-2b}` | config.toml `model_id` に対応する短縮名。`model.env` の preset 選択キー |
| `BONSAI_MODEL_GGUF_REPO` | `openbmb/MiniCPM5-2B-GGUF` | GGUF 配布元 Hugging Face repo |
| `BONSAI_MODEL_GGUF_FILE` | `MiniCPM5-2B-Q4_K_M.gguf` | ダウンロードする GGUF ファイル名 |
| `BONSAI_MODEL_MLX_REPO` | `openbmb/MiniCPM5-2B-MLX` | MLX 配布元 Hugging Face repo |
| `BONSAI_MODEL_CTX` | `16384` | `start-server.sh` が `-c` に渡す context 長。M2 16GB 向け保守的既定。32k 以上は env で opt-in |
| `BONSAI_MODEL_DIR` | `$HOME/.cache/bonsai-agent/models` | GGUF ファイルの保存先ディレクトリ |
| `BONSAI_MODEL_TEMP` | `0.6` | `start-server.sh` が `--temp` に渡す推論温度 |
| `BONSAI_MODEL_TOP_P` | `0.95` | `start-server.sh` が `--top-p` に渡す値 |
| `BONSAI_MODEL_REPEAT_PENALTY` | `1.05` | `start-server.sh` が `--repeat-penalty` に渡す値 |
| `BONSAI_ENABLE_THINKING` | `false` | `true`/`false` のみ許可 (他の値は `start-server.sh` がエラー exit)。`--chat-template-kwargs` の `enable_thinking` を制御する shell 側の唯一の切替点 |
| `BONSAI_LLAMA_SERVER_BIN` | PATH の `llama-server`、無ければ `~/Bonsai-demo/bin/mac/llama-server` | 起動する llama-server バイナリ |
| `BONSAI_GGUF_PATH` | None | 設定時、`start-server.sh` は `BONSAI_MODEL_DIR`/`BONSAI_MODEL_GGUF_FILE` より優先してこのパスを使う |

### Phase 2 メモリ最適化 sidecar (`scripts/start-mlx-sidecar.sh`)

cubist `mlx-openai-server` の drop-in 代替 (`scripts/mlx_server/server.py`)。OpenAI 互換 (`/v1/models` + `/v1/chat/completions` SSE + `/v1/embeddings`) のまま、MLX メモリ最適化を解禁する。**bonsai 側は `server_url` (port 8888) 経由の純粋 consumer で Rust 変更不要**。以下 env は sidecar 専用 (cubist は未対応)。

| env | default | 型 | 説明 |
| --- | --- | --- | --- |
| `BONSAI_MLX_CACHE_LIMIT_GB` | None | float | sidecar: `mx.set_cache_limit`。MLX バッファ上限で swap 阻止 (99% ディスク環境で致命的な swap を回避) |
| `BONSAI_MLX_WIRED_LIMIT_GB` | None | float | sidecar: `mx.set_wired_limit` |
| `BONSAI_MLX_KV_BITS` | None | 4 or 8 | sidecar: KV cache 量子化。**実測: resident KV cache 0.926→0.267GB (-71%、kv4@6417tok)**。長文 sustained メモリの本命。8=保守/4=積極 |
| `BONSAI_MLX_QUANTIZED_KV_START` | 0 | int | sidecar: 先頭 N tok を fp16 保持し量子化変換トランジェント/精度劣化を緩和 |
| `BONSAI_MLX_KV_GROUP_SIZE` | 64 | int | sidecar: KV 量子化 group size |
| `BONSAI_MLX_MAX_KV_SIZE` | None | int | sidecar: 回転 KV 上限 (長文でのメモリ暴走防止) |
| `BONSAI_MLX_MODEL` / `BONSAI_MLX_PORT` | ternary 2bit / 8888 | str/int | sidecar: model id / port |
| `BONSAI_MLX_EMBED_MODEL` | `mlx-community/all-MiniLM-L6-v2-4bit` | str | sidecar: `/v1/embeddings` 用埋め込みモデル。**初回リクエストまで lazy load** (chat 専用利用ではメモリ消費ゼロ)。`mlx_embeddings` 依存 (setup script で同 venv に導入) |

- **使い方**: `start-mlx-server.sh` (cubist) の代わりに `start-mlx-sidecar.sh` を起動するだけで bonsai は memory-optimized server を使う。
- **ローカル埋め込み (offline)**: sidecar 起動後、bonsai 側で `BONSAI_EMBED_URL=http://localhost:8888` を設定すると `/v1/embeddings` 経由で埋め込みを取得する。これにより `embeddings` feature (fastembed/ONNX、ort バイナリの build-time DL) なしで実埋め込みが使え、ビルド時 403 と実行時 HF DL の両方を回避できる。`mlx_embeddings` は `scripts/setup_mlx_ternary.sh` で venv に導入する。
- **Ruri v3 日本語埋め込み (ADR-015, 推奨・内部完結)**: チャット用 MLX とは別プロセス。仕様は [ruri-embed-sidecar.md](ruri-embed-sidecar.md)。起動例: `./scripts/start-ruri-embed.sh` のあと `BONSAI_EMBED_URL=http://127.0.0.1:8787` + `BONSAI_EMBED_MODEL=cl-nagoya/ruri-v3-30m`。`input_type`（`query`/`document`/…）の prefix 規約は sidecar 契約を正とする。
- **B-1 watchdog 併用**: `BONSAI_MLX_SPAWN_PROGRAM=<repo>/scripts/start-mlx-sidecar.sh` で idle respawn 対象を sidecar に。
- **注意 (codex)**: KV量子化は長文 recall / tool-call 安定性を劣化させ得る → 長文 paired smoke で確認必須 (短 smoke では見逃す)。`peak_gb` でなく長文 sustained の resident KV で評価する。
- 計測: `python scripts/mlx_server/measure_kv_memory.py --ctx-words N` で `/mem` の peak/cache を取得。

**推奨デフォルト構成 (2026-06-05 確定、低リスク・即採用可)**:
```sh
BONSAI_MLX_CACHE_LIMIT_GB=12      # swap 阻止 (最優先)
BONSAI_MLX_KV_BITS=8             # KV resident -46% (8bit はほぼ可逆、品質リスク小)
BONSAI_MLX_QUANTIZED_KV_START=256 # 先頭 256 tok は fp16
BONSAI_MLX_MAX_KV_SIZE=16384     # 長文 KV 上限
```
実測 (resident KV @6417tok): fp16 0.926GB → **kv8 0.496GB (-46%)** → kv4 0.267GB (-71%)。
**kv4 (-71%) は品質 paired smoke で非劣化確認後に採用** (2bit×4bit KV は累積誤差 H-A3 懸念)。
kv4 検証は長文評価セット構築 + 一晩 paired (~10-17h) が必要なため、メモリが kv8 で不足した時に着手 (deferred)。
E2E 実証済: bonsai `--exec` が kv8 構成で正答 (config は既に `backend=mlx-lm`/`server_url=8888`)。

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
