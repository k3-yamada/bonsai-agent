#!/usr/bin/env python3
"""Lab v21 — KG seed 拡張後の KG-Grounded Hallucination Check effectiveness paired analyzer.

Plan: .claude/plan/lab-v21-kg-seed-expansion.md §3 Phase 5
入力: scripts/lab_v21_paired.sh が出力する `${LOG_DIR}/test_{on,off}_{1..5}.log`
出力: stdout に各 cycle factcheck summary (matched 軸含む) + Pearson r + paired t-test
      (副次) + ACCEPT/REJECT 判定

依存: 標準ライブラリのみ (scipy 不使用、df=4 t-table を線形補間)。

ACCEPT 判定 (主条件 AND):
  (a) Pearson r >= 0.3 (ON 5 cycle の (conflict+matched+unknown)/total vs failure_rate 相関)
      — Lab v20 の (conf+unk)/total deterministic 解消後、matched 変動軸で variance 復活
  (b) ON cycle 全 5 件で total >= 8 (3 halluc + 5 success の混合発火、項目 242 plan §3 G-7b 同基準)

副次観察 (informational only):
  - paired t-test (Δscore mean / p-value、Lab v17 同) — factcheck 設計上 score 寄与なし
  - matched/total 比 (G-7c で 12/15=0.80、paired 10 cycle で 0.70-0.85 想定)
"""

from __future__ import annotations

import argparse
import math
import re
import sys
from pathlib import Path
from typing import Optional

SCORE_RE = re.compile(
    r"\[lab\]\s*ベースライン:\s*score=([0-9.]+)\s+"
    r"pass@k=([0-9.]+)\s+pass_consec=([0-9.]+)\s+\(([0-9.]+)s\)"
)

FACTCHECK_RE = re.compile(
    r"\[INFO\]\[lab\.factcheck\]\s*FactCheck post-Lab:\s+"
    r"total=(\d+)\s+matched=(\d+)\s+unknown=(\d+)\s+conflicting=(\d+)\s+"
    r"mean_path_len=([0-9.]+)"
)

AGENTHER_RE = re.compile(
    r"\[INFO\]\[lab\.agenther\]\s*AgentHER post-Lab:\s+"
    r"failed=(\d+)\s+successful=(\d+)\s+relabels=(\d+)\s+skills=(\d+)\s+insights=(\d+)"
)


def pearson_r(xs: list[float], ys: list[float]) -> float:
    """Pearson 相関係数 (stdlib only、Lab v17 paired_ttest と同設計)。

    zero variance / n<2 で 0.0 返却 (NaN 回避)。完全相関で ±1.0、無相関で 0.0。
    """
    n = len(xs)
    if n != len(ys) or n < 2:
        return 0.0
    mx = sum(xs) / n
    my = sum(ys) / n
    cov = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    vx = sum((x - mx) ** 2 for x in xs)
    vy = sum((y - my) ** 2 for y in ys)
    if vx <= 0 or vy <= 0:
        return 0.0
    return cov / math.sqrt(vx * vy)


def extract_score(log_text: str) -> Optional[tuple[float, float, float, float]]:
    """log 文字列から (score, pass@k, pass_consec, duration_s) 抽出 (cycle 内最後を採用)."""
    last: Optional[tuple[float, float, float, float]] = None
    for m in SCORE_RE.finditer(log_text):
        last = (
            float(m.group(1)),
            float(m.group(2)),
            float(m.group(3)),
            float(m.group(4)),
        )
    return last


def extract_factcheck_summary(log_text: str) -> Optional[dict]:
    """log 文字列から FactCheck summary (total/matched/unknown/conflicting/mean_path_len) 抽出.

    複数 emit があれば最後を採用 (Lab cycle 1 回 = 1 factcheck pass 想定)。
    """
    last: Optional[dict] = None
    for m in FACTCHECK_RE.finditer(log_text):
        last = {
            "total": int(m.group(1)),
            "matched": int(m.group(2)),
            "unknown": int(m.group(3)),
            "conflicting": int(m.group(4)),
            "mean_path_len": float(m.group(5)),
        }
    return last


def extract_agenther_summary(log_text: str) -> Optional[dict]:
    """log 文字列から AgentHER summary (failed/successful/...) 抽出 (failure_rate 計算用)."""
    last: Optional[dict] = None
    for m in AGENTHER_RE.finditer(log_text):
        last = {
            "failed": int(m.group(1)),
            "successful": int(m.group(2)),
            "relabels": int(m.group(3)),
            "skills": int(m.group(4)),
            "insights": int(m.group(5)),
        }
    return last


