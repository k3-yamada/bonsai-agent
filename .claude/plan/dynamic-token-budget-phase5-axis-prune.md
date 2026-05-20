# Dynamic Token Budget Phase 5 — 4 軸個別 Prune 配線 (項目 248 Phase 5 候補)

**状態**: planning-only (2026-05-19 起票)
**推奨度**: ★★ (Phase 1-4 完遂後の自然進路、log-only → 実 prune 影響の移行)
**推定工数**: ~4-5h plan + Phase 1-3 (TDD strict) + ~45 min Phase 4 smoke + Lab v22+ paired (~8h)
**起点**:
- 項目 248 本体 plan: `.claude/plan/dynamic-token-budget-compaction.md` §3.5 (Phase 5 言及箇所)
- 項目 248 Phase 4 wiring 完遂 commit `5109219` + 後続 wiring commit (`compact_if_needed` log emit hook)
- critic adversary F5 finding: **"intentional dead-data flow"** = AllocatedBudget は計算されるが prune logic に未伝播 (設計通り Phase 5 持越し)
- 上流 Zenn 記事: <https://zenn.dev/kenimo49/articles/llm-memory-context-engineering-4-architectures>

---

## §0. 背景

### 0.1 Phase 1-4 完遂状態 (2026-05-19 時点)

| Phase | 内容 | commit | 影響 |
|-------|------|--------|------|
| Phase 1 Red | `BudgetRatios` skeleton + 5 failing test | `2546d79` | (none) |
| Phase 2 Green + Phase 3 Refactor | `allocate()` 4 軸按分 / `adjusted()` 動的調整 / env getter SSOT | `5109219` | 純粋関数追加、prune 影響ゼロ |
| Phase 4 wiring | `CompactionConfig.budget_ratios: Option<BudgetRatios>` field + `with_dynamic_budget_from_env()` factory + `dynamic_budget_for_compaction(config)` helper + `compact_if_needed` 冒頭 log emit hook + `CompactionMiddleware` wiring | (followup commit) | **log emit のみ**、prune 動作は env on/off で同一 (backward compat 完全保持) |

### 0.2 Phase 5 動機

Phase 4 までで「budget 計算」「設定 propagation」「log 可視化」は完成したが、
**実 prune logic への伝播がない**ため、`BONSAI_DYNAMIC_BUDGET=1` をセットしても
score / token usage / context 構造に振る舞い変化が起きない (Phase 4 G-10a と G-10b で
`Δscore=0` 確認済みの設計通り)。

Phase 5 は `AllocatedBudget { buffer, summary, entities, kg }` を実際に
`compact_level1` / `compact_level2` / `compact_level3` 各段の prune logic に伝播し、
**4 軸別の prune 閾値で選択的に保護 / 削減**を行うフェーズ。これにより:

- 1bit Bonsai-8B の 12,288 token context で「KG 長文応答 → buffer 圧縮」のリスクが構造的に予防される
- Lab v22+ paired で env on/off の Δscore / matched 軸 variance を初めて計測できる
  (Lab v20 structural finding `matched=0 deterministic` を補強する変動軸として機能可能性)

### 0.3 既存 4 段 stage との関係 (重要)

現状 `compact_if_needed` は 4 段 stage (L0/L1/L2/L3) を **size 閾値** (`max_context_tokens * 3/4`、
`* 9/10`、`* 1`) で trigger する設計。Phase 5 は **この 4 段 stage 構造は維持**し、
各段の内部 prune logic に「メモリ種別」軸を持ち込む。**stage 軸 (時系列) × 種別軸 (memory type)
の 2 次元 prune** に拡張する形となる。

---

## §1. 問題定義

### 1.1 各 prune level でのメモリ種別判別の困難さ

| Level | 既存挙動 | 種別判別の難所 |
|-------|---------|---------------|
| L0 | 大 tool output → file offload (`content.len() > large_output_threshold`) | tool 結果が KG search 由来か file_read 由来かを content だけで判別不可 |
| L1 | `placeholder_keep_recent` + `prune_protect_tokens` + AI+Tool ペア保護 + score 低位を `[prev:id]` placeholder 化 | role=Assistant でも「直近 buffer」か「過去 summary」かを直接判定不可 |
| L2 | Assistant content が `summary_max_chars` 超なら `...[summarized]` 切詰 + thinking summary 末尾追加 | summary 化対象 = 過去 boundary 以前の Assistant、種別非考慮 |
| L3 | system 残し + handoff summary + emergency tail keep | 緊急 prune、種別考慮余地ほぼなし |

