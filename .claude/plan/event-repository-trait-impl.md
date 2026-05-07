# Plan: EventStore Trait 化 — Clean Architecture 部分強化 (Option B)

> **由来**: 項目 208 完了後の user feedback 「クリーンアーキテクチャに沿ってるか」への回答として **Option 3 (部分的強化)** を選択。`MemoryStore::conn()` の external callsite が **160 箇所** 存在し full refactor は 1-2 週間規模 (= research 速度トレードオフで非推奨) のため、**直近 plan 3 件 (ERL / Self-Verify / AgentFloor) のうち 2 件が EventStore 経由でアクセスする** 点に着目し、**EventStore のみを trait 化** する focused refactor。
>
> **目的**: dependency inversion + mock impl で test 容易性向上、ERL / Self-Verify 実装時に `&dyn EventRepository` で受け取り可能にし、SQLite なしで unit test 可能化。

## Task Type
- [ ] Frontend
- [x] Backend (`src/agent/event_store.rs` trait 抽出、callers の段階的移行)
- [ ] Fullstack

## 1. 背景

### 1.1 user feedback (項目 208 後)
「クリーンアーキテクチャに沿ってるよね？」への正直回答:
- **同心円層 (Entities → Use Cases → Adapters → Frameworks) 不在**
- **trait による依存性逆転は守れている** (`LlmBackend` / `Tool` / `Sandbox` / `Embedder`)
- **`MemoryStore::conn() -> &Connection` の external callsite 160 箇所** = 真の違反箇所、infrastructure 詳細 (rusqlite) の漏洩

### 1.2 scope 判断 (Option A vs B vs C)
| approach | 時間 | 効果 |
|---|---|---|
| A. `MemoryStore` 全体浅い trait 化 (`conn()` も含める) | 3-4h | 形式的のみ、160 callsite はそのまま |
| **B. EventStore に絞った深い refactor (本 plan)** | **6-8h** | **ERL / Self-Verify 実装の test 容易性が実質改善** |
| C. フル refactor (`conn()` 完全隠蔽、160 全改修) | 1-2 週間 | 真の Clean Architecture |

**B 採用根拠**:
1. 直近 plan 3 件 (項目 208) のうち ERL (`extract_heuristics_from_events`) / Self-Verify (`verification_success_rate`) が EventStore 拡張を含む → trait 化が implementation 直接効くタイミング
2. EventStore は AgentHER (項目 201-205) / Beyond pass@1 (項目 200) で recently 拡張、test 容易性の dividend が高い
3. 160 全改修は research 本筋 (天井打開、項目 207) を遅らせる over-engineering

### 1.3 Rust 特有の制約
- `EventStore<'a>` は `&'a Connection` を保持 → trait 抽象化時に lifetime 取り回し検討必要
- 採用方針: trait 定義は **lifetime-free** (`fn append(&self, ...)` 等)、impl 側で lifetime 持つ (`impl EventRepository for EventStore<'_>`)
- 既存 caller は `&EventStore` (concrete) のまま、新規 caller は `&dyn EventRepository` も使用可 (gradual migration、breaking change なし)

## 2. 目的

1. **EventRepository trait 定義** — EventStore の 9 public method を trait 化、SQLite 詳細を概念抽象から分離
2. **既存 EventStore に trait impl 追加** — non-breaking change、既存 21 callsite (EventStore::new) は無変更
3. **MockEventRepository 提供** — `src/memory/mocks/event_repository_mock.rs` で in-memory Vec<Event> ベースの mock impl、SQLite なしで unit test 可能
4. **直近 plan 2 件への適用準備** — ERL / Self-Verify が `&dyn EventRepository` を受けられるよう foundation
5. **後続 store (SkillStore / ExperienceStore / Vault) の trait 化テンプレート** — 本 plan が pattern を確立、後続は半分の工数で実施可

### 非目標
- `MemoryStore::conn()` の隠蔽 (Option C 範囲)
- 既存 21 callsite の `&EventStore` → `&dyn EventRepository` 移行 (本 plan は trait 提供のみ、callers は別 plan で段階移行)
- 他 store (SkillStore / ExperienceStore / Vault) の trait 化 (後続候補)

