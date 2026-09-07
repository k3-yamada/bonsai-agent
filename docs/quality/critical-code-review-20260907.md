# 批判的コードレビュー統合報告（2026-09-07）

**判定: Block（全体）** — CRITICAL 複数残。本番 default-ON の新機能採用・「安全です」宣言は不可。

スコープ計画（8 パーティション）に従い、実行面セキュリティ / ループ / MAGI・DMN / runtime / memory / 層境界 / Lab / CLI・文書を静的に批判レビューした。実装変更は行っていない。

---

## 0. スコープ分割と優先順位

| # | パーティション | リスク | 判定 |
|---|---|---|---|
| P1 | 実行面セキュリティ（tools / safety / server） | High | **Block** |
| P2 | ループと scaffolding（agent_loop / recovery） | High | **Block** |
| P3 | 自律拡張（MAGI / DMN / sensors / FastPath） | High | **Block** |
| P4 | 推論ランタイム | High | **Block** |
| P5 | 記憶・知識・永続化 | Med–High | **Block** |
| P6 | 層境界とドメイン純度 | Med | **Block** |
| P7 | Lab / 評価 / 指標 | Med | **Warning**（default-ON は Block 相当） |
| P8 | CLI・観測・文書忠実度 | Low–Med | **Block** |

**最初に叩くべき top 3:** P1 → P2+P3 → P5（書き込み面）+ P4。

---

## 1. 横断結論（1 ページ）

bonsai-agent のハーネス骨格（ループ分割、compaction、MAGI 挿入位置、default OFF の Lab 変異）は「Scaffolding > Model」方向として正しい。一方で **文書と型が約束する安全機構の多くが実行経路に乗っていない**。Permission / NetworkFilter / Autonomy / BootGuard / Hooks / `--serve` / `--mcp-server` は定義＋テスト（または死コード）止まり。破壊的操作の実防御は薄い正規表現と PathGuard の文字列検査に縮退している。

加えて:

1. **MAGI Halt 契約が三分裂**（パネル全員 Block / step は 1 Block / 3 Judge 本番では全員 Halt 到達不能）。
2. **DEP-001 は `use crate::` のみ** → `typed.rs`（tools→agent）と `evolution.rs`（memory→runtime）が緑のまま。
3. **Ctrl+C が SSE 失敗扱い → 非ストリーム再推論**（キャンセルが効かない）。
4. **ADR-005「sqlite-vec 撤去」対 default `embeddings` 生存**、Lab ACCEPT はまだ unpaired `delta > 0`。

「テストが通る」「型がある」「README に書いてある」を信頼してはいけない。

---

## 2. CRITICAL 集約（即対応）

### C1. Permission / NetworkFilter / Autonomy が本番未配線

- **証拠:** `check_permission(` の呼び出しは `permission.rs` の単体テストのみ。`NetworkFilter` / `AutonomyLevel` も自ファイル＋テスト以外ゼロ。`tool_exec` は即 `tool.call()`。
- **攻撃:** Confirm 宣言の shell / MCP / file_write が確認なし実行。Deny プラグインも `call()` すれば動く。
- **対応策:**
  1. `execute_single_call` 直前で `check_permission` を必須化（Allow 以外は `call` しない）。
  2. `NetworkFilter` を `web_fetch` / MCP HTTP のリクエスト前に差し、default を deny/allowlist。
  3. `AutonomyLevel` を `AgentConfig` に載せ、ReadOnly で write 系を拒否。
  4. **実行経路テスト**で Confirm 未確認のまま `call` されないことを固定。

### C2. `web_fetch` SSRF（Auto + フィルタ無し + MAGI 対象外）

