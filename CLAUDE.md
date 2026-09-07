# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.
General autonomous agent guidelines are maintained in [AGENTS.md](AGENTS.md).

## プロジェクト概要

`bonsai-agent` — Bonsai-8B（1ビット量子化Qwen3-8B、1.28GB）で動作するRust製自律型エージェント。
Mac M2 16GB上でllama-server HTTP API経由で推論。1486 unit test (2026-06-07 時点)、109 ソースファイル。

設計原則: **「Scaffolding > Model」** — 1ビットモデルの改善余地は限定的。ハーネス側で信頼性を底上げする。

## ビルド・テスト・実行

→ [docs/execution/runbook.md](docs/execution/runbook.md) (env 一覧 + Smoke G-RT2 起動例含む)

## アーキテクチャ + 主要トレイト

→ [docs/architecture/overview.md](docs/architecture/overview.md) (src/ tree + trait 群 + 設計原則)
→ [docs/architecture/module-layer-rules.md](docs/architecture/module-layer-rules.md) (Z-4 layer linter rule source)

## ハーネスパターン

p^n問題（ステップ蓄積による失敗確率指数的増大）への対策。**項目 1-263 は `~/.claude/projects/-Users-keizo-bonsai-agent/memory/harness_patterns_archive.md` (Claude Code session memory) に verbatim アーカイブ** (264-268 も archive 既収載、CLAUDE.md は直近 5 項目のみ 1 行サマリー保持)。CLAUDE.md は索引 + デフォルト化済み変異 + 直近 5 項目のみ保持。

### カテゴリ索引（archive 参照用）

**Core 機構**
- Core ハーネス: 項目 1-10（pass^k / Continue Sites / 2層 LoopDetector / StallDetector / fuzzy / AI+Tool ペア保護 / Deferred Schema / SOUL.md / StepOutcome / 計画強制）
- Tool / RepoMap: 項目 11, 30, 70, 74, 75, 100, 101, 119, 148
- Compaction / Context: 項目 6, 12, 41, 46, 78, 81, 82, 158, 159, 178, 187, **265** (max_context smoke/env override)
- Backend / Inference: 項目 35, 36, 49, 53, 56, 60, 61, 63, 67, 90, 103, 105, 130, 167, 168, 174, 195, 198
- Checkpoint / Audit / Logger: 項目 25-29, 38, 39, 109, 110, 152, 175
- Advisor / Critic: 項目 15-24, 89, **226** (G1 Critic 別 LLM、env-gated)
- MCP: 項目 102, 108, 124, 132-135, 137, 180, 182
- Safety / Filter / Anti-Halluc: 項目 42, 43, 44, 47, 50, 51, 88, 95, 96, 175, 178, **230/234/235/237** (KG-FactCheck Plan A 系列), **239** (regex dash fix)
- Subagent: 項目 120, 160

**Memory / Knowledge**
- Memory / Knowledge 基盤: 項目 13, 71, 76, 77, 80, 83, 84, 106, 161, 162, 177, 179
- Cerememory 三本柱: **217** (power-law decay) / **218** (ReviewState V12 freshness) / **219** (Working Memory 7±2 cap)
- AgentHER hindsight: 項目 201-205
- Graph fusion (paper agentmemory R@10=98.6 達成): **228**
- sqlite-vec: **220** (Step A 採用 + Step B REJECT) → **221** (Lab REJECT) → **222** (wiring removal)

**Lab 実験基盤 / Benchmark**
- Lab 実験基盤: 項目 107, 123, 125, 131, 138-145, 173, 184, 185, 188-198
- Beyond pass@1 RDC/VAF/GDS: **200**
- PASS@(k,T) 二軸 metric: **225**
- AgentFloor 6-tier ladder (T1-T6): **223/224** (Bonsai-8B 能力プロファイル: T1=0.68/T2=0.52/T3=0.77/T4=0.64/T5=0.70/T6=0.47、weakest=T6)
- LongMemEval-S 移植 + 500Q baseline (R@5=0.91): **227**
- Frontier Benchmark (context-length axis): **229/231/232/233** (第 6 軸 baseline 確立、bucket 0→1 gradient = -0.1944 ACCEPT)
- Paired evidence REJECT 系列 (unpaired ACCEPT 覆): **266** (D-2 MEMORY_AUG dz=-10.60) / **268** (BUDGET=1 dz=-0.86) / **267** (case B 削除 paired-driven cleanup pattern)
- Refactor / Quality: 項目 64-66, 82, 92-94, 146-156, 164-166, 209 (EventRepository trait 化)

