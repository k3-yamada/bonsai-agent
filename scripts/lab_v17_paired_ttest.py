#!/usr/bin/env python3
"""Lab v17 — ERL Heuristics Pool effectiveness paired t-test (項目 214/215).

Plan: .claude/plan/lab-v17-erl-effectiveness.md §4.6
入力: scripts/lab_v17_paired.sh が出力する `${LOG_DIR}/test_{on,off}_{1..5}.log`
出力: stdout に各 pair の score 表 + paired t-test (one-sided) + ACCEPT/REJECT 判定

依存: 標準ライブラリのみ (scipy 不使用、df=4 t-table を線形補間)。
"""

from __future__ import annotations

import argparse
import math
import re
import sys
from pathlib import Path

SCORE_RE = re.compile(
    r"\[lab\]\s*ベースライン:\s*score=([0-9.]+)\s+"
    r"pass@k=([0-9.]+)\s+pass_consec=([0-9.]+)\s+\(([0-9.]+)s\)"
)


def extract_score(log_path: Path) -> tuple[float, float, float, float] | None:
    """log file から (score, pass@k, pass_consec, duration_s) を抽出。

    cycle 内に複数 [lab] ベースライン行があれば最後を採用 (Lab cycle 1 回 = 1 baseline)。
    """
    if not log_path.exists():
        return None
    last: tuple[float, float, float, float] | None = None
    with log_path.open("r", encoding="utf-8", errors="replace") as f:
        for line in f:
            m = SCORE_RE.search(line)
            if m:
                last = (
                    float(m.group(1)),
                    float(m.group(2)),
                    float(m.group(3)),
                    float(m.group(4)),
                )
    return last


def paired_t_stat(deltas: list[float]) -> tuple[float, float, float]:
    """paired t-test (one-sided H1: mean > 0)、df = n-1。

    戻り値 (mean, t_stat, p_one_sided)。
    p は df=4 の t-table を線形補間で近似 (n=5 想定の conservative 判定用)。
    n != 5 の場合は math.erf による正規分布近似 (大標本近似、df 不問)。
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

    if n == 5:
        # df=4, one-sided p: t-table linear interp.
        # t=1.533 → p=0.10, t=2.132 → p=0.05, t=2.776 → p=0.025, t=3.747 → p=0.01
        if t <= 0:
            p = 0.5 + min(0.5, abs(t) * 0.1)
        elif t < 1.533:
            p = 0.5 - (t / 1.533) * 0.4  # 0.5 → 0.1
        elif t < 2.132:
            p = 0.10 - ((t - 1.533) / (2.132 - 1.533)) * 0.05
        elif t < 2.776:
            p = 0.05 - ((t - 2.132) / (2.776 - 2.132)) * 0.025
        elif t < 3.747:
            p = 0.025 - ((t - 2.776) / (3.747 - 2.776)) * 0.015
        else:
            p = 0.01
    else:
        # 大標本 z 近似 (df>=10 で誤差 <5%、n=5 では使わず conservative t-table を優先)
        p = 0.5 * (1.0 - math.erf(t / math.sqrt(2)))

    return (mean, t, p)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("log_dir", type=Path, help="lab v17 logs directory")
    ap.add_argument(
        "--accept-delta",
        type=float,
        default=0.015,
        help="ACCEPT threshold for mean delta (default 0.015)",
    )
    ap.add_argument(
        "--accept-p",
        type=float,
        default=0.10,
        help="ACCEPT threshold for one-sided p-value (default 0.10)",
    )
    ap.add_argument(
        "--n-pairs",
        type=int,
        default=5,
        help="number of paired test cycles (default 5)",
    )
    args = ap.parse_args()

    log_dir: Path = args.log_dir
    if not log_dir.is_dir():
        print(f"ERROR: log_dir not found: {log_dir}", file=sys.stderr)
        return 2

    # Warm-up cycle 確認 (informational only、ACCEPT 判定には不参加)
    print("=== Warm-up Phase (informational) ===")
    for i in (1, 2):
        rec = extract_score(log_dir / f"warmup_{i}.log")
        if rec is None:
            print(f"  warmup_{i}: MISSING")
        else:
            score, pak, pck, dur = rec
            print(
                f"  warmup_{i}: score={score:.4f} pass@k={pak:.4f} "
                f"pass_consec={pck:.4f} duration={dur:.1f}s"
            )
    print()

    # Test paired cycles
    print(f"=== Test Phase (n_pairs={args.n_pairs}) ===")
    print(f"  {'pair':>4} {'on_score':>10} {'off_score':>10} {'delta':>8}")
    print(f"  {'-' * 4} {'-' * 10} {'-' * 10} {'-' * 8}")
    deltas: list[float] = []
    on_scores: list[float] = []
    off_scores: list[float] = []
    missing = 0
    for i in range(1, args.n_pairs + 1):
        on_rec = extract_score(log_dir / f"test_on_{i}.log")
        off_rec = extract_score(log_dir / f"test_off_{i}.log")
        if on_rec is None or off_rec is None:
            missing += 1
            on_str = f"{on_rec[0]:.4f}" if on_rec else "MISSING"
            off_str = f"{off_rec[0]:.4f}" if off_rec else "MISSING"
            print(f"  {i:>4} {on_str:>10} {off_str:>10} {'?':>8}")
            continue
        on, off = on_rec[0], off_rec[0]
        d = on - off
        deltas.append(d)
        on_scores.append(on)
        off_scores.append(off)
        print(f"  {i:>4} {on:>10.4f} {off:>10.4f} {d:>+8.4f}")
    print()

    if missing:
        print(
            f"WARNING: {missing} pair(s) incomplete — t-test on partial data",
            file=sys.stderr,
        )

    if len(deltas) < 2:
        print("ERROR: need at least 2 complete pairs for t-test", file=sys.stderr)
        return 3

    mean, t, p = paired_t_stat(deltas)
    on_mean = sum(on_scores) / len(on_scores)
    off_mean = sum(off_scores) / len(off_scores)

    print("=== Summary ===")
    print(f"  ON  mean: {on_mean:.4f}  (n={len(on_scores)})")
    print(f"  OFF mean: {off_mean:.4f}  (n={len(off_scores)})")
    print(f"  paired delta mean: {mean:+.4f}")
    print(f"  paired t-stat:     {t:+.4f}  (df={len(deltas) - 1})")
    print(f"  one-sided p:       {p:.4f}")
    print()

    print("=== ACCEPT 判定 ===")
    cond_a = mean >= args.accept_delta
    cond_b = p < args.accept_p
    print(f"  (a) mean delta >= {args.accept_delta}: {cond_a}  (got {mean:+.4f})")
    print(f"  (b) p < {args.accept_p}:                {cond_b}  (got {p:.4f})")
    if cond_a and cond_b:
        print("  → ACCEPT (H_ERL 採用 = production default ON 確定)")
        return 0
    else:
        print("  → REJECT (H_ERL 棄却 = `BONSAI_ERL_DISABLED=1` defaults 化候補)")
        return 1


if __name__ == "__main__":
    sys.exit(main())
