#!/usr/bin/env python3
"""Cross-surface grammar-form probe (three-surfaces plan Phase 0.6d).

Takes canonical programs written for each surface and builds a COPY under
every active execution-surface extension (.bv/.ebv/.abv/.rbv/.sbv). The
failure class per cell (ok / parse / type / capability / other-error) is
the mechanical evidence for the one-surface-grammar inventory: a form
that only builds on its home surface is surface-owned; a form that builds
everywhere is core.

Output: a matrix printed to stdout and saved as JSON in the scratch dir.
Re-run: `python3 scripts/grammar_probe.py` (needs a release build;
BRIEV_GRAMMAR_WORK overrides the scratch dir).
"""
import json
import os
import shutil
import subprocess
import concurrent.futures as cf

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
BRIEVC = os.path.join(REPO, "target/release", "brievc")
WORK = os.environ.get("BRIEV_GRAMMAR_WORK", "/tmp/briev-grammar-probe")

# Canonical program per surface — REAL active sources, never hand-rewritten.
PROBES = [
    ("core", "examples/eor-demo.bv"),
    ("core-list", "examples/error-handling.bv"),
    ("electronics", "examples/electronics/usb_sensor.ebv"),
    ("electronics-min", "examples/electronics/led_blinker.ebv"),
    ("gpu", "examples/gpu/reduce.abv"),
    ("gpu-gemm", "examples/gpu/gemm_small.abv"),
    ("render", "examples/counter.rbv"),
    ("render-struct", "examples/rstruct-demo.rbv"),
    ("silicon", "examples/silicon/sensor_die.sbv"),
]

SURFACES = ["bv", "ebv", "abv", "rbv", "sbv"]


def classify(rc, out):
    if rc == 0:
        return "ok"
    if "type errors" in out or "type mismatch" in out:
        return "type"
    if "not supported by this backend" in out or "capability" in out.lower():
        return "capability"
    if "parse error" in out or "error:" in out:
        return "parse"
    if "error" in out:
        return "other-error"
    return f"rc={rc}"


def run(src_rel, surface, tag):
    src = os.path.join(REPO, src_rel)
    d = os.path.join(WORK, f"{tag}_{surface}")
    os.makedirs(d, exist_ok=True)
    link = os.path.join(d, "lib")
    if not os.path.lexists(link):
        try:
            os.symlink(os.path.join(REPO, "lib"), link)
        except FileExistsError:
            pass
    dst = os.path.join(d, f"probe.{surface}")
    shutil.copyfile(src, dst)
    out_dir = os.path.join(d, "out")
    os.makedirs(out_dir, exist_ok=True)
    p = subprocess.run(
        [BRIEVC, "build", dst, "--out", out_dir],
        capture_output=True, text=True, cwd=REPO, timeout=180,
    )
    out = (p.stderr or "") + (p.stdout or "")
    return classify(p.returncode, out), out.strip().splitlines()[:8]


def main():
    results = {}
    with cf.ThreadPoolExecutor(max_workers=8) as ex:
        futs = {
            (tag, surface): ex.submit(run, rel, surface, tag)
            for tag, rel in PROBES
            for surface in SURFACES
        }
        for (tag, surface), fut in futs.items():
            cls, head = fut.result()
            results.setdefault(tag, {})[surface] = (cls, head)

    print(f"{'program':<16}" + "".join(f"{s:>13}" for s in SURFACES))
    for tag, _ in PROBES:
        row = results.get(tag, {})
        cells = "".join(f"{row.get(s, ('?', []))[0]:>13}" for s in SURFACES)
        print(f"{tag:<16}{cells}")
    json.dump(
        {t: {s: [c, h] for s, (c, h) in r.items()} for t, r in results.items()},
        open(os.path.join(WORK, "grammar_probe_results.json"), "w"), indent=1,
    )
    print(f"\nsaved {os.path.join(WORK, 'grammar_probe_results.json')}")


if __name__ == "__main__":
    main()
