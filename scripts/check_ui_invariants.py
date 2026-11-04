#!/usr/bin/env python3
"""F-073 UI 不变量的只读静态检查，不需要 GUI 或前台窗口。

用法：python scripts/check_ui_invariants.py
退出码 0 表示全部通过；1 表示有违规（会打印文件与行号）。

覆盖前几批靠人工盘点得出的结论，把它们固化成回归检查：
  1. 手搓 role(Role::Button) 元素必须有 id / aria_label / 键盘可达。
  2. open_asset_with_system 的失败分支不能只发 toast，还要写就地错误。
  3. 命令面板展示的键位必须在 cx.bind_keys 里真实存在。
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
UI = ROOT / "crates" / "kanalib-ui" / "src"

# 图库瓦片由根容器的 track_focus + 方向键接管，避免重复 Tab 停靠点。
TILE_EXEMPT = {
    "views.rs",
    "views/grid.rs",
    "dedup.rs",
}


def lines_of(path: pathlib.Path) -> list[str]:
    return path.read_text(encoding="utf-8").splitlines()


def check_role_buttons() -> tuple[list[str], int]:
    problems: list[str] = []
    count = 0
    for path in sorted(UI.rglob("*.rs")):
        rel = path.relative_to(UI).as_posix()
        lines = lines_of(path)
        for i, line in enumerate(lines):
            if ".role(Role::Button)" not in line:
                continue
            count += 1
            before = "\n".join(lines[max(0, i - 8) : i])
            after = "\n".join(lines[i + 1 : i + 9])
            where = f"{rel}:{i + 1}"
            if ".id(" not in before:
                problems.append(f"{where} 手搓按钮缺 .id(...)（tooltip/tab_index 需要 id）")
            if ".aria_label(" not in after:
                problems.append(f"{where} 手搓按钮缺 .aria_label(...)")
            if rel not in TILE_EXEMPT and ".tab_index(" not in before and ".tab_index(" not in after:
                problems.append(f"{where} 手搓按钮键盘不可达（缺 .tab_index(0)）")
    return problems, count


def check_open_errors() -> tuple[list[str], int]:
    problems: list[str] = []
    count = 0
    for path in sorted(UI.rglob("*.rs")):
        rel = path.relative_to(UI).as_posix()
        lines = lines_of(path)
        for i, line in enumerate(lines):
            if "if let Err(error) = open_asset_with_system" not in line:
                continue
            count += 1
            block = "\n".join(lines[i : i + 14])
            where = f"{rel}:{i + 1}"
            if "push_toast" not in block:
                problems.append(f"{where} 打开失败没有即时反馈 push_toast")
            if "set_ui_error" not in block:
                problems.append(f"{where} 打开失败只有 toast，缺就地说明 set_ui_error")
    return problems, count


def check_palette_keys() -> tuple[list[str], int]:
    app = (UI / "app.rs").read_text(encoding="utf-8")
    shown = set(re.findall(r'\.keys\("([^"]+)"\)', app))
    bound = set(re.findall(r'KeyBinding::new\("([^"]+)"', app))
    missing = sorted(shown - bound)
    problems = [f"命令面板键位 {key!r} 没有 cx.bind_keys 真实绑定" for key in missing]
    return problems, len(shown)


def main() -> int:
    checks = [
        ("手搓 role(Role::Button) 元素（id/aria/键盘可达）", *check_role_buttons()),
        ("open_asset_with_system 失败分支（toast + 就地错误）", *check_open_errors()),
        ("命令面板键位（必须真实绑定）", *check_palette_keys()),
    ]
    failed = False
    for name, problems, count in checks:
        if problems:
            failed = True
            print(f"FAIL  {name}  检查 {count} 处，{len(problems)} 个问题")
            for problem in problems:
                print(f"        - {problem}")
        else:
            print(f"PASS  {name}  检查 {count} 处")
    print("== 校验结束 ==")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
