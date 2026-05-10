# Plan: ds4 (DeepSeek V4 Flash) 知見の bonsai-agent port

> **由来**: antirez/ds4 リポジトリ (https://github.com/antirez/ds4、5,036 stars、2026-05-06 created、極めてアクティブ) の調査結果に基づく適用 plan。**ds4 は "DeepSeek V4 Flash inference engine" であり "data structures 4" ではない**。本 plan は ds4 の設計哲学 + 具体的機構 4 件を bonsai-agent (1bit Bonsai-8B / M2 16GB / Rust 製自律エージェント) に **段階的 port** する戦略を定義する。
>
> **由来 research**: 本 session の ds4 deep dive (agent ID `a90de6a8...`、bonsai 応用候補 5 個を優先度付きで判定済)
>
> **関連 plan**: `agentfloor-tier-eval-impl.md` (TDD strict 5 phase 構造) / `cerememory-decay-port-impl.md` (外部 OSS port pattern、env opt-in 確立) / `cerememory-review-state-v12-impl.md` (Plan B、Cerememory 三本柱の中核)

## Task Type
- [ ] Frontend
- [x] Backend (`runtime/llama_server.rs` 起動オプション拡張、`memory/skill.rs` rax 候補化、`agent/event_store.rs` tool_id replay 拡張)
- [ ] Fullstack
- [x] Docs (CLAUDE.md 項目 223 / `memory/ds4_alignment.md` 新規)

## 1. 背景

### 1.1 ds4 リポジトリ要点
| 項目 | 値 |
|---|---|
| 正式名 | `ds4.c` (DeepSeek V4 Flash inference engine) |
| 言語 | C99 (1.37 MB) + Objective-C Metal runtime (0.64 MB) + 19 Metal kernels |
| target | DSv4 Flash 専用、generic GGUF runner ではない |
| stars | 5,036 / forks 352 |
| 作成 | 2026-05-06 (本 session 4 日前) |
| license | MIT |
| antirez 哲学 | シンプル至上 / 低依存 / 1 model 1 path / readability / "Why" コメント / disk-first KV cache |

### 1.2 antirez 哲学と bonsai 「Scaffolding > Model」原則の対応
- ds4 = **逆方向** (Model > Scaffolding、1 モデル専用に scaffolding を絞る)
- 一方で **方法論** (狭スコープ / 低依存 / correctness 優先 / disk-first cache) は bonsai 哲学と完全一致
- 特に **「KV cache を disk first で設計する」** は 1bit Bonsai-8B (1.28 GB) + M2 16GB の RAM 制約下に正面から効く設計判断

### 1.3 ds4 内の同梱 antirez DS
- **rax.c** (Redis Adaptive Radix tree、103+14 KB) — antirez 単独著作、Redis 由来 (2017-2018)
- ds4 内では `tool_id_replay_map` (`by_id` + `by_block` 2 つの rax) として **DSML ツール呼び出しブロックの byte-for-byte 再現** に使用
- bonsai 既存 `SkillStore` (項目 13/179) / `HeuristicStore` (項目 213) の prefix 検索高速化候補

### 1.4 bonsai 既存実装範囲
- llama-server HTTP API 経由推論 (`src/runtime/llama_server.rs`)
- KV cache: llama-server 内蔵、bonsai 側は **未活性** (`--cache-prompt` / `--slot-save-path` 未指定推定)
- ContextOverflowGuard (項目 187): n_ctx burst 対策、prefill 再走発生時に検知
- skill prefix 検索: SQLite FTS5 + LIKE (rax 比較対象)
- `event_store` (項目 209 EventRepository trait 化済): tool_call イベント記録

## 2. 目的
1. **ds4 哲学の体系的取込** — 4 候補 (KV cache / rax / replay map / API 極小化) を優先度判定し段階 port
2. **Lab cycle 時間 -10〜30% 短縮** — Stage 1 disk-first KV cache 採用で prefill 再走削減 (Lab core 22 / 60-90 min/cycle が支配的)
3. **天井 7 連続打開仮説** — KV cache 高速化により paired t-test の運用コスト低減 → 変異探索回数 +50% 期待 (項目 215 Lab v17 REJECT 後の打開経路)
4. **Cerememory 三本柱 (項目 217-219) の next 候補化** — env opt-in pattern 踏襲で同 quality 確保

### 非目標
- ds4 そのもの (DSv4 専用 inference engine) の bonsai 取込 (DSv4 = 284B params, 2bit 81GB、Bonsai-8B = 1.28GB と規模違い)
- ds4 の C コード逐語 port (Rust idiomatic に再設計、ただし algorithm parity 保持)
- Stage 1 完遂前の Stage 2/3 着手 (依存関係: KV cache → replay map / rax は独立)
- linenoise 対話モード port (現 Lab 自動化と競合しない nice-to-have、優先度外)

## 3. 既存項目との関係
| 項目 | 関係 |
|---|---|
| 173/183/184/195/198 (MLX 系) | KV cache 機構は backend agnostic、MLX/llama-server 両方に効く |
| 186/187 (ContextOverflowGuard) | KV cache 永続化で context overflow 後の再開が高速化、項目 187 と相補 |
| 207/212/215 (Lab 天井 7 連続) | KV cache による cycle 時間短縮 → paired t-test 試行回数増 |
| 213 (ERL Heuristics Pool) | rax port で heuristic pool 134 件の prefix 検索 O(N)→O(key_len) |
| 217-219 (Cerememory 三本柱) | 同 env opt-in pattern (`BONSAI_*_ENABLED`) で Stage 1 設計統一 |
| 220-222 (sqlite-vec wiring 削除) | 外部 OSS 採否判定の前例、本 plan も同等の Lab paired smoke 必須 |
| 209 (EventRepository trait 化) | Stage 3 tool_id replay map で `event_store` 拡張、trait 経由で SQL parity 保証 |

## 4. 設計

### 4.0 Stage 構成と依存関係
| Stage | 候補 | 優先度 | 依存 | 工数 | 別 plan 化 |
|---|---|---|---|---|---|
| **Stage 1** | Disk-first KV cache wiring | ★★★ | なし | ~1 day | 本 plan で完結 |
| **Stage 2** | rax port (skill/heuristic prefix index) | ★★ | なし (独立) | ~1.5 day | `ds4-rax-skill-index-impl.md` 派生起票 |
| **Stage 3** | tool_id replay map | ★★ | Stage 1 完遂後 | ~4-6h | `ds4-tool-id-replay-impl.md` 派生起票 |
| **Stage 4** | 公開 API 極小化哲学 (CLAUDE.md 追記) | ★ | なし | ~30 min | 本 plan で同梱 |
| **Stage 5** | linenoise/rustyline 対話モード | — | 不採用 | — | 起票しない |

### 4.1 Stage 1: Disk-first KV cache wiring (本 plan 主体)

#### 4.1.1 既存 llama-server 起動経路の確認
- bonsai は **llama-server を user 起動前提** (CLAUDE.md ビルド・テストコマンド section に起動手順なし)
- `src/runtime/llama_server.rs` は HTTP client、起動コマンド指定なし
- 本 plan は **bonsai 側 config + ドキュメント整備** で対応 (llama-server 自体の起動コマンドはユーザー操作)

#### 4.1.2 設定経路
**A. Config 拡張** (`src/config.rs::AgentSettings`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlamaServerSettings {
    /// llama-server cache-prompt 機能の有効化フラグ
    /// (cli から起動する llama-server の `--cache-prompt` 相当を bonsai 側で expect)
    #[serde(default)]
    pub kv_cache_enabled: bool,
    /// disk KV cache 保存先 (None = ~/.cache/bonsai-agent/kv)
    #[serde(default)]
    pub kv_cache_dir: Option<PathBuf>,
}
```

**B. env opt-in** (`BONSAI_KV_CACHE_ENABLED=1`):
- `LlamaServerBackend::new()` で env 確認、未設定で従来挙動
- 設定時は `slot_save_path` を request body に追記 (llama-server v1.0+ 対応)

**C. Lab cycle 計測**:
- 既存 `MultiRunBenchmarkResult::duration_secs` で計測
- 新規 metric 不要 (Beyond pass@1 RDC/VAF 項目 200 と同様の informational のみ)

#### 4.1.3 llama-server 側起動オプション (ドキュメント)
新規 `docs/kv_cache_setup.md` に記載:
```bash
llama-server \
  --model bonsai-8b-1bit.gguf \
  -c 16384 \
  --flash-attn on \
  --cache-prompt \
  --slot-save-path ~/.cache/bonsai-agent/kv \
  --slot-prompt-similarity 0.5
```

`--slot-prompt-similarity 0.5` で prefix 部分一致でも cache hit (ds4 README の 50% threshold に倣う)。

#### 4.1.4 KV cache 動作検証
**Smoke test** (`#[ignore]` integration test、`cargo test -- --ignored`):
1. **同一 prompt 2 回連続** → 2 回目の latency が 1 回目の 50% 以下
2. **prefix 共通の 2 prompt** → 2 回目の prefill token 数が `--slot-prompt-similarity` 設定通り削減
3. **異なる prompt** → cache miss、latency 変化なし

### 4.2 Stage 2: rax port (派生 plan で起票)

**派生 plan**: `.claude/plan/ds4-rax-skill-index-impl.md` (~600 行、別 session で起票)

要点:
- rax-rs crate (community 既存) または `radix_trie` crate を採用
- bonsai 側は `SkillRepository::find_by_prefix(&str)` メソッド追加
- env: `BONSAI_RAX_INDEX_ENABLED=1`
- Lab paired t-test (項目 220-222 と同 pattern) で score/duration を SQLite FTS5 比較
- ACCEPT: latency -50% 以上 + score ±0.02 以内

### 4.3 Stage 3: tool_id replay map (派生 plan で起票、Stage 1 後)

**派生 plan**: `.claude/plan/ds4-tool-id-replay-impl.md` (~400 行、Stage 1 ACCEPT 後に起票)

要点:
- `EventStore` に `original_emitted_text: String` 列追加 (SCHEMA_V11 → V12)
- `parse.rs` で `<tool_call>` 抽出時に元 text を保存
- 再 prompt rendering 時に保存済 text を優先、JSON フィールド順変動を回避
- KV cache hit 率を Stage 1 baseline と比較

### 4.4 Stage 4: API 極小化哲学 (本 plan 同梱)

**docs 追記** (`memory/ds4_alignment.md` 新規 ~150 行):
- ds4.h 173 行で全 API 公開の事実を記録
- bonsai `MemoryStore::conn()` 160 callsite (handoff 05-08c) を **是正対象** として明記
- 項目 209 EventRepository trait 化を **第一歩** と位置付け、SkillRepository / HeuristicRepository / VaultRepository の trait 化を **future work** として roadmap 化

### 4.5 SQLite / TSV / config への影響 (Stage 1 のみ)
- **SQLite**: 変更なし (KV cache は llama-server 側 file system、bonsai DB と独立)
- **TSV**: 変更なし (新規 metric 追加せず、既存 `duration_secs` で観測)
- **Config**: `~/.config/bonsai-agent/config.toml` に `[llama_server]` section 追加 (default 全 false で後方互換)

## 5. TDD strict 5 phase (Stage 1 主体)

### Phase 1 — Red
新規 test 6 件 (`src/config.rs` / `src/runtime/llama_server.rs`):
1. `test_llama_server_settings_default_disabled` — `LlamaServerSettings::default().kv_cache_enabled == false`
2. `test_kv_cache_dir_default_path` — `kv_cache_dir = None` の時 `~/.cache/bonsai-agent/kv` を返す resolver
3. `test_env_override_kv_cache_enabled` — `BONSAI_KV_CACHE_ENABLED=1` で `is_kv_cache_enabled()` が true
4. `test_request_body_includes_slot_save_path_when_enabled` — kv_cache_enabled=true で request body に `slot_save_path` field 含む
5. `test_request_body_omits_slot_save_path_when_disabled` — default で field 不含 (後方互換)
6. `test_kv_cache_dir_creates_on_first_use` — 初回 request で directory 自動作成

期待: compile error or 全 fail で Red 確認。

### Phase 2 — Green
1. `LlamaServerSettings` struct + serde derive → test 1 pass
2. `resolve_kv_cache_dir()` helper (`dirs::cache_dir()` 経由) → test 2 pass
3. `is_kv_cache_enabled()` env reader (Cerememory 三本柱と同 pattern) → test 3 pass
4. `LlamaServerBackend::generate()` 内 request body 構築で field 条件追加 → test 4, 5 pass
5. `std::fs::create_dir_all` で初回作成 → test 6 pass

期待: 既存 1150 + 新規 6 = **1156 passed** / clippy 0 / fmt 0

### Phase 3 — Refactor
- `LlamaServerSettings` の Builder パターン化検討 → field 2 個のみのため YAGNI、structured init 維持
- `is_kv_cache_enabled()` を crate 内 env module に集約 (Cerememory 三本柱と同 module)
- docstring 整備 (項目 223 参照、ds4 由来明記)

### Phase 4 — Smoke 検証 (3 段)
```bash
# G-4a: 既存経路 (env 未設定、後方互換)
./target/release/bonsai --lab --lab-experiments 0 --core
# 期待: 1156 pass 維持、duration 既存 baseline と同等 (±5%)

# G-4b: KV cache 有効化 + ユーザー側 llama-server 再起動 (--cache-prompt 付き)
BONSAI_KV_CACHE_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 0 --core
# 期待: 同 prompt 2 回目以降の prefill latency -50% 以上 (smoke 同 task k=3)

# G-4c: 完全 paired smoke (1 cycle)
# OFF run
./target/release/bonsai --lab --lab-experiments 1 --core 2>&1 | tee /tmp/kv_off.log
# ON run (llama-server 再起動 + flag 切替)
BONSAI_KV_CACHE_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 1 --core 2>&1 | tee /tmp/kv_on.log
```

判定基準:
- ✅ G-4a: 既存経路 1156 passed 維持、duration ±5%
- ✅ G-4b: 同 prompt 2 回目 prefill latency -50% 以上 (slot reuse 確証)
- ✅ G-4c: ON run の `duration_secs` が OFF run 比 **-10% 以上**、score ±0.02 以内 (Cerememory 三本柱と同 ACCEPT 基準)

### Phase 5 — Commit + handoff + CLAUDE.md 項目 223
5 commits:
1. `test(kv-cache): Phase 1 Red — LlamaServerSettings + env opt-in test`
2. `feat(kv-cache): Phase 2 Green — kv_cache_enabled config + slot_save_path request`
3. `refactor(kv-cache): Phase 3 — env reader 集約 + docstring`
4. `docs(kv-cache): docs/kv_cache_setup.md + memory/ds4_alignment.md`
5. `docs(claude.md): 項目 223 — ds4 知見 Stage 1 KV cache wiring 完遂`

## 6. API 影響
| API | 変更 | 後方互換 |
|---|---|---|
| `LlamaServerSettings` (新規 struct) | 2 field | — |
| `AgentSettings::llama_server` field | 追加 (Option<LlamaServerSettings>) | ✅ serde default + skip_if_none |
| `LlamaServerBackend::generate()` | 内部 request body 拡張 | ✅ env 未設定で従来挙動 |
| env `BONSAI_KV_CACHE_ENABLED` | 新規 | ✅ default 未設定で既存挙動 |
| SQLite | 変更なし | — |
| TSV | 変更なし | — |
| llama-server 側起動 flag | docs 追加 (`--cache-prompt` etc.) | ✅ user 操作、bonsai 強制せず |

**signature 変更ゼロ** — 全 additive、項目 205 のような必須化はなし。Cerememory 三本柱と同 pattern。

## 7. Risks / Mitigations
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| R1 | llama-server バージョン依存 (`--cache-prompt` / `--slot-save-path` は v1.0+) | KV cache 起動失敗 | (i) `docs/kv_cache_setup.md` に最低 version 明記 (ii) HTTP request body に flag 含めても古い server は無視 = 後方互換 |
| R2 | Disk 容量爆発 (KV cache 1 prompt 数 MB、Lab 1000+ session) | M2 disk 圧迫 | (i) `kv_cache_dir` 容量上限 (`max_kv_cache_size_gb=10`) 別 config (ii) Phase 4 smoke で disk usage 計測 (iii) cron で 7 日以上の cache 自動削除 (Stage 1+1 別 plan) |
| R3 | `--slot-prompt-similarity 0.5` で意図しない cache hit (異 task の prompt 共通 prefix で誤再利用) | score 低下 | (i) baseline は similarity 1.0 (完全一致のみ)、similarity 0.5 は派生実験で検証 (ii) score ±0.02 以内 ACCEPT 基準で検出 |
| R4 | KV cache 採用で score variance が増減し項目 200 RDC/VAF に影響 | stability 軸の意味変化 | (i) Phase 4 G-4c で stability 軸も同時計測 (ii) RDC/VAF Δ も ACCEPT 報告に含める |
| R5 | bonsai は llama-server を spawn しない設計のため kv_cache_dir の 「最初に作成」test が user 起動 server と同 path 共有できない | test 不安定 | (i) test は bonsai 側 path resolver のみ検証 (resolve → ensure_dir、HTTP は別) (ii) integration test (`#[ignore]`) で実 server 経路確認 |
| R6 | ds4 自体が 4 日前作成、極めて新しい (alpha quality 作者明記) | ds4 設計が変わるリスク | (i) bonsai は ds4 *発想* の port、ds4 code 直接依存しない (ii) llama-server `--cache-prompt` は実装済の機能、ds4 リリース安定待ち不要 |
| R7 | rax port (Stage 2) の SQLite FTS5 比較で効果ゼロの可能性 | Stage 2 dead-code | (i) Stage 2 は派生 plan、本 plan ACCEPT 条件外 (ii) Lab paired smoke で REJECT 時は項目 222 と同 pattern で wiring 削除 |
| R8 | Stage 3 (tool_id replay) の `original_emitted_text` 保存で DB size 増 | event_store 肥大 | (i) Stage 3 は派生 plan、Stage 1 ACCEPT 後に起票 (ii) BLOB 圧縮 (zstd) 採否を派生 plan で評価 |
| R9 | KV cache hit でも 1bit Bonsai-8B の variance で duration -10% 達成困難 | Stage 1 REJECT | (i) Phase 4 G-4c は -10% を ACCEPT 基準、変動大なら -5% に再設定 (ii) smoke 5 cycle で平均化 |

## 8. Quality Gates
- **G-1 Phase 1 Red**: 6 新規 test compile error or 全 fail
- **G-2 Phase 2 Green**: 6 新規 test PASS + 1150 維持 = 1156 passed + clippy 0 + fmt 0
- **G-3 Phase 3 Refactor**: docstring 完備 + helper 集約 + 既存 test 退行ゼロ
- **G-4 Phase 4 Smoke 3 段**:
  - G-4a: 既存経路 1156 pass 維持、duration ±5%
  - G-4b: 同 prompt 2 回目 prefill latency -50% 以上
  - G-4c: ON vs OFF paired smoke で duration -10% 以上 + score ±0.02 以内
- **G-5 Final**: handoff 起票 + CLAUDE.md 項目 223 + `memory/ds4_alignment.md` + Stage 2/3 派生 plan の起票 trigger 明記

## 9. 完了条件 (Stage 1 のみ)
1. ✅ `LlamaServerSettings` struct + 2 field 追加
2. ✅ `BONSAI_KV_CACHE_ENABLED=1` env reader 実装
3. ✅ `LlamaServerBackend::generate()` request body 拡張
4. ✅ `docs/kv_cache_setup.md` 新規 (llama-server 起動オプション ドキュメント)
5. ✅ `memory/ds4_alignment.md` 新規 (Stage 4 同梱)
6. ✅ smoke G-4a/b/c 全 PASS
7. ✅ 1156+ passed 維持
8. ✅ CLAUDE.md 項目 223
9. ✅ Stage 2 派生 plan (`ds4-rax-skill-index-impl.md`) 起票 trigger 文書化
10. ✅ Stage 3 派生 plan (`ds4-tool-id-replay-impl.md`) 起票 trigger 文書化

## 10. 見積もり
| Phase | 内容 | 時間 |
|-------|------|------|
| Phase 1 | Red — 6 test 追加 | 0.5h |
| Phase 2 | Green — config + env reader + request body | 1.5h |
| Phase 3 | Refactor — env module 集約 + docstring | 0.5h |
| Phase 4 | Smoke 3 段 (うち G-4c は paired 1 cycle 実機 60-90 min × 2) | 3.0h (実機 wall 1.5h) |
| Phase 5 | Commit + handoff + CLAUDE.md 項目 + memory/ds4_alignment.md | 1.0h |
| Buffer | llama-server バージョン確認 + slot save path 検証 | 1.5h |
| **合計** | | **~8h ≈ 1 day** |

派生 plan (Stage 2/3) は別 session、合計工数 +1.5 day + 4-6h。

## 11. Quick Start
```bash
# 0. 既存 caller 全網羅
rtk grep -rn "LlamaServerBackend" src/
rtk grep -rn "kv_cache\|cache_prompt\|slot_save" src/  # 期待 0 件
rtk grep -rn "BONSAI_.*_ENABLED" src/  # Cerememory 三本柱の env pattern 確認

# 1. Phase 1 Red
$EDITOR src/config.rs            # LlamaServerSettings struct
$EDITOR src/runtime/llama_server.rs  # env reader + test 追加
rtk cargo test --lib kv_cache    # compile error or fail

# 2. Phase 2 Green
$EDITOR src/config.rs            # serde default 化
$EDITOR src/runtime/llama_server.rs  # generate() 拡張
rtk cargo test --lib  # 1156 passed

# 3. Phase 3 Refactor
$EDITOR src/env.rs (or src/runtime/llama_server.rs)  # is_kv_cache_enabled() 集約
$EDITOR docs/kv_cache_setup.md   # 新規

# 4. Phase 4 Smoke (要 user 操作: llama-server 再起動 with --cache-prompt)
rtk cargo build --release
./target/release/bonsai --lab --lab-experiments 0 --core  # G-4a (既存経路)
# user: llama-server を --cache-prompt --slot-save-path ~/.cache/bonsai-agent/kv で再起動
BONSAI_KV_CACHE_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 0 --core  # G-4b
# G-4c paired (60-90 min × 2)
./target/release/bonsai --lab --lab-experiments 1 --core 2>&1 | tee /tmp/kv_off.log
BONSAI_KV_CACHE_ENABLED=1 ./target/release/bonsai --lab --lab-experiments 1 --core 2>&1 | tee /tmp/kv_on.log
diff <(grep duration /tmp/kv_off.log) <(grep duration /tmp/kv_on.log)

# 5. Commit + handoff + CLAUDE.md 項目 223 + Stage 2/3 派生 plan 起票方針
$EDITOR /Users/keizo/.claude/projects/-Users-keizo-bonsai-agent/memory/ds4_alignment.md
$EDITOR /Users/keizo/bonsai-agent/CLAUDE.md  # 項目 223
```

## 12. 参考
- antirez/ds4 (https://github.com/antirez/ds4) — DeepSeek V4 Flash inference engine
- antirez/ds4 README KV cache section (本 session fetch 済 `/tmp/ds4_readme.md`)
- antirez/rax (Redis Adaptive Radix tree、ds4 内同梱)
- llama.cpp llama-server `--cache-prompt` / `--slot-save-path` / `--slot-prompt-similarity` (https://github.com/ggerganov/llama.cpp/tree/master/examples/server)
- bonsai 既存 plan: `cerememory-decay-port-impl.md` (Plan A、外部 OSS port pattern 確立)
- bonsai 既存 plan: `cerememory-review-state-v12-impl.md` (Plan B、env opt-in pattern)
- bonsai CLAUDE.md 項目 217-219 (Cerememory 三本柱、本 plan の port pattern 手本)
- bonsai CLAUDE.md 項目 220-222 (sqlite-vec wiring 採否経緯、Lab paired smoke の前例)
- 派生 plan (本 plan ACCEPT 後起票):
  - `ds4-rax-skill-index-impl.md` (Stage 2、独立着手可)
  - `ds4-tool-id-replay-impl.md` (Stage 3、Stage 1 ACCEPT 後)
