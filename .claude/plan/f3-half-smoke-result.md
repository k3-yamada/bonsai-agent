# F3 RequestSizeGuard threshold 半減 smoke 結果 (項目 193)

> 実施日: 2026-05-06d  
> 前提: handoff 05-06c (項目 192 extended tier 検証で fire=0)、handoff 05-06b (項目 191 core 22 で fire=0)  
> 採否方針: 「**知見は残しつつ F3 削除 (案 A)**」(user 判断)

## TL;DR

`f3_max_message_tokens=2048` (4915→2048 半減) で smoke 5 task k=3 (10 mutations) 実測 → **F3 fire=0、HTTP 400=0、score 0.7253**。**threshold を半減しても fire は依然ゼロ**で、**現行 workload の単発 message tokens は 2048 すら下回ること**が確定。**項目 116 Layer 1 (`max_tool_output_chars=4000` ≈ 1333 tokens) の支配が極めて強固**。F3 dead-code 判定の決定打となり、**削除 (案 A) を採用**。

## 実験条件

| 項目 | 値 |
|------|-----|
| backend | llama-server :8080 単独 (fallback_chain 一時 comment-out) |
| `f3_max_message_tokens` | **2048** (4915→2048 半減) |
| `max_tool_output_chars` | 4000 (項目 116 Layer 1、不変) |
| `BONSAI_LAB_SMOKE` | 1 (5 task / experiment) |
| k | 3 (`MultiRunConfig`) |
| `--lab-experiments` | デフォルト (10 mutations) |
| MCP | detach (項目 180) |

config 編集:
- `[model] f3_max_message_tokens = 2048` 追加
- `[fallback_chain]` ブロック (4 entries) コメントアウト

backup: `config.toml.pre-f3-half-2026-05-06` (canonical SHA `e217687e1cc2d690...`)

## 結果

| 指標 | 本回 (f3=2048) | 項目 192 (f3=4915, extended) | 項目 191 (f3=4915, core 22) | 項目 188 (B1b llama smoke, no f3) |
|------|--------|---------|---------|--------|
| baseline score | **0.7253** | 0.6301 | 0.7849 | 0.7440 |
| pass@k | 0.8000 | 0.7222 | 0.8636 | 0.6667 |
| pass_consec | 0.8000 | 0.6852 | 0.8485 | 0.6667 |
| baseline duration | 607.9s (10.1 min) | n/a (extended 18 task) | 2532s (42.2 min) | 877.5s (14.6 min) |
| **F3 fire** | **0** | 0 | 0 | n/a |
| HTTP 400 | 0 | 0 | 0 | 11 |
| F2 fire | 0 | 0 | 0 | 0 |
| Abort | 0 | 0 | 0 | 0 |
| fallback events | 0 | 0 | 0 | 0 |
| ACCEPT | 0/10 (0%) | 0 | 0 | 0/5 |

## Decision Gate

| Gate | 条件 | 結果 |
|------|------|------|
| ① F3 fire >= 5 | 0 | ❌ **FAIL** |
| ② score >= 0.74 (前 smoke 0.7440 比 -0.02 許容) | 0.7253 | ✅ PASS (+0.0013 marginal) |
| ③ HTTP 400 < 3 | 0 | ✅ PASS |

**2/3 PASS / 1 FAIL** — 項目 191/192 と完全同パターン。

## 仮説判定

| 仮説 | 結果 | 根拠 |
|------|------|------|
| H1: threshold 半減で F3 が fire | **REJECT** | fire=0、現行 workload 単発 message が 2048 tokens 未満 |
| H2: F3 副作用ゼロ (退行なし) | **CONFIRM** | score 0.7253 は llama smoke baseline (0.7440) -0.0187 = variance 範囲 |
| H3: Layer 1 が完全支配 (項目 116 `max_tool_output_chars=4000` ≈ 1333 tokens) | **CONFIRM** | F3 threshold 2048 まで降ろしても fire 0、Layer 1 出力が常に F3 threshold 下 |

