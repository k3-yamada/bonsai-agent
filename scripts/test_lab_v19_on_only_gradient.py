"""Phase 1 Red — Lab v19 ON-only bucket gradient + stability analysis tests.

起点: `.claude/plan/lab-v19-on-only-bucket-gradient-analysis.md` §3 Phase 1
標準ライブラリ unittest (pytest 不在環境対応、plan §2.4 stdlib-only と整合)。
Phase 2 Green で `lab_v19_on_only_gradient.py` 実装後 全 PASS 期待。

Phase 1 Red signal: module 不在で setUpClass で skipTest → 全 8 件 skip。
Phase 2 Green: skip 解除で 8 件 PASS。
"""
from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).parent
MODULE_PATH = SCRIPT_DIR / "lab_v19_on_only_gradient.py"


def _load_module():
    spec = importlib.util.spec_from_file_location(
        "lab_v19_on_only_gradient", MODULE_PATH
    )
    if spec is None or spec.loader is None:
        raise ImportError("module spec load failed")
    loaded = importlib.util.module_from_spec(spec)
    sys.modules["lab_v19_on_only_gradient"] = loaded
    spec.loader.exec_module(loaded)
    return loaded


class GradientAnalysisTests(unittest.TestCase):
    """Phase 1 Red: module 不在で全 test skip。Phase 2 Green で 8 件 PASS。"""

    @classmethod
    def setUpClass(cls) -> None:
        if not MODULE_PATH.exists():
            raise unittest.SkipTest(
                "Phase 2 Green まで未実装 (lab_v19_on_only_gradient.py 不在)"
            )
        cls.mod = _load_module()

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.tmp_path = Path(self.tmp.name)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    # 1: parse — bucket 行の基本抽出
    def test_parse_bucket_line_basic(self) -> None:
        log = self.tmp_path / "test.log"
        log.write_text(
            "[INFO][lab.frontier] Frontier metric (baseline):\n"
            "[INFO][lab.frontier]   bucket 0 [0, 2048): 0.8944\n"
            "[INFO][lab.frontier]   bucket 1 [2048, 4096): 0.7296\n",
            encoding="utf-8",
        )
        pairs = self.mod.parse_bucket_lines(log)
        self.assertEqual(pairs, [(0, 0.8944), (1, 0.7296)])

    # 2: parse — inject 行の除外
    def test_parse_bucket_skips_inject_lines(self) -> None:
        log = self.tmp_path / "test.log"
        log.write_text(
            "[INFO][lab.frontier]   bucket 0 [0, 2048): 0.5\n"
            "[INFO][lab.frontier]   inject: (no T6 tasks populated)\n"
            "[INFO][lab.frontier]   bucket 1 [2048, 4096): 0.6\n",
            encoding="utf-8",
        )
        pairs = self.mod.parse_bucket_lines(log)
        self.assertIn((0, 0.5), pairs)
        self.assertIn((1, 0.6), pairs)
        self.assertEqual(len(pairs), 2)

    # 3: aggregate — 複数 log file から bucket-wise 集約
    def test_aggregate_buckets_per_cycle(self) -> None:
        for i in range(1, 4):
            (self.tmp_path / f"test_on_{i}.log").write_text(
                f"[INFO][lab.frontier]   bucket 0 [0, 2048): 0.{i}\n"
                f"[INFO][lab.frontier]   bucket 1 [2048, 4096): 0.{i + 1}\n",
                encoding="utf-8",
            )
        log_paths = sorted(self.tmp_path.glob("test_on_*.log"))
        agg = self.mod.aggregate_buckets(log_paths)
        self.assertEqual(agg[0], [0.1, 0.2, 0.3])
        self.assertEqual(agg[1], [0.2, 0.3, 0.4])
        self.assertEqual(agg.get(2, []), [])

    # 4: gradient — 基本計算
    def test_compute_gradient_basic(self) -> None:
        agg = {0: [0.9, 0.9, 0.9, 0.9, 0.9], 1: [0.7, 0.7, 0.7, 0.7, 0.7]}
        grad = self.mod.compute_gradient(agg, src=0, dst=1, min_populated=4)
        self.assertIsNotNone(grad)
        self.assertAlmostEqual(grad, -0.2, places=9)

    # 5: gradient — populated 不足で None
    def test_gradient_requires_min_populated(self) -> None:
        agg = {1: [0.7] * 5, 2: [0.1, 0.1, 0.1]}
        grad = self.mod.compute_gradient(agg, src=1, dst=2, min_populated=4)
        self.assertIsNone(grad)

    # 6: judge — ACCEPT (gradient ≤ threshold)
    def test_accept_when_gradient_below_threshold(self) -> None:
        agg = {0: [0.9] * 5, 1: [0.7] * 5}
        verdict = self.mod.judge_accept(
            agg, src=0, dst=1, gradient_threshold=-0.10, min_populated=4
        )
        self.assertEqual(verdict, "ACCEPT")

    # 7: judge — REJECT (gradient > threshold)
    def test_reject_when_gradient_above_threshold(self) -> None:
        agg = {0: [0.9] * 5, 1: [0.85] * 5}
        verdict = self.mod.judge_accept(
            agg, src=0, dst=1, gradient_threshold=-0.10, min_populated=4
        )
        self.assertEqual(verdict, "REJECT")

    # 8: parse — frontier 行ゼロで empty list
    def test_handles_no_frontier_lines(self) -> None:
        log = self.tmp_path / "empty.log"
        log.write_text(
            "nothing relevant here\n[INFO][other.target] foo\n",
            encoding="utf-8",
        )
        pairs = self.mod.parse_bucket_lines(log)
        self.assertEqual(pairs, [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
