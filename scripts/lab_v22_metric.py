#!/usr/bin/env python3
"""Lab v22 — Paired Δscore 主軸 metric analyzer (CCG synthesis 経由)。

Plan: .claude/plan/lab-v22-metric-redesign.md §3 + §4 Phase B
入力: scripts/lab_v22_paired.sh / lab_v22_aa_test.sh が出力する
      `${LOG_DIR}/test_{on,off}_{1..N}.log` または A/A モードで両側 OFF。
出力: stdout に各 cycle summary + Wilcoxon + paired t-test + Cohen's dz +
      factcheck 補助 sanity gate + Pearson r 診断ログ + ACCEPT/REJECT 判定。

依存: 標準ライブラリのみ (scipy 不使用、Wilcoxon は n<=25 で exact distribution)。

ACCEPT 基準 (.claude/plan/lab-v22-metric-redesign.md §3.1):
  (a) mean(Δscore) >= max(+0.010, noise_floor × 2)
  (b) Wilcoxon one-sided p <= 0.10 (smoke) / 0.05 (full lab)
  (c) paired Cohen's dz >= 0.30 (smoke) / 0.40 (full)
  (d) factcheck sanity:
      mean(matched/total) >= 0.78
      AND mean(unknown/total) <= 0.05
      AND total >= 8 per cycle

(a) AND (b) AND (c) で主判定、(d) は補助ゲート (false-alarm 抑止)。

--mode paired (default): ON×OFF paired ACCEPT/REJECT 判定
--mode aa: OFF×OFF A/A test、noise floor σ_Δ 出力のみ (verdict なし)
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


def extract_score(log_text: str) -> Optional[tuple[float, float, float, float]]:
    """log から (score, pass@k, pass_consec, duration_s) 抽出 (cycle 内最後)."""
    last: Optional[tuple[float, float, float, float]] = None
    for m in SCORE_RE.finditer(log_text):
        last = (float(m.group(1)), float(m.group(2)), float(m.group(3)), float(m.group(4)))
    return last


def extract_factcheck_summary(log_text: str) -> Optional[dict]:
    """log から FactCheck summary 抽出 (cycle 内最後の emit)."""
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
    """log から AgentHER summary 抽出 (failure_rate 計算用)."""
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


def pearson_r(xs: list[float], ys: list[float]) -> float:
    """Pearson 相関 (stdlib only)、zero variance / n<2 で 0.0 返却."""
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


def paired_t_stat(deltas: list[float]) -> tuple[float, float, float]:
    """paired t-test (one-sided H1: mean > 0)、df=n-1.

    n=5 では df=4 t-table を線形補間、n>=10 では normal approximation。
    """
    n = len(deltas)
    if n < 2:
        return (deltas[0] if n == 1 else 0.0, 0.0, 1.0)
    mean = sum(deltas) / n
    var = sum((d - mean) ** 2 for d in deltas) / (n - 1)
    if var <= 0:
        return (mean, math.inf if mean > 0 else -math.inf, 0.0 if mean > 0 else 1.0)
    se = math.sqrt(var / n)
    t = mean / se if se > 0 else 0.0

    # t-table (df=n-1) 線形補間、p は one-sided greater
    if n == 5:  # df=4
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
        # n>=10 では normal approximation (CLT)
        p = 0.5 * (1.0 - math.erf(t / math.sqrt(2)))

    return (mean, t, p)


def cohen_dz(deltas: list[float]) -> float:
    """paired Cohen's dz = mean(Δ) / sd(Δ).

    n<2 で 0、sd=0 で sign-aware infinity (mean>0 で +inf、mean<0 で -inf、mean=0 で 0)。
    """
    n = len(deltas)
    if n < 2:
        return 0.0
    mean = sum(deltas) / n
    var = sum((d - mean) ** 2 for d in deltas) / (n - 1)
    sd = math.sqrt(var)
    if sd <= 0:
        if mean > 0:
            return math.inf
        if mean < 0:
            return -math.inf
        return 0.0
    return mean / sd


def wilcoxon_signed_rank_p(deltas: list[float]) -> tuple[float, float]:
    """Wilcoxon Signed-Rank one-sided p (H1: mean > 0).

    n<=25 で exact distribution (2^n enumeration)、n>25 で normal approximation。
    zero deltas は除外、同順位は average rank。
    戻り値: (W+, p-value)。
    """
    # zero 除外
    non_zero = [d for d in deltas if d != 0]
    n = len(non_zero)
    if n == 0:
        return (0.0, 0.5)

    # 絶対値で順位付け、同順位は平均順位
    abs_vals = sorted(set(abs(d) for d in non_zero))
    # 各 abs 値の rank を計算 (1-indexed、tie は平均)
    rank_of: dict[float, float] = {}
    pos = 1
    for v in abs_vals:
        count = sum(1 for d in non_zero if abs(d) == v)
        # 順位 pos, pos+1, ..., pos+count-1 の平均
        avg = (pos + (pos + count - 1)) / 2
        rank_of[v] = avg
        pos += count

    # 正の delta の rank 合計 W+
    w_plus = sum(rank_of[abs(d)] for d in non_zero if d > 0)
    # 全 rank 合計 (n(n+1)/2)、W- = total - W+

    if n <= 25:
        # exact: 各 delta の符号を ±1 で全探索 (2^n)
        all_signed_w_plus: list[float] = []
        for mask in range(1 << n):
            wp = 0.0
            for i in range(n):
                if (mask >> i) & 1:
                    wp += rank_of[abs(non_zero[i])]
            all_signed_w_plus.append(wp)
        total = len(all_signed_w_plus)
        # one-sided greater: P(W+ >= observed)
        ge = sum(1 for x in all_signed_w_plus if x >= w_plus)
        p = ge / total
    else:
        # normal approximation
        mu = n * (n + 1) / 4
        # sigma^2 = n(n+1)(2n+1)/24、tie correction は省略 (small effect)
        sigma2 = n * (n + 1) * (2 * n + 1) / 24
        sigma = math.sqrt(sigma2)
        z = (w_plus - mu) / sigma if sigma > 0 else 0.0
        p = 0.5 * (1.0 - math.erf(z / math.sqrt(2)))

    return (w_plus, p)


def factcheck_sanity_gate(
    on_summaries: list[dict],
    min_mt: float = 0.78,
    max_ut: float = 0.05,
    min_total: int = 8,
) -> tuple[bool, dict]:
    """factcheck 補助 sanity gate (plan §3.1 (d)).

    戻り値: (passed, detail_dict)。
    """
    n = len(on_summaries)
    if n == 0:
        return (False, {"reason": "no summaries"})

    mt_rates = [s["matched"] / s["total"] if s["total"] > 0 else 0.0 for s in on_summaries]
    ut_rates = [s["unknown"] / s["total"] if s["total"] > 0 else 0.0 for s in on_summaries]
    totals = [s["total"] for s in on_summaries]

    mt_mean = sum(mt_rates) / n
    ut_mean = sum(ut_rates) / n
    total_min = min(totals)

    gate_a = mt_mean >= min_mt
    gate_b = ut_mean <= max_ut
    gate_c = total_min >= min_total
    passed = gate_a and gate_b and gate_c

    detail = {
        "mt_mean": mt_mean,
        "ut_mean": ut_mean,
        "total_min": total_min,
        "min_mt": min_mt,
        "max_ut": max_ut,
        "min_total": min_total,
        "gate_a": gate_a,
        "gate_b": gate_b,
        "gate_c": gate_c,
        "passed": passed,
    }
    return (passed, detail)


def judge_accept_v22(
    deltas: list[float],
    on_summaries: list[dict],
    noise_floor: float,
    mode: str = "smoke",
) -> tuple[bool, list[str], dict]:
    """Lab v22 統合 ACCEPT 判定 (plan §3.1).

    mode='smoke': dz>=0.30、p<=0.10
    mode='full': dz>=0.40、p<=0.05

    戻り値: (accepted, reasons, metrics_dict)。
    """
    if mode == "full":
        dz_threshold = 0.40
        p_threshold = 0.05
    else:
        dz_threshold = 0.30
        p_threshold = 0.10

    mean = sum(deltas) / len(deltas) if deltas else 0.0
    dz = cohen_dz(deltas)
    w_plus, wp = wilcoxon_signed_rank_p(deltas)
    _, t, tp = paired_t_stat(deltas)

    # (a) Δ threshold
    delta_threshold = max(0.010, noise_floor * 2)
    gate_a = mean >= delta_threshold

    # (b) Wilcoxon
    gate_b = wp <= p_threshold

    # (c) Cohen's dz
    gate_c = dz >= dz_threshold

    # (d) factcheck sanity (補助)
    sanity_passed, sanity_detail = factcheck_sanity_gate(on_summaries)

    metrics = {
        "delta_mean": mean,
        "delta_threshold": delta_threshold,
        "dz": dz,
        "dz_threshold": dz_threshold,
        "wilcoxon_w_plus": w_plus,
        "wilcoxon_p": wp,
        "ttest_t": t,
        "ttest_p": tp,
        "p_threshold": p_threshold,
        "noise_floor": noise_floor,
        "gate_a": gate_a,
        "gate_b": gate_b,
        "gate_c": gate_c,
        "gate_d_sanity": sanity_passed,
        "sanity_detail": sanity_detail,
        "mode": mode,
    }

    reasons: list[str] = []
    if not gate_a:
        reasons.append(
            f"(a) mean(Δ)={mean:+.4f} < threshold {delta_threshold:.4f} "
            f"(=max(0.010, noise_floor×2={noise_floor * 2:.4f})) NG"
        )
    if not gate_b:
        reasons.append(
            f"(b) Wilcoxon one-sided p={wp:.4f} > {p_threshold} NG"
        )
    if not gate_c:
        reasons.append(
            f"(c) Cohen's dz={dz:+.4f} < {dz_threshold} NG"
        )
    if not sanity_passed:
        # (d) は補助、主判定の否定理由には含めるが optional
        # paired re-eval (factcheck OFF) で summaries が空の場合は "no summaries" detail のみ
        if "reason" in sanity_detail:
            reasons.append(f"(d) factcheck sanity gate FAIL: {sanity_detail['reason']}")
        else:
            reasons.append(
                f"(d) factcheck sanity gate FAIL: "
                f"mt_mean={sanity_detail['mt_mean']:.4f} "
                f"ut_mean={sanity_detail['ut_mean']:.4f} "
                f"total_min={sanity_detail['total_min']} "
                "(補助、主判定への影響は (a)(b)(c) 次第)"
            )

    # 主判定 = (a) AND (b) AND (c)、(d) は補助なので reason には載せるが accept には不要
    accepted = gate_a and gate_b and gate_c
    return (accepted, reasons, metrics)


def collect_cycles(log_dir: Path, n_pairs: int) -> tuple[list, list, list, list, list, int]:
    """log_dir から各 cycle data 集約。

    戻り値: (on_summaries, on_failure_rates, on_scores, off_scores, deltas, missing)
    """
    on_summaries: list[dict] = []
    on_failure_rates: list[float] = []
    on_scores: list[float] = []
    off_scores: list[float] = []
    deltas: list[float] = []
    missing = 0

    for i in range(1, n_pairs + 1):
        # 後方互換 = test_on/test_off (A/A test 形式)
        # 新規 = cycle_a/cycle_b (g_paired_*_v2.sh 形式、ON=B / OFF=A)
        on_path = log_dir / f"test_on_{i}.log"
        off_path = log_dir / f"test_off_{i}.log"
        if not on_path.exists() or not off_path.exists():
            # fallback: cycle_b (variant=ON) / cycle_a (baseline=OFF)
            on_path = log_dir / f"cycle_b_{i}.log"
            off_path = log_dir / f"cycle_a_{i}.log"
            if not on_path.exists() or not off_path.exists():
                missing += 1
                continue
        on_text = on_path.read_text(encoding="utf-8", errors="replace")
        off_text = off_path.read_text(encoding="utf-8", errors="replace")

        on_sc = extract_score(on_text)
        off_sc = extract_score(off_text)
        if on_sc is None or off_sc is None:
            missing += 1
            continue

        on_scores.append(on_sc[0])
        off_scores.append(off_sc[0])
        deltas.append(on_sc[0] - off_sc[0])

        # factcheck / agenther は ON 側でのみ有意 (OFF は env unset)
        fc = extract_factcheck_summary(on_text)
        agh = extract_agenther_summary(on_text)
        if fc is not None:
            on_summaries.append(fc)
        if agh is not None:
            denom = agh["failed"] + agh["successful"]
            on_failure_rates.append(agh["failed"] / denom if denom > 0 else 0.0)

    return (on_summaries, on_failure_rates, on_scores, off_scores, deltas, missing)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("log_dir", type=Path, help="lab v22 logs directory")
    ap.add_argument(
        "--mode",
        choices=["paired", "aa", "smoke", "full"],
        default="paired",
        help="paired/smoke (default ACCEPT 判定 dz>=0.30, p<=0.10) "
        "/ full (dz>=0.40, p<=0.05) / aa (OFF×OFF noise floor 測定)",
    )
    ap.add_argument("--n-pairs", type=int, default=5, help="paired cycle 数 (default 5)")
    ap.add_argument(
        "--noise-floor",
        type=float,
        default=0.0,
        help="noise floor σ_Δ (A/A test で測定済の値、Δ threshold = max(0.010, σ×2))",
    )
    args = ap.parse_args()

    log_dir: Path = args.log_dir
    if not log_dir.is_dir():
        print(f"ERROR: log_dir not found: {log_dir}", file=sys.stderr)
        return 2

    on_summaries, on_failure_rates, on_scores, off_scores, deltas, missing = collect_cycles(
        log_dir, args.n_pairs
    )

    print(
        f"=== Lab v22 Metric Analyzer (mode={args.mode}, n_pairs={args.n_pairs}, "
        f"noise_floor={args.noise_floor:.4f}) ==="
    )
    print(
        f"  {'pair':>4} {'on_score':>8} {'off_score':>9} {'delta':>8} "
        f"{'fc_total':>8} {'matched':>7} {'conflict':>8} {'unknown':>7} {'mat/tot':>8}"
    )
    print(
        "  " + "-" * 4 + " " + "-" * 8 + " " + "-" * 9 + " " + "-" * 8 + " "
        + "-" * 8 + " " + "-" * 7 + " " + "-" * 8 + " " + "-" * 7 + " " + "-" * 8
    )
    for i, (on_s, off_s, d) in enumerate(zip(on_scores, off_scores, deltas), 1):
        fc_total = on_summaries[i - 1]["total"] if i - 1 < len(on_summaries) else 0
        matched = on_summaries[i - 1]["matched"] if i - 1 < len(on_summaries) else 0
        conflict = on_summaries[i - 1]["conflicting"] if i - 1 < len(on_summaries) else 0
        unknown = on_summaries[i - 1]["unknown"] if i - 1 < len(on_summaries) else 0
        mt = matched / fc_total if fc_total > 0 else 0.0
        print(
            f"  {i:>4} {on_s:>8.4f} {off_s:>9.4f} {d:>+8.4f} "
            f"{fc_total:>8} {matched:>7} {conflict:>8} {unknown:>7} {mt:>8.4f}"
        )
    print()

    if missing:
        print(f"WARNING: {missing} pair(s) incomplete", file=sys.stderr)

    if len(deltas) < 2:
        print("ERROR: need at least 2 complete pairs for analysis", file=sys.stderr)
        return 3

    if args.mode == "aa":
        # A/A モード: noise floor σ_Δ 計算のみ
        mean = sum(deltas) / len(deltas)
        var = sum((d - mean) ** 2 for d in deltas) / (len(deltas) - 1)
        sd = math.sqrt(var)
        print("=== A/A Test Noise Floor 計測 (両側 OFF、Δ_score sd σ) ===")
        print(f"  n_pairs:    {len(deltas)}")
        print(f"  mean(Δ):    {mean:+.6f}  (理想 = 0、両側 OFF なので)")
        print(f"  sd(Δ) = σ:  {sd:.6f}  ← Phase D/E の noise_floor として使用")
        print(f"  range(Δ):   [{min(deltas):+.4f}, {max(deltas):+.4f}]")
        print()
        print(f"  Phase D/E では: --noise-floor {sd:.6f} を引数で指定")
        print(f"  ACCEPT (a) 閾値は max(0.010, {sd:.4f}×2 = {sd * 2:.4f}) になる")
        return 0

    # paired/smoke/full モード: ACCEPT/REJECT 判定
    mode_for_judge = "full" if args.mode == "full" else "smoke"
    accepted, reasons, metrics = judge_accept_v22(
        deltas, on_summaries, args.noise_floor, mode=mode_for_judge
    )

    print("=== Paired Δscore Statistics (主軸) ===")
    print(f"  mean(Δ):           {metrics['delta_mean']:+.4f}")
    print(f"  delta_threshold:   {metrics['delta_threshold']:.4f}  "
          f"(=max(0.010, noise_floor×2={args.noise_floor * 2:.4f}))")
    print(f"  Cohen's dz:        {metrics['dz']:+.4f}  (threshold {metrics['dz_threshold']:.2f})")
    print(f"  Wilcoxon W+:       {metrics['wilcoxon_w_plus']:.2f}")
    print(f"  Wilcoxon p:        {metrics['wilcoxon_p']:.4f}  (threshold {metrics['p_threshold']:.2f})")
    print(f"  paired t-stat:     {metrics['ttest_t']:+.4f}")
    print(f"  paired t-test p:   {metrics['ttest_p']:.4f}  (副次、CLT 担保)")
    print()

    print("=== factcheck 補助 sanity gate (plan §3.1 (d)) ===")
    sd_detail = metrics["sanity_detail"]
    if sd_detail.get("reason"):
        print(f"  no factcheck summaries: {sd_detail['reason']}")
    else:
        print(f"  matched/total mean:   {sd_detail['mt_mean']:.4f} "
              f"(threshold >= {sd_detail['min_mt']}) {'PASS' if sd_detail['gate_a'] else 'FAIL'}")
        print(f"  unknown/total mean:   {sd_detail['ut_mean']:.4f} "
              f"(threshold <= {sd_detail['max_ut']}) {'PASS' if sd_detail['gate_b'] else 'FAIL'}")
        print(f"  total min:            {sd_detail['total_min']} "
              f"(threshold >= {sd_detail['min_total']}) {'PASS' if sd_detail['gate_c'] else 'FAIL'}")
        print(f"  sanity gate overall:  {'PASS' if sd_detail['passed'] else 'FAIL (補助、主判定に直接影響しない)'}")
    print()

    if on_failure_rates and on_summaries and len(on_summaries) >= 2:
        on_cmu_rates = [
            (s["conflicting"] + s["matched"] + s["unknown"]) / s["total"] if s["total"] > 0 else 0.0
            for s in on_summaries
        ]
        on_mt_rates = [s["matched"] / s["total"] if s["total"] > 0 else 0.0 for s in on_summaries]
        r_cmu = pearson_r(on_cmu_rates, on_failure_rates[: len(on_cmu_rates)])
        r_mt = pearson_r(on_mt_rates, on_failure_rates[: len(on_mt_rates)])
        print("=== Pearson r 診断ログ (n>=20 でのみ ACCEPT 影響、本 run では informational) ===")
        print(f"  r((conf+matched+unk)/total, fail_rate): {r_cmu:+.4f}  (Lab v20/v21 主軸、deterministic 注意)")
        print(f"  r(matched/total, fail_rate):            {r_mt:+.4f}  (Lab v22 候補軸、matched 軸単独)")
        print()

    print(f"=== ACCEPT 判定 (mode={mode_for_judge}、(a) AND (b) AND (c) 主条件) ===")
    print(f"  (a) mean(Δ) >= threshold:       {'PASS' if metrics['gate_a'] else 'FAIL'}  "
          f"(got {metrics['delta_mean']:+.4f}, threshold {metrics['delta_threshold']:.4f})")
    print(f"  (b) Wilcoxon p <= {metrics['p_threshold']:.2f}:           {'PASS' if metrics['gate_b'] else 'FAIL'}  "
          f"(got {metrics['wilcoxon_p']:.4f})")
    print(f"  (c) Cohen's dz >= {metrics['dz_threshold']:.2f}:          {'PASS' if metrics['gate_c'] else 'FAIL'}  "
          f"(got {metrics['dz']:+.4f})")
    print(f"  (d) factcheck sanity (補助):    {'PASS' if metrics['gate_d_sanity'] else 'FAIL'}")
    if accepted:
        print("  → ACCEPT (主条件 (a)(b)(c) 全 PASS)")
        return 0
    else:
        print("  → REJECT")
        for reason in reasons:
            print(f"    - {reason}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