def judge_accept(
    on_summaries: list[dict],
    pearson: float,
    accept_r: float = 0.3,
    min_total: int = 8,
) -> tuple[bool, list[str]]:
    """ACCEPT 判定 (plan §2 主条件 AND)。

    (a) Pearson r >= accept_r AND (b) ON 全 cycle で total >= min_total。
    Lab v21 では min_total=8 (3 halluc + 5 success = 項目 242 plan §3 G-7b 同基準)。
    戻り値: (accepted, [reason 文字列リスト]) — accepted=True なら reasons は空。
    """
    reasons: list[str] = []
    if pearson < accept_r:
        reasons.append(
            f"(a) Pearson 相関 r={pearson:.4f} < {accept_r} NG"
        )
    n_fired = sum(1 for s in on_summaries if s.get("total", 0) >= min_total)
    if n_fired < len(on_summaries):
        reasons.append(
            f"(b) ON cycle で total>={min_total} 件数 {n_fired}/{len(on_summaries)} NG (factcheck 不発火 cycle あり)"
        )
    accepted = len(reasons) == 0
    return (accepted, reasons)


def paired_t_stat(deltas: list[float]) -> tuple[float, float, float]:
    """paired t-test (one-sided H1: mean > 0)、df=n-1 (Lab v19 同実装)。"""
    n = len(deltas)
    if n < 2:
        return (deltas[0] if n == 1 else 0.0, 0.0, 1.0)
    mean = sum(deltas) / n
    var = sum((d - mean) ** 2 for d in deltas) / (n - 1)
    if var <= 0:
        return (mean, math.inf if mean > 0 else -math.inf, 0.0 if mean > 0 else 1.0)
    se = math.sqrt(var / n)
    t = mean / se if se > 0 else 0.0

    if n == 5:
        if t <= 0:
            p = 0.5 + min(0.5, abs(t) * 0.1)
        elif t < 1.533:
            p = 0.5 - (t / 1.533) * 0.4
        elif t < 2.132:
            p = 0.10 - ((t - 1.533) / (2.132 - 1.533)) * 0.05
        elif t < 2.776:
            p = 0.05 - ((t - 2.132) / (2.776 - 2.132)) * 0.025
        elif t < 3.747:
            p = 0.025 - ((t - 2.776) / (3.747 - 2.776)) * 0.015
        else:
            p = 0.01
    else:
        p = 0.5 * (1.0 - math.erf(t / math.sqrt(2)))

    return (mean, t, p)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("log_dir", type=Path, help="lab v21 logs directory")
    ap.add_argument(
        "--accept-r",
        type=float,
        default=0.3,
        help="ACCEPT threshold for Pearson r (default 0.3、plan §2 (a))",
    )
    ap.add_argument(
        "--n-pairs",
        type=int,
        default=5,
        help="number of paired cycles (default 5)",
    )
    ap.add_argument(
        "--min-total",
        type=int,
        default=8,
        help="minimum factcheck total for ACCEPT 基準 (b) (default 8 = 3 halluc + 5 success)",
    )
    args = ap.parse_args()

    log_dir: Path = args.log_dir
    if not log_dir.is_dir():
        print(f"ERROR: log_dir not found: {log_dir}", file=sys.stderr)
        return 2

    print(f"=== Lab v21 KG-FactCheck Effectiveness Analyzer (n_pairs={args.n_pairs}, min_total={args.min_total}) ===")
    print(f"  {'pair':>4} {'on_total':>8} {'matched':>7} {'conflict':>8} {'unknown':>7} "
          f"{'mat/tot':>8} {'on_score':>8} {'off_score':>9} {'delta':>8} {'fail_rate':>10}")
    print("  " + "-" * 4 + " " + "-" * 8 + " " + "-" * 7 + " " + "-" * 8 + " "
          + "-" * 7 + " " + "-" * 8 + " " + "-" * 8 + " " + "-" * 9 + " "
          + "-" * 8 + " " + "-" * 10)

    on_summaries: list[dict] = []
    on_failure_rates: list[float] = []
    on_cmu_rates: list[float] = []  # (conflict + matched + unknown) / total
    score_deltas: list[float] = []
    on_scores: list[float] = []
    off_scores: list[float] = []
    missing = 0

    for i in range(1, args.n_pairs + 1):
        on_path = log_dir / f"test_on_{i}.log"
        off_path = log_dir / f"test_off_{i}.log"
        if not on_path.exists() or not off_path.exists():
            missing += 1
            print(f"  {i:>4} MISSING (on={on_path.exists()} off={off_path.exists()})")
            continue
        on_text = on_path.read_text(encoding="utf-8", errors="replace")
        off_text = off_path.read_text(encoding="utf-8", errors="replace")

        fc = extract_factcheck_summary(on_text)
        agh = extract_agenther_summary(on_text)
        on_sc = extract_score(on_text)
        off_sc = extract_score(off_text)

        if fc is None or agh is None or on_sc is None or off_sc is None:
            missing += 1
            print(f"  {i:>4} INCOMPLETE (fc={fc is not None} "
                  f"agh={agh is not None} on_sc={on_sc is not None} off_sc={off_sc is not None})")
            continue

        total = fc["total"]
        conflict = fc["conflicting"]
        matched = fc["matched"]
        unknown = fc["unknown"]
        on_summaries.append(fc)

        # Lab v21 主軸: (conf + matched + unk) / total (Lab v20 の (conf+unk)/total を matched 含む形に拡張)
        cmu_rate = (conflict + matched + unknown) / total if total > 0 else 0.0
        on_cmu_rates.append(cmu_rate)
        mt_rate = matched / total if total > 0 else 0.0

        failed = agh["failed"]
        successful = agh["successful"]
        denom = failed + successful
        fail_rate = failed / denom if denom > 0 else 0.0
        on_failure_rates.append(fail_rate)

        on_s, off_s = on_sc[0], off_sc[0]
        delta = on_s - off_s
        score_deltas.append(delta)
        on_scores.append(on_s)
        off_scores.append(off_s)

        print(f"  {i:>4} {total:>8} {matched:>7} {conflict:>8} {unknown:>7} "
              f"{mt_rate:>8.4f} {on_s:>8.4f} {off_s:>9.4f} {delta:>+8.4f} {fail_rate:>10.4f}")
    print()

    if missing:
        print(f"WARNING: {missing} pair(s) incomplete — analysis on partial data",
              file=sys.stderr)

    if len(on_summaries) < 2:
        print("ERROR: need at least 2 complete ON cycles for Pearson r", file=sys.stderr)
        return 3

    pearson = pearson_r(on_cmu_rates, on_failure_rates)
    accepted, reasons = judge_accept(on_summaries, pearson, accept_r=args.accept_r, min_total=args.min_total)

    print("=== Pearson Correlation (主 ACCEPT 軸: Lab v21 matched 軸拡張) ===")
    print(f"  ON (conflict+matched+unknown)/total: {on_cmu_rates}")
    print(f"  ON failed/(failed+successful): {on_failure_rates}")
    print(f"  Pearson r: {pearson:+.4f}  (threshold accept_r={args.accept_r})")
    print()

    print("=== Score (副次 Lab v17 同 paired t-test) ===")
    if score_deltas:
        mean, t, p = paired_t_stat(score_deltas)
        on_mean = sum(on_scores) / len(on_scores)
        off_mean = sum(off_scores) / len(off_scores)
        print(f"  ON  mean: {on_mean:.4f}  (n={len(on_scores)})")
        print(f"  OFF mean: {off_mean:.4f}  (n={len(off_scores)})")
        print(f"  paired delta mean: {mean:+.4f}")
        print(f"  paired t-stat:     {t:+.4f}  (df={len(score_deltas) - 1})")
        print(f"  one-sided p:       {p:.4f}")
        print("  (factcheck is post-hoc metric — score 寄与なし設計、informational only)")
    print()

    print("=== matched/total 比 (副次 G-7c smoke 0.80 との比較参照) ===")
    if on_summaries:
        mt_rates = [s["matched"] / s["total"] if s["total"] > 0 else 0.0 for s in on_summaries]
        mt_mean = sum(mt_rates) / len(mt_rates)
        print(f"  ON 5 cycle matched/total ratios: {[f'{r:.4f}' for r in mt_rates]}")
        print(f"  mean: {mt_mean:.4f}  (G-7c smoke ref: 0.8000)")
    print()

    print("=== ACCEPT 判定 (Plan A 系列 H_factcheck 採用判断、Lab v21) ===")
    print(f"  Pearson r >= {args.accept_r}: {pearson >= args.accept_r}  (got {pearson:+.4f})")
    n_fired = sum(1 for s in on_summaries if s.get("total", 0) >= args.min_total)
    print(f"  ON 全 cycle で total>={args.min_total}: {n_fired == len(on_summaries)}  "
          f"(got {n_fired}/{len(on_summaries)})")
    if accepted:
        print("  → ACCEPT (H_factcheck 採用、項目 244 KG lint + Lab v22+ retry hook 試行検討)")
        return 0
    else:
        print("  → REJECT")
        for reason in reasons:
            print(f"    - {reason}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
