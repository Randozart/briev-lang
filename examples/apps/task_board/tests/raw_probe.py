#!/usr/bin/env python3
"""Raw HTTP framing probe for the Task Board API gate."""
import socket
import sys


def main() -> int:
    port = int(sys.argv[1])
    mode = sys.argv[2]
    if mode == "truncated":
        payload = b"GET /health HTTP/1.1\r\nHost: probe\r\n"
    elif mode == "oversized":
        body = b"a" * 5000
        payload = (
            b"POST /api/tasks HTTP/1.1\r\n"
            b"Host: probe\r\n"
            b"Content-Type: application/json\r\n"
            b"Content-Length: 5000\r\n\r\n" + body
        )
    else:
        print(f"unknown mode: {mode}", file=sys.stderr)
        return 2

    sock = socket.create_connection(("127.0.0.1", port), timeout=2)
    sock.settimeout(2)
    sock.sendall(payload)
    sock.shutdown(socket.SHUT_WR)
    chunks = []
    while True:
        try:
            chunk = sock.recv(4096)
        except socket.timeout:
            break
        except ConnectionResetError:
            break
        if not chunk:
            break
        chunks.append(chunk)
    sock.close()
    sys.stdout.buffer.write(b"".join(chunks))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