- **証拠:** `WebFetchTool` は `Permission::Auto`、`reqwest::blocking::get(url)`、timeout/プライベート IP/リダイレクト検査なし。`is_read_only` のため MAGI スキップ。
- **攻撃:** `http://127.0.0.1:8080/...`、クラウドメタデータ、公開 URL→302 でループバック。
- **対応策:** URL パース＋スキーム制限＋解決後のプライベート拒否、`redirect::Policy::none()` または Location 再検査、timeout、Confirm または権限ゲート対象化。

### C3. PathGuard は `Tool::call` では効かない

- **証拠:** `FileReadTool` は `read_to_string(path)` のみ。validate 側は値が `/` または `~/` で始まる文字列だけ。`shell` の `cat ~/.ssh/...` はスルー。`canonicalize` ゼロ件。
- **対応策:** deny を各 I/O ツールの open 直前へ。symlink 解決後照合。shell は path 抽出に頼らず Confirm/サンドボックスで縛る。deny に `.env` / credentials 等を追加。

### C4. SafetyJudge 不在 + 危険コマンドが Warn のみ

- **証拠:** ADR-012 の MELCHIOR/SafetyJudge は型もファイルも無い。`validate` の `rm -rf` 等は Warn（`is_valid==true` 固定テストあり）。ValuesJudge は `rm\s+-rf\s+[/~]` 程度。`rm -rf .` / `rm -fr /` は通過しうる。
- **対応策:** 危険パターンを Block。`check_permission` と二重化。正規表現劇場を Safety の本命にしない。`/magi` の「MELCHIOR 稼働中」を削除。

### C5. MAGI Halt 契約の分裂

- **証拠:** `panel.decide` は全員 Block で Halt。`step.rs` 最終回答は 1 Block で `MagiHalt`。Goodhart は空 checker・env OFF で常 Clear → 3 体全員 Halt は実質到達不能。
- **対応策:** 「安全系 Block は 1 票 Halt」に契約統一。`DecisionOutcome` を無視して再解釈しない。Goodhart を共有寿命 checker にするか外す。

### C6. キャンセルが generate 中に効かない（再推論する）

- **証拠:** SSE cancel Err → 「パース失敗」扱いで `generate_non_streaming`（cancel 非対応）。SIGINT ハンドラはフラグのみで 2 回目もプロセスが死なない。
- **対応策:** cancel エラーでは non-stream / Fallback 禁止。non-stream に cancel 伝播。SIGINT 2 回目で exit。

### C7. EvolutionEngine が成功パスでゲートなし自己更新

- **証拠:** `record_success` → `auto_collect` / `apply_improvements`。arXiv HTTP（TLS なし）を毎回吸い、検証なしで memories へ。
- **対応策:** 成功パスから外し、CLI/Lab 明示オプトインのみ。HTTPS・重複 UNIQUE・品質ゲート。

### C8. ADR-005 対 sqlite-vec 生存（読み経路 production、書き経路撤去）

- **証拠:** default feature `embeddings` + vec0 auto_extension + `vector_search` production path。`index_memory_if_enabled` のみ削除 → 意味検索は空/陳腐化。毎タスク `create_embedder()` で REJECT 理由の RSS コスト再発火しうる。
- **対応策:** ADR を改訂するか default から外す。使うなら write も同一トランザクション。使わないなら KNN path と auto_extension 削除。

---

## 3. HIGH 集約（パーティション横断）

