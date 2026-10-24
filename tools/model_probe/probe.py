from __future__ import annotations

import argparse
import json
import os
import time
from pathlib import Path

import numpy as np
import torch
from PIL import Image, ImageOps
from torchvision.transforms import functional as vF
from transformers import AutoModel

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MODEL = ROOT / "models" / "pixai-tagger-v1.0"
DEFAULT_OUT = Path(__file__).resolve().parent / "out"
DEFAULT_IMAGES = [
    DEFAULT_MODEL / "banner.png",
    Path("D:/Code/floral-notepaper/Docs/images/AI绘画截图.png"),
    Path("D:/Code/floral-notepaper/Docs/images/主窗口截图.png"),
    Path("C:/Windows/Web/Wallpaper/Windows/img0.jpg"),
    Path("C:/Windows/Web/Wallpaper/Windows/img19.jpg"),
]


def load_rgb(path: Path) -> Image.Image:
    img = Image.open(path)
    img = ImageOps.exif_transpose(img)
    if img.mode != "RGB":
        img = img.convert("RGBA")
        canvas = Image.new("RGBA", img.size, (255, 255, 255, 255))
        canvas.alpha_composite(img)
        img = canvas.convert("RGB")
    return img


def preprocess(images: list[Image.Image], size: int = 1008) -> torch.Tensor:
    tensors = []
    for img in images:
        t = vF.to_tensor(img)
        h, w = t.shape[-2:]
        if h != size or w != size:
            r = min(size / h, size / w)
            nh, nw = int(h * r), int(w * r)
            ph, pw = size - nh, size - nw
            left, top = pw // 2, ph // 2
            right, bottom = pw - left, ph - top
            t = vF.resize(t, [nh, nw])
            t = vF.pad(t, [left, top, right, bottom], 0)
        t = vF.normalize(t, [0.5, 0.5, 0.5], [0.5, 0.5, 0.5])
        tensors.append(t)
    return torch.stack(tensors)


def build_thresholds(config) -> tuple[torch.Tensor, dict[str, tuple[int, int]]]:
    thresholds = torch.full((config.num_classes,), 0.2, dtype=torch.float32)
    ranges: dict[str, tuple[int, int]] = {}
    start = 0
    for category, count in config.tags_split:
        thresholds[start : start + count] = float(
            config.category_best_threshold.get(category, 0.2)
        )
        ranges[category] = (start, start + count)
        start += count
    assert start == config.num_classes, (start, config.num_classes)
    return thresholds, ranges


def decode(
    probs: torch.Tensor,
    tags: list[str],
    thresholds: torch.Tensor,
    ranges: dict[str, tuple[int, int]],
    topk: int = 20,
) -> dict:
    result = {}
    for category, (start, end) in ranges.items():
        values = probs[start:end]
        threshold = thresholds[start:end]
        indices = torch.nonzero(values > threshold, as_tuple=False).flatten()
        if indices.numel() > 0:
            order = torch.argsort(values[indices], descending=True)
            indices = indices[order]
        items = [
            {"tag": tags[start + int(i)], "score": round(float(values[i]), 4)}
            for i in indices[:topk]
        ]
        result[category] = {
            "above_threshold": int(indices.numel()),
            "top": items,
        }
    return result


def forward(model, batch, device, autocast: bool):
    if autocast and device.type == "cuda":
        with torch.autocast(device_type="cuda", dtype=torch.float16):
            return model(batch)
    return model(batch)


