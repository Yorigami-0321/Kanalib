#!/usr/bin/env python3
r"""图片墙静态导出目录的契约 v1 校验（F-080 -> F-081 接口的验收入口）。

用法：
    python scripts/wall_manifest_check.py .kanalib/wall_export
    python scripts/wall_manifest_check.py web/public/data --max-errors 80

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
  5) 导出目录内的文本文件不得出现本地绝对路径（盘符、\Users\、/home/、/Users/）；
  6) search-index.json（契约 §2.1）：v=1；tags 升序去重且覆盖分片全部标签；
     groups 升序去重且不含空串；items 行数与分片条目数一致，
     每行 [id, n, [tagIdx...], groupIdx, st, rt] 逐项与分片对齐：
     tagIdx 升序去重、不越界、标签集合与该条一致；未分组 groupIdx=-1；
     st 为 0-5 整数；rt 为 g/s/q/e/空。
     文件缺失只记警告（旧发布器产物），不影响退出码；同时打印原始与 gzip 体积。

退出码：0 通过；1 契约违规；2 环境问题（目录/index.json 缺失或 JSON 解析失败）。
只用标准库，无第三方依赖；不写任何文件、不修改导出目录（纯只读校验）。
重复 id、未知标签类别、t256 缺失等「不致命但值得知道」的情况记为警告，不影响退出码。
"""

from __future__ import annotations

import argparse
import gzip
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
    defaults = {"t256": "t256/", "t512": "t512/", "t1024": "t1024/", "t2048": "t2048/", "original": "o/"}
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
        if f"{item_id}.jpg" in files["t1024"]:
            stats["t1024_ok"] += 1
        if f"{item_id}.jpg" in files["t2048"]:
            stats["t2048_ok"] += 1
        if orig_ext is not None:
            if f"{item_id}.{orig_ext}" in files["orig"]:
                stats["orig_ok"] += 1
            else:
                report.warn(where, "orig", f"声明了原图 {orig_ext} 但 o/{item_id}.{orig_ext} 不存在（前端会回退 t512）")
                stats["orig_missing"] += 1

    # 契约可选字段 sharp：0 / 1024 / 2048；声明了档位就必须有文件。
    if "sharp" in item:
        sharp = item["sharp"]
        if isinstance(sharp, bool) or not isinstance(sharp, int) or sharp not in (0, 1024, 2048):
            report.error(where, "sharp", f"必须是 0 / 1024 / 2048，实际 {brief(sharp)}")
        else:
            stats["sharp_declared"] += 1
            if sharp >= 1024:
                stats["sharp_1024"] += 1
                if item_id is not None and f"{item_id}.jpg" not in files["t1024"]:
                    report.error(where, "sharp", f"声明 t1024 但文件不存在：t1024/{item_id}.jpg")
            if sharp >= 2048:
                stats["sharp_2048"] += 1
                if item_id is not None and f"{item_id}.jpg" not in files["t2048"]:
                    report.error(where, "sharp", f"声明 t2048 但文件不存在：t2048/{item_id}.jpg")
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


SEARCH_DETAIL_LIMIT = 5  # 逐条错误只打前几条，其余聚合成一条，避免 937 条刷屏
SEARCH_RATINGS = ("g", "s", "q", "e", "")


def check_item_tags(
    value: object, tags: list[str], where: str, report: Report
) -> set[str] | None:
    """校验一行的 tagIdx 数组：整数、严格升序（已去重）、不越界；返回标签名集合。"""
    if not isinstance(value, list):
        report.error(where, "tags", f"必须是数组，实际 {type(value).__name__}")
        return None
    names: set[str] = set()
    previous = -1
    for position, index in enumerate(value):
        if not isinstance(index, int) or isinstance(index, bool):
            report.error(where, "tags", f"第 {position} 项必须是整数，实际 {type(index).__name__}")
            return None
        if index <= previous:
            report.error(
                where, "tags", f"第 {position} 项 {index} 未严格升序（上一个是 {previous}；同条标签必须去重）"
            )
            return None
        if index >= len(tags):
            report.error(where, "tags", f"第 {position} 项下标 {index} 越界（tags 只有 {len(tags)} 项）")
            return None
        previous = index
        names.add(tags[index])
    return names


