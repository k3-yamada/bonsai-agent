# Tool Whitelist deny-by-default — Z-NEW-E plan (okamyuji/go-llm-agent 適用)

**起票日**: 2026-05-30
**起源**: Zenn 記事 okamyuji/go-llm-agent (https://zenn.dev/okamyuji/articles/golang-litellm-alternative-single-binary) Ch05 「ツール契約」 + bonsai memory note `zenn_go_litellm_alternative_learnings_2026_05_30.md` Z-NEW-E 案
**前提**: bonsai は tool registry (tools/mod.rs) 経由で全 tool 自動 enable、deny-by-default 強制機構なし
**関連項目**: 244 (KG lint deny-by-default 拡張可能性) / 246 (Vault lint 起源) / 251 (Vault bail pattern 整合)
**優先度**: ★★ (短期 ROI 中、Lab 実験 sanity gate として valuable)
**production code touch**: あり (config.rs + main.rs + tools/mod.rs 経由、env-gated)

---

## 1. 背景

### 1.1 記事の design

okamyuji/go-llm-agent では `config.yaml::enabled_tools` に明示列挙したツールだけが有効化される deny-by-default 構成:
- 既定 = readonly `fs_read` / `search_files` / `http_fetch` の 3 つのみ
- `fs_write` や `shell` を使いたい場合は意図的に列挙必須
- 列挙外は registry に登録されていても call 経路に hook されない

### 1.2 bonsai 現状

- `tools/mod.rs::ToolRegistry::new()` で全 tool を自動 register
- enable/disable の env / config 経由制御は部分実装 (e.g., 一部 tool は `BONSAI_KG_LINT_LAB=1` 等で hook)
- **production 経路で「全 tool active」が default**、Lab 実験中の `file_write` 誤動作で項目 243 の README.md 改変 事故が発生 (副作用解消は input rewriting で対応、構造的予防なし)

### 1.3 構造的 risk

- Lab paired 起動中に Bonsai-8B 1bit が誤って `file_write` を出力 → production source file 改変
- Vault 構築段階で `shell_exec` が活性化 → 想定外 command 実行
- 項目 264 G-T6-D-1/2 smoke で `[saved:` marker 0 = level0 (Tool offload) も発火しないため副作用なしだったが、長 cycle で偶発 fire の risk あり

## 2. ゴール

1. **`BONSAI_ENABLED_TOOLS=tool1,tool2,...` env で whitelist 強制可能化** = unset で current 挙動 (全 tool enable) 維持、set で whitelist 列挙のみ active
2. **smoke / Lab cycle で「readonly default + write を明示列挙」 pattern 確立** = `BONSAI_LAB_SMOKE=1` で自動 readonly-only mode (Z-NEW-E 案 C variant)
3. **backward compat 完全維持** = env unset で既存 1378 test 全 PASS
4. **項目 246/251 Vault lint と統合可能化** = `BONSAI_VAULT_LINT_STRICT=1` 等の deny-by-default semantics と整合

---

## 3. 案比較 (3 案 × 5 軸)

| 軸 | 案 A (env override: BONSAI_ENABLED_TOOLS) | 案 B (config.yaml `enabled_tools` field) | 案 C (smoke-only auto: BONSAI_LAB_SMOKE=1 で readonly default) |
|----|--------------------------------------------|------------------------------------------|-----------------------------------------------------------------|
| 適用 surface | env-gated 明示 | config 経由、全 user 影響 | smoke scope 限定 |
| backward compat | ★★★ (unset で 100% 既存挙動) | ★★ (config 既存ファイル更新必要) | ★★★ (smoke 外で既存挙動) |
| 工数 | ★★ (1 env getter + tool filter chain) | ★ (config.rs serde schema 追加 + parse) | ★★ (smoke check + readonly preset chain) |
| Lab 実験事故防止 | ★★ (env=1 で発動可能) | ★★ (config 経由なら常時) | ★★★ (smoke 自動発動) |
| risk | 低 (opt-in、rollback 1 行 unset) | 高 (既存 config に新規 field) | 低 (smoke scope 限定) |
| **総合** | **12/15** | 10/15 | **13/15** |

### 推奨 = **案 C + 案 A 併用** (max_context_tokens reduction plan と同 pattern)

理由:
- production 影響ゼロ (smoke 外で全 tool enable 維持)
- 項目 244/246/251 Vault lint の trajectory と整合 (Lab cycle scope での safety enforcement)
- A/A test (`lab_v22_aa_test.sh`) 等で readonly mode 自動適用 = Lab cycle 中の file_write 事故予防
- 案 A 併用で smoke 外でも `BONSAI_ENABLED_TOOLS=fs_read,memory_search` 等で個別 enable 可能、開発者 ergonomic 損なわず

### readonly default の定義

| Tier | tool 名 (suggested) | readonly | 案 C default 適用 |
|------|---------------------|----------|---------------------|
| Read | `file_read` / `repo_map` / `memory_search` / `kg_query` / `web_fetch` / `shell_read` 等 | ✓ | ✓ (smoke で default enable) |
| Write | `file_write` / `file_edit` / `shell_exec` / `memory_save` / `kg_update` 等 | × | × (smoke で disable) |

bonsai 実装後の tool list 確認後、tier 分類は project memory `tool_tier_classification_2026_05_30.md` に書出推奨。

---

## 4. ACCEPT 条件

### 4.1 unit test (Phase 1 Red + Phase 2 Green)
- (a) `is_tool_whitelist_enabled() / parse_enabled_tools_env() / readonly_tool_whitelist()` 3 env getter + const、Phase 1 Red 4 failing test → Phase 2 Green PASS
- (b) cargo test --lib 1378 → 1382+ retention 退行ゼロ
- (c) clippy / fmt clean

### 4.2 integration (Phase 3 wiring)
- (d) ToolRegistry::new() に whitelist filter chain 統合 (env unset で全 enable、set / smoke=1 で whitelist 反映)
- (e) tests/structural.rs に 1 wiring test 追加 (smoke=1 で file_write が registry に登録されない事を assert)

### 4.3 Phase 4 Smoke G-TWL1 (smoke 実機検証)
- (f) BONSAI_LAB_SMOKE=1 + file_write 含む T6 task で実機実行、`file_write` 呼出が tool not found エラーで graceful reject
- (g) AgentHER skipping 等の log 出力 + 既存 score への退行ゼロ

---

## 5. TDD strict 実装 outline (3 phase)

### Phase 1 Red (~45 min)
- `src/config.rs` (または `src/tools/whitelist.rs` 新規) に:
  - const `READONLY_TOOL_WHITELIST: &[&str] = &["file_read", "repo_map", "memory_search", "kg_query", "web_fetch"]` (要 production tool 列挙確認後 finalize)
  - `is_tool_whitelist_enabled() -> bool` (`BONSAI_ENABLED_TOOLS` set OR `BONSAI_LAB_SMOKE=1`)
  - `parse_enabled_tools_env() -> Option<Vec<String>>` (`BONSAI_ENABLED_TOOLS` comma-separated parse、validation 含む)
  - `effective_tool_whitelist() -> Option<Vec<String>>` orchestrator (env > smoke default > None)
- `src/tools/mod.rs::ToolRegistry` に `apply_whitelist(self, whitelist: &[String]) -> Self` (registered tool から除外)
- 4 failing test (`#[should_panic]` で Phase 2 Green 期待):
  1. `t_env_whitelist_filters_registry`
  2. `t_smoke_mode_applies_readonly_default`
  3. `t_env_overrides_smoke_default`
  4. `t_no_env_no_smoke_preserves_full_registry` (backward compat)

### Phase 2 Green (~1.5 h)
- env getter + apply_whitelist 本実装
- 4 test PASS、cargo test --lib 1378 → 1382 retention

### Phase 3 Refactor + wiring (~30 min)
- rustdoc 強化 (3 段優先順位 + smoke 自動 readonly contract + tool tier 一覧)
- `src/main.rs::main()` または agent 起動 path に `ToolRegistry::new().apply_whitelist(effective_tool_whitelist())` 統合
- tests/structural.rs に 1 wiring test 追加
- BONSAI_ENABLED_TOOLS 列を runbook.md env table に追記

---

## 6. test cases

```rust
#[test]
fn t_env_whitelist_filters_registry() {
    let _g = LAB_RUNTIME_ENV_TEST_LOCK.lock();
    unsafe {
        std::env::set_var("BONSAI_ENABLED_TOOLS", "file_read,memory_search");
        std::env::remove_var("BONSAI_LAB_SMOKE");
    }
    let registry = ToolRegistry::new().apply_whitelist(
        effective_tool_whitelist().as_deref().unwrap_or(&[])
    );
    assert!(registry.has("file_read"));
    assert!(registry.has("memory_search"));
    assert!(!registry.has("file_write"), "whitelist 外は登録されない");
    unsafe {
        std::env::remove_var("BONSAI_ENABLED_TOOLS");
    }
}

#[test]
fn t_smoke_mode_applies_readonly_default() {
    let _g = LAB_RUNTIME_ENV_TEST_LOCK.lock();
    unsafe {
        std::env::set_var("BONSAI_LAB_SMOKE", "1");
        std::env::remove_var("BONSAI_ENABLED_TOOLS");
    }
    let registry = ToolRegistry::new().apply_whitelist(
        effective_tool_whitelist().as_deref().unwrap_or(&[])
    );
    for readonly_tool in READONLY_TOOL_WHITELIST {
        assert!(registry.has(readonly_tool));
    }
    assert!(!registry.has("file_write"), "smoke で writeは除外");
    unsafe {
        std::env::remove_var("BONSAI_LAB_SMOKE");
    }
}

// 残 2 test は plan §5 通り、env precedence + backward compat
```

---

## 7. Phase 4 Smoke G-TWL1/2/3

### G-TWL1 (env override): unit-test 等価、~5 min
- `BONSAI_ENABLED_TOOLS=file_read,memory_search ./target/release/bonsai --capabilities`
- ACCEPT 条件 (a)(b) 確認

### G-TWL2 (smoke auto-readonly): T6 lh_* k=3 smoke、~80 min
- `BONSAI_LAB_SMOKE=1 ./scripts/g_mct2_smoke.sh`
- 期待 = file_write 含む T6 task で graceful reject (score 影響なし、AgentHER skipping log 増加可)
- ACCEPT 条件 (f)(g)

### G-TWL3 (full registry baseline): env unset、~80 min
- 既存 G-MCT2 baseline と同 score 維持 (項目 265 G-MCT2 score=0.8209) 確認
- backward compat 確証

---

## 8. Rollback strategy

- 緊急 rollback: `BONSAI_ENABLED_TOOLS` env unset + `BONSAI_LAB_SMOKE` unset で 100% 既存挙動
- code revert: `ToolRegistry::new().apply_whitelist(...)` chain 削除 1 行 revert
- env-gated 設計のため side effect ゼロ

---

## 9. dependencies + cross-references

- 起源: `zenn_go_litellm_alternative_learnings_2026_05_30.md` Z-NEW-E 案
- 連動 plan: `.claude/plan/max-context-tokens-reduction-force-prune.md` (案 C smoke-only auto と同 design pattern)、`.claude/plan/lab-v22-paired-metric-mandatory.md` (Phase 2 paired での tool whitelist 適用検討)
- 関連項目: 244 (KG lint deny-by-default 拡張余地) / 246 (Vault lint 起源 + 統合) / 251 (Vault bail pattern 整合) / 257-260 (Z-3 drift linter quality gate と同 trajectory) / 265 (env-gated factory chain pattern)
- 関連 file: `src/tools/mod.rs` ToolRegistry / `src/config.rs` env getter / `tests/structural.rs` Z-4 layer linter wiring test

---

## 10. follow-up (本 plan ACCEPT 後の次手)

1. ★★★ Phase 4 Smoke G-TWL1/2/3 実機 (要 MLX server、~80 min × 2 cycle)
2. ★★ tool tier 分類 audit (project memory `tool_tier_classification_2026_05_30.md` 起票) = production tool 全列挙 + readonly/write 分類確定
3. ★ runbook.md env table に BONSAI_ENABLED_TOOLS 列追加
4. ★ CLAUDE.md 項目 entry 追加 (deny-by-default tool whitelist 完遂後)
