#!/usr/bin/env bash
# M3 harness (plan 2026-09-18-m3-matrix-and-m4-fattn-shim): build the
# decode-attention composition (qk -> softmax -> pv, GQA, f32), inject a
# deterministic seed + scalar CPU reference into the generated runner, run
# BOTH device lanes, gate on max relative error < 1e-3.
#
# Usage: bash benchmarks/m3_attention_harness.sh [NKV]   (default 256)
#        env D H HKV override the geometry (defaults 128/32/8; G = H/HKV).
set -euo pipefail
cd "$(dirname "$0")/.."

NKV="${1:-256}"
D="${D:-128}"
H="${H:-32}"
HKV="${HKV:-8}"
F16KV="${F16KV:-0}"
KLAYOUT="${KLAYOUT:-jd}"
[ $((H % HKV)) -eq 0 ] || { echo "H=$H not divisible by HKV=$HKV" >&2; exit 1; }
G=$((H / HKV))
BRIEVC=./target/release/brievc
OUT=$(mktemp -d /tmp/opencode/m3.XXXX)

# 1. Instantiate the .abv at the requested geometry.
F16FLAG=""
[ "$F16KV" = "1" ] && F16FLAG="--f16-kv"
TMPL="${TEMPLATE:-examples/gpu/attention_decode.abv}"
[ "$KLAYOUT" = "dmaj" ] && TMPL="examples/gpu/attention_decode_kdmaj.abv"
python3 benchmarks/attn_instantiate.py \
    --d "$D" --h "$H" --hkv "$HKV" --nkv "$NKV" $F16FLAG \
    --template "$TMPL" \
    --out examples/gpu/m3_attn_tmp.abv
# Float16 fields need the stdlib type in scope (float.bv declares it).
[ "$F16KV" = "1" ] && sed -i '1i import "std/types/float.bv";' examples/gpu/m3_attn_tmp.abv

# 2. Build the dual-image runner.
"$BRIEVC" build examples/gpu/m3_attn_tmp.abv --out "$OUT" >/dev/null
mv "$OUT/m3_attn_tmp_runner.c" "$OUT/attention_decode_runner.c"
rm -f examples/gpu/m3_attn_tmp.abv
for kp in 0 1 2; do
    # 2026-09-20 (Front C): the composite form emits 2 kernels, not 3 —
    # a missing blob length is expected, not fatal.
    grep -o "kp${kp}_len = [0-9]*u" "$OUT/attention_decode_runner.c" || true
done

# 3. Inject the harness (seed + CPU reference + verdict).
python3 - "$OUT/attention_decode_runner.c" "$OUT/harness.c" "$NKV" "$D" "$H" "$HKV" "$G" "$F16KV" "$KLAYOUT" <<'PYEOF'
import math, re, sys

runner_path, out_path = sys.argv[1], sys.argv[2]
NKV, D, H, HKV, G, F16KV, KLAYOUT = (int(x) if x.isdigit() else x for x in sys.argv[3:10])
import os
src = open(runner_path).read()

# 2026-09-20 (Front C, plan metaprogrammed-composites): in COMPOSITE mode
# the program is the ONE-node composite form — no score buffer s (the
# composite reads q/k directly) and o1 is the accumulator SCRATCH. The
# output contract is a_out only; the device-s and device-o1 checks are
# compiled out (the reference still computes scores internally).
COMPOSITE = os.environ.get("COMPOSITE") == "1"

def field(name):
    m = re.search(r'\{ "%s", 1, (\d+), (\d+),' % name, src)
    return int(m.group(1)), int(m.group(2))  # host_offset, elem_bytes

Q, QEB = field('q'); K, KEB = field('k'); V, VEB = field('v'); A, _ = field('a_out')
SOFF = None if COMPOSITE else field('s')[0]
F16 = KEB == 2 or VEB == 2
if F16KV != (1 if F16 else 0):
    raise SystemExit(f"f16-kv flag mismatch: harness={F16KV} tables k/v elem={KEB}/{VEB}")
QLEN, KVLEN = H * D, HKV * NKV * D
scale = 1.0 / math.sqrt(D)

# P0 f16 swap: k/v seed as _Float16 (2-byte elements) when the tables say 2;
# the CPU reference decodes the SAME f16 values, so the 1e-3 gate measures
# pure composition error (f16 quantization is the pipeline's input contract,
# shared with ggml's f16 KV — not an error source here).
kv_decl = (
    "    unsigned short* v_ = (unsigned short*)(state + VOFF);\n"
    if F16
    else "    float* v_ = (float*)(state + VOFF);\n"
)
kv_store_f = "{k_}[i] = (unsigned short)(_Float16)(((rng>>16)%997)/997.0f - 0.5f);" if F16 \
    else "{k_}[i] = (float)((rng>>16)%997)/997.0f - 0.5f;"
bak_f = "    {b}[i] = (float)((_Float16*)({p}))[i];" if F16 else "    {b}[i] = ((float*)({p}))[i];"

