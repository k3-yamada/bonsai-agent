#!/usr/bin/env python3
"""CLAUDE.md archive policy automation (項目 240 候補、plan: .claude/plan/claudemd-archive-policy.md)。

mode (CLI flag `--mode`):
  check     現在 size + 項目数 + 推奨 archive 件数を report (read-only、default)
  dry-run   archive 後の CLAUDE.md / archive.md の diff preview (副作用ゼロ)
  apply     実 archive 適用 (CLAUDE.md rewrite + archive append + 任意 git commit)

依存: 標準ライブラリのみ (subprocess for git、tempfile for dry-run isolation)。

設計選択 (plan §2 案 A):
- developer-driven CLI、pre-commit hook 不採用 (commit failure risk 回避)
- archive 移動先 = `memory/harness_patterns_archive.md` (project-memory directory、git 外)
- one-line summary = `**...**` 強調タイトル + test 数 delta + 1-2 文要約
"""

from __future__ import annotations

import argparse
import difflib
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

# 項目行検出: CLAUDE.md format = `- **NNN**: body` markdown bullet (Item 255 以降)
# Item 255 で CLAUDE.md trim 形式が archive-style numbered list (`NNN. body`) から
# markdown bullet 形式に shift。本 regex は現行 CLAUDE.md format に追随。
# 出力 (generate_one_line_summary) は archive 形式 `NNN. ` を維持。
ITEM_START_RE = re.compile(r"^-\s+\*\*(\d{2,4})\*\*:\s+(.+)$")

# `**強調**` 抽出 (タイトル候補、最初の match)
EMPHASIS_RE = re.compile(r"\*\*([^*]+?)\*\*")

# test 数 delta pattern (1271 → 1275 passed (+4) 等)
TEST_COUNT_RE = re.compile(r"(\d{3,5})\s*[→\-]+>?\s*(\d{3,5})\s*passed(?:\s*\(([+\-]?\d+)\))?")


@dataclass
class Item:
    """parse 結果の 1 項目。"""

    number: int
    body: str  # `NUM. ` 以降の全文 (改行含む multi-paragraph 可)
    size: int  # body のバイト数 (UTF-8)
    start_line: int  # 1-indexed 行番号 (rewrite 用)
    end_line: int  # 1-indexed 行番号 (inclusive)


def parse_items(claudemd_text: str) -> list[Item]:
    """CLAUDE.md text から Item リストを抽出。

    項目は `\\d+. body` 形式で開始、次の `\\d+. ` 行 or section header (`##`) で終了。
    """
    lines = claudemd_text.split("\n")
    items: list[Item] = []
    current: Optional[tuple[int, int, list[str]]] = None  # (number, start_idx, body_lines)

    def finalize(end_idx: int) -> None:
        nonlocal current
        if current is None:
            return
        num, start_idx, body_lines = current
        body = "\n".join(body_lines).rstrip()
        size = len(body.encode("utf-8"))
        items.append(
            Item(number=num, body=body, size=size, start_line=start_idx + 1, end_line=end_idx + 1)
        )
        current = None

    for i, line in enumerate(lines):
        m = ITEM_START_RE.match(line)
        is_section_header = line.startswith("##") or line.startswith("```")
        if m:
            num = int(m.group(1))
            # 項目番号 spam 回避: NUM が 1-999 範囲
            if 1 <= num <= 999:
                finalize(i - 1)
                current = (num, i, [line])
                continue
        if is_section_header and current is not None:
            finalize(i - 1)
            continue
        if current is not None:
            current[2].append(line)

    if current is not None:
        finalize(len(lines) - 1)

    return items


def identify_bloat_candidates(
    items: list[Item], keep_recent: int = 5, verbose_threshold: int = 500
) -> list[int]:
    """archive 対象項目番号を抽出。

    - 直近 `keep_recent` 項目は対象外 (詳細保持)
    - 残り項目のうち `verbose_threshold` byte 超のみ対象 (短い summary は既に圧縮済)
    """
    if not items:
        return []
    # 項目番号 昇順 sort (parse 順は通常昇順だが保証)
    sorted_items = sorted(items, key=lambda x: x.number)
    if len(sorted_items) <= keep_recent:
        return []
    # 直近 keep_recent を除外
    older = sorted_items[:-keep_recent] if keep_recent > 0 else sorted_items
    # verbose のみ
    return [it.number for it in older if it.size > verbose_threshold]


