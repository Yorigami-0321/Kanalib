from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
from pathlib import Path

import numpy as np
import torch
from PIL import Image, ImageOps

sys.dont_write_bytecode = True


def load_rgb(path: Path) -> Image.Image:
    image = Image.open(path)
    image = ImageOps.exif_transpose(image)
    if image.mode != "RGB":
        image = image.convert("RGBA")
        canvas = Image.new("RGBA", image.size, (255, 255, 255, 255))
        canvas.alpha_composite(image)
        image = canvas.convert("RGB")
    return image


def preprocess(path: Path, size: int) -> np.ndarray:
    image = load_rgb(path)
    width, height = image.size
    scale = min(size / height, size / width)
    new_w, new_h = max(1, int(width * scale)), max(1, int(height * scale))
    image = image.resize((new_w, new_h), Image.BILINEAR)
    canvas = Image.new("RGB", (size, size), (0, 0, 0))
    canvas.paste(image, ((size - new_w) // 2, (size - new_h) // 2))
    array = np.asarray(canvas, dtype=np.float32) / 255.0
    array = (array - 0.5) / 0.5
    return np.ascontiguousarray(array.transpose(2, 0, 1)[None, ...], dtype=np.float32)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--images", required=True, type=Path)
    parser.add_argument("--batch", type=int, default=1)
    parser.add_argument("--runs", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument("--mode", choices=["half", "fp32", "autocast"], default="half")
    parser.add_argument("--dump", type=Path)
    args = parser.parse_args()

    started = time.perf_counter()
    from transformers import AutoModel

    model = AutoModel.from_pretrained(
        args.model,
        trust_remote_code=True,
        local_files_only=True,
        dtype=torch.float32,
    )
    model.to("cuda")
    if args.mode == "half":
        model.half()
    model.eval()
    load_seconds = time.perf_counter() - started

    size = int(model.config.img_size)
    dtype = torch.float16 if args.mode == "half" else torch.float32
    torch.set_grad_enabled(False)

    paths = sorted(
        path
        for path in args.images.iterdir()
        if path.suffix.lower() in {".png", ".jpg", ".jpeg", ".webp", ".bmp"}
    )
    if not paths:
        raise SystemExit(f"no images in {args.images}")

    batches = []
    index = 0
    for _ in range(args.runs):
        chunk = [paths[(index + offset) % len(paths)] for offset in range(args.batch)]
        index += args.batch
        batches.append(chunk)

    def infer(tensor: torch.Tensor) -> torch.Tensor:
        tensor = tensor.to("cuda", non_blocking=False)
        if args.mode == "autocast":
            with torch.autocast("cuda", dtype=torch.float16):
                return model(tensor).float().cpu()
        if args.mode == "half":
            tensor = tensor.half()
        return model(tensor).float().cpu()

    torch.cuda.reset_peak_memory_stats()
    with torch.inference_mode():
        for _ in range(args.warmup):
            batch = batches[0]
            tensor = torch.from_numpy(
                np.concatenate([preprocess(path, size) for path in batch], axis=0)
            )
            infer(tensor)

        pre_times, infer_times, decode_times = [], [], []
        first_logits = None
        for batch in batches:
            started = time.perf_counter()
            tensor = torch.from_numpy(
                np.concatenate([preprocess(path, size) for path in batch], axis=0)
            )
            pre_times.append(time.perf_counter() - started)

            started = time.perf_counter()
            logits = infer(tensor)
            infer_times.append(time.perf_counter() - started)

            started = time.perf_counter()
            probabilities = torch.sigmoid(logits)
            for row in probabilities:
                torch.topk(row, 20)
            decode_times.append(time.perf_counter() - started)

            if first_logits is None:
                first_logits = logits.numpy().copy()

    if args.dump:
        args.dump.parent.mkdir(parents=True, exist_ok=True)
        np.save(args.dump, first_logits)

    def summary(values):
        return {
            "mean_ms": round(statistics.mean(values) * 1000, 2),
            "p50_ms": round(statistics.median(values) * 1000, 2),
            "min_ms": round(min(values) * 1000, 2),
            "max_ms": round(max(values) * 1000, 2),
        }

    total = [p + i + d for p, i, d in zip(pre_times, infer_times, decode_times)]
    report = {
        "backend": "pytorch",
        "device": torch.cuda.get_device_name(0),
        "torch": torch.__version__,
        "mode": args.mode,
        "batch": args.batch,
        "runs": args.runs,
        "load_seconds": round(load_seconds, 2),
        "preprocess": summary(pre_times),
        "infer": summary(infer_times),
        "decode": summary(decode_times),
        "end_to_end": summary(total),
        "images_per_second": round(args.batch / statistics.mean(total), 3),
        "peak_vram_mb": round(torch.cuda.max_memory_allocated() / 1024**2),
        "dtype": str(dtype),
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
