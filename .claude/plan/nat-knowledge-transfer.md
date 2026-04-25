# NeMo Agent Toolkit 知見適用計画

## 背景
NVIDIA NeMo Agent Toolkit (NAT) の調査から、bonsai-agentに適用可能な5つのパターンを特定。
設計原則「Scaffolding > Model」に沿い、1bitモデルの信頼性を底上げするハーネス改善に焦点。

## 優先順位（効果×実装コスト）

### Phase 1: 構造化フィードバックリトライ（効果: 高、コスト: 小）
**NAT知見**: SelfEvaluatingAgentWithFeedback — confidence < threshold でEVALUATION/MISSING/SUGGESTIONS構造を注入し再試行。best_resultをconfidenceで追跡。

**現状の課題**:
- `ContinueSite` 3段エスカレーションは存在するが、Replan時のフィードバックが非構造的
- `TrialSummary::format_for_replan()` は80/100文字截切で情報損失
- `inject_verification_step` に失敗履歴コンテキストが渡されていない

**実装**:
1. `StructuredFeedback` 構造体を `error_recovery.rs` に追加
   ```rust
   struct StructuredFeedback {
       evaluation: String,      // 現在の状態評価
       missing_steps: Vec<String>, // 未完了のステップ
       suggestions: Vec<String>,   // 具体的な次のアクション
       confidence: f64,         // 0.0-1.0
   }
   ```
2. `TrialSummary::format_structured()` メソッド追加（NAT形式テンプレート出力）
3. `inject_replan_on_stall()` で構造化フィードバックを使用
4. `inject_verification_step()` に `trial_summary` パラメータ追加
5. `best_result` 追跡: LoopStateにベスト回答+スコアを保持、最終的にベストを返す

**変更ファイル**:
- `src/agent/error_recovery.rs` — StructuredFeedback, format_structured()
- `src/agent/agent_loop.rs` — inject_verification_stepにtrial_summary渡し、best_result追跡
- `src/agent/context_inject.rs` — フィードバックテンプレート

**テスト**: StructuredFeedback生成テスト、format_structured()テスト、best_result追跡テスト（6-8件）

---

### Phase 2: 軌跡評価（効果: 高、コスト: 中）
**NAT知見**: expected_trajectory vs actual_trajectory ペア保存。ツール呼出シーケンスの正確性を評価。

**現状の課題**:
- `TaskScore` は keyword + completion のみ（40/30/20/10固定重み）
- `evaluate_task_response` はmatch-on-idの巨大関数、プラグ不可
- 「正しい答えを間違った理由で出す」ケースを検出できない

**実装**:
1. `BenchmarkTask` に `expected_trajectory: Vec<String>` フィールド追加（期待ツール呼出順序）
2. `TrajectoryScore` 構造体追加
   ```rust
   struct TrajectoryScore {
       sequence_accuracy: f64,  // 順序一致率（LCS/編集距離）
       tool_coverage: f64,      // 期待ツールのカバー率
       extra_calls: usize,      // 不要なツール呼出数
   }
   ```
3. `TaskScore::score()` を拡張: 既存4指標 + trajectory_score（重み15%、既存を按分縮小）
4. `AgentLoopResult` に `tools_called: Vec<String>` は既存 → これを trajectory として使用
5. 22タスクの `expected_trajectory` を定義

**変更ファイル**:
- `src/agent/benchmark.rs` — TrajectoryScore, BenchmarkTask拡張, score()改訂
- テスト: TrajectoryScore計算テスト、LCS/編集距離テスト、統合テスト（8-10件）

---

### Phase 3: オラクルフィードバック変異生成（効果: 中、コスト: 小）
**NAT知見**: GA prompt optimizer の `extract_worst_reasoning()` — 最悪スコアの失敗理由を抽出し、次の変異生成プロンプトに注入。

**現状の課題**:
- `HypothesisGenerator` は固定36候補を順次消費、失敗理由を活用しない
- REJECT変異の失敗パターンから学習しない
- v8/v9で全REJECT（収束）→ 変異空間の知的拡張が必要

**実装**:
1. `ExperimentResult` に `worst_task_id: String, worst_reason: String` フィールド追加
2. `extract_worst_reasoning(results: &[Experiment]) -> Vec<(String, String)>` 関数
   - 直近N実験のREJECTから、最もdeltaが悪いタスク+失敗パターンを抽出
