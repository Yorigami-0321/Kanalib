#!/usr/bin/env python3
"""HTTP wrapper around the stdio tagger protocol used by kanalib-ingest.

POST /tag with multipart field `image` returns {"tags": ["..."]}.
GET /health returns model/device state.
"""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from server import Tagger  # noqa: E402


def extract_image(body: bytes, boundary: bytes) -> bytes | None:
    for part in body.split(b"--" + boundary):
        part = part.strip(b"\r\n")
        if not part or part == b"--":
            continue
        head, separator, data = part.partition(b"\r\n\r\n")
        if not separator or b'name="image"' not in head:
            continue
        return data.rstrip(b"\r\n")
    return None


class TaggerHandler(BaseHTTPRequestHandler):
    server_version = "KanalibTagger/0.1"

    def log_message(self, fmt: str, *args: object) -> None:
        print(f"[http] {self.address_string()} {fmt % args}", file=sys.stderr, flush=True)

    def reply(self, status: int, payload: dict) -> None:
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json; charset=utf-8")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/health":
            self.reply(200, self.server.health)
        else:
            self.reply(404, {"error": "not found"})

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/tag":
            self.reply(404, {"error": "not found"})
            return
        content_type = self.headers.get("content-type", "")
        if "multipart/form-data" not in content_type or "boundary=" not in content_type:
            self.reply(400, {"error": "expected multipart/form-data"})
            return
        boundary = content_type.split("boundary=", 1)[1].strip().strip('"').encode()
        try:
            length = int(self.headers.get("content-length", "0"))
        except ValueError:
            self.reply(400, {"error": "invalid content-length"})
            return
        if length <= 0:
            self.reply(400, {"error": "empty body"})
            return
        image = extract_image(self.rfile.read(length), boundary)
        if not image:
            self.reply(400, {"error": 'missing multipart field "image"'})
            return
        suffix = ".jpg"
        with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as handle:
            handle.write(image)
            path = handle.name
        try:
            with self.server.inference_lock:
                result = self.server.tagger.tag_paths([path])[0]
            if not result.get("ok"):
                self.reply(422, {"error": result.get("error", "decode failed")})
                return
            tags = [str(item["tag"]) for item in result.get("tags", [])]
            if not tags:
                self.reply(422, {"error": "no tags above threshold"})
                return
            self.reply(200, {"tags": tags})
        except Exception as error:  # noqa: BLE001
            self.reply(500, {"error": f"{type(error).__name__}: {error}"})
        finally:
            Path(path).unlink(missing_ok=True)


class TaggerServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address: tuple[str, int], tagger: Tagger, health: dict) -> None:
        super().__init__(address, TaggerHandler)
        self.tagger = tagger
        self.health = health
        self.inference_lock = threading.Lock()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--device", default="auto", choices=["auto", "cuda", "cpu"])
    parser.add_argument("--no-half", action="store_true")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", default=8793, type=int)
    args = parser.parse_args()

    tagger = Tagger(args.model, args.device, not args.no_half)
    health = {
        "ok": True,
        "device": str(tagger.device),
        "half": tagger.half,
        "classes": int(tagger.config.num_classes),
        "model": str(args.model),
    }
    server = TaggerServer((args.host, args.port), tagger, health)
    print(json.dumps(health, ensure_ascii=False), flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
