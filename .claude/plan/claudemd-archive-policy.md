# CLAUDE.md Archive Policy Automation (項目 240 候補)

**状態**: planning-only (2026-05-16 起票)
**推奨度**: ★★ (運用負債、N+10 項目蓄積で再発、本 session 案 C 手動再整理の自動化)
**推定工数**: ~3-4h Phase 1-3 (TDD strict) + ~30 min dry-run 検証 + 後続 session で必要時実行
**起点**:
- 本 session 案 C 手動再整理 = 82 KB → 13.5 KB (-83%) を達成、ただし将来 N+10 蓄積で再発
- 既存 precedent = 項目 1-201 archive (2026-05-07) + 202-219 追加 (2026-05-10) + 220-239 追加 (2026-05-16 = 本 session)
- 過去 3 回 = 5 ヶ月で 3 回 (約 2 ヶ月毎) 手動 archive 発生、自動化 ROI 高

---

## §1. 問題定義

### 1.1 観測される肥大化 pattern
| 時期 | 状態 | size | 行数 | 対応 |
|---|---|---|---|---|
| 2026-04-14 | 初期 | ~5 KB | ~50 行 | - |
| 2026-05-07 | 項目 201 時点 | ~50 KB | ~180 行 | archive 1-201 (手動) |
| 2026-05-10 | 項目 219 時点 | ~45 KB | ~170 行 | archive 202-219 (手動追記) |
| 2026-05-16 | 項目 239 時点 (本 session) | **82 KB** | **261 行** | **案 C 全面再整理 (手動)** |
| 2026-07 推定 | 項目 ~270 | ~120 KB? | ~350 行? | (自動化済なら自動 archive) |

### 1.2 肥大化の structural cause
- 1 項目あたり verbose 多段落 = 3-7 KB
- N 項目 / 1 month 蓄積 = +21-49 KB / month
- claude session 起動時 system prompt の context 圧迫 (CLAUDE.md は auto-load)
- 「直近項目」section title が「1 行サマリー」と謳いつつ実体は多段落で乖離

### 1.3 既存対応の限界
- 手動 archive = N+10 蓄積時点で session を一時中断して整理作業発生
- archive 移動先 = `memory/harness_patterns_archive.md` (project-memory directory、git 外)
- 1 行 summary 生成 = LLM 手動圧縮で品質ばらつき

---

## §2. 設計 — 3 案比較 (推奨 = 案 A、要 user 判断)

| 案 | trigger | implementation | 採否候補 |
|---|---|---|---|
| **A** | Manual CLI (developer-driven) | Python script `scripts/claudemd_archive.py` (CLI mode: check / dry-run / apply) | ★★ 推奨 |
| B | Pre-commit hook (auto on every commit) | shell hook + Python script | ★ 危険 (commit failure risk) |
| C | Size threshold warning (read-only check) | Python script + GitHub Action | ★ 緩和案 |

### 2.1 案 A (推奨): Manual CLI

**変更**:
- `scripts/claudemd_archive.py` 新規 (~250 行、stdlib only)
- mode:
  - `--check`: 現在 size + 項目数 + 推奨 archive 件数を report (read-only)
  - `--dry-run`: archive 後の CLAUDE.md + archive.md の diff preview
  - `--apply`: 実 archive 適用 + git add + commit (`docs(claude): N 項目 archive` メッセージ)
- 設定可能 threshold (env or CLI flag):
  - `--keep-recent N` (default 5): 直近 N 項目は CLAUDE.md に詳細保持
  - `--max-size-kb K` (default 20): 警告閾値
  - `--archive-path P` (default `memory/harness_patterns_archive.md`)

**Pros**:
- developer-driven = git workflow に介入なし、commit failure 0
- dry-run で安全確認可能
- 設計 / 推奨基準は LLM の判断で柔軟調整可

**Cons**:
- 手動実行が必要 (週次 / N+10 蓄積時)
- CLI 認知が必要 (CLAUDE.md 注意事項に記述 + slash command 化を後続検討)

### 2.2 案 B (棄却): Pre-commit hook

**理由**: commit のたびに archive 処理が走ると developer 流速を阻害、archive 判断は文脈依存で誤判断時の rollback 困難、TDD strict workflow との相性悪

### 2.3 案 C (緩和案 / 補助): Size threshold warning

`scripts/claudemd_archive.py --check` を CI で実行し、size > 20 KB で warning を comment。案 A の補強策として後続別 plan で検討。

---

## §3. 実装 scripts (案 A 採用想定)

### `scripts/claudemd_archive.py` (新規 ~250 行)

