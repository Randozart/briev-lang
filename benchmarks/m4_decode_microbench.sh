#!/usr/bin/env bash
# M4 microbench gate (plan 2026-09-18-m3-matrix-and-m4-fattn-shim): decode-
# attention kernel parity, Briev composition vs stock ggml fattn, at an
# exact geometry, CUDA lane only (the M4 integration target).
#
# Briev side: instantiate + build the composition runner, replace main()
# with the decode shim (per step: rotate q, append one K/V row per kv head
# via briev_accel_push_ranges, reset counters, dispatch qk -> softmax -> pv;
# a_out stays device-resident — the ggml integration consumes it on-device,
# so host readback is a device-pointer-ABI TODO, not part of kernel parity).
#
# Usage: bash benchmarks/m4_decode_microbench.sh [D] [H] [HKV] [NKV] [REPS]
#        defaults = bitnet-2b decode geometry at NKV 4096.
set -euo pipefail
cd "$(dirname "$0")/.."

D="${1:-128}"; H="${2:-20}"; HKV="${3:-5}"; NKV="${4:-4096}"; REPS="${5:-200}"
F16KV="${F16KV:-0}"
[ $((H % HKV)) -eq 0 ] || { echo "H=$H not divisible by HKV=$HKV" >&2; exit 1; }
G=$((H / HKV))
BRIEVC=./target/release/brievc
# 2026-09-25: extra brievc flags from the environment (e.g. BRIEVC_FLAGS="--config-dir <d>" for tuning A/Bs) — unquoted expansion is deliberate.
BRIEVC_FLAGS="${BRIEVC_FLAGS:-}"
CYBER=/home/randozart/Desktop/Projects/cyberllama
OUT=$(mktemp -d /tmp/opencode/m4.XXXX)
echo "artifacts: $OUT"

# ── 1. Briev side ────────────────────────────────────────────────────────────
F16FLAG=""
[ "$F16KV" = "1" ] && F16FLAG="--f16-kv"
# M1 layout experiment (plan coalesced-kv-memory-path): KLAYOUT=dmaj builds
# the K-d-major template and PREFILLS all K rows (seed path) — per-step K
# appends are skipped, so push/chain times exclude K append cost (the
# d-major append shape is M2's batching problem, deliberately out of M1).
KLAYOUT="${KLAYOUT:-jd}"
TMPL="examples/gpu/attention_decode.abv"
[ "$KLAYOUT" = "dmaj" ] && TMPL="examples/gpu/attention_decode_kdmaj.abv"
python3 benchmarks/attn_instantiate.py \
    --d "$D" --h "$H" --hkv "$HKV" --nkv "$NKV" $F16FLAG \
    --template "$TMPL" \
    --out examples/gpu/m4_attn_tmp.abv
# Float16 fields need the stdlib type in scope (float.bv declares it).
[ "$F16KV" = "1" ] && sed -i '1i import "std/types/float.bv";' examples/gpu/m4_attn_tmp.abv
$BRIEVC build examples/gpu/m4_attn_tmp.abv $BRIEVC_FLAGS --out "$OUT" >/dev/null
mv "$OUT/m4_attn_tmp_runner.c" "$OUT/attn_runner.c"
rm -f examples/gpu/m4_attn_tmp.abv

python3 - "$OUT/attn_runner.c" "$OUT/m4_briev.c" "$D" "$H" "$HKV" "$G" "$NKV" "$REPS" "$F16KV" "$KLAYOUT" <<'PYEOF'
import re, sys

runner_path, out_path = sys.argv[1], sys.argv[2]
D, H, HKV, G, NKV, REPS, F16KV = (int(x) for x in sys.argv[3:10])
KLAYOUT = sys.argv[10]
src = open(runner_path).read()

# Field tables: { name, kind, host_offset, elem_bytes, count, is_write, proj_offset }
fields = {}
for m in re.finditer(r'\{ "(\w+)", (\d+), (\d+), (\d+), (\d+), ([01]), (\d+) \}', src):
    fields[m.group(1)] = {
        "kind": int(m.group(2)),
        "host": int(m.group(3)),
        "elem": int(m.group(4)),
        "proj": int(m.group(7)),
    }

need = ["q", "k", "v", "a_out", "t", "r", "u"]
missing = [n for n in need if n not in fields]
if missing:
    raise SystemExit(f"fields missing from tables: {missing}")

