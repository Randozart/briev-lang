#!/usr/bin/env python3
"""Intel Panther Lake benchmark harness — Workstream B.

Measures OpenVINO GPU and CPU plugins on the PTL node with a correctness
gate BEFORE any timing. The Briev-side timing lives in collector.bv (the
plan's intended timing source: Briev's checked clock + bounded histogram via
lib/std/os/time.bv + lib/std/stats/histogram.bv); this script additionally
records perf_counter_ns samples as the competitor-harness reference.

Geometry (2026-11-06 first pass):
  elem:  self-add f32, N = 64 Mi        (memory-bound ceiling truth)
  gemm:  square MatMul f16, 4096^3
  sdpa:  scaled_dot_product_attention f16, B=8 H=32 S=1024 D=128
         (plan asks 32/32/4096/128; the node had ~3 GB available on
          2026-11-06 — seq 4096 needs >3.2 GB just for Q/K/V — so the SDPA
          lane is scaled and the reduction is recorded in the results file.)

Outputs (all under --work):
  <bench>.xml/.bin        IR model for the Briev collector
  <bench>_in<i>.bin       raw C-order input blobs for the collector
  results.json            gates + perf_counter samples + metadata

No timing is reported unless its correctness gate passed.
"""

import argparse
import json
import os
import sys
import time

import numpy as np
import openvino as ov

ELEM_N = 64 * 1024 * 1024
GEMM_N = 4096
SDPA_B, SDPA_H, SDPA_S, SDPA_D = 8, 32, 1024, 128

ITERS = {
    ("elem", "GPU"): 60,
    ("elem", "CPU"): 8,
    ("gemm", "GPU"): 20,
    ("gemm", "CPU"): 2,
    ("sdpa", "GPU"): 12,
    ("sdpa", "CPU"): 1,
}


def build_models(args):
    q = ov.opset13.parameter([SDPA_B, SDPA_H, SDPA_S, SDPA_D], ov.Type.f16, name="q")
    k = ov.opset13.parameter([SDPA_B, SDPA_H, SDPA_S, SDPA_D], ov.Type.f16, name="k")
    v = ov.opset13.parameter([SDPA_B, SDPA_H, SDPA_S, SDPA_D], ov.Type.f16, name="v")
    att = ov.opset13.scaled_dot_product_attention(q, k, v)

    x = ov.opset13.parameter([args.elem_n], ov.Type.f32, name="x")
    gemm_t = av_type(args.gemm_dtype)
    a = ov.opset13.parameter([args.gemm_n, args.gemm_n], gemm_t, name="a")
    b = ov.opset13.parameter([args.gemm_n, args.gemm_n], gemm_t, name="b")

    return {
        "elem": ov.Model([ov.opset13.result(ov.opset13.add(x, x))], [x], "elem"),
        "gemm": ov.Model(
            [ov.opset13.result(ov.opset13.matmul(a, b, False, False))], [a, b], "gemm"
        ),
        "sdpa": ov.Model([ov.opset13.result(att)], [q, k, v], "sdpa"),
    }


def av_type(name):
    return {"f16": ov.Type.f16, "f32": ov.Type.f32}[name]


def np_dtype(name):
    return {"f16": np.float16, "f32": np.float32}[name]


def make_inputs(rng, args, name):
    """Inputs for ONE bench, at the requested geometry.

    Built per bench rather than all at once: the 4096^3 f16 pair plus the
    64 Mi-element vector is ~0.4 GB, which need not exist when the caller only
    wants the 512^3 rung.
    """
    if name == "elem":
        return [rng.standard_normal(args.elem_n, dtype=np.float32)]
    if name == "gemm":
        dt = np_dtype(args.gemm_dtype)
        return [
            rng.standard_normal((args.gemm_n, args.gemm_n)).astype(dt),
            rng.standard_normal((args.gemm_n, args.gemm_n)).astype(dt),
        ]
    if name == "sdpa":
        return [
            rng.standard_normal(shape).astype(np.float16)
            for shape in ((SDPA_B, SDPA_H, SDPA_S, SDPA_D),) * 3
        ]
    raise KeyError(name)


def reference(name, arrays):
    if name == "elem":
        return arrays[0] + arrays[0]
    if name == "gemm":
        a, b = arrays
        # f32 reference; f16 inputs are widened first so the gate measures the
        # plugin's arithmetic, not an f16 re-read.
        return a.astype(np.float32) @ b.astype(np.float32)
    if name == "sdpa":
        return sdpa_reference(*arrays)
    raise KeyError(name)


def sdpa_reference(q, k, v):
    # Chunk over batch: a full f32 scores+probs pair would be ~2 GB at this
    # geometry (the node had ~3 GB available on 2026-11-06).
    scale = 1.0 / np.sqrt(np.float32(SDPA_D))
    ref_sdpa = np.empty((SDPA_B, SDPA_H, SDPA_S, SDPA_D), dtype=np.float32)
    for bi in range(SDPA_B):
        qf = q[bi].astype(np.float32)
        kf = k[bi].astype(np.float32)
        vf = v[bi].astype(np.float32)
        scores = (qf @ np.swapaxes(kf, -1, -2)) * scale
        scores -= np.max(scores, axis=-1, keepdims=True)
        probs = np.exp(scores)
        probs /= np.sum(probs, axis=-1, keepdims=True)
        ref_sdpa[bi] = probs @ vf
    return ref_sdpa