```python
#!/usr/bin/env python3
"""CLAUDE.md archive policy automation (項目 240 候補)。

mode:
  --check     現在 size + 項目数を report
  --dry-run   archive 後の diff preview
  --apply     実 archive + git add + commit
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

ITEM_RE = re.compile(r"^(\d+)\.\s+\*\*(.+?)\*\*(.*)$", re.DOTALL)
SECTION_RECENT_RE = re.compile(r"^### 直近 (\d+) 項目", re.MULTILINE)
ARCHIVE_LINK_RE = re.compile(r"\*\*項目 1-(\d+) は \[memory/harness_patterns_archive\.md\]")


def parse_items(claudemd_text: str) -> list[tuple[int, str, int]]:
    """CLAUDE.md から (number, body, byte_size) を抽出。"""
    # ...


def identify_bloat_candidates(items, keep_recent: int) -> list[int]:
    """直近 keep_recent を除外、verbose (>500 byte) の項目を archive 対象に。"""
    # ...


def generate_one_line_summary(item_body: str) -> str:
    """verbose body の最初の `**...**` 強調 + 1-2 文の要約抽出。"""
    # ...


def append_to_archive(archive_path: Path, items_verbatim: list[tuple[int, str]]):
    """archive.md 末尾に items を verbatim 追記。"""
    # ...


def rewrite_claudemd(claudemd_path: Path, archived_items: list[int], summaries: dict[int, str]):
    """CLAUDE.md を再構成 (verbose を summary に置換、archive link 更新)。"""
    # ...


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--mode", choices=["check", "dry-run", "apply"], default="check")
    ap.add_argument("--keep-recent", type=int, default=5)
    ap.add_argument("--max-size-kb", type=int, default=20)
    ap.add_argument("--claudemd", type=Path, default=Path("CLAUDE.md"))
    ap.add_argument("--archive", type=Path,
                    default=Path("~/.claude/projects/-Users-keizo-bonsai-agent/memory/harness_patterns_archive.md").expanduser())
    args = ap.parse_args()
    # ...


if __name__ == "__main__":
    sys.exit(main())
```

### `scripts/test_claudemd_archive.py` (新規 ~150 行、TDD strict)

```python
import unittest

class ParseItemsTest(unittest.TestCase):
    def test_parse_items_extracts_number_body_size(self): ...
    def test_parse_items_ignores_non_item_lines(self): ...

class BloatIdentificationTest(unittest.TestCase):
    def test_keep_recent_5_skips_last_5_items(self): ...
    def test_verbose_threshold_500_bytes(self): ...

class SummaryGenerationTest(unittest.TestCase):
    def test_one_line_summary_extracts_title_emphasis(self): ...
    def test_one_line_summary_includes_test_count_delta(self): ...

class IntegrationTest(unittest.TestCase):
    def test_dry_run_produces_diff_without_modification(self): ...
    def test_apply_modifies_claudemd_and_archive(self): ...
```

---

## §4. TDD strict 5 phase

### Phase 1 (Red) — 8 failing test
1. `t_parse_items_extracts_number_body_size`: regex で項目番号/body/size 抽出
2. `t_parse_items_ignores_non_item_lines`: section header 等は無視
3. `t_keep_recent_5_skips_last_5_items`: 直近 N 項目は archive 対象外
4. `t_verbose_threshold_500_bytes`: 短い 1-line summary は archive 対象外
5. `t_one_line_summary_extracts_title_emphasis`: `**...**` 強調抽出
6. `t_one_line_summary_includes_test_count_delta`: `1271 → 1275 passed (+4)` パターン抽出
7. `t_dry_run_produces_diff_without_modification`: dry-run で副作用ゼロ
8. `t_apply_modifies_claudemd_and_archive`: apply で両 file 更新

### Phase 2 (Green)
- `scripts/claudemd_archive.py` 実装 (~250 行)
- `scripts/test_claudemd_archive.py` 8 test PASS

### Phase 3 (Refactor)
- chmod +x `scripts/claudemd_archive.py`
- docstring に項目 240 起源 + 案 A 選定理由明示
- CLAUDE.md「注意事項」に「N+10 蓄積時 `./scripts/claudemd_archive.py --check` 実行推奨」追記

### Phase 4 (Smoke)
- `./scripts/claudemd_archive.py --check`: 現在 (13.5 KB / 191 行 / 6 verbose items 235-239) → 0 archive 推奨 (直近 5 内、verbose 5 だが --keep-recent 5)
- `./scripts/claudemd_archive.py --dry-run`: 仮想シナリオ (項目 245 まで蓄積) を test fixture で生成 → dry-run diff 確認
- `--apply` は実 Lab 完走後の N+10 蓄積時に user 都合で実行 (本 plan scope 外)

