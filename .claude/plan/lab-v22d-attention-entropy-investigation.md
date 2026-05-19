# Lab v22d Attention Entropy Investigation — 間接効果検証 (項目 247d 候補)

**状態**: planning-only (2026-05-19 起票、研究 spike 候補)
**推奨度**: ☆ (実装難易度高、future research、優先度低)
**推定工数**: ~4-6h research spike + ~10h impl (実機制約次第)
**起点**:
- Gemini 提案 (CCG synthesis): 「Context KV-Cache Denoising 計測 — factcheck 前後の Context における平均 Attention Entropy を比較。ON の方が Entropy が低ければ、推論が安定している証拠」
- Lab v21 smoke で副次 score Δ=+0.0227 (p=0.2429 非有意) が ON 有利方向、間接効果仮説の検証経路

---

## §1. 仮説と検証目的

### 1.1 仮説 (Gemini 提案)
1bit Bonsai-8B は重みの表現力が低いため、Context 内のノイズ (不正確な事実) に敏感。
factcheck が不確実な triple を除去 → Attention Mechanism が正しい Token に集中 →
推論安定 → score 微改善。

→ Lab v21 smoke の Δ=+0.0227 (3/5 cycle で ON 有利) はこの間接効果の signal の可能性。

### 1.2 検証アプローチ
factcheck pass 前後で **生成中の attention entropy** を測定し、ON の方が低ければ仮説支持。

---

## §2. 設計 (案 A: 限定的検証、案 B: full impl 棄却)

### 案 A (推奨、研究 spike): 既存 API 範囲での近似計測
- mlx-openai-server / llama-server の **`logprobs` API** で各 token 生成時の top_k log-prob を取得
- 近似 entropy H ≈ -Σ p_i log p_i over top-k tokens
- factcheck ON cycle と OFF cycle の同 prompt で H を比較

**Pros**:
- 既存 API のみで実装可、PrismML fork modify 不要
- entropy proxy として top-k 範囲で十分

**Cons**:
- 真の attention entropy (各層の attention weights) は取れない
- log-prob entropy は output distribution の entropy で、attention とは別物 (相関はあるが equivalence は弱い)

### 案 B (棄却): 真の attention entropy
- llama.cpp / mlx 内部 attention weights を hook で抽出
- PrismML fork modify 必須、binary build 工程増、実装 1 週間 +
- 効果不確実な研究 spike にコスト過大

---

## §3. 実装 (Phase 1-3、案 A、研究 spike)

### Phase 1 (Red) — 2 failing test
1. `t_logprobs_entropy_computes_correctly`: 既知 distribution で entropy 一致 (unit test)
2. `t_lab_logprobs_capture_writes_audit_log`: cycle 中 logprobs collect され audit に記録

### Phase 2 (Green)
- `src/runtime/llama_server.rs::LlamaServerBackend::generate_with_logprobs` (top_k=10 等)
- `src/agent/experiment.rs` に Phase 「post-cycle entropy aggregation」追加 (env-gated `BONSAI_LAB_LOGPROBS_ENABLED=1`)
- `scripts/lab_v22_entropy_analyzer.py` で paired Δentropy 計算

### Phase 3 (Refactor)
- env getter SSOT、log prefix `[INFO][lab.entropy]`、clippy/fmt

---

## §4. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | logprobs 取得で API 仕様が backend ごとに違う | mlx-openai-server / llama-server 両方の OpenAI compat API は logprobs 対応 (検証要) |
| R2 | entropy 差が背景 noise (cycle 間 random variation) より小さい | A/A test の Δentropy で background floor 測定後比較 |
| R3 | attention entropy ≠ logprob entropy、仮説検証として弱い | research spike として位置づけ、結果が positive なら案 B 真 attention entropy へ |
| R4 | 1bit Bonsai-8B の logprobs は coarse-grained (確率 distribution が steep) | top_k=20+ で範囲拡大、または softmax temperature low で sharp distribution として解釈 |

---

## §5. 期待効果
- Lab v21 smoke の Δ=+0.0227 が「Context Denoising」由来か、ノイズ偶然かを切り分け
- positive な場合: factcheck の真効力 = direct (conflict 検出) + indirect (context noise reduction) と確証
- negative な場合: 間接効果仮説 reject、direct 効果のみが factcheck の価値

---

## §6. 依存 / 並行性

### 完遂前提
- 項目 247 Phase A-D 完遂 (Lab v22 で Δscore=+0.0227 signal が再現するか確証してから本 plan)
- 項目 247c FActScore plan 完遂後だと relatively orthogonal な軸として比較しやすい

### 並行可
- 項目 247b/c (task / metric 軸) と独立、本 plan は推論経路軸

---

## §7. metadata
- 起点: CCG Gemini 「Context KV-Cache Denoising 計測」
- 関連 plan: `lab-v22-metric-redesign.md` (項目 247)、`lab-v22c-factscore-brier-metric.md` (247c)
- 想定 commit 範囲: 3-4 commit (logprobs capture + entropy compute + smoke G-VV)
- 想定 line 範囲: +200 行 / -5 行 (llama_server.rs + experiment.rs + analyzer)
- **note**: 本 plan は研究 spike 性質強、Phase 4 で結果 negative ならソフト close、positive なら項目 248+ で本格化
