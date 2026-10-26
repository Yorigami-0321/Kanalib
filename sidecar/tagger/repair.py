"""Re-encode a damaged image in tolerant mode. Never writes to the input path."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

os.environ.setdefault("PYTHONUTF8", "1")

from PIL import Image, ImageFile, ImageOps


def classify_error(exc: BaseException) -> tuple[str, str]:
    detail = f"{type(exc).__name__}: {exc}"
    text = detail.lower()
    if "truncated" in text or "unexpected end" in text or "eof" in text:
        return "文件不完整（图片数据被截断）", detail
    if (
        "cannot identify image file" in text
        or type(exc).__name__ == "UnidentifiedImageError"
    ):
        return "无法识别的图片，文件可能已损坏", detail
    if "decompression bomb" in text:
        return "图片尺寸超出安全上限", detail
    if isinstance(exc, FileNotFoundError):
        return "文件不存在或已被移动", detail
    if isinstance(exc, PermissionError):
        return "没有读取权限", detail
    return "图片解码失败，文件可能已损坏", detail


def load_tolerant(path: Path) -> tuple[Image.Image, str | None]:
    try:
        image = Image.open(path)
        image.load()
        return ImageOps.exif_transpose(image), None
    except Exception as strict_exc:
        _, strict_detail = classify_error(strict_exc)
    previous = ImageFile.LOAD_TRUNCATED_IMAGES
    ImageFile.LOAD_TRUNCATED_IMAGES = True
    try:
        image = Image.open(path)
        image.load()
        return ImageOps.exif_transpose(image), strict_detail
    finally:
        ImageFile.LOAD_TRUNCATED_IMAGES = previous


def flatten(image: Image.Image) -> Image.Image:
    if image.mode == "RGB":
        return image
    if image.mode in ("RGBA", "LA", "P"):
        rgba = image.convert("RGBA")
        canvas = Image.new("RGBA", rgba.size, (255, 255, 255, 255))
        canvas.alpha_composite(rgba)
        return canvas.convert("RGB")
    return image.convert("RGB")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--max-size", type=int, default=0)
    args = parser.parse_args()

    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    sys.stderr.reconfigure(encoding="utf-8", newline="\n")

    try:
        image, strict_error = load_tolerant(args.input)
        image = flatten(image)
        if args.max_size > 0:
            image.thumbnail((args.max_size, args.max_size), Image.Resampling.LANCZOS)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        suffix = args.output.suffix.lower()
        if suffix in (".jpg", ".jpeg"):
            image.save(args.output, format="JPEG", quality=92, optimize=True)
        elif suffix == ".png":
            image.save(args.output, format="PNG")
        else:
            raise ValueError("仅支持导出为 PNG 或 JPEG")
        damage = (
            f"{classify_error_text(strict_error)}，已按容错模式重编码"
            if strict_error
            else None
        )
        print(
            json.dumps(
                {
                    "ok": True,
                    "width": image.width,
                    "height": image.height,
                    "damage": damage,
                    "strict_error": strict_error,
                },
                ensure_ascii=False,
            ),
            flush=True,
        )
    except Exception as exc:
        label, detail = classify_error(exc)
        print(
            json.dumps(
                {
                    "ok": False,
                    "error": label,
                    "error_detail": detail,
                },
                ensure_ascii=False,
            ),
            flush=True,
        )
        print(f"[repair] {args.input}: {detail}", file=sys.stderr, flush=True)
        raise SystemExit(1)


def classify_error_text(detail: str) -> str:
    text = detail.lower()
    if "truncated" in text or "unexpected end" in text or "eof" in text:
        return "文件不完整（图片数据被截断）"
    if "cannot identify image file" in text or "unidentifiedimageerror" in text:
        return "无法识别的图片，文件可能已损坏"
    return "图片解码异常，文件可能已损坏"


if __name__ == "__main__":
    main()
