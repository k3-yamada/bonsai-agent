# ADR-014: 単一プロセス=単一セッション実行モデルの明文化

## Status: Accepted (2026-09-10)

## Context

Issue #22（Phase B.3、長時間実行/複数エージェント境界の安全性強化）のスコープ確定
（`scope-planner`）にあたり、bonsai-agent の実行モデルを一次情報（`src/main.rs` /
`src/agent/agent_loop/core.rs` / `src/agent/subagent.rs` 等の実読取）から確認した。

起動モードとプロセス内で並存する `Session`（`domain::conversation::Session`）数の対応は
以下のとおりである。

| 起動モード | セッション数 | 根拠 |
|---|---|---|
| `--exec` | 1 | `main.rs`, `agent_loop/core.rs` |
| REPL既定 | 1（全ターン共有） | `main.rs` |
| `--resume` | 1 | `main.rs` |
| `--serve` | 0（エージェントループを起動せず read-only JSON 応答のみ） | `src/server.rs` |
| `--mcp-server` | 0（同上） | `src/mcp_server.rs` |
| `--lab`/benchmark | 1（時間的に多数だが同時実行は常に1） | `src/agent/benchmark.rs` |
| サブエージェント並列 | N（`std::thread::scope` で各スレッドが独立 `Session`） | `src/agent/subagent.rs::SubAgentExecutor::execute_parallel` |

プロセス内で複数 `Session` が同時並存するのは `SubAgentExecutor::execute_parallel`
（`src/agent/subagent.rs`）**ただ1箇所**である。デーモンモード
（`AgentConfig.is_daemon` / `DaemonPolicy`）は型としては存在するが、`main.rs` に
それを `true` にする起動経路は現時点で存在しない。

この事実を確認したことで、Issue #22 の受入条件（セッション境界の分離、キャンセル
伝播、長時間リソース管理）は「マルチセッション実行環境における分離」ではなく、
「単一セッションを前提とした足場（harness）の安全性強化」として扱うべきだと判明した。
実行モデル自体を文書化せずに機能追加を続けると、将来「セッション横断の共有状態」を
誤って導入するリスク（例: A-MEM をセッション別スコープ化しようとする、キャッシュを
プロセス全体で永続化しようとする、等）があるため、本 ADR で前提を固定する。

## Decision

**bonsai-agent は「1プロセス = 1ユーザーセッション」を実行モデルの原則とする。**

1. **通常実行（`--exec`/REPL/`--resume`/`--lab`）は常に単一 `Session`。**
   プロセスの生存期間中、`Session` インスタンスは（REPL の会話継続を除き）1つのみ
   生成される。`--serve`/`--mcp-server` はエージェントループそのものを起動せず、
   read-only な問い合わせ API のみを提供するため「0セッション」として扱う。

2. **サブエージェント並列実行は唯一の例外であり、意図的な設計。**
   `SubAgentExecutor::execute_parallel` は `std::thread::scope` で複数スレッドを
   起動し、各スレッドが `MemoryStore::open()` で独立 Connection を確保した上で
   `run_agent_loop` を呼び、その中で新規 `Session`（`new_session_with_system` 経由）
   を生成する。これにより一時的に N 個の `Session` がプロセス内に並存するが、
   各 `Session` のライフサイクルはスレッドのライフサイクルと一致し、親 `Session`
   とは構造的に独立している（`Session.messages` の共有経路は存在しない）。
   このプロパティは `tests/session_isolation.rs` で回帰テストとして固定した
   （AC-4-1〜AC-4-3）。

3. **`ToolResultCache` / Working Memory (`Session.messages`) はセッションスコープ。**
   いずれもインスタンスとして生成され、`Session`/実行呼び出しに束縛される。
   グローバル状態やプロセス全体で共有されるキャッシュではないため、セッション間の
   汚染は構造的に発生しない（既存実装が既にこの性質を持っており、本 ADR はこれを
   固定するのみで新規実装は不要だった）。

