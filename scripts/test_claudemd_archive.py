#!/usr/bin/env python3
"""CLAUDE.md archive automation の単体 test (TDD strict Phase 1 Red、項目 240 候補)。

実行: `python3 scripts/test_claudemd_archive.py`
or:   `python3 -m unittest scripts.test_claudemd_archive -v`

Phase 1 Red: claudemd_archive.py 未実装で全 test fail (ImportError)。
Phase 2 Green: claudemd_archive.py 実装で全 8 test PASS。
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))


SAMPLE_CLAUDEMD = """# CLAUDE.md

## プロジェクト概要

`bonsai-agent` — Bonsai-8B エージェント。

## ハーネスパターン

**項目 1-219 は archive にアーカイブ**。

### 直近 5 項目

180. 短い 1 行 summary (項目 180 sample) — production code 変更ゼロ、1100 passed
181. 短い 1 行 summary (項目 181 sample) — Lab v15 ACCEPT、1101 passed (+1)

182. **長い verbose 項目 (項目 182 sample)** — `.claude/plan/sample.md` TDD strict Phase 1+2+3 atomic 完遂 (1 commit `abc1234`、+150/-3): 詳細な実装説明が続く。設計選択 = 案 A 採用、Phase 1 Red 4 test 全 fail 確証、Phase 2 Green で全 PASS、1101 → 1105 passed (+4 / 退行ゼロ / clippy 0 警告 / fmt clean)、API 完全 additive、production default OFF (env unset → 既存挙動 100% 互換)、Lab v22 で effectiveness 検証予定。副次 finding = 重要な observation がここに記述される。次=★★★ #1 Phase 4 G-5a smoke / ★ Pattern X 対応 plan 起票。

183. **🎉 別 verbose 項目 (項目 183 sample)** — TDD strict 完遂 (1 commit `def5678`、+200/-5): production agent_loop の event emit 配線、本 fix で event sourcing 完全性確立、conflicting=3 検出初成功 = 真効力初実証、1105 → 1109 passed (+4 / 退行ゼロ)、教訓 = TDD strict は agent 跨ぎで wiring gap が残るため Phase 4 smoke での実機検証必須。

184. **最新 verbose 項目 (項目 184 sample、★★ 直近、archive 対象外)** — Lab v20 起動 (PID 32568、ETA 2026-05-17)、Pearson r ACCEPT 基準 = 0.3、production code 変更ゼロ、1109 → 1110 passed (+1)。

## テストパターン

