#!/usr/bin/env bash
# declared_matmul_gate.sh — device correctness for the DECLARED matmul
# channel (plan 2026-10-03-declared-matmul-gemmplan-retirement.md,
# increment 4). The fixture declares `matmul!(...)`; this gate proves the
# declared build computes the right product on BOTH device lanes before
# any timing claim rides the channel.
#
# Protocol (the 5b standard): splice a deterministic seed + verifier into
# the GENERATED runner (the compiler's own desc — never a hand-built
# field table), then run BOTH device lanes:
#   mode 0 — all-ones: y == K exactly (integer-exact check).
#   mode 1 — patterned seed ((j%7)-class LCG): 16 spread rows verified
#            against an f64 reference recomputed C-side with the SAME
#            LCG (bit-exact by construction — no cross-language drift).
# The patterned f64 bound: 1e-5 rel for f32 accumulators; the f16 patterned
# mode mirrors gemm_h_bench.c (f16-exact periodic seeds (j%7)/4, (j%5)/2,
# 16 spread rows vs an f64 reference, rel gate 5e-3 — 1e-2 under
# BRIEV_GEMM_F16ACC). All-ones alone is blind to stale-fill races: a ring
# stage holding identical ones still sums to K — the f16 patterned mode is
# the shallow-K fill-race discriminator (2026-10-03, BUGS.md 2026-09-16).
#
# Usage: declared_matmul_gate.sh <fixture.abv> <M> <N> <K> [elem: f32|f16]
set -uo pipefail
cd "$(dirname "$0")/.."

FIXTURE=$1; M=$2; N=$3; K=$4; ELEM=${5:-f32}
OUTFIELD=${6:-y}   # the product field (chained fixtures: the LAST product)
case "$ELEM" in
  f32) TOL="1e-5" ;;
  f16) TOL="1e-2" ;;
  *) echo "elem must be f32 or f16"; exit 2 ;;
esac

BRIEVC=./target/release/brievc
OUT=$(mktemp -d /tmp/opencode/dmg.XXXXXX)