### 1.2 メモリ種別の操作定義 (Phase 5 仕様)

以下を「観測可能な signal」で判別 (1bit Bonsai-8B 環境で安定動作可能な範囲のみ採用):

| 種別 | 操作定義 (Phase 5) | 判別 cost |
|------|-------------------|-----------|
| **buffer** | 直近 `placeholder_keep_recent` 件の `Role::User` / `Role::Assistant` (会話末尾固定窓) | O(N) 末尾走査 |
| **summary** | `Role::Assistant` かつ content prefix が `[Preserved Thinking]` / `...[summarized]` / `[Handoff Summary]` のいずれか | O(1) prefix 判定 |
| **entities** | `Role::Tool` で `tool_call_id` prefix が `agenther_` (AgentHER 由来) または content prefix が `[entities:...]` | O(1) prefix 判定 |
| **kg** | `Role::Tool` で `tool_call_id` prefix が `memory_search` / `kg_query` / `graph_search` (memory tool group) | O(1) prefix 判定 |
| (それ以外) | 「unclassified」として既存 score 判定にフォールバック | 既存挙動温存 |

**設計補足**:
- prefix-based 判定は 1bit context で安定。LLM 出力解析 (entity 列挙の意味的判別等) は項目 226 R5 gate
  finding (Uncertain 92.3%) より信頼性低のため不採用
- AgentHER 経路の tool_call_id 命名規約は既存 production の慣習に合わせる (項目 201-205 系列)
- 「unclassified」軸は backward compat を保証 (既存 score 判定が走る)

### 1.3 既存 4 段 stage との整合性要件

- **R1**: env unset = 完全な従来動作 (Phase 4 G-10a と同様、Phase 5 で再確証)
- **R2**: 4 段 stage の trigger 閾値 (`3/4 / 9/10 / 1`) は不変。種別軸は **stage 内** の prune 判定で導入
- **R3**: AI+Tool ペア保護、最初/最後 User 保護、tool_call_id reference 整合 (項目 12 系) を継承
- **R4**: 4 軸の合計 = `allocated.total` (= `config.max_context_tokens`)、超過時は scale_factor 均等縮小

---

## §2. 設計 — 3 案比較

### 2.1 比較表 (評価軸 5 つ)

| 軸 | 案 A: `prune_protect_tokens` 動的 override | 案 B: 軸別独立 compact pass | **案 C: 既存 stage + 軸別 budget overflow** |
|---|---|---|---|
| 1. 既存 4 段 stage との整合 | ◎ stage 構造温存、L1 の閾値だけ動的化 | × stage 構造を全面再設計、L0/L2/L3 経路の温存不可 | ◎ stage 構造温存、各段で「軸別 budget overflow 判定」を追加 |
| 2. 実装影響範囲 | ◎ L1 の `prune_protect_tokens` を `allocated.buffer` に置換のみ (~20 行) | × ~300 行、関数 4 つ新規 | ○ 各 stage 内に種別判別 helper + budget check 注入 (~80 行) |
| 3. 4 軸保証の強さ | △ buffer 軸のみ厳密、summary/entities/kg は L2 の content.len() 経由で間接的 | ◎ 4 軸ともに独立 prune、保証強固 | ○ 4 軸ともに同等 budget check、stage 順序に応じた優先 prune |
| 4. backward compat | ◎ env unset で完全温存 | △ 大規模再設計、env unset 経路も regression risk | ◎ env unset で `dynamic_budget_for_compaction` が None 返す既存経路で温存 |
| 5. bug risk / test 量 | ◎ Phase 4 test を流用、+3-5 test | × 関数 4 新規、+15-20 test | ○ 各 stage の種別判別 helper + budget check、+8-10 test |
| **総合** | 軸別保証弱、最小実装 | 設計純度高だが影響甚大 | **バランス良 = 推奨** |

### 2.2 推奨案 = 案 C (既存 stage + 軸別 budget overflow)

**コア概念**: stage 軸 (L0→L1→L2→L3) は維持しつつ、各段の内部 prune 判定で
「現状各軸の token 消費 vs `allocated.{buffer,summary,entities,kg}` の余裕」を計算し、
overflow 軸を優先的に prune する。

#### 2.2.1 新規 helper の責務 (案 C 詳細)

