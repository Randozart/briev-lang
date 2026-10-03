#!/usr/bin/env bash
# emitter_ab_gate.sh — the cooperative-emitter retirement A/B instrument
# (plan 2026-10-03-emitter-retirement-ab.md). Splices a timed repeat
# loop + an exact verifier into the GENERATED runner (the compiler's own
# desc), then runs BOTH lanes per knob arm.
#
# softmax_rows protocol: the state is zero-initialized, so logits ≡ 0 →
# max = 0, Exp#(0) = 1, the row sum = NKV, and EVERY output is exactly
# 1/NKV (f32-exact for NKV = 2^8: the sum and the divide are exact).
# The verifier checks the f32 BIT PATTERN over the whole output.
# The timed loop resets the work counter and re-launches the node
# ITERS times (the kernel reads logits, writes result — idempotent).
#
# Usage: emitter_ab_gate.sh <fixture.abv> <rows> <nkv> <knob: on|off> <iters> [kind]
# kind=softmax (default): zero-seed, every output = 1/NKV (bitwise).
# kind=dot: all-ones seed, every output = K exactly (the DECLARED dot!
# semantics: o[w] = sum x*y). The dot fixture's runner dispatches the same
# 32-lane x rows geometry, so the timed loop is shared.
set -uo pipefail
cd "$(dirname "$0")/.."

FIXTURE=$1; ROWS=$2; NKV=$3; KNOB=${4:-on}; ITERS=${5:-500}; KIND=${6:-softmax}
case "$KNOB" in
  on)  COOP=1 ;;
  off) COOP=0 ;;
  *) echo "knob must be on|off"; exit 2 ;;
esac
case "$KIND" in
  softmax|dot) ;;
  *) echo "kind must be softmax|dot"; exit 2 ;;
esac

mkdir -p /tmp/opencode/ab_knob_$COOP
printf 'spirv_row_cooperative: %d;\n' "$COOP" > /tmp/opencode/ab_knob_$COOP/ir-lowering.dbvl

BRIEVC=./target/release/brievc
OUT=$(mktemp -d /tmp/opencode/eab.XXXXXX)

"$BRIEVC" build "$FIXTURE" --config-dir /tmp/opencode/ab_knob_$COOP --out "$OUT" \
  >/dev/null || { echo "build failed"; exit 1; }
RUNNER=$(ls "$OUT"/*_runner.c | head -1)
[ -f "$RUNNER" ] || { echo "no runner"; exit 1; }

python3 - "$RUNNER" "$ROWS" "$NKV" "$ITERS" "$KIND" "$NKV" <<'PYEOF'
import re, sys
runner, ROWS, NKV, ITERS, KIND, K = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), sys.argv[5], int(sys.argv[6])
src = open(runner).read()

def field_off(name_):
    m = re.search(r'\{ "%s", [0-9]+, (\d+), ([0-9]+),' % name_, src)
    assert m, f"field {name_} not in table"
    return int(m.group(1))

# the output field: softmax -> result; dot -> o (the fixture's output).
out_field = "result" if KIND == "softmax" else "o"
off_out = field_off(out_field)
# softmax: count = ROWS*NKV outputs; dot: count = ROWS outputs (o[w], w < ROWS).
count = ROWS * NKV if KIND == "softmax" else ROWS

if '#include <time.h>' not in src:
    src = src.replace('#include', '#include <time.h>\n#include', 1)

# kind=dot: the all-ones seed (x = rows*K, y = K) spliced after init —
# the softmax fixture needs NO seed (zero logits = the uniform case).
seed_block = ""
if KIND == "dot":
    def field_off2(name_):
        m = re.search(r'\{ "%s", [0-9]+, (\d+), [0-9]+,' % name_, src)
        assert m, f"field {name_} not in table"
        return int(m.group(1))
    ox, oy = field_off2("x"), field_off2("y")
    seed_block = (
        "  {\n"
        f"    float* x = (float*)(state + {ox});\n"
        f"    float* y = (float*)(state + {oy});\n"
        f"    for (int i = 0; i < {ROWS * K}; i++) x[i] = 1.0f;\n"
        f"    for (int i = 0; i < {K}; i++) y[i] = 1.0f;\n"
        "  }\n"
    )

if KIND == "softmax":
    want_bits = hex(0x3B800000)  # 1/NKV as f32 bits (NKV = 256)
else:
    import struct
    want_bits = hex(struct.unpack("<I", struct.pack("<f", float(K)))[0])  # K as f32 bits
tail = f'''
  {{
    // the timed repeat: reset the work counter, re-launch the node
    int iters = {ITERS};
    long long* iv = (long long*)(state + {field_off("i") if '"i"' in src else 0});
    struct timespec ta, tb;
    clock_gettime(CLOCK_MONOTONIC, &ta);
    for (int it = 0; it < iters; it++) {{
      *iv = 0;
      if (!briev_accel_launch_resident_2d(0, state, 32, {ROWS})) {{
        fprintf(stderr, "dispatch failed\\\\n");
        return 1;
      }}
    }}
    clock_gettime(CLOCK_MONOTONIC, &tb);
    double ns = ((double)(tb.tv_sec - ta.tv_sec) * 1e9 + (double)(tb.tv_nsec - ta.tv_nsec)) / (double)iters;
    printf("TIMING_US_PER_LAUNCH: %.3f\\\\n", ns / 1000.0);
    // exact verifier: zero logits -> every output = 1/{NKV}
    const unsigned* r = (const unsigned*)(state + {off_out});
    unsigned want = {want_bits};
    int bad = 0;
    for (int i = 0; i < {count}; i++) {{
      if (r[i] != want) {{ bad++; }}
    }}
    printf("VERIFY: %s (%d bad of {count})\\\\n", bad == 0 ? "EXACT PASS" : "FAIL", bad);
  }}
'''
assert '  briev_accel_shutdown();' in src, "shutdown marker"
src = src.replace('  briev_accel_shutdown();', tail + '  briev_accel_shutdown();')
if KIND == "dot":
    # the all-ones seed BEFORE the reactor loop (the softmax = zero-init):
    # inline statements at the top of main.
    src = src.replace('  long guard = 0;', seed_block + '  long guard = 0;')
open(runner, 'w').write(src)
print("gate harness injected", file=sys.stderr)
PYEOF

cc -O2 -I"$OUT" -L"$OUT" -o "$OUT/gate" "$RUNNER" \
  -lbriev_accel_rt -lbriev_gpu_rt -lvulkan -lOpenCL -lcuda -lpthread -lm || { echo "compile failed"; exit 1; }

echo "=== KNOB=$KNOB ITERS=$ITERS ==="
for lane in cuda vulkan; do
  out=$(BRIEV_ACCEL_DEVICE=$lane "$OUT/gate" 2>/dev/null | grep -E "TIMING|VERIFY")
  echo "$lane: $out"
done
