#!/usr/bin/env python3
"""Lab v19 — ON-only bucket gradient + stability analysis (案 A 再設計)。

起点: `.claude/plan/lab-v19-on-only-bucket-gradient-analysis.md`
旧 plan の二重前提崩壊 (OFF emit ゼロ + bucket 3 サンプル不足) を回避し、
ON 群単独の bucket gradient + cycle stability を測定する。

目的: 第 6 軸 = context-length axis baseline の data-backed 公式記録。
ACCEPT 判定: bucket 0→1 gradient ≤ -0.10 + 両 bucket 4+ cycle populated +
bucket 1 cycle std < 0.10 → 第 6 軸 baseline 確立。

Lab v19 既収集データ (`./lab-v19-logs/test_on_{1..5}.log`) を read-only で解析、
production code 変更ゼロ、Python 標準ライブラリのみ使用。
"""
from __future__ import annotations

import argparse
import re
import statistics
import sys
from pathlib import Path
from typing import Optional

# `[INFO][lab.frontier]   bucket {idx} [{lo}, {hi}): {score}` を parse。
# `inject:` 行は別系統 (frontier_inject_scores) のため regex で自動的に skip。
# experiment_log.rs::emit_frontier_log の format に依存 (本 plan §2.4 設計選択 A)。
BUCKET_RE = re.compile(
    r"\[INFO\]\[lab\.frontier\]\s+bucket\s+(\d+)\s+"
    r"\[(\d+),\s*(\d+|∞)\):\s+([0-9.]+)"
)

BUCKET_RANGES: dict[int, str] = {
    0: "[0, 2048)",
    1: "[2048, 4096)",
    2: "[4096, 8192)",
    3: "[8192, ∞)",
}


def parse_bucket_lines(log_path: Path) -> list[tuple[int, float]]:
    """1 log file から (bucket_idx, score) tuple list を抽出。

    `inject:` 行は frontier_inject_scores 用 metric なので除外
    (regex が bucket 数値を要求するため自動的に skip される)。
    log 不在時は空 list を返す (caller 側で aggregate 時に欠損として扱う)。
    """
    if not log_path.exists():
        return []
    out: list[tuple[int, float]] = []
    with log_path.open("r", encoding="utf-8", errors="replace") as f:
        for line in f:
            m = BUCKET_RE.search(line)
            if m:
                out.append((int(m.group(1)), float(m.group(4))))
    return out


def aggregate_buckets(log_paths: list[Path]) -> dict[int, list[float]]:
    """複数 log file から bucket-wise score list 集約。

    返り値の dict は欠損 bucket を持たないため caller が `.get(k, [])` で fallback 必須。
    """
    agg: dict[int, list[float]] = {}
    for path in log_paths:
        for bucket_idx, score in parse_bucket_lines(path):
            agg.setdefault(bucket_idx, []).append(score)
    return agg


def compute_gradient(
    agg: dict[int, list[float]],
    src: int,
    dst: int,
    min_populated: int,
) -> Optional[float]:
    """bucket src→dst の mean 差分 = gradient (degradation curve、負値で degrade)。

    両 bucket の populated cycle 数が `min_populated` 未満なら計算不能で None を返す。
    None は caller 側で "insufficient data" として扱う (本 plan §3 Phase 1 t5)。
    """
    src_scores = agg.get(src, [])
    dst_scores = agg.get(dst, [])
    if len(src_scores) < min_populated or len(dst_scores) < min_populated:
        return None
    return statistics.mean(dst_scores) - statistics.mean(src_scores)


def judge_accept(
    agg: dict[int, list[float]],
    src: int,
    dst: int,
    gradient_threshold: float,
    min_populated: int,
) -> str:
    """ACCEPT/REJECT/INSUFFICIENT 3 値判定。

    ACCEPT 条件: gradient が threshold 以下 (より degrade) かつ両 bucket robust populated。
    INSUFFICIENT は compute_gradient が None (data 不足) の場合。
    """
    grad = compute_gradient(agg, src=src, dst=dst, min_populated=min_populated)
    if grad is None:
        return "INSUFFICIENT"
    return "ACCEPT" if grad <= gradient_threshold else "REJECT"


def _stdev_or_zero(scores: list[float]) -> float:
    """n>=2 で stdev、それ未満は 0.0 (deterministic な bucket 0 想定)。"""
    return statistics.stdev(scores) if len(scores) >= 2 else 0.0


