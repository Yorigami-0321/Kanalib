from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--python", required=True, type=Path)
    parser.add_argument("--script", required=True, type=Path)
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--images", required=True, type=Path)
    parser.add_argument("--batch", type=int, default=1)
    parser.add_argument("--runs", type=int, default=20)
    parser.add_argument("--warmup", type=int, default=3)
    args = parser.parse_args()

    paths = sorted(
        str(path.resolve())
        for path in args.images.iterdir()
        if path.suffix.lower() in {".png", ".jpg", ".jpeg", ".webp", ".bmp"}
    )
    if not paths:
        raise SystemExit("no images")

    started = time.perf_counter()
    process = subprocess.Popen(
        [
            str(args.python),
            str(args.script),
            "--model",
            str(args.model),
            "--device",
            "auto",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        encoding="utf-8",
        bufsize=1,
    )
    assert process.stdin and process.stdout
    ready = json.loads(process.stdout.readline())
    load_seconds = time.perf_counter() - started
    if not ready.get("ready"):
        raise SystemExit(f"sidecar failed: {ready}")

    latencies = []
    index = 0
    total = args.warmup + args.runs
    for step in range(total):
        chunk = [paths[(index + offset) % len(paths)] for offset in range(args.batch)]
        index += args.batch
        request = {"id": step, "cmd": "tag", "paths": chunk}
        started = time.perf_counter()
        process.stdin.write(json.dumps(request, ensure_ascii=False) + "\n")
        process.stdin.flush()
        response = json.loads(process.stdout.readline())
        elapsed = time.perf_counter() - started
        if not response.get("ok"):
            raise SystemExit(f"sidecar error: {response}")
        if step >= args.warmup:
            latencies.append(elapsed)

    process.stdin.write('{"cmd":"shutdown"}\n')
    process.stdin.flush()
    process.wait(timeout=20)

    report = {
        "backend": "python-sidecar",
        "ready": {k: ready.get(k) for k in ("device", "half", "classes")},
        "batch": args.batch,
        "runs": args.runs,
        "load_seconds": round(load_seconds, 2),
        "round_trip": {
            "mean_ms": round(statistics.mean(latencies) * 1000, 2),
            "p50_ms": round(statistics.median(latencies) * 1000, 2),
            "min_ms": round(min(latencies) * 1000, 2),
            "max_ms": round(max(latencies) * 1000, 2),
        },
        "images_per_second": round(args.batch / statistics.mean(latencies), 3),
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
