#!/usr/bin/env bash
# atomic_gate.sh — AtomicAddAt# device gate (plan
# 2026-10-01-atomic-element-rmw.md A5). N work items each atomically
# increment total[0]; a lost update (non-atomic lowering) shows up as a
# short count. BOTH lanes must count exactly N.
set -uo pipefail
FIXTURE=${1:-examples/gpu/atomic_inc.abv}
N=1024
OUT=$(mktemp -d /tmp/opencode/atomgate.XXXXXX)
NAME=$(basename "${FIXTURE%.abv}")
./target/release/brievc build "$FIXTURE" --out "$OUT" >/dev/null || { echo "build failed"; exit 1; }
RUNNER="$OUT/${NAME}_runner.c"
[ -f "$RUNNER" ] || { echo "no runner generated"; exit 1; }
python3 - "$RUNNER" "$N" "$NAME" <<'PYEOF'
import re, sys
runner_path, N, NAME = sys.argv[1], int(sys.argv[2]), sys.argv[3]
src = open(runner_path).read()
m = re.search(r'\{ "total", \d+, (\d+), (\d+),', src)
assert m, "total field not found in the desc"
OFF, EB = int(m.group(1)), int(m.group(2))
assert EB == 8, f"total elements must be i64, got {EB} bytes"
tail = f'''
  {{
    long long* total = (long long*)(state + {OFF});
    printf("GATE {NAME}: total=%lld expected={N} -> %s\\n",
           total[0], total[0] == {N} ? "PASS" : "FAIL");
  }}
'''
assert '  briev_accel_shutdown();' in src, 'shutdown marker missing'
src = src.replace('  briev_accel_shutdown();', tail + '  briev_accel_shutdown();')
open(runner_path, 'w').write(src)
print("gate harness injected", file=sys.stderr)
PYEOF
cc -O2 -I"$OUT" -L"$OUT" -o "$OUT/gate" "$RUNNER" -lbriev_accel_rt -lbriev_gpu_rt -lvulkan -lOpenCL -lcuda -lpthread -lm || { echo "gate compile failed"; exit 1; }
echo "== CUDA lane (authored PTX atom) =="
BRIEV_ACCEL_DEVICE=cuda "$OUT/gate" 2>/dev/null | grep GATE || true
echo "== VULKAN lane (OpAtomicIAdd) =="
BRIEV_ACCEL_DEVICE=vulkan "$OUT/gate" 2>/dev/null | grep GATE || true
echo "artifacts: $OUT"
