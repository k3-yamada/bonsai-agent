# Session 2026-05-01e Completion Plan

## Task Type
- [x] Backend (process orchestration / docs / no UI)
- [ ] Fullstack
- [ ] Frontend

## Multi-Model Synthesis 状況

| 項目 | 状況 |
|------|------|
| Codex | **不可** (`codex` CLI 未インストール、exit 127) |
| Gemini | **完了** (`brv336uo0`、UX/handoff 観点 5 件助言) |
| Claude (本体) | 主導、技術判断・実装計画・実行 |

→ Backend 観点 (技術的シーケンシング) は Claude 単独、UX 観点 (handoff/commit 粒度) は Gemini 採択。

## Technical Solution (合意プラン)

**Phase A 完了** (2 commits, 1012 passed maintained), **Phase B1 進行中** (background `bgf3h426g`, 60-90 min ETA, no polling)。残作業を以下の順序・粒度で完結:

```
[B1 完了通知]
  → B2: log parse (score / pass@k / pass_consec / duration / SSE timeout / RSS)
  → B3: P3-α (0.7079 / 60.6 min) と比較、判断ゲート判定
  → B4: .claude/plan/mcp-detach-core22-result.md 記録
  → C1: MLX-LM startup (background, 5 min health check timeout)
       success → C2: BONSAI_LAB_SMOKE=1 で smoke 実行 (~30 min)
                C3: smoke 結果 vs llama-server smoke (0.5253) 比較
       failure → C99: skip, handoff 上部 "⚠️ ブロッカー" として記載
  → D1: CLAUDE.md 項目 181/182/183 個別追記 (Gemini #2 採択)
  → D2: MEMORY.md index 1 行追加 (handoff へのポインタ)
  → D3: session_2026_05_01e_handoff.md 作成 (~150 行、既存テンプレ準拠) (Gemini #1)
  → D4: docs commit を 2 分割 (CLAUDE.md+MEMORY.md / handoff) (Gemini #3)
  → 最終: push 段階提案を user 提示 (Gemini #5)
```

## Implementation Steps

### B2: ログ解析

| Step | 内容 | 失敗時 |
|------|------|--------|
| B2-1 | `tail -200 /tmp/bonsai-llama/mcp-detach-core22.log` で最終結果ブロック確認 | log truncation 検知 → grep "score=" で全件抽出 |
| B2-2 | "Average Score" / "Pass@k" / "Duration" 行を抽出 | パターン不一致 → benchmark.rs 出力 format 参照 |
| B2-3 | SSE timeout 件数集計 (`grep -c "SSE.*timeout"`) | — |
| B2-4 | llama-server PID/RSS 確認 (`ps aux | grep llama-server`) | — |

### B3: 判断ゲート

| 指標 | P3-α 比較対象 | ゲート | 評価 |
|------|--------------|------|------|
| score | 0.7079 | ≥ 0.7079 → MCP detach 改善 confirm | docs に明記 |
| duration | 60.6 min | ≤ 60.6 min → 速度向上 confirm | docs に明記 |
| SSE timeout | 5 件 | < 5 → 環境安定 / ≥ 5 → 不安定 | docs に明記 |
| pass@k | 0.8030 | ≥ 0.8030 → 一貫性向上 | docs に明記 |

### B4: 結果文書

`.claude/plan/mcp-detach-core22-result.md` 新規作成。セクション: 計測条件 / 結果メトリクス / P3-α 比較 / 判断結論 / 副次知見。

### C1-C3: MLX 試行 (best-effort)

```bash
# C1 startup (mlx_lm.server を background で起動)
nohup mlx_lm.server --model <path> --port 8000 > /tmp/bonsai-llama/mlx-startup.log 2>&1 &

# C1 health check loop (max 5 min, 10 sec interval)
for i in $(seq 1 30); do
  if curl -s --max-time 3 http://127.0.0.1:8000/v1/models > /dev/null 2>&1; then
    echo "MLX healthy at attempt $i"; break
  fi
  sleep 10
done

# C2 smoke (only if C1 success)
# [fallback_chain] config を MLX primary / llama-server fallback に再設定
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
```

### D1-D4: docs (Gemini 採択)

