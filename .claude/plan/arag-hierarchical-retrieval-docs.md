# A-RAG Hierarchical Retrieval Interfaces — bonsai-agent 既存設計の external validation (docs PR)

## メタ情報

- **scope**: docs only (production code 変更ゼロ)
- **見積もり**: ~2-3h、計 0.5 day
- **plan owner**: docs lane (Beyond pass@1 / AgentHER plan と独立、コンフリクトなし)
- **関連 plan ファイル**: 本ファイル単独 (相互参照のみ)
- **trigger 論文**: arxiv 2602.03442 "A-RAG: Scaling Agentic RAG via Hierarchical Retrieval Interfaces" (2026-02)
- **bonsai 関連項目**: 25 / 30 / 71 / 76 / 80 / 106 / 116 / 149 / 157 / 158 / 162 / 179
- **ahead commits**: 0 (docs PR のため、CLAUDE.md / memory/MEMORY.md / arag_alignment.md 追加のみ)

---

## 1. 背景・動機

arxiv 2602.03442 "A-RAG" は agentic RAG を **3 階層 retrieval interface** に taxonomy 化した:

1. **keyword search** — 高速で語彙一致、recall 重視
2. **semantic search** — 埋め込みコサインで意味類似、precision 重視
3. **chunk read** — file/document の特定範囲 (offset/limit) に直接 zoom-in

bonsai-agent は v6.x 以降、独立した設計判断の積み重ねで **偶然この 3 階層と一致する** retrieval primitive を持っていた:

- 項目 search.rs (FTS5 + ベクトル RRF 融合) → Layer 1+2 hybrid
- 項目 71 KnowledgeGraph BFS 双方向探索 → Layer 2 の高度版 (連想記憶)
- 項目 25 / file.rs FileReadTool (offset/limit、構造化出力) → Layer 3
- 項目 30 Vault エントリ参照 → Layer 3 の知識ストック版

本 docs PR の動機は **設計が偶然 A-RAG taxonomy と一致していた事実を external validation として記録** する点にある。「Scaffolding > Model」原則 (項目冒頭設計原則) において、外部論文 framework と一致する設計は ① 設計判断の妥当性裏付け、② 認知負荷低減 (新規参加者が arxiv 1 本で全体像把握可能)、③ 将来拡張時の語彙統一 (Layer 1/2/3 という共通語) という 3 つの恩恵を生む。

production 影響ゼロ (docs only) のため、Lab pass^k regression risk なし、本 docs PR は次セッションで Beyond pass@1 / AgentHER 等の実装系 plan と並列 merge 可能。

---

## 2. マッピング表 (詳細版)

| A-RAG primitive | A-RAG 論文の定義 (2602.03442) | bonsai 実装 | 既存項目 # | 階層 |
|---|---|---|---|---|
| keyword search (lexical) | BM25 / FTS / 語彙完全一致 | `MemoryStore::search_memories` (FTS5)、`shell` ツール経由の grep | 30 / search.rs | Layer 1 |
| semantic search (dense) | embedding cosine + KNN | `Embedder` (AllMiniLML6V2 / SimpleEmbedder fallback) + `HybridSearch::vector_search` | 157 / 158 | Layer 2 |
| graph traversal (associative) | (A-RAG では irreducible に扱われない、bonsai 独自拡張) | `KnowledgeGraph::neighbors` (BFS 双方向、depth 制御) | 71 / 106 | Layer 2.5 |
| hybrid retrieval (fusion) | (A-RAG paper では Layer 1 のフォールバック扱い) | `HybridSearch::rrf_merge` (Reciprocal Rank Fusion、α 重み) | 76 / 149 | Layer 1+2 |
| chunk read (file) | offset/length specified read | `FileReadTool` (`offset` + `limit` + 行番号付与構造化出力) | 25 / 116 | Layer 3 |
| chunk read (knowledge stock) | (A-RAG: passage retrieval) | `Vault::read_category` / `read_rules` / `read_docs_for_context`、`inject_contextual_memories` (`<context type="vault-rules">` / `vault-docs` タグ) | 30 / 76 / 80 / 162 | Layer 3 |
| memory block read | (A-RAG: 構造化 doc 読み込み) | `inject_memory_blocks` (`<context type="block:{label}">`、SOUL.md + `[[memory.blocks]]`) | 179 | Layer 3 |