def max_rel(out, ref):
    out = np.asarray(out, dtype=np.float32).reshape(-1)
    ref = np.asarray(ref, dtype=np.float32).reshape(-1)
    denom = max(1e-6, float(np.max(np.abs(ref))))
    return float(np.max(np.abs(out - ref)) / denom)


def percentile(samples, p):
    ordered = sorted(samples)
    idx = max(0, min(len(ordered) - 1, int(round((p / 100.0) * (len(ordered) - 1)))))
    return ordered[idx]


def time_compiled(compiled, inputs, iters):
    compiled(inputs)  # warmup (JIT, allocations)
    samples = []
    for _ in range(iters):
        t0 = time.perf_counter_ns()
        compiled(inputs)
        t1 = time.perf_counter_ns()
        samples.append(t1 - t0)
    return samples


def save_case(work, name, model, arrays):
    ov.save_model(model, os.path.join(work, f"{name}.xml"))
    for i, arr in enumerate(arrays):
        np.ascontiguousarray(arr).tofile(os.path.join(work, f"{name}_in{i}.bin"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", required=True)
    ap.add_argument("--benches", default="elem,gemm,sdpa")
    ap.add_argument("--elem-n", type=int, default=ELEM_N)
    # 2026-11-22 (P3): the GEMM geometry is a parameter so a comparator can be
    # measured at the geometry it is compared at. The Briev/saltpetre lanes and
    # g0_gemm are 512^3 f32, so an f16-4096^3-only harness could not supply a
    # same-geometry vendor number.
    ap.add_argument("--gemm-n", type=int, default=GEMM_N)
    ap.add_argument("--gemm-dtype", choices=("f16", "f32"), default="f16")
    args = ap.parse_args()

    os.makedirs(args.work, exist_ok=True)
    models = build_models(args)
    rng = np.random.default_rng(20261106)

    core = ov.Core()
    devices = [d for d in ("GPU", "CPU") if d in core.available_devices]
    results = {"timestamp_unix": int(time.time()), "runtime": ov.__version__,
               "devices": devices,
               "args": {"elem_n": args.elem_n, "gemm_n": args.gemm_n,
                        "gemm_dtype": args.gemm_dtype,
                        "benches": args.benches},
               "cases": {}}

    gates = {"elem": 1e-6, "gemm": 2e-2, "sdpa": 2e-2}

    for name in [b.strip() for b in args.benches.split(",") if b.strip()]:
        if name not in models:
            print(f"unknown bench {name}", file=sys.stderr)
            sys.exit(2)
        arrays = make_inputs(rng, args, name)
        ref = reference(name, arrays)
        save_case(args.work, name, models[name], arrays)
        case = {"gate_rel_max": gates[name], "geometry": {}, "devices": {}}
        if name == "elem":
            case["geometry"] = {"n": int(arrays[0].size), "dtype": "f32"}
        elif name == "gemm":
            case["geometry"] = {"n": args.gemm_n, "dtype": args.gemm_dtype}
        else:
            case["geometry"] = {"b": SDPA_B, "h": SDPA_H, "s": SDPA_S,
                                "d": SDPA_D, "dtype": "f16"}

        for dev in devices:
            iters = ITERS.get((name, dev), 1)
            try:
                # elementwise: force f32 precision so the x+x gate is exact;
                # gemm/sdpa: default (f16 inference precision, the compared
                # point).
                cfg = {}
                if name == "elem":
                    cfg = {"INFERENCE_PRECISION_HINT": "f32"}
                compiled = core.compile_model(models[name], dev, cfg)
                out = compiled(arrays)[0]
                err = max_rel(out, ref)
                entry = {
                    "iters": iters,
                    "max_rel_error": err,
                    "gate": "PASS" if err <= gates[name] else "FAIL",
                }
                if entry["gate"] == "FAIL":
                    entry["samples_ns"] = []
                else:
                    samples = time_compiled(compiled, arrays, iters)
                    entry["samples_ns"] = samples
                    entry["p50_ns"] = percentile(samples, 50)
                    entry["p90_ns"] = percentile(samples, 90)
                    entry["p99_ns"] = percentile(samples, 99)
                    entry["min_ns"] = min(samples)
                case["devices"][dev] = entry
            except Exception as exc:  # plugin absent/unsupported geometry
                case["devices"][dev] = {"gate": "ERROR", "error": str(exc)[:300]}
        results["cases"][name] = case
        print(f"[{name}] " + ", ".join(
            f"{d}: {v.get('gate')}"
            + (f" p50={v.get('p50_ns')}ns err={v.get('max_rel_error'):.3g}"
               if v.get("gate") == "PASS" else "")
            for d, v in case["devices"].items()))

    out_path = os.path.join(args.work, "results.json")
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"wrote {out_path}")

    failed = [f"{n}/{d}" for n, c in results["cases"].items()
              for d, v in c["devices"].items() if v.get("gate") != "PASS"]
    if failed:
        print("GATE FAILURES: " + ", ".join(failed), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
