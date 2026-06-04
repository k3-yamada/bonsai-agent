# M2 16GB メモリ最適化 着手計画

作成: 2026-06-05 / 前提: **bonsai 2-bit モデルをメインに使用** / 制約: **クリーンアーキテクチャ厳守**

## 背景 (調査済 → memory `memory_optimization_2026_06_05.md`)

- MLX 未起動でも空きRAM 126MB / compressor 7.27GB。圧迫源 = 背景アプリ + KV cache + MLX バッファ。
- weight は 2-bit (2.2GB) で**頭打ち** → メモリ削減は重み以外に向ける。
- engine は `QuantizedKVCache` / DWQ / mem制御API 対応済だが、稼働中 cubist `mlx-openai-server` が未配線。

## クリーンアーキテクチャ準拠の原則 (全 Phase 共通・厳守)

層順序: **`domain` (純粋+port) → `runtime` (I/O/MLX/HTTP/process) → `agent` → `config`/`main`**。
- MLX 自前サーバ (Phase 2) は **HTTP 境界の外側 (Python sidecar)**。bonsai 側は既存 `domain::llm::LlmBackend` port 経由の純粋 consumer に留め、**domain を一切変更しない**。
- KV量子化等のパラメータは HTTP リクエスト体で送るため `runtime` 層の backend 実装に閉じる。`domain` の trait は不変 (必要なら既存 `InferenceParams` を runtime 内で拡張)。
- 新規 env getter は `config` 層。スクリプト/モデル変換は `scripts/` (層外)。
- **各 Phase 完了ゲート**: `cargo test --test structural` (層違反0) + `cargo clippy --lib` + `cargo fmt --check`。`WHITELIST_DEP` 追加で回避しない。

---

## Phase 0 — 運用ルール化 (即時・コード変更なし・リスク0)

目的: 最大の即効レバー = 背景RAM解放。
- [ ] 実機検証/Lab 稼働時は Chrome 等の大型アプリを閉じる運用を runbook に明記。
- [ ] `scripts/` に `mem_probe.sh` (= `top -l1` PhysMem + MLX `get_active_memory` 印字) を追加し before/after 計測の基準化。
- 完了条件: runbook 追記 + probe で MLX 起動前後の RSS 差分を 1 回記録。
- 層影響: なし (scripts/docs のみ)。

## Phase 1 — DWQ 再量子化 (品質↑・メモリ不変) → **off-box 専用・後回し** [調査更新 2026-06-05]