各 cell の **コード path は `cargo build` reachable で実証済** (G-2 で再確認)。`<context type="...">` タグ統一 (項目 80) によって 注入経路が単一フォーマットで明示されている点は A-RAG の "structured retrieval response" 思想と整合。

---

## 3. アーキテクチャ図 (ASCII)

```
┌─ Layer 3: chunk read ─────────────────────────────────────────┐
│   FileReadTool (offset/limit, 行番号付与)         [項目 25/116] │
│   Vault::read_category / read_rules / read_docs_for_context    │
│                                                    [項目 30/76] │
│   inject_memory_blocks (<context type="block:{label}">)        │
│                                                       [項目 179]│
├─ Layer 2.5: associative (bonsai 独自) ────────────────────────┤
│   KnowledgeGraph::neighbors (BFS 双方向, depth=N)              │
│                                                    [項目 71/106]│
├─ Layer 2: semantic search ────────────────────────────────────┤
│   Embedder (AllMiniLML6V2 ONNX / SimpleEmbedder fallback)      │
│                                                       [項目 157]│
│   HybridSearch::vector_search (cosine + L2 norm)               │
│                                                       [項目 158]│
├─ Layer 1+2: hybrid fusion (bonsai 独自、A-RAG 想定外の高度版) ─┤
│   HybridSearch::rrf_merge (RRF k=60, α 重み)        [項目 76]  │
│   rrf_merge 所有権受取 (clone 除去)                  [項目 149] │
├─ Layer 1: keyword search ─────────────────────────────────────┤
│   MemoryStore::search_memories (FTS5)               [項目 30]  │
│   shell ツール経由 grep / ripgrep                              │
└────────────────────────────────────────────────────────────────┘
                            ▲
            inject_contextual_memories (項目 80, 162)
            <context type="memory" / "experience" / "vault-rules"
                      / "vault-docs" / "skills" / "graph">
```

- **Layer 1+2 の hybrid 段** は A-RAG paper では明示的に位置づけられず、bonsai が独自に強化している点
- **Layer 2.5 (graph)** は A-RAG taxonomy では irreducible primitive として扱われない一方、bonsai の連想記憶 (項目 71) はここに positioning される
- 全 Layer の出力は **`<context type="...">` タグで agent_loop に注入** される (項目 80) ため、Layer 選択の影響範囲が明示的に観測可能

---

## 4. 既存 hybrid 検索の位置付け

### 4.1 `HybridSearch::rrf_merge` (項目 76 / 149)

A-RAG 論文の framework では Layer 1 (keyword) と Layer 2 (semantic) は **別 tool として LLM が選択** する想定だが、bonsai の `HybridSearch` は **両 Layer の結果を Reciprocal Rank Fusion で機械融合** する。

- **A-RAG の想定**: LLM が "keyword search でダメなら semantic search にフォールバック" と判断
- **bonsai の実装**: 両方を常に実行し RRF で statistically merge、α 重みで balance 調整 (default 0.5)

これは A-RAG が "Layer 1 のフォールバック" と位置づける挙動より **一段高度** であり、1bit Bonsai-8B のような Layer 選択判断が不安定なモデルに対して特に有効 (Layer 選択を LLM 判断から外し RRF に委譲)。

### 4.2 `KnowledgeGraph` BFS (項目 71)

A-RAG 3 primitive のいずれにも直接対応しないが、Layer 2 の **構造的拡張** として位置づける。コサイン類似度が "個別ノードペアの距離" しか見ないのに対し、graph BFS は "関係エッジを辿った N-hop 連想" を返すため、Bonsai-8B の限られた context window で精度の高い想起を実現する (graph.rs docstring より)。

