# Implementation Plan: 構造的改善ロードマップ v2（残タスク＋Lab v11計測）

## Task Type
- [x] Backend (Rust / harness-level structural changes)
- [ ] Frontend
- [ ] Fullstack

## 背景（v1からの進展）

v1プラン以降、以下が完了:
- ✅ **P0 Step 1**: middleware.rs unsafe除去（項目156、ライフタイム化）
- ✅ **P1 Step 3**: セマンティックツール選択（項目158、fastembed + cos類似度+キーワードハイブリッド）
- ✅ **P1 Step 4**: セマンティックコンパクション（項目159、SemanticScorer、opt-in）
- テスト: 886 → **892**（+6セマンティックコンパクションテスト）

**Lab v10**: score=0.8087, pass@k=0.8939（プロンプト変異は天井到達）

**残作業**: P0 Step 2, P1 Step 5, P2 Step 6-7, P3 Step 8-9 + **Lab v11計測未実行**

---

## Technical Solution

v1の残Step + Lab v11測定を優先度付きで実施。**Step 0（Lab v11）を最優先**とし、既完了構造改善の効果を定量化してから次フェーズに進む。

---

## Implementation Steps

### ✅ Step 0: Lab v11測定（完了、項目160-162の効果計測 → memory/lab_history_v9_period.md）

- **目的**: 項目156-159の効果を定量化（セマンティック選択・コンパクションの実効性確認）
- **実行**: `cargo run -- --lab` をバックグラウンド起動、pass^k版ExperimentLoop（k=3, jitter_seed=true）
- **比較基準**: Lab v10ベースライン（score=0.8087, pass@k=0.8939）
- **期待効果**: score +0.005-0.015（構造変更による緩やかな改善を予想）
- **所要時間目安**: 2-4時間（14変異候補×3回実行×22タスク）
- **中断保護**: ScheduleWakeupで定期的にTaskOutputポーリング
- **完了条件**: `.vcsdd/experiments.tsv` に新規行14件追加

### ✅ P0 Step 2: subagent.rs rayon並列実行化（完了、項目160 — std::thread::scope で代替実装）

- **現状**: `src/agent/subagent.rs:148` 順次 `for (i, goal) in subtask_goals.iter().enumerate()` ループ
- **変更設計**:
  ```rust
  // 独立性判定: サブタスクゴール文字列から相互参照チェック
  let independent = check_independence(&subtask_goals);
  if independent && subtask_goals.len() >= 2 {
      use rayon::prelude::*;
      let results: Vec<_> = subtask_goals.par_iter().enumerate()
          .map(|(i, goal)| execute_single_subtask(...))
          .collect();
  } else {
      // 既存の順次実行を保持
  }
  ```
- **並列時の考慮点**:
  - MemoryStoreはConnection単位で独立（rusqliteのSend制約、clone不要）
  - TaskManager登録は親タスクIDで衝突なし
  - cancel_tokenはArc<AtomicBool>で共有可
- **期待効果**: M2マルチコア活用で独立サブタスク 2-4倍高速化
- **影響範囲**: subagent.rs + Cargo.toml（rayon既存チェック）
- **テスト**:
  - 独立サブタスク3件の並列実行（時間測定）
  - 依存サブタスク（A→B→C）の順次維持
  - キャンセル時の速やかな停止
  - MemoryStore競合なし確認

### ✅ P1 Step 5: Event Sourcingからの軌跡抽出（完了、項目161/162）

- **現状**: `src/agent/event_store.rs` 198行、append-onlyイベントストリームあり。学習用抽出API未実装
- **追加API設計**:
  ```rust
  impl<'a> EventStore<'a> {
      /// 成功軌跡をスキル候補として抽出
      pub fn extract_successful_trajectories(
          &self,
          min_tool_success_rate: f64,  // 0.8
          min_steps: usize,              // 3
      ) -> Result<Vec<TrajectoryCandidate>> { ... }
  }

  pub struct TrajectoryCandidate {
      pub session_id: String,
      pub task_description: String,
      pub tool_sequence: Vec<String>,
      pub success_rate: f64,
      pub duration_ms: u64,
  }
  ```
