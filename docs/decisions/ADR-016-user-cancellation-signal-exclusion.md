# ADR-016: ユーザー取消（Ctrl+C）はいかなる学習信号にも変換しない

## Status: Accepted (2026-09-10)

## Context

Issue #22（qa-reviewer MEDIUM-2）と Issue #34 は、ユーザーが Ctrl+C でツール実行を
中断した場合に、その中断を `circuit_breaker` / `trial_summary` /
`KnowledgeGraph`（error pattern）/ `FileStuckGuard` の失敗記録に反映しない対応を
入れた（`src/agent/tool_exec.rs::apply_tool_result`）。
ユーザーの割り込みはツールの欠陥ではないため、これを負の学習信号にしないという
判断である（docs/VALUES.md V1/V4）。

qa-reviewer は Issue #34 のこの修正を検証する過程で、新たな中程度の欠陥を発見した。
`domain::event::build_trajectory_from_events` は、cancelled な
`tool_call_start`/`tool_call_end` を `tool_sequence`・`total_steps`・
`tool_success_rate` の計算から個別に除外していた。
除外は分子（成功数）だけでなく分母（試行総数）にも及ぶため、実際には中断で
未完遂だったセッションが、残った試行がすべて成功であれば `tool_success_rate`
1.0 の完全な成功 trajectory として記録される。
1 件でも実失敗を含む中断セッションは同様に成功へ反転はしないが、
中断が起きた事実そのものは分母から消え、あたかも中断なしで完走したかのような
記録になる。
`domain::event::classify_session_for_verification` は同じ問題に別の形で対処済み
だった。
こちらは全 `tool_call_end` が cancelled の場合にのみ `None`（検証サンプル対象外）
を返し、部分的な中断は通常どおり集計する非対称な実装だった。

この非対称性を精査すると、真の欠陥は率の計算式ではなく、「このセッションを
エージェント能力のラベル付き標本として使ってよいか」という、セッション単位の
適格性判定が欠けていたことだとわかった。
ツール呼び出し単位で cancelled を除外する Issue #34 の規律自体は正しい。
壊れていたのは 1 段上の層で、中断が混じったセッションから「完遂したか」という
事実を欠損したまま完遂の代理指標（tool_success_rate）を作り続けていた設計である。

## Decision

**ユーザー取消（Ctrl+C）に由来する tool call を 1 件でも含むセッションは、
成功・失敗いずれの学習信号にもしない。**

1. **正の信号にしない。**
   スキル昇格、成功 trajectory への計上、検証成功率の分子への算入のいずれも
   行わない。

2. **負の信号にもしない。**
   `circuit_breaker` / `trial_summary` / `KnowledgeGraph` の error pattern /
   `FileStuckGuard` / 失敗 trajectory への計上のいずれも行わない。
   「正の信号にしない」だけでは、分母に残す限り部分中断セッションが失敗として
   記録され、ユーザーのキーストロークを負の学習信号に変換してしまう。
   本 ADR はこれも禁じる。

3. **中断を含むセッションは標本対象外とし、成功・失敗の二値に押し込まない。**
   `domain::event::has_user_cancelled_tool_call(events: &[Event]) -> bool` を
   domain 層に新設し、`build_trajectory_from_events` と
   `classify_session_for_verification` の両方がこの述語を共有する。
   前者はセッション内に cancelled な tool call が 1 件でもあれば
   `TrajectoryCandidate` を構築せず `None` を返す。
   後者も同じ条件で `None` を返すよう、従来の「全 `tool_call_end` が cancelled」
   という条件を「1 件でも cancelled」へ強化した。
   `Option::None` は既に「`SessionEnd` 不在で候補にならない」場合に使われており、
   「標本として不適格」を `None` に載せるのは既存の意味論と一貫する。
   新規エラー型は導入しない。
   これは失敗ではなく不在であり、`Result` の `Err` は I/O 障害のみに予約する
   既存方針を維持する。

4. **判定はセッションの構築点 1 箇所に集約する。**
   `build_trajectory_from_events` は `EventStore`（SQLite）と
   `MockEventRepository`（in-memory）の両実装から共有される pure helper である。
   ここで `None` を返せば、既存の 4 メソッド（成功/失敗 × 通常/since_id）と
   将来の任意の呼び出し元に自動的に適用される。
   実装ごとの分岐は生じない。
   この gate は `min_steps` に依存しないため、将来 `min_steps=0` で
   trajectory 抽出を呼び出す経路（項目 235
   `BONSAI_FACTCHECK_ALL_TRAJECTORIES=1` など）が追加されても、不変条件は
   構造的に保たれる。

5. **例外: エージェントループの継続制御用 `is_failed = true` は維持する。**
   これは学習ではなく、ループを打ち切るための制御フラグであり、本 ADR の
   対象範囲外である。

