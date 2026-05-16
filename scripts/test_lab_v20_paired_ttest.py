#!/usr/bin/env python3
"""Lab v20 ttest analyzer の単体 test (TDD strict Phase 1 Red、項目 232 候補)。

実行: `python3 -m unittest scripts.test_lab_v20_paired_ttest -v`
or:   `python3 scripts/test_lab_v20_paired_ttest.py`

Phase 1 Red: lab_v20_paired_ttest.py 未実装で全 test fail (ImportError)。
Phase 2 Green: lab_v20_paired_ttest.py 実装で全 test PASS (5 件)。
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))


class PearsonRTest(unittest.TestCase):
    """Pearson r 純関数の 3 件 (perfect positive / zero variance / negative)。"""

    def test_perfect_positive_correlation(self) -> None:
        """r([1,2,3], [2,4,6]) == 1.0 (line through origin)."""
        from lab_v20_paired_ttest import pearson_r  # type: ignore[import-not-found]

        r = pearson_r([1.0, 2.0, 3.0], [2.0, 4.0, 6.0])
        self.assertAlmostEqual(r, 1.0, places=6)

    def test_zero_variance_returns_zero(self) -> None:
        """zero variance (xs 全同値) で 0.0 (NaN 回避、Lab v17 同 pattern)."""
        from lab_v20_paired_ttest import pearson_r  # type: ignore[import-not-found]

        r = pearson_r([1.0, 1.0, 1.0], [1.0, 2.0, 3.0])
        self.assertEqual(r, 0.0)

    def test_perfect_negative_correlation(self) -> None:
        """r([1,2,3], [3,2,1]) == -1.0."""
        from lab_v20_paired_ttest import pearson_r  # type: ignore[import-not-found]

        r = pearson_r([1.0, 2.0, 3.0], [3.0, 2.0, 1.0])
        self.assertAlmostEqual(r, -1.0, places=6)


class FactCheckLogParsingTest(unittest.TestCase):
    """`[INFO][lab.factcheck] FactCheck post-Lab:` log 行の regex 抽出。"""

    def test_extract_factcheck_summary_parses_log(self) -> None:
        """G-6b 実機 log と完全同型 `total=5 matched=0 unknown=2 conflicting=3 mean_path_len=0.00`."""
        from lab_v20_paired_ttest import extract_factcheck_summary  # type: ignore[import-not-found]

        log_text = (
            "[INFO][checkpoint] タスク開始時CP作成 id=1\n"
            "[lab] ベースライン: score=0.6828 pass@k=0.8000 pass_consec=0.7667 (2226.6s)\n"
            "[INFO][lab.factcheck] FactCheck post-Lab: total=5 matched=0 unknown=2 conflicting=3 mean_path_len=0.00\n"
        )
        rec = extract_factcheck_summary(log_text)
        self.assertIsNotNone(rec)
        assert rec is not None
        self.assertEqual(rec["total"], 5)
        self.assertEqual(rec["matched"], 0)
        self.assertEqual(rec["unknown"], 2)
        self.assertEqual(rec["conflicting"], 3)
        self.assertAlmostEqual(rec["mean_path_len"], 0.0, places=6)


class AcceptJudgmentTest(unittest.TestCase):
    """plan §2 ACCEPT 基準 (a Pearson r >= 0.3 AND b ON 全 5 件 total >= 1)。"""

    def test_accept_when_pearson_above_threshold_and_all_fired(self) -> None:
        """r=0.5、ON 5 cycle 全 total>=1 → ACCEPT (True)."""
        from lab_v20_paired_ttest import judge_accept  # type: ignore[import-not-found]

        on_summaries = [
            {"total": 5, "matched": 0, "unknown": 2, "conflicting": 3},
            {"total": 7, "matched": 1, "unknown": 3, "conflicting": 3},
            {"total": 4, "matched": 0, "unknown": 1, "conflicting": 3},
            {"total": 6, "matched": 1, "unknown": 2, "conflicting": 3},
            {"total": 8, "matched": 0, "unknown": 4, "conflicting": 4},
        ]
        pearson = 0.5
        accepted, reasons = judge_accept(on_summaries, pearson, accept_r=0.3)
        self.assertTrue(accepted, f"ACCEPT 期待だが REJECT、理由={reasons}")

    def test_reject_when_pearson_below_threshold(self) -> None:
        """r=0.2 (< 0.3)、ON 5 cycle 全 total>=1 → REJECT (Pearson r 不足)."""
        from lab_v20_paired_ttest import judge_accept  # type: ignore[import-not-found]

        on_summaries = [
            {"total": 5, "matched": 0, "unknown": 2, "conflicting": 3},
        ] * 5
        pearson = 0.2
        accepted, reasons = judge_accept(on_summaries, pearson, accept_r=0.3)
        self.assertFalse(accepted)
        self.assertTrue(any("Pearson" in r or "相関" in r for r in reasons))


if __name__ == "__main__":
    unittest.main()