- **スキル昇格フロー**: TrajectoryCandidate → `memory/skill.rs` のpromote_to_skill → SKILL.mdエクスポート
- **期待効果**: ゼロショット知識転送の自動化（Karpathyパターン強化）
- **影響範囲**: event_store.rs（拡張）+ memory/skill.rs + knowledge/vault.rs
- **テスト**:
  - 擬似セッション投入→軌跡抽出（成功/失敗フィルタ）
  - スキル自動昇格の決定性
  - 重複スキル回避

### ✅ P2 Step 6: ベンチマーク22→40+タスク拡張（完了、ADK Phase C / 項目163 / +TaskTag::Smoke 5タスク化 項目167）

- **現状**: `src/agent/benchmark.rs` 1454行、22タスク
- **追加タスク候補**（18件）:
  - マルチファイル編集: 同一変数リネーム3ファイル / 関数シグネチャ変更4ファイル
  - 長期タスク: 10ステップ連続ツール呼出 / 50ステップ実装タスク
  - ツール連携: RepoMap→Read→Edit→Test の連鎖 / Grep→MultiEdit の連鎖
  - エラー回復: ツール失敗3回後の別手段移行 / 破損ファイル読取→修復
  - MCP連携: HTTP MCP経由の外部API呼出（モック）
  - セマンティック必須: 曖昧な要求（「ログを改善」）からの具体化
- **期待効果**: Lab評価の統計信頼性 ±0.01 → ±0.005以下
- **影響範囲**: benchmark.rs + experiment.rs（タスクタグsmoke/full追加）
- **テスト**:
  - タスク決定性（同seed再実行でスコア一致）
  - tag="smoke"（5タスク）／"full"（40タスク）の切替動作

### ✅ P2 Step 7: agent_loop.rs モジュール分割（完了、項目164 — 9 ファイル分割、最大 315 行、commit 72d969d〜3019cb6）

- **現状**: 前回tool_exec.rs/context_inject.rsに分離済みだが2497行残
- **分割設計**:
  ```
  src/agent/agent_loop/
  ├── mod.rs              (200行、run_agent_loop公開API)
  ├── core.rs             (700行、メインループ+LoopState管理)
  ├── step.rs             (600行、execute_step+推論呼出)
  ├── outcome.rs          (500行、handle_outcome+OutcomeAction)
  └── pipeline.rs         (500行、middleware統合+pipeline stage注入)
  ```
- **注意点**:
  - pub使用箇所（benchmark.rs/experiment.rsからの呼出）を維持
  - clippy auto-fix巻戻しリスク → 分割後即commit + CLAUDE.md警告追加
- **期待効果**: 保守性向上、テスト実行時の認知負荷低減
- **テスト**: 既存892テスト全通過確認（挙動不変）

### 🔵 P3 Step 8: 依存関係最適化（評価書あり: `.claude/plan/step-8-dependency-eval.md` — **軽量実施推奨**、ureq 重複削減のみ）

- **現状調査要点**:
  - HTTPクライアント: ureq（llama-server通信） vs reqwest（web_fetch, HTTP MCP）の重複
  - tree-sitter言語パーサ: Rust/Python/TS/JS/Go 5個の必要性確認
  - fastembed: embeddings featureフラグで既制御済
- **変更方針**:
  - reqwestに統一検討（ただしureqはブロッキング+native-tlsで安定、慎重に判断）
  - tree-sitter-javascriptはTSパーサと重複 → 削除検討
  - 未使用featureフラグ削除（cargo machete / cargo-udeps）
- **期待効果**: ビルド時間 -20%, バイナリ -10%
- **影響範囲**: Cargo.toml + 該当import箇所
- **テスト**: cargo build全通過+既存892テスト

### 🔵 P3 Step 9: テストカバレッジ強化（設計書あり: `.claude/plan/step-9-coverage-design.md` — cargo-llvm-cov + 50テスト案、推定 9h）

- **現状**: 892テスト、カバレッジ未計測
- **実施手順**:
  1. `cargo tarpaulin --out Html` で計測
  2. 80%未満モジュールを特定（想定: observability/runtime/safety一部）
  3. 優先度高モジュールに+50テスト
- **期待効果**: リグレッション耐性向上、リファクタ安心感
- **影響範囲**: 各モジュール tests
- **テスト**: カバレッジレポート（HTML）

---

## Key Files