def check_search_index(
    root: pathlib.Path,
    report: Report,
    summaries: list[dict],
    stats: dict[str, int],
) -> None:
    """校验 search-index.json（契约 §2.1/§7）：共享表、行对齐、下标与集合一致性。

    summaries 是按分片顺序收集的清单摘要（id/n/rt/g/st/tags），只读现有分片，用来逐项对齐。
    文件缺失只记警告：旧发布器产物仍应通过其余校验。
    """
    path = root / "search-index.json"
    if not path.is_file():
        report.warn("search-index.json", "文件", "不存在（旧发布器产物；用 --index-only 生成）")
        return
    try:
        raw = path.read_bytes()
    except OSError as exc:
        report.error("search-index.json", "文件", f"读取失败：{exc}")
        return
    stats["search_bytes"] = len(raw)
    try:
        stats["search_gzip_bytes"] = len(gzip.compress(raw, 6, mtime=0))
    except (OSError, ValueError):
        pass
    payload = load_json(path, report, "search-index.json")
    if not isinstance(payload, dict):
        return

    version = payload.get("v")
    if not isinstance(version, int) or isinstance(version, bool):
        report.error("search-index.json", "v", f"必须是整数，实际 {type(version).__name__}")
    elif version != 1:
        report.error("search-index.json", "v", f"当前契约只有 v1，实际 {version}")

    count = len(summaries)
    tags = payload.get("tags")
    if not isinstance(tags, list):
        report.error("search-index.json", "tags", f"必须是数组，实际 {type(tags).__name__}")
        tags = []
    elif any(not isinstance(name, str) or not name for name in tags):
        report.error("search-index.json", "tags", "必须是标签名（非空字符串）数组")
        tags = [name for name in tags if isinstance(name, str) and name]
    previous = None
    for position, name in enumerate(tags):
        if previous is not None and name <= previous:
            report.error(
                "search-index.json",
                "tags",
                f"必须升序且去重：第 {position} 项 {brief(name)} 不大于上一项 {brief(previous)}",
            )
            break
        previous = name
    stats["search_tags"] = len(tags)

    groups = payload.get("groups")
    if not isinstance(groups, list):
        report.error("search-index.json", "groups", f"必须是数组，实际 {type(groups).__name__}")
        groups = []
    else:
        for position, name in enumerate(groups):
            if not isinstance(name, str) or not name:
                report.error(
                    "search-index.json",
                    f"groups[{position}]",
                    "必须是非空字符串（未分组用 groupIdx=-1，不写空串）",
                )
                groups = []
                break
        for position, name in enumerate(groups):
            if position > 0 and name <= groups[position - 1]:
                report.error(
                    "search-index.json",
                    "groups",
                    f"必须升序且去重：第 {position} 项 {brief(name)} 不大于上一项 {brief(groups[position - 1])}",
                )
                break

    rows = payload.get("items")
    if not isinstance(rows, list):
        report.error("search-index.json", "items", f"必须是数组，实际 {type(rows).__name__}")
        return
    if len(rows) != count:
        report.error("search-index.json", "items", f"行数 {len(rows)} 与分片条目数 {count} 不一致")

    details = 0
    for position, summary in enumerate(summaries):
        if details >= SEARCH_DETAIL_LIMIT or position >= len(rows):
            break
        where = f"search-index.json item[{position}]"
        row = rows[position]
        if not isinstance(row, list) or len(row) != 6:
            details += 1
            report.error(
                where, "items", "每行必须是 [id, n, [tagIdx...], groupIdx, st, rt]（6 项）"
            )
            continue
        item_id, name, indices, group_index, star, rating = row
        if summary["id"] is not None and item_id != summary["id"]:
            details += 1
            report.error(where, "id", f"{brief(item_id)} 与分片条目 id {brief(summary['id'])} 不一致")
            continue
        if summary["n"] is not None and name != summary["n"]:
            details += 1
            report.error(where, "n", f"{brief(name)} 与分片条目 n {brief(summary['n'])} 不一致")
            continue
        if not isinstance(rating, str) or rating not in SEARCH_RATINGS:
            details += 1
            report.error(where, "rt", f"必须是 g/s/q/e/空，实际 {brief(rating)}")
            continue
        if summary["rt"] is not None and rating != str(summary["rt"]).strip().lower():
            details += 1
            report.error(where, "rt", f"{brief(rating)} 与分片条目 rt {brief(summary['rt'])} 不一致")
            continue
        if not isinstance(star, int) or isinstance(star, bool) or not 0 <= star <= 5:
            details += 1
            report.error(where, "st", f"必须是 0-5 的整数，实际 {brief(star)}")
            continue
        if (
            isinstance(summary["st"], int)
            and not isinstance(summary["st"], bool)
            and star != summary["st"]
        ):
            details += 1
            report.error(where, "st", f"{star} 与分片条目 st {summary['st']} 不一致")
            continue
        if not isinstance(group_index, int) or isinstance(group_index, bool) or group_index < -1:
            details += 1
            report.error(where, "groupIdx", f"必须是 -1 或 groups 下标，实际 {brief(group_index)}")
            continue
        expected_group = summary["g"]
        if group_index == -1:
            if expected_group:
                details += 1
                report.error(
                    where, "groupIdx", f"-1 表示未分组，但分片条目分组是 {brief(expected_group)}"
                )
                continue
        elif group_index >= len(groups):
            details += 1
            report.error(where, "groupIdx", f"下标 {group_index} 越界（groups 只有 {len(groups)} 项）")
            continue
        elif groups[group_index] != expected_group:
            details += 1
            report.error(
                where,
                "groupIdx",
                f"指向 {brief(groups[group_index])}，但分片条目分组是 {brief(expected_group)}",
            )
            continue
        decoded_names = check_item_tags(indices, tags, where, report)
        if decoded_names is None:
            details += 1
            continue
        if summary["tags_ok"] and decoded_names != set(summary["tags"]):
            details += 1
            missing = sorted(set(summary["tags"]) - decoded_names)[:3]
            extra = sorted(decoded_names - set(summary["tags"]))[:3]
            report.error(
                where, "tags", f"与分片标签集合不一致：缺 {missing}、多 {extra}"
            )
    if details >= SEARCH_DETAIL_LIMIT and count > SEARCH_DETAIL_LIMIT:
        report.error(
            "search-index.json",
            "行对齐",
            f"至少 {SEARCH_DETAIL_LIMIT} 条出现错位，其余条目未逐项展开（先修前几条再重跑）",
        )

    # 共享表必须覆盖全库标签：既不能漏（长尾搜索失效），也不能多（死条目）。
    if summaries and all(summary["tags_ok"] for summary in summaries):
        expected_tags: set[str] = set()
        for summary in summaries:
            expected_tags.update(summary["tags"])
        actual_tags = set(tags)
        if expected_tags != actual_tags:
            report.error(
                "search-index.json",
                "tags",
                f"标签表与分片标签集合不一致：缺 {sorted(expected_tags - actual_tags)[:3]}、"
                f"多 {sorted(actual_tags - expected_tags)[:3]}",
            )


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
        "t256_ok", "t1024_ok", "t2048_ok", "sharp_declared", "sharp_1024", "sharp_2048",
        "facet_tags", "facet_groups", "text_files", "shards", "dups", "group_ref",
        "search_tags", "search_bytes", "search_gzip_bytes",
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
        "t1024": list_names(paths.get("t1024")),
        "t2048": list_names(paths.get("t2048")),
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
    summaries: list[dict] = []  # 搜索索引逐项对齐用的摘要（顺序 = 分片顺序）
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
            if isinstance(item, dict):
                raw_tags = item.get("tags")
                summaries.append({
                    "id": item_id,
                    "n": item.get("n"),
                    "rt": item.get("rt"),
                    "g": item.get("g") if isinstance(item.get("g"), str) else "",
                    "st": item.get("st"),
                    "tags_ok": isinstance(raw_tags, list),
                    "tags": [
                        tag[0]
                        for tag in raw_tags
                        if isinstance(tag, list) and tag and isinstance(tag[0], str)
                    ] if isinstance(raw_tags, list) else [],
                })
            else:
                summaries.append({
                    "id": item_id,
                    "n": None,
                    "rt": None,
                    "g": "",
                    "st": None,
                    "tags_ok": False,
                    "tags": [],
                })
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
    check_search_index(root, report, summaries, stats)

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
    t1024_ratio = (100.0 * stats["t1024_ok"] / stats["items"]) if stats["items"] else 0.0
    t2048_ratio = (100.0 * stats["t2048_ok"] / stats["items"]) if stats["items"] else 0.0
    print("\n—— 统计 ——")
    print(f"  分片          {stats['shards']} 片（shard_size={shard_size}，index.count={total_count}）")
    print(f"  条目          {stats['items']} 条，tags {stats['tags']} 个，fav {stats['fav']} 条，"
          f"ph {stats['phash']} 条，分组 {stats['group_ref']} 条")
    print(f"  t512 覆盖     {stats['t512_ok']}/{stats['items']}")
    print(f"  t256 覆盖     {stats['t256_ok']}/{stats['items']}（{t256_ratio:.1f}%，可缺）")
    print(f"  t1024 覆盖    {stats['t1024_ok']}/{stats['items']}（{t1024_ratio:.1f}%，可缺）")
    print(f"  t2048 覆盖    {stats['t2048_ok']}/{stats['items']}（{t2048_ratio:.1f}%，可缺）")
    print(f"  sharp 声明    {stats['sharp_declared']}/{stats['items']}（t1024 {stats['sharp_1024']}、"
          f"t2048 {stats['sharp_2048']}；缺失表示旧发布器）")
    print(f"  原图          声明 {stats['orig_ref']} 条，在位 {stats['orig_ok']} 个，缺 {stats['orig_missing']} 个")
    print(f"  facets.tags   {stats['facet_tags']} 项，facets.groups {stats['facet_groups']} 项")
    if stats["search_bytes"]:
        gzip_ratio = (100.0 * stats["search_gzip_bytes"] / stats["search_bytes"]) if stats["search_bytes"] else 0.0
        print(f"  search-index  {stats['search_tags']} 个标签，原始 {stats['search_bytes'] / 1024:.1f}KB，"
              f"gzip {stats['search_gzip_bytes'] / 1024:.1f}KB（{gzip_ratio:.1f}%）")
    else:
        print("  search-index  未生成（旧发布器产物；--index-only 可补）")
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
