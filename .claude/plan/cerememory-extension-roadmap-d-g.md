# Plan: Cerememory Extension Roadmap (Phase D〜G) — bonsai memory layer 段階拡張 master roadmap

> **由来**: 本 session 先行 plan `cerememory-decay-port-impl.md` (Phase A、~0.5 day) + `cerememory-review-state-v12-impl.md` (Phase B、~1.5 day) で Cerememory (`co-r-e/cerememory` MIT、commit b08d201、2026-05-08) の最大インパクト 2 軸 (decay / Strength-Freshness 分離) を起票済。本 plan は **残 4 候補 (Phase D-G) を統合 master roadmap** として整理し、個別実装 plan は採否ゲート達成後に展開する。`post-lab-v13-roadmap.md` の master roadmap pattern 踏襲。
>
> **目的**: Cerememory 5-store + 周辺機構の bonsai 取込み余地を **網羅 + 優先順位 + 採否ゲート**で見える化。premature な個別 plan を避け、Lab v17/v18/v19 各 effectiveness 結果に応じて Phase 着手順を動的決定する。
>
> **前提**: 本 plan は **planning-only** (production code 変更ゼロ、各 Phase は別 plan で展開)。Lab v17 (項目 214 進行中) と独立、commit 時点で 1104 passed 維持。各 Phase は env opt-in 設計 (項目 214 toggle pattern と一貫)。

## Task Type
- [ ] Frontend
- [x] Backend (memory layer 段階拡張 master roadmap)
- [ ] Fullstack

## 1. 背景: Cerememory 5-store 設計と bonsai gap (再掲)

| Cerememory store | bonsai 対応 | gap |
|---|---|---|
| `cerememory-store-episodic` | EventStore (項目 209 trait 化済) | ほぼ同等 |
| `cerememory-store-semantic` + `-association` | KnowledgeGraph (graph.rs、BFS 双方向) | 同等 + decay/spreading activation 一部不足 |
| `cerememory-store-procedural` | SkillStore + HeuristicStore (項目 213) | 統合度差 (Cerememory は behavioral patterns + skills 1 store) |
| `cerememory-store-emotional` | **無** | **Phase D 候補** |
| `cerememory-store-working` | LoopState (無制限) | **Phase G 候補** |

加えて非-store 機構:
- `cerememory-decay/math.rs` → Plan A (`cerememory-decay-port-impl.md` 起票済)
- ADR-011 ReviewState → Plan B (`cerememory-review-state-v12-impl.md` 起票済)
- `cerememory-transport-mcp` → **Phase E 候補** (bonsai が MCP server として併用)
- ADR-010 tamper-evident audit log → **Phase F 候補**

## 2. Phase D: Emotional metadata cross-cutting plane

### 2.1 概要
Cerememory `cerememory-store-emotional` は cross-cutting affective metadata layer (cf. amygdala) で他 4 store に**横断適用**される修飾要素。`E_mod = 1.0 + emotion_intensity * 0.5` (Plan A `decay/math.rs` 既出) で decay を緩和、emotionally salient な記憶ほど長く保持される。

bonsai HeuristicStore は重大度 (Critical/High/Medium/Low) の概念なし。**致命失敗回避策の助言**と**軽微な効率改善**が同等に decay 適用される設計上の不均衡。

### 2.2 主要設計
- `heuristics.emotion_intensity REAL NOT NULL DEFAULT 0.0` 1 列追加 (SCHEMA V13)
- `category` から自動推定:
  - `failure_recovery` → `intensity=0.6` (高、致命失敗回避)
  - `verification` → `intensity=0.3` (中)
  - `efficiency` → `intensity=0.1` (低)
- `compute_fidelity(score, t, stability, d=0.3, E_mod=compute_emotion_mod(intensity))` で Plan A の decay に直接乗算
- `inject_heuristics` の top-K ranking で `score * (1 + E_mod_weight)` でも E_mod 反映可

### 2.3 採否ゲート
- **着手前**: Plan A (`decay-port-impl.md`) merge + Lab v18 paired t-test で decay-on Δscore ≥ +0.015 確認
- **採否判定**: Lab v20 paired t-test で `BONSAI_EMOTION_ENABLED` ON/OFF、Δscore ≥ +0.015 AND p < 0.1
- **REJECT 時**: env-only feature 残置、failure_recovery 助言の保持期間延長 patch のみ抽出可能性

