#!/usr/bin/env python3
"""Delta-debug a slow parse input down to a minimal reproducer.

Keeps any candidate whose brievc parse wall-time exceeds THRESHOLD seconds.
Run: python3 scripts/reduce_slow_parse.py <input> [threshold]
"""
import subprocess, sys, time, tempfile, os

BRIEVC = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "target/debug/brievc")
THRESHOLD = float(sys.argv[2]) if len(sys.argv) > 2 else 2.0


def elapsed(data: bytes) -> float:
    with tempfile.NamedTemporaryFile(suffix=".bv", delete=False) as fh:
        fh.write(data)
        path = fh.name
    try:
        t0 = time.monotonic()
        try:
            subprocess.run([BRIEVC, "build", path], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=120)
        except subprocess.TimeoutExpired:
            return 120.0
        return time.monotonic() - t0
    finally:
        os.unlink(path)


def is_slow(data: bytes) -> bool:
    return elapsed(data) > THRESHOLD


def ddmin(data: bytes) -> bytes:
    n = 2
    while len(data) >= 2:
        chunk = len(data) // n
        if chunk == 0:
            break
        reduced = False
        for i in range(n):
            start = i * chunk
            end = len(data) if i == n - 1 else start + chunk
            candidate = data[:start] + data[end:]
            if candidate and is_slow(candidate):
                data = candidate
                n = max(2, n - 1)
                reduced = True
                break
        if not reduced:
            if n >= len(data):
                break
            n = min(len(data), n * 2)
    return data


def main() -> None:
    with open(sys.argv[1], "rb") as fh:
        data = fh.read()
    print(f"start: {len(data)} bytes, {elapsed(data):.2f}s")
    data = ddmin(data)
    print(f"minimized: {len(data)} bytes, {elapsed(data):.2f}s")
    print(repr(data))
    out = "/tmp/opencode/nest/minimized.bv"
    with open(out, "wb") as fh:
        fh.write(data)
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