# M1 layout experiment (plan coalesced-kv-memory-path): KLAYOUT=dmaj stores
# K d-major in STATE (kh, d, j) while the seed/reference keep j-major host
# arrays — one scatter pass in, one rebuild pass out, values identical.
# The index uses the KOFF token convention so the plain-string replace
# below injects the offset.
if KLAYOUT == "dmaj":
    kidx = "(size_t)kh_ * (KSCATTER_BASE) + (size_t)d_ * (NKVS) + (size_t)j_"
else:
    kidx = "(size_t)kh_ * (NKVS) * (DS) + (size_t)j_ * (DS) + (size_t)d_"
kstore = ("(unsigned short)(_Float16)kv" if F16 else "kv")
kload = ("(float)((_Float16*)(state + KOFF))[" + kidx + "]" if F16 else
         "((float*)(state + KOFF))[" + kidx + "]")
kscatter = (
    "    for (int kh_ = 0; kh_ < KHKV_N; kh_++)\n"
    "      for (int j_ = 0; j_ < NKV_N; j_++)\n"
    "        for (int d_ = 0; d_ < D_N; d_++) {\n"
    "          float kv = kref[(size_t)kh_ * (NKV_N) * (D_N) + (size_t)j_ * (D_N) + d_];\n"
    "          " + ("((unsigned short*)(state + KOFF))[" if F16 else "((float*)(state + KOFF))[") + kidx + "] = " + kstore + ";\n"
    "        }\n"
)
krebuild = (
    "    for (int kh_ = 0; kh_ < KHKV_N; kh_++)\n"
    "      for (int j_ = 0; j_ < NKV_N; j_++)\n"
    "        for (int d_ = 0; d_ < D_N; d_++) {\n"
    "          Kbak[(size_t)kh_ * (NKV_N) * (D_N) + (size_t)j_ * (D_N) + d_] = " + kload + ";\n"
    "        }\n"
)