| ID | 内容 | 主な場所 | 対応策（要約） |
|---|---|---|---|
| H1 | sandbox は ulimit 主張だが素の `sh -c`、二重 sh | `tools/sandbox.rs`, `shell.rs` | 1 段のみ、setrlimit/killpg、コメント一致 |
| H2 | plugin `{param}` 無エスケープ + permission 未強制 | `tools/plugin.rs` | shell_escape / argv 化、Confirm 必須 |
| H3 | MCP 任意プロセス / HTTP 無検証 | `mcp_client.rs` | allowlist、npx -y 禁止、Confirm |
| H4 | 秘密: エラー/args/cache が redact バイパス | `tool_exec.rs` | session/audit/cache/log 単一ゲート |
| H5 | `server.rs` CORS `*`・無認証（CLI 未配線） | `server.rs`, `main.rs` | 配線するなら認証。しないなら削除 |
| H6 | LoopDetected が `attempt >= max_retries` で無視 | `error_recovery.rs`, `step.rs` | LoopDetected 即 Abort |
| H7 | ContinueSite / FileStuckGuard / TrialSummary 未配線 | `error_recovery.rs` | 失敗経路に接続 |
| H8 | Stall は abort せず advisor 枯渇で reset | `advisor_inject.rs` | 枯渇後 Aborted |
| H9 | `ToolResult{success:false}` が成功扱い | `tool_exec.rs` | CB/stall/trial に失敗算入 |
| H10 | unclosed `</think>` が tool_call を飲み込む | `parse.rs` | 回復パース or ParseError |
| H11 | RecoveryAction 大半が死んでいる | `step.rs` | RetryWithFix を user/system 注入 |
| H12 | subagent `allowed_tools` がレジストリに効かない | `subagent.rs` | deny-by-default フィルタ |
| H13 | MultiFileEditCycleDetector が `file_path` を見るが実体は `path` | `tool_exec.rs` | 両方見る |
| H14 | REJECT 済み dynamic budget が env=1 で本番 prune | `compaction.rs`, scripts | factory から外すか Lab-only |
| H15 | DMN が Vault/KG/A-MEM に無確認書き込み | `dmn/**` | inbox + Confirm、persist キルスイッチ |
| H16 | ValuesJudge が本番 `user_input=""` で V5 テストと不一致 | `values.rs`, `step.rs` | `with_user_input`、拒否文と実行文を区別 |
| H17 | Fallback が cancel を失敗として切替 | `inference.rs` | Cancelled 分類、record_failure しない |
| H18 | SSE「チャンク間」timeout はヘッダ完了からの全体切れ | `http_agent.rs` | ureq 先行 RecvResponse に合わせて再設計 |
| H19 | Fallback チェーンが API キーを落とす / body に model 無し | `main.rs`, `llama_server.rs` | 全エントリに key/model |
| H20 | `claude` CLI に timeout/cancel 無し | `model_router.rs` | wait_timeout + cancel |
| H21 | ProcessSupervisor: 死んだ child を respawn しない / 推論中 idle kill | `process_supervisor.rs` | try_wait、in-flight 禁止 |
| H22 | DEP-001 が FQ `crate::上層` を見逃す | `tests/structural.rs` | 全文スキャン + fixture |
| H23 | tools→agent 循環（typed.rs） | `tools/typed.rs` | coerce を domain/tools へ |
| H24 | LAYER_ORDER が eval/crate root をスキップ | `structural.rs` | メタテストで所属強制 |
| H25 | Vault append 重複が 50 文字部分一致 | `vault.rs` | 正規化ハッシュ、部分一致禁止 |
| H26 | Lab ACCEPT が unpaired `delta > 0` | `experiment_log.rs` | paired 分離、accepted 封印 |
| H27 | paired scripts が死 env / 仮説不一致 | `scripts/g_paired_*` | SSOT 固定、削除済み env 除去 |
| H28 | runbook が unpaired ACCEPT を推奨 | `docs/execution/runbook.md` | REJECT / default OFF に更新 |
| H29 | AgentHER が Lab で常時本番 DB 学習 | Lab 経路 | cycle ごと一時 DB、opt-in |
| H30 | `--serve` / `--mcp-server` 死フラグ、README は生きていると書く | `main.rs`, README | 配線か削除。無視→REPL は危険 |
| H31 | hooks 未配線 | `hooks.rs` | 配線か文書から削除 |
| H32 | `/magi` が存在しない audit 列を見て常に 0 | `repl.rs`, `cli_diagnose.rs` | events の magi_halt/warn を見る |

---

