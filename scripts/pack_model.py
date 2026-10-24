"""Create an FP16 copy of the tagger model for distribution."""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path

import torch
from safetensors.torch import load_file, save_file

SIDE_FILES = (
    "config.json",
    "preprocessor_config.json",
    "tagger_pipeline.py",
    "banner.png",
)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--src", default="models/pixai-tagger-v1.0", type=Path)
    parser.add_argument("--out", default="dist/models/pixai-tagger-v1.0", type=Path)
    args = parser.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    for name in SIDE_FILES:
        source = args.src / name
        if source.exists():
            shutil.copy2(source, args.out / name)

    tensors = load_file(str(args.src / "model.safetensors"))
    half = {key: (value.half() if value.is_floating_point() else value) for key, value in tensors.items()}
    save_file(half, str(args.out / "model.safetensors"), metadata={"format": "pt"})

    total = sum(path.stat().st_size for path in args.out.rglob("*") if path.is_file())
    print(f"packed {args.out} ({total / 2**30:.2f} GiB)")


if __name__ == "__main__":
    main()