- 単一 query → cosine top-K = Layer 2 (純粋 semantic)
- 単一 query → BFS depth=2 = Layer 2.5 (associative semantic)

### 4.3 Vault Rules vs Docs 分離 (項目 76)

`Vault::read_rules` (Decision/Pattern を常時注入) と `read_docs_for_context` (Fact/Insight/Preference/Todo をタスク連動注入) の 2 経路は、A-RAG の "always-on retrieval" と "query-driven retrieval" の混合に対応するが、bonsai 側は **content-type ベースで static 分類** している点が違う (LLM 判断不要)。

---

## 5. diff の発見 (現状とのギャップ)

### 5.1 A-RAG にあって bonsai にない

| A-RAG primitive / 機能 | bonsai 現状 | gap 度 |
|---|---|---|
| **hierarchical query routing** (Layer 選択を明示判断するメタロジック) | 持たない (LLM の tool selection に委譲、または `select_relevant_split` の type 別 routing で間接的) | ★★ 中 |
| **Layer 別観測 metric** (各 Layer の hit / miss / latency 計測) | `AuditAction` に Layer 単位の record なし | ★ 低 |
| **explicit Layer fallback policy** (Layer 1 fail → Layer 2 escalate) | `HybridSearch` で常に併用、明示 fallback なし | ★ 低 (RRF で代替済) |

### 5.2 bonsai にあって A-RAG にない (bonsai 独自拡張)

| bonsai 機能 | A-RAG 想定外の理由 | 価値 |
|---|---|---|
| **hybrid RRF fusion** (項目 76 / 149) | A-RAG は Layer 別ツールを LLM 選択に任せる | ★★★ 高 (1bit モデルに特に有効) |
| **双方向 BFS graph** (項目 71) | A-RAG は flat retrieval のみ | ★★ 中 (連想記憶) |
| **Rules vs Docs 分離** (項目 76) | A-RAG は content-type 分類なし | ★★ 中 (常時注入 vs context 連動) |
| **`<context type="...">` タグ統一** (項目 80) | A-RAG は output format 不指定 | ★★ 中 (観測性) |
| **Vault → KnowledgeGraph 自動相互リンク** (項目 106 / `Vault::record_to_graph`) | A-RAG は store 間 cross-reference なし | ★ 低 |

### 5.3 結論

bonsai は A-RAG taxonomy を **superset として実装** している。docs PR で taxonomy 共通語を導入しつつ、**bonsai 独自拡張は 「A-RAG enhancement」として明記** することで external validation と独自性 claim を両立する。

---

## 6. docs 更新内容

### 6.1 CLAUDE.md (項目 199 を追加)

```markdown
199. **A-RAG hierarchical retrieval framework との整合 (docs validation、★ docs PR)**: arxiv 2602.03442 "A-RAG: Scaling Agentic RAG via Hierarchical Retrieval Interfaces" (2026-02) の 3 階層 retrieval taxonomy (keyword / semantic / chunk) と bonsai 既存 3 検索ツールが完全対応構造であることを external validation として記録。Layer 1 = keyword (FTS5 項目 30 / grep)、Layer 2 = semantic (Embedder 項目 157/158)、Layer 2.5 = associative (KnowledgeGraph BFS 項目 71/106、bonsai 独自拡張)、Layer 1+2 = hybrid RRF (項目 76/149、bonsai 独自で A-RAG paper より高度)、Layer 3 = chunk read (FileReadTool 項目 25/116、Vault 項目 30/76、memory_blocks 項目 179)。詳細マッピング表 + ASCII 図 + diff 分析は `.claude/plan/arag-hierarchical-retrieval-docs.md` に集約。**production code 変更ゼロ、テスト 1032 passed 維持**、docs 3 ファイル更新 (CLAUDE.md 項目追加 / memory/MEMORY.md reference 追加 / memory/arag_alignment.md 新規)、認知負荷低減 + 設計知見の external validation 達成。次の docs PR 候補 (本 plan 範囲外): ① Layer 1/2/3 の per-task selection routing を `select_relevant_split` で明示 (項目 137 split policy + A-RAG layer 分離の 2 軸化)、② hybrid RRF を Layer 1+2 融合と明示し project_memory に観測 metric 追加、③ Vault 検索を Layer 2.5 (graph 化済 semantic) として中間 layer に位置付け。
```

