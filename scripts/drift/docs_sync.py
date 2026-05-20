#!/usr/bin/env python3
"""Z-3 Phase 2: docs ↔ code 整合性検出 (Read-only Drift Linter).

検出軸:
1. CLAUDE.md「直近項目」 ↔ memory/harness_patterns_archive.md cross-ref
   - 直近項目で言及される項目番号が archive にも存在するか
2. docs/architecture/module-layer-rules.md の LAYER_ORDER ↔ tests/structural.rs の LAYER_ORDER 整合性

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

    if not claude_items:
        return True, "INFO: No `**NNN**:` items found in CLAUDE.md"

    if not ARCHIVE.exists():
        return False, (
            f"**FAIL**: archive file not found at `{ARCHIVE}`. "
            "修正方法: archive 整備状態を確認、または項目 247-252 の verbatim 追加が抜けていないか. "
            "参照: docs/architecture/module-layer-rules.md"
        )

    archive_text = ARCHIVE.read_text(encoding="utf-8")
    # archive 形式: "NNN. 🎉 **..." / "NNN. 🟡 **..." / "NNN. **..." / "NNN. **(欠番)**" 等、
    # 絵文字 (status marker) や直接 ** で開始する 3 桁項目を catch.
    # 単純 "^NNN. " (3-digit + dot + space) で開始する line を全件項目として扱う.
    archive_items = sorted(set(int(m) for m in re.findall(r"^(\d{3})\.\s", archive_text, re.MULTILINE)))

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

    return 0 if overall_pass else 1


if __name__ == "__main__":
    sys.exit(main())
