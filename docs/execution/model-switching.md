# モデルを差し替える

bonsai-agent の既定モデルは **MiniCPM5-2B**（GGUF Q4_K_M、1.56GB）である。
モデル identity（repo id、gguf ファイル名、context 長、推論パラメータ既定値）は
Rust 側で `src/domain/model_profile.rs` の `ModelProfile` レジストリに、shell 側で
`scripts/model.env` に一元化してある。差し替えは以下の3手順で完結する。

## 手順1: config.toml (または CLI / env) を変える

もっとも簡単な差し替えは `config.toml` の `[model]` セクションを書き換えることである。

```toml
[model]
model_id = "minicpm5-1b"
context_length = 16384
```

`model_id` は既知プロファイルの `id` / `gguf_repo` / `mlx_repo` / gguf ファイル名
（拡張子を除く）のいずれかに大文字小文字を無視して一致させれば、
`find_profile()` がそのプロファイルを解決する。未知の文字列を渡した場合は
自由入力のモデルIDとしてそのまま扱われる（`find_profile()` は `None` を返す）。

一時的な切替は `--model` CLI 引数、または `BONSAI_MODEL` / `BONSAI_MODEL_ID` 環境変数でも
上書きできる（`BONSAI_MODEL_ID` は `BONSAI_MODEL` の alias。両方設定した場合は `BONSAI_MODEL` が勝つ）。
shell 側の `scripts/model.env`（手順2）ではこの優先順位が逆で、`BONSAI_MODEL_ID` を優先する。
`BONSAI_MODEL_ID` が未設定のときに `BONSAI_MODEL` をフォールバックとして採用するのは、値が
既知 preset id と一致する場合に限られ、一致しない自由文字列（HF repo id 等）は無視して
警告を出す。両方を設定していて値が食い違う場合も、Rust 側と shell 側で別モデルになる旨の
警告が出る。
優先順位は `resolve_model_id()`（`src/domain/model_profile.rs`）に実装されており、
`--model` > `BONSAI_MODEL` > `BONSAI_MODEL_ID` > `UNSLOTH_MODEL`（後方互換）>
config の `model_id` の順で解決する。
Unsloth backend かつ `model_id` が**旧既定** `bonsai-8b` のままの場合に限り、既存挙動を保つため
`Aratako/Qwen3-8B-ERP-v0.1-GGUF` に置き換わる。現行既定の `minicpm5-2b` はこの特例の対象外であり、
Unsloth backend でもそのまま `minicpm5-2b` が渡る。

`BONSAI_LAB_MLX_ONLY=1`（Lab 実行時限定、`--lab` 経由）は primary backend を強制的に MLX に
切替え、model_id も MLX 対応 repo に置換する。この置換先解決は `mlx_only_model_id()`
（`src/domain/model_profile.rs`）が担うが、`model_id` が既知 profile に一致していても
`mlx_repo` が空 (legacy profile、例 `bonsai-8b`) の場合は**エラーで停止する**（#14-2）。
以前は既定 profile 側の `mlx_repo` へ黙って置換していたが、operator が意図しないモデル
（config.toml で指定した profile とは無関係な既定 profile）に混成してしまうため、
`apply_lab_overrides()` が `anyhow::Error` に変換して `?` で即座にエラー終了するよう変更した。
`model_id` が既知 profile に一致しない場合（`find_profile()` が `None` を返す場合）も同様に
エラーで停止する。独自ホスティングの MLX repo をそのまま渡したい場合は、
`BONSAI_LAB_MLX_ALLOW_UNKNOWN=1` を明示的に付けない限り通らない（#17）。
`BONSAI_LAB_MLX_ONLY=1` を使う場合は `BONSAI_MODEL_ID=minicpm5-2b` 等、`mlx_repo` を持つ
profile を明示する必要がある。

Unsloth backend で `model_id = "bonsai-8b"` のまま `BONSAI_LAB_MLX_ONLY=1` を付けると、
`resolve_model_id()` が既に `Aratako/Qwen3-8B-ERP-v0.1-GGUF` へ置換した後の id を
`mlx_only_model_id()` が受け取り、これは既知 MLX profile に一致しないため
`BONSAI_LAB_MLX_ALLOW_UNKNOWN=1` を付けない限り拒否される。

profile 既定値の適用は `AppConfig::load()` 内でキー単位に行われる。つまり `config.toml` で
明示していないキー（`context_length` や `[model.inference]` の各値）だけが profile 値で埋まり、
明示したキーはそのまま維持される。`--init` が生成する config は全キーを明示的に書き出すため、
profile 既定は一切効かない。モデルを変える際に profile 側の値へ追従させたいキーがあれば、
該当キーを config.toml から削除するか、新しいモデルの値に手で書き換える必要がある。