### 6.2 memory/MEMORY.md (調査・研究セクションに追加)

```markdown
- [A-RAG Alignment](arag_alignment.md) — arxiv 2602.03442 (A-RAG) と bonsai 既存 3 検索ツールの完全対応マッピング (項目 30/71/76/80/106/116/149/157/158/162/179)、bonsai 独自 5 拡張 (hybrid RRF / BFS graph / Rules vs Docs / context タグ統一 / Vault→Graph 相互リンク)、CLAUDE.md 項目 199 完了
```

`research_arxiv_2026_05_07.md` 領域 5 への ★★★ 高優先 7 番「A-RAG hierarchical interfaces — 既存 3 検索ツールを A-RAG taxonomy で整理 (1 day PR)」の status を **完了** 表記に更新する 1 行追記も含める。

### 6.3 memory/arag_alignment.md (新規)

本 plan の section 2-5 (マッピング表 / ASCII 図 / hybrid 位置付け / diff 発見) を memory に persist する形で配置。CLAUDE.md は項目 199 で要約のみ、詳細は arag_alignment.md と本 plan ファイルへの相互参照で集約。

### 6.4 knowledge_synthesis_2026_04_20.md (任意、scope 拡張)

「外部設計検証」節がもしあれば A-RAG 引用追加。**section が現状不在なら本 PR では追加せず、次回 knowledge_synthesis 改訂時に統合** (scope creep 回避)。

---

## 7. 将来の拡張余地 (本 plan 範囲外、次の docs PR 候補)

以下は **本 docs PR scope 外** で、効果が定量検証必要な変更のため別 plan として切り出す:

1. **Layer 別 per-task selection routing** — 項目 137 (`select_relevant_split` の MCP/built-in 分離) と整合する形で Layer 1/2/3 の per-TaskType 優先度を `ToolRegistry` に拡張、TaskType::Research → Layer 2 優先 / TaskType::FileOp → Layer 3 直行 など。実装は production code 変更を含むため別 plan。
2. **Layer 別観測 metric を `AuditAction` に追加** — `AuditAction::RetrievalLayerUsed { layer, hit_count, latency_ms }` で Layer 別 hit rate を Lab dashboard に表示。Beyond pass@1 plan の RDC/VAF と組合せると Layer 別の stability 評価が可能。
3. **hybrid RRF α 重みの動的最適化** — 現状 α=0.5 固定、TaskType に応じて 0.3-0.7 で動的調整するロジック追加。Lab variation で +Δscore 効果を計測。
4. **Vault entry を Layer 2.5 として graph 化** — 項目 106 `record_to_graph` の双方向リンクを query 時に活用し `Vault::read_via_graph(query, depth)` API 追加。

これらはいずれも **本 docs PR の merge 前提条件ではない**。本 PR で taxonomy 共通語が定着すれば、上記将来 PR の 設計ドキュメント記述コストが下がる (Layer 1/2/3 という語彙で議論できる)。

---

## 8. 判定 gate (docs PR 軽量版)