seed = (
    "\n  float* kref = (float*)malloc((size_t)" + str(KVLEN) + " * sizeof(float));\n"
    "  { unsigned rng = 12345;\n"
    "    float* q_ = (float*)(state + QOFF);\n"
    + kv_decl
    + f"    for (long long i = 0; i < {QLEN}; i++) {{ rng = rng*1103515245u+12345u; q_[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}\n"
    + f"    for (long long i = 0; i < {KVLEN}; i++) {{ rng = rng*1103515245u+12345u; kref[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}\n"
    + f"    for (long long i = 0; i < {KVLEN}; i++) {{ rng = rng*1103515245u+12345u; " + kv_store_f.format(k_='v_') + " }\n"
    "  }\n"
    "  /* K: seed j-major into kref, scatter into STATE at the layout's\n"
    "   * index (KLAYOUT=dmaj stores kh,d,j — the M1 coalescing experiment),\n"
    "   * then rebuild Kbak from the same storage so the CPU reference\n"
    "   * computes on what the device reads. V stays j-major. */\n"
    + f"  const int KHKV_N = {HKV}, NKV_N = {NKV}, D_N = {D}, KSCATTER_BASE = {D * NKV}, NKVS = {NKV}, DS = {D};\n"
    + f"  float* Kbak = (float*)malloc((size_t){KVLEN} * sizeof(float));\n"
    + kscatter
    + krebuild
    + "  /* backup Q/V before composition (q and a_out alias in proj); V\n"
    "   * decodes the f16 storage so the reference computes on what the\n"
    "   * device actually reads */\n"
    f"  float* Qbak = (float*)malloc({QLEN} * sizeof(float));\n"
    f"  float* Vbak = (float*)malloc({KVLEN} * sizeof(float));\n"
    f"  memcpy(Qbak, state + QOFF, {QLEN} * sizeof(float));\n"
    f"    for (long long i = 0; i < {KVLEN}; i++) {{ " + bak_f.format(b='Vbak', p='state + VOFF') + " }\n"
    "  free(kref);\n"
)
tail = (
    "\n  {\n"
    f"    const int D_ = {D}, H_ = {H}, G_ = {G}, NKV_ = NKVOFF;\n"
    "    const float* Q = Qbak;\n"
    "    const float* K = Kbak;\n"
    "    const float* V = Vbak;\n"
    "    const float* A = (const float*)(state + AOFF);\n"
    + ("" if COMPOSITE else "    const float* S = (const float*)(state + SOFF);\n")
    + ("" if COMPOSITE else "    const float* O1 = (const float*)(state + O1OFF);\n")
    + "    float* Sref = (float*)malloc(sizeof(float) * H_ * NKV_);\n"
    "    float* Pref = (float*)malloc(sizeof(float) * H_ * NKV_);\n"
    "    double max_err = 0.0, s_err = 0.0, o1_err = 0.0;\n"
    "    double ref_a0 = 0.0, ref_a1 = 0.0;\n"
    "    float dump_a0 = 0.0f, dump_ref0 = 0.0f;\n"
    "    int n_bad = 0;\n"
    "    int ws_i = -1; double ws_dev = 0.0, ws_ref = 0.0;\n"
    f"    const float scale_ = {scale:.17g}f;\n"
    "    for (int h = 0; h < H_; h++) {\n"
    "        int kh = h / G_;\n"
    "        float mx = -1e30f;\n"
    "        for (int j = 0; j < NKV_; j++) {\n"
    "            double acc = 0.0;\n"
    "            for (int d = 0; d < D_; d++) acc += Q[h*D_+d] * K[kh*NKV_*D_ + j*D_ + d];\n"
    "            acc *= scale_;\n"
    "            Sref[h*NKV_+j] = acc;\n"
    "            if (acc > mx) mx = acc;\n"
    + ("" if COMPOSITE else
    "            double se = __builtin_fabs((double)S[h*NKV_+j] - (double)acc) / (__builtin_fabs((double)acc) + 1e-3);\n"
    "            if (se > s_err) { s_err = se; ws_i = h*NKV_+j; ws_dev = S[h*NKV_+j]; ws_ref = acc; }\n")
    + "        }\n"
    "        double sum = 0.0;\n"
    "        for (int j = 0; j < NKV_; j++) { Pref[h*NKV_+j] = __builtin_expf(Sref[h*NKV_+j] - mx); sum += Pref[h*NKV_+j]; }\n"
    "        for (int j = 0; j < NKV_; j++) { Pref[h*NKV_+j] /= sum; CHECK_O1 }\n"
    "        for (int d = 0; d < D_; d++) {\n"
    "            double ref_ = 0.0;\n"
    "            for (int j = 0; j < NKV_; j++) ref_ += Pref[h*NKV_+j] * V[kh*NKV_*D_ + j*D_ + d];\n"
    "            double err = __builtin_fabs((double)A[h*D_+d] - (double)ref_) / (__builtin_fabs((double)ref_) + 1e-3);\n"
    "            if (err > max_err) max_err = err;\n"
    "            if (err > 0.05 && n_bad < 5) { if (n_bad == 0) printf(\"BAD: \"); printf(\"[h=%d,d=%d] A=%.5f ref=%.5f  \", h, d, A[h*D_+d], ref_); n_bad++; }\n"
    "            if (h == 0 && d == 0) { dump_a0 = A[0]; dump_ref0 = ref_; }\n"
    "        }\n"
    "    }\n"
    "    ref_a0 = (double)A[0]; ref_a1 = (double)A[1];\n"
    "    free(Sref); free(Pref); free(Qbak); free(Kbak); free(Vbak);\n"
    "    printf(\"M3 s_err=%.3g o1_err=%.3g a_err=%.3g -> %s\\n\", s_err, o1_err, max_err, max_err < 1e-3 ? \"PASS\" : \"FAIL\");\n"
    "    printf(\"worst S: idx=%d dev=%.6f ref=%.6f | A0=%.6f ref=%.6f\\n\", ws_i, ws_dev, ws_ref, dump_a0, dump_ref0);\n"
    "  }\n"
)
# 2026-09-20 (Front C, plan metaprogrammed-composites): in COMPOSITE mode
# the o1 buffer is the fused node's accumulator SCRATCH — its value is not
# part of the semantic contract (the output contract is a_out + s). The
# o1-vs-Pref check is compiled out.
check_o1 = "SKIP" if __import__("os").environ.get("COMPOSITE") == "1" else "CHECK"
tail = tail.replace("CHECK_O1",
    'if (0) { double oe = 0.0; (void)oe; (void)Pref; }' if check_o1 == "SKIP" else
    '{ double oe = __builtin_fabs((double)O1[h*NKV_+j] - (double)Pref[h*NKV_+j]) / (Pref[h*NKV_+j] + 1e-6); if (oe > o1_err) o1_err = oe; }')

for key, val in ([('NKVOFF', NKV), ('QOFF', Q), ('KOFF', K), ('VOFF', V), ('AOFF', A)] if COMPOSITE else
                 [('SOFF', SOFF), ('O1OFF', field('o1')[0]), ('NKVOFF', NKV), ('QOFF', Q), ('KOFF', K), ('VOFF', V), ('AOFF', A)]):
    seed = seed.replace(key, str(val))
    tail = tail.replace(key, str(val))
tail = tail.replace('PASSFAIL', '%s')

src = src.replace('  briev_accel_shutdown();', tail + '  briev_accel_shutdown();')
src = src.replace('  long guard = 0;', seed + '  long guard = 0;')
open(out_path, 'w').write(src)
print("harness injected", file=sys.stderr)
PYEOF

# 4. Compile + run both lanes.
# 2026-09-21 (Family K): orchestration + driver archives (Rust-built).
cc -O2 -I"$OUT" -L"$OUT" -o "$OUT/harness" "$OUT/harness.c" -lbriev_accel_rt -lbriev_gpu_rt -lvulkan -lOpenCL -lcuda -lpthread -lm
echo "== CUDA lane =="
BRIEV_ACCEL_DEVICE=cuda "$OUT/harness" 2>/dev/null | grep -E "M3" || true
echo "== VULKAN lane =="
BRIEV_ACCEL_DEVICE=vulkan "$OUT/harness" 2>/dev/null | grep -E "M3" || true
echo "artifacts: $OUT"
