# Lab v11 ACCEPT 詳細調査レポート

## 調査対象
TSV: `/Users/keizo/Library/Application Support/bonsai-agent/experiments.tsv`
DB:  `/Users/keizo/Library/Application Support/bonsai-agent/bonsai.db` (table: experiments)

## ACCEPT 3件の生データ

| experiment_id | mutation_type | mutation_detail | baseline | score | delta |
|---------------|---------------|-----------------|----------|-------|-------|
| `exp_1777038142_0002` | `agent_param` | `max_retries: 4 (+1)` | 0.7965 | 0.7968 | **+0.0003** |
| `exp_1777047275_0004` | `meta_mutation` | `meta compound: [ツール使用前に思考を強制] + [マルチステップ計画の強制] (delta: +0.0319, +0.0250)` | 0.7968 | 0.8077 | **+0.0108** |
| `exp_1777051871_0005` | `prompt_rule` | `insight: 前回の変異(temperature: 0.2 (超精密))がdelta=-0.0003で悪化。この方向を避け逆のアプローチを試す` | 0.8077 | 0.8099 | **+0.0023** |

累積: 0.7965 → 0.8099（+0.0134）

## 判定マトリクス

### exp_0002: max_retries 3→4
- **既存実装**: `AgentConfig::default()` `max_retries: 3` (`src/agent/agent_loop.rs:55`)
- **delta=+0.0003** は誤差レベル（pass^k 1サンプル分以下）
- **判定: 保留** — Lab v12 で再現性要確認、現状デフォルト維持

### exp_0004: meta compound（最大効果）
- **既存実装**: 現 `DEFAULT_SYSTEM_PROMPT` に下記が明文化済（`src/agent/agent_loop.rs:120-121`）:
  - ルール9: 「複数ステップが必要な場合、まず計画を <think> に書いてから実行する」
  - ルール10: 「ツールを使う前に必ず <think> で意図と期待結果を書く」
- **メタ変異の正体**: `MetaMutationGenerator`（`src/agent/experiment.rs:339-410`）が**既存ACCEPT変異を別文字列で組合せ再注入**する仕組み
- **+0.0108 の解釈**:
  - 既デフォルト2ルールの「**重複強調**」が 1bit モデルの注意を引いた可能性
  - または Lab v11 ベースライン低下（0.7965）の reproducibility ブレ
- **判定: 保留** — デフォルト追記は冗長で巻戻しリスク高。表現改善（番号付け強化／太字化）は Lab v12 での再検証案件。

### exp_0005: oracle insight（temperature 逆向き）
- **元 REJECT 実験**: `temperature: 0.2 (超精密)` (delta=-0.0003、これも誤差レベル)
- **insight ルール**: 「temperature 0.2 を避け、逆方向（高め）を試す」
- **既存実装**: `inference_for_task()` で TaskType ごとに動的調整（`src/agent/agent_loop.rs:71-83`）
  - FileOperation/CodeExecution → 0.3
  - Research → 0.6
  - General → base
- **delta=+0.0023** は中程度だが、insight が抽象的（具体的な temperature 値なし）
- **判定: 保留** — 現状の動的 temperature がすでに「逆向き（高め寄り）」の方針

## 総合判定

**3件すべてデフォルト化見送り**。理由:

1. exp_0002, exp_0005 は **誤差〜中程度** で再現性未確認
2. exp_0004 の効果はあるが、**既存ルールの重複再注入** であり、デフォルト追記はプロンプト膨張リスク
3. CLAUDE.md「巻戻し禁止」原則に従い、不確実な変更でコア状態を壊さない

## 次のアクション

Phase A 完了。コード変更なし。**Phase B（EventStore ランタイム統合）に進む**。

Lab v12 はそのままのデフォルトで実行し、v11 の delta が再現するかを観察する（追加変更なしのベースライン安定性確認も兼ねる）。

## 補足: Lab 信頼性向上の必要性

- Lab v11 ベースライン 0.7965 は v10 の 0.8087 から低下（pass^k 1サンプル誤差で説明可能）
- **Phase C（ベンチマーク 22→40 タスク）の優先度上昇** — 統計信頼性 ±0.005 まで絞らないと、+0.003〜+0.011 の効果差は判定困難