def format_report(
    agg: dict[int, list[float]],
    log_paths: list[Path],
    gradient_threshold: float,
    min_populated: int,
) -> str:
    """plan §2.2 出力フォーマットに沿う report を生成。

    `format_report` は副作用ゼロ (stdout 書込みは main() 側)、test 容易化のため。
    """
    lines: list[str] = []
    lines.append("=== Lab v19 ON-only Bucket Gradient + Stability Analysis ===")
    lines.append(f"Source: {len(log_paths)} ON cycle log files")
    lines.append("")
    lines.append("=== Bucket Statistics (ON 群単独) ===")
    lines.append("bucket  range            n  mean    std     populated")
    cycles_total = len(log_paths)
    for k in range(4):
        scores = agg.get(k, [])
        n = len(scores)
        if n == 0:
            row = (
                f"{k}       {BUCKET_RANGES[k]:<16} 0  -       -       "
                f"0/{cycles_total} (insufficient)"
            )
        else:
            mean_v = statistics.mean(scores)
            std_v = _stdev_or_zero(scores)
            std_disp = f"{std_v:.4f}" if n >= 2 else "N/A"
            tag = " ★ deterministic" if n >= 2 and std_v == 0.0 else ""
            row = (
                f"{k}       {BUCKET_RANGES[k]:<16} {n}  "
                f"{mean_v:.4f}  {std_disp}  {n}/{cycles_total}{tag}"
            )
        lines.append(row)

    lines.append("")
    lines.append("=== Degradation Gradient ===")
    for src, dst in [(0, 1), (1, 2), (2, 3)]:
        grad = compute_gradient(agg, src=src, dst=dst, min_populated=min_populated)
        if grad is None:
            note = "insufficient data"
        elif grad <= gradient_threshold:
            note = f"{gradient_threshold:+.2f} threshold で ACCEPT 確証 ✓"
        else:
            note = f"{gradient_threshold:+.2f} threshold より上 (REJECT 寄り)"
        grad_disp = f"{grad:+.4f}" if grad is not None else "N/A"
        lines.append(f"bucket {src} → {dst}: Δ = {grad_disp}  ({note})")

    lines.append("")
    lines.append("=== Stability per bucket (cycle 間) ===")
    for k in range(4):
        scores = agg.get(k, [])
        if len(scores) < 2:
            lines.append(f"bucket {k} std: N/A (cycle 数 {len(scores)} < 2)")
            continue
        std_v = _stdev_or_zero(scores)
        if std_v == 0.0:
            note = "deterministic、stability metric として trivial"
        elif std_v < 0.10:
            note = "stability 軸として actionable"
        else:
            note = "high noise、informational のみ"
        lines.append(f"bucket {k} std: {std_v:.4f}  ({note})")

    lines.append("")
    lines.append("=== ACCEPT 判定 (第 6 軸 = context-length axis baseline) ===")
    grad_01 = compute_gradient(agg, src=0, dst=1, min_populated=min_populated)
    bucket_0_n = len(agg.get(0, []))
    bucket_1_n = len(agg.get(1, []))
    bucket_1_std = _stdev_or_zero(agg.get(1, []))
    a1_pass = grad_01 is not None and grad_01 <= gradient_threshold
    a2_pass = bucket_0_n >= min_populated and bucket_1_n >= min_populated
    a3_pass = bucket_1_std < 0.10

    grad_disp = f"{grad_01:+.4f}" if grad_01 is not None else "N/A"
    lines.append(
        f"  (a1) bucket 0 → 1 gradient ≤ {gradient_threshold:+.2f}: "
        f"{'PASS' if a1_pass else 'FAIL'} ({grad_disp})"
    )
    lines.append(
        f"  (a2) bucket 0 と bucket 1 が両方 {min_populated}+ populated: "
        f"{'PASS' if a2_pass else 'FAIL'} "
        f"(bucket0={bucket_0_n}, bucket1={bucket_1_n})"
    )
    lines.append(
        f"  (a3) bucket 1 cycle std < 0.10: "
        f"{'PASS' if a3_pass else 'FAIL'} ({bucket_1_std:.4f})"
    )
    if a1_pass and a2_pass and a3_pass:
        lines.append(
            "  → ACCEPT (第 6 軸 baseline = context-length axis confirmed in [0, 4K))"
        )
    elif not a2_pass:
        lines.append("  → INSUFFICIENT (bucket data 不足、Lab 再 run 必要)")
    else:
        lines.append("  → REJECT (degrade 観測されず、第 6 軸の効力否定)")

    lines.append("")
    lines.append("NOTE: bucket 2/3 は LADDER + extended benchmark で再 run 必要 (別 plan)")
    return "\n".join(lines) + "\n"


def main(argv: Optional[list[str]] = None) -> int:
    """CLI entry: log_dir 配下の test_on_*.log を解析、verdict を return code で示す。

    exit code: 0=ACCEPT, 1=REJECT, 2=INSUFFICIENT (data 不足)。
    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "log_dir",
        nargs="?",
        default="./lab-v19-logs",
        help="Lab v19 ON cycle log directory (default: ./lab-v19-logs)",
    )
    parser.add_argument(
        "--gradient-threshold",
        type=float,
        default=-0.10,
        help="ACCEPT 基準 gradient 上限 (default -0.10)",
    )
    parser.add_argument(
        "--min-populated-cycles",
        type=int,
        default=4,
        help="bucket k の min populated cycle 数 (default 4)",
    )
    args = parser.parse_args(argv)

    log_dir = Path(args.log_dir)
    if not log_dir.exists():
        print(f"ERROR: log directory 不在: {log_dir}", file=sys.stderr)
        return 2
    log_paths = sorted(log_dir.glob("test_on_*.log"))
    if not log_paths:
        print(f"ERROR: test_on_*.log が {log_dir} に見つからない", file=sys.stderr)
        return 2

    agg = aggregate_buckets(log_paths)
    if not agg:
        print(
            f"ERROR: log 内に [INFO][lab.frontier] bucket 行が無い "
            f"({len(log_paths)} files)",
            file=sys.stderr,
        )
        return 2

    report = format_report(
        agg,
        log_paths=log_paths,
        gradient_threshold=args.gradient_threshold,
        min_populated=args.min_populated_cycles,
    )
    print(report)

    verdict = judge_accept(
        agg,
        src=0,
        dst=1,
        gradient_threshold=args.gradient_threshold,
        min_populated=args.min_populated_cycles,
    )
    if verdict == "ACCEPT":
        return 0
    if verdict == "REJECT":
        return 1
    return 2


if __name__ == "__main__":
    sys.exit(main())