`cargo run -- --diagnose` は、現在の `model_id` に `find_profile()` が一致した場合のみ
`profile: <display_name> (ctx native=…, default=…)` という 1 行を出す。登録済みプロファイルの
一覧そのものは `--diagnose` には現れない。全件を見るには `src/domain/model_profile.rs` の
`PROFILES` 配列、または後述の `scripts/model.env` の `case` 節を直接参照する。

具体例は `config.toml.example`（リポジトリルート）に、minicpm5-1b / ternary-bonsai-8b (mlx-lm) /
bonsai-8b (legacy) への切替ブロックとして併記してある。

## 手順2: scripts の実行時 env を変える

`scripts/start-server.sh` や `scripts/download_model.sh` などの shell スクリプトは
`scripts/model.env` を source して `BONSAI_MODEL_*` env を読む。config.toml とは別の
仕組みなので、モデルファイルの取得先やサーバー起動コマンドはこちらで制御する。

`scripts/model.env` は `BONSAI_MODEL_ID` の値で分岐する `case` 節を持ち、4件の preset
（`minicpm5-2b` / `minicpm5-1b` / `bonsai-8b` / `ternary-bonsai-8b`、いずれも
`src/domain/model_profile.rs` の `ModelProfile` と 1:1 対応）を切り替える。つまり
`BONSAI_MODEL_ID` を変えるだけで、GGUF/MLX repo、context 長、推論パラメータ、保存先ディレクトリが
まとめて切り替わる。個々の値はさらに env で上書きできる（`: "${VAR:=preset値}"` なので、
env が既に設定されていれば preset より優先される）。

| env | 既定値 (`minicpm5-2b`) | 意味 |
|---|---|---|
| `BONSAI_MODEL_ID` | `${BONSAI_MODEL:-minicpm5-2b}` | config.toml の `model_id` に対応する短縮名。`model.env` の preset 選択キー。未設定なら Rust 側の `BONSAI_MODEL` を読み、それも無ければ `minicpm5-2b` |
| `BONSAI_MODEL_GGUF_REPO` | `openbmb/MiniCPM5-2B-GGUF` | GGUF 配布元 Hugging Face repo |
| `BONSAI_MODEL_GGUF_FILE` | `MiniCPM5-2B-Q4_K_M.gguf` | ダウンロードする GGUF ファイル名 |
| `BONSAI_MODEL_MLX_REPO` | `openbmb/MiniCPM5-2B-MLX` | MLX 配布元 Hugging Face repo |
| `BONSAI_MODEL_CTX` | `16384` | `start-server.sh` が `-c` に渡す context 長。M2 16GB 向け保守的既定。32k 以上に増やす場合はこの env で opt-in する |
| `BONSAI_MODEL_DIR` | `$HOME/.cache/bonsai-agent/models` | GGUF ファイルの保存先ディレクトリ |
| `BONSAI_MODEL_TEMP` | `0.6` | `start-server.sh` が `--temp` に渡す推論温度 |
| `BONSAI_MODEL_TOP_P` | `0.95` | `start-server.sh` が `--top-p` に渡す値 |
| `BONSAI_MODEL_REPEAT_PENALTY` | `1.05` | `start-server.sh` が `--repeat-penalty` に渡す値 |
| `BONSAI_ENABLE_THINKING` | `false` | `true`/`false` のみ許可。`--chat-template-kwargs` の `enable_thinking` を制御する |
| `BONSAI_LLAMA_SERVER_BIN` | PATH の `llama-server`、無ければ `~/Bonsai-demo/bin/mac/llama-server` | 起動する llama-server バイナリ |
| `BONSAI_GGUF_PATH` | 未設定 | 設定時、`start-server.sh` は `BONSAI_MODEL_DIR`/`BONSAI_MODEL_GGUF_FILE` より優先してこのパスをそのまま使う |

thinking（`<think>` 出力）を有効にするかどうかは `BONSAI_ENABLE_THINKING` のみで制御する。
`ModelProfile`（Rust 側）に `enable_thinking` フィールドは存在しない。`true`/`false` 以外の値を
渡すと、`start-server.sh` はバイナリの存在確認やモデルファイルの存在確認より前にエラー終了する。

未知の `BONSAI_MODEL_ID` を渡した場合、`model.env` は標準エラー出力に警告を出したうえで
`BONSAI_MODEL_GGUF_REPO` / `BONSAI_MODEL_GGUF_FILE` / `BONSAI_MODEL_MLX_REPO` の3つだけを
空文字列にフォールバックする。`BONSAI_MODEL_CTX` / `BONSAI_MODEL_DIR` / `BONSAI_MODEL_TEMP` /
`BONSAI_MODEL_TOP_P` / `BONSAI_MODEL_REPEAT_PENALTY` は `minicpm5-2b` と同じ値で埋まる
（repo/file/mlx_repo だけが未知なので空にする、という設計であり、全部が空になるわけではない）。
`scripts/download_model.sh` と `scripts/start-mlx-server.sh` 系は repo が空のままだと動作できないため、
curl/mlx 起動より前に空 repo を検出して早期にエラー終了する。未知モデルを使う場合は
`BONSAI_MODEL_GGUF_REPO` / `BONSAI_MODEL_GGUF_FILE`（MLX を使うなら `BONSAI_MODEL_MLX_REPO` も）を
明示的に指定する必要がある。

