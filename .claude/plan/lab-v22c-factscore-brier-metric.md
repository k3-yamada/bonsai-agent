# Lab v22c FActScore + Brier Score Metric — Atomic Fact 軸 (項目 247c 候補)

**状態**: planning-only (2026-05-19 起票、CCG Gemini 提案より)
**推奨度**: ★ (項目 247 metric redesign の効果サイズ補強案、長期)
**推定工数**: ~2h plan + ~3-4h impl + Phase 4 smoke
**起点**:
- Gemini 提案: 「FActScore (Min et al., 2023): Atomic Facts 分解 + KG 照合で事実密度精度、n=15 task で 100-200 fact 抽出可」
- Lab v21 smoke で task 単位 score (pass@k) の variance が小さい、atomic 単位なら n 稼げる

---

## §1. 問題定義

### 1.1 現行 metric の n 不足
- 15 task × k=3 cycle = 45 task 実行、ただし score は 1 cycle 1 数値 (n=5 paired)
- Wilcoxon p=0.156 で n=5 限界に当たる
- 効果サイズ「effect per atomic fact」を測れば n=100+ で検出力上がる

### 1.2 FActScore (Min et al., 2023) の核心
1. 生成 answer を atomic facts に分解 (1 fact = 1 命題)
2. 各 fact を KG/reference で verify (supported / contradicted / unknown)
3. fact_precision = supported / total atomic facts

### 1.3 Brier Score の核心
- factcheck が「unknown」と判定した triple が、実際に間違っていたかを 0/1 で評価
- Brier Score = mean((p_prediction - actual)^2)、calibration 測定

---

## §2. 設計 (案 A 推奨)

### 案 A (推奨): FActScore + Brier を補助 metric として追加、ACCEPT 主判定は paired Δscore 維持
- 項目 247 ACCEPT 基準 (a)(b)(c)(d) は維持
- 新規補助 metric:
  - fact_precision = supported / total (range 0-1、informational)
  - brier_score = mean((unknown_pred - 1.0_if_wrong)^2) (range 0-1、low = calibrated)
- ACCEPT 判定にはまだ組込まない、本 plan では「観測のみ」の Phase

### 案 B (棄却): Wilcoxon を捨てて FActScore 主判定化
- 1bit Bonsai-8B の atomic fact 分解精度自体が不確実
- 主判定切替は metric 信頼性確証後

### 案 C (棄却): 外部 LLM (GPT-4 等) で atomic 分解
- bonsai-agent の self-contained 原則と矛盾

---

## §3. 実装 (Phase 1-3、TDD strict)

### Phase 1 (Red) — 3 failing test
1. `t_factscore_extracts_atomic_facts`: "Bonsai-Agent is a Rust project that uses 1bit quantization" → 2 facts extract
2. `t_factscore_verifies_against_kg`: KG seed と照合、supported/contradicted/unknown 分類
3. `t_brier_score_calibration`: 既知の unknown 予測リストで Brier 計算

### Phase 2 (Green)
- `src/memory/factcheck.rs` に `extract_atomic_facts(text) -> Vec<AtomicFact>` (regex-based、~50 行)
- `verify_atomic_fact(fact, &KG) -> VerifyResult { Supported | Contradicted | Unknown }`
- `scripts/lab_v22_metric.py` に `fact_precision()` / `brier_score()` 追加

### Phase 3 (Refactor)
- log prefix `[INFO][lab.factscore]` で metric 出力
- `BONSAI_LAB_FACTSCORE_ENABLED=1` env で gate (default OFF、計測 overhead 回避)

---

## §4. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | 1bit Bonsai-8B の atomic fact 分解精度不確実 | Phase 4 で manual annotation 10 sample で precision check (>=0.8 で続行) |
| R2 | FActScore 計算 overhead で cycle 延長 | env gate default OFF、計測対象 cycle のみ ON |
| R3 | Brier の actual ground truth 設定が困難 (unknown が実際に true か false かの determinator) | KG verify 結果を ground truth proxy として使用、circular reasoning は限定的 |

---

## §5. 期待効果
- n=15 task でも atomic fact 100+ で検出力 up (Gemini 提案根拠)
- factcheck の calibration (unknown 判定の妥当性) 数値化
- Lab v23+ で ACCEPT 主判定軸への昇格候補

---

## §6. 依存 / 並行性

### 完遂前提
- 項目 247 Lab v22 Phase A-D 完遂 (本 plan は metric 拡張軸、Phase B analyzer ベース)
- 項目 244 KG lint 完遂 ✅ (atomic fact verify の KG seed 整合性)

### 並行可
- 項目 247b task composition (benchmark.rs 軸)
- 項目 247d Attention Entropy (推論経路軸)

---

## §7. metadata
- 起点: CCG Gemini 「FActScore (Min et al., 2023) 採用」
- 論文: Min, S. et al. (2023). "FActScore: Fine-grained Atomic Evaluation of Factual Precision"
- 関連 plan: `lab-v22-metric-redesign.md` (項目 247、metric redesign 主軸)
- 想定 commit 範囲: 3-4 commit (atomic fact extract + verify + Brier + smoke)
- 想定 line 範囲: +250 行 / -10 行 (factcheck.rs + lab_v22_metric.py 拡張)