```rust
// src/agent/compaction.rs に追加 (公開 API は最小)

/// メモリ種別タグ (prefix-based 判別、§1.2)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryKind {
    Buffer,
    Summary,
    Entities,
    Kg,
    Unclassified,
}

/// 単一 Message の種別判別 (prefix + role + tool_call_id ベース、O(1))
pub(crate) fn classify_memory_kind(msg: &Message, idx: usize, total: usize, keep_recent: usize) -> MemoryKind { /* ... */ }

/// messages 全体の 4 軸 token 消費を集計
pub(crate) struct AxisUsage { pub buffer: usize, pub summary: usize, pub entities: usize, pub kg: usize, pub unclassified: usize }
pub(crate) fn measure_axis_usage(messages: &[Message], keep_recent: usize) -> AxisUsage { /* ... */ }

/// allocated との差分で overflow 軸を返す (大きい順)
pub(crate) fn overflow_axes(usage: &AxisUsage, allocated: &AllocatedBudget) -> Vec<(MemoryKind, usize)> { /* ... */ }
```

#### 2.2.2 各 stage への注入点

- **L1** (`compact_level1` line 624-684): `candidates` ソート後、各候補 msg の `MemoryKind` を判別し、
  **overflow 軸の候補を優先的に prune**。env unset 時は `allocated=None` で従来 score ソート温存
- **L2** (`compact_level2` line 685-716): `summary_max_chars` 切詰前に「summary 軸 overflow 時は
  切詰量を increase (summary_max_chars × 0.7 等)」。env unset 時は既存値
- **L3** (`compact_level3` line 717-): emergency stage、軸概念は無視 (`emergency_keep` 優先)
- **L0** (`compact_level0` line 539-560): file offload、種別非依存。**変更なし**

#### 2.2.3 `compact_if_needed` の改修

```rust
pub fn compact_if_needed(messages: &mut Vec<Message>, config: &CompactionConfig) -> (u8, Vec<String>) {
    // (1) allocated 取得 (env unset で None)
    let allocated = dynamic_budget_for_compaction(config);
    if let Some(ref a) = allocated {
        log_event(LogLevel::Info, "compaction.budget", &format!(...));
    }
    let off = compact_level0(messages, config);
    let mut lv = 0u8;
    if estimate_tokens(messages) > config.max_context_tokens * 3 / 4 {
        // (2) L1 に allocated を伝播 (None で従来挙動)
        compact_level1_with_budget(messages, config, allocated.as_ref());
        lv = 1;
    }
    if estimate_tokens(messages) > config.max_context_tokens * 9 / 10 {
        compact_level2_with_budget(messages, config, allocated.as_ref());
        lv = 2;
    }
    if estimate_tokens(messages) > config.max_context_tokens {
        compact_level3(messages, config);  // L3 軸非依存、変更なし
        lv = 3;
    }
    (lv, off)
}

// 既存 compact_level1 / compact_level2 は `_with_budget(.., None)` を呼ぶ wrapper として温存
pub fn compact_level1(messages: &mut [Message], config: &CompactionConfig) {
    compact_level1_with_budget(messages, config, None);
}
```

**Pros**:
- 既存 stage 構造 100% 温存、`compact_level1` / `compact_level2` の public API も維持 (wrapper)
- env unset 時 = `allocated=None` で **既存と同一 code path**、backward compat 数学的に保証
- 種別判別が prefix-based で O(1)、latency 影響微少 (<1ms / call)
- 4 軸保証は L1 で buffer 中心、L2 で summary 中心、と stage 順序により自然に役割分担

**Cons**:
- `compact_level1_with_budget` / `compact_level2_with_budget` の wrapper / impl 分割が必要 (~30 行追加)
- 種別判別 prefix が production の AgentHER / memory_search tool_call_id 命名規約と一致している前提
  (Phase 1 で grep 確証必要、§3.1 Phase 1 Red の test fixture でカバー)

### 2.3 `MemoryRelevance::current()` hook 実装

項目 248 本体 plan §2.1 で言及された `MemoryRelevance` 計測 (entity hit rate / kg path success rate)
は Phase 5 では **MVP として lightweight 実装**:

```rust
/// 直近 N message から各軸の relevance を粗推定
/// Phase 5 MVP: count-based (将来 Phase 6 で session.experiment_log 統計に置換)
pub fn current_from_messages(messages: &[Message], keep_recent: usize) -> MemoryRelevance {
    let usage = measure_axis_usage(messages, keep_recent);
    let total = (usage.buffer + usage.summary + usage.entities + usage.kg).max(1) as f32;
    MemoryRelevance {
        buffer: 1.0,  // 直近 buffer は常に最大 relevance
        summary: (usage.summary as f32 / total).clamp(0.0, 1.0),
        entities: (usage.entities as f32 / total).clamp(0.0, 1.0),
        kg: (usage.kg as f32 / total).clamp(0.0, 1.0),
    }
}
```

