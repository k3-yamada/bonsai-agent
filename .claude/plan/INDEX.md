# `.claude/plan/` インデックス

19 ファイルの状態と関係を一覧化。新規セッションで最初に参照する起点。

**Last Updated:** 2026-04-27

---

## 🔥 アクティブ（今後の作業の起点）

| ファイル | 役割 | 状態 |
|---|---|---|
| `post-lab-v13-roadmap.md` | **マスタープラン** — Lab v13 完了後の構造改善 v3 全体計画 | 🔥 起点 |
| `lab-v13-config-draft.md` | Lab v13 起動設定（[experiment] セクション） | 🔄 実行中 |
| `structural-improvements-v2.md` | Step 0-9 全体ロードマップ（Step 7 までは ✅ 完了マーク済） | 📊 状態管理 |

## 📐 構造改善 v3 詳細設計（Lab v13 完了後実装）

| ファイル | 候補 | 工数 | 採否ゲート |
|---|---|---|---|
| `diffstore-rust-impl.md` | A: DiffStore (★★★) | 4h | Lab v13 で diff 平均 150+ tok |
| `edit-cycle-detector-impl.md` | B: Edit Cycle (★★) | 1h | Lab v13 で同ファイル交互編集 REJECT |
| `fallback-chain-impl.md` | C: Fallback Chain (★★) | 3h | Lab v13 で MLX 接続断 2+ 回 |
| `step-8-dependency-eval.md` | D: 依存最適化 | 1h | 軽量実施推奨 |
| `step-9-coverage-design.md` | E: テストカバレッジ | 9h | 950→1000 テスト |

## 📚 知見集約・判定ドキュメント

| ファイル | 内容 | 状態 |
|---|---|---|
| `macos26-agent-learnings-v2.md` | macOS26/Agent 8 ファイル分析、3 候補抽出 | 📚 参照 |
| `phase-d-evaluation.md` | ADK Phase D YAGNI 判定 = 見送り | 📚 参照 |
| `phase-b2-judge-gate.md` | ADK Phase B2 設計（実装済） | 📚 履歴 |
| `agent-loop-split-validated.md` | agent_loop 分割設計検証（実装済） | 📚 履歴 |
| `lab-v11-accept-analysis.md` | Lab v11 ACCEPT 詳細（defaults 化見送り） | 📚 履歴 |
| `lab-v12-accept-analysis.md` | Lab v12 ACCEPT 詳細（temperature 0.7 推奨） | 📚 履歴 |

## 🗄 古い計画（参照のみ、新計画でカバー済）

| ファイル | 状態 | 後継 |
|---|---|---|
| `adk-integration.md` | ADK 取込ロードマップ（A-D） | DESIGN_SPEC.md + post-lab-v13-roadmap.md |
| `continuation-2026-04-25.md` | Lab v11 後継作業 | 項目160-163 で対応済 |
| `next-actions-2026-04-25.md` | Lab v12 並行作業候補 v1 | v2 に置換 |
| `next-actions-2026-04-25-v2.md` | Lab v12 並行作業候補 v2 | post-lab-v13-roadmap に統合 |
| `phase-c-and-refactor-draft.md` | Phase C + 分割草稿 | 項目163-164 で完了 |

## 関係マップ

```
post-lab-v13-roadmap.md (★ 起点)
├── Phase 1: lab-v13-config-draft.md → 結果分析
├── Phase 2: 構造改善 v3
│   ├── diffstore-rust-impl.md (★★★)
│   ├── edit-cycle-detector-impl.md (★★)
│   └── fallback-chain-impl.md (★★)
├── Phase 3: 品質強化
│   ├── step-8-dependency-eval.md
│   └── step-9-coverage-design.md
└── Phase 5: 知見継続
    ├── macos26-agent-learnings-v2.md
    └── phase-d-evaluation.md (再評価ゲート)

structural-improvements-v2.md ← 全体俯瞰（Step 0-9 状態管理）
```

## メンテナンス方針

- 新規 plan 作成時、本 INDEX に行を追加
- 完了/置換時、状態を 🔥/🔄/📚/🗄 で更新
- 90 日以上未参照の 🗄 ファイルは memory に集約検討

## 関連

- 上位: `docs/DESIGN_SPEC.md`（章 7 で本ディレクトリ参照）
- メモリ: `memory/MEMORY.md`（個別知見ファイル索引）