## 3. 既存項目との関係

| 項目 | 関係 |
|---|---|
| 162 (EventStore Option B export hook) | trait 化の対象、既存 API 維持 |
| 200 (Beyond pass@1) | EventStore 拡張済、trait 化で test 容易性 dividend |
| 201-205 (AgentHER) | `extract_failed_trajectories_since_id` 等を trait method に含める |
| 206 (`current_max_id` helper) | trait method として追加 |
| 208 plan #1 (ERL) | `extract_heuristics_from_events` が `&dyn EventRepository` を受ける design に変更可能 (将来 PR で) |
| 208 plan #3 (Self-Verify) | `verification_success_rate` が trait method として追加される予定 |

## 4. 設計

### 4.1 EventRepository trait 定義 (新規 `src/agent/event_repository.rs` または event_store.rs 内)

```rust
/// Event 永続化抽象。SQLite/in-memory 詳細から callers を分離。
/// 実装: `EventStore` (本番、SQLite-backed) / `MockEventRepository` (test、Vec-backed)
pub trait EventRepository: Send + Sync {
    /// Event を追加し、付与された id を返す
    fn append(
        &self,
        kind: EventKind,
        session_id: &str,
        payload: &str,
    ) -> Result<i64>;

    /// session_id 内の全 event を id 昇順で返す
    fn get_session_events(&self, session_id: &str) -> Result<Vec<Event>>;

    /// session_id 内の tool 呼び出し回数 ((tool_name, count) の tuple Vec)
    fn count_tool_calls_per_session(&self, session_id: &str) -> Result<Vec<(String, usize)>>;

    /// 全 event 数
    fn total_count(&self) -> Result<usize>;

    /// 既知 session_id 一覧 (debug 用、distinct)
    fn list_sessions(&self) -> Result<Vec<String>>;

    /// 全 event から成功 trajectory 抽出 (project pattern: AgentHER HSL)
    fn extract_successful_trajectories(
        &self,
        min_score: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>>;

    /// since_event_id より新しい event のみから成功 trajectory 抽出 (Lab cycle scoping、項目 162)
    fn extract_successful_trajectories_since_id(
        &self,
        since_event_id: i64,
        min_score: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>>;

    /// 同上、失敗側
    fn extract_failed_trajectories(
        &self,
        max_score: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>>;

    /// 同上、scoping 付き失敗
    fn extract_failed_trajectories_since_id(
        &self,
        since_event_id: i64,
        max_score: f64,
        min_steps: usize,
    ) -> Result<Vec<TrajectoryCandidate>>;

    /// 現時点の events.id MAX (Lab cycle 開始時 snapshot 用、項目 206)
    fn current_max_id(&self) -> Result<i64>;
}
```

### 4.2 既存 EventStore に impl 追加

```rust
impl<'a> EventRepository for EventStore<'a> {
    fn append(&self, kind: EventKind, session_id: &str, payload: &str) -> Result<i64> {
        // 既存の inherent method を委譲呼び出し (lossless)
        self.append(kind, session_id, payload)
    }

    fn get_session_events(&self, session_id: &str) -> Result<Vec<Event>> {
        self.get_session_events(session_id)
    }
    // ... 他 7 method も同パターン
}
```

**ポイント**: 既存 inherent method (`impl<'a> EventStore<'a> { pub fn append(...) }`) は **削除しない**。trait method はそれを委譲呼び出し。
- 既存 caller (21 箇所) は `EventStore::new(conn).append(...)` のまま動く
- 新規 caller は `let repo: &dyn EventRepository = &store; repo.append(...)` として呼べる
- 非 breaking、段階移行可能

### 4.3 MockEventRepository (新規 `src/memory/mocks/event_repository_mock.rs`)