### Phase 5 (運用)
- 月次 / N+10 蓄積時に `./scripts/claudemd_archive.py --check` で size monitor
- 自動 archive 実行は user 都合で `--apply` 選択
- 将来 `/oh-my-claudecode:archive-claudemd` slash command 化検討 (別 plan)

---

## §5. risks / mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | one-line summary の品質低下 (重要 metadata 欠落) | regex で必須 metadata (test 数 / commit SHA / Lab 名 / 🎉 マーカー) 強制保持、Phase 1 Red test で検証 |
| R2 | archive 順序破壊 (項目番号の昇順 violation) | parse 時に sort、append 前に archive 末尾の最大番号 verify |
| R3 | dry-run で誤った diff 表示 (apply 後挙動と不一致) | dry-run = 一時 file に write + `diff` で表示、apply と同コード path |
| R4 | git commit failure 時の partial state | apply は (archive append → CLAUDE.md rewrite → git add → git commit) を try/except で囲み、失敗時は git checkout で rollback |
| R5 | section header / category index の自動更新失敗 | category index は手動保持 (apply は body 置換のみ、index は user 責任)、注意事項に明記 |

---

## §6. 期待効果

### CLAUDE.md size の長期維持
- 現在: 13.5 KB (本 session 案 C 後)
- 目標: 月次 / N+10 蓄積時に自動 archive で 15 KB 上限維持
- claude session context load の安定化

### archive の体系化
- 1 行 summary 自動生成で品質ばらつき低減
- archive 順序保証 (番号昇順)
- archive を navigation 可能な knowledge base 化

### developer workflow への影響ゼロ
- 案 A = developer 主導 = commit / TDD workflow 不変
- 必要時のみ `./scripts/claudemd_archive.py` 実行

---

## §7. ロールバック戦略

- `--apply` 失敗時 = git checkout で CLAUDE.md 復元 + archive append revert (1 commit revert)
- production code 変更ゼロ = revert 影響範囲は CLAUDE.md + archive のみ
- 1 行 summary 品質不満 = `--keep-recent` を増やして再 archive (手動上書き)

---

## §8. 起票候補項目

- **項目 240 候補** = 本 plan の Phase 1-3 完遂 (script delivery、TDD strict)
- 将来項目 = `/oh-my-claudecode:archive-claudemd` slash command 化 (別 plan)

---

## §9. 依存 / 並行性

### 完遂前提
- 本 session 案 C 手動再整理 完遂 ✅ (CLAUDE.md 13.5 KB / archive 281 行)
- precedent = 項目 1-201 archive (2026-05-07) の 1 行 summary pattern ✅

### 並行可
- production code touch ゼロ = code 変更系 plan / Lab 系全てと並行可
- Lab v20 進行中 (PID 32568) でも `cargo build` / cargo test に影響なし

---

## §10. Quick Start

```bash
cd /Users/keizo/bonsai-agent

# Phase 1 Red
mkdir -p scripts && $EDITOR scripts/test_claudemd_archive.py
python3 scripts/test_claudemd_archive.py  # 8 FAIL (ModuleNotFoundError)

# Phase 2 Green
$EDITOR scripts/claudemd_archive.py       # 250 行実装
python3 scripts/test_claudemd_archive.py  # 8 PASS

# Phase 3 Refactor
chmod +x scripts/claudemd_archive.py

# Phase 4 Smoke
./scripts/claudemd_archive.py --check
./scripts/claudemd_archive.py --dry-run

# Commit
git add scripts/claudemd_archive.py scripts/test_claudemd_archive.py
git commit -m "feat(claudemd): 項目 240 archive policy automation script (TDD Phase 1-3)"

# 運用 (N+10 蓄積時、別 session)
./scripts/claudemd_archive.py --check
./scripts/claudemd_archive.py --apply  # user 都合で実行
```

---

## §11. 参考

- 本 session 案 C 全面再整理 commit `a4a607e` (82 KB → 13.5 KB / -83%)
- 既存 precedent: 項目 1-201 archive (2026-05-07) / 202-219 追加 (2026-05-10)
- `memory/harness_patterns_archive.md` (項目 1-239 verbatim、281 行)
- Lab v19 paired script template (`scripts/lab_v19_paired.sh`) + ttest analyzer (`scripts/lab_v19_paired_ttest.py`) と同 pattern の Python stdlib only 実装
