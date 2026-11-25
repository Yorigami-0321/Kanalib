from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import re
import sys
import threading
import warnings
from pathlib import Path

sys.dont_write_bytecode = True
os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
warnings.filterwarnings("ignore")

import torch
from PIL import Image, ImageFile, ImageOps
from torchvision.transforms import functional as vF
from transformers import AutoModel

THRESHOLD_KEYS = ("general", "character", "copyright", "style", "meta", "rating")
DEFAULT_THRESHOLDS = {
    "general": 0.17,
    "character": 0.27,
    "copyright": 0.24,
    "style": 0.15,
    "meta": 0.17,
    "rating": 0.41,
}
# 每类落库候选上限，与 Rust model.rs 的 candidate_limit 一致。
CANDIDATE_CAPS = {
    "general": 40,
    "character": 40,
    "copyright": 40,
    "style": 40,
    "meta": 40,
    "rating": 4,
}
CANDIDATE_FLOOR = 0.005
# 448 输入只需缩略图；显式限制像素数，避免超大图在 RGBA 画布/tensor 上吃满内存。
MAX_TAG_PIXELS = 32_000_000
# 单次推理的图片数上限；配合逐批加载，避免整批解码后同时驻留内存。
MAX_INFER_BATCH = 4

_DECODE_LOCK = threading.Lock()


