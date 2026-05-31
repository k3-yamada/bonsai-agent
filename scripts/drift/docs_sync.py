#!/usr/bin/env python3
"""Z-3 Phase 2: docs ↔ code 整合性検出 (Read-only Drift Linter).

検出軸:
1. CLAUDE.md「直近項目」 ↔ memory/harness_patterns_archive.md cross-ref
   - 直近項目で言及される項目番号が archive にも存在するか
2. docs/architecture/module-layer-rules.md の LAYER_ORDER ↔ tests/structural.rs の LAYER_ORDER 整合性
3. CLAUDE.md「### 直近 N 項目」 header N ↔ section 内 `**NNN**:` 実数整合
   - header と実態の乖離 (例: 「直近 5 項目」と銘打って 21 項目蓄積) を機械検出
   - 起源: .claude/plan/claudemd-size-reduction-item-255-recreate.md (Item 255 規模再現 plan)
   - 再肥大の mechanical enforcement、FIFO 運用ルール違反を CI で catch 可能化

出力: docs/quality/drift-YYYYMMDD.md に append.
Read-only: 検出のみ、auto-fix なし.
"""

import datetime
import re
import sys
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[2]
CLAUDE_MD = PROJECT_ROOT / "CLAUDE.md"
MODULE_LAYER_RULES = PROJECT_ROOT / "docs" / "architecture" / "module-layer-rules.md"
STRUCTURAL_TEST = PROJECT_ROOT / "tests" / "structural.rs"
ARCHIVE = (
    Path.home()
    / ".claude"
    / "projects"
    / "-Users-keizo-bonsai-agent"
    / "memory"
    / "harness_patterns_archive.md"
)

DATE = datetime.datetime.now().strftime("%Y%m%d")
REPORT = PROJECT_ROOT / "docs" / "quality" / f"drift-{DATE}.md"


def ensure_report_initialized() -> None:
    """Initialize report file if missing."""
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    if not REPORT.exists():
        REPORT.write_text(
            f"# bonsai-agent Drift Report ({DATE})\n\n"
            f"> Z-3 (Zenn Codex Harness Step 8) Lightweight Drift Linter 出力.\n"
            f"> Read-only 検出のみ、auto-fix なし. 確認後の修正は manual.\n\n",
            encoding="utf-8",
        )


def append_section(title: str, body: str) -> None:
    """Append a section to the report."""
    timestamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S %Z").strip()
    section = f"\n## {title}\n\nGenerated: {timestamp}\n\n{body}\n"
    with REPORT.open("a", encoding="utf-8") as f:
        f.write(section)


def check_claude_archive_crossref() -> tuple[bool, str]:
    """CLAUDE.md「直近項目」で言及される項目番号 ↔ archive に存在確認."""
    if not CLAUDE_MD.exists():
        return False, "**FAIL**: CLAUDE.md not found"

    claude_text = CLAUDE_MD.read_text(encoding="utf-8")
    # "**252**: ..." or "**256**: ..." pattern を grep
    claude_items = sorted(set(int(m) for m in re.findall(r"\*\*(\d{3})\*\*:", claude_text)))

    # critic HIGH #1 fix: silent regression bypass 防止.
    # CLAUDE.md が non-trivial (>100 行) で項目 0 検出は marker 変更等の suspicious signal、FAIL.
    line_count = len(claude_text.splitlines())
    if not claude_items:
        if line_count > 100:
            return False, (
                f"⚠️ **CLAUDE.md non-trivial ({line_count} 行) だが `**NNN**:` 形式の項目が 0 件**. "
                "marker 変更 / format drift の可能性. 修正方法: CLAUDE.md の直近項目 section が "
                "`**NNN**:` 形式を維持しているか確認、または regex を新 marker に更新. "
                "参照: docs/architecture/module-layer-rules.md"
            )
        return True, f"INFO: No `**NNN**:` items found in CLAUDE.md (line_count={line_count}, trivial OK)"

    if not ARCHIVE.exists():
        return False, (
            f"**FAIL**: archive file not found at `{ARCHIVE}`. "
            "修正方法: archive 整備状態を確認、または項目 247-252 の verbatim 追加が抜けていないか. "
            "参照: docs/architecture/module-layer-rules.md"
        )

    archive_text = ARCHIVE.read_text(encoding="utf-8")
    # archive 形式: "NNN. 🎉 **..." / "NNN. 🟡 **..." / "NNN. **..." / "NNN. **(欠番)**" 等.
    # critic HIGH #2 fix: regex robustness — leading whitespace (bullet indent) + tab/全角空白許容.
    # `^\s*(\d{3})[.)]\s+` で indent / 数字 / `.` or `)` / 1+ 任意空白 (tab、全角含む) を catch.
    archive_items = sorted(
        set(
            int(m)
            for m in re.findall(r"^\s*(\d{3})[.)]\s+", archive_text, re.MULTILINE)
        )
    )

    missing = [n for n in claude_items if n not in archive_items]

    if not missing:
        return True, (
            f"✅ All {len(claude_items)} CLAUDE.md items found in archive "
            f"(items: {claude_items[:5]}... {claude_items[-5:]})"
        )
    return False, (
        f"⚠️ **{len(missing)} CLAUDE.md item(s) missing from archive**: {missing}.\n"
        f"修正方法: archive (harness_patterns_archive.md) に欠落項目を verbatim 追加. "
        f"参照: docs/architecture/module-layer-rules.md"
    )


