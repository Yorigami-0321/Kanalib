#!/usr/bin/env python3
"""F-073 / F-070 UI 不变量的只读静态检查，不需要 GUI 或前台窗口。

用法：python scripts/check_ui_invariants.py
退出码 0 表示全部通过；1 表示有违规（会打印文件与行号）。

覆盖前几批靠人工盘点得出的结论，把它们固化成回归检查：
  1. 手搓 role(Role::Button) 元素必须有 id / aria_label / 键盘可达。
  2. open_asset_with_system 的失败分支不能只发 toast，还要写就地错误。
  3. 设置页分区标签数量与滚动锚点必须一一对应（F-070 方案 A）。
  4. 命令面板展示的键位必须在 cx.bind_keys 里真实存在。
  5. 上下文侧栏结构：单份标签搜索句柄、四种上下文都有分派、活动栏只在 sidebar.rs 构造。
  6. 根 FocusScope：应用根必须包 `.root()` + `.node(...)`，否则 Ely 的 Tab 焦点遍历不生效。
  7. core 状态 i18n：不得裸写 `shared.status`；所有 `notice.*` 键必须在中英文目录同时存在。
  8. 直接 UI 文案：`.child("…")`、`push_toast(..., "…")`、`Button::new(..., "…")`
     不得直接写中文（`is_english()` 分支除外）。
  9. D5 来源错误结构化：`status.error` 不得裸写中文；恢复/重试判定不得依赖
     `contains("监听"/"离线")`；去重固定保留原因必须在 UI 映射表里。
 10. UI 消息键：`t(cx, "…")` / `t_args(cx, "…")` / `placeholder_key(…, "…")` 的
     字面量键必须在中英目录同时存在（缺失键会触发 vendor i18n panic）。
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
UI = ROOT / "crates" / "kanalib-ui" / "src"
CORE = ROOT / "crates" / "kanalib-core" / "src"
STATUS_CATALOG = UI / "localization" / "extra" / "status.rs"

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
            # 失败分支还可能登记可重试动作，取 24 行仍限定在同一分支内。
            block = "\n".join(lines[i : i + 24])
            where = f"{rel}:{i + 1}"
            if "push_toast" not in block:
                problems.append(f"{where} 打开失败没有即时反馈 push_toast")
            if "set_ui_error" not in block:
                problems.append(f"{where} 打开失败只有 toast，缺就地说明 set_ui_error")
    return problems, count


def check_settings_anchors() -> tuple[list[str], int]:
    """F-070 方案 A：侧栏分区标签数量必须与设置页滚动锚点一一对应。"""
    sidebar = (UI / "sidebar.rs").read_text(encoding="utf-8")
    match = re.search(
        r"SETTINGS_SECTIONS: \[&str; (\d+)\] = \[(.*?)\];", sidebar, re.S
    )
    if not match:
        return ["sidebar.rs 找不到 SETTINGS_SECTIONS 常量"], 0
    labels = re.findall(r'"([^"]+)"', match.group(2))
    settings = (UI / "settings.rs").read_text(encoding="utf-8")
    anchors = re.findall(
        r"\.child\(div\(\)\.w\(column_width\)\.child\((\w+)\)\)", settings
    )
    problems = []
    if int(match.group(1)) != len(labels):
        problems.append("SETTINGS_SECTIONS 声明长度与实际条目数不一致")
    if len(labels) != len(anchors):
        problems.append(
            f"侧栏分区标签 {len(labels)} 个，设置页锚点 {len(anchors)} 个，索引会错位"
        )
    return problems, len(anchors)


def check_sidebar_contexts() -> tuple[list[str], int]:
    """F-070 方案 A：单搜索句柄、四上下文分派、活动栏与菜单的单一实现。"""
    app = (UI / "app.rs").read_text(encoding="utf-8")
    sidebar = (UI / "sidebar.rs").read_text(encoding="utf-8")
    tags = (UI / "tags.rs").read_text(encoding="utf-8")
    problems: list[str] = []
    checks = 0

    # 1) 标签管理搜索只创建一份，否则侧栏与标签页会变成两套状态。
    checks += 1
    creators = sum(
        text.count('use_keyed_state("tag-manager-search"')
        for text in (app, sidebar, tags)
    )
    if app.count('use_keyed_state("tag-manager-search"') != 1 or creators != 1:
        problems.append(
            "tag-manager-search 必须且只能在 app.rs 创建一次"
            f"（app/sidebar/tags 合计 {creators} 次）"
        )

    # 2) 四种上下文都要有各自的 body 分支。
    checks += 1
    dispatch = re.search(
        r"let body: AnyElement = match ctx \{(.*?)\n        \};", sidebar, re.S
    )
    if not dispatch:
        problems.append("sidebar.rs 找不到 `let body: AnyElement = match ctx` 分派块")
    else:
        for ctx in ("Library", "Tags", "Settings", "Duplicates"):
            if f"SidebarContext::{ctx} =>" not in dispatch.group(1):
                problems.append(f"侧栏 body 分派缺少 SidebarContext::{ctx} 分支")

    # 3) 活动栏只在 sidebar.rs 构造，app.rs 不得复制。
    checks += 1
    if '"activity-bar"' in app:
        problems.append("app.rs 出现 activity-bar，活动栏被重复构造")

    # 4) 资源管理器专属菜单动作只在 Library 分支，其余上下文给出返回图库。
    checks += 1
    menu = re.search(
        r"let mut more_menu = Menu::new\(\);(.*?)\n        let panel = div\(\)",
        sidebar,
        re.S,
    )
    if not menu:
        problems.append("sidebar.rs 找不到按上下文构造的 more_menu")
    else:
        block = menu.group(1)
        if 'common.back_to_library' not in block:
            problems.append("非 Library 上下文的溢出菜单缺「返回图库」")
        library_branch = block.split("} else {")[0]
        for key in ("sidebar.more.add_folder", "sidebar.more.collapse_all"):
            if key not in library_branch:
                problems.append(f"{key} 应只出现在 Library 上下文菜单分支")

    return problems, checks


def check_root_focus_scope() -> tuple[list[str], int]:
    """Ely 的 Tab 绑定挂在 FocusScope 的 ElyFocus 上下文；根必须用 .root() + .node()。"""
    app = (UI / "app.rs").read_text(encoding="utf-8")
    lib = (UI / "lib.rs").read_text(encoding="utf-8")
    problems: list[str] = []
    checks = 0

    checks += 1
    if "root_focus: FocusHandle" not in app or "root_focus: cx.focus_handle()" not in app:
        problems.append("KanalibApp 缺少 root_focus 字段或初始化")

    checks += 1
    if "FocusScope::new(&self.root_focus)" not in app:
        problems.append("app.rs 根视图没有 FocusScope::new(&self.root_focus)")

    checks += 1
    scope = re.search(r"FocusScope::new\(&self\.root_focus\)(.*?)\)\s*\.size_full\(\)", app, re.S)
    block = scope.group(1) if scope else ""
    if ".root()" not in block:
        problems.append("根 FocusScope 缺少 .root()")
    if '.node("kanalib-root-focus"' not in block:
        problems.append("根 FocusScope 缺少 .node(...)，Tab 动作分发路径会断在无 id 的根节点")

    checks += 1
    if "FocusScope" not in lib:
        problems.append("lib.rs 未导入 FocusScope")

    return problems, checks


def check_palette_keys() -> tuple[list[str], int]:
    app = (UI / "app.rs").read_text(encoding="utf-8")
    shown = set(re.findall(r'\.keys\("([^"]+)"\)', app))
    bound = set(re.findall(r'KeyBinding::new\("([^"]+)"', app))
    missing = sorted(shown - bound)
    problems = [f"命令面板键位 {key!r} 没有 cx.bind_keys 真实绑定" for key in missing]
    return problems, len(shown)


def check_core_status_i18n() -> tuple[list[str], int]:
    """core 状态必须结构化；notice.* 键必须在中英文目录同时存在。"""
    problems: list[str] = []
    checks = 0
    core_files = [CORE / "state.rs", CORE / "watch.rs", CORE / "model.rs"]
    keys: set[str] = set()
    for path in core_files:
        rel = path.relative_to(ROOT).as_posix()
        lines = lines_of(path)
        checks += 1
        for i, line in enumerate(lines):
            if re.search(r"\b(?:shared|shared_status)\.status\s*=", line):
                problems.append(
                    f"{rel}:{i + 1} 裸写 status；必须走 set_status!/set_status_message/set_plain_status"
                )
        keys |= set(re.findall(r'"(notice\.[a-z0-9_.]+)"', path.read_text(encoding="utf-8")))

    catalog = STATUS_CATALOG.read_text(encoding="utf-8")
    zh_part = catalog.split("pub(crate) const EN", 1)[0]
    en_part = "pub(crate) const EN" + catalog.split("pub(crate) const EN", 1)[1]
    zh = set(re.findall(r'"(notice\.[a-z0-9_.]+)"', zh_part))
    en = set(re.findall(r'"(notice\.[a-z0-9_.]+)"', en_part))

    checks += 1
    for key in sorted(keys - zh):
        problems.append(f"core 使用的 {key} 缺中文目录")
    for key in sorted(keys - en):
        problems.append(f"core 使用的 {key} 缺英文目录")
    checks += 1
    if zh != en:
        for key in sorted(zh - en):
            problems.append(f"{key} 只有中文目录")
        for key in sorted(en - zh):
            problems.append(f"{key} 只有英文目录")
    checks += 1
    for key in sorted(zh - keys):
        problems.append(f"目录中的 {key} 未被 core 使用（键名漂移？）")
    return problems, checks


def check_direct_ui_text() -> tuple[list[str], int]:
    """直接传给 UI 构造器/ toast 的中文字面量必须走目录（is_english 分支除外）。"""
    cjk = "[\u4e00-\u9fff]"
    patterns = {
        "child": re.compile(r'\.child\(\s*"([^"\n]*' + cjk + r'[^"\n]*)"'),
        "push_toast": re.compile(
            r'push_toast\([^;]{0,300}?"([^"\n]*' + cjk + r'[^"\n]*)"', re.S
        ),
        "button": re.compile(
            r'Button::new\([^;]{0,250}?,\s*"([^"\n]*' + cjk + r'[^"\n]*)"', re.S
        ),
        "menu": re.compile(
            r'MenuItem::new\([^;]{0,200}?"([^"\n]*' + cjk + r'[^"\n]*)"', re.S
        ),
        "command": re.compile(
            r'Command::new\([^;]{0,200}?"([^"\n]*' + cjk + r'[^"\n]*)"', re.S
        ),
        "placeholder": re.compile(
            r'\.(?:placeholder|least)\([^;]{0,120}?"([^"\n]*' + cjk + r'[^"\n]*)"',
            re.S,
        ),
        "tooltip": re.compile(
            r'\.tooltip\([^;]{0,120}?"([^"\n]*' + cjk + r'[^"\n]*)"', re.S
        ),
        "empty_state": re.compile(
            r'empty_state\([^;]{0,300}?"([^"\n]*' + cjk + r'[^"\n]*)"', re.S
        ),
    }
    problems: list[str] = []
    checks = 0
    for path in sorted(UI.rglob("*.rs")):
        rel = path.relative_to(UI).as_posix()
        if rel.startswith("localization"):
            continue
        text = path.read_text(encoding="utf-8")
        for kind, pattern in patterns.items():
            for match in pattern.finditer(text):
                checks += 1
                if "is_english()" in match.group(0):
                    continue
                line_no = text[: match.start(1)].count("\n") + 1
                problems.append(
                    f"{rel}:{line_no} 直接传入中文 {kind} 文案 {match.group(1)!r}；改用 t()/t_args()"
                )
    return problems, checks


def check_source_error_structure() -> tuple[list[str], int]:
    """D5：来源错误结构化、恢复判定类型化、去重固定原因可翻译。"""
    problems: list[str] = []
    checks = 0
    for path in (CORE / "state.rs", CORE / "watch.rs"):
        rel = path.relative_to(ROOT).as_posix()
        text = path.read_text(encoding="utf-8")
        checks += 1
        for match in re.finditer(r"status\.error\s*=\s*Some\(\s*format!", text):
            line_no = text[: match.start()].count("\n") + 1
            problems.append(
                f"{rel}:{line_no} 来源错误仍裸写中文；改用 StatusMessage::keyed/plain"
            )
        checks += 1
        for match in re.finditer(r'contains\("(?:监听|离线)"\)', text):
            line_no = text[: match.start()].count("\n") + 1
            problems.append(
                f"{rel}:{line_no} 恢复/重试判定仍匹配错误文案；改用 status.watch_retry"
            )
    checks += 1
    dedup_core = (CORE / "dedup.rs").read_text(encoding="utf-8")
    dedup_ui = (UI / "dedup.rs").read_text(encoding="utf-8")
    for reason in ("保护路径", "分辨率最高", "文件更大", "最早导入", "路径最短", "标签最多"):
        if f'"{reason}"' in dedup_core and f'"{reason}"' not in dedup_ui:
            problems.append(f"core 去重保留原因 {reason} 在 UI 映射里缺失")
    return problems, checks


def check_ui_message_keys() -> tuple[list[str], int]:
    """UI 使用的消息键必须在中英目录同时存在；缺失键会让 vendor i18n panic。"""
    key_pattern = re.compile(r'"([a-z][a-z0-9_]*(?:\.[a-z0-9_]+)+)"')
    zh: set[str] = set()
    en: set[str] = set()
    catalog_files = [UI / "localization.rs"] + sorted((UI / "localization").rglob("*.rs"))
    for path in catalog_files:
        source = path.read_text(encoding="utf-8")
        split = re.search(r"pub\(crate\) const EN", source)
        if not split:
            continue  # extra/mod.rs 等无目录文件
        zh |= set(key_pattern.findall(source[: split.start()]))
        en |= set(key_pattern.findall(source[split.start():]))

    used: dict[str, str] = {}
    call_patterns = [
        re.compile(r"t\(\s*cx\s*,\s*\"([^\"]+)\""),
        re.compile(r"t_args\(\s*cx\s*,\s*\"([^\"]+)\""),
        re.compile(r"\.placeholder_key\([^)]*?\"([^\"]+)\""),
    ]
    checks = 0
    for path in sorted(UI.rglob("*.rs")):
        rel = path.relative_to(UI).as_posix()
        if rel.startswith("localization"):
            continue
        text = path.read_text(encoding="utf-8")
        for pattern in call_patterns:
            for match in pattern.finditer(text):
                checks += 1
                used.setdefault(match.group(1), rel)
    problems = [
        f"{rel} 使用的键 {key!r} 缺目录条目（会 panic）"
        for key, rel in sorted(used.items())
        if key not in zh or key not in en
    ]
    return problems, checks


def main() -> int:
    checks = [
        ("手搓 role(Role::Button) 元素（id/aria/键盘可达）", *check_role_buttons()),
        ("open_asset_with_system 失败分支（toast + 就地错误）", *check_open_errors()),
        ("设置页分区标签与滚动锚点一一对应", *check_settings_anchors()),
        ("上下文侧栏结构（单搜索/四上下文/单一活动栏与菜单）", *check_sidebar_contexts()),
        ("根 FocusScope（.root() + .node()，保证 Tab 遍历）", *check_root_focus_scope()),
        ("命令面板键位（必须真实绑定）", *check_palette_keys()),
        ("core 状态 i18n（结构化写入 + 双语键齐全）", *check_core_status_i18n()),
        ("直接 UI 文案（不得硬编码中文）", *check_direct_ui_text()),
        ("D5 来源错误结构化（key/类型化恢复/去重原因映射）", *check_source_error_structure()),
        ("UI 消息键（t/t_args/placeholder_key 双语齐全）", *check_ui_message_keys()),
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
