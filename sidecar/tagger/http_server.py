#!/usr/bin/env python3
"""HTTP wrapper around the stdio tagger protocol used by kanalib-ingest.

POST /tag with multipart field `image` returns {"tags": ["..."]}.
GET /health returns model/device state.

Both endpoints require `x-tagger-token` (or `Authorization: Bearer`) when a
shared token is configured. Bodies are read only after the declared length is
checked against a hard limit, and concurrent requests are capped by a
semaphore so a remote peer cannot exhaust host memory.
"""

from __future__ import annotations

import argparse
import hmac
import json
import os
import sys
import tempfile
import threading
import traceback
from email.parser import BytesParser
from email.policy import default as DEFAULT_POLICY
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from server import Tagger  # noqa: E402

DEFAULT_MAX_BODY = 32 * 1024 * 1024
DEFAULT_MAX_INFLIGHT = 4
READ_TIMEOUT_SECS = 30
LOOPBACK_NAMES = {"127.0.0.1", "::1", "localhost"}


def extract_image(content_type: str, body: bytes) -> bytes | None:
    header = f"content-type: {content_type}\r\n\r\n".encode("latin-1")
    message = BytesParser(policy=DEFAULT_POLICY).parsebytes(header + body)
    if not message.is_multipart():
        return None
    for part in message.iter_parts():
        if part.get_param("name", header="content-disposition") != "image":
            continue
        payload = part.get_payload(decode=True)
        if payload:
            return payload
    return None


class TaggerHandler(BaseHTTPRequestHandler):
    server_version = "KanalibTagger/0.2"

    def log_message(self, fmt: str, *args: object) -> None:
        print(f"[http] {self.address_string()} {fmt % args}", file=sys.stderr, flush=True)

    def reply(self, status: int, payload: dict) -> None:
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json; charset=utf-8")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authorized(self) -> bool:
        expected = self.server.token
        if not expected:
            return True
        provided = self.headers.get("x-tagger-token")
        if not provided:
            authorization = self.headers.get("authorization", "")
            if authorization.lower().startswith("bearer "):
                provided = authorization[7:]
        if not provided:
            return False
        return hmac.compare_digest(
            provided.strip().encode("utf-8"), expected.encode("utf-8")
        )

    def do_GET(self) -> None:  # noqa: N802
        if not self.authorized():
            self.reply(401, {"error": "unauthorized"})
            return
        if self.path == "/health":
            self.reply(200, self.server.health)
        else:
            self.reply(404, {"error": "not found"})

    def do_POST(self) -> None:  # noqa: N802
        try:
            self.handle_tag()
        except Exception:  # noqa: BLE001
            traceback.print_exc()
            self.close_connection = True
            try:
                self.reply(500, {"error": "internal error"})
            except OSError:
                pass

    def handle_tag(self) -> None:
        if not self.authorized():
            self.reply(401, {"error": "unauthorized"})
            return
        if self.path != "/tag":
            self.reply(404, {"error": "not found"})
            return
        content_type = self.headers.get("content-type", "")
        if "multipart/form-data" not in content_type:
            self.close_connection = True
            self.reply(400, {"error": "expected multipart/form-data"})
            return
        try:
            length = int(self.headers.get("content-length", "0"))
        except ValueError:
            self.close_connection = True
            self.reply(400, {"error": "invalid content-length"})
            return
        if length <= 0:
            self.close_connection = True
            self.reply(400, {"error": "empty body"})
            return
        if length > self.server.max_body:
            self.close_connection = True
            self.reply(413, {"error": "body too large"})
            return
        if not self.server.slots.acquire(blocking=False):
            self.close_connection = True
            self.reply(503, {"error": "busy"})
            return
        path: str | None = None
        try:
            self.connection.settimeout(READ_TIMEOUT_SECS)
            body = self.rfile.read(length)
            if len(body) != length:
                self.reply(400, {"error": "incomplete body"})
                return
            image = extract_image(content_type, body)
            if not image:
                self.reply(400, {"error": 'missing multipart field "image"'})
                return
            with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as handle:
                handle.write(image)
                path = handle.name
            with self.server.inference_lock:
                result = self.server.tagger.tag_paths([path])[0]
            if not result.get("ok"):
                detail = result.get("error_detail")
                if detail:
                    print(f"[http] decode failed: {detail}", file=sys.stderr, flush=True)
                self.reply(422, {"error": result.get("error", "decode failed")})
                return
            tags = [str(item["tag"]) for item in result.get("tags", [])]
            if not tags:
                self.reply(422, {"error": "no tags above threshold"})
                return
            self.reply(200, {"tags": tags})
        finally:
            if path:
                Path(path).unlink(missing_ok=True)
            self.server.slots.release()


class TaggerServer(ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 32

    def __init__(
        self,
        address: tuple[str, int],
        tagger: Tagger,
        health: dict,
        token: str | None,
        max_body: int,
        max_inflight: int,
    ) -> None:
        super().__init__(address, TaggerHandler)
        self.tagger = tagger
        self.health = health
        self.token = token
        self.max_body = max_body
        self.slots = threading.BoundedSemaphore(max(1, max_inflight))
        self.inference_lock = threading.Lock()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--device", default="auto", choices=["auto", "cuda", "cpu"])
    parser.add_argument("--no-half", action="store_true")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", default=8793, type=int)
    parser.add_argument("--token-file", type=Path)
    parser.add_argument(
        "--max-body",
        type=int,
        default=int(os.environ.get("KANALIB_TAGGER_MAX_BODY", DEFAULT_MAX_BODY)),
    )
    parser.add_argument(
        "--max-inflight",
        type=int,
        default=int(os.environ.get("KANALIB_TAGGER_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT)),
    )
    args = parser.parse_args()

    if args.token_file:
        token = args.token_file.read_text(encoding="utf-8").strip()
        if not token:
            raise SystemExit(f"token file {args.token_file} is empty")
    else:
        token = os.environ.get("KANALIB_TAGGER_TOKEN", "").strip() or None
    if token is None and args.host not in LOOPBACK_NAMES:
        raise SystemExit(
            f"refusing to bind {args.host} without --token-file or KANALIB_TAGGER_TOKEN"
        )

    tagger = Tagger(args.model, args.device, not args.no_half)
    health = {
        "ok": True,
        "device": str(tagger.device),
        "half": tagger.half,
        "classes": int(tagger.config.num_classes),
    }
    server = TaggerServer(
        (args.host, args.port),
        tagger,
        health,
        token,
        args.max_body,
        args.max_inflight,
    )
    print(json.dumps(health, ensure_ascii=False), flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