**Phase 5 で `adjusted()` を活用する経路**:
- `compact_if_needed` 冒頭で `MemoryRelevance::current_from_messages(&messages, ...)` を計測
- `BudgetRatios::default().adjusted(&relevance)` で base ratio を動的調整
- `adjusted_ratios.allocate(total)` で `AllocatedBudget` を生成し、各 stage に伝播

**Phase 5 で実装しない (Phase 6 候補)**:
- AgentHER successful rate の long-term 統計 (experiment_log 経由)
- KG path success rate の cross-session 集計
- α 係数の auto-tuning (env override は維持)

---

## §3. TDD strict 3-phase outline

### Phase 1 (Red) — 8 failing test

| # | Test 名 | 検証内容 |
|---|---------|---------|
| 1 | `t_classify_buffer_role` | `Role::User` 末尾 N 件が `MemoryKind::Buffer` 判定 |
| 2 | `t_classify_summary_prefix` | `Role::Assistant` で content `"...[summarized]"` suffix が `MemoryKind::Summary` |
| 3 | `t_classify_entities_tool_call_id` | `Role::Tool` で tool_call_id `"agenther_xyz"` が `MemoryKind::Entities` |
| 4 | `t_classify_kg_tool_call_id` | `Role::Tool` で tool_call_id `"memory_search_1"` が `MemoryKind::Kg` |
| 5 | `t_measure_axis_usage_sums_correctly` | 4 軸 + unclassified の合計 == 全 message の token 合計 |
| 6 | `t_overflow_axes_descending` | usage > allocated の軸を超過量降順で返す |
| 7 | `t_compact_level1_with_budget_prunes_overflow_axis_first` | overflow=Kg のとき score 高 KG msg も Buffer 低 score msg より先に prune |
| 8 | `t_compact_if_needed_backward_compat_when_env_unset` | `BONSAI_DYNAMIC_BUDGET` unset で compact_level1 出力が `_with_budget(.., None)` と完全一致 |

**Red 期待**: 8 test 全 FAIL (helper / 種別判別が未実装)。1294 passed のうち 8 件 FAIL = 1286 passed。

#### Red test 例 (#7)

```rust
#[test]
fn t_compact_level1_with_budget_prunes_overflow_axis_first() {
    let mut msgs = vec![
        Message::system("s"),
        Message::user("q"),
        // KG 軸が overflow するように KG tool message を多めに配置
        Message::assistant("plan"),
        Message::tool_with_id("kg long content ...", "memory_search_1"),
        Message::tool_with_id("kg long content ...", "memory_search_2"),
        Message::tool_with_id("entity short", "agenther_x"),
        Message::user("recent"),
        Message::assistant("recent"),
    ];
    let config = CompactionConfig {
        max_context_tokens: 100,
        placeholder_keep_recent: 2,
        prune_protect_tokens: 30,
        prune_minimum_chars: 5,
        ..Default::default()
    };
    let allocated = AllocatedBudget {
        total: 100, buffer: 40, summary: 30, entities: 20, kg: 10,  // KG 10 だけが overflow 候補
    };
    compact_level1_with_budget(&mut msgs, &config, Some(&allocated));
    // KG msg が placeholder 化されている (overflow 軸優先 prune)
    assert!(msgs.iter().any(|m| m.content.starts_with("[prev:memory_search_")),
        "KG overflow 時に KG tool が最優先 prune される");
    // entity msg は短いので残る
    assert!(msgs.iter().any(|m| m.content == "entity short"),
        "entities 軸は overflow せず温存");
}
```

### Phase 2 (Green)

| 変更ファイル | 追加 | 削除 | 内容 |
|------------|------|------|------|
| `src/agent/compaction.rs` | +150 | -10 | `MemoryKind` / `classify_memory_kind` / `AxisUsage` / `measure_axis_usage` / `overflow_axes` / `compact_level1_with_budget` / `compact_level2_with_budget` / `compact_level1` wrapper / `compact_level2` wrapper / `MemoryRelevance::current_from_messages` |
| (`src/agent/compaction.rs` 内 test mod) | +120 | 0 | 8 test 実装 (Phase 1 で skeleton 済) |