3. `HypothesisGenerator::next_mutation_with_context(worst_reasons)` メソッド
   - 失敗理由から逆向きに変異候補を生成（例: tool_success_rate低い→ツール引数精度変異）
4. `run_experiment_loop` で毎REJECT後にworst_reasoning蓄積

**変更ファイル**:
- `src/agent/experiment.rs` — extract_worst_reasoning(), next_mutation_with_context()
- `src/agent/experiment_log.rs` — ExperimentResult拡張
- テスト: worst_reasoning抽出テスト、コンテキスト付き変異生成テスト（4-6件）

---

### Phase 4: 適応的トリガー（効果: 中、コスト: 小）
**NAT知見**: check_adaptive_triggers() — 停滞（best fitness不変N世代）、分散崩壊、多様性崩壊の3条件でオラクル注入。

**現状の課題**:
- Dreamer間隔が固定（`dreamer_interval`、デフォルト10）
- REJECT連続時にDreamerを早期起動する仕組みなし
- v8の全10REJECT、v9の8/9REJECTは「停滞」の典型

**実装**:
1. `LabStagnationDetector` 構造体
   ```rust
   struct LabStagnationDetector {
       best_score_unchanged_count: usize,  // ベスト不変カウント
       recent_deltas: VecDeque<f64>,       // 直近N実験のdelta
       stagnation_threshold: usize,        // デフォルト3
       variance_collapse_threshold: f64,   // デフォルト0.001
   }
   ```
2. `check_triggers() -> LabTrigger` メソッド
   - `Stagnation`: best不変3回以上
   - `VarianceCollapse`: 直近5実験のdelta分散 < 0.001
   - `None`: トリガーなし
3. `run_experiment_loop` でトリガー発火時にDreamer早期起動
4. config.toml `[experiment]` に `stagnation_threshold`, `variance_collapse_threshold` 追加

**変更ファイル**:
- `src/agent/experiment.rs` — LabStagnationDetector, check_triggers()
- `src/config.rs` — ExperimentConfig拡張
- テスト: 停滞検出テスト、分散崩壊テスト（4-6件）

---

### Phase 5: before_stepフック（効果: 中、コスト: 中）
**NAT知見**: プリスクリーン、プロンプト修正、コンテキスト剪定をLLM呼出前に実行。

**現状の課題**:
- `Middleware` トレイトに `after_step` のみ、`before_step` なし
- LLM呼出前の介入ポイントが `execute_step` 内部にハードコード
- 将来のNeMo的パターン（プロンプト圧縮、安全ガード）に対応不可

**実装**:
1. `Middleware` トレイトに `before_step()` メソッド追加（デフォルト実装: Ok）
   ```rust
   fn before_step(&mut self, session: &Session, iteration: usize) -> MiddlewareSignal {
       MiddlewareSignal::Ok
   }
   ```
2. `MiddlewareSignal` に `Abort(String)` バリアント追加
3. `MiddlewareChain::run_before_step()` メソッド追加
4. `execute_step` の冒頭で `middleware_chain.run_before_step()` 呼出
5. 既存4ミドルウェアはデフォルト実装を継承（変更なし）

**変更ファイル**:
- `src/agent/middleware.rs` — before_step(), MiddlewareSignal::Abort, run_before_step()
- `src/agent/agent_loop.rs` — execute_step冒頭にbefore_step呼出

**テスト**: before_step呼出順序テスト、Abortシグナルテスト（4件）

---

## 実装順序とスケジュール

```
Phase 1 (構造化フィードバック)  → 項目138-139  (~20テスト追加)
Phase 2 (軌跡評価)              → 項目140-141  (~20テスト追加)
Phase 3 (オラクルフィードバック) → 項目142      (~10テスト追加)
Phase 4 (適応的トリガー)        → 項目143      (~10テスト追加)
Phase 5 (before_stepフック)     → 項目144      (~8テスト追加)
```

合計: ~68テスト追加、845→913テスト見込み

## Lab検証計画
- Phase 1+2完了後にLab v10実行（構造化フィードバック+軌跡評価の複合効果測定）
- Phase 3+4完了後にLab v11実行（変異生成改善の効果測定）

## リスク
| リスク | 軽減策 |
|--------|--------|
| Phase 2の重み変更で既存ベースライン低下 | 旧score()を`score_v1()`として保持、比較可能に |
| Phase 5のbefore_stepが既存MW動作に影響 | デフォルト実装=Okで後方互換 |
| 軌跡評価の22タスク定義コスト | 既存`expected_tools`を基盤に順序情報のみ追加 |
