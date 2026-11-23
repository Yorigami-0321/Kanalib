#!/usr/bin/env python3
"""Kanalib alert inbox.

Public listener (behind Nginx) requires a token in the path:
    POST /kanalib-alert/<token>   text/plain report -> 204
    GET  /kanalib-alert/<token>   recent alerts as HTML

Optional private listener (EasyTier address) accepts tokenless requests:
    POST /kanalib-alert            text/plain report -> 204

Both listeners append to the same JSONL file under KANALIB_ALERT_STATE.
"""

import fcntl
import html
import json
import os
import sys
import threading
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

TOKEN = os.environ.get("KANALIB_ALERT_TOKEN", "")
PUBLIC_BIND = os.environ.get("KANALIB_ALERT_BIND_PUBLIC", "127.0.0.1:8794")
PRIVATE_BIND = os.environ.get("KANALIB_ALERT_BIND_PRIVATE", "")
STATE = Path(os.environ.get("KANALIB_ALERT_STATE", "/var/lib/kanalib-alert"))
LOG = STATE / "alerts.jsonl"
MAX_BODY = 64 * 1024
KEEP = 500
SHOW = 50
LOCK = threading.Lock()


def fail(message: str) -> None:
    sys.stderr.write(message + "\n")
    raise SystemExit(2)


if len(TOKEN) < 24:
    fail("KANALIB_ALERT_TOKEN is required (>= 24 chars)")


def parse_bind(value: str) -> tuple[str, int]:
    host, _, port = value.rpartition(":")
    if not host or not port.isdigit():
        fail(f"invalid bind address: {value}")
    return host, int(port)


def read_alerts() -> list[dict]:
    if not LOG.exists():
        return []
    rows: list[dict] = []
    with LOG.open("r", encoding="utf-8", errors="replace") as fh:
        fcntl.flock(fh, fcntl.LOCK_SH)
        try:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    rows.append(json.loads(line))
                except json.JSONDecodeError:
                    continue
        finally:
            fcntl.flock(fh, fcntl.LOCK_UN)
    return rows[-SHOW:]


def append_alert(entry: dict) -> None:
    STATE.mkdir(parents=True, exist_ok=True, mode=0o750)
    with LOCK:
        with LOG.open("a+", encoding="utf-8") as fh:
            fcntl.flock(fh, fcntl.LOCK_EX)
            try:
                fh.write(json.dumps(entry, ensure_ascii=False) + "\n")
                fh.flush()
                fh.seek(0)
                lines = fh.read().splitlines()
                if len(lines) > KEEP:
                    fh.seek(0)
                    fh.truncate()
                    fh.write("\n".join(lines[-KEEP:]) + "\n")
                    fh.flush()
            finally:
                fcntl.flock(fh, fcntl.LOCK_UN)
        os.chmod(LOG, 0o600)


def render(rows: list[dict]) -> bytes:
    parts = [
        "<!doctype html><meta charset=utf-8><title>Kanalib alerts</title>",
        "<style>body{font:14px/1.5 system-ui;max-width:960px;margin:24px auto;"
        "padding:0 16px;background:#111;color:#eee}pre{white-space:pre-wrap;"
        "background:#1b1b1b;padding:10px;border-radius:6px;overflow-wrap:anywhere}"
        "time{color:#888}</style>",
        f"<h1>Kanalib alerts</h1><p>最近 {len(rows)} 条</p>",
    ]
    for row in reversed(rows):
        parts.append(
            f"<p><time>{html.escape(str(row.get('ts', '')))}</time> "
            f"{html.escape(str(row.get('ip', '')))}</p>"
            f"<pre>{html.escape(str(row.get('body', '')))}</pre>"
        )
    if not rows:
        parts.append("<p>暂无告警</p>")
    return "".join(parts).encode("utf-8")


def make_handler(require_token: bool):
    class Handler(BaseHTTPRequestHandler):
        server_version = "kanalib-alert/1"

        def version_string(self) -> str:
            return self.server_version

        def path_ok(self) -> bool:
            path = urlparse(self.path).path.rstrip("/")
            token_path = f"/kanalib-alert/{TOKEN}"
            if path == token_path:
                return True
            if path == "/kanalib-alert":
                return not require_token
            return False

        def send_text(self, code: int, text: str) -> None:
            data = text.encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_POST(self) -> None:
            if not self.path_ok():
                self.send_text(404, "not found\n")
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
            except ValueError:
                length = 0
            if length <= 0:
                self.send_text(411, "length required\n")
                return
            if length > MAX_BODY:
                self.send_text(413, "payload too large\n")
                return
            body = self.rfile.read(length).decode("utf-8", "replace").strip()
            entry = {
                "ts": datetime.now(timezone.utc).isoformat(timespec="seconds"),
                "ip": self.headers.get("X-Real-IP", self.client_address[0]),
                "body": body,
            }
            append_alert(entry)
            self.send_response(204)
            self.send_header("Cache-Control", "no-store")
            self.end_headers()

        def do_GET(self) -> None:
            if not self.path_ok():
                self.send_text(404, "not found\n")
                return
            data = render(read_alerts())
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, fmt: str, *args) -> None:
            sys.stderr.write("%s - %s\n" % (self.address_string(), fmt % args))

    return Handler


def serve(bind: str, require_token: bool) -> ThreadingHTTPServer:
    host, port = parse_bind(bind)
    server = ThreadingHTTPServer((host, port), make_handler(require_token))
    sys.stderr.write(f"kanalib-alert: listening {host}:{port} token={require_token}\n")
    return server


def main() -> None:
    servers = [serve(PUBLIC_BIND, True)]
    if PRIVATE_BIND:
        servers.append(serve(PRIVATE_BIND, False))
    for extra in servers[1:]:
        threading.Thread(target=extra.serve_forever, daemon=True).start()
    servers[0].serve_forever()


if __name__ == "__main__":
    main()
