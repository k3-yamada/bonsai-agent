---
id: bonsai-trait-mock-testing
trigger: "エージェントのテストを書くとき"
confidence: 0.95
domain: testing
source: project-design
---

# トレイトモックでモデル不要テスト

## アクション
`LlmBackend`トレイトに対して`MockLlmBackend`を実装し、スクリプト化されたレスポンス
（`Vec<String>`）を返す。エージェントループ・パーサー・ツールレジストリのテストは
実モデルなしで実行する。実モデルが必要なテストには`#[ignore]`を付ける。

## 根拠
llama-cpp-2のモデルロードは数秒かかり、1.28GBのモデルDLが必要。
トレイト抽象でモック可能にすることで、`cargo test`が高速に回り、CIでも動作する。