def generate_one_line_summary(item_body: str) -> str:
    """verbose body から 1 行 summary を生成。

    構成: `NUM. **タイトル強調** — archive 参照 (test 数 delta 等の重要 metadata)`
    """
    # 1 行目から番号抽出
    first_line = item_body.split("\n", 1)[0]
    num_match = ITEM_START_RE.match(first_line)
    num_str = num_match.group(1) + "." if num_match else ""

    # `**強調**` タイトル抽出
    emphasis = EMPHASIS_RE.search(item_body)
    title = emphasis.group(1).strip() if emphasis else "(タイトル抽出失敗)"

    # test 数 delta 抽出
    tc_match = TEST_COUNT_RE.search(item_body)
    if tc_match:
        before, after, delta = tc_match.groups()
        delta_str = f"({delta})" if delta else ""
        test_info = f"、{before}→{after} passed {delta_str}".strip()
    else:
        test_info = ""

    summary = f"{num_str} **{title}** — archive 参照{test_info}".strip()
    return summary.replace("\n", " ")


def rewrite_claudemd(
    claudemd_text: str, items: list[Item], archived_numbers: list[int]
) -> str:
    """archive 対象項目を 1-line summary に置換した CLAUDE.md text を返す。

    - 項目 body の line range を summary 1 行に置換
    - 順序保持 (項目番号順)
    """
    if not archived_numbers:
        return claudemd_text

    archive_set = set(archived_numbers)
    archive_items = {it.number: it for it in items if it.number in archive_set}

    lines = claudemd_text.split("\n")
    # 後ろから replace で line index 不変
    sorted_archive = sorted(archive_items.values(), key=lambda x: x.start_line, reverse=True)
    for it in sorted_archive:
        summary = generate_one_line_summary(it.body)
        # start_line..end_line (1-indexed inclusive) を summary 1 行に置換
        start_idx = it.start_line - 1
        end_idx = it.end_line  # exclusive in slice
        lines[start_idx:end_idx] = [summary]

    return "\n".join(lines)


def append_to_archive(archive_text: str, items: list[Item], archived_numbers: list[int]) -> str:
    """archive.md text の末尾に items verbatim を追記。順序保証 (番号昇順)。"""
    if not archived_numbers:
        return archive_text

    archive_set = set(archived_numbers)
    targets = sorted([it for it in items if it.number in archive_set], key=lambda x: x.number)
    if not targets:
        return archive_text

    appendix_parts = [""]  # leading blank line
    for it in targets:
        appendix_parts.append(it.body)
        appendix_parts.append("")  # blank line between items

    appendix = "\n".join(appendix_parts)
    # 既存末尾に改行 1 つ追加して append
    base = archive_text.rstrip("\n")
    return base + "\n" + appendix


def run_check(claudemd_path: Path, archive_path: Path, keep_recent: int, max_size_kb: int) -> str:
    """check mode: read-only report。"""
    text = claudemd_path.read_text(encoding="utf-8")
    items = parse_items(text)
    candidates = identify_bloat_candidates(items, keep_recent=keep_recent)
    size_kb = len(text.encode("utf-8")) / 1024
    warning = " ⚠ over threshold" if size_kb > max_size_kb else ""

    parts = [
        "=== CLAUDE.md archive check ===",
        f"  file: {claudemd_path}",
        f"  size: {size_kb:.1f} KB / threshold {max_size_kb} KB{warning}",
    ]
    if items:
        parts.append(
            f"  項目数: {len(items)} (number range: {items[0].number}-{items[-1].number})"
        )
    else:
        parts.append("  項目数: 0")
    parts.append(f"  keep_recent: {keep_recent}")
    if candidates:
        parts.append(f"  archive 候補: {len(candidates)} 件 = {candidates}")
    else:
        parts.append("  archive 候補: 0 件 (健全)")
    return "\n".join(parts)


