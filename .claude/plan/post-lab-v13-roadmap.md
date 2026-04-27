# Post Lab v13 ロードマップ（構造改善 v3）

**Date:** 2026-04-27
**Status:** マスタープラン（Lab v13 完了後の着手判断材料を集約）

---

## 全体像

Lab v13 完了後の作業を 4 系統 × 7 ステップに整理:

```
[A 系: 構造改善 v3 — Lab v13 中断結果で優先度再編]
  ★緊急  Step 13 →  Socket Timeout（新規）   1h  recvfrom hang 防止
  ★緊急  Step 12 →  Fallback Chain (★★)     3h  Lab v13 で必須確認
  Step 11  →  Edit Cycle Detection (★★)  1h  +4 テスト
  Step 10  →  DiffStore (★★★)         4h  +91% トークン削減見込み

[B 系: 品質強化]
  Step 8   →  依存最適化軽量実施       1h  ureq 重複削減
  Step 9   →  テストカバレッジ +50    9h  cargo-llvm-cov 統合

[C 系: 知見継続活用]
  v3 後   →  macOS26/Agent v3 確認     ?h  3 候補すべて取込後の次回 audit
  常時   →  ADK Phase D 再評価ゲート   —   3 トリガー監視

[D 系: Lab 反復]
  v14 計画 →  v13 結果 + Step 10-12 の効果反映
```

## 着手順序（推奨）

### Phase 1: Lab v13 結果分析（必須）

```bash
# Lab 終了確認
/tmp/lab-progress.sh

# TSV 結果取得
cat ~/.local/share/bonsai-agent/experiments.tsv | head -20

# 評価
- baseline score（v12, v11, v10 と比較）
- ACCEPT 数（v9 の 1/14 = 7% との比較）
- Judge Gate 効果（mean composite 分布）
- 実行時間（推定 9h vs 実測）
```

**判定**:
- baseline ≥ 0.78 かつ ACCEPT ≥ 2 件 → 順調、構造改善 v3 着手
- baseline < 0.75 → MLX 状態 / 新タスクの問題、Lab v14 で再計測
- ACCEPT 0 件 → 改善天井、Phase D 再評価トリガー発動

### Phase 2: 構造改善 v3 (A 系、合計 8h)

優先順位（ROI 順、各 commit 独立実装）:

#### Step 10: DiffStore (★★★、4h)
- 詳細: `.claude/plan/diffstore-rust-impl.md`
- 採否ゲート:
  - Lab v13 で MultiEdit 平均 diff サイズ 150+ トークン
  - 連続 5+ 回呼出が観測される
- 期待効果: トークン -91%（理論最大）、未確認の場合 50-70% 程度

#### Step 11: Edit Cycle Detection (★★、1h)
- 詳細: `.claude/plan/edit-cycle-detector-impl.md`
- 採否ゲート:
  - Lab v13 で「同ファイル交互編集 REJECT 変異」観測
  - Lab v13 ループ検出 15 件（既観測）の内訳分析

#### Step 12: Fallback Chain (★★、3h)
- 詳細: `.claude/plan/fallback-chain-impl.md`
- 採否ゲート:
  - Lab v13 で MLX 接続断 / API 失敗 2+ 回
  - SSE タイムアウトフォールバックが既発動（観測済 → ★ 推奨）

### Phase 3: 品質強化 (B 系、合計 10h)

#### Step 8: 依存最適化軽量実施 (1h)
- 詳細: `.claude/plan/step-8-dependency-eval.md`
- 内容: ureq 重複削減のみ、200KB バイナリ削減
- 当初 -10% 目標は -2% に下方修正

#### Step 9: テストカバレッジ +50 テスト (9h)
- 詳細: `.claude/plan/step-9-coverage-design.md`
- 段階1: cargo-llvm-cov 計測基盤（30分）
- 段階2: 優先 5 ファイルに +50 テスト（8h）
- 段階3: CI 統合（1h）
- 950 → 1000 テスト、推定カバレッジ 70-80%

### Phase 4: Lab v14 計画

Lab v13 + Step 10-12 完了後、効果計測のため Lab v14 実行:

```toml
[experiment]
max_experiments = 14
judge_threshold = 0.7  # v13 と同じ条件
judge_sample_size = 4
```

期待:
- baseline +0.01 〜 +0.03（DiffStore でトークン削減 → コンパクション低減）
- ACCEPT +1 〜 +2 件（プロンプト天井 14 個目を超える可能性）

### Phase 5: 継続的活用 (C 系)

- macOS26/Agent v3 確認（候補 3 件すべて取込後）
- ADK Phase D 再評価トリガー監視
- 新規 OSS 知見抽出（cycle: 月次推奨）

## 工数見積もり総括

| Phase | 工数 | 累計 |
|-------|------|------|
| 1: Lab v13 分析 | 1h | 1h |
| 2: 構造改善 v3 | 8h | 9h |
| 3: 品質強化 | 10h | 19h |
| 4: Lab v14 実行 | 9h（自動） | 28h |
| 5: 継続活用 | 月次 | — |

**合計: ~3 セッション分（次回 + 次々回）で完結見込み**

## リスクと依存関係

| リスク | 影響 | 緩和策 |
|---|---|---|
| Lab v13 結果が悪化（baseline < 0.75） | Phase 2 着手判断見送り | Lab v14 で再計測、原因特定優先 |
| Step 10 (DiffStore) で 1bit モデルが UUID 概念学習できず | トークン削減効果なし | descriptions.rs に明示例増、Lab で対比計測 |
| Step 12 (Fallback) で in-flight token 整合性問題 | 推論結果汚染 | record_failure 後の再試行は新セッションで再開 |
| Step 8 で ureq upgrade 不可（依存固定） | 依存削減不可 | 完全スキップ、Step 9 に集中 |

## 参照集約

| ドキュメント | 用途 |
|---|---|
| `.claude/plan/diffstore-rust-impl.md` | Step 10 詳細 |
| `.claude/plan/edit-cycle-detector-impl.md` | Step 11 詳細 |
| `.claude/plan/fallback-chain-impl.md` | Step 12 詳細 |
| `.claude/plan/step-8-dependency-eval.md` | Step 8 詳細 |
| `.claude/plan/step-9-coverage-design.md` | Step 9 詳細 |
| `.claude/plan/lab-v13-config-draft.md` | Lab v13 起動設定 |
| `.claude/plan/phase-d-evaluation.md` | Phase D YAGNI 判定 |
| `.claude/plan/macos26-agent-learnings-v2.md` | C 系候補集約 |
| `.claude/plan/structural-improvements-v2.md` | 全体プラン（Step 0-9） |

## 次セッション開始時の手順

```bash
# 1. Lab v13 状態確認
/tmp/lab-progress.sh

# 2. Lab 完了なら結果取得
cat ~/.local/share/bonsai-agent/experiments.tsv

# 3. 本ロードマップ Phase 1 から進める
cat .claude/plan/post-lab-v13-roadmap.md
```

## 結論

Lab v13 完了 → 構造改善 v3 着手 → Lab v14 実行 のサイクルで bonsai-agent を v0.2 相当に進化させる。各ステップは独立 commit で実装可能、リスクは段階的に検証可能。
