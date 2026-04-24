# Implementation Plan: 構造的改善ロードマップ（Post Lab v10）

## Task Type
- [x] Backend (Rust / harness-level structural changes)
- [ ] Frontend
- [ ] Fullstack

## 背景

Lab v8-v10の3連続で**実質全REJECT**（v10は9実験中1件のみ+0.003の微増）。
プロンプト変異は飽和 → 次の改善軸は**ハーネスの構造的変更**が必要。

ベースライン: score=0.8087, pass@k=0.8939（v10計測値）

## Technical Solution

優先度順に7カテゴリ・9候補の構造的改善を提案。**P0（安全性+並列性）→ P1（アルゴリズム）→ P2（測定基盤）→ P3（保守）**の順で段階的に実施し、各段階後にLabを回して効果測定する。

---

## Implementation Steps

### 🔴 P0: 緊急度最高（安全性+並列性）

#### Step 1: middleware.rs unsafe→Arc<Mutex>化
- **現状**: `src/agent/middleware.rs:122-127` でraw pointerによるlifetime extension、UAF（Use-After-Free）リスク
- **変更**: `Arc<Mutex<LoopState>>`に置換、スレッド安全性確保
- **期待効果**: クラッシュ耐性向上、並列実行の安全な基盤
- **影響範囲**: middleware.rs全体 + MiddlewareChain統合箇所（run_after_step呼出5箇所）
- **テスト**: 既存ミドルウェアチェーンテスト全通過+並列アクセステスト追加

#### Step 2: subagent.rs rayon並列実行化
- **現状**: `src/agent/subagent.rs:148` でサブタスク順次実行、深度制限2
- **変更**: 独立サブタスクは`rayon::iter::par_iter()`で並列化、依存あるものは逐次保持
- **期待効果**: 3-4x speedup（マルチコアM2活用）
- **影響範囲**: subagent.rs + SubAgentExecutor呼出元
- **テスト**: サブタスクベンチマーク追加（単一/並列/混合パターン）

### 🟡 P1: 効果期待（アルゴリズム改善）

#### Step 3: ツール選択のembedding-based化
- **現状**: `src/tools/mod.rs:284-310` select_relevant/select_relevant_with_type はキーワードマッチ
- **変更**: fastembed統合（既にembedder.rs存在）、ツール説明ベクトル×タスククエリベクトルのcos類似度でtop-k選択
- **期待効果**: ツール選択recall 85→95%、max_tools_in_context=8の精度向上
- **影響範囲**: tools/mod.rs + embedder.rs + ToolRegistry初期化
- **テスト**: tool_selection_bench.rs拡張（16→30ケース）で精度比較

#### Step 4: セマンティック重要度スコアリング（コンパクション）
- **現状**: `src/agent/compaction.rs` 固定重要度（User=1.0〜Toolエラー=0.2）
- **変更**: embedding類似度で「現在タスクとの関連度」を重要度に反映、動的スコア計算
- **期待効果**: 長タスク時の情報保持率向上、不要文脈の積極削除
- **影響範囲**: compaction.rs（level2/level3アルゴリズム）
- **テスト**: 長タスクベンチマーク追加（50ステップ超）

#### Step 5: Event Sourcingからの軌跡抽出
- **現状**: `src/agent/event_store.rs` 統一イベントストリームは存在するが、学習用抽出なし
- **変更**: 成功軌跡（TaskComplete + tool_success_rate>0.8）からスキル候補を自動抽出、SKILL.mdに昇格
- **期待効果**: ゼロショット知識転送の自動化、再現性向上
- **影響範囲**: event_store.rs + memory/skill.rs + knowledge/vault.rs
- **テスト**: 軌跡→スキル変換ユニットテスト

### 🟢 P2: 測定基盤（効果測定精度の向上）

#### Step 6: ベンチマーク22→40+タスク拡張
- **現状**: `src/agent/benchmark.rs` 22タスク（rename/git_diff/json_parse等含む）
- **変更**: マルチファイル編集/長期タスク/ツール連携タスク18件追加
- **期待効果**: Lab評価の統計的信頼性向上（現在の±0.01変動幅を±0.005以下へ）
- **影響範囲**: benchmark.rs + experiment.rs評価ロジック
- **テスト**: タスクごとの決定性チェック（同seed再実行で同スコア）