**Green 期待**: 8 test 全 PASS、1294 → 1302 passed (+8)、clippy clean、fmt clean。

#### Green コード例 (`compact_level1_with_budget` 中核)

```rust
pub fn compact_level1_with_budget(
    messages: &mut [Message],
    config: &CompactionConfig,
    allocated: Option<&AllocatedBudget>,
) {
    let t = messages.len();
    if t <= config.placeholder_keep_recent { return; }

    // (1) 既存の boundary / protected 計算 (compact_level1 と同一)
    let keep_by_count = config.placeholder_keep_recent;
    let keep_by_tokens = { /* ... 既存 ... */ };
    let boundary = t.saturating_sub(keep_by_count.max(keep_by_tokens));
    if boundary == 0 { return; }
    let pairs = find_ai_tool_pairs(messages);
    let protected: HashSet<usize> = /* 既存 */;
    let first_user_idx = messages[..boundary].iter().position(|m| matches!(m.role, Role::User));
    let last_user_idx = messages[..boundary].iter().rposition(|m| matches!(m.role, Role::User));

    // (2) Phase 5 拡張: allocated があれば overflow 軸を計算
    let overflow_kinds: Vec<MemoryKind> = match allocated {
        Some(a) => {
            let usage = measure_axis_usage(&messages[..boundary], config.placeholder_keep_recent);
            overflow_axes(&usage, a).into_iter().map(|(k, _)| k).collect()
        }
        None => Vec::new(),  // env unset = 既存 score-only 経路
    };

    // (3) candidate ソート: overflow 軸の msg を高優先 prune、それ以外は既存 score 順
    let mut candidates: Vec<(usize, f64, bool)> = (0..boundary)
        .filter(|&i| !protected.contains(&i) && Some(i) != first_user_idx && Some(i) != last_user_idx)
        .map(|i| {
            let kind = classify_memory_kind(&messages[i], i, t, config.placeholder_keep_recent);
            let is_overflow = overflow_kinds.contains(&kind);
            (i, score_message_importance(&messages[i]), is_overflow)
        })
        .collect();
    // overflow=true を先頭、次に score 低位順
    candidates.sort_by(|a, b| {
        b.2.cmp(&a.2)  // overflow=true (=1) を先
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });

    // (4) 既存と同じ placeholder 化 (overflow 軸を先に処理)
    for (i, _score, _is_overflow) in &candidates {
        let msg = &mut messages[*i];
        if matches!(msg.role, Role::Tool) && msg.content.len() > config.prune_minimum_chars {
            let id = msg.tool_call_id.as_deref().unwrap_or("?");
            msg.content = format!("[prev:{id}]");
        }
    }
}

// backward compat wrapper
pub fn compact_level1(messages: &mut [Message], config: &CompactionConfig) {
    compact_level1_with_budget(messages, config, None);
}
```

### Phase 3 (Refactor)

- `classify_memory_kind` の prefix リストを `const` 化 (`AGENTHER_PREFIX = "agenther_"` 等)、tool_call_id
  命名規約変更時の SSOT
- `overflow_axes` の戻り値型を `Vec<(MemoryKind, usize)>` → `SmallVec<[(MemoryKind, usize); 4]>`
  検討 (最大 4 軸固定、heap allocation 削減)
- `MemoryRelevance::current_from_messages` を `pub` から `pub(crate)` に絞り、外部 API 不要なら
  internal hold
- clippy `-D warnings` clean、`cargo fmt -- --check` clean
- 項目 226 R5 pattern の cross-file env mutex を `BONSAI_DYNAMIC_BUDGET` test で標準適用
  (`pub(crate) static DYNAMIC_BUDGET_ENV_TEST_LOCK: Mutex<()>` 等)
- doc コメントで `// Phase 5: §X.Y 参照` リンク埋め込み (.claude/plan/dynamic-token-budget-phase5-axis-prune.md)

---

## §4. Phase 4 Smoke — G-11a/b/c/d 4 gate

