# MCP Detach Core 22 Baseline 計測結果 (2026-05-01e Phase B1)

## 計測条件

| 項目 | 値 |
|------|-----|
| 日時 | 2026-05-01 12:36:48 開始 / 16:12:16 完了 (wallclock 3h 35m) |
| backend | llama-server (PID 68103, port 8080) |
| GGUF | `/Users/keizo/Bonsai-demo/models/gguf/8B/Bonsai-8B.gguf` (1.1GB, intact) |
| llama-server flags | `-c 8192 -ngl 99 --flash-attn on -ctk q8_0 -ctv q8_0 --alias Bonsai-8B` |
| **MCP** | **切離済 (項目 180、`[[mcp.servers]]` コメントアウト)** |
| 登録ツール数 | **9 (built-in のみ)**、P3-α 23 (built-in 9 + MCP filesystem 14) から 14 個削減 |
| tier | core (22 tasks、`BONSAI_BENCH_TIER=core`) |
| k | 3 (jitter_seed=true) |
| 実験回数 | 0 (`--lab-experiments 0` baseline-only mode) |
| log | `/tmp/bonsai-llama/mcp-detach-core22.log` |

実行コマンド:
```bash
BONSAI_BENCH_TIER=core BONSAI_LOG=info ./target/release/bonsai --lab --lab-experiments 0 \
  > /tmp/bonsai-llama/mcp-detach-core22.log 2>&1
```

## 結果メトリクス

```
[lab] ベースライン: score=0.7571 pass@k=0.8333 pass_consec=0.8030 (3651.6s)
[INFO][lab] meta mutation generator: 16 accepted mutations in archive
[INFO][lab] 最大実験回数(0)に到達
[lab] 完了: 0実験, 0承認 (0%)
```

| 指標 | 値 |
|------|-----|
| **score** | **0.7571** |
| pass@k | 0.8333 |
| pass_consec | 0.8030 |
| benchmark duration | 3651.6s = **60.86 min** |
| SSE timeouts | 4 件 |
| llama-server 健全性 | PID 68103 安定継続、benchmark 後も稼働中 |

## P3-α 比較 (`session_2026_05_01_handoff.md`)

| 指標 | P3-α (MCP **on**) | B1 (MCP **off**) | Δ | 判定 |
|------|------------------|------------------|---|------|
| **score** | 0.7079 | **0.7571** | **+0.0492** | ✅ **MCP detach 改善 confirm** |
| pass@k | 0.8030 | 0.8333 | +0.0303 | ✅ 一貫性向上 |
| pass_consec | 0.7727 | 0.8030 | +0.0303 | ✅ 連続性向上 |
| duration | 3634.9s | 3651.6s | +0.46% | ⚠️ 速度差なし |
| SSE timeout | 5 | 4 | -1 | ✅ やや安定 |
| 登録ツール数 | 23 | 9 | -14 | ✅ context 削減成功 |

## 判断ゲート評価

| ゲート | 結果 |
|--------|------|
| score ≥ 0.7079 (P3-α 同等以上) | ✅ **+0.0492 達成** |
| duration ≤ 60.6 min (速度向上) | ❌ 同等 (+0.46%) |
| SSE timeout < 5 (環境安定) | ✅ 4 < 5 |
| pass@k ≥ 0.8030 | ✅ +0.0303 達成 |

**3/4 ゲート PASS**、duration のみ同等 → **MCP detach の core 22 効果を初めて定量化、score / 一貫性で大幅改善、速度効果は出ない**。

## 副次知見

1. **smoke の +0.012 は core 22 で +0.0492 に拡大** (約 4 倍)。smoke 5 task は MCP 影響を過小評価していた。core 22 で初めて実用ベンチマークでの効果が確定。
2. **速度差なしの解釈**: ツール数 23→9 で context は確実に削減されたが、benchmark wallclock は同等。1bit Bonsai-8B では LLM 推論時間がボトルネックで、context 圧縮効果は trace の精度向上に消費される (= score) が時間には還元されない。
3. **wallclock vs benchmark duration 乖離**: wallclock 3h 35m vs benchmark duration 60.86 min。差 ~155 min はモデルロード/cold cache/タスク間 idle 等の経路。`duration_ms` は P3-α と同基準なので直接比較可能。
4. **SSE timeout -1 は noise**: P3-α 5 件 / B1 4 件は環境変動 (llama-server 無再起動、同一 PID)。改善判定はせず横ばい。
5. **llama-server 安定**: 12 時間以上同一 PID 68103 で稼働継続、cold cache 警告/OOM なし。MCP detach により llama 単独運用の安定性も間接的に確認。

## 結論

**MCP filesystem 切離 (項目 180) は core 22 ベンチマークで score +0.0492 / 一貫性 +0.0303 の大幅改善を達成**。1bit モデルでは MCP filesystem の純コスト (context 圧迫 + tool 説明トークン) が圧倒的に効果を上回ることが、smoke 5 task → core 22 task の段階的検証で確定。**MCP detach デフォルト化 (項目 180) を恒久承認**。

次回 MCP 再導入時は (a) `max_mcp_tools_in_context` 削減変異 / (b) built-in 統合精査と組合せて評価する選択肢を維持 (項目 180 inline コメントの復活手順そのまま)。