**D1 commit**: `docs: CLAUDE.md 項目 181/182/183 + MEMORY.md index 更新`
- 項目 181: Phase A clippy 6 件整理 (chore commits 2 件、test code only)
- 項目 182: MCP detach core 22 baseline (B 結果 + P3-α 比較)
- 項目 183: Phase C MLX smoke (結果 or ⚠️ ブロッカー)
- MEMORY.md: handoff 1 行追加

**D2 commit (separate)**: `docs: session_2026_05_01e_handoff.md 作成`
- ~150 行、既存テンプレ (`session_2026_05_01d_handoff.md`) 準拠
- セクション順 (Gemini #1):
  1. 完遂サマリー (table)
  2. (失敗時のみ) ⚠️ ブロッカー: MLX-LM 起動失敗 (Gemini #4)
  3. Phase 別詳細 (A/B/C/D)
  4. 環境状態
  5. TODO ハンドオフ (優先度順、05-01d carry-over + 派生)
  6. 知見メモ
  7. 次セッション最初の確認

### 最終: push 段階提案 (Gemini #5)

3 択提示で user に確認:
```
Plan A (一括): 73+ commits を一度に origin/master へ push
Plan B (段階): Stage 1 (chore/docs only) → Stage 2 (feat/fix bundle、確認後)
Plan C (見送り): 次セッション持ち越し
```

## Key Files

| ファイル | Operation | 説明 |
|---------|----------|------|
| `.claude/plan/session-2026-05-01e-completion.md` | Created (本ファイル) | Multi-model 統合プラン |
| `.claude/plan/mcp-detach-core22-result.md` | Will create (B4) | B 結果記録 |
| `CLAUDE.md` | Will modify (D1) | 項目 181/182/183 追記 |
| `memory/MEMORY.md` | Will modify (D1) | index 1 行追加 |
| `memory/session_2026_05_01e_handoff.md` | Will create (D2) | handoff |
| Production code (.rs) | **NEVER modify** in remainder of session | Phase A clippy 以外なし |

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| Phase B1 が中断/異常終了 | log 全文 review、Phase A revert 不要、handoff に異常記載 + 次セッション再試行 TODO |
| MLX cold start が 5 min 超過 | C2/C3 skip、handoff 上部に ⚠️ ブロッカー記載 (Gemini #4) |
| llama-server OOM/crash | `dmesg` / Activity Monitor 確認、handoff に記載 |
| handoff line count 超過 | テーブル化推進、詳細は別 plan ファイルへリンク |
| Push 段階提案が複雑 | Plan A / Plan B / Plan C の 3 択で簡潔化 |

## Decision Audit

| 判断 | 根拠 |
|------|------|
| Codex skip | CLI 未インストール、wrapper も codex 既定 fallback 不可。Claude+Gemini で十分 |
| Phase B1 継続 (kill 不可) | system instructions: "Don't poll, do not kill" + 60-90 min 投資価値 (P3-α 直接比較) |
| Phase C 試行 | user 「最適と考えられる方法」承認、cold start 失敗時の defer path 明記 |
| Push は user 確認必須 | system instructions: "shared state 変更 → confirm first" |
| Commit 2 分割 (Gemini #3) | 履歴透明性、後日 docs revert 容易性 |
| 項目 181/182/183 split (Gemini #2) | Phase 別 status 管理、依存関係明示 |

## SESSION_ID (for /ccg:execute)
- CODEX_SESSION: **N/A** (codex CLI absent)
- GEMINI_SESSION: `brv336uo0` (one-shot non-interactive `gemini -p`、resume 不要)

## 進行ステータス

- [x] Phase A (chore commits 2 件、a7974a4 / 2fde274)
- [ ] Phase B1 (running, bgf3h426g, ETA 60-90 min)
- [ ] Phase B2-B4 (B1 完了通知後)
- [ ] Phase C1-C3 (best-effort)
- [ ] Phase D1-D2 (docs commits)
- [ ] Push (user 確認後)

---

User の事前承認 ("yes" + "最適と考えられる方法で実施して") 通り、Phase B1 完了通知 →
B2-B4 → C → D → push 提案の順で実行を継続します。本ファイルは透明性のための
記録であり、新規承認待ちは Phase D 完了後の **push 判断** のみです。