| Gate | env | 期待 |
|------|-----|------|
| G-11a | env unset | 1 cycle smoke (15 task) で **Phase 4 (log-only) と完全一致**、backward compat 確証 (`compact_level1` wrapper 経路) |
| G-11b | `BONSAI_DYNAMIC_BUDGET=1` | 1 cycle で `overflow_axes` 発火 log emit (`[INFO][compaction.budget.overflow] kind=Kg amount=N`)、score 微変動 OK |
| G-11c | `BONSAI_DYNAMIC_BUDGET=1` + 長 KG response 強制 task | KG overflow 時に KG tool が先 prune される production trace 確証 |
| G-11d | `BONSAI_DYNAMIC_BUDGET_RATIOS="0.5,0.25,0.15,0.10"` + `BONSAI_DYNAMIC_BUDGET_ALPHA=0.3` | override + α 反映、log で `ratios = [0.5, 0.25, 0.15, 0.10] alpha=0.3` 確証 |

**Phase 4 smoke 完走条件**:
- 4 gate 全 PASS
- 1302 passed 維持 (test 退行ゼロ)
- production binary `cargo build --release` 完走、`./target/release/bonsai --lab --lab-experiments 0` が
  G-11a で Phase 4 baseline 同等 score (`±0.005` 以内)

---

## §5. Phase 5 Lab v22+ paired 実験設計

### 5.1 paired smoke 設計

- **対象**: 15 task smoke × k=3 × 5 cycle (= ~8h wall、Lab v22 同基準)
- **arm**:
  - arm A: `BONSAI_DYNAMIC_BUDGET=0` (Phase 4 と同等、log emit 無し / 4 軸 prune 無し)
  - arm B: `BONSAI_DYNAMIC_BUDGET=1` (Phase 5 完成形、4 軸 prune 有効)