```rust
use std::sync::Mutex;

/// In-memory Vec ベース mock。test 専用、Send + Sync を満たす。
pub struct MockEventRepository {
    events: Mutex<Vec<Event>>,
    next_id: Mutex<i64>,
}

impl MockEventRepository {
    pub fn new() -> Self {
        Self { events: Mutex::new(Vec::new()), next_id: Mutex::new(1) }
    }

    /// test fixture 用、scenario seed
    pub fn seed_event(&self, kind: EventKind, session_id: &str, payload: &str) -> i64 {
        let mut events = self.events.lock().unwrap();
        let mut id = self.next_id.lock().unwrap();
        let event = Event { id: *id, kind, session_id: session_id.to_string(), payload: payload.to_string(), ts: "2026-05-08T00:00:00Z".to_string() };
        events.push(event);
        let new_id = *id;
        *id += 1;
        new_id
    }
}

impl EventRepository for MockEventRepository {
    fn append(&self, kind: EventKind, session_id: &str, payload: &str) -> Result<i64> {
        Ok(self.seed_event(kind, session_id, payload))
    }

    fn get_session_events(&self, session_id: &str) -> Result<Vec<Event>> {
        Ok(self.events.lock().unwrap().iter().filter(|e| e.session_id == session_id).cloned().collect())
    }
    // ... 他 method も in-memory 実装 (~120 行 total)
}
```

`#[cfg(any(test, feature = "test-utils"))]` で gate するか検討 (production binary 含めるかは Phase 3 で判断)。

### 4.4 後方互換性

- 既存 21 callsite: 無変更で動く (`EventStore::new(store.conn()).append(...)`)
- 新規 caller: `fn process(repo: &dyn EventRepository)` で受けられる
- ERL plan / Self-Verify plan は本 plan merge 後に **任意で** trait-based design へ書き直せる (本 plan が強制しない)

## 5. TDD strict 5 phase

### Phase 1 — Red (test 先行、~1h)
新規 test file: `src/agent/event_store_trait_tests.rs` または既存 `event_store.rs` test mod に追加。

1. `test_event_store_impls_event_repository` — `let _: &dyn EventRepository = &store;` でコンパイル確認 (compile-time guarantee)
2. `test_mock_event_repository_append_and_get` — Mock の append → get_session_events で seed 確認
3. `test_mock_event_repository_extract_failed_filters_by_score` — Mock seed 5 events (3 fail / 2 success) → extract_failed で 3 件返却
4. `test_mock_event_repository_current_max_id_empty_returns_zero` — 既存 EventStore test と同等を Mock でも確認
5. `test_event_repository_trait_object_can_be_passed` — `fn helper(repo: &dyn EventRepository) -> Result<i64> { repo.total_count() }` を呼び出し、`&store` (concrete) と `&mock` (mock) 両方で動作確認

期待: 全 5 fail (compile error: trait undefined / Mock undefined)、commit `test(event-repo): Phase 1 Red — EventRepository trait + MockEventRepository tests`

### Phase 2 — Green (実装、~3h)
1. `EventRepository` trait 定義 (`src/agent/event_repository.rs` 新規 or `event_store.rs` 内 module top)
2. `impl<'a> EventRepository for EventStore<'a>` 追加 (9 method 全部、既存 inherent method への委譲)
3. `MockEventRepository` 新規 (`src/memory/mocks/event_repository_mock.rs` ~150 行 + `mocks/mod.rs` 整備)
4. 必要に応じて `Event` / `TrajectoryCandidate` / `EventKind` の `pub use` 整理 (caller が trait 経由で型を取り回せるよう)

期待: 1058 + 5 = **1063 PASS** + clippy 0 + fmt 0、commit `feat(event-repo): Phase 2 Green — EventRepository trait + Mock impl`

### Phase 3 — Refactor (~1h)
1. trait method docstring 整備 (clean architecture motivation 引用)
2. MockEventRepository を `#[cfg(any(test, feature = "test-utils"))]` で gate するか判断
   - production size 影響軽微 (~150 行) なら gate 不要
   - feature flag 採用なら `Cargo.toml` `[features]` 追加
3. `tool_chain_key` ヘルパー (TrajectoryCandidate impl) を trait に含めるか判断 (現状 inherent method、trait に含めると mock 実装側で重複)
   - **判断**: trait 外に維持 (TrajectoryCandidate の inherent method は Event-agnostic)
