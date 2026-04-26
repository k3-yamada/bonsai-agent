# ADK Phase D: Workflow Primitive 形式化 — YAGNI 再評価ゲート

**Date:** 2026-04-26
**Phase:** ADK Phase D（Phase A/B1/B2/C 完了後の再評価）
**Status:** 設計（着手判断保留）

---

## 経緯

`adk-integration.md` Phase D は当初から「再評価ゲート設置」前提だった:

> Phase C 完了時点で本当にプリミティブ化が必要か再評価する（YAGNI 原則）。
> `run_agent_loop` のハードコードが具体的にどの拡張を阻害しているかを実例で示せない場合は **見送り**。

Phase A/B1/B2 + ベンチマーク 22→40 タスク（Phase C 第二解釈）+ agent_loop.rs 8 分割が完了した今、改めて評価する。

## 1. ADK Workflow primitive とは（再掲）

ADK 2.0 の3類型:

| Primitive | 用途 | bonsai での近接コード |
|-----------|------|---------------------|
| `SequentialAgent` | 子エージェントを順次実行 | `SubAgentExecutor::execute()`（順次パス） |
| `ParallelAgent` | 子を並列実行、結果集約 | `SubAgentExecutor::execute()`（並列パス、`std::thread::scope`） |
| `LoopAgent` | 終了条件付きループ | `run_agent_loop_with_session`（`max_iterations` ループ） |

## 2. YAGNI 評価チェックリスト

Phase D 着手判断のため、以下 5 項目を埋めて判定する。

### A. 具体的拡張ニーズ

- **Q1**: 現在のハードコードで実装困難な機能要件が存在するか？
  - **回答**: 現時点で具体的に未実装な「Workflow が必要だが書けない」機能はない。
  - サブエージェント並列は `SubAgentExecutor` で既に動作（項目160）。
  - メインループの繰り返しは `for iteration in 0..max_iterations` で十分。
  - 順次合成は `run_agent_loop_with_session` を呼び出すだけで成立。

- **Q2**: 直近 30 日のコミット履歴に「ハードコードゆえに諦めた」痕跡があるか？
  - **回答**: なし。`SubAgentExecutor` の追加（項目120/160）も既存 API の拡張で吸収できた。

### B. 抽象化の対価

- **Q3**: trait 化すると `run_agent_loop` のシグネチャ変更が必要か？
  - **回答**: 必要。`fn run_agent_loop(...)` → `Box<dyn WorkflowAgent>` 経由になり、main.rs/benchmark.rs/experiment.rs/subagent.rs の呼出全てを更新。
  - 機械的 grep でも 8+ 箇所、テストを含めると 30+ 箇所。

- **Q4**: 1bit モデルの精度劣化リスク
  - **回答**: trait 経由の動的呼出はトレースが追いづらく、Lab で REJECT 増加の懸念。Phase B2 Judge Gate で品質崩壊は弾けるが、Phase C のベンチマークで pass^k が悪化する可能性は残る。

### C. 既存テストへの影響

- **Q5**: trait 化で破壊されるテスト件数
  - **回答**: `run_agent_loop` を直接呼ぶ tests.rs の 13 件 + benchmark/experiment/subagent の統合テストが影響範囲。最低でも 20+ 件のリライト。

## 3. 判定マトリクス

| 評価軸 | 重み | スコア | 加重 |
|--------|------|--------|------|
| 具体的ニーズあり (Q1+Q2) | 3.0 | 0/2 | 0.0 |
| 抽象化対価が低い (Q3) | 2.0 | 0/2 | 0.0 |
| 精度劣化リスク低 (Q4) | 2.0 | 1/2 | 1.0 |
| テスト破壊小 (Q5) | 1.5 | 0/2 | 0.0 |
| **合計** | | | **1.0 / 17.0** |

**判定閾値**: 加重 ≥ 12.0 で着手、< 12.0 で見送り。

→ **判定: 見送り（1.0 << 12.0）**

## 4. 推奨アクション

### 即着手しないこと
- Workflow primitive trait 抽出 = **未着手**
- `Box<dyn SequentialAgent>` 等の形式化 = **未着手**

### 代わりに行うべき軽量改善

| 項目 | 概要 | 工数 |
|------|------|------|
| **D-α**: SubAgentExecutor の docstring 強化 | "Sequential/Parallel" の意味論を明文化、ADK 用語と対応付け | 30分 |
| **D-β**: BenchmarkSuite に LoopAgent 相当の category 追加 | 「ループ収束」を測る ToolChain タスク（既に Phase C で 18 件追加済、追加不要） | 0分 |
| **D-γ**: experiment.rs `run_experiment_loop` のコメントに ADK Phase D 評価結果を明記 | 将来の再評価のためにこのドキュメントへリンク | 10分 |

### 再評価トリガー

以下のいずれかが発生したら Phase D 着手判定を再開する:

1. **複合ワークフロー要件の発生**: 「リサーチエージェント → コーダー → レビュアー」のような3段階以上の固定パイプラインが具体的に必要になる
2. **Lab pass^k 改善天井**: Lab v13/v14 で全 REJECT が3サイクル続き、構造的拡張が改善経路の最後の選択肢として浮上
3. **ADK 2.0+ で primitive 標準化**: Google ADK 側で Workflow primitive の事実上の標準スキーマ（YAML/Proto）が公開され、それに合わせる必要が生じる

## 5. 結論

**Phase D は着手しない**。

理由:
- 現時点で Workflow primitive のハードコード除去によって unblock される具体的拡張が存在しない（YAGNI）
- 抽象化の対価（呼出元 30+ 箇所、20+ テスト）が便益を上回る
- 1bit モデルの精度に対する trait 動的呼出の影響が未知数

代わりに **D-α + D-γ**（合計 ~40分）でドキュメンテーションを補強し、再評価トリガーが発生するまで保留。

---

## 後続計画

- **Lab v13** 実行（Judge Gate 有効化、`judge_threshold=0.7`）
- **DESIGN_SPEC.md** に「Phase D = 見送り（YAGNI 判定）」を記載
- **CLAUDE.md** 項目164/165 として agent_loop 8 分割完了 + Phase D 見送り判定を追記