| File | Operation | Description | Priority |
|------|-----------|-------------|----------|
| (Lab実行のみ) | Execute | `cargo run -- --lab` バックグラウンド | Step 0 |
| src/agent/subagent.rs:148 | Modify | 順次→rayon par_iter | P0 |
| src/agent/event_store.rs | Extend | extract_successful_trajectories API | P1 |
| src/memory/skill.rs | Extend | 軌跡→スキル自動昇格 | P1 |
| src/agent/benchmark.rs | Extend | 22→40+タスク + smoke/fullタグ | P2 |
| src/agent/agent_loop.rs | Refactor | 2497行 → 4モジュール分割 | P2 |
| Cargo.toml | Cleanup | ureq/tree-sitter整理 | P3 |
| (各モジュール) | Tests | カバレッジ80%到達 | P3 |

---

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| Lab v11が長時間化 | ScheduleWakeup定期ポーリング（10-20分間隔）+ TaskOutput非ブロッキング |
| rayon並列でSQLite同時書込エラー | MemoryStore::Connection per-task分離＋書込はsession_id別 |
| 軌跡抽出で低品質スキル混入 | 3シグナルスコア（成功率+頻度+経過時間）しきい値保守的 |
| 40タスクでLab実行時間倍増 | smokeタグ（5タスク）でプレ評価→fullは承認候補のみ |
| agent_loop.rs分割でpub API破綻 | 分割前後でcargo test+cargo run --lab smokeで挙動確認 |
| ureq→reqwest置換でllama-server接続不安定化 | 段階移行（web_fetchから開始、llama-serverは最後） |
| tree-sitter削除で既存RepoMapテスト破綻 | 削除前に依存グラフを`cargo tree`で確認 |
| clippy auto-fixによる分割ファイル巻戻し | 各分割直後にgit commit、CLAUDE.md「巻戻し禁止」明記追加 |

---

## 実施順序（推奨）

```
今すぐ: Step 0 (Lab v11バックグラウンド起動) — ~2-4時間
        ↓ 並行してP0 Step 2を実装可能

Phase A: P0 Step 2 (rayon並列) → Lab v11結果確認
        目標: 独立サブタスク2-4倍、Lab v11でscore+0.005以上

Phase B: P1 Step 5 (軌跡抽出→スキル昇格) → Lab v12
        目標: SKILL.mdエントリ自動生成確認、再利用率向上

Phase C: P2 Step 6 (ベンチマーク40タスク) + Step 7 (agent_loop分割)
        Step 6完了後にLab v13で標準偏差計測、±0.005以下を目標
        Step 7は独立、並行作業可能

Phase D (Backlog): P3 Step 8-9
        Step 8: cargo-udepsで未使用depsリストアップ後に慎重実施
        Step 9: tarpaulinで測定後に優先度決定
```

---

## Success Metrics

- **Step 0 (Lab v11)**: ベースライン更新、構造改善効果 score +0.005-0.015
- **P0 Step 2**: 独立サブタスク実行時間 -50%以上（3件並列）
- **P1 Step 5**: SKILL.md自動生成件数（成功セッション10件あたり≥2スキル）
- **P2 Step 6**: Lab評価標準偏差 ±0.01 → ±0.005
- **P2 Step 7**: agent_loop.rs各モジュール <800行、全テスト通過
- **P3 Step 8**: ビルド時間 -20%、バイナリサイズ -10%
- **P3 Step 9**: 全モジュールカバレッジ ≥80%

---

## 注意事項（CLAUDE.mdから継承）

- **Edit/Write後のclippy警告を理由とした巻戻し禁止** — 特にagent_loop.rs分割作業では必ず即commit
- **大量変更時はPython subprocess+即git commit** — 分割時に活用
- **TDD原則**: Red→Green→Refactor、テスト先行必須
- **Lab実行は非ブロッキング**: `run_in_background: true`で起動後、他タスクと並行

---

## SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: (not invoked — role prompt `analyzer.md` not present; used Read/Grep-based analysis)
- GEMINI_SESSION: (not invoked)

---

## 備考

本v2プランはv1完了分（項目156/158/159）を差し引いた残タスク＋未実行のLab v11測定を統合。dual-model analysis（Codex/Gemini）は役割プロンプト`analyzer.md`が未配置のため省略し、既存コードベースの直接調査ベースで作成。