4. clippy / fmt clean、commit `refactor(event-repo): Phase 3 — docstring + feature gate`

### Phase 4 — Smoke (~30 min、production code 動作変更ゼロ前提)
- `cargo build --release`
- `BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0` 5 task k=3 で 1 cycle
- 観測:
  - 既存 EventStore caller が trait 化後も同じ event を append している (events DB の row 数が変化なし)
  - Lab smoke score が項目 207 baseline (0.7344) と ±0.02 範囲 (退行ゼロ確認)
  - clippy 0 / fmt 0

期待: production 動作変更ゼロ、smoke は trait 化前後で binary equivalent

### Phase 5 — Docs + handoff (~30 min)
- CLAUDE.md 項目 209 として記録 (EventRepository trait 化完遂、後続 store の template 確立)
- handoff 05-08c に「EventStore trait 化済、ERL / Self-Verify 実装時に `&dyn EventRepository` 採用検討」明記
- commit `docs(claude.md): 項目 209 — EventRepository trait 化完遂`

## 6. API 影響

| API | 変更 | 後方互換 |
|---|---|---|
| `EventRepository` trait | 新規 9 method | — |
| `EventStore<'a>` inherent method | **無変更** | ✅ 100% |
| `EventStore<'a>::EventRepository` impl | 新規追加 | ✅ additive |
| `MockEventRepository` | 新規 | — |
| 既存 21 callsite (`EventStore::new`) | **無変更** | ✅ 100% |
| 既存 16 method callsite | **無変更** | ✅ 100% |

**signature 変更ゼロ** — 完全 additive。

## 7. Risks

| # | risk | severity | mitigation |
|---|------|----------|------------|
| R1 | trait method と inherent method の **重複** で clippy `same_name_method` 警告 | LOW | trait method docstring に「inherent への委譲」明記、必要なら `#[allow(clippy::same_name_method)]` |
| R2 | Send + Sync 制約で `EventStore<'a>` が trait satisfy しない (Connection が `Send` でない可能性) | MEDIUM | `rusqlite::Connection` は `Send` 実装あり (`Mutex<Connection>` でないため `Sync` でないが、`&Connection` は `Sync`)。trait の `Send + Sync` bound を `Send` のみに緩和して様子見、Phase 1 Red で確認 |
| R3 | MockEventRepository を production binary に含めることで size 増 | LOW | ~150 行 (10KB 未満) なら無視可。feature gate なら `[dev-dependencies]` にせず `[features] test-utils = []` で opt-in |
| R4 | trait method の細かい挙動差 (例: `extract_failed_trajectories_since_id` の境界条件) が SQLite impl と Mock impl で異なる | HIGH | Phase 1 で **同じテストケースを SQLite と Mock 両方で実行** する macro を導入、parity 強制 |
| R5 | 後続 store (SkillStore 等) の trait 化を本 plan で同時に実施したい誘惑 | MEDIUM | 本 plan は **EventStore のみ**、scope creep 防止。後続 store は別 plan |
| R6 | `TrajectoryCandidate` の Event 依存で trait 抽象化困難 | LOW | TrajectoryCandidate は構造体 (Event ではない)、trait method の戻り値として安全に返却可 |
| R7 | `ERL plan / Self-Verify plan` の implementation 時に「trait 受けにすべきか具体型受けにすべきか」の判断分岐 | LOW | 本 plan は **基盤提供のみ**、消費側 plan が判断。ERL/Self-Verify plan の Phase 1 で個別決定 |

## 8. Gates

| Gate | 内容 | 検証 | 必須 |
|---|---|---|---|
| **G-1 Red** | 5 新規 test compile error or fail | `cargo test --lib event_repo` | 必須 |
| **G-2 Green** | 1058 + 5 = 1063 PASS + clippy 0 + fmt 0 + Send + Sync 制約 OK | `cargo test/clippy/fmt --release` | 必須 |
| **G-3 Refactor** | docstring 完備 + feature gate 判断完了 + 既存 21 callsite 退行ゼロ | code review | 必須 |
| **G-4 Smoke** | smoke 1 cycle で events DB row 数 / score / duration 全て trait 化前後で equivalent | smoke run + log 比較 | 必須 |
| **G-5 Final** | Mock 経由の unit test が SQLite なしで動作 (test_mock_event_repository_* が SQLite test と独立) | `cargo test --lib event_repo` 単独実行 | 必須 |
| **G-6 Parity** | 同じ scenario の test を SQLite/Mock 両 backend で実行し結果一致 | parametrized test | 必須 |