def benchmark(
    model,
    tensors: torch.Tensor,
    device: torch.device,
    batch_size: int,
    repeats: int = 3,
    half: bool = False,
    autocast: bool = False,
):
    batch = tensors[:batch_size].to(device, dtype=torch.float16 if half else torch.float32)
    use_cuda = device.type == "cuda"
    if use_cuda:
        torch.cuda.synchronize()
        torch.cuda.reset_peak_memory_stats()
    with torch.inference_mode():
        for _ in range(2):
            forward(model, batch, device, autocast)
        if use_cuda:
            torch.cuda.synchronize()
        started = time.perf_counter()
        for _ in range(repeats):
            forward(model, batch, device, autocast)
        if use_cuda:
            torch.cuda.synchronize()
        elapsed = time.perf_counter() - started
    peak_mb = torch.cuda.max_memory_allocated() / 1024**2 if use_cuda else None
    per_image_ms = elapsed / (repeats * batch_size) * 1000
    return {
        "batch_size": batch_size,
        "repeats": repeats,
        "images_per_second": round(batch_size * repeats / elapsed, 3),
        "ms_per_image": round(per_image_ms, 2),
        "peak_vram_mb": round(peak_mb, 1) if peak_mb else None,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("images", nargs="*", type=Path)
    parser.add_argument("--model", type=Path, default=DEFAULT_MODEL)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--device", default="cuda" if torch.cuda.is_available() else "cpu")
    parser.add_argument("--topk", type=int, default=20)
    parser.add_argument("--batch-size", type=int, default=0)
    parser.add_argument("--bench", action="store_true")
    parser.add_argument("--half", action="store_true")
    parser.add_argument("--autocast", action="store_true")
    parser.add_argument("--dump-logits", action="store_true")
    args = parser.parse_args()

    paths = args.images or [p for p in DEFAULT_IMAGES if p.exists()]
    paths = [p for p in paths if p.exists()]
    if not paths:
        raise SystemExit("no input images found")
    if not args.model.exists():
        raise SystemExit(f"model directory not found: {args.model}")

    device = torch.device(args.device)
    if device.type == "cpu":
        torch.set_num_threads(max(1, (os.cpu_count() or 4) - 2))
    print(f"torch={torch.__version__} cuda={torch.version.cuda} device={device}")
    if device.type == "cuda":
        print(f"gpu={torch.cuda.get_device_name(0)} vram={torch.cuda.get_device_properties(0).total_memory/1024**3:.1f}GiB")

    print(f"loading model from {args.model}")
    started = time.perf_counter()
    model = AutoModel.from_pretrained(
        args.model,
        trust_remote_code=True,
        torch_dtype=torch.float32,
    ).to(device)
    if args.half:
        model.half()
    model.eval()
    print(f"model loaded in {time.perf_counter() - started:.1f}s half={args.half} autocast={args.autocast}")

    config = model.config
    tags = list(config.tags)
    thresholds, ranges = build_thresholds(config)
    print(
        f"classes={config.num_classes} tags={len(tags)} "
        f"categories={[(c, n) for c, n in config.tags_split]}"
    )
    print(f"thresholds={config.category_best_threshold}")

    images = [load_rgb(p) for p in paths]
    for p, img in zip(paths, images):
        print(f"input {p} size={img.size} mode={img.mode}")

    started = time.perf_counter()
    cpu_tensors = preprocess(images, size=config.img_size)
    preprocess_ms = (time.perf_counter() - started) * 1000
    print(f"preprocess {len(images)} images: {preprocess_ms:.2f}ms")

    batch_size = args.batch_size or len(images)
    started = time.perf_counter()
    logits_parts = []
    with torch.inference_mode():
        for start in range(0, len(cpu_tensors), batch_size):
            batch = cpu_tensors[start : start + batch_size].to(
                device, dtype=torch.float16 if args.half else torch.float32
            )
            logits_parts.append(
                forward(model, batch, device, args.autocast).float().cpu()
            )
    logits = torch.cat(logits_parts)
    infer_seconds = time.perf_counter() - started
    probs = logits.sigmoid()
    print(
        f"inference batch={len(images)}: {infer_seconds*1000:.2f}ms "
        f"({len(images)/infer_seconds:.2f} img/s)"
    )

    payload = {
        "model_dir": str(args.model),
        "torch": torch.__version__,
        "cuda": torch.version.cuda,
        "device": str(device),
        "gpu": torch.cuda.get_device_name(0) if device.type == "cuda" else None,
        "num_classes": config.num_classes,
        "categories": [[c, n] for c, n in config.tags_split],
        "thresholds": dict(config.category_best_threshold),
        "preprocess_ms": round(preprocess_ms, 2),
        "inference_ms": round(infer_seconds * 1000, 2),
        "images_per_second": round(len(images) / infer_seconds, 3),
        "items": [],
    }

    if device.type == "cuda":
        payload["peak_vram_mb_batch"] = round(
            torch.cuda.max_memory_allocated() / 1024**2, 1
        )

    for path, image, prob in zip(paths, images, probs):
        decoded = decode(prob, tags, thresholds, ranges, topk=args.topk)
        item = {
            "path": str(path),
            "width": image.width,
            "height": image.height,
            "categories": decoded,
        }
        payload["items"].append(item)
        print(f"\n=== {path.name} {image.width}x{image.height}")
        for category in ranges:
            data = decoded[category]
            top = ", ".join(f"{x['tag']}:{x['score']:.3f}" for x in data["top"][:8])
            print(f"  {category:10s} n={data['above_threshold']:4d}  {top}")

    if args.bench:
        payload["bench"] = []
        max_batch = min(8, len(cpu_tensors))
        for batch_size in [1, 2, 4, 8]:
            if batch_size > max_batch:
                continue
            try:
                stats = benchmark(
                    model,
                    cpu_tensors,
                    device,
                    batch_size,
                    half=args.half,
                    autocast=args.autocast,
                )
                payload["bench"].append(stats)
                print(f"bench {stats}")
            except torch.cuda.OutOfMemoryError:
                torch.cuda.empty_cache()
                print(f"bench batch={batch_size} OOM")
                payload["bench"].append({"batch_size": batch_size, "error": "oom"})
                break

    if args.dump_logits:
        args.out.mkdir(parents=True, exist_ok=True)
        np.savez_compressed(
            args.out / "logits.npz",
            logits=logits.numpy(),
            probs=probs.numpy(),
            paths=np.array([str(p) for p in paths]),
        )
        np.save(args.out / "input_batch.npy", cpu_tensors.numpy())
        print(f"dumped logits/input to {args.out}")

    args.out.mkdir(parents=True, exist_ok=True)
    result_path = args.out / "probe.json"
    result_path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print(f"\nwrote {result_path}")


if __name__ == "__main__":
    main()