# Kernel dispatch order from descs[] rows: { "name", kN, ...
kernels = re.findall(r'\{ "(\w+)", k\d+,', src)
if len(kernels) != 3:
    raise SystemExit(f"expected 3 kernel descs, found {kernels}")
QKIDX, SMIDX, PVIDX = (kernels.index(n) for n in ("qk", "softmax", "pv"))

F16 = fields["k"]["elem"] == 2
if F16KV != (1 if F16 else 0):
    raise SystemExit(f"f16-kv flag mismatch: harness={F16KV} tables k elem={fields['k']['elem']}")
QOFF, QPROJ = fields["q"]["host"], fields["q"]["proj"]
KOFF, KPROJ = fields["k"]["host"], fields["k"]["proj"]
VOFF, VPROJ = fields["v"]["host"], fields["v"]["proj"]
TOFF, ROFF, UOFF = fields["t"]["host"], fields["r"]["host"], fields["u"]["host"]
KEB, VEB = fields["k"]["elem"], fields["v"]["elem"]
QLEN = H * D
NQK = H * NKV
NPD = H * D
kptr = "unsigned short" if KEB == 2 else "float"
kcast = "(_Float16)" if KEB == 2 else "(float)"
vptr = "unsigned short" if VEB == 2 else "float"
vcast = "(_Float16)" if VEB == 2 else "(float)"

# Per-step K/V handling: jd appends both rows (contiguous per kv head);
# dmaj prefills all K rows before the warmup (d-major scatter) and appends
# V only — K append under d-major is M2's batching problem.
if KLAYOUT == "dmaj":
    # dmaj mode (post-M2): K appends need the strided push — DISABLED
    # pending the cuMemcpy2D driver quirk (BUGS.md) — so K stays
    # prefilled (the M1 measurement mode) and per-step appends are V-only
    # on the flat ranges path.
    kv_step_block = (
        "        rng = 8888u + (unsigned)step;\n"
        "        for (int kh = 0; kh < " + str(HKV) + "; kh++) {\n"
        "            size_t row_off = ((size_t)kh * " + str(NKV) + " + (size_t)j) * " + str(D) + " * " + str(VEB) + ";\n"
        "            for (int i = 0; i < " + str(D) + "; i++) {\n"
        "                rng = rng * 1103515245u + 12345u;\n"
        "                float val = (float)((rng >> 16) % 997) / 997.0f - 0.5f;\n"
        "                M4_VROW(state, " + str(VOFF) + " + row_off + (size_t)i * " + str(VEB) + ") =\n"
        "                    " + vcast + "val;\n"
        "            }\n"
        "            m4_ranges[3 * n] = " + str(VPROJ) + " + row_off;\n"
        "            m4_ranges[3 * n + 1] = " + str(VOFF) + " + row_off;\n"
        "            m4_ranges[3 * n + 2] = (size_t)" + str(D) + " * " + str(VEB) + ";\n"
        "            n++;\n"
        "        }\n"
    )
else:
    kv_step_block = (
        "        for (int kh = 0; kh < " + str(HKV) + "; kh++) {\n"
        "            size_t row_off = ((size_t)kh * " + str(NKV) + " + (size_t)j) * " + str(D) + " * " + str(KEB) + ";\n"
        "            rng = 7777u + (unsigned)step;\n"
        "            for (int i = 0; i < " + str(D) + "; i++) {\n"
        "                rng = rng * 1103515245u + 12345u;\n"
        "                float val = (float)((rng >> 16) % 997) / 997.0f - 0.5f;\n"
        "                M4_KROW(state, " + str(KOFF) + " + row_off + (size_t)i * " + str(KEB) + ") =\n"
        "                    " + kcast + "val;\n"
        "            }\n"
        "            rng = 8888u + (unsigned)step;\n"
        "            for (int i = 0; i < " + str(D) + "; i++) {\n"
        "                rng = rng * 1103515245u + 12345u;\n"
        "                float val = (float)((rng >> 16) % 997) / 997.0f - 0.5f;\n"
        "                M4_VROW(state, " + str(VOFF) + " + row_off + (size_t)i * " + str(VEB) + ") =\n"
        "                    " + vcast + "val;\n"
        "            }\n"
        "            m4_ranges[3 * n] = " + str(KPROJ) + " + row_off;\n"
        "            m4_ranges[3 * n + 1] = " + str(KOFF) + " + row_off;\n"
        "            m4_ranges[3 * n + 2] = (size_t)" + str(D) + " * " + str(KEB) + ";\n"
        "            n++;\n"
        "            m4_ranges[3 * n] = " + str(VPROJ) + " + row_off;\n"
        "            m4_ranges[3 * n + 1] = " + str(VOFF) + " + row_off;\n"
        "            m4_ranges[3 * n + 2] = (size_t)" + str(D) + " * " + str(VEB) + ";\n"
        "            n++;\n"
        "        }\n"
    )