def run_dry_run(claudemd_path: Path, archive_path: Path, keep_recent: int) -> str:
    """dry-run mode: diff preview のみ、file modification ゼロ。"""
    text = claudemd_path.read_text(encoding="utf-8")
    archive_text = archive_path.read_text(encoding="utf-8") if archive_path.exists() else ""
    items = parse_items(text)
    candidates = identify_bloat_candidates(items, keep_recent=keep_recent)

    if not candidates:
        return "(no archive candidates)"

    new_claudemd = rewrite_claudemd(text, items, candidates)
    new_archive = append_to_archive(archive_text, items, candidates)

    diff_claudemd = "\n".join(
        difflib.unified_diff(
            text.split("\n"),
            new_claudemd.split("\n"),
            fromfile=f"a/{claudemd_path.name}",
            tofile=f"b/{claudemd_path.name}",
            lineterm="",
        )
    )
    diff_archive = "\n".join(
        difflib.unified_diff(
            archive_text.split("\n"),
            new_archive.split("\n"),
            fromfile=f"a/{archive_path.name}",
            tofile=f"b/{archive_path.name}",
            lineterm="",
        )
    )
    header = f"=== dry-run: archive {len(candidates)} 件 = {candidates} ===\n"
    return header + diff_claudemd + "\n\n" + diff_archive


def run_apply(
    claudemd_path: Path, archive_path: Path, keep_recent: int, do_commit: bool = False
) -> list[int]:
    """apply mode: 実際に CLAUDE.md + archive を modify、任意で git commit。

    戻り値: archived 項目番号 list。
    """
    text = claudemd_path.read_text(encoding="utf-8")
    archive_text = archive_path.read_text(encoding="utf-8") if archive_path.exists() else ""
    items = parse_items(text)
    candidates = identify_bloat_candidates(items, keep_recent=keep_recent)

    if not candidates:
        return []

    new_claudemd = rewrite_claudemd(text, items, candidates)
    new_archive = append_to_archive(archive_text, items, candidates)

    # write (apply order: archive first で partial state ならば CLAUDE.md unchanged)
    archive_path.write_text(new_archive, encoding="utf-8")
    claudemd_path.write_text(new_claudemd, encoding="utf-8")

    if do_commit:
        try:
            subprocess.run(
                ["git", "add", str(claudemd_path)],
                check=True,
                cwd=claudemd_path.parent,
            )
            msg = (
                f"docs(claude): 項目 {candidates[0]}-{candidates[-1]} archive "
                f"(auto、{len(candidates)} 件)"
            )
            subprocess.run(
                ["git", "commit", "-m", msg],
                check=True,
                cwd=claudemd_path.parent,
            )
        except subprocess.CalledProcessError as e:
            print(f"WARNING: git commit failed: {e}", file=sys.stderr)

    return candidates


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--mode",
        choices=["check", "dry-run", "apply"],
        default="check",
        help="動作 mode (default: check)",
    )
    ap.add_argument(
        "--keep-recent", type=int, default=5, help="直近 N 項目を詳細保持 (default 5)"
    )
    ap.add_argument(
        "--max-size-kb", type=int, default=20, help="warning 閾値 KB (default 20)"
    )
    ap.add_argument("--claudemd", type=Path, default=Path("CLAUDE.md"))
    ap.add_argument(
        "--archive",
        type=Path,
        default=Path(
            "~/.claude/projects/-Users-keizo-bonsai-agent/memory/harness_patterns_archive.md"
        ).expanduser(),
    )
    ap.add_argument("--commit", action="store_true", help="apply mode で git commit も実行")
    args = ap.parse_args()

    if not args.claudemd.exists():
        print(f"ERROR: claudemd not found: {args.claudemd}", file=sys.stderr)
        return 2

    if args.mode == "check":
        print(run_check(args.claudemd, args.archive, args.keep_recent, args.max_size_kb))
        return 0
    elif args.mode == "dry-run":
        print(run_dry_run(args.claudemd, args.archive, args.keep_recent))
        return 0
    elif args.mode == "apply":
        archived = run_apply(
            args.claudemd, args.archive, args.keep_recent, do_commit=args.commit
        )
        if archived:
            print(f"=== applied: {len(archived)} 件 archive = {archived} ===")
        else:
            print("(no archive candidates、CLAUDE.md unchanged)")
        return 0
    return 1


if __name__ == "__main__":
    sys.exit(main())
