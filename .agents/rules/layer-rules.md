---
trigger: always_on
description: "Clean Architecture レイヤー境界ルール (DEP-001) の強制"
---

# bonsai-agent Clean Architecture & Layer Rules (DEP-001)

bonsai-agent では、モジュール間の結合度を抑え、単体テスト可能性と循環依存防止を保証するため、厳格なレイヤー順序を定義しています。

## 1. レイヤー順序

```
domain < db < observability < safety < memory < knowledge < runtime < tools < agent < main
```

1. **domain**: 純粋なエンティティ、値オブジェクト、port trait（他層への依存ゼロ）。
2. **db**: SQLite スキーマ定義、マイグレーション (`apply_all`)。
3. **observability**: 構造化ログ、監査ログ。
4. **safety**: シークレットフィルタ、ブートガード、サンドボックス、ネットワークポリシー。
5. **memory**: 記憶層（A-MEM store, experience, skill, graph, factcheck, decay, review, dreams）。
6. **knowledge**: ナレッジ抽出、vault、vault_lint。
7. **runtime**: 推論エンジン、llama_server 連携、model_router、FallbackBackend。
8. **tools**: Tool trait、ToolRegistry、各具象ツール（shell, git, web, file, mcp 等）。
9. **agent**: agent_loop、benchmark、experiment、middleware、event_store、compaction、task。
10. **main**: CLI エントリポイント、バイナリ。

## 2. 依存制約

- **各レイヤーは「自身より下層」のみをインポート可能 (`use crate::<下層>::*`)。**
- 上層への依存（例: `db` から `agent` を参照、`memory` から `tools` を参照など）は即座にアーキテクチャ違反。
- **例外なしのテストコード適用**:
  - `#[cfg(test)]` 内のコードも DEP-001 の対象です。
  - テストだからといって上層の具象型を直接インポートしてはいけません。
  - `domain::llm::MockLlmBackend` や `domain::event::MockEventRepository` などの下層モック、または port trait を通じてテストしてください。
- cross-cutting concern（全層から参照可能）は `cancel`（キャンセルトークン）および `config` のみ。

## 3. 検証方法

レイヤー違反は `cargo test --test structural` で機械的に検出されます。コード追加・変更後は必ずこのテストをパスさせてください。