## 4. MEDIUM / LOW（抜粋）

- working memory cap ON 時の User 消失・ペア破壊リスク（default OFF は正しい）。
- AI+Tool 保護が隣接 1 ペアのみ / level3 非保護。
- `max_tool_output_chars` がループに未接続（リテラル 4000）。
- FastPath version 直書き、クールダウン時 LLM フォールスルー。
- Idle/Window センサーは実質スタブ、PrivacyFilter 部分一致の過不足。
- SIZE-001 whitelist が行数 ratchet 無し（benchmark 4500 行等）。
- docs が sandbox を safety と書くが実体は tools。
- SKILL.md は llama-cpp-2 / async の別設計図。
- LOG-001 は `eprintln!` のみ、`eprint!` 非検知、ファイル単位免除。
- Goodhart `record_snapshot` 本番ゼロ → V7 形骸。

---

## 5. 文書ドリフト表（必須）

| 文書主張 | 実体 | リスク |
|---|---|---|
| Confirm = ユーザー確認 | `check_permission` 未配線 | **HIGH** |
| graduated autonomy / NetworkFilter | 定義＋テストのみ | **HIGH** |
| sandbox に ulimit | 素の `sh -c` | **HIGH** |
| `--serve` / `--mcp-server` で起動 | main 未分岐 → REPL に落下 | **HIGH** |
| pre/post hooks が動く | HookRunner はテストのみ | **HIGH** |
| ADR-005: sqlite-vec 撤去済 | default embeddings + vec0 | **HIGH** |
| ADR-012: SafetyJudge (MELCHIOR) | 型なし。Values が兼務 | **HIGH** |
| MAGI 全員一致で Halt | step は 1 Block。全員 Halt 到達不能 | **HIGH** |
| overview: ミドルウェア 5 段 | 実装 4 段（Stall 除外） | LOW |
| Dynamic Budget を原則扱い | 項目 268 REJECT / default OFF | MED |
| runbook: T6 +14.4% ACCEPT | 項目 269 paired REJECT | **HIGH** |
| SKILL.md: インプロセス FFI / async | HTTP 同期アーキ | MED |
| ADR-012 SIZE 100% / 1581 PASS | whitelist 20、本環境未再現 | MED |

---

## 6. パーティション別サマリ

### P1 実行面セキュリティ — Block
権限・ネットワーク・自律度が未配線。SSRF・PathGuard 迂回・sandbox 名目・plugin/MCP・秘密 redact 穴・死んだ API サーバ。対応は C1–C3, H1–H5。

### P2 ループと scaffolding — Block
検出器はあるが abort 経路が複数死。LoopDetected 無視、ContinueSite 未配線、Stall reset、false success、parse/recovery 穴、subagent フィルタ無効、REJECT budget 再点灯。対応は C4 一部, H6–H14。

### P3 自律拡張 — Block
ADR-012 と実装の乖離が最大。SafetyJudge 欠落、Halt 分裂、正規表現劇場、DMN 無確認永続化、センサー形骸、診断の虚偽ゼロ。対応は C4–C5, H15–H16, H32。

### P4 推論ランタイム — Block
キャンセル再推論、timeout 偽契約、Fallback×cancel、API key/model 欠落、claude 無限待ち、supervisor respawn/kill。tokio 侵食は無し（良い）。対応は C6, H17–H21。

### P5 記憶・知識 — Block
sqlite-vec 半生半死、Evolution 暴走、DEP-001 すり抜け、factcheck ON でも壊/汚染、Vault 部分一致、`!Sync` 設計未完成、V3 ログのみ / V7 Clear。対応は C7–C8, H22, H25。

### P6 層境界 — Block
ゲートが嘘をつく（FQ 見逃し、eval/root スキップ）。typed 循環、domain の FastEmbedder、SIZE ratchet 無し。対応は H22–H24。