"$BRIEVC" build "$FIXTURE" --out "$OUT" >/dev/null || { echo "build failed: $FIXTURE"; exit 1; }
RUNNER=$(ls "$OUT"/*_runner.c | head -1)
[ -f "$RUNNER" ] || { echo "no runner generated"; exit 1; }

python3 - "$RUNNER" "$M" "$N" "$K" "$TOL" "$ELEM" "$OUTFIELD" <<'PYEOF'
import re, sys
runner, M, N, K, TOL, ELEM, OUTFIELD = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), sys.argv[5], sys.argv[6], sys.argv[7]
K_LOG2 = K.bit_length() - 1
assert K == (1 << K_LOG2), "f16 all-ones needs K = power of two"
elem_f16 = 1 if ELEM == "f16" else 0
src = open(runner).read()

def field_desc(name_):
    m = re.search(r'\{ "%s", 1, (\d+), (\d+), (\d+),' % name_, src)
    assert m, f"field {name_} not in table"
    # host offset (the seed writes the host layout), element size, count
    return int(m.group(1)), int(m.group(3))

off = {f: field_desc(f)[0] for f in ("a", "b", OUTFIELD)}
count = {"a": M * K, "b": K * N, OUTFIELD: M * N}
# A fixture whose fields are smaller than the shape (e.g. `Float16[MN]`
# for a non-cube where M*K > M*N) makes the seed overflow into neighbor
# fields — clobbering `i` stops the launch and the verifier then reads
# seed garbage. Fail loudly with the fix instead.
for f in ("a", "b", OUTFIELD):
    _, desc_count = field_desc(f)
    if desc_count < count[f]:
        print(f"fixture field '{f}' holds {desc_count} elements but this shape "
              f"needs {count[f]} — declare a[M*K], b[K*N], y[M*N] "
              f"(M={M} N={N} K={K})", file=sys.stderr)
        sys.exit(2)
count = {f: field_desc(f)[1] if f != OUTFIELD else count[f] for f in count}

if '#include <stdlib.h>' not in src:
    src = src.replace('#include', '#include <stdlib.h>\n#include', 1)
if '#include <math.h>' not in src:
    src = src.replace('#include', '#include <math.h>\n#include', 1)
if '#include <string.h>' not in src:
    src = src.replace('#include', '#include <string.h>\n#include', 1)

gate = f'''
/* f16 helpers — the exact encode/decode pair from gemm_h_bench.c (seeds
   must be f16 EXACT so the f64 reference has no cross-language drift). */
static unsigned short f32_to_f16(float v) {{
  unsigned int bits; memcpy(&bits, &v, 4);
  unsigned short sign = (unsigned short)((bits >> 16) & 0x8000u);
  int exp = (int)((bits >> 23) & 0xff);
  unsigned int mant = bits & 0x007fffffu;
  if (exp == 255) return (unsigned short)(mant ? (sign | 0x7e00u) : (sign | 0x7c00u));
  int u = exp - 127;
  if (u > 15) return (unsigned short)(sign | 0x7c00u);
  if (u >= -14) {{
    unsigned int m = mant >> 13;
    unsigned int rem = mant & 0x1fffu;
    if (rem > 0x1000u || (rem == 0x1000u && (m & 1u) == 1u)) m += 1;
    unsigned int e = (unsigned int)(u + 15);
    if (m == 0x400u) {{ m = 0; e += 1; }}
    if (e >= 31) return (unsigned short)(sign | 0x7c00u);
    return (unsigned short)(sign | (e << 10) | m);
  }}
  if (u >= -25) {{
    unsigned int combined = 0x00800000u | mant;
    unsigned int d = (unsigned int)(-(u + 1));
    unsigned int f10 = combined >> d;
    unsigned int rem = combined & ((1u << d) - 1u);
    unsigned int half = 1u << (d - 1);
    if (rem > half || (rem == half && (f10 & 1u) == 1u)) f10 += 1;
    if (f10 >= 0x400u) return (unsigned short)(sign | (1u << 10));
    return (unsigned short)(sign | f10);
  }}
  return sign;
}}
static double f16_to_f64(unsigned short h) {{
  double s = (h & 0x8000u) ? -1.0 : 1.0;
  int e = (h >> 10) & 0x1f;
  unsigned int m = h & 0x3ffu;
  if (e == 0) return s * ldexp((double)m, -24);
  if (e == 31) return m ? 0.0 / 0.0 : s * 1.0 / 0.0;
  return s * ldexp(1.0 + (double)m / 1024.0, e - 15);
}}
static int gate_mode = 0;
static int elem_f16 = 0;
static void gate_seed(void) {{
  unsigned rng = 20260101;
  if (elem_f16) {{
    /* f16 buffers hold f16 BITS: 1.0f16 = 0x3C00. mode 1: the f16-exact
       periodic seeds from gemm_h_bench.c — a stale-fill stage reads a
       different k-slice phase and the sampled rows detect it (all-ones
       cannot: every stage holds ones). */
    unsigned short* a = (unsigned short*)(state + {off["a"]});
    unsigned short* b = (unsigned short*)(state + {off["b"]});
    if (gate_mode == 0) {{
      for (int i = 0; i < {count["a"]}; i++) a[i] = 0x3C00;
      for (int i = 0; i < {count["b"]}; i++) b[i] = 0x3C00;
      return;
    }}
    if (gate_mode == 2) {{
      /* Index/count probe: a constant per row, b constant per column.
         Correct kernel: y = K*alpha(m)*beta(n); pitch / transpose /
         pairing errors decode in gate_verify mode 2. */
      for (unsigned long i = 0; i < {count["a"]}UL; i++) a[i] = f32_to_f16((float)((i / {K}UL) % 7 + 1) * 0.25f);
      for (unsigned long j = 0; j < {count["b"]}UL; j++) b[j] = f32_to_f16((float)((j % {N}UL) % 5 + 1) * 0.5f);
      return;
    }}
    for (int i = 0; i < {count["a"]}; i++) a[i] = f32_to_f16((float)(i % 7) * 0.25f);
    for (int i = 0; i < {count["b"]}; i++) b[i] = f32_to_f16((float)(i % 5) * 0.5f);
    return;
  }}
  float* a = (float*)(state + {off["a"]});
  float* b = (float*)(state + {off["b"]});
  if (gate_mode == 0) {{
    for (int i = 0; i < {count["a"]}; i++) a[i] = 1.0f;
    for (int i = 0; i < {count["b"]}; i++) b[i] = 1.0f;
  }} else {{
    for (int i = 0; i < {count["a"]}; i++) {{ rng = rng*1103515245u + 12345u; a[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}
    for (int i = 0; i < {count["b"]}; i++) {{ rng = rng*1103515245u + 12345u; b[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}
  }}
}}
static unsigned gate_rng = 20260101;
static float gate_draw(void) {{
  gate_rng = gate_rng*1103515245u + 12345u;
  return (float)((gate_rng>>16)%997)/997.0f - 0.5f;
}}
static void gate_skip(unsigned long n) {{
  for (unsigned long i = 0; i < n; i++) {{ gate_rng = gate_rng*1103515245u + 12345u; }}
}}
static int gate_verify(void) {{
  const float* a = (const float*)(state + {off["a"]});
  const float* b = (const float*)(state + {off["b"]});
  const float* y = (const float*)(state + {off[OUTFIELD]});
  if (gate_mode == 0) {{
    /* f16 fixtures: the output is f16 BITS — K = 2^e is exactly
       representable; its f16 pattern is (15+e)<<10. */
    if (elem_f16) {{
      unsigned short want = (unsigned short)(((15 + {K_LOG2}) << 10));
      const unsigned short* y16 = (const unsigned short*)(state + {off[OUTFIELD]});
      for (unsigned long i = 0; i < {count[OUTFIELD]}; i++) {{
        if (y16[i] != want) {{
          printf("all-ones FAIL at [%lu]: got 0x%04x want 0x%04x\\n", i, y16[i], want);
          return 1;
        }}
      }}
      printf("all-ones: EXACT PASS ({count[OUTFIELD]}/{count[OUTFIELD]})\\n");
      return 0;
    }}
    for (unsigned long i = 0; i < {count[OUTFIELD]}; i++) {{
      if (y[i] != (float){K}) {{
        printf("all-ones FAIL at [%lu]: got %.6g want {K}\\n", i, y[i]);
        return 1;
      }}
    }}
    printf("all-ones: EXACT PASS ({count[OUTFIELD]}/{count[OUTFIELD]})\\n");
    return 0;
  }}
  if (gate_mode == 2) {{
    /* Index/count probe decode: row-const a, col-const b. For each
       sampled cell print (a) k-count under the correct indices,
       (b) the effective a-row class and b-col class at count=K —
       pitch/transposition errors decode to the wrong class directly. */
    const unsigned short* y16 = (const unsigned short*)(state + {off[OUTFIELD]});
    unsigned long mism = 0, shown = 0;
    for (int r = 0; r < 16; r++) {{
      unsigned long m = (unsigned long)(r * 7919) % {M}UL;
      double am = (double)((m % 7) + 1) * 0.25;
      unsigned long row_bad = 0, first_n = 0, last_n = 0;
      for (unsigned long n = 0; n < {N}UL; n++) {{
        double bn = (double)((n % 5) + 1) * 0.5;
        double want = {K}.0 * am * bn;
        double got = f16_to_f64(y16[m * {N}UL + n]);
        if (fabs(got - want) <= 1e-9) continue;
        mism++;
        row_bad++;
        if (row_bad == 1) first_n = n;
        last_n = n;
        if (shown < 4) {{
          printf("probe2 m=%lu n=%lu got=%.6g want=%.6g cnt=%.4f acol=%.3f bcol=%.3f\\n",
                 m, n, got, want, got / (am * bn),
                 got / ({K}.0 * bn) / 0.25 - 1.0, got / ({K}.0 * am) / 0.5 - 1.0);
          shown++;
        }}
      }}
      if (row_bad && r < 2) {{
        /* run-length encode bad columns for the first two sampled rows */
        unsigned long run_start = 0, prev = 0; int in_run = 0;
        printf("probe2 RLE m=%lu: ", m);
        for (unsigned long n = 0; n < {N}UL; n++) {{
          double bn = (double)((n % 5) + 1) * 0.5;
          double want = {K}.0 * am * bn;
          double got = f16_to_f64(y16[m * {N}UL + n]);
          int bad = fabs(got - want) > 1e-9;
          if (bad && !in_run) {{ in_run = 1; run_start = n; }}
          if (!bad && in_run) {{ in_run = 0; printf("%lu-%lu ", run_start, prev); }}
          if (bad) prev = n;
        }}
        if (in_run) printf("%lu-%lu ", run_start, prev);
        printf("\\n");
      }}
      if (row_bad) printf("probe2 ROW m=%lu bad=%lu first_n=%lu last_n=%lu\\n", m, row_bad, first_n, last_n);
    }}
    printf("probe2: mismatches=%lu\\n", mism);
    return mism ? 1 : 0;
  }}
  if (elem_f16) {{
    /* f16 patterned: gemm_h_bench.c's sampled-row protocol — 16 spread
       rows vs an f64 reference built from the f16-exact seeds. The bound
       is the f16 store-rounding + f32 accumulation: 5e-3 (1e-2 under
       BRIEV_GEMM_F16ACC — ptx_tensor_f16acc raises accumulation error). */
    const unsigned short* a16 = (const unsigned short*)(state + {off["a"]});
    const unsigned short* b16 = (const unsigned short*)(state + {off["b"]});
    const unsigned short* y16 = (const unsigned short*)(state + {off[OUTFIELD]});
    double tol16 = getenv("BRIEV_GEMM_F16ACC") ? 1e-2 : 5e-3;
    double max_rel = 0.0;
    unsigned long worst_m = 0, worst_n = 0, bad = 0, checked = 0;
    double worst_got = 0.0, worst_ref = 0.0;
    for (int r = 0; r < 16; r++) {{
      unsigned long m = (unsigned long)(r * 7919) % {M}UL;
      for (unsigned long n = 0; n < {N}UL; n++) {{
        double ref = 0.0;
        for (unsigned long k = 0; k < {K}UL; k++) {{
          ref += f16_to_f64(a16[m * {K}UL + k]) * f16_to_f64(b16[k * {N}UL + n]);
        }}
        double got = f16_to_f64(y16[m * {N}UL + n]);
        double rel = ref == 0.0 ? fabs(got) : fabs(got - ref) / fabs(ref);
        checked++;
        if (rel > tol16) bad++;
        if (rel > max_rel) {{ max_rel = rel; worst_m = m; worst_n = n; worst_got = got; worst_ref = ref; }}
      }}
    }}
    if (max_rel > tol16) {{
      printf("patterned16 WORST: y[%lu,%lu]=%.6g ref=%.6g bad=%lu/%lu\\n", worst_m, worst_n, worst_got, worst_ref, bad, checked);
    }}
    int ok = max_rel <= tol16;
    printf("patterned16: max_rel=%.3e tol=%.3e -> %s\\n", max_rel, tol16, ok ? "PASS" : "FAIL");
    return ok ? 0 : 1;
  }}
  /* Regenerate the b stream ONCE (draws [count_a, count_a+K*N)) and each
     verified row's a slice (draws [m*K, m*K+K)) — sequential, O(K*N). */
  float* bf = (float*)malloc((size_t){count["b"]} * 4);
  float* ar = (float*)malloc((size_t){K} * 4);
  double* row = (double*)malloc((size_t){N} * 8);
  if (!bf || !ar || !row) {{ printf("patterned: oom\\n"); return 1; }}
  gate_rng = 20260101;
  gate_skip({count["a"]}UL);
  for (unsigned long i = 0; i < {count["b"]}UL; i++) bf[i] = gate_draw();
  /* Row-normalized error: |y-ref| / max_n |ref[row]| — a raw per-element
     rel explodes at near-zero references (cancellation), which measures
     the METRIC, not the kernel. The bound scales with K (f32
     accumulation: err ~ K * 2^-24 * row_scale). */
  double max_err = 0.0;
  for (int r = 0; r < 16; r++) {{
    unsigned long m = (unsigned long)(r * 7919) % {M}UL;
    gate_rng = 20260101;
    gate_skip(m * {K}UL);
    for (unsigned long k = 0; k < {K}UL; k++) ar[k] = gate_draw();
    double row_scale = 0.0;
    for (unsigned long n = 0; n < {N}UL; n++) {{
      double ref = 0.0;
      for (unsigned long k = 0; k < {K}UL; k++) {{
        ref += (double)ar[k] * (double)bf[k * {N}UL + n];
      }}
      row[n] = ref;
      if (fabs(ref) > row_scale) row_scale = fabs(ref);
    }}
    for (unsigned long n = 0; n < {N}UL; n++) {{
      double err = fabs(((double)y[m * {N}UL + n] - row[n]) / row_scale);
      if (err > max_err) max_err = err;
    }}
  }}
  free(bf); free(ar); free(row);
  double tol = {TOL} > {K}L * 2e-7 * 8 ? {TOL} : {K}L * 2e-7 * 8;
  int ok = max_err <= tol;
  printf("patterned: row_norm_err=%.3e tol=%.3e -> %s\\n", max_err, tol, ok ? "PASS" : "FAIL");
  return ok ? 0 : 1;
}}
'''
tail = '''
  printf("GATE-RESULT: %d\\n", gate_verify());
'''
assert '  briev_accel_shutdown();' in src, "shutdown marker"
src = src.replace('  briev_accel_shutdown();', tail + '  briev_accel_shutdown();')
src = src.replace('int main(void) {',
                  gate + 'int main(void) {\n  gate_mode = getenv("GATE_MODE") ? atoi(getenv("GATE_MODE")) : 0;\n  elem_f16 = getenv("GATE_F16") ? atoi(getenv("GATE_F16")) : 0;')
# The seed lands AFTER init, before the run loop (the AB-gate anchor) —
# the first resident launch seeds the projection from the authored bytes.
src = src.replace('  long guard = 0;', '  gate_seed();\n  long guard = 0;')
assert '  gate_seed();' in src
open(runner, 'w').write(src)
print("gate harness injected", file=sys.stderr)
PYEOF

cc -O2 -I"$OUT" -L"$OUT" -o "$OUT/gate" "$RUNNER" \
  -lbriev_accel_rt -lbriev_gpu_rt -lvulkan -lOpenCL -lcuda -lpthread -lm || { echo "compile failed"; exit 1; }

fail=0
for lane in cuda vulkan; do
  modes="0 1"
  for mode in $modes; do
    GATE_F16=$([ "$ELEM" = "f16" ] && echo 1 || echo 0) \
    out=$(BRIEV_ACCEL_DEVICE=$lane GATE_MODE=$mode GATE_F16=$([ "$ELEM" = "f16" ] && echo 1 || echo 0) "$OUT/gate" 2>/dev/null | grep -E "PASS|FAIL|WORST" | tail -3)
    echo "== $lane mode=$mode =="
    echo "$out"
    echo "$out" | grep -q "FAIL" && fail=1
    [ -z "$out" ] && { echo "NO OUTPUT — FAIL"; fail=1; }
  done
done
echo "DECLARED-MATMUL GATE: $([ $fail -eq 0 ] && echo PASS || echo FAIL)"
exit $fail