6. **`memory::experience::extract_reflection_full`（AgentHER Hindsight
   Subgoal Labeling）の per-call cancelled 除外は変更しない。**
   この経路は事象列を直接読み、cancelled な tool call をすでに除外した状態で
   hindsight relabel を組み立てている。
   決定 3 によりセッション単位の gate が先に働くため、中断セッションはそもそも
   失敗 trajectory の候補に選ばれない。
   二重の安全策になっており、既存実装を変更する理由がない。

## Consequences

### Positive

- `build_trajectory_from_events` と `classify_session_for_verification` が
  同じ問いに同じ答えを返すようになり、モジュール間の自己矛盾（同一の tool
  call が `circuit_breaker` では除外され trajectory 率では計上される、という
  状態）が解消した。
- 判定が構築点 1 箇所に閉じているため、`EventStore`/`MockEventRepository` の
  parity が壊れるリスクが構造的に小さい。
  `tests/session_isolation.rs` に相当する固定は、
  `src/agent/event_store.rs::t_event_store_partial_cancel_excluded_from_both_directions`
  と
  `src/memory/mocks/event_repository_mock.rs::t_mock_partial_cancel_excluded_even_with_min_steps_zero`
  が担う。
- `verification_success_rate` は標本数が減る方向にしか変化しない。
  `min_samples` 未満なら `None` を返し、
  `AdvisorConfig::dynamic_skip_threshold`（既定 0.0、OFF 相当）へ fallback
  するため、検証 step を誤って skip する方向には倒れない。

### Negative

- 対話利用時、中断を挟みつつ最終的に完走したセッションからの学習素材が
  失われる。
  Lab/benchmark セッションは Ctrl+C を発生させないため、この損失は
  対話セッションに限られ、Lab スコアには影響しない。
- `summarize_events` が中断セッションの reflection prompt に渡す
  `TOOL_END(cancelled)` 相当の表示は、本 ADR の決定 3 が働けばそもそも
  当該セッションが trajectory 抽出の対象外になるため到達しなくなる。
  到達しなくなったコード経路への追加修正は本 ADR の範囲外とし、別 Issue の
  対象とする。

## Rejected Alternatives

- **分母に残す案。**
  中断セッションを失敗バケットへ通常どおり計上する案。
  ユーザーの割り込みを負の学習信号に変換することになり、決定 2 と正面から
  衝突するため恒久的に却下する。
- **成功側のみ除外する非対称案。**
  失敗側（AgentHER Hindsight Subgoal Labeling）は未完遂の trajectory を前提と
  する機構であり中断に頑健なため、成功側だけを塞げば理屈は通る。
  しかし判定を方向ごとに分けると構築点で一括強制できず、`EventStore` と
  `MockEventRepository` の 2 実装 × 2 方向で手動の分岐が発生する。
  得られる利益は対話セッションでしか発生しない少量データであり、Lab では
  Ctrl+C が起きないため効果を測定する手段もない。
  「中断セッションの hindsight relabel に価値がある」という実測が得られた
  場合に再検討する。
  その場合も `build_trajectory_from_events` に方向を渡す形（enum 引数）を取り、
  構築点の集約は維持する。
- **中断後に再開して完走したセッションの救済案。**
  最後の cancelled tool call より後に非 cancelled な `tool_call_end` が
  存在するかどうかで安価に判別できる。
  しかし正しさを検証する ground truth がなく、Lab では再現しない。
  本リポジトリは過去に unpaired な思いつき改善を paired evidence で 3 回
  連続 REJECT した実績があり（[ADR-003](ADR-003-paired-evidence-over-unpaired.md)）、
  検証できない refinement を先に入れることは避ける。
  paired evidence による評価設計が用意できてから再検討する。

## Related

- Issue #22（qa-reviewer MEDIUM-2）: `ToolResult::cancelled` の導入
- Issue #34: `apply_tool_result` の学習記録スキップ、
  `domain::event::is_tool_call_cancelled` の導入
- Issue #34 follow-up（本 ADR）: `has_user_cancelled_tool_call` の導入、
  `build_trajectory_from_events`/`classify_session_for_verification` の
  セッション単位 gate 化
- `docs/VALUES.md` V1/V4: フィードバックシグナルの設計原則
- [ADR-002](ADR-002-scaffolding-over-model.md): Scaffolding > Model、
  外部ガードレールは保守的側に倒す
- [ADR-003](ADR-003-paired-evidence-over-unpaired.md): paired evidence の規律
- `src/domain/event.rs`:
  `has_user_cancelled_tool_call`/`build_trajectory_from_events`/
  `classify_session_for_verification`
- `src/agent/event_store.rs`,
  `src/memory/mocks/event_repository_mock.rs`: SQLite/Mock parity test