def extract_layer_order(text: str, marker: str) -> list[str]:
    """Extract a LAYER_ORDER-style sequence from text after `marker`."""
    if "<" in marker:
        for line in text.splitlines():
            if marker in line and "<" in line:
                parts = [p.strip().strip("`") for p in line.split("<") if p.strip()]
                clean = []
                for p in parts:
                    name = re.search(r"[a-z_]+", p)
                    if name:
                        clean.append(name.group(0))
                if len(clean) >= 3:
                    return clean
    return []


def extract_layer_order_rust(text: str) -> list[str]:
    """Extract LAYER_ORDER from tests/structural.rs source."""
    match = re.search(
        r"const LAYER_ORDER:\s*&\[&str\]\s*=\s*&\[(.*?)\];", text, re.DOTALL
    )
    if not match:
        return []
    body = match.group(1)
    items = re.findall(r'"([^"]+)"', body)
    return items


def check_layer_order_sync() -> tuple[bool, str]:
    """docs/architecture/module-layer-rules.md ↔ tests/structural.rs LAYER_ORDER 整合性."""
    if not MODULE_LAYER_RULES.exists():
        return False, (
            "**FAIL**: docs/architecture/module-layer-rules.md not found. "
            "修正方法: Z-1 Phase 2 で本 file を作成済みであるべき. 参照: docs/architecture/module-layer-rules.md"
        )
    if not STRUCTURAL_TEST.exists():
        return False, (
            "**FAIL**: tests/structural.rs not found. "
            "修正方法: Z-4 (項目 256) で本 file を作成済みであるべき. 参照: docs/architecture/module-layer-rules.md"
        )

    docs_text = MODULE_LAYER_RULES.read_text(encoding="utf-8")
    rust_text = STRUCTURAL_TEST.read_text(encoding="utf-8")

    docs_order = extract_layer_order(docs_text, "db <")
    rust_order = extract_layer_order_rust(rust_text)

    if not docs_order:
        return False, (
            "**FAIL**: docs/architecture/module-layer-rules.md から layer 順を抽出できず. "
            "修正方法: 'db < observability < ...' 形式の行が存在するか確認. "
            "参照: docs/architecture/module-layer-rules.md"
        )
    if not rust_order:
        return False, (
            "**FAIL**: tests/structural.rs から LAYER_ORDER を抽出できず. "
            "修正方法: `const LAYER_ORDER: &[&str] = &[...]` 形式が存在するか確認. "
            "参照: docs/architecture/module-layer-rules.md"
        )

    if docs_order == rust_order:
        return True, (
            f"✅ LAYER_ORDER fully synchronized ({len(docs_order)} layers): "
            f"`{' < '.join(docs_order)}`"
        )
    return False, (
        f"⚠️ **LAYER_ORDER mismatch (drift detected)**:\n"
        f"  - docs/architecture/module-layer-rules.md: `{' < '.join(docs_order)}`\n"
        f"  - tests/structural.rs LAYER_ORDER: `{' < '.join(rust_order)}`\n"
        f"修正方法: 両 file の順序を一致させる. SSOT は docs/architecture/module-layer-rules.md. "
        f"参照: docs/architecture/module-layer-rules.md"
    )


