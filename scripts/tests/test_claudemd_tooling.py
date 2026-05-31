#!/usr/bin/env python3
"""Regression tests for CLAUDE.md curation tooling.

対象 (2026-05-31 session 修正の regression guard):
1. scripts/claudemd_archive.py の ITEM_START_RE (CLAUDE.md bullet 形式追随)
   - Item 255 format drift で項目数=0 になった bug の再発防止
2. scripts/drift/docs_sync.py の check_recent_items_section_count (Z-3 第 3 軸)
   - header N ↔ section 実数整合の検出

実行: python3 -m unittest scripts.tests.test_claudemd_tooling
      または python3 scripts/tests/test_claudemd_tooling.py
stdlib only (pytest 不要)。
"""

import sys
import tempfile
import unittest
from pathlib import Path

# scripts/ を import path に追加 (scripts/tests/ から見て親)
SCRIPTS_DIR = Path(__file__).resolve().parents[1]
PROJECT_ROOT = SCRIPTS_DIR.parent
sys.path.insert(0, str(SCRIPTS_DIR))
sys.path.insert(0, str(SCRIPTS_DIR / "drift"))

import claudemd_archive  # noqa: E402
import docs_sync  # noqa: E402

CLAUDE_MD = PROJECT_ROOT / "CLAUDE.md"

# UTF-8 で verbose_threshold (500 byte) を確実に超える synthetic body
_LONG_BODY = "これは synthetic 本文で UTF-8 換算 500 bytes 超を意図的に達成する verbose entry " * 8


def _make_section(numbers, header_n=5):
    """`### 直近 {header_n} 項目` + 各 number の bullet を含む synthetic CLAUDE.md text."""
    lines = ["# CLAUDE.md", "## ハーネスパターン", f"### 直近 {header_n} 項目", ""]
    for n in numbers:
        lines.append(f"- **{n}**: 🎉 {_LONG_BODY}")
    lines += ["", "## End"]
    return "\n".join(lines) + "\n"


class ClaudemdArchiveRegexTest(unittest.TestCase):
    """claudemd_archive.py ITEM_START_RE format 追随 (Item 255 drift fix)."""

    def test_parse_items_matches_bullet_format(self):
        text = _make_section([264, 265, 266, 267, 268])
        items = claudemd_archive.parse_items(text)
        self.assertEqual(len(items), 5, "bullet 形式 5 項目を検出すべき")
        self.assertEqual([it.number for it in items], [264, 265, 266, 267, 268])

    def test_parse_items_does_not_match_old_numbered_format(self):
        # 旧 archive 形式 `247. body` は CLAUDE.md では使われない → 検出されないことを確認
        old_format = "## H\n### 直近 5 項目\n\n247. 🎉 some body\n248. 🎉 other\n\n## End\n"
        items = claudemd_archive.parse_items(old_format)
        self.assertEqual(len(items), 0, "旧 numbered 形式は現行 regex で検出されない")

    def test_identify_bloat_candidates_fifo_oldest(self):
        text = _make_section([260, 261, 262, 264, 265, 266, 267])  # 7 items
        items = claudemd_archive.parse_items(text)
        self.assertEqual(len(items), 7)
        candidates = claudemd_archive.identify_bloat_candidates(items, keep_recent=5)
        self.assertEqual(candidates, [260, 261], "keep_recent=5 で最古 2 件を FIFO flush 候補化")

    def test_live_claudemd_has_five_items(self):
        # 実 CLAUDE.md が現在 5 項目を保持 (ADR-001 governance 準拠) を確認
        if not CLAUDE_MD.exists():
            self.skipTest("CLAUDE.md not found")
        items = claudemd_archive.parse_items(CLAUDE_MD.read_text(encoding="utf-8"))
        self.assertEqual(len(items), 5, f"live CLAUDE.md は 5 項目であるべき (実={len(items)})")


class DriftRecentItemsCheckTest(unittest.TestCase):
    """docs_sync.py check_recent_items_section_count (Z-3 第 3 軸)."""

    def _run_check_with_text(self, text):
        with tempfile.NamedTemporaryFile(
            "w", suffix=".md", delete=False, encoding="utf-8"
        ) as f:
            f.write(text)
            tmp = Path(f.name)
        original = docs_sync.CLAUDE_MD
        try:
            docs_sync.CLAUDE_MD = tmp
            return docs_sync.check_recent_items_section_count()
        finally:
            docs_sync.CLAUDE_MD = original
            tmp.unlink()

    def test_header_matches_actual_passes(self):
        ok, msg = self._run_check_with_text(_make_section([264, 265, 266, 267, 268], header_n=5))
        self.assertTrue(ok, f"header 5 ↔ 実数 5 で PASS すべき: {msg}")

    def test_bloat_mismatch_fails(self):
        # header 5 だが 6 項目 = 肥大 → FAIL
        ok, msg = self._run_check_with_text(
            _make_section([264, 265, 266, 267, 268, 269], header_n=5)
        )
        self.assertFalse(ok, "header 5 ↔ 実数 6 で FAIL すべき")
        self.assertIn("肥大", msg)

    def test_self_adjust_header_n(self):
        # header を 3 にすれば閾値も 3 に追従 (self-adjusting)
        ok, msg = self._run_check_with_text(_make_section([100, 101, 102], header_n=3))
        self.assertTrue(ok, f"header 3 ↔ 実数 3 で PASS すべき: {msg}")

    def test_section_absent_graceful(self):
        ok, msg = self._run_check_with_text("# CLAUDE.md\n## H\n[no recent section]\n## End\n")
        self.assertTrue(ok, "section 不在は graceful INFO で PASS")
        self.assertIn("INFO", msg)

    def test_live_claudemd_passes(self):
        # 実 CLAUDE.md の header N ↔ 実数が整合していることを確認
        ok, msg = docs_sync.check_recent_items_section_count()
        self.assertTrue(ok, f"live CLAUDE.md は header/actual 整合すべき: {msg}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
