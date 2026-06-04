# Phase 1: DWQ 再量子化 off-box 実行 runbook

作成 2026-06-05。**ローカル M2 16GB では DWQ 実行不可**（教師 bf16 ~15GB + 生徒同時ロードで >16GB、教師未キャッシュ、空きディスク 4.7GB）。
本 runbook は **別マシン (off-box) で 1 回 DWQ を焼き、成果物 (~2.2GB) を持ち帰る**手順。根拠は memory `memory_optimization_2026_06_05.md` + omc/gemini/codex/ecc(exa) 合議 (2026-06-05)。

> **位置づけ (合議結果)**: DWQ は **品質改善候補であってメモリ対策ではない** (成果物は同 2.2GB、慢性メモリ圧を解かない)。
> メモリ最適化の本命は Phase 2 (KV量子化 + `mx.set_cache_limit` + `max_kv_size`)。**本 Phase は Phase 2 安定後の A/B 実験として温存**。

## 重大な前提 (調査で確定)

1. **実行環境は ≥32GB の Apple Silicon Mac (Metal)。CUDA クラウド GPU は不可。**
   - 理由: DWQ の生徒は量子化モデル。mlx の CUDA 量子化 matmul (QMM) は WIP で seq>1 失敗 / 出力 garbled (mlx issue #3122 / #2536、整備 PR #3137-3236 は 2026 春)。Metal は成熟。
   - 候補: 手持ち/借用の M3/M4 32-64GB Mac、AWS EC2 `mac2-m2pro.metal` (M2 Pro 32GB) 等。
2. **教師は「同一 bonsai モデルの高精度版」必須。上流 `Qwen/Qwen3-8B` は使わない。**
   - 理由: bonsai は PrismML が ternary 学習した派生。上流 Qwen3-8B を教師にすると **bonsai 固有の特性を失い base Qwen3 へ引き戻る** (teacher/student mismatch)。
   - **要確認 (実行前ブロッカー)**: `prism-ml/Ternary-Bonsai-8B-unpacked` (= ローカルの空 stub の HF 本体) が bf16/fp16 重みを公開しているか。`huggingface-cli download prism-ml/Ternary-Bonsai-8B-unpacked` で確認。無ければ本 Phase は実施不可 (PrismML に bf16 公開要請)。
3. **メモリ削減**: docs LEARNED_QUANTS「8-bit 教師は 16-bit 同等品質」→ 教師 8-bit 化で ~8GB に半減 (24GB Mac でも余裕)。

## 手順 (≥32GB Mac 上)

```bash
# 0. venv (Apple Silicon Mac)
python3 -m venv ~/.venvs/dwq && source ~/.venvs/dwq/bin/activate
pip install -U mlx-lm

# 1. 教師の確認・取得 (bf16)
huggingface-cli download prism-ml/Ternary-Bonsai-8B-unpacked --local-dir ./teacher-bf16
#    (任意) メモリ削減: 教師を 8-bit 化
# mlx_lm.convert --hf-path ./teacher-bf16 -q --q-bits 8 --q-group-size 64 --mlx-path ./teacher-8bit

# 2. DWQ 実行 (生徒 = 既存 2-bit、教師 = bf16 or 8bit)
mlx_lm.dwq \
  --model ./teacher-bf16 \
  --quantized-model prism-ml/Ternary-Bonsai-8B-mlx-2bit \
  --bits 2 --group-size 32 \
  --data-path mlx-community/qwen3_dwq_calibration_1332 \
  --num-samples 512 --max-seq-length 512 --batch-size 1 \
  --mlx-path ./bonsai-8b-2bit-dwq
#  - group-size 32: 2-bit は tunable param 倍増で品質改善 (docs 推奨)
#  - calibration: Qwen3 専用 DWQ データセット (tulu-3 + Qwen3 thinking traces)
#  - validation loss が下がらなければ警告 = 改善見込み無しのシグナル。2-bit は lr 高めが効く

# 3. 成果物確認 + 持ち帰り (~2.2GB)
du -sh ./bonsai-8b-2bit-dwq
#  本番機 (16GB) は空きディスク 4.7GB → 旧モデル削除後配置 or 外部ストレージ経由
```

## 持ち帰り後の検証 (本番 M2 16GB)

- [ ] **format 確認**: DWQ `--bits 2` 出力は**標準 MLX 2-bit affine** (PrismML ternary でない)。
      → stock mlx-lm で読めれば **PrismML フォーク依存が外れる**可能性 (運用簡素化)。逆に ternary 専用カーネルの速度/特性は失う可能性 → 速度・品質を実機計測。
- [ ] **Lab smoke paired (ADR-003)** で DWQ 版 vs 現 ternary 2-bit を score 比較。**非劣化が確認できて初めて差替**。
- [ ] config の `model_id` を新成果物へ切替 ([Lab 稼働中の release ビルド禁止] 厳守)。

## 判定

off-box DWQ は「≥32GB Mac + PrismML bf16 教師の存在」が揃って初めて成立する**非自明・品質専用タスク**。メモリは解かない。
**順序: Phase 2 (実行時メモリ) を先に安定させ、品質不足が残った場合のみ本 Phase を A/B 実験として実施。**

関連: `.claude/plan/memory-optimization-m2-16gb.md` / memory `memory_optimization_2026_06_05.md`。