個々の値は呼び出し時にさらに上書きできる。

```sh
BONSAI_MODEL_ID=minicpm5-1b \
BONSAI_MODEL_GGUF_REPO=openbmb/MiniCPM5-1B-GGUF \
BONSAI_MODEL_GGUF_FILE=MiniCPM5-1B-Q4_K_M.gguf \
./scripts/download_model.sh
```

legacy モデル (`bonsai-8b` / `ternary-bonsai-8b`) への切替は `BONSAI_MODEL_ID=bonsai-8b` /
`BONSAI_MODEL_ID=ternary-bonsai-8b` を指定するだけでよい。後方互換ラッパー
`scripts/download_ternary.sh`（`BONSAI_MODEL_ID=ternary-bonsai-8b` を export して
`download_model.sh` を呼ぶだけの薄いラッパー）も引き続き使える。

旧 `BONSAI_DEMO` env は廃止済みである。現在は `BONSAI_LLAMA_SERVER_BIN`（バイナリパス）と
`BONSAI_MODEL_DIR`（モデル保存先ディレクトリ）に置き換わっている。

## 手順3: 新モデルを恒久登録する

一時的な上書きではなく新モデルを常設の選択肢として加えるなら、
`src/domain/model_profile.rs` の `PROFILES` 配列に `ModelProfile` を1件追加し、
`scripts/model.env` の `case` 節にも同じ内容の preset を追加し、
`config.toml.example` にも切替ブロックの例を足す。`id` の重複はテスト
（`test_known_profiles_no_duplicate_ids`）で検出される。

`PROFILES` と `scripts/model.env` の食い違い（例: 片方だけ値を更新し忘れる）は
`tests/model_env_sync.rs` の drift テストが検出する。各 profile について
`gguf_repo` / `gguf_file` / `mlx_repo` / `context_length` / 推論パラメータを
`scripts/model.env` を実際に source した結果と突き合わせるほか、`model.env` の
`case` 節に定義されていて `PROFILES` に無い preset（逆方向の drift）も検出する。
新しい profile を追加したら、両方を同時に更新すること。

## 関連する設計判断

- [ADR-011](../decisions/ADR-011-chat-template-backend-source-of-truth.md) — chat template は backend tokenizer 側が source of truth であり、モデルを差し替えても bonsai 側の template 実装は変更不要
- [ADR-013](../decisions/ADR-013-model-profile-minicpm5.md) — 本レジストリ設計と MiniCPM5-2B を既定に選んだ経緯
- [ADR-003](../decisions/ADR-003-paired-evidence-over-unpaired.md) — 推論パラメータの初期推奨値は Lab paired evidence による検証を経ていない。unpaired 評価だけでチューニングを確定させない

## MiniCPM5-2B の事実（Hugging Face 一次情報、2026-09-08 確認）

| 項目 | 値 |
|---|---|
| HF repo (BF16) | `openbmb/MiniCPM5-2B` |
| GGUF repo | `openbmb/MiniCPM5-2B-GGUF`（`MiniCPM5-2B-Q4_K_M.gguf` 1.56GB / `MiniCPM5-2B-Q8_0.gguf` 2.68GB / `MiniCPM5-2B-F16.gguf` 5.0GB） |
| MLX repo | `openbmb/MiniCPM5-2B-MLX`（4bit、1.42GB） |
| アーキ | `LlamaForCausalLM` 標準（42 layers、GQA 16/2、head_dim 128）。素の llama.cpp / mlx-lm で動き、PrismML fork は不要 |
| native context | 131072 |
| generation_config | temperature 1.0 / top_p 0.95（chat 用の公式値。agent 用途は低温を推奨） |
| chat template | ChatML + `<think>`、`enable_thinking` kwarg。native tool call は `<function name=..><param ..>` の XML 形式だが、bonsai は system prompt 経由の `<tool_call>` JSON 方式を使うため当面関係しない |
| license | Apache-2.0 |

Mac M2 16GB 向けの既定値は GGUF = Q4_K_M（重み 1.56 GB）、context_length = 16384、
推論は temperature 0.6 / top_p 0.95 / top_k 20 / min_p 0.05 / max_tokens 2048 / repeat_penalty 1.05 とした。
KV cache は q8_0 量子化で ≈21.5 KB/token（42 layer × 2 KV head × 128 dim × (K+V) × 1 byte の概算）となり、
16k トークンで ≈0.35 GB、32k トークンで ≈0.7 GB を追加で消費する。32k 以上へ増やす場合は
`context_length`（config.toml）または `BONSAI_MODEL_CTX`（scripts）で明示的に opt-in する。
**これらは初期値であり、確定した推奨値ではない。** Lab paired evidence（ADR-003）による検証は後日行う。
