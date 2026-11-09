#!/usr/bin/env bash
# Intel lane ceiling matrix — hand-written OpenCL C references, device time.
# Run on the PTL node. Values: copy/elem bandwidth, reduce, GEMM 512^3/4096^3.
set -euo pipefail
cd "$(dirname "$0")"
cc -O2 -o ceiling_harness ceiling_harness.c -ldl -lm
echo "# device: $(clinfo 2>/dev/null | grep -m1 'Device Name' | sed 's/.*: //' || echo unknown)"
for dt in f32 f16; do
    ./ceiling_harness copy   "$dt" 1048576     0    0 20
    ./ceiling_harness elem   "$dt" 1048576     0    0 20
    ./ceiling_harness reduce "$dt"    4096  1024    0 20
    ./ceiling_harness gemm   "$dt"     512   512  512 10
    ./ceiling_harness gemm   "$dt"    4096  4096 4096 3
done