### P7 Lab / 指標 — Warning（採用は Block）
REJECT 機能の default OFF は守れている。Lab ACCEPT=`delta>0`、死 env runner、runbook の stale ACCEPT、AgentHER の学習漏洩が危険。対応は H26–H29。

### P8 CLI・文書 — Block
死フラグ＋生きた README、hooks/Permission 空振り、ADR 嘘、SecurityEvent 未 emit。対応は H30–H32 + ドリフト表。

---

## 7. 良い点（残すべき骨格）

- 同期アーキ（`src/` に tokio 無し）と `CancellationToken` 設計方向。
- ループのモジュール分割（config/state/step/outcome/middleware）。
- ContextOverflowGuard の実 Abort、compaction の境界ペア保護の一部。
- Lab 変異の production default OFF（T6 aug / dynamic budget / MEMORY_AUG 削除）と paired evidence 規律の**意図**。
- FastPath の全文アンカー、Window 永続化が app 名のみ、Accessibility 未許可でスキップ。
- Event 純粋ロジックの domain 化、ingest のタグ完全一致、LOG-001 path exact match。
- `web_search` の固定 URL + shared_agent（対して `web_fetch` が悪い）。

骨格を理由に Approve できない。中身の契約が空または矛盾している。

---

## 8. 推奨ロードマップ（対応策の順序）

### Phase A — 実行を止められるようにする（P1+C4）
1. `check_permission` を tool_exec に配線 + 実行経路テスト。
2. 危険コマンドを Block。PathGuard を I/O ツール内へ。
3. `web_fetch` SSRF 対策。sandbox 主張を実装か文書のどちらかに合わせる。
4. `--serve`/`--mcp-server`/hooks を配線または削除。README 同期。

### Phase B — 「止まったつもり」を直す（P2+P3）
1. Halt 契約を panel と step で統一。`/magi` 虚偽表示を止める。
2. LoopDetected 即 Abort、RecoveryAction 実装、ContinueSite 配線。
3. DMN 永続化を Confirm/opt-in。SafetyJudge は正規表現増殖ではなく実行意図ゲート。

### Phase C — ランタイムと記憶の誠実さ（P4+P5）
1. cancel → 再推論禁止、SSE timeout 契約修正、supervisor try_wait。
2. Evolution を成功パスから除去。sqlite-vec を ADR かコードのどちらかに合わせる。
3. Vault 重複をハッシュ化。SecretsFilter を全経路単一ゲート。

### Phase D — ゲートと Lab 規律（P6+P7+P8）
1. DEP-001 を FQ 検出に拡張、typed/evolution を解消。
2. Lab ACCEPT を paired 分離。死 scripts/runbook 掃除。
3. ADR/overview/SKILL をコードに追従（またはコードを ADR に）。

---

## 9. 受入条件（再レビューで Approve する最低条件）

1. C1–C8 が閉じている（または意図的縮小として ADR 改訂＋ユーザー向け文書から「安全」表現を削除）。
2. `check_permission` / Halt / cancel について **実行経路テスト** がある。
3. DEP-001 が FQ 上向きを落とす。typed/evolution 解消。
4. 文書ドリフト表の HIGH 行が解消。
5. Lab 由来の default ON を出さない（ADR-003）。

---

## 10. レビュー限界

- 本統合は静的追跡＋既存テストの主張に基づく。一部サブレビューで `cargo test --lib`（runtime）は通過確認あり。全体の `cargo test --lib` / structural / 実機 SSRF・Ctrl+C・Lab smoke は未実行または環境制約あり。
- Skill ファイル `code-review-expert` 等は本環境に無く、リポジトリ規則・ADR・VALUES・セキュリティ観点で代替適用。
- Lab 実機・Mac M2 RSS は未測。

---

## 付録: パーティション委譲結果の所在

各サブエージェントの詳細 findings は本ファイルに統合済み。個別の長文証跡はレビューセッション内の P1–P8 出力を一次ソースとする。
