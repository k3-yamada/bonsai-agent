# Step 8 (依存最適化) 詳細評価書

**Source:** `structural-improvements-v2.md` Step 8（P3、未着手）
**Date:** 2026-04-27
**Status:** 評価（着手判断資料）

---

## サマリー

`cargo tree --duplicates` で 14 重複が検出されたが、**直接対応可能なのは 1 件のみ（ureq v2 → v3 への hf-hub 経由削減）**。残る 13 件は深い transitive 依存で、削減には fastembed/reqwest/tokenizers などの上流変更が必要。

**推奨**: ureq 重複の 1 件のみ削減（または許容）、他は維持。バイナリサイズ -10% は達成困難（実測 -2% 程度）。**Step 8 全体の優先度を下げる**判断を提案。

---

## 詳細分析（14 重複）

### 直接対応可能（★優先）

#### **1. ureq v2.12.1 vs v3.3.0**
- **v2.12.1 利用元**: `hf-hub v0.4.3` (fastembed 経由)
- **v3.3.0 利用元**: `bonsai-agent` 直接 + `ort-sys` (build-dep)
- **対応案**:
  - hf-hub を ureq v3 対応版（あれば）に upgrade
  - fastembed の代わりに ONNX 直叩きに切替（大規模変更）
- **削減効果**: ureq 自体 ~50KB、依存 chain で 200KB 程度
- **判定**: ★ **試行価値あり**（hf-hub upgrade で済めば 1h 工数）

### 許容推奨（★★星なし）

| # | crate | バージョン | 利用元 | 削減困難な理由 |
|---|-------|----------|--------|---------------|
| 2 | base64 v0.13.1 | spm_precompiled (transitive) | hf-hub 経由、上流変更不可 |
| 3 | base64 v0.22.1 | reqwest/ureq/hyper (multi) | 主要 HTTP 系全部、削減不可 |
| 4 | core-foundation v0.9.4 | system-configuration | macOS 専用、軽量 |
| 5 | core-foundation v0.10.1 | security-framework | macOS 専用、軽量 |
| 6 | getrandom v0.2.17 | ring（rustls 経由） | 暗号系、変更危険 |
| 7 | getrandom v0.3.4 | ahash/rand_core/tokenizers | 多数の transitive、変更不可 |
| 8 | getrandom v0.4.2 | tempfile/uuid | 比較的新しいが全体 chain 変更必要 |
| 9 | hashbrown v0.14.5 | rusqlite (hashlink) | rusqlite が古い hashbrown 利用 |
| 10 | hashbrown v0.16.1 | indexmap/safetensors | 主要、削減不可 |
| 11 | http v1.4.0 (×2) | h2/reqwest/hyper + ureq-proto | 名前空間衝突なし、許容 |
| 12 | httparse v1.10.1 | ureq-proto | ureq v3 専用 |
| 13 | memchr | nom 経由など | 多数、削減不可 |
| 14 | nom | toml 系など | parser 系 |
| 15 | rustls-pki-types | rustls 経由 | 暗号系 |
| 16 | serde_core / serde_json | 1.0.149 主、build-dep に異版 | build-time のみ重複 |
| 17 | webpki-roots | rustls 経由 | 暗号系 |

### 削減できない理由カテゴリ

1. **fastembed transitive**: hf-hub/safetensors/tokenizers/ort が古い base64/ureq/hashbrown を引き連れる
2. **暗号スタック**: ring/rustls 系の getrandom/webpki-roots は変更危険
3. **HTTP 系**: reqwest と ureq の共存（reqwest=async、ureq=blocking）必須

## バイナリサイズ削減見込み

| シナリオ | 削減可能サイズ | 工数 | リスク |
|---------|---------------|------|--------|
| ureq 重複削減（hf-hub upgrade） | ~200KB | 1h | 低（破壊的変更なし想定） |
| fastembed 削除（embedding を別実装に） | ~5MB | 16h+ | 高（セマンティック検索機能影響） |
| reqwest または ureq 一方削除 | ~500KB | 8h | 中（HTTP 系の async/blocking 統一が必要） |

**現実的な削減**: **~200KB（バイナリ -2%）**。Step 8 plan の「-10%」目標は達成困難。

## 推奨判断

### 採用案 A: 軽量実施（推奨）
- ureq 重複削減のみ（1h、200KB 削減）
- 他は許容、`cargo tree --duplicates` を CI に組み込んで監視

### 採用案 B: スキップ（推奨次点）
- 200KB のメリットに対しコスト（破壊リスク）が見合わない
- Step 8 全体を「将来 fastembed を別実装に置き換える際にまとめて対応」として保留

### 不採用案: 全件削減
- 工数 30h+、依存上流の変更必要、現実的でない

## 判定ゲート

実施前に確認:

- [ ] `bonsai` バイナリの現サイズ（`ls -la target/release/bonsai`）
- [ ] hf-hub crate の最新版が ureq v3 をサポートするか確認（`cargo search hf-hub`）
- [ ] Lab v13 完了 + バイナリサイズ重要度の評価

## 結論

**Step 8 は軽量実施（案 A）が妥当だが、Lab v13 + 構造改善 v3 (DiffStore 等) の方が ROI 高い。
Step 8 を後続化、ureq 重複は CI 警告として可視化のみ実施することを推奨。**

## 関連
- 親計画: `.claude/plan/structural-improvements-v2.md` Step 8
- 後続: `.claude/plan/diffstore-rust-impl.md`（より高 ROI）
