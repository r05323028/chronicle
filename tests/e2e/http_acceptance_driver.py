#!/usr/bin/env python3
"""Local HTTP workload and replay target for privileged acceptance."""

from __future__ import annotations

import argparse
import json
import socket
import socketserver
from pathlib import Path
from urllib.parse import urlsplit

RESPONSES = {
    ("GET", "/content-length"): ("200 OK", b"content-length"),
    ("GET", "/chunked"): ("200 OK", b"b"),
}


def append_json(path: Path, value: dict[str, object]) -> None:
    with path.open("a", encoding="utf-8") as output:
        output.write(json.dumps(value, sort_keys=True) + "\n")
        output.flush()


def read_request(connection: socket.socket, buffered: bytes) -> tuple[str, str, bytes, bytes] | None:
    while b"\r\n\r\n" not in buffered:
        chunk = connection.recv(4096)
        if not chunk:
            return None
        buffered += chunk
        if len(buffered) > 64 * 1024:
            raise ValueError("HTTP request head exceeds 64 KiB")
    head, buffered = buffered.split(b"\r\n\r\n", 1)
    lines = head.decode("iso-8859-1").split("\r\n")
    method, target, version = lines[0].split(" ", 2)
    if version != "HTTP/1.1":
        raise ValueError("expected HTTP/1.1 request")
    headers = {}
    for line in lines[1:]:
        name, value = line.split(":", 1)
        headers[name.lower()] = value.strip()
    length = int(headers.get("content-length", "0"))
    while len(buffered) < length:
        chunk = connection.recv(4096)
        if not chunk:
            raise ValueError("truncated request body")
        buffered += chunk
    return method, target, buffered[:length], buffered[length:]


def response(method: str, target: str, body: bytes) -> bytes:
    status, payload = RESPONSES.get((method, target), ("201 Created", body) if (method, target) == ("POST", "/echo") else ("404 Not Found", b"not found"))
    if target == "/chunked":
        return f"HTTP/1.1 {status}\r\ntransfer-encoding: chunked\r\n\r\n".encode() + b"1\r\nb\r\n0\r\n\r\n"
    return f"HTTP/1.1 {status}\r\ncontent-length: {len(payload)}\r\n\r\n".encode() + payload


class Handler(socketserver.BaseRequestHandler):
    requests: Path

    def handle(self) -> None:
        buffered = b""
        while True:
            request = read_request(self.request, buffered)
            if request is None:
                return
            method, target, body, buffered = request
            append_json(self.requests, {"method": method, "target": target, "body_bytes": len(body)})
            self.request.sendall(response(method, target, body))


class Server(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True


def serve(args: argparse.Namespace) -> None:
    request_log = Path(args.requests)
    request_log.touch()
    Handler.requests = request_log
    with Server(("127.0.0.1", args.port), Handler) as server:
        Path(args.port_file).write_text(str(server.server_address[1]), encoding="utf-8")
        server.serve_forever(poll_interval=0.1)


def receive(connection: socket.socket, buffered: bytes, size: int) -> tuple[bytes, bytes]:
    while len(buffered) < size:
        chunk = connection.recv(4096)
        if not chunk:
            raise ValueError("truncated HTTP response")
        buffered += chunk
    return buffered[:size], buffered[size:]


def response_from(connection: socket.socket, buffered: bytes) -> tuple[tuple[int, bytes], bytes]:
    while b"\r\n\r\n" not in buffered:
        chunk = connection.recv(4096)
        if not chunk:
            raise ValueError("truncated HTTP response head")
        buffered += chunk
    head, buffered = buffered.split(b"\r\n\r\n", 1)
    lines = head.decode("iso-8859-1").split("\r\n")
    status = int(lines[0].split(" ", 2)[1])
    headers = dict(line.split(":", 1) for line in lines[1:])
    if "transfer-encoding" not in headers:
        body, buffered = receive(connection, buffered, int(headers.get("content-length", "0")))
        return (status, body), buffered
    body = b""
    while True:
        while b"\r\n" not in buffered:
            buffered += connection.recv(4096)
        size_text, buffered = buffered.split(b"\r\n", 1)
        size = int(size_text, 16)
        if size == 0:
            _, buffered = receive(connection, buffered, 2)
            return (status, body), buffered
        chunk, buffered = receive(connection, buffered, size + 2)
        body += chunk[:-2]


def request(connection: socket.socket, buffered: bytes, method: str, target: str, body: bytes = b"") -> tuple[tuple[int, bytes], bytes]:
    head = f"{method} {target} HTTP/1.1\r\nhost: x\r\n".encode()
    if body:
        head += f"content-length: {len(body)}\r\n".encode()
    connection.sendall(head + b"\r\n" + body)
    return response_from(connection, buffered)


def workload(args: argparse.Namespace) -> None:
    parsed = urlsplit(args.origin)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or parsed.port is None:
        raise ValueError("origin must be explicit loopback HTTP URL")
    first = socket.create_connection((parsed.hostname, parsed.port), timeout=5)
    buffered = b""
    first_result, buffered = request(first, buffered, "GET", "/content-length")
    second_result, buffered = request(first, buffered, "GET", "/chunked")
    first.close()
    second = socket.create_connection((parsed.hostname, parsed.port), timeout=5)
    third_result, _ = request(second, b"", "POST", "/echo", b"c")
    second.close()
    results = [first_result, second_result, third_result]
    expected = [(200, b"content-length"), (200, b"b"), (201, b"c")]
    if results != expected:
        raise AssertionError(f"unexpected workload replies: {results!r}")
    print(json.dumps({"connections": 2, "requests": len(results), "status": "ok"}, sort_keys=True))


def main() -> None:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(required=True)
    server = commands.add_parser("serve")
    server.add_argument("--port", type=int, default=0)
    server.add_argument("--port-file", required=True)
    server.add_argument("--requests", required=True)
    server.set_defaults(run=serve)
    workload_parser = commands.add_parser("workload")
    workload_parser.add_argument("--origin", required=True)
    workload_parser.set_defaults(run=workload)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