- MockLlmBackend
"""


class ParseItemsTest(unittest.TestCase):
    """項目番号 + body + byte_size の抽出。"""

    def test_parse_items_extracts_number_body_size(self) -> None:
        """項目 180-184 = 5 件抽出、各 (number, body, size) が取れる."""
        from claudemd_archive import parse_items  # type: ignore[import-not-found]

        items = parse_items(SAMPLE_CLAUDEMD)
        nums = [it.number for it in items]
        self.assertEqual(nums, [180, 181, 182, 183, 184])
        # 項目 182 は verbose (>500 byte)
        item_182 = next(it for it in items if it.number == 182)
        self.assertGreater(item_182.size, 500)
        # 項目 180 は短い 1 行 (<500 byte)
        item_180 = next(it for it in items if it.number == 180)
        self.assertLess(item_180.size, 500)

    def test_parse_items_ignores_non_item_lines(self) -> None:
        """section header / project 概要等は無視、項目行のみ捕捉."""
        from claudemd_archive import parse_items  # type: ignore[import-not-found]

        items = parse_items(SAMPLE_CLAUDEMD)
        # 項目行のみ、5 件
        self.assertEqual(len(items), 5)
        # body に section header が含まれない
        for it in items:
            self.assertNotIn("## プロジェクト概要", it.body)


class BloatIdentificationTest(unittest.TestCase):
    """archive 対象 (verbose かつ古い項目) の特定。"""

    def test_keep_recent_5_skips_last_5_items(self) -> None:
        """keep_recent=5 + 5 項目 = 全 keep、archive 対象ゼロ."""
        from claudemd_archive import identify_bloat_candidates, parse_items  # type: ignore[import-not-found]

        items = parse_items(SAMPLE_CLAUDEMD)
        candidates = identify_bloat_candidates(items, keep_recent=5)
        self.assertEqual(candidates, [])

    def test_verbose_threshold_500_bytes(self) -> None:
        """keep_recent=2 で項目 180-182 が archive 候補、ただし short (180/181) は除外、verbose のみ."""
        from claudemd_archive import identify_bloat_candidates, parse_items  # type: ignore[import-not-found]

        items = parse_items(SAMPLE_CLAUDEMD)
        # keep_recent=2 = 直近 183, 184 を保持、180/181/182 が候補
        # 但し short threshold で 180/181 は除外、182 のみ
        candidates = identify_bloat_candidates(items, keep_recent=2, verbose_threshold=500)
        self.assertEqual(candidates, [182])


class SummaryGenerationTest(unittest.TestCase):
    """verbose body から 1 行 summary を抽出。"""

    def test_one_line_summary_extracts_title_emphasis(self) -> None:
        """項目 182 の最初の `**...**` 強調を抽出."""
        from claudemd_archive import generate_one_line_summary, parse_items  # type: ignore[import-not-found]

        items = parse_items(SAMPLE_CLAUDEMD)
        item_182 = next(it for it in items if it.number == 182)
        summary = generate_one_line_summary(item_182.body)
        # 最初の **強調** タイトルが含まれる
        self.assertIn("長い verbose 項目", summary)
        # summary は 1 行 (改行なし)
        self.assertNotIn("\n", summary)
        # summary は元 body より短い
        self.assertLess(len(summary), item_182.size)

    def test_one_line_summary_includes_test_count_delta(self) -> None:
        """summary に test 数 delta `1101 → 1105 passed (+4)` パターンが含まれる."""
        from claudemd_archive import generate_one_line_summary, parse_items  # type: ignore[import-not-found]

        items = parse_items(SAMPLE_CLAUDEMD)
        item_182 = next(it for it in items if it.number == 182)
        summary = generate_one_line_summary(item_182.body)
        # 重要 metadata = test 数遷移
        self.assertTrue(
            "1101 → 1105" in summary or "1105 passed" in summary or "+4" in summary,
            f"test 数 delta が summary に含まれるべき、got={summary}",
        )


class IntegrationTest(unittest.TestCase):
    """end-to-end mode (check / dry-run / apply)。"""

    def test_dry_run_produces_diff_without_modification(self) -> None:
        """dry-run は file modification ゼロ、stdout に diff 出力."""
        import tempfile

        from claudemd_archive import run_dry_run  # type: ignore[import-not-found]

        with tempfile.TemporaryDirectory() as tmp:
            tmpdir = Path(tmp)
            claudemd = tmpdir / "CLAUDE.md"
            archive = tmpdir / "archive.md"
            claudemd.write_text(SAMPLE_CLAUDEMD, encoding="utf-8")
            archive.write_text("# archive header\n\n", encoding="utf-8")

            original_claudemd_bytes = claudemd.read_bytes()
            original_archive_bytes = archive.read_bytes()

            diff_text = run_dry_run(claudemd, archive, keep_recent=2)

            # file は modify されない
            self.assertEqual(claudemd.read_bytes(), original_claudemd_bytes)
            self.assertEqual(archive.read_bytes(), original_archive_bytes)
            # diff 出力に項目 182 (archive 候補) が含まれる
            self.assertIn("182", diff_text)

    def test_apply_modifies_claudemd_and_archive(self) -> None:
        """apply mode は CLAUDE.md + archive 両 file 更新."""
        import tempfile

        from claudemd_archive import run_apply  # type: ignore[import-not-found]

        with tempfile.TemporaryDirectory() as tmp:
            tmpdir = Path(tmp)
            claudemd = tmpdir / "CLAUDE.md"
            archive = tmpdir / "archive.md"
            claudemd.write_text(SAMPLE_CLAUDEMD, encoding="utf-8")
            archive.write_text("# archive header\n\n", encoding="utf-8")

            archived_nums = run_apply(claudemd, archive, keep_recent=2, do_commit=False)

            # 項目 182 が archive に追加されている
            archive_content = archive.read_text(encoding="utf-8")
            self.assertIn("182", archive_content)
            self.assertIn("長い verbose 項目", archive_content)

            # CLAUDE.md は項目 182 の verbose body が短縮されている
            claudemd_content = claudemd.read_text(encoding="utf-8")
            self.assertLess(len(claudemd_content), len(SAMPLE_CLAUDEMD))

            # 戻り値 = archived 番号 list
            self.assertEqual(archived_nums, [182])


if __name__ == "__main__":
    unittest.main()
