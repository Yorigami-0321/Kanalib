#!/usr/bin/env python3
r"""图片墙静态导出目录的契约 v1 校验（F-080 -> F-081 接口的验收入口）。

用法：
    python scripts/wall_manifest_check.py .kanalib/wall_export
    python scripts/wall_manifest_check.py --dir web/public/data --max-errors 80

校验口径以 docs/backlog/photo-wall-contract.md（v1）为准：
  1) index.json 必备 schema / count / shard_size / shards / paths 及类型；
     shards[].i 从 0 连续、count <= shard_size、path 相对且存在；
     index.count == 各片 items 数之和（同时核对各片自报 count）；
  2) item 必填 id / w / h / n / rt / st / fav / tags / mtime；
     id 为 40 或 64 位小写 hex；tags 为 [name, category, score] 三元组（score 0-1）；
     ph 若存在为 16 位小写 hex；orig 若存在为 [a-z0-9]+；
     g 若存在为分组名（string；契约可选字段，缺失不算错）；
  3) 每条 item 的 t512/<id>.jpg 必须存在（t256 可缺，只统计覆盖率）；
  4) facets.tags 按 count 降序、最多 2000 项（facets.groups 最多 200 项）；
  5) 导出目录内的文本文件不得出现本地绝对路径（盘符、\Users\、/home/、/Users/）。

退出码：0 通过；1 契约违规；2 环境问题（目录/index.json 缺失或 JSON 解析失败）。
只用标准库，无第三方依赖；不写任何文件、不修改导出目录（纯只读校验）。
重复 id、未知标签类别、t256 缺失等「不致命但值得知道」的情况记为警告，不影响退出码。
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import sys
import time

# ---- 契约常量（改动必须先改 docs/backlog/photo-wall-contract.md）----
ID_RE = re.compile(r"^(?:[0-9a-f]{40}|[0-9a-f]{64})$")
PHASH_RE = re.compile(r"^[0-9a-f]{16}$")
ORIG_RE = re.compile(r"^[a-z0-9]+$")
SHARD_REL_RE = re.compile(r"^m/\d{4,}\.json$")
DRIVE_RE = re.compile(r"^[A-Za-z]:")
RATED = ("G", "S", "Q", "E")
TAG_CATEGORIES = ("general", "character", "copyright", "artist", "manual", "meta")
FACET_TAG_LIMIT = 2000
FACET_GROUP_LIMIT = 200
DEFAULT_SHARD_SIZE = 5000

# 隐私扫描：隐私口径 W4 的落点，导出目录里任何文本都不许泄漏本机路径。
PRIVACY_PATTERNS = (
    (re.compile(r"(?<![A-Za-z0-9])[A-Za-z]:[\\/]"), "Windows 盘符"),
    (re.compile(r"\\Users\\", re.IGNORECASE), "Windows 用户目录"),
    (re.compile(r"/Users/", re.IGNORECASE), "macOS 用户目录"),
    (re.compile(r"/home/", re.IGNORECASE), "Linux 用户目录"),
    (re.compile(r"file://", re.IGNORECASE), "file:// 链接"),
)
DEFAULT_TEXT_SUFFIXES = ("json", "html", "htm", "txt")
MAX_SCAN_BYTES = 32 * 1024 * 1024  # 单个文本文件的扫描上限，超大文件只扫前半段


class Report:
    """收集错误/警告；打印按上限截断，计数不截断（避免刷屏但结果不撒谎）。"""

    def __init__(self, max_print: int) -> None:
        self.max_print = max_print
        self.lines_error: list[str] = []
        self.lines_warn: list[str] = []
        self.n_error = 0
        self.n_warn = 0

    def error(self, where: str, field: str, message: str) -> None:
        self.n_error += 1
        if len(self.lines_error) < self.max_print:
            self.lines_error.append(f"[错误] {where} 字段 {field}：{message}")

    def warn(self, where: str, field: str, message: str) -> None:
        self.n_warn += 1
        if len(self.lines_warn) < self.max_print:
            self.lines_warn.append(f"[警告] {where} 字段 {field}：{message}")


def is_num(value: object) -> bool:
    """JSON 意义上的 number；bool 在 Python 里是 int，必须排除。"""
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def brief(value: object, limit: int = 80) -> str:
    """错误信息里的值摘要，避免把整片 JSON 打到终端。"""
    try:
        text = json.dumps(value, ensure_ascii=False)
    except (TypeError, ValueError):
        text = repr(value)
    return text if len(text) <= limit else text[:limit] + "…"


def load_json(path: pathlib.Path, report: Report, where: str) -> object | None:
    try:
        raw = path.read_bytes()
    except OSError as exc:
        report.error(where, "文件", f"读取失败：{exc}")
        return None
    try:
        return json.loads(raw.decode("utf-8-sig"))
    except UnicodeDecodeError as exc:
        report.error(where, "编码", f"不是 UTF-8：{exc}")
    except json.JSONDecodeError as exc:
        report.error(where, "JSON", f"解析失败（第 {exc.lineno} 行第 {exc.colno} 列）：{exc.msg}")
    return None


def safe_child(root: pathlib.Path, rel: object) -> pathlib.Path | None:
    """把相对路径解析到导出目录内部；绝对路径、盘符、.. 逃逸一律拒绝。"""
    if not isinstance(rel, str) or not rel.strip():
        return None
    text = rel.replace("\\", "/").strip()
    if text.startswith("/") or DRIVE_RE.match(text):
        return None
    candidate = (root / text).resolve()
    try:
        candidate.relative_to(root.resolve())
    except ValueError:
        return None
    return candidate


def list_names(path: pathlib.Path | None) -> set[str]:
    """一次性列出目录内的文件名集合；目录不存在时返回空集（调用方负责报告）。"""
    if path is None or not path.is_dir():
        return set()
    try:
        with os.scandir(path) as entries:
            return {entry.name for entry in entries if entry.is_file()}
    except OSError:
        return set()


def check_paths_block(root: pathlib.Path, report: Report, paths: object) -> dict[str, pathlib.Path | None]:
    """校验 index.paths 并给出各档位目录的实际路径（缺省按契约第 1 节默认值兜底）。"""
    defaults = {"t256": "t256/", "t512": "t512/", "t1024": "t1024/", "original": "o/"}
    resolved: dict[str, pathlib.Path | None] = {}
    if not isinstance(paths, dict):
        report.error("index.json", "paths", f"必须是对象，实际 {type(paths).__name__}")
        paths = {}
    for key in ("t256", "t512"):
        if key not in paths:
            report.error("index.json", "paths." + key, "必备键缺失（档位前缀）")
    for key, default in defaults.items():
        value = paths.get(key, default)
        if not isinstance(value, str):
            report.error("index.json", f"paths.{key}", f"必须是字符串，实际 {type(value).__name__}")
            value = default
        if value.startswith("/") or DRIVE_RE.match(value):
            report.error("index.json", f"paths.{key}", f"必须是相对站点根的路径，实际 {brief(value)}")
        if not value.endswith("/"):
            report.warn("index.json", f"paths.{key}", f"目录前缀通常以 / 结尾，实际 {brief(value)}")
        resolved[key] = safe_child(root, value)
        if resolved[key] is None and key in ("t256", "t512"):
            report.error("index.json", f"paths.{key}", f"无法解析到导出目录内部：{brief(value)}")
    return resolved


def check_item(
    report: Report,
    where: str,
    item: object,
    files: dict[str, set[str]],
    stats: dict[str, int],
) -> str | None:
    """校验单条 item；返回合法 id（不合法或缺失时返回 None）。"""
    if not isinstance(item, dict):
        report.error(where, "(item)", f"必须是 JSON 对象，实际 {type(item).__name__}")
        return None
    stats["items"] += 1

    # 契约 §4 标注「必有」的字段；mtime 不在任务点名清单里，但契约表同样标了必有，
    # 前端按 mtime 兜底排序，缺了会静默乱序，所以按契约判错。
    for key in ("id", "w", "h", "n", "rt", "st", "fav", "tags", "mtime"):
        if key not in item:
            report.error(where, key, "必填字段缺失")
            stats["missing"] += 1

    item_id: str | None = None
    if "id" in item:
        value = item["id"]
        if not isinstance(value, str):
            report.error(where, "id", f"必须是字符串，实际 {type(value).__name__}")
        elif not ID_RE.match(value):
            hint = "长度 {} 位".format(len(value))
            report.error(where, "id", f"必须是 40 或 64 位小写 hex（{hint}）：{brief(value)}")
        else:
            item_id = value

    if "w" in item:
        value = item["w"]
        if not is_num(value):
            report.error(where, "w", f"必须是数字，实际 {type(value).__name__}")
        elif value <= 0:
            report.error(where, "w", f"像素宽必须 > 0，实际 {value}")
    if "h" in item:
        value = item["h"]
        if not is_num(value):
            report.error(where, "h", f"必须是数字，实际 {type(value).__name__}")
        elif value <= 0:
            report.error(where, "h", f"像素高必须 > 0，实际 {value}")

    if "n" in item:
        value = item["n"]
        if not isinstance(value, str) or not value:
            report.error(where, "n", "展示名必须是非空字符串")
        elif "/" in value or "\\" in value:
            report.error(where, "n", f"展示名必须是 basename（不许含路径分隔符）：{brief(value)}")
        elif DRIVE_RE.match(value):
            report.error(where, "n", f"展示名疑似本地绝对路径：{brief(value)}")
        else:
            stats["named"] += 1

    if "rt" in item:
        value = item["rt"]
        if not isinstance(value, str):
            report.error(where, "rt", f"必须是字符串，实际 {type(value).__name__}")
        elif value not in RATED:
            report.error(where, "rt", f"必须属于 {'/'.join(RATED)}，实际 {brief(value)}")

    if "st" in item:
        value = item["st"]
        if not is_num(value):
            report.error(where, "st", f"必须是数字，实际 {type(value).__name__}")
        elif value < 0 or value > 5:
            report.error(where, "st", f"星级必须 0-5，实际 {value}")
        else:
            stats["rated"] += 1

    if "fav" in item and not isinstance(item["fav"], bool):
        report.error(where, "fav", f"必须是布尔，实际 {type(item['fav']).__name__}")
    if item.get("fav") is True:
        stats["fav"] += 1

    if "mtime" in item:
        value = item["mtime"]
        if not is_num(value):
            report.error(where, "mtime", f"必须是数字（UTC 秒），实际 {type(value).__name__}")
        elif value <= 0:
            report.warn(where, "mtime", f"UTC 秒不是正数，排序会退化：{value}")

    if "tags" in item:
        tags = item["tags"]
        if not isinstance(tags, list):
            report.error(where, "tags", f"必须是数组，实际 {type(tags).__name__}")
        else:
            stats["tags"] += len(tags)
            for index, tag in enumerate(tags):
                field = f"tags[{index}]"
                if not isinstance(tag, list):
                    report.error(where, field, f"必须是 [name, category, score] 数组，实际 {type(tag).__name__}")
                    continue
                if len(tag) != 3:
                    report.error(where, field, f"必须是三元组，实际 {len(tag)} 个元素：{brief(tag)}")
                    continue
                name, category, score = tag
                if not isinstance(name, str) or not name:
                    report.error(where, field + "[0]", "标签名必须是非空字符串")
                # 契约 v1：category 是库内原值透传，常见 general/character/... 但不限于此，
                # 只要是非空字符串即可；不认识的值只提醒，不当作违规。
                if not isinstance(category, str):
                    report.error(where, field + "[1]", f"类别必须是字符串，实际 {type(category).__name__}")
                elif not category.strip():
                    report.error(where, field + "[1]", "类别不能是空字符串")
                elif category not in TAG_CATEGORIES:
                    report.warn(where, field + "[1]", f"类别不是常见取值（允许透传自定义类别）：{brief(category)}")
                if not is_num(score):
                    report.error(where, field + "[2]", f"score 必须是数字，实际 {type(score).__name__}")
                elif score < 0 or score > 1:
                    report.warn(where, field + "[2]", f"score 应在 0-1，实际 {score}")

    if "ph" in item:
        value = item["ph"]
        if not isinstance(value, str) or not PHASH_RE.match(value):
            report.error(where, "ph", f"必须是 16 位小写 hex，实际 {brief(value)}")
        else:
            stats["phash"] += 1

    orig_ext: str | None = None
    if "orig" in item:
        value = item["orig"]
        if not isinstance(value, str) or not ORIG_RE.match(value):
            report.error(where, "orig", f"必须是小写扩展名（[a-z0-9]+，无点），实际 {brief(value)}")
        else:
            orig_ext = value
            stats["orig_ref"] += 1

    # 契约可选字段 g：分组名（assets.group 的展示值）。缺失合法，存在则必须是字符串。
    if "g" in item:
        value = item["g"]
        if not isinstance(value, str):
            report.error(where, "g", f"分组名必须是字符串，实际 {type(value).__name__}")
        elif not value.strip():
            report.warn(where, "g", "分组名为空字符串（前端应按无分组处理）")
        else:
            stats["group_ref"] += 1

    # 文件落位：t512 必存在；t256 统计覆盖；orig 引用时核对 o/<id>.<ext>。
    if item_id is not None:
        if f"{item_id}.jpg" in files["t512"]:
            stats["t512_ok"] += 1
        else:
            report.error(where, "t512", f"文件不存在：t512/{item_id}.jpg")
            stats["t512_missing"] += 1
        if f"{item_id}.jpg" in files["t256"]:
            stats["t256_ok"] += 1
        if orig_ext is not None:
            if f"{item_id}.{orig_ext}" in files["orig"]:
                stats["orig_ok"] += 1
            else:
                report.warn(where, "orig", f"声明了原图 {orig_ext} 但 o/{item_id}.{orig_ext} 不存在（前端会回退 t512）")
                stats["orig_missing"] += 1
    return item_id


def check_facets(report: Report, facets: object, stats: dict[str, int]) -> None:
    if not isinstance(facets, dict):
        report.warn("index.json", "facets", "缺失或不是对象（前端筛选 chips 会为空）")
        return
    tags = facets.get("tags")
    if not isinstance(tags, list):
        report.warn("index.json", "facets.tags", "缺失或不是数组")
    else:
        stats["facet_tags"] = len(tags)
        if len(tags) > FACET_TAG_LIMIT:
            report.error("index.json", "facets.tags", f"最多 {FACET_TAG_LIMIT} 项，实际 {len(tags)}")
        previous: float | None = None
        for index, entry in enumerate(tags):
            field = f"facets.tags[{index}]"
            if not isinstance(entry, dict):
                report.error("index.json", field, f"必须是对象，实际 {type(entry).__name__}")
                continue
            name = entry.get("name")
            category = entry.get("category")
            count = entry.get("count")
            if not isinstance(name, str) or not name:
                report.error("index.json", field + ".name", "必须是非空字符串")
            # 同 item.tags：库内原值透传，非空字符串即可，未知取值只提醒。
            if not isinstance(category, str):
                report.error("index.json", field + ".category", f"必须是字符串，实际 {type(category).__name__}")
            elif not category.strip():
                report.error("index.json", field + ".category", "类别不能是空字符串")
            elif category not in TAG_CATEGORIES:
                report.warn("index.json", field + ".category", f"类别不是常见取值（允许透传自定义类别）：{brief(category)}")
            if not is_num(count):
                report.error("index.json", field + ".count", f"必须是数字，实际 {type(count).__name__}")
                previous = None
                continue
            if previous is not None and count > previous:
                report.error(
                    "index.json",
                    field + ".count",
                    f"facets.tags 必须按 count 降序：第 {index} 项 {count} > 上一项 {previous}",
                )
            previous = count
    groups = facets.get("groups")
    if groups is not None:
        if not isinstance(groups, list):
            report.error("index.json", "facets.groups", f"必须是数组，实际 {type(groups).__name__}")
        else:
            stats["facet_groups"] = len(groups)
            if len(groups) > FACET_GROUP_LIMIT:
                report.error("index.json", "facets.groups", f"最多 {FACET_GROUP_LIMIT} 项，实际 {len(groups)}")
            for index, group in enumerate(groups):
                if not isinstance(group, str):
                    report.error("index.json", f"facets.groups[{index}]", f"必须是字符串，实际 {type(group).__name__}")


def scan_privacy(root: pathlib.Path, report: Report, suffixes: tuple[str, ...], stats: dict[str, int]) -> None:
    """扫描导出目录内的文本文件，命中本地绝对路径即判错（不变量：任何文件都不含本地路径）。"""
    total = 0
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        if path.suffix.lower().lstrip(".") not in suffixes:
            continue
        total += 1
        try:
            raw = path.read_bytes()[:MAX_SCAN_BYTES]
        except OSError as exc:
            report.warn(str(path.relative_to(root)), "文件", f"读取失败，跳过隐私扫描：{exc}")
            continue
        text = raw.decode("utf-8", errors="replace")
        for lineno, line in enumerate(text.splitlines(), start=1):
            for pattern, label in PRIVACY_PATTERNS:
                match = pattern.search(line)
                if match is None:
                    continue
                snippet = line.strip()
                if len(snippet) > 120:
                    snippet = snippet[:120] + "…"
                report.error(
                    f"{path.relative_to(root)}:{lineno}",
                    "隐私",
                    f"出现本地绝对路径（{label}）：{snippet}",
                )
                break
    stats["text_files"] = total


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="校验图片墙静态导出目录是否符合契约 v1（只读，不写任何文件）。",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument("dir", nargs="?", default=".kanalib/wall_export", help="导出目录（含 index.json）")
    parser.add_argument("--max-errors", type=int, default=40, help="终端最多打印多少条错误/警告")
    parser.add_argument(
        "--suffixes",
        default=",".join(DEFAULT_TEXT_SUFFIXES),
        help="参与本地路径扫描的文本扩展名（逗号分隔，不带点）",
    )
    parser.add_argument("--no-privacy", action="store_true", help="跳过本地绝对路径扫描")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:  # Windows 控制台默认代码页会打碎中文，这里强制 UTF-8
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass

    root = pathlib.Path(args.dir)
    if not root.is_dir():
        print(f"[环境] 导出目录不存在：{root}", file=sys.stderr)
        print("       先跑发布器：cargo run -p kanalib-publish -- --db .kanalib/kanalib.db --out "
              + str(root), file=sys.stderr)
        return 2
    index_path = root / "index.json"
    if not index_path.is_file():
        print(f"[环境] 缺少 {index_path}（导出目录不完整）", file=sys.stderr)
        return 2

    started = time.time()
    report = Report(args.max_errors)
    stats: dict[str, int] = {key: 0 for key in (
        "items", "missing", "named", "rated", "fav", "tags", "phash",
        "orig_ref", "orig_ok", "orig_missing", "t512_ok", "t512_missing",
        "t256_ok", "facet_tags", "facet_groups", "text_files", "shards", "dups", "group_ref",
    )}

    print(f"图片墙导出目录契约校验 v1 —— {root.resolve()}")

    index = load_json(index_path, report, "index.json")
    if not isinstance(index, dict):
        # index.json 都读不出来就没法继续逐项核对，按环境问题退出（报告里已写清原因）。
        for line in report.lines_error:
            print(line)
        print("[环境] index.json 不是合法 JSON 对象，后续校验无法进行", file=sys.stderr)
        return 2

    # ---- index.json 顶层 ----
    if "schema" not in index:
        report.error("index.json", "schema", "必备键缺失")
    elif not isinstance(index["schema"], int) or isinstance(index["schema"], bool):
        report.error("index.json", "schema", f"必须是整数，实际 {type(index['schema']).__name__}")
    elif index["schema"] != 1:
        report.error("index.json", "schema", f"当前契约只有 v1，实际 {index['schema']}")

    total_count = index.get("count")
    if "count" not in index:
        report.error("index.json", "count", "必备键缺失")
        total_count = None
    elif not isinstance(total_count, int) or isinstance(total_count, bool):
        report.error("index.json", "count", f"必须是整数，实际 {type(total_count).__name__}")
        total_count = None
    elif total_count < 0:
        report.error("index.json", "count", f"不能为负，实际 {total_count}")
        total_count = None

    shard_size = index.get("shard_size")
    if "shard_size" not in index:
        report.error("index.json", "shard_size", "必备键缺失")
        shard_size = DEFAULT_SHARD_SIZE
    elif not isinstance(shard_size, int) or isinstance(shard_size, bool) or shard_size <= 0:
        report.error("index.json", "shard_size", f"必须是正整数，实际 {brief(shard_size)}")
        shard_size = DEFAULT_SHARD_SIZE

    paths = check_paths_block(root, report, index.get("paths"))
    files = {
        "t256": list_names(paths.get("t256")),
        "t512": list_names(paths.get("t512")),
        "orig": list_names(paths.get("original")),
    }
    for key, label in (("t512", "t512/"), ("t256", "t256/")):
        if paths.get(key) is not None and not paths[key].is_dir():
            report.error("index.json", f"paths.{key}", f"目录不存在：{paths[key]}（{label}）")

    shards = index.get("shards")
    if "shards" not in index:
        report.error("index.json", "shards", "必备键缺失")
        shards = []
    elif not isinstance(shards, list):
        report.error("index.json", "shards", f"必须是数组，实际 {type(shards).__name__}")
        shards = []
    if isinstance(shards, list) and total_count is not None and not shards and total_count > 0:
        report.error("index.json", "shards", f"count={total_count} 但分片数组为空")

    # ---- 逐片校验 ----
    declared_sum = 0
    loaded_sum = 0
    seen_ids: dict[str, str] = {}
    for position, shard in enumerate(shards):
        field = f"shards[{position}]"
        if not isinstance(shard, dict):
            report.error("index.json", field, f"必须是对象，实际 {type(shard).__name__}")
            continue
        shard_index = shard.get("i")
        if not isinstance(shard_index, int) or isinstance(shard_index, bool):
            report.error("index.json", field + ".i", f"必须是整数，实际 {brief(shard_index)}")
        elif shard_index != position:
            report.error("index.json", field + ".i", f"i 必须从 0 连续递增：期望 {position}，实际 {shard_index}")
        shard_count = shard.get("count")
        if not is_num(shard_count) or shard_count != int(shard_count):
            report.error("index.json", field + ".count", f"必须是整数，实际 {brief(shard_count)}")
            shard_count = None
        elif shard_count > shard_size:
            report.error("index.json", field + ".count", f"count={shard_count} 超过 shard_size={shard_size}")
        else:
            declared_sum += int(shard_count)
        rel = shard.get("path")
        if not isinstance(rel, str):
            report.error("index.json", field + ".path", f"必须是字符串，实际 {brief(rel)}")
            continue
        if not SHARD_REL_RE.match(rel.replace("\\", "/")):
            report.warn("index.json", field + ".path", f"路径不是 m/NNNN.json 形态：{brief(rel)}")
        shard_path = safe_child(root, rel)
        if shard_path is None:
            report.error("index.json", field + ".path", f"必须是导出目录内的相对路径：{brief(rel)}")
            continue
        if not shard_path.is_file():
            report.error("index.json", field + ".path", f"文件不存在：{rel}")
            continue

        payload = load_json(shard_path, report, rel)
        if not isinstance(payload, dict):
            continue
        stats["shards"] += 1
        if payload.get("index") != position:
            report.error(rel, "index", f"分片自报 index={brief(payload.get('index'))} 与清单位置 {position} 不一致")
        if "schema" in payload and payload["schema"] != index.get("schema"):
            report.error(rel, "schema", f"与 index.json 的 schema={brief(index.get('schema'))} 不一致")
        items = payload.get("items")
        if not isinstance(items, list):
            report.error(rel, "items", f"必须是数组，实际 {type(items).__name__}")
            continue
        if shard_count is not None and len(items) != int(shard_count):
            report.error(rel, "items", f"长度 {len(items)} 与 index 声明的 count={int(shard_count)} 不一致")
        loaded_sum += len(items)

        for offset, item in enumerate(items):
            global_index = loaded_sum - len(items) + offset
            where = f"{rel} item[{offset}]（全局 #{global_index}）"
            item_id = check_item(report, where, item, files, stats)
            if item_id is not None:
                previous = seen_ids.get(item_id)
                if previous is not None:
                    stats["dups"] += 1
                    report.warn(where, "id", f"与 {previous} 重复（getItem 会覆盖，t512 文件名冲突）：{item_id}")
                else:
                    seen_ids[item_id] = where

    # ---- 计数一致性 ----
    if total_count is not None and declared_sum != total_count:
        report.error("index.json", "count", f"count={total_count} 与各片声明之和 {declared_sum} 不一致")
    if total_count is not None and loaded_sum != total_count:
        report.error("index.json", "count", f"count={total_count} 与实际 items 之和 {loaded_sum} 不一致")
    stats["items"] = loaded_sum

    check_facets(report, index.get("facets"), stats)

    if not args.no_privacy:
        suffixes = tuple(part.strip().lstrip(".").lower() for part in args.suffixes.split(",") if part.strip())
        scan_privacy(root, report, suffixes, stats)

    # ---- 输出 ----
    if report.lines_error:
        print("\n—— 错误明细 ——")
        for line in report.lines_error:
            print("  " + line)
        hidden = report.n_error - len(report.lines_error)
        if hidden > 0:
            print(f"  …… 另有 {hidden} 条错误未打印（--max-errors {report.max_print}）")
    if report.lines_warn:
        print("\n—— 警告明细 ——")
        for line in report.lines_warn:
            print("  " + line)
        hidden = report.n_warn - len(report.lines_warn)
        if hidden > 0:
            print(f"  …… 另有 {hidden} 条警告未打印（--max-errors {report.max_print}）")

    t256_ratio = (100.0 * stats["t256_ok"] / stats["items"]) if stats["items"] else 0.0
    print("\n—— 统计 ——")
    print(f"  分片          {stats['shards']} 片（shard_size={shard_size}，index.count={total_count}）")
    print(f"  条目          {stats['items']} 条，tags {stats['tags']} 个，fav {stats['fav']} 条，"
          f"ph {stats['phash']} 条，分组 {stats['group_ref']} 条")
    print(f"  t512 覆盖     {stats['t512_ok']}/{stats['items']}")
    print(f"  t256 覆盖     {stats['t256_ok']}/{stats['items']}（{t256_ratio:.1f}%，可缺）")
    print(f"  原图          声明 {stats['orig_ref']} 条，在位 {stats['orig_ok']} 个，缺 {stats['orig_missing']} 个")
    print(f"  facets.tags   {stats['facet_tags']} 项，facets.groups {stats['facet_groups']} 项")
    if not args.no_privacy:
        print(f"  隐私扫描      {stats['text_files']} 个文本文件")
    print(f"  重复 id       {stats['dups']} 条")
    print(f"  耗时          {time.time() - started:.2f}s")

    print("\n—— 结论 ——")
    if report.n_error:
        print(f"  ✗ 契约 v1 校验失败：{report.n_error} 条错误、{report.n_warn} 条警告")
        return 1
    print(f"  ✓ 契约 v1 校验通过（{report.n_warn} 条警告）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
