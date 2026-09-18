#!/usr/bin/env bash
# M3 harness (plan 2026-09-17-abv-attention-ab): build the decode-attention
# composition (qk -> softmax -> pv, GQA, f32), inject a deterministic seed +
# scalar CPU reference into the generated runner, run BOTH device lanes,
# gate on max relative error < 1e-3.
#
# Usage: bash benchmarks/m3_attention_harness.sh [NKV]   (default 256)
set -euo pipefail
cd "$(dirname "$0")/.."

NKV="${1:-256}"
BRIEVC=./target/release/brievc
OUT=$(mktemp -d /tmp/opencode/m3.XXXX)

# 1. Instantiate the .abv (D/H/HKV/G fixed to the bitnet-2b decode geometry).
sed "s/const NKV: Int = 256;/const NKV: Int = ${NKV};/" \
    examples/gpu/attention_decode.abv > examples/gpu/m3_attn_tmp.abv

# 2. Build the dual-image runner.
"$BRIEVC" build examples/gpu/m3_attn_tmp.abv --out "$OUT" >/dev/null
mv "$OUT/m3_attn_tmp_runner.c" "$OUT/attention_decode_runner.c"
rm -f examples/gpu/m3_attn_tmp.abv
for kp in 0 1 2; do
    grep -o "kp${kp}_len = [0-9]*u" "$OUT/attention_decode_runner.c"
done

# 3. Inject the harness (seed + CPU reference + verdict).
python3 - "$OUT/attention_decode_runner.c" "$OUT/harness.c" "$NKV" <<'PYEOF'
import re, sys

runner_path, out_path, NKV = sys.argv[1], sys.argv[2], int(sys.argv[3])
src = open(runner_path).read()

def off(name):
    m = re.search(r'\{ "%s", 1, (\d+), 4,' % name, src)
    return int(m.group(1))

Q, K, V, A = off('q'), off('k'), off('v'), off('a_out')

seed = (
    "\n  { unsigned rng = 12345;\n"
    "    float* q_ = (float*)(state + QOFF);\n"
    "    float* k_ = (float*)(state + KOFF);\n"
    "    float* v_ = (float*)(state + VOFF);\n"
    "    for (long long i = 0; i < 4096; i++) { rng = rng*1103515245u+12345u; q_[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }\n"
    "    for (long long i = 0; i < 262144; i++) { rng = rng*1103515245u+12345u; k_[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }\n"
    "    for (long long i = 0; i < 262144; i++) { rng = rng*1103515245u+12345u; v_[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }\n"
    "  }\n"
    "  /* backup Q/K/V before composition (q and a_out alias in proj) */\n"
    "  float* Qbak = (float*)malloc(4096 * sizeof(float));\n"
    "  float* Kbak = (float*)malloc(262144 * sizeof(float));\n"
    "  float* Vbak = (float*)malloc(262144 * sizeof(float));\n"
    "  memcpy(Qbak, state + QOFF, 4096 * sizeof(float));\n"
    "  memcpy(Kbak, state + KOFF, 262144 * sizeof(float));\n"
    "  memcpy(Vbak, state + VOFF, 262144 * sizeof(float));\n"
)
tail = (
    "\n  {\n"
    "    const int D_ = 128, H_ = 32, G_ = 4, NKV_ = NKVOFF;\n"
    "    const float* Q = Qbak;\n"
    "    const float* K = Kbak;\n"
    "    const float* V = Vbak;\n"
    "    const float* A = (const float*)(state + AOFF);\n"
    "    const float* S = (const float*)(state + SOFF);\n"
    "    const float* O1 = (const float*)(state + O1OFF);\n"
    "    float* Sref = (float*)malloc(sizeof(float) * H_ * NKV_);\n"
    "    float* Pref = (float*)malloc(sizeof(float) * H_ * NKV_);\n"
    "    double max_err = 0.0, s_err = 0.0, o1_err = 0.0;\n"
    "    double ref_a0 = 0.0, ref_a1 = 0.0;\n"
    "    float dump_a0 = 0.0f, dump_ref0 = 0.0f;\n"
    "    int n_bad = 0;\n"
    "    int ws_i = -1; double ws_dev = 0.0, ws_ref = 0.0;\n"
    "    const float scale_ = 0.08838834764831845f;\n"
    "    for (int h = 0; h < H_; h++) {\n"
    "        int kh = h / G_;\n"
    "        float mx = -1e30f;\n"
    "        for (int j = 0; j < NKV_; j++) {\n"
    "            double acc = 0.0;\n"
    "            for (int d = 0; d < D_; d++) acc += Q[h*D_+d] * K[kh*NKV_*D_ + j*D_ + d];\n"
    "            acc *= scale_;\n"
    "            Sref[h*NKV_+j] = acc;\n"
    "            if (acc > mx) mx = acc;\n"
    "            double se = __builtin_fabs((double)S[h*NKV_+j] - (double)acc) / (__builtin_fabs((double)acc) + 1e-3);\n"
    "            if (se > s_err) { s_err = se; ws_i = h*NKV_+j; ws_dev = S[h*NKV_+j]; ws_ref = acc; }\n"
    "        }\n"
    "        double sum = 0.0;\n"
    "        for (int j = 0; j < NKV_; j++) { Pref[h*NKV_+j] = __builtin_expf(Sref[h*NKV_+j] - mx); sum += Pref[h*NKV_+j]; }\n"
    "        for (int j = 0; j < NKV_; j++) { Pref[h*NKV_+j] /= sum; double oe = __builtin_fabs((double)O1[h*NKV_+j] - (double)Pref[h*NKV_+j]) / (Pref[h*NKV_+j] + 1e-6); if (oe > o1_err) o1_err = oe; }\n"
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
for key, val in [('SOFF', off('s')), ('O1OFF', off('o1')), ('NKVOFF', NKV), ('QOFF', Q), ('KOFF', K), ('VOFF', V), ('AOFF', A)]:
    seed = seed.replace(key, str(val))
    tail = tail.replace(key, str(val))
tail = tail.replace('PASSFAIL', '%s')

src = src.replace('  briev_accel_shutdown();', tail + '  briev_accel_shutdown();')
src = src.replace('  long guard = 0;', seed + '  long guard = 0;')
open(out_path, 'w').write(src)
print("harness injected", file=sys.stderr)
PYEOF

# 4. Compile + run both lanes.
cc -O2 -I"$OUT" -o "$OUT/harness" "$OUT/harness.c" -lvulkan -lOpenCL -lcuda -lpthread -lm
echo "== CUDA lane =="
BRIEV_ACCEL_DEVICE=cuda "$OUT/harness" 2>/dev/null | grep -E "M3" || true
echo "== VULKAN lane =="
BRIEV_ACCEL_DEVICE=vulkan "$OUT/harness" 2>/dev/null | grep -E "M3" || true
echo "artifacts: $OUT"
