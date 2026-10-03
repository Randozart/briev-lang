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
# The patterned f64 bound: 1e-5 rel for f32 accumulators (the f32 fixtures;
# f16 fixtures keep their long-standing instrument, gemm_h_bench.c — this
# gate's all-ones mode still runs on them).
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

def field_off(name_):
    m = re.search(r'\{ "%s", 1, (\d+), (\d+),' % name_, src)
    assert m, f"field {name_} not in table"
    return int(m.group(1))  # host offset — the seed writes the host layout

off = {f: field_off(f) for f in ("a", "b", OUTFIELD)}
count = {"a": M * K, "b": K * N, OUTFIELD: M * N}

if '#include <stdlib.h>' not in src:
    src = src.replace('#include', '#include <stdlib.h>\n#include', 1)
if '#include <math.h>' not in src:
    src = src.replace('#include', '#include <math.h>\n#include', 1)

gate = f'''
static int gate_mode = 0;
static int elem_f16 = 0;
static void gate_seed(void) {{
  unsigned rng = 20260101;
  if (elem_f16) {{
    /* f16 buffers hold f16 BITS: 1.0f16 = 0x3C00. */
    unsigned short* a = (unsigned short*)(state + {off["a"]});
    unsigned short* b = (unsigned short*)(state + {off["b"]});
    for (int i = 0; i < {count["a"]}; i++) a[i] = 0x3C00;
    for (int i = 0; i < {count["b"]}; i++) b[i] = 0x3C00;
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
  modes=$(if [ "$ELEM" = "f32" ]; then echo "0 1"; else echo "0"; fi)
  for mode in $modes; do
    GATE_F16=$([ "$ELEM" = "f16" ] && echo 1 || echo 0) \
    out=$(BRIEV_ACCEL_DEVICE=$lane GATE_MODE=$mode GATE_F16=$([ "$ELEM" = "f16" ] && echo 1 || echo 0) "$OUT/gate" 2>/dev/null | grep -E "PASS|FAIL" | tail -2)
    echo "== $lane mode=$mode =="
    echo "$out"
    echo "$out" | grep -q "FAIL" && fail=1
    [ -z "$out" ] && { echo "NO OUTPUT — FAIL"; fail=1; }
  done
done
echo "DECLARED-MATMUL GATE: $([ $fail -eq 0 ] && echo PASS || echo FAIL)"
exit $fail