### 2.4 工数 + risks
- **工数**: ~0.5 day (P1 Red 0.5h + P2 Green 1.5h + P3 Refactor 0.5h + P4 Smoke 0.5h + P6 commit 0.5h)
- **R1**: emotion_intensity の自動推定が category 4 値で粗い → MetaMemory plane 拡張で精緻化 (将来)
- **R2**: E_mod が decay と独立に inject ranking でも作用 → double counting risk → ranking では使わず decay のみ適用が安全

### 2.5 個別 plan 展開条件
- Plan A (decay) merge 済 + Lab v18 ACCEPT
- 個別 plan ファイル名候補: `cerememory-emotional-metadata-v13-impl.md`

## 3. Phase E: MCP server として Cerememory を bonsai が併用

### 3.1 概要
Cerememory は MCP server として `cerememory serve --data-dir ~/.cerememory/data` で起動可、HTTP port 8420 + MCP stdio proxy 提供。bonsai は MCP client (項目 102/108/124) を持つため、**追加 memory backend として直結可能**。

bonsai 既存 5 store と Cerememory 5 store の併用 (10 store 並存) または **段階移行** (bonsai 既存 → Cerememory 統合) のどちらを取るか戦略選択が必要。本 roadmap 段階では両戦略を並列提示し、Phase E 個別 plan で 1 つに収束させる (Cerememory 上流の 1-process-per-data_dir 制約と bonsai test parallel 要件の対立から、データ実機運用で判断分岐するため)。

### 3.2 主要設計
- **戦略 1: 併用 (低リスク)**
  - bonsai 既存 layer 維持、Cerememory を `bonsai-cerememory-mcp` 接続経由で新規 query 用に追加
  - context_inject に Cerememory `recall.query` 経由の retrieval を opt-in 追加
  - 重複あり、複雑度増、merge 候補メモリ整合性 challenge
- **戦略 2: 段階移行 (高リスク・高利得)**
  - bonsai HeuristicStore (項目 213) → Cerememory procedural store
  - bonsai EventStore → Cerememory episodic store
  - 1 store ずつ migrate、effectiveness 各段階で確認
  - bonsai code 大幅縮小 (~3000 行 net 削減見込み)、Cerememory 上流追従で機能自動増

### 3.3 採否ゲート
- **着手前**: Lab v17 結果 (項目 213 ACCEPT) + Plan A/B merge + Cerememory v0.2.8 stable 動作検証
- **採否判定**: Phase 4 Smoke で `recall.query` 動作 + paired t-test (bonsai-only vs bonsai+cerememory) で Δscore ≥ 0 (退行なし)
- **REJECT 時**: 戦略 1 維持で副次 store として残置、戦略 2 は破棄

### 3.4 工数 + risks
- **工数**: 戦略 1 = ~2 day / 戦略 2 = ~3-5 day (段階移行)
- **R1**: Cerememory 上流 breaking change で bonsai が影響 (mitigations: pin commit hash + 上流追従 plan)
- **R2**: 1 process 制約 (Cerememory は data_dir 1 process 限定)、bonsai test parallel と相性問題 (mitigations: in-process embedding mode 検証 or 戦略 1 単独)
- **R3**: MCP transport 経由のレイテンシで Lab cycle duration +20-30%? (要計測)
- **R4**: stop-the-world migration で Lab v17 中断必須 (mitigations: Lab v17 完了後着手)

### 3.5 個別 plan 展開条件
- Plan A/B merge 済 + Lab v17 ACCEPT (項目 213 維持決定) + Cerememory 上流 1 ヶ月 stable
- 個別 plan ファイル名候補: `cerememory-mcp-integration-strategy-1.md` / `cerememory-store-migration-strategy-2.md`

## 4. Phase F: Tamper-evident audit log (ADR-010 hash chain)

### 4.1 概要
Cerememory `cerememory-engine` は `data_dir/audit.jsonl` に **JSONL hash chain** を書く (各 entry に `prev_hash` + `entry_hash`、SHA-256)。`cerememory audit-verify` で sequence number + hash チェーン整合性検証。truncation detection には head hash の外部記録が必要。

bonsai `src/observability/audit.rs` (LlmCall / ToolCall / SecurityEvent / StepOutcome) は SQLite `audit_log` テーブルに INSERT のみ、改竄検出機構なし。security-sensitive な production 利用 (秘密情報フィルタ通過 audit、tool 実行記録) で改竄不可性が必要。