### デフォルト化済み変異（Lab ACCEPT → 恒久適用）

- 項目 10: 計画強制ルール（Lab v6.2 唯一の ACCEPT）
- 項目 47: ツール使用前 `<think>` で意図記述（+0.032 実証）
- 項目 50: フォールバック戦略（+0.001 実証）
- 項目 136: 回答前ファイル内容確認（Lab v9 +0.0157 実証）

### 直近 5 項目 (詳細は archive 参照)

> **運用ルール**: 6 項目目追加時は最古 1 件を `harness_patterns_archive.md` に flush (FIFO)、本 section は常に直近 5 項目に保つ。新規追加 entry は 1 行 (200-400 字目安) に圧縮し改行禁止。詳細展開は archive verbatim を参照。

- **265**: 🎉 max_context_tokens 縮小 (smoke/env override) Phase 1-3 完遂 (TDD strict 3 commit + middleware wiring、SMOKE_DEFAULT_MAX_CTX=6000 const + 3 段優先順位 `BONSAI_LAB_MAX_CTX` env > `BONSAI_LAB_SMOKE=1` > default 14000) + G-MCT2 Phase 4 Smoke (62.7 min wall、score 0.8209 / pass@k 1.0 / T6=0.82、compaction.budget emit 49 件全 total=6000 完璧反映) + **構造的 finding 決定的解明**: production smoke k=3 baseline 各 iteration は独立 session で context reset → level1 never fires、max_context=14000 でも 6000 でも prune accumulate しない構造設計に依拠、項目 263 ACCEPT +9.5% は H-A4 noise 確定
- **266**: 🚨 D-2 (MEMORY_AUG) DEFINITIVE paired evidence REJECT (`scripts/g_paired_265_v2.sh` 4 paired complete、pair 1-4 Δ=-0.1443/-0.1488/-0.1412/-0.1194、**mean Δ=-0.1384 / Cohen's dz=-10.60 / 全 B<A by 0.12-0.15** statistically overwhelming)、H-A3 (1bit precision floor で dual augmentation attention dispersion) 仮説支持 + `scripts/lab_v22_metric.py::judge_accept_v22` KeyError bug fix 副次成果 (factcheck OFF paired graceful 化)、`BONSAI_T6_MEMORY_AUG` env default OFF 確定 + infrastructure 削除 (case B) を ROI 最高 deletion candidate に格上げ
- **267**: 🎉 D-2 case B 削除完遂 (paired evidence REJECT 後の infrastructure 撤去、commit `df135df`、**~-466 LOC** = `src/agent/t6_memory_aug.rs` 全削除 + `benchmark.rs` 3 wiring site + 6 test + runbook env table 1 行) + 1378→1372 passed 退行ゼロ + clippy let_and_return follow-up fix + structural test 4 passed + **paired-evidence-driven cleanup pattern 確立** (Codex Harness "Scaffolding > Model" 原則準拠)、Track A (g_paired_263_v2 ~13h smoke) との並列実行で release binary 不変・干渉ゼロ設計確証
- **268**: 🚨 263 ratio tune (BUDGET=1) DEFINITIVE paired evidence REJECT (`scripts/g_paired_263_v2.sh` 5/5 paired complete、pair 1-5 Δ=-0.1873/-0.1037/+0.0000/-0.0513/+0.0007、**mean Δ=-0.0683 / Cohen's dz=-0.86 medium-large** / Wilcoxon p=0.9375 destructive 方向)、**戦略的 implications**: unpaired ACCEPT +9.5% 完全覆 (cherry-picked noise 確証) / H-A4 noise floor ~5% 超過で真 destructive code path 仮説支持 (BudgetRatios overhead / allocated 伝播 path 軽微 latency or branch divergence 仮説) / Phase 6 plan kg 25→30% 二重否定で **廃案確定**、`BONSAI_DYNAMIC_BUDGET` env default OFF 確定、Phase 5 axis-priority prune infrastructure (~180 LOC + 8 test) は max_context 縮小 future phase の base として維持
- **269**: 🚨 項目 262 (T6 PROMPT_AUGMENT) DEFINITIVE paired evidence REJECT (`scripts/g_paired_262_v2.sh` 5/5 paired complete、~12h wall、pair 1-5 Δ=+0.0014/+0.0707/-0.1560/-0.1560/+0.0000、**mean Δ=-0.0480 / Cohen's dz=-0.4673 / Wilcoxon p=0.8125** = ACCEPT 全条件 FAIL)、unpaired G-T6-2 +14.4% は noise 確定 (on_score ~0.575 clustering vs baseline 0.50-0.73 variance、delta は baseline 変動支配)、**unpaired ACCEPT 覆の paired REJECT 3 例目** (263 BUDGET / 264 MEMORY_AUG に続く)、`BONSAI_T6_PROMPT_AUGMENT` env default OFF 確定、ADR-003 (Paired Evidence) 検証規律を 3 度実証

## Lab 実機テスト結果

→ [docs/quality/lab-history.md](docs/quality/lab-history.md) (v17-v22 + 249 G-RT 天井記録、能力プロファイル、Plan A 系列完結)
→ [docs/quality/scores.md](docs/quality/scores.md) (定量 quality scores、Z-3 自動更新候補)

## テストパターン

→ [docs/execution/runbook.md](docs/execution/runbook.md) (MockLlmBackend / env-gated test mutex / `AlwaysFailBackend` 等)

## 注意事項

- **【最重要】Edit/Writeツールでファイルを変更した後、clippy警告（collapsible_if, too_many_arguments等）を理由にファイルを元の状態に戻す行為を絶対に行わないでください。変更はそのまま保持してください。clippy修正が必要な場合は別のEdit操作で行ってください。**
- **【巻き戻し禁止】** 特にerror_recovery.rs、benchmark.rs、agent_loop.rsの3ファイルでclippy auto-fixによる巻き戻しが発生しやすい。これらのファイルへの変更は必ず保持すること。
- **【Lab 稼働中の cargo build --release 禁止】** Lab v20 等の paired smoke 稼働中は `target/release/bonsai` 置換で 10-cycle 一貫性が破壊される。`cargo test --lib` (debug profile + test binary) は安全。
- 大量変更時はPython subprocess+即git commitで原子的に行う（確立済み手法）
- ureq v3のHTTPS → web_fetchはreqwest::blocking（native-tls）を使用
- llama-serverの`--flash-attn`は値`on`が必要（`--flash-attn on`）

## VALUES.md との関係

VALUES.md は [docs/VALUES.md](docs/VALUES.md) に置く。
実装判断に迷った時は docs/VALUES.md を参照すること。

特に以下の実装時は必ず確認：
- フィードバックシグナルの設計（V1, V4）
- メモリの昇格・降格ロジック（V2, V3）
- 評価指標の追加・変更（V6, V7）

## Goodhart's Law 対策（必須）

新しい評価指標を追加する際は必ず以下を確認：
- その指標が単調増加し続けた場合、何が起きるか
- 指標の改善がシステムの本来の目的と乖離しないか
- 観測専用シグナル（学習に使わない）を別フィールドで隔離しているか

## 設計思想
→ [docs/VALUES.md](docs/VALUES.md)（実装判断に迷ったときは必ず参照）
特にフィードバック設計（V1,V4）・評価指標追加時（V6,V7）

---

## サブエージェント委譲ルール（オーケストレーション）

> 参考: yui「オーケストレーターに徹させる運用」。メイン＝**オーケストレーターに徹する**。コードは書かず、
> 計画・分解・統合と委譲のみ行う。

### 絶対規則（恒久・使用モデルによらず適用）
- メインエージェントは**恒久的にオーケストレーターに徹する**。実装・テスト実行・ファイル編集を自分で行わず、**必ず**サブエージェントへ委譲する。単純作業に見えても自己判断で直接手を出さない。
- **委譲時は必ずタスクに適したモデルとスキルの両方を明示すること。** どちらか一方でも省略した委譲は禁止。
  - モデル: 下記ワークフローの指定（scope-planner/architect/qa-reviewer=opus、builder/shipper=sonnet）に従う。表にない新規タスクは難度で判断（設計判断・高リスク=opus、標準実装=sonnet、軽量作業=haiku）。
  - スキル: 委譲先に、そのタスクで使うべきSkillツールのスキル名を具体的に指示する（例:「rust-specialistへ委譲。skill: rust-idiomatic」）。スキル選定をエージェント任せにしない。
- **メインオーケストレーター自身のモデルはSonnet 5を推奨。** Opus/Fableではない。理由: メインの仕事は委譲判断・進捗管理・結果統合であり、深い設計判断は既にopus指定のサブエージェント（architect等）に委譲される設計のため、メイン自身が高単価モデルである必要はない（Fable 5はOpusの2倍単価、雑談・進捗確認等の軽いターンにまで課金されコスト効率が悪い）。
- **Fable（`claude-fable-5`）はメインモデルにしない。** 通常のOpus級サブエージェント（architect等）でも力不足と判断される、突出して難度が高い個別タスク1件に限り、そのサブエージェント呼び出し1回に`model: fable`を指定してエスカレーションする（例: 極めて複雑なアーキ判断、成果物完成時の最終レビュー）。乱用しない——Anthropic報告値でも「Fable常時使用」は同品質の代替構成に対し2倍以上のコストになる。

### コードレビュー・セキュリティレビューのコスト最適化（2026-09-04導入）
上司フィードバック: 編集の都度発火するコードレビュー・セキュリティレビューがopus固定だと高頻度作業でコストが辛い。**一次実行をAntigravity CLI（`agy`）に委ねてコストを下げる。**
- **コードレビューの一次実行**: `agy -p "<レビュー観点>" --model gemini-3.8-flash-high`（専用レビューエージェント未整備のため無指定）
- **セキュリティレビューの一次実行**: 同上
- 既定モデルは `gemini-3.8-flash-high`（安価・高速）。精度不足を感じたら `gemini-3.1-pro-high` に上げてから判断する。
- **Claude opus の `code-reviewer` / `security-reviewer` へのエスカレーションは以下に限定する**（`~/.claude/rules/common/security.md` のSTOPトリガーに準拠）:
  - 認証・認可、決済、暗号処理、DB操作、外部API呼び出し、個人情報等の高リスク領域を触っている
  - Antigravity側の一次レビューでCRITICAL/HIGH相当の指摘が出た、または判断に自信が持てない
  - アーキテクチャ変更を伴う
- push / 本番デプロイ前の最終ゲートは、上記条件に関わらずClaude opusでの最終レビューを推奨する（コストより正確性を優先する局面のため）。
> 実装の詳細でコンテキストを汚さないことで、長時間セッションでも判断がブレない。

### ワークフロー（依頼を受けたら）
1. **scope-planner**（opus）— 要件確定・スコープ削り・受入条件の明文化
2. **architect**（opus）— 設計判断・builderへの割当（高リスク/非可逆な変更時。小さな変更はスキップ可）
3. **builder**（sonnet, 技術スペシャリスト）— 実装
4. **qa-reviewer**（opus）— 実装者と独立した検証（自己LGTM禁止）
5. **shipper**（sonnet）— qa-reviewerのApprove後にコミット等の出荷作業

### この環境の builder（実装担当）
- `rust-specialist` — crate実装/tokio/エラー設計/clippy/テスト

### 追加レビュー観点（必要に応じ qa-reviewer と並行起動）
- （専用レビュアー未配置。qa-reviewer が code-review-expert / security-review スキルで代替）

### 委譲の原則
- 各エージェントの `description` を見て最適な担当へ振る。境界ケースは scope-planner / architect に相談してから割り当てる。
- **実装と検証は必ず別エージェント**（builderに自己承認させない）。
- サブエージェントは各自 **Skillツール** で該当スキルを起動してよい（例: `flutter-*`, `dart-*`, `rust-idiomatic`, `clean-architecture`, `code-review-expert`, `security-review`, `japanese-tech-writing` 等）。各エージェント定義の「活用するスキル」節を参照。
- push / 本番デプロイ / リリースは**上司の明示指示があるまで実行しない**（承認ゲート）。
- 工数が小さい単純作業は 1〜4 を簡略化し、builder→qa-reviewer の2段でよい。
### 既存の共有エージェント（グローバル `~/.claude/agents`）の活用
オーケストレーターは、上記チームに加えて以下の既存エージェントも状況に応じて**併用**してよい（必須段ではなく補助レンズ）。
- `researcher` — 一次情報の収集・競合/技術/企業調査（scope-planner・architect の前工程）
- `oh-my-claudecode:document-specialist` — 外部SDK/API/フレームワークの公式ドキュメント確認（architect の設計裏付け）
- `security-auditor` / `oh-my-claudecode:security-reviewer` — セキュリティ監査（qa-reviewer の補強。認証/権限/入力/決済/機密に触れる変更時）
- `code-refactorer` — 機能を変えない構造改善・重複除去
- `oh-my-claudecode:critic` — 高リスク・非可逆な計画/設計のセカンドオピニオン（architect の設計を別視点で叩く）
- `reporter` — 作業サマリ・日報の集計

> 使い分け: 「自プロジェクトの team（scope-planner/architect/builder/qa-reviewer/shipper）」を主軸に、上記の汎用エージェントは調査・監査・別視点が要る局面でオーケストレーターが差し込む。委譲先が曖昧なら description で判断する。

## ツール選択ルール

このプロジェクトには4つのコード解析ツールが入っている（Serena / code-review-graph / better-code-review-graph / Graphify）。

**Serenaのrust-analyzerは `.serena/project.local.yml` で意図的に無効化している**（コメント: 「Serenaのrust-analyzerを無効化（Write/Editの巻き戻し防止）」）。したがって `find_symbol` / `get_symbols_overview` / `find_referencing_symbols` / `find_implementations` / `rename_symbol` 等のSerenaのシンボル系ツールは**使用不可**（呼び出すとエラーになる）。**この無効化設定を誤って戻さないこと。** 有効化が必要になった場合は上司に確認する。Serenaの `read_memory` / `write_memory` / `list_memories` 等メモリ系ツールは言語サーバーに依存しないため通常どおり利用可能。

代わりに、シンボル解析はすべて言語非依存の better-code-review-graph / Graphify で行う。better-code-review-graph はアクション型の統合ツール構成で、`query` / `review` / `security` に `action` パラメータを渡して使う。

| 質問の種類 | 使うツール | 例 |
| :--- | :--- | :--- |
| 意味・目的でコードを探す | better-code-review-graph `query`（`action=search`） | 「認証してる処理ある？」「エラーハンドリングどこ？」 |
| 変更の影響範囲 | better-code-review-graph `query`（`action=impact`） | 「この変更どこに影響する？」「blast radiusは？」 |
| 呼び出し元・依存の追跡、定義元・実装クラス | better-code-review-graph `query`（`action=query`、pattern=callers_of / definitions 等） | 「〇〇の呼び出し元は？」「〇〇の定義元は？」 |
| リポジトリ全体の構造・横断質問 | `graphify query` | 「全体構成は？」「設計書とコードの関係は？」 |
| コード変更のレビュー | better-code-review-graph `review` | 「直近の変更をレビューして」 |
| セキュリティスキャン | better-code-review-graph `security`（`action=scan`） | 「脆弱性ない？」「セキュリティチェックして」 |
| リネーム・シンボル単位の編集 | Read で該当箇所を特定した上で Edit | 「関数名を変えたい」「メソッドの中身を書き換えて」 |
| 設定ファイル・ドキュメント・非コードファイル | Read / Grep | いずれのツールの解析対象外 |

### 競合回避
- **code-review-graph のツールは質問に使わない**（pre-commitフックでのレビュー用コンテキスト供給が専任）
- 検索・影響範囲・レビューは code-review-graph と better-code-review-graph で機能が重複するが、**better-code-review-graph 側を使う**

### フォールバック
better-code-review-graph の `query`（`action=search`）が0件 → `graphify query` で広域検索 → Grep / Glob / Read。

### グラフを最新に保つ
| ツール | 更新方法 |
| --- | --- |
| code-review-graph | 自動（pre-commitフック + 編集時フック） |
| better-code-review-graph | **自動**（pre-commitフックで `graph build`＋`graph embed` を実行、2026-09-03導入）。手動が要るのは大規模リファクタ直後のフルリビルドのみ（`graph build --full-rebuild`） |
| Graphify | **自動**（`graphify hook install` のpost-commit/post-checkoutフック、2026-09-03導入。バックグラウンド実行でコミットをブロックしない）。手動が要るのはレポート再生成のみ（`graphify cluster-only --no-label`） |

**注意（このリポジトリ固有）**: `git config core.hooksPath` が `/Users/keizo/bonsai-agent/.git/hooks`（このリポジトリ外の絶対パス）を指している。旧配置場所からの移動時の設定残骸と思われる。上記フックの実体は `~/Dev/rust/bonsai-agent/.git/hooks/` ではなくそちらに存在する。フック関連のトラブル時は `git config core.hooksPath` を先に確認すること。
