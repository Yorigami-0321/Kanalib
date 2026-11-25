#!/usr/bin/env python3
"""TCP proxy: box -> cloud (EasyTier) -> local GPU tagger.

EasyTier relay between two wss clients does not forward payload traffic in this
setup, so the cloud node terminates the TCP connection and re-originates it to
the local tagger over the direct p2p link. Gating is unchanged: when the local
client is offline, the upstream connect fails and the proxy closes the socket.
"""

from __future__ import annotations

import os
import socket
import socketserver
import sys
import threading

LISTEN = (
    os.environ.get("KANALIB_TAGGER_PROXY_BIND", "10.126.126.2"),
    int(os.environ.get("KANALIB_TAGGER_PROXY_PORT", "8793")),
)
TARGET = (
    os.environ.get("KANALIB_TAGGER_PROXY_TARGET_HOST", "10.126.126.1"),
    int(os.environ.get("KANALIB_TAGGER_PROXY_TARGET_PORT", "8793")),
)
ALLOW = {
    item.strip()
    for item in os.environ.get("KANALIB_TAGGER_PROXY_ALLOW", "").split(",")
    if item.strip()
}
# Client->upstream reads must make progress; upstream->client may stay idle for
# the whole inference, so it intentionally has no read timeout.
REQUEST_IDLE_TIMEOUT = float(os.environ.get("KANALIB_TAGGER_PROXY_IDLE", "60"))


def pump(src: socket.socket, dst: socket.socket, timeout: float | None) -> None:
    if timeout is not None:
        src.settimeout(timeout)
    try:
        while True:
            data = src.recv(65536)
            if not data:
                break
            dst.sendall(data)
    except OSError as error:
        print(f"[proxy] pump closed: {error}", file=sys.stderr, flush=True)
    finally:
        try:
            dst.shutdown(socket.SHUT_WR)
        except OSError:
            pass


class Handler(socketserver.BaseRequestHandler):
    def handle(self) -> None:
        peer = self.client_address[0]
        if ALLOW and peer not in ALLOW:
            print(f"[proxy] rejected {peer}", file=sys.stderr, flush=True)
            return
        try:
            upstream = socket.create_connection(TARGET, timeout=10)
        except OSError:
            return
        upstream.settimeout(None)
        upstream.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.request.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        threading.Thread(
            target=pump,
            args=(self.request, upstream, REQUEST_IDLE_TIMEOUT),
            daemon=True,
        ).start()
        pump(upstream, self.request, None)
        upstream.close()


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True
    request_queue_size = 64


if __name__ == "__main__":
    with Server(LISTEN, Handler) as server:
        server.serve_forever()
