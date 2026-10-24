from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

URL = (
    "https://mirror.sjtu.edu.cn/pytorch-wheels/cu128/"
    "torch-2.9.1%2Bcu128-cp310-cp310-win_amd64.whl"
)
TOTAL = 2_862_053_390
SHA256 = "64399adaa8ea0896d02cf844cba3c5dd77e769520a1af73572599e0eaa2cf551"
OUT = Path(__file__).resolve().parent / "torch-2.9.1+cu128-cp310-cp310-win_amd64.whl"
PARTS = 24
WORKERS = 16


def fetch(index: int) -> int:
    chunk = (TOTAL + PARTS - 1) // PARTS
    start = index * chunk
    end = min(start + chunk, TOTAL) - 1
    expected = end - start + 1
    part = OUT.with_suffix(OUT.suffix + f".part{index:02d}")
    if part.exists() and part.stat().st_size == expected:
        return index
    for attempt in range(1, 6):
        part.unlink(missing_ok=True)
        cmd = [
            shutil.which("curl") or "curl",
            "-L",
            "--fail",
            "--silent",
            "--show-error",
            "--retry",
            "3",
            "--connect-timeout",
            "30",
            "--max-time",
            "1200",
            "-r",
            f"{start}-{end}",
            "-o",
            str(part),
            URL,
        ]
        try:
            subprocess.run(cmd, check=True)
            if part.stat().st_size == expected:
                return index
        except (subprocess.CalledProcessError, FileNotFoundError) as exc:
            print(f"part {index} attempt {attempt} failed: {exc}", flush=True)
        if attempt < 5:
            part.unlink(missing_ok=True)
    raise RuntimeError(f"part {index} failed")


def main() -> None:
    print(f"downloading {TOTAL/1024**3:.2f}GiB to {OUT}", flush=True)
    with ThreadPoolExecutor(max_workers=WORKERS) as pool:
        futures = [pool.submit(fetch, i) for i in range(PARTS)]
        done = 0
        for future in as_completed(futures):
            future.result()
            done += 1
            print(f"parts {done}/{PARTS}", flush=True)

    merged = OUT.with_suffix(OUT.suffix + ".merge")
    with merged.open("wb") as dst:
        for i in range(PARTS):
            part = OUT.with_suffix(OUT.suffix + f".part{i:02d}")
            with part.open("rb") as src:
                shutil.copyfileobj(src, dst, length=8 * 1024 * 1024)
            part.unlink()
    if merged.stat().st_size != TOTAL:
        raise SystemExit(f"size mismatch: {merged.stat().st_size} != {TOTAL}")

    digest = hashlib.sha256()
    with merged.open("rb") as f:
        for block in iter(lambda: f.read(8 * 1024 * 1024), b""):
            digest.update(block)
    if digest.hexdigest() != SHA256:
        raise SystemExit(f"sha256 mismatch: {digest.hexdigest()} != {SHA256}")
    merged.replace(OUT)
    print(f"ok {OUT} sha256={digest.hexdigest()}", flush=True)


if __name__ == "__main__":
    main()