### 4.2 主要設計
- **新規 `src/observability/hashchain.rs`** (~150 行):
  - `compute_entry_hash(prev_hash: [u8;32], payload: &[u8]) -> [u8;32]` (SHA-256(prev || canonical_payload))
  - `verify_chain(entries: &[AuditEntry]) -> Result<HeadHash, ChainError>` (順次再計算 + mismatch 検出)
- **新規 column** `audit_log.prev_hash BLOB NOT NULL DEFAULT zero` + `audit_log.entry_hash BLOB NOT NULL DEFAULT zero` (SCHEMA V14)
- **新規 CLI** `bonsai --audit-verify` (Cerememory `audit-verify` 相当)
- **Optional JSONL mirror**: `~/.config/bonsai-agent/audit.jsonl` に並行 append (SQLite 万一破損時の冗長性、format = `{"seq": N, "ts": "ISO 8601 RFC 3339", "prev_hash": "hex", "entry_hash": "hex", "payload": {...}}`)
- env opt-in: `BONSAI_AUDIT_HASHCHAIN_ENABLED=1` (production default OFF、既存挙動 100% 互換)

### 4.3 採否ゲート
- **着手前**: production code 安定 (Lab v17/v18/v19 後)、security 要件明確化 (秘密情報外部漏洩リスクの ROI 計算)
- **採否判定**: Phase 4 Smoke で 1000 entries chain 構築 + 中間 entry tampering で `verify_chain` が CONFIRM detect
- **REJECT 時**: 機能保留、env opt-in なので production 影響ゼロ

### 4.4 工数 + risks
- **工数**: ~1 day (P1 Red 1h + P2 Green 4h + P3 Refactor 1h + P4 Smoke 1h + P6 commit 1h)
- **R1**: SHA-256 計算が hot path で audit insert latency +0.1ms? (要 bench、許容範囲想定)
- **R2**: chain rebuild が大量 entry で O(N) → cargo bench で 100k entry 検証
- **R3**: head hash 外部記録の運用負担 (mitigations: 起動時自動 export to `audit-head.txt`、user 確認のみ)

### 4.5 個別 plan 展開条件
- production 安定 + security audit 要件具体化
- 個別 plan ファイル名候補: `cerememory-audit-hashchain-v14-impl.md`

## 5. Phase G: Working memory capacity 7±2 制限

### 5.1 概要
Cerememory `cerememory-store-working` は PFC (prefrontal cortex) 風 **limited-capacity** working memory cache、`Volatile, limited-capacity, high-speed active context cache` (README §"Five Memory Stores")。Miller の魔法数 7±2 (1956) の系譜、認知負荷上限を構造的に強制。

bonsai `LoopState` (`src/agent/agent_loop/state.rs`) は無制限 (Vec<Message> + Vec<i64> 等)、大規模 task で context 圧迫 → 項目 82 (ContextOverflowGuard、F2) で実質的な強制 compaction が発火。Phase G は **発火前の上流で構造的に制限**することで F2 fire 頻度削減を狙う。

### 5.2 主要設計
- **`LoopState::max_active_messages: usize = 9` (env override `BONSAI_WORKING_CAP=N`)** 新規 field
- `LoopState::push_message` で `if active.len() >= max_active_messages { evict_oldest_low_priority() }`
- **`MessagePriority` enum**: `System(highest), Pinned, ToolResult, AiResponse, UserMessage(lowest)` の 5 段階
- 退避先: 既存 compaction.rs の archived buffer に積む (F2 / step-12 fallback と協調)
- env opt-in: `BONSAI_WORKING_CAP_ENABLED=1` (production default OFF、既存挙動 100% 互換)
- `inject_heuristics` 結果の Pinned 化で freshness gate (Plan B) 通過分は退避対象外

### 5.3 採否ゲート
- **着手前**: 項目 82 ContextOverflowGuard (F2) の fire 頻度ベースライン取得 (Lab v17 logs 末尾で `ContextOverflowGuard fire count=N` 抽出)
- **採否判定**: Lab v21 paired t-test で `BONSAI_WORKING_CAP_ENABLED` ON/OFF、Δscore ≥ 0 (退行なし) AND F2 fire 頻度 -50% 以上
- **REJECT 時**: env-only feature、現行 LoopState 無制限維持