**重要な調査結論 (omc/gemini/codex/ecc 合議)**: DWQ は **ローカル実行不可**、かつ **品質改善であってメモリ対策ではない** (成果物は同 2.2GB)。
- ローカル不可の3理由: 教師 bf16 ~15GB + 生徒同時で >16GB / 教師未キャッシュ (unpacked は空 stub) / 空きディスク 4.7GB で ~16GB DL 不可。
- off-box も非自明: **≥32GB Apple Silicon Mac (Metal) 必須** (CUDA は量子化 matmul WIP で不可、mlx #3122/#2536)、**教師は PrismML の bf16 (同一モデル) 必須** (上流 Qwen3-8B は mismatch)。
- → **手順は `scripts/dwq_offbox_runbook.md` に分離**。実行前ブロッカー = `prism-ml/Ternary-Bonsai-8B-unpacked` の bf16 が HF 公開されているか確認。
- **着手判定: Phase 2 安定後、品質不足が残った場合のみ A/B 実験。メモリ最適化の本命ではない。**

## Phase 2 — 案B 自前 MLX サーバ (メモリ本丸・keystone)

目的: KV量子化 + max_kv_size + `mx.set_cache_limit` + context拡張(64k) を一括解禁。
**クリーンアーキテクチャ: sidecar は HTTP 境界の外。bonsai 側変更は runtime/config に限定、domain 不変。**

- 2a. **最小サーバで現状再現** (Python sidecar, `scripts/mlx_server/`):
  - [ ] FastAPI `/v1/models` + `/v1/chat/completions` (SSE stream) を `mlx_lm.stream_generate` + chat template で実装。
  - [ ] cubist 版と差替え、既存 smoke が通ることを確認 (機能等価)。
- 2b. **メモリ最適化を配線** (sidecar 内、bonsai コード不変):
  - [ ] `stream_generate(..., kv_bits=8(K)/4(V) 相当, quantized_kv_start=N)` + `max_kv_size`。
  - [ ] 起動時 `mx.set_cache_limit()` / 必要に応じ `set_wired_limit()`。
  - [ ] env で ON/OFF (`BONSAI_MLX_KV_BITS` 等)、既定は安全側。
- 2c. **bonsai 側 (Rust)**:
  - [ ] `server_url` / 起動方針の `config` 更新のみ。新 env getter は `config` 層。
  - [ ] HTTP backend (runtime::inference) は既存のまま (リクエスト体は不変、サーバ側で量子化)。**domain::llm 不変**。
- 2d. **検証**:
  - [ ] `scripts/mem_probe.sh` で KV量子化 ON/OFF の peak RSS 差分計測 (V4bit で KV ~1/4 を実証)。
  - [ ] **Lab smoke paired** で品質非劣化を確認 (2-bit + KV量子化の累積誤差 = H-A3 懸念、`quantized_kv_start` で先頭 fp16 保持して緩和)。
  - [ ] `cargo test --test structural` で層違反0 を確認。
- 完了条件: peak RSS 削減 + smoke 品質非劣化 + 構造テスト緑。
- リスク: sidecar 保守コスト / 量子化精度劣化 → paired evidence で gate。

## Phase 3 — 先鋭 KV圧縮 (任意・将来)

- [ ] TurboQuant-MLX / VeloxQuant-MLX (Metal カーネル 4.6〜16x) の評価。Phase 2 の sidecar に差込む形で実験。
- [ ] mlx-lm 上流の PolarQuant (ICLR 2026, issue #1060) 取込状況を watch。
- 完了条件: Phase 2 で不足の場合のみ着手。

---

## 優先順位と判断 [調査更新 2026-06-05: omc/gemini/codex/ecc 合議で再順序化]

**合議結論**: 慢性メモリ圧の支配項は **重みでなく KV cache + MLX 実行時 cache + macOS compressor**。
よって **Phase 2 (実行時メモリ) が ROI 最高**。DWQ (Phase 1) は同 2.2GB でメモリを解かない品質専用 → 後回し。

1. **Phase 2-2c 先行: `mx.set_cache_limit` (+ `max_kv_size`)** — 99% ディスク環境で致命的な swap を阻止する最安・最即効の本命。codex 推奨「メモリ上限を先に固定」。
2. **Phase 2-2a/2b: sidecar 化 → KV量子化 K8 → V4 の順**。K8 (品質劣化小) を先、V4 は**長文 recall / JSON・tool-call 安定性 / 自己修正**の劣化を paired で見て採否 (短 smoke では見逃すので長文タスク必須、codex 指摘)。
3. **Phase 1 (DWQ off-box)** は Phase 2 安定後、品質不足が残れば A/B 実験 (`scripts/dwq_offbox_runbook.md`)。
4. **Phase 3 (TurboQuant 等)** は標準 KV量子化で不足時のみ (mlx-lm エコシステム外の保守コスト、gemini 指摘)。

**落とし穴 (codex)**: `max_kv_size` 過小→文脈欠落 / `set_cache_limit` 過小→再計算 latency 悪化 / sidecar に prompt整形・tool policy を持たせ過ぎると chat template の source-of-truth 分裂 (sidecar は推論実行 + メモリ制御に限定 = クリーンアーキ境界厳守)。

関連: [[mlx_context_expansion_2026_06_05]] (案B は context 拡張と同一 keystone) / [[memory_optimization_2026_06_05]] / [[feedback_clean_architecture]]。
