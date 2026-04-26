# Lab v12 ACCEPT 詳細調査レポート

**Date:** 2026-04-26
**Source:** `~/Library/Application Support/bonsai-agent/experiments.tsv` (timestamp 1777096745…1777148260)
**Status:** 解析のみ・コード変更なし

---

## サマリー

- 10 experiments / 3 ACCEPT (30%) — v8-v11 (0-22%) を超える受理率だが、各 delta は +0.0017〜+0.0029 と小さい
- ベースライン score: 0.8043 → 最終 0.8110（+0.0067、4-5 サイクル累積効果）
- 全 ACCEPT の累積寄与: +0.0066（最終ベスト 0.8110 と整合）
- **結論: defaults 化は exp_0002 のみ条件付き候補、他は見送り**

## ACCEPT 3 件の詳細

| ID | 種別 | 内容 | delta | 累積 score |
|---|---|---|---:|---:|
| exp_0002 | agent_param | `temperature: 0.7 (バランス)` | **+0.0020** | 0.8043 → 0.8063 |
| exp_0004 | meta_mutation | `[ツール使用前に思考を強制] + [マルチステップ計画の強制]` | **+0.0017** | 0.8063 → 0.8080 |
| exp_0006 | prompt_rule | `insight: temperature 0.5 を避ける` | **+0.0029** | 0.8080 → 0.8110 |

## 判定

### exp_0002: `temperature: 0.7` — **条件付き候補（v13 確認実験で再検証）**

- 現状 default は `config.rs:202` で `0.5`
- exp_0002 (+0.0020) と exp_0006 (+0.0029) が**同じ軸の独立シグナル**として一致 → 単独 +0.0020 より信号強度が高い
- ただし各 delta は v6-v11 のノイズフロア（過去 REJECT で +0.0023〜+0.0029 を観測）と区別困難
- **defaults 化判断: v13 で `temperature=0.7` をベースラインに据えて 3 サイクル安定維持を確認してから昇格**
- 即 default 化はリスク（過去の v9 ACCEPT「事実確認」+0.0157 ですら 1 サイクル単独で defaults 化したが、本件はその 1/8 の delta）

### exp_0004: meta compound `思考強制 + 計画強制` — **defaults 化見送り**

- 構成要素はすでに defaults 済（CLAUDE.md 項目 47「思考強制」+ 項目 10「計画強制」）
- compound 効果 +0.0017 は両ルール再注入による僅差。新規追加項目なし
- v11 ACCEPT 解析と同じ「再注入経由の小幅改善」パターン（`.claude/plan/lab-v11-accept-analysis.md`）

### exp_0006: `temperature 0.5 を避ける` — **defaults 化不要（exp_0002 と同一軸）**

- exp_0002 の確認シグナルとして扱う
- prompt_rule として注入しても結局 InferenceParams.temperature を変えないと効かない（自然言語の prompt rule は推論パラメータに作用しない）
- 真の改善は exp_0002 の数値変更側に集約される

## 推奨アクション

| アクション | 着手条件 | 想定コスト |
|---|---|---|
| **(A) v13 確認実験**: `temperature=0.7` をベースラインに据えて 3 cycle 計測 | judge wire（Phase B1）と独立に着手可 | Lab 1 回分（~6h） |
| (B) 即 default 化（`config.rs:202` 0.5 → 0.7） | A の結果がベスト維持時のみ | 1 行変更 + テスト調整 |
| (C) 何もしない | A が時間的に走れない場合の保留 | 0 |

**現時点の推奨: (A)**。

## v13 で確認すべき仮説

1. **H1 (主)**: `temperature=0.7` を新ベースラインにすると、3 cycle 連続でベスト 0.8110 以上を維持する
2. **H2 (副)**: temperature を変動させない他軸（prompt_rule / meta_mutation）の探索でさらなる改善余地がある
3. **H3 (反証)**: +0.0020/+0.0029 はノイズで、0.7 でも 0.5 と同等のスコアに収束する

H3 が成立した場合、**プロンプト最適化天井に再到達**を意味し、ハーネス側の構造改善（ADK Phase B 以降の judge wire）に注力すべき。

## 参照

- 過去サイクル統計: `CLAUDE.md` 末尾「Lab実機テスト結果」セクション
- v11 同様の「再注入小幅改善」パターン: `.claude/plan/lab-v11-accept-analysis.md`
- ADK Phase B1 (judge wire): `.claude/plan/adk-integration.md`