## 核心知見

1. **F3 fire 計 0/180 run** (core 22 = 66 run + extended 18 task = 54 run + 本回 smoke 5 task = 60 run、計 180 run)
2. **threshold 半減 (4915→2048) でも fire 不発** — 現行 workload の単発 message の実 tokens は **2048 未満**、Layer 1 truncate (4000 chars ≈ 1333 tokens) が常に先行
3. **F3 を機能させるには threshold ≤ 1024 まで降ろす必要** — Layer 1 直下では二重切捨で副作用懸念、効果対コスト不釣り合い
4. **F3 副作用ゼロ確証** — 退行なし、AuditAction::F3SizeGuard も発火 0 = SQLite 永続化 query もゼロ → safe to delete
5. **プロンプト天井 5 連続確証** (v8 / v9 / v10 / v14 / 本回 v15) — 0 ACCEPT/10、Lab v15 プロンプト探索空間が枯渇
6. **項目 116 Layer 1 の支配確定** — 項目 190-193 を通して F3 が一度も fire しなかった事実は、Layer 1 = `max_tool_output_chars=4000` が bonsai workload において **完全に十分** であることを意味

## 採否判定: 案 A = F3 削除

handoff 05-06c の 3 選択肢:
- **(A) F3 完全削除** ← **採用** (★★★ 最優先候補)
- (B) default disabled で棚保留 ← 不採用
- (C) threshold 動的化 ← YAGNI

理由:
- fire=0 の dead-code を維持するコストが safety net 価値を上回る
- 現行 workload の実 token 分布から threshold 動的化しても fire しない (=動的化に意味なし)
- Layer 1 が支配しており、F3 削除しても safety net は保持される
- 将来 non-llama backend / multi-modal task で必要なら git history (項目 190-193) から復元可能

## 削除対象 (8 ソースファイル + 1 doc)

1. `src/agent/middleware.rs`: RequestSizeGuard struct + impl + helper + 14 tests + build_default_chain F3 引数
2. `src/observability/audit.rs`: AuditAction::F3SizeGuard variant + match arm
3. `src/config.rs`: ModelConfig.f3_max_message_tokens field + Default
4. `src/agent/agent_loop/config.rs`: AgentConfig.f3_max_message_tokens field + Default
5. `src/agent/agent_loop/core.rs`: build_default_chain 引数
6. `src/main.rs`: AgentConfig 構築 callsite
7. `src/agent/experiment.rs`: 2 callsite
8. `src/agent/benchmark.rs`: 2 callsite
9. `src/agent/compaction.rs`: doc comment 更新 ("F3 RequestSizeGuard などが" 部分削除)

期待: 1040 → 約 1026 passed (-14 tests = 11 RSG unit + 1 build_default_chain + 3 Phase 3 integration)、`test_build_default_chain_has_5_middlewares` を `_has_4_middlewares` に修正。

## 副次知見

- duration 607.9s baseline は項目 190 smoke (481s) 比 +27%、本セッションが llama-server 1h+ uptime 後の run = variance 範囲
- log に `{"name": "shell", "arguments": ...}` の textual tool_call leak 観測継続 (項目 192 から carry-over、項目 47 思考強制の副作用、別 plan 候補)

## 記録ファイル

- log: `/tmp/bonsai-llama/f3-half-smoke-2026-05-06.log` (5285 行 / 225KB)
- backup config: `~/Library/Application Support/bonsai-agent/config.toml.pre-f3-half-2026-05-06` (canonical SHA `e217687e1cc2d690...`)

## 次セッション TODO (削除完了後)

- ★ MLX primary + fallback sticky 動作見直し (項目 137 split policy + R13 CachedBackend disable、handoff 05-06b carry-over)
- ★ textual tool_call leak 調査 (項目 192 副次、parser 誤検出疑い)
- (任意) extended tier with optimal Layer 1 (8000 chars 等) の探索
