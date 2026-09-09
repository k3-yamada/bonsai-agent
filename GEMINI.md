# GEMINI.md - bonsai-agent Project Context & Instructions

このファイルは、Antigravity が `bonsai-agent` リポジトリで作業する際に適用される最優先プロジェクトコンテキストです。

---

## 1. プロジェクト概要

- **名称**: `bonsai-agent`
- **中核技術**: ローカル小型LLM（既定 MiniCPM5-2B、GGUF Q4_K_M 1.56GB。旧既定 Bonsai-8B 1-bit）で動作する Rust 製自律型エージェント。
- **実行環境**: Mac M2 16GB、llama-server / MLX HTTP API 経由で推論。
- **テスト規模**: テスト数は `cargo test --lib`、ソースファイル数は `find src -name '*.rs' | wc -l` で確認。Rust 2024 edition。
- **アーキテクチャ特性**: 非同期ランタイム (tokio) ではなく、**完全同期アーキテクチャ** (`ureq`, `reqwest::blocking`, `CancellationToken`) を採用。

---

## 2. 最重要設計原則

### ① 「Scaffolding > Model」原則
小型ローカルモデル（旧 1-bit 量子化 Bonsai-8B、現 MiniCPM5-2B）の推論能力には物理的限界があります。モデルの能力に依存するのではなく、**ハーネス（Scaffolding）、ガードレール、自己反省ループ、文脈圧縮、リトライ・検証機構** によってシステム全体の信頼性を底上げします。

### ② VALUES.md（思想の錨）
設計や判断に迷ったときは、必ず [`docs/VALUES.md`](docs/VALUES.md) に立ち返ってください。
- **V1. 表層の問いの背後を見る**: 真のニーズ・暗黙の意図を推定。
- **V2. 未知の自己開示**: 盲点や前提の気づきの種を埋め込む。
- **V3. 経験からの自己更新**: 同じ失敗を繰り返さない。
- **V4. 依存ではなく、共鳴**: ユーザーの思考の触媒となる。
- **V5. 自立的判断**: 盲目的な服従は洗練された無責任。必要に応じ明確に異議を唱える。
- **V6. 不確実性への誠実さ**: 確信のないことを断定しない。
- **V7. 自己の変質への警戒**: 改善と変質を区別し、価値のドリフトを定期的に問い直す。

### ③ Goodhart's Law 対策
指標（スコア、成功率）の単調増加や安易な改善に騙されてはいけません。
全指標が同時に単調増加し、重みが固定されている場合は形骸化の疑い（`MetricConsistencyChecker`）を考慮してください。

### ④ ADR-003: Paired Evidence 規律
単一実行（unpaired）での「スコア向上」はノイズである可能性が高いため、必ず統計的裏付け（Cohen's dz, Wilcoxon 等による paired 検証）を重視します。

---

## 3. レイヤー境界ルール（DEP-001）

Clean Architecture を厳格に適用しています。依存の向きは**常に下層（外側から内側）のみ**です。

```
domain < db < observability < safety < memory < knowledge < runtime < tools < agent < main
```

- **下層のみ `use crate::<下層>::*` 可能**。上層への依存は固く禁止されています。
- **テストコード (`#[cfg(test)]`) も DEP-001 の対象** です。テストだからといって上層の具象をインポートしてはならず、port trait または下層モック（`MockLlmBackend`, `MockEventRepository` 等）を使用してください。
- レイヤー整合性は `cargo test --test structural` で検証されます。

---

## 4. 厳格な運用注意事項（絶対遵守）

1. **【最重要】Clippy 巻き戻し禁止**:
   `write_to_file` や `replace_file_content` で編集後、clippy 警告（`collapsible_if`, `too_many_arguments` 等）を理由に変更前の状態に巻き戻す行為は**絶対に禁止**です。必要な修正は追加のコード修正で行ってください。特に `error_recovery.rs`, `benchmark.rs`, `agent_loop.rs` では要注意。
2. **【Lab 稼働中の `cargo build --release` 禁止】**:
   Lab や Smoke（`scripts/lab_v22_aa_test.sh` 等）の稼働中に release ビルドを行うと、`target/release/bonsai` が上書きされ実験の十数時間に及ぶ一貫性が破壊されます。ユニットテスト・検証には `cargo test --lib` を使用してください。
3. **完全同期ランタイムの維持**:
   コアロジックやハーネスに安易に `tokio::spawn` などの非同期プリミティブを持ち込まず、既存の同期設計・`CancellationToken`・スレッドモデルを尊重してください。

---

## 5. コマンドリファレンス

- ユニットテスト: `cargo test --lib`
- 構造・レイヤー・ログ検証: `cargo test --test structural`
- リント: `cargo clippy -- -D warnings`
- フォーマット: `cargo fmt -- --check`
- ケイパビリティ一覧: `cargo run -- --manifest`