k_statics = ""
k_prefill_block = (
    ""
    if KLAYOUT != "dmaj"
    else (
        "    /* dmaj: prefill the whole K cache (d-major); per-step K\n"
        "     * appends skipped — M2 batches that append shape. */\n"
        "    for (int kh = 0; kh < " + str(HKV) + "; kh++)\n"
        "        for (int j = 0; j < " + str(NKV) + "; j++)\n"
        "            for (int i = 0; i < " + str(D) + "; i++)\n"
        "                M4_KROW(state, " + str(KOFF) + " + ((size_t)kh * " + str(D * NKV) + " + (size_t)i * " + str(NKV) + " + (size_t)j) * " + str(KEB) + ") =\n"
        "                    " + kcast + "0.25f;\n"
    )
)

# Correctness check for the M2 strided path: stage a known row, push it
# strided, download K, verify every element landed at its pitched slot.
# M2 strided-append verification: moot while push_strided is disabled
# (BUGS.md cuMemcpy2D quirk). Restore with the strided kv_step_block.
verify_block = ""
# Push call: both layouts ride the flat ranges path (K/V rows contiguous
# per kv head in jd; V-only in dmaj — the d-major K append needs the
# strided push, disabled pending the cuMemcpy2D driver quirk, BUGS.md).
push_call = (
    '        if (!briev_accel_push_ranges(state, m4_ranges, n)) {{\n'
    '            fprintf(stderr, "push failed at step %d\\\\n", step);\n'
    '            return 1;\n'
    '        }}'
)