def model_guard_hash(model_dir: Path) -> str:
    """Hash the executable surface of a model directory (`*.py` and `*.json`).

    `trust_remote_code=True` executes Python shipped next to the weights, so a
    swapped directory is arbitrary code execution. Pinning this digest makes
    that swap detectable without hashing multi-GB weight files.
    """
    digest = hashlib.sha256()
    for path in sorted(model_dir.rglob("*")):
        if not path.is_file() or path.suffix not in {".py", ".json"}:
            continue
        digest.update(path.relative_to(model_dir).as_posix().encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def classify_error(exc: BaseException) -> tuple[str, str, str]:
    detail = f"{type(exc).__name__}: {exc}"
    text = detail.lower()
    if "truncated" in text or "unexpected end" in text or "eof" in text:
        return "truncated", "文件不完整（图片数据被截断）", detail
    if (
        "cannot identify image file" in text
        or type(exc).__name__ == "UnidentifiedImageError"
    ):
        return "unreadable", "无法识别的图片，文件可能已损坏", detail
    if "decompression bomb" in text:
        return "too_large", "图片尺寸超出安全上限", detail
    if "too large" in text:
        return "too_large", "图片尺寸超出安全上限", detail
    if isinstance(exc, FileNotFoundError):
        return "missing", "文件不存在或已被移动", detail
    if isinstance(exc, PermissionError):
        return "denied", "没有读取权限", detail
    return "decode_failed", "图片解码失败，文件可能已损坏", detail


def load_rgb(
    path: Path, tolerant: bool = False, max_side: int = 2048
) -> Image.Image:
    # `LOAD_TRUNCATED_IMAGES` is process-global in Pillow; serialize the toggle
    # so concurrent decodes cannot observe each other's setting.
    with _DECODE_LOCK:
        previous = ImageFile.LOAD_TRUNCATED_IMAGES
        ImageFile.LOAD_TRUNCATED_IMAGES = tolerant
        try:
            img = Image.open(path)
            # JPEG/MPO can decode at reduced scale, so only non-draftable
            # formats need the pixel cap before decoding.
            draftable = img.format in {"JPEG", "MPO"}
            if not draftable and img.width * img.height > MAX_TAG_PIXELS:
                raise ValueError(f"image too large: {img.width}x{img.height}")
            # JPEG can decode at a reduced scale; do that before allocating the
            # full RGBA canvas for very large originals.
            if draftable and max(img.size) > max_side:
                img.draft("RGB", (max_side, max_side))
            img.load()
            if img.width * img.height > MAX_TAG_PIXELS:
                raise ValueError(f"image too large: {img.width}x{img.height}")
            img = ImageOps.exif_transpose(img)
            if img.mode != "RGB":
                img = img.convert("RGBA")
                canvas = Image.new("RGBA", img.size, (255, 255, 255, 255))
                canvas.alpha_composite(img)
                img = canvas.convert("RGB")
            return img
        finally:
            ImageFile.LOAD_TRUNCATED_IMAGES = previous


def preprocess(images: list[Image.Image], size: int) -> torch.Tensor:
    tensors = []
    for img in images:
        t = vF.to_tensor(img)
        h, w = t.shape[-2:]
        if h != size or w != size:
            scale = min(size / h, size / w)
            new_h, new_w = max(1, int(h * scale)), max(1, int(w * scale))
            pad_h, pad_w = size - new_h, size - new_w
            left, top = pad_w // 2, pad_h // 2
            t = vF.resize(t, [new_h, new_w])
            t = vF.pad(t, [left, top, pad_w - left, pad_h - top], 0)
        t = vF.normalize(t, [0.5, 0.5, 0.5], [0.5, 0.5, 0.5])
        tensors.append(t)
    return torch.stack(tensors)


def build_thresholds(
    config,
) -> tuple[torch.Tensor, dict[str, tuple[int, int]], dict[str, float]]:
    thresholds = torch.full((config.num_classes,), 0.2, dtype=torch.float32)
    ranges: dict[str, tuple[int, int]] = {}
    defaults: dict[str, float] = {}
    start = 0
    for category, count in config.tags_split:
        value = float(
            config.category_best_threshold.get(
                category, DEFAULT_THRESHOLDS.get(category, 0.2)
            )
        )
        thresholds[start : start + count] = value
        ranges[category] = (start, start + count)
        defaults[category] = value
        start += count
    if start != config.num_classes:
        raise RuntimeError(f"tag split totals {start}, expected {config.num_classes}")
    return thresholds, ranges, defaults


class Tagger:
    def __init__(self, model_dir: Path, device_name: str, half: bool):
        self.model_dir = model_dir
        self.device = torch.device(
            "cuda"
            if device_name == "auto" and torch.cuda.is_available()
            else device_name
        )
        if self.device.type == "cuda" and not torch.cuda.is_available():
            self.device = torch.device("cpu")
        self.half = bool(half and self.device.type == "cuda")

        expected_guard = os.environ.get("KANALIB_TAGGER_MODEL_SHA256", "").strip().lower()
        guard = model_guard_hash(model_dir)
        if expected_guard:
            if not hmac.compare_digest(guard, expected_guard):
                raise RuntimeError(
                    f"model directory guard mismatch: expected {expected_guard}, got {guard}"
                )
        else:
            print(
                f"[model] guard hash {guard}; set KANALIB_TAGGER_MODEL_SHA256 to pin it",
                file=sys.stderr,
                flush=True,
            )

        model = AutoModel.from_pretrained(
            model_dir,
            trust_remote_code=True,
            local_files_only=True,
            dtype=torch.float16 if self.half else torch.float32,
        )
        model.to(self.device)
        if self.half:
            model.half()
        model.eval()
        self.model = model
        self.config = model.config
        self.tags = list(model.config.tags)
        self.thresholds, self.ranges, self.default_thresholds = build_thresholds(
            model.config
        )
        start, end = self.ranges["character"]
        self.character_tags = set(self.tags[start:end])
        torch.set_grad_enabled(False)

    def _infer(self, images: list[Image.Image]) -> torch.Tensor:
        tensor = preprocess(images, int(self.config.img_size))
        tensor = tensor.to(
            self.device, dtype=torch.float16 if self.half else torch.float32
        )
        try:
            with torch.inference_mode():
                return self.model(tensor).sigmoid().float().cpu()
        except torch.cuda.OutOfMemoryError:
            torch.cuda.empty_cache()
            if len(images) > 1:
                parts = [self._infer([img]) for img in images]
                return torch.cat(parts)
            raise

    def _normalize_character(self, tag: str) -> str:
        """Fold outfit/form variants onto the base character when the base tag exists."""
        if "_(" in tag and tag.endswith(")"):
            base = tag.split("_(", 1)[0]
            if base in self.character_tags:
                return base
        return tag

    def _choose_copyright(self, tags: list[str]) -> str | None:
        """Prefer a specific game tag over its generic `_(series)` parent."""
        fallback = None
        for tag in tags:
            if tag == "original":
                continue
            if fallback is None:
                fallback = tag
            if not tag.endswith("_(series)"):
                return tag
            root = re.split(r"[_:]", tag, maxsplit=1)[0]
            for other in tags:
                if other == tag or other.endswith("_(series)"):
                    continue
                if re.split(r"[_:]", other, maxsplit=1)[0] == root:
                    return other
        return fallback

    def decode(self, prob: torch.Tensor) -> dict:
        categories: dict[str, dict] = {}
        all_tags: list[dict] = []
        candidates: list[dict] = []
        for category, (start, end) in self.ranges.items():
            values = prob[start:end]
            threshold = self.thresholds[start:end]
            indices = torch.nonzero(values > threshold, as_tuple=False).flatten()
            if indices.numel() > 0:
                order = torch.argsort(values[indices], descending=True)
                indices = indices[order]
            top = [
                {
                    "tag": self.tags[start + int(i)],
                    "score": round(float(values[i]), 4),
                    "vocab_index": start + int(i),
                }
                for i in indices[:40]
            ]
            categories[category] = {"count": int(indices.numel()), "top": top}
            for item in top:
                all_tags.append(
                    {
                        "tag": item["tag"],
                        "category": category,
                        "score": item["score"],
                        "vocab_index": item["vocab_index"],
                    }
                )
            limit = CANDIDATE_CAPS.get(category, 0)
            if limit > 0:
                count = min(limit, int(values.numel()))
                scores, indexes = torch.topk(values, count)
                for score, index in zip(scores.tolist(), indexes.tolist()):
                    if score < CANDIDATE_FLOOR:
                        break
                    candidates.append(
                        {
                            "tag": self.tags[start + int(index)],
                            "category": category,
                            "score": round(float(score), 4),
                            "vocab_index": start + int(index),
                        }
                    )
        all_tags.sort(key=lambda item: item["score"], reverse=True)
        candidates.sort(key=lambda item: item["score"], reverse=True)
        rating = categories.get("rating", {}).get("top", [])
        rating_value = (
            rating[0]["tag"].replace("rating:", "") if rating else ""
        )
        copyright_top = self._choose_copyright(
            [item["tag"] for item in categories.get("copyright", {}).get("top", [])]
        )
        character_top = next(
            (
                self._normalize_character(item["tag"])
                for item in categories.get("character", {}).get("top", [])
            ),
            None,
        )
        general_top = {
            item["tag"] for item in categories.get("general", {}).get("top", [])
        }
        meta_top = {
            item["tag"] for item in categories.get("meta", {}).get("top", [])
        }
        if general_top & {"text_focus", "text-only_page", "fake_screenshot"}:
            group = "截图/文本"
        elif meta_top & {"photo_(medium)", "off-topic"}:
            group = "非动漫/实景"
        elif copyright_top:
            group = f"作品: {copyright_top}"
        elif character_top:
            group = f"角色: {character_top}"
        elif categories.get("copyright", {}).get("top"):
            group = "原创"
        else:
            group = "未分类"
        return {
            "rating": rating_value,
            "group": group,
            "categories": categories,
            "tags": all_tags[:60],
            "candidates": candidates,
        }

    def tag_paths(self, paths: list[str]) -> list[dict]:
        results: list[dict] = []
        pending: list[tuple[dict, Image.Image]] = []
        pending_pixels = 0

        def flush() -> None:
            nonlocal pending_pixels
            if not pending:
                return
            probs = self._infer([image for _, image in pending])
            for (result, _image), prob in zip(pending, probs):
                result.update(self.decode(prob))
            pending.clear()
            pending_pixels = 0

        for raw in paths:
            path = Path(raw)
            try:
                image = load_rgb(path)
                damage = None
            except Exception as strict_exc:
                code, label, detail = classify_error(strict_exc)
                print(
                    f"[tag] strict decode failed for {raw}: {detail}",
                    file=sys.stderr,
                    flush=True,
                )
                if code not in ("truncated", "decode_failed"):
                    results.append(
                        {
                            "path": raw,
                            "ok": False,
                            "error": label,
                            "error_detail": detail,
                            "damage": label,
                        }
                    )
                    continue
                try:
                    image = load_rgb(path, tolerant=True)
                except Exception as tolerant_exc:
                    _, tolerant_label, tolerant_detail = classify_error(tolerant_exc)
                    print(
                        f"[tag] tolerant decode failed for {raw}: {tolerant_detail}",
                        file=sys.stderr,
                        flush=True,
                    )
                    results.append(
                        {
                            "path": raw,
                            "ok": False,
                            "error": label,
                            "error_detail": tolerant_detail,
                            "damage": label,
                        }
                    )
                    continue
                damage = f"{label}，已按容错模式读取"
                print(
                    f"[tag] tolerant decode accepted {raw}: {detail}",
                    file=sys.stderr,
                    flush=True,
                )
            result = {
                "path": raw,
                "ok": True,
                "width": image.width,
                "height": image.height,
                "damage": damage,
            }
            results.append(result)
            pending.append((result, image))
            pending_pixels += image.width * image.height
            if len(pending) >= MAX_INFER_BATCH or pending_pixels >= MAX_TAG_PIXELS:
                flush()
        flush()
        return results


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--device", default="auto", choices=["auto", "cuda", "cpu"])
    parser.add_argument("--no-half", action="store_true")
    args = parser.parse_args()

    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    sys.stderr.reconfigure(encoding="utf-8", newline="\n")

    try:
        tagger = Tagger(args.model, args.device, not args.no_half)
    except Exception as exc:
        print(
            json.dumps(
                {"ready": False, "error": f"{type(exc).__name__}: {exc}"},
                ensure_ascii=False,
            ),
            flush=True,
        )
        raise SystemExit(1)

    print(
        json.dumps(
            {
                "ready": True,
                "device": str(tagger.device),
                "half": tagger.half,
                "classes": int(tagger.config.num_classes),
                "thresholds": tagger.default_thresholds,
                "model": str(args.model),
            },
            ensure_ascii=False,
        ),
        flush=True,
    )

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        request_id = None
        try:
            request = json.loads(line)
            request_id = request.get("id")
            command = request.get("cmd")
            if command == "shutdown":
                break
            if command == "ping":
                print(
                    json.dumps({"id": request_id, "ok": True}, ensure_ascii=False),
                    flush=True,
                )
                continue
            if command != "tag":
                raise ValueError(f"unknown command: {command}")
            items = tagger.tag_paths(list(request.get("paths", [])))
            print(
                json.dumps(
                    {"id": request_id, "ok": True, "items": items},
                    ensure_ascii=False,
                ),
                flush=True,
            )
        except Exception as exc:
            print(
                json.dumps(
                    {
                        "id": request_id,
                        "ok": False,
                        "error": f"{type(exc).__name__}: {exc}",
                    },
                    ensure_ascii=False,
                ),
                flush=True,
            )


if __name__ == "__main__":
    main()
