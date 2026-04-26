# Lab v13 Config 草稿

**Date:** 2026-04-26
**Status:** 草稿（実行は GO 判断後）

---

## 目的

Phase B2 Judge Gate（項目163）を有効化し、ベンチマーク 22→40 タスク拡張（同）後の最初の Lab 実走を計測する。

## 推奨 `[experiment]` セクション追加

`~/.config/bonsai-agent/config.toml` の末尾に以下を追加:

```toml
[experiment]
# 最大実験回数（None=無限）。v13 は 14 サイクルで打ち切り
max_experiments = 14

# Dreamerレポート間隔（N実験ごと）
dreamer_interval = 10

# プリスクリーニング（4タスク×k=1で事前評価、deltaが負ならフル評価をスキップ）
enable_prescreening = true
prescreening_threshold = -0.01

# タスク単位タイムアウト（300秒、ウォールクロック）
task_timeout_secs = 300

# Judge Gate（Phase B2 ADK rubric_based_final_response_quality_v1）
# 0.7 = mean composite >= 0.7 で ACCEPT 二次ゲート通過
judge_threshold = 0.7

# Judge にかけるサンプル数（負荷制御）
# 40 タスク中 4 タスクのみ評価 → 1 サイクルあたり advisor 呼出 +4 回
judge_sample_size = 4
```

## 推定実行コスト

| 項目 | 値 |
|------|-----|
| ベンチマークタスク | 40 |
| pass^k k値 | 3 |
| 1サイクルあたりラン数 | 40 × 3 × 2（baseline + experiment）= 240 |
| 最大サイクル | 14 |
| 推定総ラン数 | 14 × 240 = **3360 ラン** |
| MLX 平均時間/ラン | ~30秒（推定、stream 込） |
| **推定総時間** | **~28 時間**（保守的、prescreening 早期棄却で実質 1/3 ~ 9 時間想定） |
| Judge 呼出 | 14 × 4 × 2（baseline + exp）= 112 回（claude-code 経由 = $0） |

## 実行コマンド

```bash
# プロジェクトルートで
cargo build --release  # Lab 中の競合を避けるため事前ビルド
cargo run --release -- --lab 2>&1 | tee /tmp/bonsai-lab-v13-$(date +%Y%m%d-%H%M).log
```

または既存 release バイナリ:
```bash
./target/release/bonsai-agent --lab 2>&1 | tee /tmp/bonsai-lab-v13.log
```

## 監視ポイント

1. **進捗ログ**: `eprintln` で `[experiment cycle X/14] baseline=... experiment=... delta=...`
2. **Judge ゲート発動**: `eprintln` で `[judge gate] mean=0.XX threshold=0.7 → PASS/FAIL`
3. **TSV ログ**: `~/.local/share/bonsai-agent/experiments.tsv`（11カラム、pass^k 指標含）
4. **早期停止条件**: 3 サイクル連続 REJECT（adaptive trigger 項目141 が起動、Dreamer 早期発火）

## 監視中断方法

- フォアグラウンド実行中: `Ctrl+C` （signal handler で graceful shutdown）
- バックグラウンド実行中: `kill -SIGTERM <pid>` （cargo の cancel token 経由）

## 完了後の作業

1. **memory に v13 結果記録**: `lab_history_v13.md` 新規作成、ACCEPT 変異と REJECT 理由を集計
2. **CLAUDE.md 項目167 追記**: v13 結果サマリー（ベースライン score, ACCEPT 数, judge gate 効果）
3. **session_handoff 04-27 作成**: 次セッション引継ぎ

## 中止判断トリガー

実行を中断すべき条件:

- [ ] **MLX サーバ応答喪失**: `/v1/models` HTTP 404 が 5 回連続
- [ ] **メモリ枯渇**: M2 16GB の swap 使用 > 50%（vm_stat 監視）
- [ ] **3 サイクル連続全 REJECT + delta variance < 0.001**: 改善天井に到達、適応トリガー発動済（Phase D 再評価）
- [ ] **judge_threshold が高すぎて全 REJECT**: 5 サイクル連続で `mean_composite < 0.7` なら閾値見直し（0.6 等）

## 後方互換性

- `[experiment]` セクションを削除すれば従来動作（`Default` 値、judge_threshold=None で delta > 0 のみで ACCEPT）
- `judge_threshold = 0.7` だけ削除すれば judge ゲート無効、他の experiment 設定は有効

## 推奨実行タイミング

- 別作業がない夜間・週末
- Mac の他プロセス（Chrome/Slack/Discord 等）を最小化
- llama-server/MLX サーバが稼働中であることを `curl http://localhost:8000/v1/models` で確認後

---

## 判定: 実行 GO/NO-GO

ユーザー判断を待つ:

- [ ] **GO**: 上記 config を `~/.config/bonsai-agent/config.toml` に追加し、`cargo run --release -- --lab` 起動
- [ ] **NO-GO（理由保留）**: 別の優先タスクを優先、v13 は次回セッション持越し
- [ ] **修正 GO**: `judge_threshold` や `max_experiments` を調整した上で実行