- **計測 metric**:
  - **主軸**: paired Δscore (Wilcoxon signed-rank test + Cohen's dz、項目 247 Lab v22 Phase B 流用)
  - **副軸 1**: factcheck `matched / total` 軸 variance (Lab v20 structural finding 補強)
  - **副軸 2**: avg context tokens at L1/L2 trigger (4 軸 ratio の実 prune 効果計測)
  - **副軸 3**: AgentHER successful rate (entity 保護の効果)

### 5.2 ACCEPT / REJECT 基準

| 判定 | 主軸 | 副軸 1 | 副軸 2 | 副軸 3 |
|------|------|--------|--------|--------|
| ACCEPT | Δscore ≥ +0.005 / p < 0.10 (片側) | matched stdev 増 OR Pearson r ≥ 0.3 | KG overflow 時の avg buffer tokens 増 (実 prune 確証) | successful rate 増 OR 不変 |
| REJECT | 上記いずれも不成立 OR Δscore < 0 / p > 0.30 | (主軸 fail で確定) | (主軸 fail で確定) | (主軸 fail で確定) |
| 判定保留 | Δscore +0.000〜+0.005 で副軸 1-3 のうち 2 つ以上が positive | 改良 1-2 回後再試行 | | |

### 5.3 REJECT 時の代替案

- base ratio 変更: `BONSAI_DYNAMIC_BUDGET_RATIOS="0.5,0.3,0.15,0.05"` (Bonsai-8B 1bit で buffer 厚め)
  で paired 再試行
- α 係数変更: `BONSAI_DYNAMIC_BUDGET_ALPHA=0.1` (動的調整弱め) / `=0.3` (強め) で 2-3 回試行
- prefix-based 種別判別の不一致が原因なら、production AgentHER 経路の tool_call_id 命名を grep 検証
  し、`classify_memory_kind` の prefix リスト更新後再 Phase 4 smoke

---

## §6. Risks / Mitigations

| # | Risk | Severity | Mitigation |
|---|------|----------|-----------|
| R1 | `classify_memory_kind` の prefix が production 命名規約と不一致 (= 全 msg が Unclassified に倒れる silent bug) | **HIGH** | Phase 1 Red の test 4 件で実 prefix を hardcode、Phase 4 G-11b 起動前に `grep -rn "agenther_\\|memory_search_\\|kg_query_" src/` で production 出現確証 |
| R2 | `compact_level1_with_budget` で overflow 軸を先 prune した結果、AI+Tool ペアが壊れて整合不良 | **HIGH** | 既存 `find_ai_tool_pairs` の `protected` を継承、overflow 軸でも `protected` index は除外 (Phase 1 #7 test で確証) |
| R3 | `MemoryRelevance::current_from_messages` の集計 overhead で latency 増 | LOW | prefix 判定 O(1) × N message = O(N)、N=200 程度なら <0.5ms、measure_axis_usage 1 回 / compact 呼び出しのみ |
| R4 | 4 軸合計が `allocated.total` 超過時の scale_factor 適用で重要 buffer も削られる | MEDIUM | `allocated.buffer` には最小 floor (`total * 0.2`) を設定、項目 248 本体 plan §4 R4 と整合 |
| R5 | env on で Lab v22 REJECT 時の knowledge debt | LOW | REJECT 結果も memory に記録、ratio 4 軸の Bonsai-8B 1bit における実証 not-good を確証 (Lab v20 系列と整合) |
| R6 | `compact_level1` wrapper 経路で既存挙動と微妙な差 (例: `Vec` order 違い) | MEDIUM | Phase 4 G-11a で `Δscore=0 ±0.005` を確証、Phase 1 #8 test で wrapper の output 同値性確証 |
| R7 | production の MCP / memory tool が新規 tool_call_id 命名を導入 (将来) | LOW | `classify_memory_kind` の prefix リストを `const` SSOT 化 (Phase 3 で実施)、追加時は 1 行 const 追記のみ |
| R8 | Phase 5 完成後に項目 246 Vault lint や項目 244 KG lint との conflict (memory.rs 系) | LOW | compaction.rs はメッセージ列のみ touch、memory store (sqlite / KG / vault) には write しない。直交設計 |

---

## §7. 期待効果

### 7.1 1bit context 制約下での 4 軸保証

12,288 token context で「KG search 長文応答 1 件で buffer 圧縮」「entity 列挙過剰で summary 蒸発」を
構造的に予防。Phase 4 までは log 計測のみだったが、Phase 5 で **実際の prune 動作が 4 軸 budget に
従う**ようになる。

### 7.2 Lab v22+ paired の振る舞い変化軸確立

Phase 4 までは env on/off で挙動同一 (= Lab paired で観測できる差分なし)。Phase 5 で初めて
arm A vs arm B の **score / context structure / matched 軸 variance に観測可能な差分**が生じる。

Lab v20 structural finding (`(conf+matched+unknown)/total=1.0` deterministic、matched=0 で
variance ゼロ) に対する補強変動軸として、4 軸 ratio が新たな計測軸候補となる可能性。

### 7.3 Zenn 4 architecture 取り込みの完成

項目 248 本体 plan §5 で謳った「Zenn 記事の 4 ratio 配分」が **production code path に実体としても
反映**される (Phase 4 までは数値定義のみ、Phase 5 で prune logic に到達)。

---

## §8. 並行性 / 依存

### 8.1 完遂前提

- 項目 248 Phase 1-3 (BudgetRatios + allocate + adjusted + env getter) ✅ commit `5109219`
- 項目 248 Phase 4 wiring (CompactionConfig.budget_ratios + with_dynamic_budget_from_env + log emit) ✅ followup commit
- 項目 244 KG lint 完遂 ✅ (KG 軸 input quality 担保)
- 項目 246 Vault lint Phase 1-4 完遂 ✅ commit `30a38d6` (summary 軸 input quality 担保)

### 8.2 並行可

- 項目 247 Lab v22 Phase A-D (paired metric redesign) の完遂後着手推奨だが、Phase 5 Phase 1-3 (実装) は
  Lab paired 起動と独立 (production binary は Phase 4 smoke 完了後にのみ更新)
- 項目 251 候補 (Vault lint bail branch test) と本 Phase 5 は ファイル独立 (`vault_lint.rs` vs `compaction.rs`)、並行可

### 8.3 排他

- compaction.rs 編集中は Lab paired 起動禁止 (production binary 不整合 risk、項目 248 本体 plan §7
  排他規約継承)
- Phase 4 smoke (cargo build --release) は Lab 同時稼働不可

### 8.4 Phase 5 完遂後の Phase 6 候補

- **Phase 6a**: `MemoryRelevance` を `experiment_log` 経由 long-term 集計に拡張 (項目 226 系列の
  pattern 適用、env-gated)
- **Phase 6b**: α 係数 auto-tuning (Lab paired Δscore 最大化を gradient で探索、別 plan で起票検討)
- **Phase 6c**: 4 軸定義の意味的精緻化 (現状 prefix-based → semantic embedding-based)、ただし
  項目 226 R5 finding (Uncertain 92.3%) の制約から優先度低

---

## §9. ロールバック戦略

- 全変更は `Option<AllocatedBudget>` 引数追加 + env-gated 分岐のみ
- `compact_level1` / `compact_level2` は wrapper 化、public API 互換維持
- env unset = `allocated=None` で完全な従来動作 (Phase 4 G-11a / Phase 1 #8 で確証)
- 即時 rollback = `unset BONSAI_DYNAMIC_BUDGET` で disable (binary 再 build 不要)
- 完全 rollback = `git revert <Phase 5 commit>` で 3 commit reversal (Phase 1 / Phase 2 / Phase 3)
- Phase 5 commit 後に Phase 4 baseline と smoke score 不一致が出た場合、Phase 4 wiring の log emit
  まで rollback (Phase 5 の `compact_level1_with_budget` のみ revert)

---

## §10. Quick Start

```bash
cd /Users/keizo/bonsai-agent
git log -3 --oneline  # Phase 4 wiring 完了確認

# Pre-Phase 1: production の tool_call_id 命名規約 grep 確証 (R1 mitigation)
grep -rn "agenther_\\|memory_search_\\|kg_query_\\|graph_search_" src/ | head -20

# Phase 1 Red — 8 failing test 追加
$EDITOR src/agent/compaction.rs  # MemoryKind / classify / measure / overflow / wrapper skeleton
cargo test --lib --quiet -- t_classify t_measure t_overflow t_compact_level1_with_budget t_compact_if_needed_backward_compat 2>&1 | tail -20
# Expected: 8 FAIL

# Phase 2 Green — 実装
cargo test --lib  # 1294 → 1302 passed (+8)
cargo clippy -- -D warnings
cargo fmt -- --check

# Phase 3 Refactor + commit
git add -A && git commit -m "feat(compaction): 項目 248 Phase 5 — 4 軸 prune wiring (案 C)"

# Phase 4 Smoke G-11a/b/c/d
cargo build --release  # binary 更新
./target/release/bonsai --lab --lab-experiments 0 2>&1 | tail -20  # G-11a (env unset)
BONSAI_DYNAMIC_BUDGET=1 ./target/release/bonsai --lab --lab-experiments 0 2>&1 | grep "compaction.budget"  # G-11b
BONSAI_DYNAMIC_BUDGET=1 ./target/release/bonsai --lab --lab-experiments 0 --task-id kg_long_response 2>&1 | grep "overflow"  # G-11c
BONSAI_DYNAMIC_BUDGET=1 BONSAI_DYNAMIC_BUDGET_RATIOS="0.5,0.25,0.15,0.10" BONSAI_DYNAMIC_BUDGET_ALPHA=0.3 \
  ./target/release/bonsai --lab --lab-experiments 0 2>&1 | grep "compaction.budget"  # G-11d

# Phase 5 Lab v22 paired (別 session、~8h wall、Phase 4 smoke 4/4 PASS 後)
nohup ./scripts/lab_v22_paired_dynamic_budget.sh ./lab-v22-phase5-logs > /tmp/lab_v22_phase5.log 2>&1 &
# ~8h 後
python3 scripts/lab_v22_metric.py ./lab-v22-phase5-logs
```

---

## §11. metadata

- **起点 commit**:
  - `2546d79` (項目 248 Phase 1 Red)
  - `5109219` (項目 248 Phase 2 Green + Phase 3 Refactor)
  - Phase 4 wiring followup commit (log emit hook + CompactionConfig.budget_ratios field)
- **関連 plan**:
  - `.claude/plan/dynamic-token-budget-compaction.md` (項目 248 本体、本 plan は §3.5 Phase 5 詳細化)
  - `.claude/plan/vault-lint-coverage-check.md` (項目 246、summary 軸 input quality)
  - `.claude/plan/lab-v22-metric-redesign.md` (項目 247、Phase 5 paired 計測 metric 流用)
  - `.claude/plan/assistant-message-event-emit-fix.md` (項目 236、event emit hook 設計の参考)
- **関連 memory**:
  - `context_failure_modes_audit_2026_05_19.md` (4 軸保証で Poisoning / Distraction / Confusion を構造的予防)
  - `ternary_bonsai_paths_2026_05_19.md` (Ternary primary 切替後の context budget 検証連携)
- **想定 commit 範囲**: 3 commits (Phase 1 Red / Phase 2 Green / Phase 3 Refactor)
- **想定 line 範囲**: +280 行 / -10 行 (src/agent/compaction.rs に集中、mod 構造影響なし)
- **起票日**: 2026-05-19
- **起票根拠**:
  - Phase 4 critic adversary F5 finding "intentional dead-data flow" の解消
  - Lab v22+ paired での **観測可能な振る舞い変化軸**確立
  - Zenn 4 architecture 取り込みの完成 (Phase 1-4 は数値定義 + log 計測、Phase 5 で prune 動作到達)