main = f'''
#include <time.h>
/* ── M4 decode-shim main (plan 2026-09-18-m3-matrix-and-m4-fattn-shim) ──
   Per step: rotate q, append one K/V row per kv head, reset the chain
   counters, push ranges, dispatch qk -> softmax -> pv. a_out stays
   device-resident (the ggml integration consumes it on-device). */
static float m4_qrow[{QLEN}];
static size_t m4_ranges[3 * 16];
{k_statics}
/* k/v row fills honor the field element width: 4 = f32, 2 = f16 storage
 * (_Float16 rows — the device's widening loads read the same values the
 * host wrote, plan fused-f16-decode-node P0). */
#define M4_KROW(state, off) (*({kptr}*)((unsigned char*)(state) + (off)))
#define M4_VROW(state, off) (*({vptr}*)((unsigned char*)(state) + (off)))

static double m4_now(void) {{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e6 + (double)ts.tv_nsec / 1e3;
}}
static int m4_cmp(const void* a, const void* b) {{
    double x = *(const double*)a, y = *(const double*)b;
    return (x > y) - (x < y);
}}

int main(void) {{
    if (!briev_accel_init(descs, N_KERNELS)) {{ fprintf(stderr, "briev: no device\\n"); return 1; }}
{k_prefill_block}    /* warmup chain: triggers the program-level seed (full_sync) */
    *(long long*)(state + {TOFF}) = 0;
    *(long long*)(state + {ROFF}) = 0;
    *(long long*)(state + {UOFF}) = 0;
    if (!briev_accel_launch_resident({QKIDX}, state, {NQK})) {{ fprintf(stderr, "qk failed\\n"); return 1; }}
    if (!briev_accel_launch_resident_2d({SMIDX}, state, 32, {H})) {{ fprintf(stderr, "softmax failed\\n"); return 1; }}
    if (!briev_accel_launch_resident({PVIDX}, state, {NPD})) {{ fprintf(stderr, "pv failed\\n"); return 1; }}

    double* push_t = malloc(sizeof(double) * {REPS});
    double* qk_t = malloc(sizeof(double) * {REPS});
    double* sm_t = malloc(sizeof(double) * {REPS});
    double* pv_t = malloc(sizeof(double) * {REPS});
    for (int step = 0; step < {REPS}; step++) {{
        unsigned rng = 999u + (unsigned)step;
        for (int i = 0; i < {QLEN}; i++) {{
            rng = rng * 1103515245u + 12345u;
            m4_qrow[i] = (float)((rng >> 16) % 997) / 997.0f - 0.5f;
        }}
        memcpy(state + {QOFF}, m4_qrow, (size_t){QLEN} * 4);
        *(long long*)(state + {TOFF}) = 0;
        *(long long*)(state + {ROFF}) = 0;
        *(long long*)(state + {UOFF}) = 0;
        long long j = step % {NKV};
        uint32_t n = 0;
        m4_ranges[3 * n] = {QPROJ};
        m4_ranges[3 * n + 1] = {QOFF};
        m4_ranges[3 * n + 2] = (size_t){QLEN} * 4;
        n++;
 {kv_step_block}        double t0 = m4_now();
{push_call}
        double t1 = m4_now();
        if (!briev_accel_launch_resident({QKIDX}, state, {NQK})) {{ fprintf(stderr, "qk failed\\n"); return 1; }}
        double t2a = m4_now();
        if (!briev_accel_launch_resident_2d({SMIDX}, state, 32, {H})) {{ fprintf(stderr, "softmax failed\\n"); return 1; }}
        double t2b = m4_now();
        if (!briev_accel_launch_resident({PVIDX}, state, {NPD})) {{ fprintf(stderr, "pv failed\\n"); return 1; }}
        double t2c = m4_now();
        push_t[step] = t1 - t0;
        qk_t[step] = t2a - t1;
        sm_t[step] = t2b - t2a;
        pv_t[step] = t2c - t2b;
    }}
    qsort(push_t, {REPS}, sizeof(double), m4_cmp);
    qsort(qk_t, {REPS}, sizeof(double), m4_cmp);
    qsort(sm_t, {REPS}, sizeof(double), m4_cmp);
    qsort(pv_t, {REPS}, sizeof(double), m4_cmp);
    printf("briev decode D={D} H={H} HKV={HKV} NKV={NKV}:\\n");
    printf("  push    p50=%.1f us\\n", push_t[{REPS} / 2]);
    printf("  qk      p50=%.1f us\\n", qk_t[{REPS} / 2]);
    printf("  softmax p50=%.1f us\\n", sm_t[{REPS} / 2]);
    printf("  pv      p50=%.1f us\\n", pv_t[{REPS} / 2]);
    printf("  chain   p50=%.1f us | total p50=%.1f us\\n",
           qk_t[{REPS} / 2] + sm_t[{REPS} / 2] + pv_t[{REPS} / 2],
           push_t[{REPS} / 2] + qk_t[{REPS} / 2] + sm_t[{REPS} / 2] + pv_t[{REPS} / 2]);
    free(push_t); free(qk_t); free(sm_t); free(pv_t);
{verify_block}    briev_accel_shutdown();
    return 0;
}}
'''

prefix = src.split('int main(void) {')[0]
open(out_path, 'w').write(prefix + main)
print("m4 shim injected", file=sys.stderr)
PYEOF

# 2026-09-21 (Family K): orchestration + driver archives (Rust-built).
cc -O2 -I"$OUT" -L"$OUT" -o "$OUT/m4_briev" "$OUT/m4_briev.c" -lbriev_accel_rt -lbriev_gpu_rt -lvulkan -lOpenCL -lcuda -lpthread -lm
echo "== BRIEV (CUDA lane) =="
BRIEV_ACCEL_DEVICE=cuda "$OUT/m4_briev"

# ── 2. ggml stock fattn reference ────────────────────────────────────────────
# test-backend-ops perf presets at hsk=128, nb=1 (decode), f16 KV — the
# stock fattn per-layer time at kv=$NKV. Head count is nearly free at fixed
# kv (KV-read-bound: nr23=[1,1]/[4,1]/[8,1] rows are flat), so bitnet's
# H=20/HKV=5 sits on the same rows.
echo "== GGML STOCK FATTN (test-backend-ops, nb=1, kv=$NKV) =="
CUDA_VISIBLE_DEVICES=0 "$CYBER/build/bin/test-backend-ops" perf \
    -o FLASH_ATTN_EXT -p "kv=$NKV,nb=1" 2>&1 \
    | grep -E "hsk=128" -A1 | grep "runs -" || true
