#!/usr/bin/env python3
"""Instantiate examples/gpu/attention_decode.abv at an arbitrary decode
attention geometry (plan 2026-09-18-m3-matrix-and-m4-fattn-shim).

The template's consts (D/H/HKV/G/NKV/SCALE) and literal buffer sizes are
rewritten to the requested geometry. Array sizes are literals because the
language does not accept const-expression lengths in let-decls (verified
2026-09-18: `Float[H * D]` is a parse error).
"""
import argparse
import math
import pathlib
import re

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--d", type=int, default=128, help="head dim")
p.add_argument("--h", type=int, default=32, help="query heads")
p.add_argument("--hkv", type=int, default=8, help="kv heads")
p.add_argument("--nkv", type=int, default=256, help="kv length")
p.add_argument("--f16-kv", action="store_true",
               help="declare k/v as Float16 (P0 f16 swap, plan "
                    "2026-09-18-fused-f16-decode-node)")
p.add_argument("--template", default="examples/gpu/attention_decode.abv")
p.add_argument("--out", required=True)
a = p.parse_args()

if a.h % a.hkv != 0:
    raise SystemExit(f"H={a.h} not a multiple of HKV={a.hkv}")
g = a.h // a.hkv

src = pathlib.Path(a.template).read_text()
src = re.sub(r"const D: Int = \d+;", f"const D: Int = {a.d};", src)
src = re.sub(r"const H: Int = \d+;", f"const H: Int = {a.h};", src)
src = re.sub(r"const HKV: Int = \d+;", f"const HKV: Int = {a.hkv};", src)
src = re.sub(r"const G: Int = \d+;", f"const G: Int = {g};", src)
src = re.sub(r"const NKV: Int = \d+;", f"const NKV: Int = {a.nkv};", src)
src = re.sub(
    r"const SCALE: Float = [0-9.e-]+;",
    f"const SCALE: Float = {1.0 / math.sqrt(a.d)!r};",
    src,
)

sizes = {
    "q": a.h * a.d,
    "a_out": a.h * a.d,
    "k": a.hkv * a.nkv * a.d,
    "v": a.hkv * a.nkv * a.d,
    "s": a.h * a.nkv,
    "o1": a.h * a.nkv,
}
kv_elem = "Float16" if a.f16_kv else "Float"
for name, n in sizes.items():
    elem = kv_elem if name in ("k", "v") else "Float"
    # 2026-09-20 (Front C, plan metaprogrammed-composites): the composite
    # form has no score buffer s — buffers the template omits are skipped,
    # not fatal.
    src, cnt = re.subn(rf"let {name}: \w+\[\d+\];", f"let {name}: {elem}[{n}];", src)
    if cnt != 1 and not (name == "s" and cnt == 0):
        raise SystemExit(f"field '{name}': expected exactly 1 decl, found {cnt}")

pathlib.Path(a.out).write_text(src)
print(f"geometry: D={a.d} H={a.h} HKV={a.hkv} G={g} NKV={a.nkv} -> {a.out}")