| Gate | 内容 | 検証手段 | 必須 |
|---|---|---|---|
| **G-1** | 既存テスト 1032 passed 維持 (production code 変更ゼロ確認) | `cargo test --release --lib` | 必須 |
| **G-2** | マッピング表 (section 2) の各 row が実コード path で reachable | `Grep` で関数名 / 構造体名検索、各項目で 1 hit 以上確認 | 必須 |
| **G-3** | CLAUDE.md / memory/MEMORY.md / knowledge_synthesis の整合性 (既存テキストとの矛盾なし) | 既存項目 30/71/76/80/106/116/149/157/158/162/179 の記述と新規項目 199 の文言が衝突しない目視確認 | 必須 |
| **G-4** | `cargo clippy --release --lib --tests -- -D warnings` 0 warning 維持 | clippy 実行 (production code 変更ゼロのため当然 0、念のため) | 任意 |
| **G-5** | `cargo fmt --check` 0 件 (本 PR は production code 不変のため当然 0) | fmt check | 任意 |

G-1 から G-3 が PASS なら docs PR として merge 可能。effectiveness 評価 (score 改善 / duration 短縮) は本 PR の対象外 (docs だから)。

---

## 9. risk

| Risk | 影響度 | 軽減策 |
|---|---|---|
| **後方互換性** | ゼロ (docs 変更のみ、production への影響なし) | n/a |
| **既存 knowledge_synthesis との重複** | 低 — knowledge_synthesis に「外部設計検証」節がない場合、整合性問題なし。ある場合は相互参照で整理 | section 6.4 で knowledge_synthesis 統合は scope 外と明記、次回改訂時に統合 |
| **過剰命名** | 中 — 「Layer 1/2/3」を入れ替えると既存項目 (30/71/76 等) との関連が分かりにくくなる | A-RAG paper の番号付けに従う (Layer 1=keyword、Layer 2=semantic、Layer 3=chunk read)、既存項目との対応表を CLAUDE.md 項目 199 で明示 |
| **A-RAG paper の定義変更** | 低 — arxiv preprint のため将来 update で定義変更可能性 | 本 PR で参照する arxiv ID (2602.03442) と version を docs に固定記載 |
| **bonsai 独自拡張の dilution** | 中 — A-RAG taxonomy に揃えすぎて bonsai 独自性 (hybrid RRF / BFS graph) が希薄化 | section 5.2 の bonsai 独自 5 拡張を明示的に「A-RAG enhancement」として記述、CLAUDE.md 項目 199 でも明記 |
| **Beyond pass@1 / AgentHER plan との conflict** | ゼロ — file 名 / 項目番号 (199 vs 200/201) / API 名空間 全て独立 | 本 plan の項目番号は 199、API 名空間 (Layer / RetrievalLayer) は新規定義のみ、既存 API 変更ゼロ |

---

## 10. 見積もり

| Phase | 内容 | 所要 |
|---|---|---|
| **P1** | 本 plan ファイル Write (本作業) | 完了 |
| **P2** | G-1 / G-2 検証 (`cargo test --lib` + `Grep` で各 row の reachability 確認) | 0.5h |
| **P3** | `memory/arag_alignment.md` 新規 Write (section 2-5 を移植) | 0.5h |
| **P4** | CLAUDE.md 項目 199 追記 (section 6.1) | 0.3h |
| **P5** | `memory/MEMORY.md` reference 追記 + research_arxiv_2026_05_07.md status update (section 6.2) | 0.2h |
| **P6** | G-3 整合性確認 (項目 30/71/76/80/106/116/149/157/158/162/179 と項目 199 の文言衝突チェック) | 0.3h |
| **P7** | commit + handoff (1-2 commits、`docs(arag)` prefix) | 0.2h |
| **計** | | **~2-3h, 0.5 day** |

---

## 11. 採否判定

**docs PR scope 内で完結、production 影響ゼロ、external validation 価値高、bonsai 独自拡張の差別化記述あり**。次セッションで本 plan の P2-P7 を順次実行することで完遂可能。Beyond pass@1 plan / AgentHER plan の docs lane と並列 merge 可能 (項目番号 / file 名 / API 名空間ともに独立)。

**承認条件**: G-1 / G-2 / G-3 PASS。effectiveness 評価不要 (docs PR)。
