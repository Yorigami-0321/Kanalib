"""Generate a deterministic synthetic image library for local stress tests."""

from __future__ import annotations

import argparse
import json
import random
import shutil
from pathlib import Path

from PIL import Image, ImageDraw, ImageEnhance, ImageOps

WORDS = (
    "sky",
    "forest",
    "city",
    "portrait",
    "abstract",
    "gradient",
    "night",
    "flower",
    "ocean",
    "room",
)
CHINESE = ("风景", "角色", "合成", "图集", "测试")
SIZES = ((256, 256), (320, 240), (240, 320))


def make_base(rng: random.Random) -> Image.Image:
    width, height = rng.choice(SIZES)
    top = tuple(rng.randrange(40, 230) for _ in range(3))
    bottom = tuple(rng.randrange(40, 230) for _ in range(3))
    image = Image.new("RGB", (width, height))
    pixels = image.load()
    for y in range(height):
        mix = y / max(1, height - 1)
        color = tuple(int(top[c] * (1 - mix) + bottom[c] * mix) for c in range(3))
        for x in range(width):
            pixels[x, y] = color
    draw = ImageDraw.Draw(image)
    for _ in range(rng.randrange(3, 8)):
        shape = rng.choice(("ellipse", "line", "rect"))
        x0 = rng.randrange(-20, width)
        y0 = rng.randrange(-20, height)
        box = (
            x0,
            y0,
            rng.randrange(x0 + 2, width + 20),
            rng.randrange(y0 + 2, height + 20),
        )
        color = tuple(rng.randrange(20, 255) for _ in range(3))
        if shape == "ellipse":
            draw.ellipse(box, outline=color, width=rng.randrange(2, 6))
        elif shape == "rect":
            draw.rectangle(box, outline=color, width=rng.randrange(1, 4))
        else:
            draw.line(box, fill=color, width=rng.randrange(1, 4))
    return image


def variant(base: Image.Image, rng: random.Random, label: str) -> Image.Image:
    image = base.copy()
    if rng.random() < 0.35:
        image = ImageOps.mirror(image)
    image = ImageEnhance.Brightness(image).enhance(rng.uniform(0.75, 1.2))
    image = ImageEnhance.Color(image).enhance(rng.uniform(0.7, 1.3))
    draw = ImageDraw.Draw(image)
    x = rng.randrange(0, max(1, image.width - 90))
    y = rng.randrange(0, max(1, image.height - 20))
    draw.rectangle((x, y, min(image.width, x + 90), min(image.height, y + 20)), fill=(20, 20, 20))
    draw.text((x + 3, y + 4), label, fill=(240, 240, 240))
    return image


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--count", type=int, default=10_000)
    parser.add_argument("--folders", type=int, default=10)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--duplicate-every", type=int, default=100)
    parser.add_argument("--corrupt", type=int, default=10)
    args = parser.parse_args()

    rng = random.Random(args.seed)
    bases = [make_base(rng) for _ in range(64)]
    args.out.mkdir(parents=True, exist_ok=True)
    folders = [args.out / f"set{index:02d}_{WORDS[index % len(WORDS)]}" for index in range(args.folders)]
    for folder in folders:
        folder.mkdir(parents=True, exist_ok=True)

    saved: list[Path] = []
    duplicates = 0
    corrupt = 0
    for index in range(args.count):
        folder = folders[index % len(folders)]
        word = WORDS[index % len(WORDS)]
        chinese = CHINESE[index % len(CHINESE)]
        is_corrupt = index >= args.count - args.corrupt
        is_duplicate = (
            not is_corrupt
            and args.duplicate_every > 0
            and index > 0
            and index % args.duplicate_every == 0
        )
        slot = index % 10
        extension = "png" if slot == 3 else "webp" if slot == 7 else "jpg"
        if is_duplicate:
            source = rng.choice(saved)
            extension = source.suffix.lstrip(".")
        name = f"synthetic_{index:05d}_{word}_{chinese}.{extension}"
        target = folder / name
        if is_corrupt:
            target.write_text("not an image\n", encoding="utf-8")
            corrupt += 1
        elif is_duplicate:
            shutil.copyfile(source, target)
            duplicates += 1
        else:
            image = variant(bases[index % len(bases)], rng, f"{index:05d}")
            if extension == "jpg":
                image.save(target, format="JPEG", quality=85)
            elif extension == "png":
                image.save(target, format="PNG")
            else:
                image.save(target, format="WEBP", quality=85)
        saved.append(target)
        if (index + 1) % 1000 == 0:
            print(f"generated {index + 1}/{args.count}", flush=True)

    manifest = {
        "count": args.count,
        "folders": args.folders,
        "seed": args.seed,
        "duplicates": duplicates,
        "corrupt": corrupt,
        "out": str(args.out),
    }
    (args.out / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print(json.dumps(manifest, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