### 5.4 工数 + risks
- **工数**: ~0.5 day (P1 Red 0.5h + P2 Green 1.5h + P3 Refactor 0.5h + P4 Smoke 0.5h + P6 commit 0.5h)
- **R1**: System message を誤って evict すると agent crash (mitigations: System priority は evict 対象外、test で確認)
- **R2**: ToolResult evict で次 step の reasoning context 欠落 → score 退行 (mitigations: max_active_messages を 9 = 7+2 に設定、ToolResult は最低 3 直近保持)
- **R3**: 項目 82 F2 fire 頻度 -50% は楽観的、実機で要計測 (mitigations: ACCEPT 基準を退行なし + F2 fire 削減傾向 ≥ -10% に緩和可)

### 5.5 個別 plan 展開条件
- 項目 82 fire 頻度ベースライン取得 (Lab v17 結果から計測可)
- 個別 plan ファイル名候補: `cerememory-working-memory-cap-impl.md`

## 6. 採否優先順 (Lab v17 結果に応じて動的)

### 6.1 Lab v17 ACCEPT (項目 213 維持) シナリオ
1. **Plan A (decay)** ★★★ 0.5 day → Lab v18 で effectiveness 検証
2. **Plan B (ReviewState)** ★★★ 1.5 day → Lab v19 で effectiveness 検証 (V11 → V12 順)
3. **Phase D (Emotional)** ★★ 0.5 day → Lab v20 (Plan A merge 後)
4. **Phase G (Working cap)** ★ 0.5 day → Lab v21 (項目 82 ベースライン取得後)
5. Phase F (Audit hashchain) ★ 1 day → security 要件明確化後
6. Phase E (MCP integration) heavy → 上流安定 + 全 Phase 集積後

### 6.2 Lab v17 REJECT (項目 213 dead-code 候補) シナリオ
1. **Plan A (decay)** ★★ 0.5 day → 他 store (Skill / Experience / Vault) の汎用 prune 基盤として転用、HeuristicStore は不問
2. **Plan B (ReviewState)** ★ 1.5 day → 他 store で Strength/Freshness 分離適用検討、HeuristicStore 削除と並行
3. **Phase G (Working cap)** ★★ 0.5 day → 項目 82 F2 上流対策として独立価値、HeuristicStore 不要
4. **Phase F (Audit hashchain)** ★ 1 day → security 価値は HeuristicStore 不問、独立着手可
5. Phase D (Emotional) ★ → HeuristicStore dead-code なら必然不要、他 store 適用は限定
6. Phase E (MCP integration) → 戦略 2 (段階移行) で bonsai memory layer 全置換が現実的選択肢化

### 6.3 着手判断ルール
- **必須前提**: Lab v17 完了 + Plan A/B 着手判断確定 (DB 状態破壊回避)
- **並列実装**: Phase D / G / F は機構独立、相互依存なし → 並列実装可
- **シリアル実装**: Phase E は heavy、全 Phase 集積後単独着手

## 7. 共通設計ルール (Phase D-G で踏襲)

| ルール | 由来 | 理由 |
|---|---|---|
| **production default OFF** | 項目 214 / Plan A / Plan B | 観測動作完全互換、後方互換 100% |
| **env opt-in (`BONSAI_*_ENABLED=1`)** | 項目 214 toggle pattern | env name 一貫性 |
| **TDD strict 5 phase** | Project CLAUDE.md | Red → Green → Refactor → Smoke → Effectiveness |
| **TSV 出力 + paired t-test** | 項目 214 Lab v17 pattern | scipy 不使用、df=4 t-table 線形補間 |
| **MIT attribution** | Cerememory MIT license | `docs/THIRD_PARTY_LICENSES.md` 全文記載 |
| **SCHEMA V** 連番 | bonsai db migration 規約 | V11 (decay) / V12 (ReviewState) / V13 (Emotional) / V14 (Audit hashchain) |
| **API 完全 additive** | 項目 209 trait pattern | signature 変更ゼロ、既存 caller 無変更 |

## 8. quality gates (本 master roadmap)

| Gate | 内容 | 検証 |
|---|---|---|
| **G-1** | 4 Phase の概要 + 採否ゲート + 工数明確化 | self-review |
| **G-2** | INDEX.md 「🆕 外部 OSS 取込み」セクションに追記 | grep |
| **G-3** | Lab v17 完了前は **planning-only** (production code 変更ゼロ、DB 状態破壊リスクなし) | git diff stat |
| **G-4 (個別 plan 展開時)** | 各 Phase 採否ゲート達成後に個別実装 plan を展開 | per-phase |