4. **A-MEM（`memories`）/ KnowledgeGraph（`knowledge_nodes`/`knowledge_edges`）/
   KnowledgeVault は意図的にセッション横断（グローバル）である。**
   これらのテーブルには `session_id` 列が存在しない。`messages`/`events`/
   `audit_log`/`checkpoints` のみが `session_id` を保持する。長期記憶・知識グラフ・
   Vault をセッション別スコープに分離することは、bonsai-agent の中核機能（過去の
   経験からの学習、DMN 自発思考、Reflexion）を破壊するため非ゴールとする。

5. **安全コンテキスト（`confirm_callback`/`autonomy`/`is_daemon`/`daemon_policy`/
   `task_timeout`）はサブエージェント境界を越えて明示的に継承する。**
   `SubAgentConfig::from_parent()` が親 `AgentConfig` からこれらを複製し、
   `build_sub_config()` が子 `AgentConfig` へ伝播する（Issue #22 B3-3、
   `src/agent/subagent.rs`）。単調性（子の権限は親を超えない）は
   `from_parent()` のみを唯一の生成経路とし、より広い権限を設定するビルダーを
   追加しないことで型レベルに近い形で担保する。

6. **並列サブエージェントで `confirm_callback` が設定されている場合は並列化しない。**
   確認プロンプトは対話的（stdin）であり、複数スレッドから同時に呼ばれると
   どの確認がどの呼び出しへの応答か区別できなくなる。`should_parallelize()`
   （純関数、`src/agent/subagent.rs`）が `has_confirm_callback == true` の場合に
   必ず `false` を返すことで、この競合を構造的に排除する
   （`tests/session_isolation.rs::t_confirm_callback_forces_sequential_subagents`
   で固定）。

7. **マルチセッション実行モデル（デーモン/サーバモード/マルチテナント）の新設は
   非ゴール。** ユースケースが存在しない YAGNI 判断であり、`is_daemon`/
   `DaemonPolicy` という型は将来の拡張点として残すが、それを有効化する起動経路
   は本 Issue の対象外とする。

## Consequences

### Positive
- 実行モデルの前提が一次文書として固定され、将来「セッション横断の共有状態」の
  誤った導入を設計レビュー段階で検出できる（本 ADR を根拠として指摘できる）。
- サブエージェント境界での安全コンテキスト継承により、`SubAgentRole::Verifier`
  （`shell` 実行が本質）が親の autonomy/confirm_callback を正しく引き継ぎ、
  設計意図どおりに機能する（従来は常に `confirm_callback=None` かつ
  `autonomy=Supervised` に戻り、`Permission::Confirm` なツールが無条件拒否
  されていた）。
- `tests/session_isolation.rs` により、セッション分離とサブエージェント安全
  コンテキスト継承の両方が回帰テストとして固定され、将来の変更で構造が
  壊れた場合に検出可能になる。

### Negative
- 親が `AutonomyLevel::Full` を設定している場合、これまで拒否されていたサブ
  エージェントの `shell` 実行が実際に実行されるようになる（安全側→通常側への
  挙動変化）。この変化は Issue #22 スコープ確定時にオーナー承認済みであり、
  単調性（子は親を超えない）が維持されている限り許容する。
- `SubAgentExecutor` には本 Issue 時点で production 呼び出し元が存在しない
  （`grep` 確認済み、参照は `agent_loop/tests.rs` のみ）。そのため本 ADR が
  記述する保証は、現時点では API 契約およびテストによってのみ検証されており、
  実運用での実証（dogfooding）は未達である。

## Related

- Issue #22 [Phase B.3] スコープ確定書（scope-planner）: 実行モデルの一次情報調査
- `src/agent/subagent.rs`: `SubAgentConfig::from_parent` / `build_sub_config` /
  `should_parallelize`
- `tests/session_isolation.rs`: AC-4-1〜AC-4-4 の回帰テスト
- [ADR-003](ADR-003-paired-evidence-over-unpaired.md): 本 Issue のスコープ外の
  変更（性能に影響する変更）を入れない規律との整合
- Issue #33: MCP 子プロセスの process group 化（B3-5、本 Issue のスコープ外へ
  切り出し済み）
