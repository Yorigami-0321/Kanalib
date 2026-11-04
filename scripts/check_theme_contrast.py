#!/usr/bin/env python3
"""亮/暗主题文本对比度的静态 WCAG 2.2 AA 检查，不需要 GUI 或前台窗口。

用法：python scripts/check_theme_contrast.py
退出码 0 表示所有正文配对达到 AA(4.5:1)；1 表示有不达标项。

颜色取自 vendor Ely 的 `theme/palette.rs`（light/dark 的 `c(0xRRGGBB)` 常量），
算法照抄 `theme/contrast.rs` 的相对亮度与 `(L+0.05)/(D+0.05)`。
只解析不透明字段；带 alpha 的字段需要与背景合成，单独列出而不参与判定。
这检查的是 token 的**设计值**，不等同于渲染后的实测对比度。
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
PALETTE = (
    ROOT
    / "vendor"
    / "ely-gpui-components"
    / "crates"
    / "ely-core"
    / "src"
    / "theme"
    / "palette.rs"
)

# 正文/弱化文本落在常见容器底色上，规范要求不低于 4.5:1。
STRICT = [
    ("fg", "bg"),
    ("fg", "surface"),
    ("fg", "sunken"),
    ("fg", "overlay"),
    ("fg_muted", "bg"),
    ("fg_muted", "surface"),
    ("fg_muted", "sunken"),
    ("fg_muted", "overlay"),
    ("fg_subtle", "bg"),
    ("fg_subtle", "surface"),
    ("fg_subtle", "sunken"),
    ("fg_subtle", "overlay"),
    ("on_accent", "accent"),
    ("tooltip_fg", "tooltip_bg"),
    ("ink", "paper"),
    ("link", "bg"),
    ("link", "surface"),
]

# 不参与判定、仅记录：禁用态 WCAG 豁免；状态色多作图标/边框，按 3.0 记参考值。
INFO = [
    ("fg_disabled", "bg"),
    ("success", "success_subtle"),
    ("warning", "warning_subtle"),
    ("danger", "danger_subtle"),
    ("info", "info_subtle"),
]


def palettes() -> dict[str, dict[str, tuple[int, int, int]]]:
    src = PALETTE.read_text(encoding="utf-8")
    result: dict[str, dict[str, tuple[int, int, int]]] = {}
    for name in ("light", "dark"):
        match = re.search(
            r"pub fn " + name + r"\(high_contrast: bool\) -> Self \{(.*?)\n    \}",
            src,
            re.S,
        )
        if not match:
            raise RuntimeError(f"palette.rs 里找不到 {name}() 函数体")
        body = match.group(1)
        fields: dict[str, tuple[int, int, int]] = {}
        for key, value in re.findall(r"(\w+):\s*c\(0x([0-9a-fA-F]{6})\)", body):
            fields[key] = tuple(int(value[i : i + 2], 16) for i in (0, 2, 4))  # type: ignore[assignment]
        result[name] = fields
    return result


def luminance(rgb: tuple[int, int, int]) -> float:
    def linear(channel: int) -> float:
        value = channel / 255.0
        return value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4

    r, g, b = rgb
    return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)


def contrast(a: tuple[int, int, int], b: tuple[int, int, int]) -> float:
    hi, lo = sorted((luminance(a), luminance(b)), reverse=True)
    return (hi + 0.05) / (lo + 0.05)


def main() -> int:
    data = palettes()
    failures: list[str] = []
    for theme, fields in data.items():
        print(f"== {theme} ==")
        for fg, bg in STRICT + INFO:
            if fg not in fields or bg not in fields:
                print(f"  SKIP {fg}/{bg}（字段缺失）")
                continue
            ratio = contrast(fields[fg], fields[bg])
            strict = (fg, bg) in STRICT
            tag = "AA " if strict and ratio >= 4.5 else ("!! " if strict else "ref")
            print(f"  {tag} {fg:14s} / {bg:14s}  {ratio:5.2f}:1")
            if strict and ratio < 4.5:
                failures.append(f"{theme}: {fg}/{bg} = {ratio:.2f}:1（低于 AA 4.5）")
        print()
    if failures:
        print(f"FAIL  正文对比度 {len(failures)} 项未达 AA")
        for item in failures:
            print(f"        - {item}")
    else:
        print("PASS  亮/暗主题全部正文配对达到 WCAG 2.2 AA (4.5:1)")
    print("说明：5 个 rgba(alpha) 字段未参与判定；本检查是 token 设计值，不是渲染实测。")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
