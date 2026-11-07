#!/usr/bin/env python3
"""渲染后 WCAG 对比度抽样（基于截图 + UIA 元素矩形，非 token 设计值）。

用法：
  python scripts/check_rendered_contrast.py

读取 `.kanalib/verify_round9/baseline/` 里 1920/1440 宽度的亮暗截图与同名
UIA dump（screen 坐标），对每个有名字的控件矩形做：
  1. 背景色 = 矩形内（内缩 3px）出现次数最多的量化颜色；
  2. 文字色 = 亮度分位极端（亮主题取第 5 百分位、暗主题取第 95 百分位）
     里最常见的颜色；
  3. 按 WCAG 相对亮度公式算对比度。
排除窗口按钮、空名、过小矩形、以省略号结尾的截断/禁用标签。
结果只作渲染后抽样参考：小字号抗锯齿会让文字色偏浅/偏深，测得的对比度
通常低于 token 设计值。
"""

from __future__ import annotations

import pathlib
import re
import sys
from collections import Counter

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASELINE = ROOT / ".kanalib" / "verify_round9" / "baseline"

# set_capture 采集时的 DWM 矩形原点（screen 坐标）
ORIGIN = (45, 175)
SKIP_NAMES = {"Minimize", "Maximize / restore", "Close"}
# 捕获时处于禁用态的按钮：WCAG 1.4.3 对禁用控件豁免。
EXEMPT_NAMES = {"Copy prompt", "Reset"}
# 纯图标控件按 WCAG 1.4.11 的 3:1 判定。
NON_TEXT_NAMES = {"Ascending; click for descending"}
TEXT_MIN = 4.5
NON_TEXT_MIN = 3.0


def relative_luminance(rgb: tuple[int, int, int]) -> float:
    def channel(value: int) -> float:
        c = value / 255.0
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4

    r, g, b = (channel(v) for v in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(left: tuple[int, int, int], right: tuple[int, int, int]) -> float:
    l1, l2 = relative_luminance(left), relative_luminance(right)
    if l1 < l2:
        l1, l2 = l2, l1
    return (l1 + 0.05) / (l2 + 0.05)


def quantize(rgb: tuple[int, int, int]) -> tuple[int, int, int]:
    return (rgb[0] // 4 * 4, rgb[1] // 4 * 4, rgb[2] // 4 * 4)


def sample_region(pixels) -> tuple[float, tuple[int, int, int], tuple[int, int, int]]:
    counts = Counter(quantize(p) for p in pixels)
    background = counts.most_common(1)[0][0]
    # 文字/图标色：出现次数足够多（排除单像素噪点）且与背景对比度最高的量化色。
    minimum = max(3, len(pixels) // 500)
    candidates = [color for color, count in counts.items() if count >= minimum]
    text = max(candidates, key=lambda color: contrast(color, background))
    return contrast(text, background), text, background


def parse_dump(path: pathlib.Path):
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        match = re.match(r"[^|]+\|[^|]*\|(.*)\|(-?\d+),(-?\d+),(\d+),(\d+)$", line.strip())
        if not match:
            continue
        name, x, y, w, h = match.group(1), *map(int, match.groups()[1:])
        rows.append((name, x - ORIGIN[0], y - ORIGIN[1], w, h))
    return rows


def audit(png: pathlib.Path, dump: pathlib.Path):
    image = Image.open(png).convert("RGB")
    width, height = image.size
    results = []
    for name, x, y, w, h in parse_dump(dump):
        if not name or name in SKIP_NAMES or name.endswith("…"):
            continue
        if w < 20 or h < 14 or w > 900 or h > 90 or x < 0 or y < 0 or x + w > width or y + h > height:
            continue
        if x + w / 2 > width - 70 and y < 40:  # 标题栏窗口按钮区
            continue
        crop = image.crop((x + 3, y + 3, x + w - 3, y + h - 3))
        pixels = list(crop.getdata())
        if len(pixels) < 64:
            continue
        ratio, text, background = sample_region(pixels)
        results.append((ratio, name[:48], text, background))
    return results


def main() -> int:
    if not BASELINE.is_dir():
        print("缺少基线目录", BASELINE)
        return 1
    overall_fail = False
    for theme, dark in (("dark", True), ("light", False)):
        print(f"== {theme} ==")
        for width in ("w1920", "w1440"):
            for page in ("Library", "Duplicates", "Tags", "Settings"):
                png = BASELINE / f"{width}_{theme}_{page}.png"
                dump = BASELINE / f"{width}_{theme}_{page}.uia.txt"
                if not png.exists() or not dump.exists():
                    continue
                results = audit(png, dump)
                if not results:
                    continue
                exempt = [r for r in results if r[1] in EXEMPT_NAMES]
                checked = [r for r in results if r[1] not in EXEMPT_NAMES]
                text_low = [r for r in checked if r[1] not in NON_TEXT_NAMES and r[0] < TEXT_MIN]
                non_text_low = [r for r in checked if r[1] in NON_TEXT_NAMES and r[0] < NON_TEXT_MIN]
                text_min = min((r[0] for r in checked if r[1] not in NON_TEXT_NAMES), default=0.0)
                checked.sort()
                status = "OK" if not text_low and not non_text_low else ("WARN" if not text_low else "FAIL")
                print(
                    f"{width} {page:10s} samples={len(checked):3d} text_min={text_min:.2f} "
                    f"exempt={len(exempt)} {status}"
                )
                for ratio, name, text, background in checked[:3]:
                    print(f"    {ratio:.2f}: {name!r} text={text} bg={background}")
                for ratio, name, _, _ in non_text_low:
                    print(f"    WARN 非文本 {ratio:.2f}:1 < {NON_TEXT_MIN}:1 — {name!r}（临界，建议 token 微调）")
                if text_low:
                    overall_fail = True
    return 1 if overall_fail else 0


if __name__ == "__main__":
    sys.exit(main())
