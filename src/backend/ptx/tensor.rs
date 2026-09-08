//! Tensor GEMM PTX emission (plan 2026-09-08-ptx-tier-execution S3b).
//!
//! Correctness-first tiled GEMM using `mma.sync.m16n8k16`. Geometry:
//!
//! - Block = 32 threads (1 warp), warp tile = **32×16** C (2 M-halves ×
//!   2 N-groups = 4 mma per k-step, 16 f32 accumulator registers).
//! - Grid = (M/32) × (N/16) blocks, decoded from `ctaid.y` (the runner's
//!   cooperative 32-lane dispatch: nx=32, ny=blocks).
//! - K-loop steps by 16 (the mma k-dimension); A/B fragments load fresh
//!   from GLOBAL each step (correctness-first — smem/ldmatrix/cp.async
//!   arrive in S3b+).
//! - A/B are f16 (2 bytes); C is f32 (4 bytes) or f16 (2 bytes, cvt at
//!   store).
//!
//! Fragment layout (S3a-locked):
//!   A: a0={A[r][2t],A[r][2t+1]} a1={A[r+8][2t],A[r+8][2t+1]}
//!      a2={A[r][2t+8],A[r][2t+9]} a3={A[r+8][2t+8],A[r+8][2t+9]}
//!   B: b0={B[2t][c],B[2t+1][c]} b1={B[2t+8][c],B[2t+9][c]}
//!   C: c0=C[g][2t] c1=C[g][2t+1] c2=C[g+8][2t] c3=C[g+8][2t+1]
//! where g=lane>>2, t=lane&3.
//!
//! Register contract (fixed, no aliasing):
//!   %rd1 proj base | %rd2 a-tile base | %rd3 b-tile base | %rd6 c-tile base
//!   %rd4 64-bit offset scratch | %rd5 64-bit addr scratch
//!   %r3 m_cta | %r4 n_cta | %r6 g | %r8 2t | %r10 kstep | %r11 kstep*32
//!   %r12 kstep | %r13 offset32 scratch | %r14 scratch

