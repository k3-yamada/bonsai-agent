# 「ツール成功率 0%」根本原因調査レポート (2026-05-01c)

> Step 1 of `.claude/plan/recommended-todo-execution-2026-05-01c.md`

## 結論

**根本原因**: `src/agent/agent_loop/support.rs::check_invariants` (line 23-51) の「ツール成功率」判定ロジックが、ツールメッセージ content 内に **日本語の「エラー」または「失敗」が部分一致するか** だけで失敗扱いする偽陽性ヘビーなヒューリスティックになっていた。

**当初仮説 (parser yield 起源) は REJECT**: parse.rs / validate.rs / tool_exec.rs ではなく、support.rs の invariant チェック関数が単独で起こしていた症状。

**スコア影響なし**: invariant violation は `log_event(LogLevel::Warn, ...)` でログ出力されるだけの非ブロッキング警告 (outcome.rs:56「不変条件チェック（非ブロッキング警告）」)。Lab スコアやベンチマーク評価に直接寄与しない。当初 plan の「baseline +10% 期待」は誤算定。

**真の影響**: ログノイズ + 認知負荷。Lab 自己改善ループが自身のログを読む経路がある場合、虚偽の「失敗」シグナルが mutation 評価へ流入する潜在リスクあり（現状は未確認）。

## 症状の証拠

`/tmp/bonsai-llama/iter0-aede413.log` (γ Iter0, aede413) と `p3a-baseline.log` (P3-α HEAD) 双方で:

```
[WARN][invariant] ツール成功率が低い: 0%
[WARN][invariant] ツール成功率が低い: 0%
... (連発)
```

handoff `session_2026_05_01b_handoff.md` 副次知見 5 で「ツール成功率 0%警告は構造課題、v10 期から存在」と既に記録あり。

## 根本原因の詳細

### 該当コード（修正前）

```rust
// src/agent/agent_loop/support.rs:23-39
pub(super) fn check_invariants(session: &Session, task_context: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let tool_msgs: Vec<_> = session
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .collect();
    if !tool_msgs.is_empty() {
        let success_count = tool_msgs
            .iter()
            .filter(|m| !m.content.contains("エラー") && !m.content.contains("失敗"))
            .count();
        let rate = success_count as f64 / tool_msgs.len() as f64;
        if rate < 0.5 {
            violations.push(format!("ツール成功率が低い: {:.0}%", rate * 100.0));
        }
    }
    ...
}
```

### 偽陽性が発生する理由

1. **実際のツールエラー format は `"ツール実行エラー: {e}"`** (`src/agent/tool_exec.rs:78`) — 明確なプレフィックスを持つ
2. `check_invariants` の filter は **content 内の任意位置** に "エラー" または "失敗" の 2 文字が出現すれば失敗扱い
3. 結果として、以下の **正常な** ツール出力が全て「失敗」とカウントされる:
   - `file_read` で読んだ Rust ソース内のコメント "// エラーハンドリング..."
   - `git_diff` で取得した patch 内の `Result<T, Error>` 周辺コメント
   - `repomap` 出力内の `error_handler` 等の日本語シンボル名（稀）
   - 任意のドキュメント・README に含まれる「エラー」「失敗」言及
4. bonsai-agent のソースコード自体が error handling を多用するため、自己ベンチマーク中に file_read を使う多くのタスクで全件「失敗」判定 → 0%

### 既存テストが catch しなかった理由

`src/agent/agent_loop/tests.rs:1283` `t_check_invariants_low_success_rate` のテスト fixture:

```rust
content: "エラー: ファイルが見つかりません".to_string(),  // 実 format と異なる
```

非実在的な合成テキストで「エラー」始まりにしているため、現実の偽陽性パターンを捕捉できなかった。実際のツールは `"ツール実行エラー: ..."` プレフィックスを使う。

## 修正方針

### Approach A++ (採用): プレフィックス照合への切替

```rust
.filter(|m| {
    let trimmed = m.content.trim_start();
    !trimmed.starts_with("ツール実行エラー")
        && !trimmed.starts_with("Error:")
        && !trimmed.starts_with("[Tool error]")
})
```

- **Pros**: 5-7 行 patch、後方互換、既存テスト fixture を実 format に更新するだけで OK、新テストで偽陽性解消を demonstrate 可能
- **Cons**: 非標準なエラー prefix を使うツールがあれば検出漏れ（現状 `tool_exec.rs:78` の単一 prefix で十分）

### Approach B (defer): EventStore 連携

`EventStore::aggregate_events_for_trajectory` (event_store.rs:200-216) は `ToolCallEnd` イベントの `success: bool` フィールドを使った正確な集計を既に実装済み (項目 161/162)。`check_invariants` を `store: Option<&MemoryStore>` + `session_id` を受け取る形に変更し、EventStore から正確な rate を取得する経路。

- **Pros**: 真の成功率取得、ロジック重複解消
- **Cons**: 関数シグネチャ変更 + outcome.rs 呼出側の plumbing 更新 + LoopState から session_id 取得経路の確認が必要、変更範囲がやや広い
- **判定**: Approach A++ で偽陽性解消 → Lab 観測継続 → 必要なら次セッションで Approach B に発展

### 提案: 次の改善

将来 `Message` 構造体に `success: Option<bool>` フィールドを追加し、tool_exec から伝播させれば全関連箇所で一貫した success 判定が可能になる（compaction.rs:200/201 の "error"/"Error" 含有判定も同症状）。今回は対象外。

## 既存影響範囲（同パターンの他箇所）

- `src/agent/compaction.rs:200-202` — メッセージ重要度スコアリングで Tool message に "error"/"Error"/"エラー" 含有時に重要度を下げる。これも誤判定の可能性ありだが影響は副次的（重要度スコアのみ、ロジックゲートではない）。本セッションは support.rs のみ対象。
- `src/agent/compaction.rs:269` — ハンドオフサマリーで「エラー」含有 Tool messageを優先抽出。同症状で誤抽出するが、対外影響軽微（コンパクション要約品質のみ）。

## 修正と検証計画

1. **RED**: `tests.rs` に新テスト `t_check_invariants_no_false_positive_on_file_content_with_error_word` を追加（file_read 風の "エラー" 含有 Tool メッセージで違反検出されないことを確認）
2. **GREEN**: support.rs の filter を prefix 照合に変更
3. **既存テスト fixture 更新**: `t_check_invariants_low_success_rate` の content を `"ツール実行エラー: ファイルが見つかりません"` (実 format) に変更
4. **検証**: `cargo test --lib` 992+1 passing
5. **Commit**: `fix(invariant): ツール成功率判定の偽陽性除去 (実エラー prefix のみ照合)`

## ROI 再評価

- 当初 plan: HIGHEST (baseline +10% 期待) — **撤回**
- 実態: MED-LOW (ログノイズ解消、認知負荷削減、潜在的な mutation 評価の信号品質向上)
- 工数: 30min (修正 + 既存テスト更新 + 新テスト追加 + commit)
- 採用判定: **採用** (工数小・リスク低・恒久的な品質改善)