G-2 / G-4 / G-6 が NG なら 棚保留 (revert)、G-3 / G-5 NG は次セッション継続。

## 9. 見積もり

| Phase | 内容 | 時間 |
|---|---|---|
| Phase 0 | EventStore method 詳細読込 + Send/Sync 制約確認 | 0.5h |
| Phase 1 (Red) | 5 test 記述 + 不合格確認 | 1.0h |
| Phase 2 (Green) | trait + EventStore impl + MockEventRepository (~150 行) | 3.0h |
| Phase 3 (Refactor) | docstring / feature gate / clippy | 1.0h |
| Phase 4 (Smoke) | smoke 1 cycle + 退行確認 | 0.5h (実機 ~15 min + 分析) |
| Phase 5 (Docs) | CLAUDE.md 項目 209 + handoff + commit | 1.0h |
| Buffer | Send/Sync trait bound 調整、parity test macro 設計 | 1.0h |
| **合計** | | **~8h、1 day** |

## 10. 後続候補

1. **SkillStore / ExperienceStore の同様 trait 化** — 本 plan の pattern を copy、各 ~6h で実施可
2. **Vault の trait 化** — `~/.config/bonsai-agent/vault/` の md filesystem detail を `VaultRepository` trait で隠蔽 (~8h、I/O abstract のため重め)
3. **MemoryStore::conn() の段階廃止** — repository trait が出揃った時点で `MemoryStore` から `conn()` 削除、上記 1+2 完遂後 +4h
4. **integration test framework の Mock 化** — Lab smoke を MockEventRepository で SQLite-less に走らせる選択肢、CI 高速化候補

## 11. Quick Start

```bash
# 0. Pre-flight check
rtk grep -rn "EventStore::new\|EventStore<" src/ --include="*.rs"  # 21 callsite 一覧
rtk grep -n "Send\|Sync" src/agent/event_store.rs                  # 既存 trait bound

# 1. Phase 1 Red
$EDITOR src/agent/event_store.rs  # trait stub + 5 test
rtk cargo test --lib event_repo  # 期待: compile error or fail

# 2. Phase 2 Green
$EDITOR src/agent/event_store.rs              # trait 完成 + impl
$EDITOR src/memory/mocks/event_repository_mock.rs  # 新規
$EDITOR src/memory/mocks/mod.rs               # 新規 module
$EDITOR src/memory/mod.rs                     # pub mod mocks
rtk cargo test --lib --release | tail -3  # 期待: 1063 passed

# 3. Phase 3 Refactor
$EDITOR src/agent/event_store.rs              # docstring
rtk cargo clippy --release -- -D warnings && rtk cargo fmt --check

# 4. Phase 4 Smoke
rtk cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 比較対象: 項目 207 smoke baseline 0.7344

# 5. Phase 5 Docs
$EDITOR CLAUDE.md  # 項目 209 追記
git add -A && git commit -m "docs(claude.md): 項目 209 — EventRepository trait 化完遂"
```

## 12. 参考

- 項目 162 (EventStore Option B export hook + scoping)
- 項目 200 (Beyond pass@1)
- 項目 201-205 (AgentHER, EventStore 拡張)
- 項目 206 (current_max_id helper)
- 項目 208 (arxiv 構造変異 plan 3 件、本 plan は支援基盤)
- Robert C. Martin "Clean Architecture" Chapter 22 (Repository pattern)
- Rust idiomatic: trait + impl pattern (`std::io::Write` / `std::io::Read` 風)
- 関連 plan: `agenther-option-a-migration.md` (TDD strict 構成 reference)
- CLAUDE.md 項目候補: 209 (本 plan 完遂時)