/// Emit the tensor GEMM PTX for a fixed M×N×K with the given device
/// projection offsets. `y_elem` = 4 (f32) or 2 (f16 — cvt at store).
pub fn tensor_gemm_ptx(
    m: i64,
    n: i64,
    k: i64,
    a_off: u64,
    b_off: u64,
    y_off: u64,
    y_elem: u32,
) -> String {
    debug_assert!(m % 32 == 0, "tensor tier: M%32");
    debug_assert!(n % 16 == 0, "tensor tier: N%16");
    debug_assert!(k % 16 == 0, "tensor tier: K%16");
    let a_row = k * 2; // A row stride (bytes): K f16
    let b_row = n * 2; // B row stride (bytes): N f16
    let y_row = n * 4; // C row stride (bytes): N f32
    let mut out = String::new();

    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n");
    out.push_str(".visible .entry main (.param .b64 proj_param)\n{\n");
    out.push_str("    .reg .b64  %rd1, %rd2, %rd3, %rd4, %rd5, %rd6;\n");
    out.push_str("    .reg .u32  %r1, %r2, %r3, %r4, %r5, %r6, %r7, %r8, %r9, %r10;\n");
    out.push_str("    .reg .u32  %r11, %r12, %r13, %r14, %r15;\n");
    out.push_str("    .reg .b32  %a0, %a1, %a2, %a3, %b0, %b1, %t0, %t1, %u0, %u1;\n");
    out.push_str("    .reg .f32  %c0, %c1, %c2, %c3, %c4, %c5, %c6, %c7, %c8, %c9;\n");
    out.push_str("    .reg .f32  %c10, %c11, %c12, %c13, %c14, %c15;\n");
    out.push_str("    .reg .pred %p1;\n");
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");

    // Block decode: block id = ctaid.y (32-lane cooperative dispatch).
    out.push_str("    mov.u32 %r1, %ctaid.y;\n");
    out.push_str(&format!("    mov.u32 %r2, {};\n", n / 16)); // blocks per M-row
    out.push_str("    div.u32 %r3, %r1, %r2;  // m_cta\n");
    out.push_str("    rem.u32 %r4, %r1, %r2;  // n_cta\n");
    out.push_str("    mov.u32 %r5, %tid.x;    // lane\n");
    out.push_str("    setp.ge.u32 %p1, %r5, 32;\n");
    out.push_str("    @%p1 ret;  // driver launches 64-thread blocks; this warp-tile uses 32\n");
    out.push_str("    shr.u32 %r6, %r5, 2;    // g\n");
    out.push_str("    and.b32 %r7, %r5, 3;    // t\n");
    out.push_str("    shl.b32 %r8, %r7, 1;    // 2t\n");

    // rd2 = proj + a_off + m_cta*32*a_row
    out.push_str("    mov.u32 %r9, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r9, %r9, {};\n", 32 * a_row));
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str(&format!("    mov.u64 %rd2, {};\n", a_off));
    out.push_str("    add.u64 %rd2, %rd1, %rd2;\n");
    out.push_str("    add.u64 %rd2, %rd2, %rd4;\n");
    // rd3 = proj + b_off + n_cta*32 (n_cta*16 cols * 2 bytes)
    out.push_str("    mov.u32 %r9, %r4;\n");
    out.push_str("    mul.lo.u32 %r9, %r9, 32;\n");
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str(&format!("    mov.u64 %rd3, {};\n", b_off));
    out.push_str("    add.u64 %rd3, %rd1, %rd3;\n");
    out.push_str("    add.u64 %rd3, %rd3, %rd4;\n");
    // rd6 = proj + y_off + m_cta*32*y_row + n_cta*64 (n_cta*16 cols * 4 bytes)
    out.push_str("    mov.u32 %r9, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r9, %r9, {};\n", 32 * y_row));
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str(&format!("    mov.u64 %rd6, {};\n", y_off));
    out.push_str("    add.u64 %rd6, %rd1, %rd6;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");
    out.push_str("    mov.u32 %r9, %r4;\n");
    out.push_str("    mul.lo.u32 %r9, %r9, 64;\n");
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");

    // Zero C.
    for i in 0..16 {
        out.push_str(&format!("    mov.f32 %c{}, 0f00000000;\n", i));
    }

    // K loop.
    out.push_str("    mov.u32 %r10, 0;  // kstep\nKLOOP:\n");
    out.push_str(&format!("    setp.ge.u32 %p1, %r10, {};\n", k));
    out.push_str("    @%p1 bra KEND;\n");
    out.push_str("    mov.u32 %r11, %r10;\n");
    out.push_str(&format!("    mul.lo.u32 %r11, %r11, {};\n", 2)); // kstep (element offset) * 2 bytes
    out.push_str("    mov.u32 %r12, %r10;  // kstep for B rows\n");

    for mh in 0..2 {
        // A fragment for half mh. Tile-relative row = mh*16 + row_off + g.
        // Byte = (mh*16+row_off)*a_row + g*a_row + kstep*32 + 2t + col*2
        for (reg, row_off, col) in [
            ("a0", 0i64, 0i64),
            ("a1", 8, 0),
            ("a2", 0, 8),
            ("a3", 8, 8),
        ] {
            out.push_str(&format!("    mov.u32 %r13, {};\n", (16 * mh + row_off) * a_row));
            out.push_str(&format!("    mul.lo.u32 %r14, %r6, {};\n", a_row));
            out.push_str("    add.u32 %r13, %r13, %r14;\n"); // row bytes
            out.push_str("    mov.u32 %r14, %r8;\n");
            out.push_str("    shl.b32 %r14, %r14, 1;\n");      // 2t*2
            out.push_str("    add.u32 %r14, %r14, %r11;\n");  // + kstep*32
            out.push_str(&format!("    add.u32 %r14, %r14, {};\n", col * 2));
            out.push_str("    add.u32 %r13, %r13, %r14;\n");  // + col bytes
            out.push_str("    mul.wide.u32 %rd4, %r13, 1;\n");
            out.push_str("    mul.wide.u32 %rd4, %r13, 1;\n");
            out.push_str("    add.u64 %rd5, %rd2, %rd4;\n");
            out.push_str("    ld.global.u16 %t0, [%rd5];\n");
            out.push_str("    add.u64 %rd5, %rd5, 2;\n");
            out.push_str("    ld.global.u16 %t1, [%rd5];\n");
            out.push_str("    shl.b32 %t1, %t1, 16;\n");
            out.push_str(&format!("    or.b32 %{}, %t0, %t1;\n", reg));
        }

        for ng in 0..2 {
            // B fragment for group ng. Tile-relative row = kstep*16 + krow + 2t.
            // Byte = (kstep*16+krow+2t)*b_row + (ng*8 + g)*2
            for (reg, krow) in [("b0", 0i64), ("b1", 8)] {
                out.push_str(&format!("    mov.u32 %r13, {};\n", krow * b_row));
                out.push_str(&format!("    mul.lo.u32 %r14, %r12, {};\n", b_row)); // kstep (element) * b_row
                out.push_str("    add.u32 %r13, %r13, %r14;\n");
                out.push_str(&format!("    mul.lo.u32 %r14, %r8, {};\n", b_row));
                out.push_str("    add.u32 %r13, %r13, %r14;\n"); // row bytes
                out.push_str("    mov.u32 %r14, %r6;\n");
                out.push_str("    shl.b32 %r14, %r14, 1;\n");      // g*2
                out.push_str(&format!("    add.u32 %r14, %r14, {};\n", ng * 16)); // + ng*8 cols * 2 bytes
                out.push_str("    add.u32 %r13, %r13, %r14;\n");  // + col bytes
                out.push_str("    mul.wide.u32 %rd4, %r13, 1;\n");
                out.push_str("    add.u64 %rd5, %rd3, %rd4;\n");
                out.push_str("    ld.global.u16 %u0, [%rd5];\n");
                out.push_str(&format!("    add.u64 %rd5, %rd5, {};\n", b_row));
                out.push_str("    ld.global.u16 %u1, [%rd5];\n");
                out.push_str("    shl.b32 %u1, %u1, 16;\n");
                out.push_str(&format!("    or.b32 %{}, %u0, %u1;\n", reg));
            }
            // mma into C[(mh,ng)] = c[4*(mh*2+ng)]
            let cb = 4 * (mh * 2 + ng);
            out.push_str(&format!(
                "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c{}, %c{}, %c{}, %c{}}}, {{%a0, %a1, %a2, %a3}}, {{%b0, %b1}}, {{%c{}, %c{}, %c{}, %c{}}};\n",
                cb, cb + 1, cb + 2, cb + 3, cb, cb + 1, cb + 2, cb + 3
            ));
        }
    }
    out.push_str("    add.u32 %r10, %r10, 16;\n");
    out.push_str("    bra.uni KLOOP;\nKEND:\n");

    // Store C. Tile-relative row = mh*16 + row_off + g ; col = ng*8 + 2t + col_off.
    // Byte = (mh*16+row_off)*y_row + g*y_row + (ng*8+2t+col_off)*y_elem
    for mh in 0..2 {
        for ng in 0..2 {
            let cb = 4 * (mh * 2 + ng);
            for (coff, row_off, col_off) in [
                (0i64, 0i64, 0i64),
                (1, 0, 1),
                (2, 8, 0),
                (3, 8, 1),
            ] {
                let cname = format!("c{}", cb + coff);
                out.push_str(&format!("    mov.u32 %r13, {};\n", (16 * mh + row_off) * y_row));
                out.push_str(&format!("    mul.lo.u32 %r14, %r6, {};\n", y_row));
                out.push_str("    add.u32 %r13, %r13, %r14;\n");
                out.push_str(&format!("    add.u32 %r13, %r13, {};\n", ng * 8 * (y_elem as i64)));
                out.push_str("    mov.u32 %r14, %r8;\n"); // 2t (elements)
                out.push_str(&format!("    mul.lo.u32 %r14, %r14, {};\n", y_elem));
                out.push_str("    add.u32 %r13, %r13, %r14;\n");
                out.push_str(&format!("    add.u32 %r13, %r13, {};\n", col_off * (y_elem as i64)));
                out.push_str("    mul.wide.u32 %rd4, %r13, 1;\n");
                out.push_str("    add.u64 %rd5, %rd6, %rd4;\n");
                if y_elem == 4 {
                    out.push_str(&format!("    st.global.f32 [%rd5], %{};\n", cname));
                } else {
                    out.push_str(&format!("    cvt.rn.f16.f32 %t0, %{};\n", cname));
                    out.push_str("    st.global.u16 [%rd5], %t0;\n");
                }
            }
        }
    }
    out.push_str("    ret;\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tensor_gemm_ptx_geometry_and_mma() {
        let ptx = tensor_gemm_ptx(64, 32, 32, 0, 4096, 16384, 4);
        assert!(ptx.contains(".entry main"), "entry");
        // 2 M-halves x 2 N-groups = 4 mma
        assert_eq!(ptx.matches("mma.sync.aligned.m16n8k16").count(), 4, "mma count");
        assert!(ptx.contains("div.u32 %r3, %r1, %r2;"), "m_cta decode");
        assert!(ptx.contains("KLOOP:"), "k loop");
        assert!(ptx.contains("setp.ge.u32 %p1, %r10, 32;"), "K bound");
        // 16 accs zeroed
        assert!(ptx.contains("mov.f32 %c15, 0f00000000;"), "c15 zero");
    }

    #[test]
    fn tensor_gemm_ptx_small_shape() {
        let ptx = tensor_gemm_ptx(32, 16, 16, 0, 1024, 4096, 4);
        assert!(ptx.contains("setp.ge.u32 %p1, %r10, 16;"), "K=16");
        assert!(ptx.contains("div.u32 %r3, %r1, %r2;"), "decode");
    }

    #[test]
    fn tensor_gemm_ptx_f16_y_emits_cvt() {
        let ptx = tensor_gemm_ptx(32, 16, 16, 0, 1024, 4096, 2);
        assert!(ptx.contains("cvt.rn.f16.f32 %t0, %c0;"), "f16 store cvt");
        assert!(ptx.contains("st.global.u16 [%rd5], %t0;"), "f16 store");
    }

    #[test]
    fn tensor_gemm_ptx_proj_bases_include_rd1() {
        // The a/b/y bases MUST add the projection pointer %rd1 — a device
        // address, never a bare literal (a forgotten base was the S3b
        // address-0 illegal-access bug).
        let ptx = tensor_gemm_ptx(64, 32, 32, 0, 4096, 16384, 4);
        assert!(ptx.contains("add.u64 %rd2, %rd1, %rd2;"), "a base += proj");
        assert!(ptx.contains("add.u64 %rd3, %rd1, %rd3;"), "b base += proj");
        assert!(ptx.contains("add.u64 %rd6, %rd1, %rd6;"), "y base += proj");
    }

    #[test]
    fn tensor_gemm_ptx_guards_extra_lanes() {
        // The driver launches 64-thread blocks; this warp-tile uses 32.
        let ptx = tensor_gemm_ptx(32, 16, 16, 0, 1024, 4096, 4);
        assert!(ptx.contains("setp.ge.u32 %p1, %r5, 32;"), "lane guard");
        assert!(ptx.contains("@%p1 ret;"), "early return");
    }

    #[test]
    fn tensor_gemm_ptx_cstore_uses_tile_accs() {
        // Each (mh,ng) tile stores ITS OWN 4 accumulators (c0..c15), not a
        // hardcoded c0..c3 for every tile (the S3b wrong-store bug).
        let ptx = tensor_gemm_ptx(32, 32, 16, 0, 2048, 8192, 4);
        for cb in [0, 4, 8, 12] {
            assert!(ptx.contains(&format!("st.global.f32 [%rd5], %c{};", cb)),
                "tile store c{}", cb);
        }
    }
}