def check_recent_items_section_count() -> tuple[bool, str]:
    """CLAUDE.md「### 直近 N 項目」 header N ↔ section 内 `**NNN**:` 実数整合.

    - header pattern: `### 直近 (\\d+) 項目` (例: `### 直近 5 項目 (詳細は archive 参照)`)
    - section 範囲: 該当 header 直後から次の `## ` or `### ` header または EOF まで
    - 実数 count: 該当 section 内の `**NNN**:` 形式項目番号 (重複除く)
    - PASS 条件: header N == 実数 (section 不在は INFO graceful)
    - 自己調整: header を「直近 10 項目」に変更すれば閾値 10 で再検証 (rule source = header 自身)
    """
    if not CLAUDE_MD.exists():
        return False, "**FAIL**: CLAUDE.md not found"

    claude_text = CLAUDE_MD.read_text(encoding="utf-8")
    lines = claude_text.splitlines()

    header_re = re.compile(r"^###\s+直近\s+(\d+)\s+項目")
    next_header_re = re.compile(r"^#{2,3}\s+")
    item_re = re.compile(r"\*\*(\d{3})\*\*:")

    header_idx = None
    expected_n = None
    for i, line in enumerate(lines):
        m = header_re.match(line)
        if m:
            header_idx = i
            expected_n = int(m.group(1))
            break

    if header_idx is None:
        return True, (
            "INFO: `### 直近 N 項目` header not found in CLAUDE.md "
            "(section absent or renamed — graceful skip)"
        )

    end_idx = len(lines)
    for j in range(header_idx + 1, len(lines)):
        if next_header_re.match(lines[j]):
            end_idx = j
            break

    section_text = "\n".join(lines[header_idx + 1 : end_idx])
    actual_items = sorted(set(int(m) for m in item_re.findall(section_text)))
    actual_n = len(actual_items)

    if actual_n == expected_n:
        return True, (
            f"✅ Recent items section header (N={expected_n}) matches actual count "
            f"({actual_n} items: {actual_items})"
        )
    diff = actual_n - expected_n
    direction = "肥大" if diff > 0 else "不足"
    return False, (
        f"⚠️ **Recent items section header/actual mismatch ({direction})**: "
        f"header「直近 {expected_n} 項目」だが実数 {actual_n} 項目 (Δ={diff:+d}).\n"
        f"  - actual items: {actual_items}\n"
        f"修正方法: {'最古項目を harness_patterns_archive.md に flush (FIFO 運用ルール)' if diff > 0 else 'header N を実数に合わせるか、不足分を再追加'}. "
        f"参照: .claude/plan/claudemd-size-reduction-item-255-recreate.md"
    )


def main() -> int:
    ensure_report_initialized()
    overall_pass = True

    ok, msg = check_claude_archive_crossref()
    overall_pass &= ok
    append_section("Docs ↔ Code: CLAUDE.md ↔ memory archive cross-ref", msg)
    print(f"{'PASS' if ok else 'FAIL'}: claude_archive_crossref")

    ok, msg = check_layer_order_sync()
    overall_pass &= ok
    append_section("Docs ↔ Code: LAYER_ORDER (docs ↔ tests/structural.rs)", msg)
    print(f"{'PASS' if ok else 'FAIL'}: layer_order_sync")

    ok, msg = check_recent_items_section_count()
    overall_pass &= ok
    append_section("Docs ↔ Code: CLAUDE.md 直近 N 項目 header ↔ actual count", msg)
    print(f"{'PASS' if ok else 'FAIL'}: recent_items_section_count")

    return 0 if overall_pass else 1


if __name__ == "__main__":
    sys.exit(main())
