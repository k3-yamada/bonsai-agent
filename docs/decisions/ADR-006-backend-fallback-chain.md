# ADR-006: 推論バックエンド戦略 — MLX Ternary primary + llama-server fallback chain

## Status: Accepted (2026-05-31)

## Context

bonsai-agent は複数の推論バックエンドを fallback chain で運用する (項目 35/36/49/53/56/60/61/63/67/90/103/105/130/167/168/174/195/198)。
主要選択肢:
- **llama-server (1-bit gguf)**: 軽量・低 latency、Bonsai-8B 1.28GB の標準経路。
- **MLX (2-bit / Ternary)**: PrismML Ternary-Bonsai-8B-mlx-2bit、精度 +5pt だが latency が高い。

実機 finding (項目 249 G-RT): MLX 2-bit primary は llama-server 1-bit gguf より初トークン latency が **~2x** 高い。Lab v22 paired 5h 完走で SSE timeout が頻発 (F1 180s 延長でも catch しきれず)。即ち **Ternary +5pt の精度向上には latency という対価**がある。

過去 finding:
- 項目 35/36 等で fallback chain (primary → 代替 → 非ストリーミング fallback) を確立。
- MLX recvfrom kernel hang (Lab v13、19.5h で kill) → socket timeout + fallback chain 採用の契機。
- production config 実体パスは `~/Library/Application Support/` (fallback_chain_mlx_finding)。

## Decision

**用途別にバックエンドを使い分け、fallback chain で堅牢性を担保する。**

1. **Lab 精度評価 (paired re-eval 等)**: MLX Ternary primary (`BONSAI_LAB_MLX_ONLY=1`) で +5pt 精度を活かす。latency 対策に SSE timeout 延長 (`BONSAI_LAB_LONG_SSE=1`) + pre-warm (項目 252) を併用。
2. **通常運用 / 低 latency 要求**: llama-server 1-bit gguf を primary。
3. **fallback chain**: primary 失敗時は代替バックエンド → 非ストリーミング fallback の順で graceful degradation。socket timeout で kernel hang を回避。
4. **MLX latency は構造的対価として受容**: Lab cycle 完走の prerequisite として pre-warm / timeout bound (項目 252 M2) で緩和するが、根本的な 2x は許容。

## Consequences

**Positive**:
- 精度 (MLX +5pt) と速度 (llama-server) を用途で使い分け、Lab evidence の質を確保。
- fallback chain で単一バックエンド障害 (MLX hang 等) に対する堅牢性。

**Negative / Trade-off**:
- MLX primary の Lab cycle は wall time が長い (~77 min/cycle、paired 5 pairs で ~13h)。ADR-003 の paired 必須と相まって評価コスト増。
- MLX server は user 環境で手動起動が必要 (CI 自動化困難)。
- 複数バックエンド維持の複雑性 (config 実体パス分散等)。

## Related

- ADR-003 (Paired evidence — MLX primary での paired re-eval が前提)
- CLAUDE.md Backend / Inference カテゴリ (項目 35/36/.../198)、項目 249 (latency finding)、項目 252 (pre-warm)
- memory: ternary_bonsai.md / ternary_bonsai_paths_2026_05_19.md / fallback_chain_mlx_finding.md / lab_v13_result.md
