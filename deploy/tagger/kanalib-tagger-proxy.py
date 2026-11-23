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
import threading

LISTEN = (
    os.environ.get("KANALIB_TAGGER_PROXY_BIND", "10.126.126.2"),
    int(os.environ.get("KANALIB_TAGGER_PROXY_PORT", "8793")),
)
TARGET = (
    os.environ.get("KANALIB_TAGGER_PROXY_TARGET_HOST", "10.126.126.1"),
    int(os.environ.get("KANALIB_TAGGER_PROXY_TARGET_PORT", "8793")),
)


def pump(src: socket.socket, dst: socket.socket) -> None:
    try:
        while True:
            data = src.recv(65536)
            if not data:
                break
            dst.sendall(data)
    except OSError:
        pass
    finally:
        try:
            dst.shutdown(socket.SHUT_WR)
        except OSError:
            pass


class Handler(socketserver.BaseRequestHandler):
    def handle(self) -> None:
        try:
            upstream = socket.create_connection(TARGET, timeout=10)
        except OSError:
            return
        upstream.settimeout(None)
        threading.Thread(target=pump, args=(self.request, upstream), daemon=True).start()
        pump(upstream, self.request)
        upstream.close()


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


if __name__ == "__main__":
    with Server(LISTEN, Handler) as server:
        server.serve_forever()
