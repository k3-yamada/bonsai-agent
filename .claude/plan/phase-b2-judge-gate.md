# Phase B2: experiment.rs Judge Gate 統合

**Date:** 2026-04-26
**Phase:** ADK Phase B2（Phase B1 完了後継）
**Status:** 設計

---

## 背景

- Phase B1 で `HttpAdvisorJudge::evaluate()` が wire 完了（923テスト、commit `0c05882`）
- 現状の ACCEPT 判定は `delta > 0.0` の単一基準（`experiment_log.rs:197`）
- ADK `rubric_based_final_response_quality_v1` を ACCEPT ゲートに組込み、品質崩壊（高 score だが推論破綻）を弾く

## ゴール

`delta > 0.0` AND `judge_score >= judge_threshold(=0.7)` の AND 条件で ACCEPT。
judge 未設定時は従来通り `delta > 0.0` のみ（後方互換）。

## 設計

### 構造変化（最小侵襲）

1. **`MultiRunTaskScore`** に representative trajectory を保持（最終 run の `AgentLoopResult` を Option で保持）
   - `last_response: Option<String>`, `last_trajectory: Option<Vec<String>>`
   - 既存テストには影響しない（追加フィールド）

2. **`BenchmarkSuite::run_k`** で最終 run の `loop_result` を `MultiRunTaskScore` に格納
   - 既存スコア計算は無変更

3. **`ExperimentLoopConfig`** に判定用 field 追加:
   ```rust
   pub judge_threshold: Option<f64>,  // None=disabled, Some(0.7)=有効
   pub judge_sample_size: usize,       // 何タスク judge にかけるか（負荷制御）
   ```

4. **新関数 `judge_gate_check`**（experiment.rs）:
   - 入力: judge, &MultiRunBenchmarkResult, threshold, sample_size, &task_descriptions
   - 出力: `Result<JudgeGateOutcome { passed: bool, mean_composite: f64, scores: Vec<RubricScore> }>`
   - judge エラーは Warn ログして passed=true で fail-open（experiment 経路は止めない）

5. **`Experiment::from_multi_results_with_judge`**:
   - 既存 `from_multi_results` を呼出して、`judge_passed: bool` で `accepted` を上書き
   - `(delta > 0) && judge_passed` のみ ACCEPT

### 実装順（TDD Red → Green）

| Step | 内容 | 検証 |
|---|---|---|
| 1 | `MultiRunTaskScore` フィールド追加 + Default 初期化 | 既存テスト全 PASS |
| 2 | `judge_gate_check` 関数（TDD Red: スタブ Err） | 1 件失敗テスト |
| 3 | judge gate Green 実装（mean composite + threshold 比較） | 4–6 件テスト |
| 4 | `run_k` で last_response/last_trajectory 格納 | 既存テスト無破壊 |
| 5 | `run_experiment_loop` で judge_gate 配線（opt-in） | mock judge で統合テスト |
| 6 | config.toml 統合（[experiment] judge_threshold = 0.7） | wire 確認 |

### リスク

- **判定の安定性**: judge LLM 自体がノイジーな場合、ACCEPT 確率が変動。対策: 同一 judge prompt はキャッシュヒットするので、同セッション内では決定論的
- **コスト**: 22 タスク × k=3 = 66 run で judge 呼出が 22 回（sample_size=22）。Lab 1 サイクル数分の追加レイテンシ。sample_size=4 で軽量化可能
- **ベースラインも judge 通すか**: ベースライン judge を毎サイクル実行するとコスト 2 倍。**判断**: ベースラインは judge にかけず、experiment side のみ判定。`delta > 0` で baseline 越えは保証されるため、experiment 側 judge >= threshold で品質保証成立

### 後方互換性

- `judge_threshold: None`（デフォルト）→ 従来動作（delta > 0 のみ）
- 既存 ExperimentLoopConfig 利用箇所（main.rs --lab 経路）は明示設定なしで動作継続

## 完了条件

- judge_threshold = Some(0.7) 設定時に delta > 0 だが judge < 0.7 で REJECT
- judge_threshold = None で従来動作維持
- judge 失敗時 fail-open（Warn ログ + ACCEPT 判定継続）
- 単体テスト 4–6 件追加、923→928 程度
- DESIGN_SPEC.md Phase B2 を完了にマーク

## 後続計画

- **Phase C**: ベンチマーク 22 → 30+ タスク拡張（`.claude/plan/phase-c-and-refactor-draft.md`）
- **v13 確認実験**: temperature=0.7 ベースライン 3 サイクル（`.claude/plan/lab-v12-accept-analysis.md`）
- B2 完了後、judge wire を Lab v13 のメトリクスとして観測