#### Step 7: 大ファイルのモジュール分割
- **現状**: `src/agent/agent_loop.rs` 2497行（既にtool_exec/context_injectに分割済みだが依然大）
- **変更**: 中心ループ + step実行 + outcome handling + middleware統合 に4分割
- **期待効果**: 保守性向上、clippy auto-fixの巻戻しリスク低減
- **影響範囲**: agent_loop.rs + benchmark.rs/experiment.rs呼出元（最小限）
- **テスト**: 既存全通過確認のみ（挙動不変）

### 🔵 P3: 保守性（継続運用）

#### Step 8: 依存関係最適化
- **現状**: Cargo.toml — 複数のHTTPクライアント（ureq/reqwest）、tree-sitter-*多数
- **変更**: reqwestに統一、tree-sitterは使用言語のみ、未使用featureフラグ削除
- **期待効果**: ビルド時間短縮、バイナリサイズ削減
- **影響範囲**: Cargo.toml + 該当コード
- **テスト**: cargo build + 既存テスト全通過

#### Step 9: テストカバレッジ強化
- **現状**: 886テスト、一部モジュール（observability/runtime一部）カバレッジ低
- **変更**: `cargo tarpaulin`計測、80%未満モジュールに+50テスト
- **期待効果**: リグレッション耐性向上、リファクタ安全性
- **影響範囲**: 各モジュールtests
- **テスト**: カバレッジレポート生成

---

## Key Files

| File | Operation | Description | Priority |
|------|-----------|-------------|----------|
| src/agent/middleware.rs:122-127 | Modify | unsafe raw pointer → Arc<Mutex<LoopState>> | P0 |
| src/agent/subagent.rs:148 | Modify | 順次実行 → rayon par_iter | P0 |
| src/tools/mod.rs:284-310 | Modify | select_relevant embedding化 | P1 |
| src/runtime/embedder.rs | Extend | ToolEmbedding初期化統合 | P1 |
| src/agent/compaction.rs | Modify | 動的重要度スコア（embedding類似度） | P1 |
| src/agent/event_store.rs | Extend | 成功軌跡抽出API追加 | P1 |
| src/memory/skill.rs | Extend | 軌跡→スキル自動昇格 | P1 |
| src/agent/benchmark.rs | Extend | 22→40+タスク | P2 |
| src/agent/agent_loop.rs | Refactor | 2497行 → 4モジュール分割 | P2 |
| Cargo.toml | Cleanup | ureq削除/tree-sitter整理 | P3 |

---

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| Arc<Mutex>化でデッドロック発生 | MutexGuardスコープ最小化+既存並列テスト強化 |
| rayon並列化でサブタスク間の状態汚染 | MemoryStore接続分離+read-onlyツール優先並列化 |
| embedding選択で"think"等共通語の類似度が高く精度劣化 | ストップワード除去+TaskType併用でハイブリッド選択 |
| セマンティック圧縮で重要情報の誤削除 | 閾値保守的（0.3以下のみ削除）+削除前ログで監査 |
| ベンチマーク拡張でLab実行時間倍増 | タスクタグで段階的評価（smoke/full）、prescreening活用 |
| agent_loop.rs分割でAPIテスト破綻 | 分割前後でcargo test全通過を必須条件に |
| rayon追加で依存肥大化 | 既存tokio/std::thread活用検討、必要最小限に |

---

## 実施順序（推奨）

```
Week 1: P0完了（Step 1-2）→ Lab v11測定
        目標: score+0.01以上（構造改善の効果確認）

Week 2: P1の一部（Step 3: embedding tool選択）→ Lab v12測定
        目標: ツール選択精度+10%

Week 3: P1残り（Step 4-5）→ Lab v13測定
        目標: 長タスク成功率+5%

Week 4: P2（Step 6-7）→ Lab v14（40タスク）でベースライン再計測
        目標: 統計信頼性向上（標準偏差半減）

Backlog: P3（Step 8-9）
```

---

## Success Metrics

- **Lab v11**: P0完了後、score≥0.82（+0.01以上）
- **Lab v12**: embedding tool選択、tool_success_rate≥0.90
- **Lab v13**: 長タスク成功率（50+ステップ）、現行比+5%
- **Lab v14**: 40タスクベンチでスコア変動幅≤0.005

---

## SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: (not invoked — Explore agent analysis used in lieu of dual-model)
- GEMINI_SESSION: (not invoked)

---

## 備考

本プランはPhase 1（コンテキスト取得）でExplore agent thoroughモードによる包括的コードベース分析を実施し、その結果を基に作成した単一モデル（Claude Opus 4.6 1M）の統合版である。dual-model analysis（Codex/Gemini並列）は省略。
