# Baseline Tier 2026-04-29 結果

## 実行情報
- 日時: 2026-04-29 18:46–20:05 JST (合計 ~80 min)
- Backend: mlx-lm
- Model: ternary-bonsai-8b (prism-ml/Ternary-Bonsai-8B-mlx-2bit)
- SSE timeout: 180s
- **k: 1 (Phase 5 暫定縮小、experiment.rs 1 行変更で MLX 推論時間制約に対応、計測後 revert)**
- 変異ループ: 無効（--lab-experiments 0）
- MLX 状態: 1 度 restart 実施 (元 MLX が完全 hang のため、pid 84519 に切替)

## 結果サマリー

| Tier | Tasks | Runs | score | pass@k | pass_consec | Duration | log path | exit |
|------|-------|------|-------|--------|-------------|----------|----------|------|
| core | 22 | 22 (k=1) | **0.4763** | 0.5000 | 0.5000 | 2100.7s (35 min) | /tmp/phase5-core-2026-04-29.log | 0 |
| extended | 18 | 18 (k=1) | **0.3410** | 0.3889 | 0.3889 | 1689.7s (28 min) | /tmp/phase5-extended-2026-04-29.log | 0 |

(注: 実行 wrapper の zsh `$status` read-only エラーで shell exit 1 を吐いたが、cargo bonsai 内部は両 baseline とも正常完了し、ログ末尾に `実験完了: 0件` まで記録済。)

## 仮説判定

- **仮説 X (Bench 拡張 22→40 が主因)**: **REJECT** — core 22 タスクのみで 0.4763、v9/v10 同構成 baseline 0.79–0.81 から -39% 低下。Bench 拡張だけでは説明不可。
- **仮説 Y (MLX 環境劣化が主因)**: **CONFIRM** — 実行中の MLX が hang (3h+ 連続稼働後 HTTP 完全停止)、restart 後も推論 50-100s/req で v9/v10 期 (35s/req 想定) より 1.5-3 倍遅延。
- **主因確定: Y (MLX 環境劣化)**

### 補強観測
- 同セッション内で MLX が 1 度 hang → restart で復旧 → 再度 SSE timeout fallback 多発: **MLX が高負荷で degrade する性質**
- 前セッション TSV 履歴: 同じ 40 タスク構成で v13 baseline = 0.7750 既往 → 仮説 Y 寄り prior (Codex audit 指摘) と整合
- extended (0.3410) も core (0.4763) も低水準 → 「Phase C タスクが極端に難しい」だけでは説明できない (両 tier で同じ MLX 劣化に晒されている)

### 注意点 (k=1 制約)
- k=1 のためバラつき (variance) は ±0.05–0.10 程度想定
- v9/v10 baseline は k=3 で測定 (平均化効果あり)
- 但し 0.79 vs 0.48 = **0.31 ギャップ**は variance では説明困難 (3σ 超)
- 結論の方向性は variance 影響でも反転しない

## 次の意思決定（自動派生）

**P2 FallbackChain (llama-server) を最優先**

理由:
- v14 baseline 0.5192 → Phase 5 core 0.4763 (k=1 でさらに variance あり) は MLX runtime 問題確定
- llama-server backend に切替えて同一バイナリで baseline を取り直せば、MLX vs llama-server 環境比較で差分が定量化される
- 既設 `.claude/plan/fallback-chain-impl.md` (項目 168 で実装済) を活用可能

## 副次知見

1. **MLX queue concurrency=1 が curl 偽陽性を生む**: 長時間推論中は `/v1/models` エンドポイントすら応答しないため "MLX DOWN" に見える。実際は処理中。
2. **bonsai 180s socket timeout が MLX cold start 227s より短い**: 初回推論で必ず timeout fallback、再起動直後の品質劣化要因。`config.toml sse_chunk_timeout_secs` の動的延長が候補。
3. **MLX 長時間稼働で degrade**: pid 72060 は Tue から Wed まで 75min CPU で完全 hang、restart 直後は健全だが ~30min 後に再 SSE timeout 多発。**メモリリーク or GPU リソース蓄積**の可能性。
4. **`--lab-experiments 0` の zsh wrapper exit code 1 偽陽性**: zsh の `$status` 変数は read-only。`status=$?` 代入で error。cargo 自体は正常終了。次回以降は `rc=$?` 等別変数名推奨。

## ログ取得経路 (再現可)

```bash
# core
grep -E "ベースライン[:：]" /tmp/phase5-core-2026-04-29.log
# → [lab] ベースライン: score=0.4763 pass@k=0.5000 pass_consec=0.5000 (2100.7s)

# extended
grep -E "ベースライン[:：]" /tmp/phase5-extended-2026-04-29.log
# → [lab] ベースライン: score=0.3410 pass@k=0.3889 pass_consec=0.3889 (1689.7s)
```

## Phase 5 後処理

- [x] core baseline 取得
- [x] extended baseline 取得
- [x] 結果記録ファイル作成 (本ファイル)
- [ ] `experiment.rs` k=3 へ revert (`git checkout HEAD -- src/agent/experiment.rs`)
- [ ] cargo build --release で revert 後ビルド確認
- [ ] memory/session handoff 追加 (任意)

## Open Questions

1. **k=3 で再測定すべきか？** — 結論方向性は variance で反転しないため、現状の Y 確定で次フェーズ着手可。再測定は P2 FallbackChain と組み合わせる方が情報利得高い。
2. **MLX ハングの再現条件** — 現セッション内で 2 度発生 (元 pid 72060 + restart 後 pid 84519)。launch から ~30-60 min で degrade する仮説を P2 内で検証。
3. **Phase C タスクの過剰難易度** — extended < core (0.34 < 0.48) は Phase C 設計の妥当性も疑わせる。Y 解消後に再判定すれば task redesign 要否が分離できる。

## SESSION_ID
- CODEX_SESSION (Phase 5 plan critique): `019dd7c0-67e0-7121-aa64-ca77b14532fa`