## 9. 見積もり (master roadmap)
| Phase | 内容 | 所要 |
|---|---|---|
| **本 plan 起票** | 4 phase 統合 master roadmap | 1.5h (本 session 内) |
| Phase A 個別 plan | 既起票 (`decay-port-impl.md`) | - |
| Phase B 個別 plan | 既起票 (`review-state-v12-impl.md`) | - |
| Phase D 個別 plan | 採否ゲート後 | 1h (Lab v18 後) |
| Phase E 個別 plan | 採否ゲート後 | 2h (上流安定後) |
| Phase F 個別 plan | 採否ゲート後 | 1h (security 要件後) |
| Phase G 個別 plan | 採否ゲート後 | 1h (項目 82 baseline 後) |

実装着手は Lab v17 完了後、各 Phase 個別 plan 採否判断を経て段階展開。

## 10. 次の段階
### 着手判断
- ✅ Phase A/B 起票済 (本 session 先行)
- ✅ Cerememory deep dive 完了 (decay/math.rs / ADR-005 / ADR-010 / ADR-011 / mcp-agent-metadata 確認)
- ⏳ Lab v17 進行中、完了後着手判断
- ⏳ Plan A/B 個別 phase ごとに採否ゲート逐次確認

### 先送り条件
- ❌ Lab v17 完了前 (DB 状態破壊リスク)
- ❌ Cerememory 上流が breaking change (commit b08d201 → 別 commit) で全 plan re-sync 必要

## 11. 着手前チェックリスト (各 Phase 個別 plan 展開時に逐次確認)
1. [ ] Lab v17 完了 + ACCEPT/REJECT 確定
2. [ ] 当該 Phase の前提 Phase 採否ゲート達成 (例: Phase D は Plan A merge 必須)
3. [ ] Cerememory 上流 commit hash 確認 (b08d201 から差分なしを確認)
4. [ ] `docs/THIRD_PARTY_LICENSES.md` 既存有無 (Plan A 先行で作成想定)

## 12. Quick Start (個別 plan 展開時)
```bash
# 1. 前提 Phase merge 確認
git log --oneline | grep -E "decay-port|review-state-v12"

# 2. 当該 Phase 採否ゲート確認 (Lab vN 結果)
python3 scripts/lab_vN_paired_ttest.py ./lab-vN-logs

# 3. 採否判定 = ACCEPT なら個別 plan 展開
$EDITOR .claude/plan/cerememory-{phase-name}-impl.md
# 既存 Plan A/B のフォーマット踏襲

# 4. Plan に従って TDD strict 5 phase 実装
```

## 13. 参考
- [co-r-e/cerememory](https://github.com/co-r-e/cerememory) commit b08d201 (2026-05-08)
- README.md (5-store 設計 + decay/association/evolution)
- ADR-005 (power-law decay rationale) — Plan A
- ADR-010 (tamper-evident audit log) — Phase F
- ADR-011 (Adaptive Review and Freshness) — Plan B
- mcp-agent-metadata.md (MetaMemory フィールド契約) — Phase E
- 項目 213 ERL Phase 2 Green commit `41b6ac3` (前提実装)
- 項目 214 Lab v17 toggle 機構 commit `0013f31` (env opt-in 設計踏襲)
- 項目 82 ContextOverflowGuard (Phase G の上流位置)
- `post-lab-v13-roadmap.md` (本 plan が踏襲する master roadmap pattern)

## 14. SESSION_ID (for /ccg:execute use)
- 本 plan は roadmap、個別 plan 展開時に各 SESSION_ID 取得

## 15. ★ 失敗時 (全 Phase REJECT) handling
全 4 Phase が effectiveness で REJECT:
1. **CLAUDE.md** に negative finding 「Cerememory 取込みは Bonsai-8B 1bit には translate しない」記録
2. **Plan A (decay) のみ汎用基盤として残置**、他 Phase は env-only feature で production 影響ゼロ
3. **Cerememory 上流追従中止**、本 roadmap を 🗄 (history) に状態変更
4. 後続調査軸: 別 OSS (Letta / mem0 / etc.) 比較、または独自設計
