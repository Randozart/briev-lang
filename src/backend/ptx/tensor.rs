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
    let y_row = n * (y_elem as i64); // C row stride (bytes): N × y element size
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
    // rd6 = proj + y_off + m_cta*32*y_row + n_cta*16*y_elem
    out.push_str("    mov.u32 %r9, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r9, %r9, {};\n", 32 * y_row));
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str(&format!("    mov.u64 %rd6, {};\n", y_off));
    out.push_str("    add.u64 %rd6, %rd1, %rd6;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");
    out.push_str("    mov.u32 %r9, %r4;\n");
    out.push_str(&format!("    mul.lo.u32 %r9, %r9, {};\n", 16 * (y_elem as i64)));
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

/// Smem-staged tensor GEMM (S3b+). Same 32×16 warp-tile geometry as
/// `tensor_gemm_ptx` but A/B tiles stage through shared memory per kstep
/// and the fragments come from `ldmatrix` (x4 for A, x2.trans for B) —
/// the S3b+ fragment recipes, device-verified:
///   A: smem row-major, `ldmatrix.m8n8.x4` → exact A fragment
///   B: smem row-major, `ldmatrix.m8n8.x2.trans` → exact B fragment
/// `cp.async` multi-stage arrives in the next rung; this is the smem +
/// coalesced-fill foundation.
pub fn tensor_gemm_ptx_smem(
    m: i64,
    n: i64,
    k: i64,
    a_off: u64,
    b_off: u64,
    y_off: u64,
    y_elem: u32,
) -> String {
    debug_assert!(m % 32 == 0 && n % 16 == 0 && k % 16 == 0);
    let a_row = k * 2;
    let b_row = n * 2;
    let y_row = n * (y_elem as i64);
    let mut out = String::new();

    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n");
    out.push_str(".visible .entry main (.param .b64 proj_param)\n{\n");
    out.push_str("    .reg .b64  %rd1, %rd2, %rd3, %rd4, %rd5, %rd6, %rd5b, %rd5c;\n");
    out.push_str("    .reg .u32  %r1, %r2, %r3, %r4, %r5, %r6, %r7, %r8, %r9, %r10;\n");
    out.push_str("    .reg .u32  %r11, %r12, %r13, %r14, %r15, %r16, %r17, %r18, %r19;\n");
    out.push_str("    .reg .b32  %a0, %a1, %a2, %a3, %a4, %a5, %a6, %a7, %b0, %b1, %b2, %b3, %t0;\n");
    out.push_str("    .reg .f32  %c0, %c1, %c2, %c3, %c4, %c5, %c6, %c7, %c8, %c9;\n");
    out.push_str("    .reg .f32  %c10, %c11, %c12, %c13, %c14, %c15;\n");
    out.push_str("    .reg .pred %p1;\n");
    out.push_str("    .shared .align 16 .b8 asmem[1024];\n"); // 32x16 f16 A tile
    out.push_str("    .shared .align 16 .b8 bsmem[512];\n");  // 16x16 f16 B tile
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");

    // Block decode from ctaid.y.
    out.push_str("    mov.u32 %r1, %ctaid.y;\n");
    out.push_str(&format!("    mov.u32 %r2, {};\n", n / 16));
    out.push_str("    div.u32 %r3, %r1, %r2;  // m_cta\n");
    out.push_str("    rem.u32 %r4, %r1, %r2;  // n_cta\n");
    out.push_str("    mov.u32 %r5, %tid.x;\n");
    out.push_str("    setp.ge.u32 %p1, %r5, 32;\n");
    out.push_str("    @%p1 ret;\n");
    out.push_str("    shr.u32 %r6, %r5, 2;    // g\n");
    out.push_str("    and.b32 %r7, %r5, 3;    // t\n");
    out.push_str("    shl.b32 %r8, %r7, 1;    // 2t\n");

    // rd2 = a tile base (a_off + m_cta*32*a_row).
    out.push_str("    mov.u32 %r9, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r9, %r9, {};\n", 32 * a_row));
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str(&format!("    mov.u64 %rd2, {};\n", a_off));
    out.push_str("    add.u64 %rd2, %rd1, %rd2;\n");
    out.push_str("    add.u64 %rd2, %rd2, %rd4;\n");
    // rd3 = b tile base (b_off + n_cta*32).
    out.push_str("    mov.u32 %r9, %r4;\n");
    out.push_str("    mul.lo.u32 %r9, %r9, 32;\n");
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str(&format!("    mov.u64 %rd3, {};\n", b_off));
    out.push_str("    add.u64 %rd3, %rd1, %rd3;\n");
    out.push_str("    add.u64 %rd3, %rd3, %rd4;\n");
    // rd6 = y base (y_off + m_cta*32*y_row + n_cta*16*y_elem).
    out.push_str("    mov.u32 %r9, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r9, %r9, {};\n", 32 * y_row));
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str(&format!("    mov.u64 %rd6, {};\n", y_off));
    out.push_str("    add.u64 %rd6, %rd1, %rd6;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");
    out.push_str("    mov.u32 %r9, %r4;\n");
    out.push_str(&format!("    mul.lo.u32 %r9, %r9, {};\n", 16 * (y_elem as i64)));
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");

    for i in 0..16 {
        out.push_str(&format!("    mov.f32 %c{}, 0f00000000;\n", i));
    }

    // K loop.
    out.push_str("    mov.u32 %r10, 0;  // kstep\nKLOOP:\n");
    out.push_str(&format!("    setp.ge.u32 %p1, %r10, {};\n", k));
    out.push_str("    @%p1 bra KEND;\n");

    // A fill: thread t (0-31) copies A row t (16 f16 = 32 B) global→asmem.
    // Global: rd2 + t*a_row + kstep*2 (kstep is element column offset).
    out.push_str("    mov.u32 %r11, %r5;\n");
    out.push_str(&format!("    mul.lo.u32 %r12, %r11, {};\n", a_row));
    out.push_str("    mov.u32 %r13, %r10;\n");
    out.push_str("    mul.lo.u32 %r13, %r13, 2;\n");
    out.push_str("    add.u32 %r12, %r12, %r13;\n");
    out.push_str("    mul.wide.u32 %rd4, %r12, 1;\n");
    out.push_str("    add.u64 %rd5, %rd2, %rd4;\n");
    out.push_str("    mov.u64 %rd4, asmem;\n");
    out.push_str("    mul.wide.u32 %rd5b, %r11, 32;\n");
    out.push_str("    add.u64 %rd4, %rd4, %rd5b;\n");
    for _ in 0..8 {
        out.push_str("    ld.global.b32 %t0, [%rd5]; st.shared.b32 [%rd4], %t0;\n");
        out.push_str("    add.u64 %rd5, %rd5, 4; add.u64 %rd4, %rd4, 4;\n");
    }

    // B fill: blocked 2x2 layout so the x2.trans col-shifted second 8x8
    // (M1 = +16 bytes, side-by-side) yields the mma's rows-8-15 fragment.
    //   bsmem[r<8][c]      = B[(r%8)+0][c]      (ng=0 rows 0-7)
    //   bsmem[r<8][8+c]    = B[(r%8)+8][c]      (ng=0 rows 8-15)
    //   bsmem[8+r][c]      = B[r][8+c]          (ng=1 rows 0-7)
    //   bsmem[8+r][8+c]    = B[8+r][8+c]        (ng=1 rows 8-15)
    // Thread t (row=t%16, half=t/16) reads B[(row%8)+8*half][half*... ; col
    // offset +(row>=8 ? 8 : 0)] and writes bsmem[row*32 + half*16].
    out.push_str("    mov.u32 %r11, %r5;\n");
    out.push_str("    and.b32 %r12, %r11, 15;   // row = t%16\n");
    out.push_str("    shr.u32 %r13, %r11, 4;    // half = t/16\n");
    out.push_str("    mov.u32 %r14, %r10;\n");
    out.push_str(&format!("    mul.lo.u32 %r14, %r14, {};\n", b_row)); // kstep*b_row
    out.push_str("    and.b32 %r15, %r12, 7;    // row%8\n");
    out.push_str(&format!("    mul.lo.u32 %r15, %r15, {};\n", b_row));
    out.push_str("    add.u32 %r14, %r14, %r15;\n");
    out.push_str(&format!("    mul.lo.u32 %r15, %r13, {};\n", 8 * b_row)); // half*8*b_row
    out.push_str("    add.u32 %r14, %r14, %r15;\n");
    out.push_str("    shr.u32 %r15, %r12, 3;    // row>=8 ? 1 : 0\n");
    out.push_str("    mul.lo.u32 %r15, %r15, 16;\n"); // +8 cols * 2 bytes for ng=1 block
    out.push_str("    add.u32 %r14, %r14, %r15;\n");
    out.push_str("    mul.wide.u32 %rd4, %r14, 1;\n");
    out.push_str("    add.u64 %rd5, %rd3, %rd4;\n");
    out.push_str("    mov.u64 %rd4, bsmem;\n");
    out.push_str(&format!("    mul.lo.u32 %r15, %r12, {};\n", 32)); // row*32 bytes
    out.push_str("    mul.wide.u32 %rd5b, %r15, 1;\n");
    out.push_str("    add.u64 %rd4, %rd4, %rd5b;\n");
    out.push_str("    mul.wide.u32 %rd5c, %r13, 16;\n");
    out.push_str("    add.u64 %rd4, %rd4, %rd5c;\n");
    for _ in 0..4 {
        out.push_str("    ld.global.b32 %t0, [%rd5]; st.shared.b32 [%rd4], %t0;\n");
        out.push_str("    add.u64 %rd5, %rd5, 4; add.u64 %rd4, %rd4, 4;\n");
    }

    out.push_str("    bar.sync 0;\n");

    // A fragments: ldmatrix.x4 for half 0 (asmem) and half 1 (asmem+512).
    // addr(l) = asmem + ((m/2)*8 + l%8)*32 + (m%2)*16, m=l/8. (locked)
    out.push_str("    mov.u64 %rd4, asmem;\n");
    out.push_str("    mov.u32 %r11, %r5;\n");
    out.push_str("    shr.u32 %r12, %r11, 3;\n");
    out.push_str("    and.b32 %r13, %r11, 7;\n");
    out.push_str("    and.b32 %r14, %r12, 1;\n");
    out.push_str("    shr.u32 %r15, %r12, 1;\n");
    out.push_str("    mul.lo.u32 %r15, %r15, 8;\n");
    out.push_str("    add.u32 %r15, %r15, %r13;\n");
    out.push_str("    mul.lo.u32 %r15, %r15, 32;\n");
    out.push_str("    mul.lo.u32 %r14, %r14, 16;\n");
    out.push_str("    add.u32 %r16, %r15, %r14;\n");
    out.push_str("    mul.wide.u32 %rd5, %r16, 1;\n");
    out.push_str("    add.u64 %rd5, %rd4, %rd5;\n");
    out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a0, %a1, %a2, %a3}, [%rd5];\n");
    out.push_str("    add.u64 %rd5, %rd5, 512;\n");
    out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a4, %a5, %a6, %a7}, [%rd5];\n");
    // ldmatrix.x4 yields the four 8x8 tiles as {M0=rows0-7/cols0-7, M1=rows0-7/
    // cols8-15, M2=rows8-15/cols0-7, M3=rows8-15/cols8-15}. The mma.m16n8k16
    // A operand instead interleaves rows with k-halves: a1 must be the OTHER
    // row-block's k0-7 (M2), a2 this row-block's k8-15 (M1). Device-verified:
    // without the swap the seeded 64x64x64 A-side is ~5% off while all-ones
    // (degenerate) hides it. (B ldmatrix.x2.trans has no such swap.)
    out.push_str("    mov.b32 %t0, %a1; mov.b32 %a1, %a2; mov.b32 %a2, %t0;\n");
    out.push_str("    mov.b32 %t0, %a5; mov.b32 %a5, %a6; mov.b32 %a6, %t0;\n");

    // B fragments: ldmatrix.x2.trans for ng=0 (bsmem block A) and ng=1
    // (bsmem+256, block B). addr(l) = bsmem + ng*256 + (m*8 + (l%16)%8)*32
    // + ((l%16)/8)*16, m=l/16. (locked 2026-09-09: x2.trans M1 is the
    // col-shifted +16 8x8, so the bsmem fill stores the B tile as 2x2 8x8
    // blocks — see the B-fill comment above.)
    for ng in 0..2 {
        out.push_str("    mov.u64 %rd4, bsmem;\n");
        out.push_str(&format!("    add.u64 %rd4, %rd4, {};\n", ng * 256));
        out.push_str("    mov.u32 %r11, %r5;\n");
        out.push_str("    shr.u32 %r12, %r11, 4;\n");
        out.push_str("    and.b32 %r13, %r11, 15;\n");
        out.push_str("    and.b32 %r14, %r13, 7;\n");
        out.push_str("    shr.u32 %r16, %r13, 3;\n");
        out.push_str("    mul.lo.u32 %r15, %r12, 8;\n");
        out.push_str("    add.u32 %r15, %r15, %r14;\n");
        out.push_str("    mul.lo.u32 %r15, %r15, 32;\n");
        out.push_str("    mul.lo.u32 %r16, %r16, 16;\n");
        out.push_str("    add.u32 %r17, %r15, %r16;\n");
        out.push_str("    mul.wide.u32 %rd5, %r17, 1;\n");
        out.push_str("    add.u64 %rd5, %rd4, %rd5;\n");
        if ng == 0 {
            out.push_str("    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {%b0, %b1}, [%rd5];\n");
        } else {
            out.push_str("    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {%b2, %b3}, [%rd5];\n");
        }
    }

    // 4 mma: (mh, ng).
    for mh in 0..2 {
        for ng in 0..2 {
            let cb = 4 * (mh * 2 + ng);
            let (a0, a1, a2, a3) = if mh == 0 { ("a0", "a1", "a2", "a3") } else { ("a4", "a5", "a6", "a7") };
            let (b0, b1) = if ng == 0 { ("b0", "b1") } else { ("b2", "b3") };
            out.push_str(&format!(
                "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c{}, %c{}, %c{}, %c{}}}, {{%{}, %{}, %{}, %{}}}, {{%{}, %{}}}, {{%c{}, %c{}, %c{}, %c{}}};\n",
                cb, cb + 1, cb + 2, cb + 3, a0, a1, a2, a3, b0, b1, cb, cb + 1, cb + 2, cb + 3
            ));
        }
    }

    out.push_str("    bar.sync 0;\n");
    out.push_str("    add.u32 %r10, %r10, 16;\n");
    out.push_str("    bra.uni KLOOP;\nKEND:\n");

    // Store C (same as tensor_gemm_ptx).
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
                out.push_str("    mov.u32 %r14, %r8;\n");
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

/// Register-blocked tensor GEMM PTX (S3b+ perf rungs): warp tile = **32×64**
/// C (2 M-halves × 8 N-groups = 16 `mma.m16n8k16` per k-step, 64 f32
/// accumulator registers). The 8 ng groups reuse each A fragment 8× and each
/// B fragment 2×, raising arithmetic intensity from the 32×16 warp tile's
/// ~10 FLOP/byte to ~21 — the first rung toward the S5 gate.
///
/// B smem holds the 16×64 tile as eight 2×2-blocked 8×16 fragments (the
/// blocked fill from `tensor_gemm_ptx_smem`); each `ldmatrix.x2.trans`
/// reads one 8×16 block (M0 = rows 0-7 / cols 0-7, M1 col-shifted = rows
/// 8-15). A/B fills, C store, and the fragment recipes otherwise mirror the
/// 32×16 kernel. Grid = (M/32) × (N/64) blocks decoded from ctaid.y.
pub fn tensor_gemm_ptx_smem_r16(
    m: i64,
    n: i64,
    k: i64,
    a_off: u64,
    b_off: u64,
    y_off: u64,
    y_elem: u32,
) -> String {
    debug_assert!(m % 32 == 0 && n % 64 == 0 && k % 16 == 0);
    let a_row = k * 2;
    let b_row = n * 2;
    let y_row = n * (y_elem as i64);
    let ng = 8;
    let mut out = String::new();

    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n");
    out.push_str(".visible .entry main (.param .b64 proj_param)\n{\n");
    out.push_str("    .reg .b64  %rd1, %rd2, %rd3, %rd4, %rd5, %rd6, %rd5b, %rd5c;\n");
    out.push_str("    .reg .u32  %r1, %r2, %r3, %r4, %r5, %r6, %r7, %r8, %r9, %r10;\n");
    out.push_str("    .reg .u32  %r11, %r12, %r13, %r14, %r15, %r16, %r17, %r18, %r19, %r20;\n");
    let bregs: Vec<String> = (0..2 * ng).map(|i| format!("%b{}", i)).collect();
    let mut bdecl = String::from("    .reg .b32  %a0, %a1, %a2, %a3, %a4, %a5, %a6, %a7");
    for b in &bregs {
        bdecl.push_str(&format!(", {}", b));
    }
    bdecl.push_str(", %t0;\n");
    out.push_str(&bdecl);
    let cregs: Vec<String> = (0..4 * 2 * ng).map(|i| format!("%c{}", i)).collect();
    let cdecl = format!("    .reg .f32  {};\n", cregs.join(", "));
    out.push_str(&cdecl);
    out.push_str("    .reg .pred %p1;\n");
    out.push_str("    .shared .align 16 .b8 asmem[1024];\n");
    out.push_str("    .shared .align 16 .b8 bsmem[2048];\n");
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");

    // Block decode from ctaid.y: n/64 blocks per M-row.
    out.push_str("    mov.u32 %r1, %ctaid.y;\n");
    out.push_str(&format!("    mov.u32 %r2, {};\n", n / 64));
    out.push_str("    div.u32 %r3, %r1, %r2;  // m_cta\n");
    out.push_str("    rem.u32 %r4, %r1, %r2;  // n_cta\n");
    out.push_str("    mov.u32 %r5, %tid.x;\n");
    out.push_str("    setp.ge.u32 %p1, %r5, 32;\n");
    out.push_str("    @%p1 ret;\n");
    out.push_str("    shr.u32 %r6, %r5, 2;    // g\n");
    out.push_str("    and.b32 %r7, %r5, 3;    // t\n");
    out.push_str("    shl.b32 %r8, %r7, 1;    // 2t\n");

    // rd2 = a_off + m_cta*32*a_row
    out.push_str("    mov.u32 %r9, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r9, %r9, {};\n", 32 * a_row));
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str(&format!("    mov.u64 %rd2, {};\n", a_off));
    out.push_str("    add.u64 %rd2, %rd1, %rd2;\n");
    out.push_str("    add.u64 %rd2, %rd2, %rd4;\n");
    // rd3 = b_off + n_cta*64*2
    out.push_str("    mov.u32 %r9, %r4;\n");
    out.push_str("    mul.lo.u32 %r9, %r9, 128;\n");
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str(&format!("    mov.u64 %rd3, {};\n", b_off));
    out.push_str("    add.u64 %rd3, %rd1, %rd3;\n");
    out.push_str("    add.u64 %rd3, %rd3, %rd4;\n");
    // rd6 = y_off + m_cta*32*y_row + n_cta*64*y_elem
    out.push_str("    mov.u32 %r9, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r9, %r9, {};\n", 32 * y_row));
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str(&format!("    mov.u64 %rd6, {};\n", y_off));
    out.push_str("    add.u64 %rd6, %rd1, %rd6;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");
    out.push_str("    mov.u32 %r9, %r4;\n");
    out.push_str(&format!("    mul.lo.u32 %r9, %r9, {};\n", 64 * (y_elem as i64)));
    out.push_str("    mul.wide.u32 %rd4, %r9, 1;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");

    // Zero the 64 C accumulators.
    for i in 0..64 {
        out.push_str(&format!("    mov.f32 %c{}, 0f00000000;\n", i));
    }

    // K loop.
    out.push_str("    mov.u32 %r10, 0;  // kstep\nKLOOP:\n");
    out.push_str(&format!("    setp.ge.u32 %p1, %r10, {};\n", k));
    out.push_str("    @%p1 bra KEND;\n");

    // A fill: thread t copies A row t (32 B) global→asmem (unchanged from
    // the 32×16 kernel — the A tile is still 32×16).
    out.push_str("    mov.u32 %r11, %r5;\n");
    out.push_str(&format!("    mul.lo.u32 %r12, %r11, {};\n", a_row));
    out.push_str("    mov.u32 %r13, %r10;\n");
    out.push_str("    mul.lo.u32 %r13, %r13, 2;\n");
    out.push_str("    add.u32 %r12, %r12, %r13;\n");
    out.push_str("    mul.wide.u32 %rd4, %r12, 1;\n");
    out.push_str("    add.u64 %rd5, %rd2, %rd4;\n");
    out.push_str("    mov.u64 %rd4, asmem;\n");
    out.push_str("    mul.wide.u32 %rd5b, %r11, 32;\n");
    out.push_str("    add.u64 %rd4, %rd4, %rd5b;\n");
    for _ in 0..8 {
        out.push_str("    ld.global.b32 %t0, [%rd5]; st.shared.b32 [%rd4], %t0;\n");
        out.push_str("    add.u64 %rd5, %rd5, 4; add.u64 %rd4, %rd4, 4;\n");
    }

    // B fill: thread t copies 64 B (4 × 16-B chunks) of the blocked 16×64
    // tile. Chunk k: dest bsmem + t*64 + k*16; block b = t/4; row within
    // block r = (t%4)*2 + (k>=2 ? 1 : 0); half = k%2. B source:
    // row = r + 8*half, cols = b*8 .. b*8+7.
    out.push_str("    mov.u32 %r11, %r5;\n");
    out.push_str("    shr.u32 %r12, %r11, 2;    // t/4 = block b\n");
    out.push_str("    and.b32 %r13, %r11, 3;    // t%4\n");
    for k in 0..4 {
        // dest byte D = t*64 + k*16
        out.push_str("    mov.u32 %r14, %r5;\n");
        out.push_str("    mul.lo.u32 %r14, %r14, 64;\n");
        out.push_str(&format!("    add.u32 %r14, %r14, {};\n", k * 16));
        // row r = (t%4)*2 + (k>=2 ? 1 : 0)
        out.push_str("    mov.u32 %r15, %r13;\n");
        out.push_str("    mul.lo.u32 %r15, %r15, 2;\n");
        if k >= 2 {
            out.push_str("    add.u32 %r15, %r15, 1;\n");
        }
        // B row = r + 8*half
        out.push_str("    mov.u32 %r16, %r15;\n");
        if k % 2 == 1 {
            out.push_str("    add.u32 %r16, %r16, 8;\n");
        }
        // global byte = rd3 + kstep*b_row + B_row*b_row + b*16
        out.push_str("    mov.u32 %r17, %r10;\n");
        out.push_str(&format!("    mul.lo.u32 %r17, %r17, {};\n", b_row));
        out.push_str(&format!("    mul.lo.u32 %r18, %r16, {};\n", b_row));
        out.push_str("    add.u32 %r17, %r17, %r18;\n");
        out.push_str(&format!("    mul.lo.u32 %r18, %r12, {};\n", 16));
        out.push_str("    add.u32 %r17, %r17, %r18;\n");
        out.push_str("    mul.wide.u32 %rd4, %r17, 1;\n");
        out.push_str("    add.u64 %rd5, %rd3, %rd4;\n");
        // dest = bsmem + D
        out.push_str("    mov.u64 %rd4, bsmem;\n");
        out.push_str("    mul.wide.u32 %rd5b, %r14, 1;\n");
        out.push_str("    add.u64 %rd4, %rd4, %rd5b;\n");
        for _ in 0..4 {
            out.push_str("    ld.global.b32 %t0, [%rd5]; st.shared.b32 [%rd4], %t0;\n");
            out.push_str("    add.u64 %rd5, %rd5, 4; add.u64 %rd4, %rd4, 4;\n");
        }
    }

    out.push_str("    bar.sync 0;\n");

    // A fragments: 2 × ldmatrix.x4 (same recipe as the 32×16 kernel).
    out.push_str("    mov.u64 %rd4, asmem;\n");
    out.push_str("    mov.u32 %r11, %r5;\n");
    out.push_str("    shr.u32 %r12, %r11, 3;\n");
    out.push_str("    and.b32 %r13, %r11, 7;\n");
    out.push_str("    and.b32 %r14, %r12, 1;\n");
    out.push_str("    shr.u32 %r15, %r12, 1;\n");
    out.push_str("    mul.lo.u32 %r15, %r15, 8;\n");
    out.push_str("    add.u32 %r15, %r15, %r13;\n");
    out.push_str("    mul.lo.u32 %r15, %r15, 32;\n");
    out.push_str("    mul.lo.u32 %r14, %r14, 16;\n");
    out.push_str("    add.u32 %r16, %r15, %r14;\n");
    out.push_str("    mul.wide.u32 %rd5, %r16, 1;\n");
    out.push_str("    add.u64 %rd5, %rd4, %rd5;\n");
    out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a0, %a1, %a2, %a3}, [%rd5];\n");
    out.push_str("    add.u64 %rd5, %rd5, 512;\n");
    out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a4, %a5, %a6, %a7}, [%rd5];\n");
    out.push_str("    mov.b32 %t0, %a1; mov.b32 %a1, %a2; mov.b32 %a2, %t0;\n");
    out.push_str("    mov.b32 %t0, %a5; mov.b32 %a5, %a6; mov.b32 %a6, %t0;\n");

    // B fragments: 8 × ldmatrix.x2.trans, block g at bsmem + g*256.
    for g in 0..ng {
        let (b0, b1) = (&bregs[2 * g], &bregs[2 * g + 1]);
        out.push_str("    mov.u64 %rd4, bsmem;\n");
        out.push_str(&format!("    add.u64 %rd4, %rd4, {};\n", g * 256));
        out.push_str("    mov.u32 %r11, %r5;\n");
        out.push_str("    shr.u32 %r12, %r11, 4;\n");
        out.push_str("    and.b32 %r13, %r11, 15;\n");
        out.push_str("    and.b32 %r14, %r13, 7;\n");
        out.push_str("    shr.u32 %r16, %r13, 3;\n");
        out.push_str("    mul.lo.u32 %r15, %r12, 8;\n");
        out.push_str("    add.u32 %r15, %r15, %r14;\n");
        out.push_str("    mul.lo.u32 %r15, %r15, 32;\n");
        out.push_str("    mul.lo.u32 %r16, %r16, 16;\n");
        out.push_str("    add.u32 %r17, %r15, %r16;\n");
        out.push_str("    mul.wide.u32 %rd5, %r17, 1;\n");
        out.push_str("    add.u64 %rd5, %rd4, %rd5;\n");
        out.push_str(&format!(
            "    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {{{}, {}}}, [%rd5];\n",
            b0, b1
        ));
    }

    // 16 mma: (mh, ng), C offset cb = 4*(mh*8+ng).
    for mh in 0..2 {
        for g in 0..ng {
            let cb = 4 * (mh * 8 + g);
            let (a0, a1, a2, a3) = if mh == 0 { ("%a0", "%a1", "%a2", "%a3") } else { ("%a4", "%a5", "%a6", "%a7") };
            let (b0, b1) = (&bregs[2 * g], &bregs[2 * g + 1]);
            out.push_str(&format!(
                "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c{}, %c{}, %c{}, %c{}}}, {{{}, {}, {}, {}}}, {{{}, {}}}, {{%c{}, %c{}, %c{}, %c{}}};\n",
                cb, cb + 1, cb + 2, cb + 3, a0, a1, a2, a3, b0, b1, cb, cb + 1, cb + 2, cb + 3
            ));
        }
    }

    out.push_str("    bar.sync 0;\n");
    out.push_str("    add.u32 %r10, %r10, 16;\n");
    out.push_str("    bra.uni KLOOP;\nKEND:\n");

    // Store C: 2 mh × 8 ng, 4 regs each. Tile-relative row = 16*mh+row_off+g,
    // col = ng*8 + 2t + col_off.
    for mh in 0..2 {
        for g in 0..ng {
            let cb = 4 * (mh * 8 + g);
            for (coff, row_off, col_off) in [
                (0i64, 0i64, 0i64),
                (1, 0, 1),
                (2, 8, 0),
                (3, 8, 1),
            ] {
                let cname = format!("c{}", cb + coff as usize);
                out.push_str(&format!("    mov.u32 %r13, {};\n", (16 * mh as i64 + row_off) * y_row));
                out.push_str(&format!("    mul.lo.u32 %r14, %r6, {};\n", y_row));
                out.push_str("    add.u32 %r13, %r13, %r14;\n");
                out.push_str(&format!("    add.u32 %r13, %r13, {};\n", (g as i64) * 8 * (y_elem as i64)));
                out.push_str("    mov.u32 %r14, %r8;\n");
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

    #[test]
    fn tensor_gemm_ptx_f16_y_stride_is_n_elem_not_n4() {
        // f16 y: row stride must be N*2 (not N*4 — the f32 stride). The
        // S3b f16-store OOB bug used N*4 for a 2-byte y.
        let ptx = tensor_gemm_ptx(32, 16, 16, 0, 1024, 4096, 2);
        assert!(ptx.contains("mul.lo.u32 %r14, %r6, 32;"), "y_row=N*2=32: {ptx}");
        assert!(ptx.contains("cvt.rn.f16.f32 %t0, %c0;"), "f16 cvt");
        assert!(ptx.contains("st.global.u16 [%rd5], %t0;"), "f16 store");
        // n_cta*16*y_elem = n_cta*32 for f16 (NOT *64)
        assert!(ptx.contains("mul.lo.u32 %r9, %r9, 32;"), "n_cta col = 16*2: {ptx}");
    }
}

/// Multi-warp register-blocked tensor GEMM PTX (S3b+ perf rungs). CTA tile
/// = (mw*32) rows × (nw*64) cols across mw*nw warps; each warp runs the
/// 32×64 warp-tile from `tensor_gemm_ptx_smem_r16` (2 M-halves × 8 N-groups
/// = 16 mma, 64 f32 accumulators). Arithmetic intensity = mw*nw*32*64 /
/// (mw*32 + nw*64) FLOP/byte — 85 for the 256×128 CTA (mw=8, nw=2) vs 21
/// for the single-warp r16.
///
/// A/B smem tiles are shared across all warps (A = mw*32 × 16 f16, B = the
/// 16 × nw*64 f16 tile in 64-col blocked slices) and filled cooperatively.
/// Each warp loads its 32-row A slice and 64-col B slice via ldmatrix, then
/// runs 16 mma. The block (tile) is decoded from ctaid.x; the block size is
/// mw*nw*32 (the RunnerKernel.block_threads dispatch contract).
/// cp.async pipeline depth for the mw kernel (2026-09-10): one outstanding
/// fill cannot hide the ~600ns DRAM latency behind ~90ns of mma work, so
/// stages-1 fills stay in flight. Stage arrays live in ONE dynamic shared

pub fn tensor_gemm_ptx_smem_mw(
    m: i64,
    n: i64,
    k: i64,
    a_off: u64,
    b_off: u64,
    y_off: u64,
    y_elem: u32,
    mw: usize,
    nw: usize,
    f16_acc: bool,
    stages: usize,
    warp_mh: usize,
) -> String {
    // Warp tiling (2026-09-11 double-pump plan): the warp covers
    // warp_mh 16-row blocks x (16/warp_mh) 8-col groups — mma count per
    // kstep is invariant (16), so accumulators stay at 32 b32 (f16acc)
    // for every warp shape. warp_mh=2 = the historical 32x64 warp;
    // warp_mh=4 = the 64x32 A-sharing variant (B loaded 4x per kstep
    // instead of 16x — 21.3 vs 12.8 FLOP per shared-read byte).
    let mhr = warp_mh;
    let gr = 16 / warp_mh;
    debug_assert!(mhr * gr == 16 && m % (16 * mhr as i64 * mw as i64) == 0 && n % (8 * gr as i64 * nw as i64) == 0 && k % 16 == 0);
    let a_row = k * 2;
    let b_row = n * 2;
    let y_row = n * (y_elem as i64);
    let threads = (mw * nw * 32) as i64;
    // per-thread 4-byte copy counts: per_X * threads * 4 must equal the
    // stage's X bytes exactly (the fill covers its stage, no more — an
    // overcount smears into the next stage, an undercount leaves smem
    // unfilled).
    let per_a = (mw as i64 * mhr as i64 * 128) / threads;
    let per_b = (nw as i64 * gr as i64 * 64) / threads;
    let asmem_buf = mw * mhr * 512;
    // B slab per warp = 16 k-rows x (8*gr) n-cols, k-major: (16*gr)-byte
    // k-rows. ldmatrix lane max = 15*(16*gr) + (gr-1)*16 + 16 = 256*gr =
    // slab size exactly.
    let bsmem_buf = nw * gr * 256;
    let mut out = String::new();

    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n");
    // Register cap (2026-09-10): natural allocation is 138 regs → 1 CTA/SM
    // (8 warps). The 128-reg cap restores 2 CTAs — same-window A/B
    // 2048^3: 15.7 vs 12.3 TFLOP/s, 0 spills, correctness unchanged
    // (re-tested on the fixed kernel; the historical "capped cubins fault
    // IMA" verdict was contaminated by that era's OOB bugs). Applied by
    // compile_cubin's -maxrregcount flag (the PTX .maxnreg directive is
    // not supported by any local ptxas).
    out.push_str("    .extern .shared .align 16 .b8 dsmem[];\n");
    out.push_str(".visible .entry main (.param .b64 proj_param)\n{\n");
    out.push_str("    .reg .b64  %rd1, %rd2, %rd3, %rd4, %rd5, %rd6, %rd7, %rd8, %rd9, %rd5b, %rd5c;\n");
    out.push_str("    .reg .u32  %r1, %r2, %r3, %r4, %r5, %r6, %r7, %r8, %r9, %r10;\n");
    out.push_str("    .reg .u32  %r11, %r12, %r13, %r14, %r15, %r16, %r17, %r18, %r19, %r20;\n");
    out.push_str("    .reg .u32  %r21, %r22;\n");
    // Register-trimmed scheduling: only %a0-%a3 (one A 16x16 block live per
    // mh) and %b0-%b1 (one B fragment live per g) exist — see the compute
    // section's scheduling comment below.
    // f16-acc pipelines the B loads (ld-ahead into the alternate pair) and
    // gets 4 B regs; f32 stays at 2 (the 128-reg 2-CTA budget is exact).
    let bregs: Vec<String> = if f16_acc {
        vec!["%b0".to_string(), "%b1".to_string(), "%b2".to_string(), "%b3".to_string()]
    } else {
        vec!["%b0".to_string(), "%b1".to_string()]
    };
    out.push_str(&format!(
        "    .reg .b32  %a0, %a1, %a2, %a3, {}, %t0;\n",
        bregs.join(", ")
    ));
    // Accumulators: f32-acc = 4 f32 per (mh,g) (64 regs); f16-acc = 2 f16x2
    // b32 per (mh,g) (32 regs — funds an extra CTA's worth of registers).
    // f16 contract: chunked accumulation — the mma chains f16x2 over
    // 8 ksteps, then each pair is promoted into the CTA-private f16 y tile
    // (read-modify-write) and reset. Tier gate 1e-2 (vs 5e-3 f32-acc).
    let cregs: Vec<String> = (0..if f16_acc { 2 * mhr * gr } else { 4 * mhr * gr })
        .map(|i| format!("%c{}", i))
        .collect();
    if f16_acc {
        out.push_str(&format!("    .reg .b32  {};\n", cregs.join(", ")));
        for c in &cregs {
            out.push_str(&format!("    mov.b32 {}, 0;\n", c));
        }
    } else {
        out.push_str(&format!("    .reg .f32  {};\n", cregs.join(", ")));
        for c in &cregs {
            out.push_str(&format!("    mov.f32 {}, 0f00000000;\n", c));
        }
    }

    // f16-acc contract: the kernel OWNS its y tile — zero it here, then the
    // KLOOP promotes each f16x2 chunk into it by read-modify-write (CTA-
    // private tiles, no atomics). Each thread zeroes/promotes exactly the
    // fragments it accumulates, so no cross-thread hazard is introduced.
    // y RMV pass emitter: accumulate=true folds the f16x2 chunk accs into
    // y (add.rn.f16x2, then resets the chunk accs); accumulate=false zeroes
    // the tile. Addressing mirrors the f32 store tail exactly.
    let mut emit_y_pass = |out: &mut String, accumulate: bool| {
        out.push_str("    mov.u32 %r12, %r10;\n");
        out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", 16 * mhr as i64 * y_row));
        out.push_str("    mov.u32 %r13, %r11;\n");
        out.push_str(&format!("    mul.lo.u32 %r13, %r13, {};\n", 8 * gr as i64 * (y_elem as i64)));
        out.push_str("    mov.u32 %r9, %r12;\n");
        out.push_str("    add.u32 %r9, %r9, %r13;\n");
        if !accumulate {
            out.push_str("    mov.b32 %t0, 0;\n");
        }
        for mh in 0..mhr {
            for g in 0..gr {
                let cb = 2 * (mh * gr + g);
                out.push_str(&format!("    mov.u32 %r12, {};\n", (16 * mh as i64) * y_row));
                out.push_str(&format!("    mul.lo.u32 %r13, %r6, {};\n", y_row));
                out.push_str("    add.u32 %r12, %r12, %r13;\n");
                out.push_str(&format!(
                    "    add.u32 %r12, %r12, {};\n",
                    (g as i64) * 8 * (y_elem as i64)
                ));
                out.push_str("    add.u32 %r12, %r12, %r9;\n");
                out.push_str("    mov.u32 %r13, %r8;\n");
                out.push_str(&format!("    mul.lo.u32 %r13, %r13, {};\n", y_elem));
                out.push_str("    add.u32 %r12, %r12, %r13;\n");
                out.push_str("    mul.wide.u32 %rd4, %r12, 1;\n");
                out.push_str("    add.u64 %rd5, %rd6, %rd4;\n");
                // 2 pair-regs per (mh,g): reg0 = (row0: cols 2t,2t+1),
                // reg1 = (row8: same cols) — the f16x2 packing merges the
                // column pair that f32 kept in two separate regs.
                for (pair, addr_step) in [
                    (0i64, None::<i64>),
                    (1, Some(8 * y_row)), // row +8
                ] {
                    if let Some(d) = addr_step {
                        out.push_str(&format!("    add.u64 %rd5, %rd5, {};\n", d));
                    }
                    let reg = &cregs[cb + pair as usize];
                    if accumulate {
                        out.push_str("    ld.global.b32 %t0, [%rd5];\n");
                        out.push_str(&format!("    add.rn.f16x2 %t0, %t0, {};\n", reg));
                        out.push_str("    st.global.b32 [%rd5], %t0;\n");
                        out.push_str(&format!("    mov.b32 {}, 0;\n", reg));
                    } else {
                        out.push_str("    st.global.b32 [%rd5], %t0;\n");
                    }
                }
            }
        }
    };
    out.push_str("    .reg .pred %p1;\n");
    // 4-stage cp.async pipeline (2026-09-10): the fill's ~600ns DRAM latency
    // cannot hide behind a single ~90ns compute phase, so STAGES-1 fills stay
    // in flight. Stages live in ONE dynamic shared array — 4 stages at
    // (2,4) need 80KB and (4,4) 80KB+, both past the 48KB static cap, so the
    // array is `.extern` and the runtime opts in via cuFuncSetAttribute +
    // launch sharedMemBytes (briev_dev_cuda already had that path).
    // %r21/%r22: u32 stage-0 bases (asmem/bsmem) for the cp.async dsts;
    // %rd8/%rd9: the same as u64 for the ldmatrix srcs.
    out.push_str("    mov.u32 %r21, dsmem;\n");
    out.push_str(&format!("    add.u32 %r22, %r21, {};\n", asmem_buf * stages));
    out.push_str("    cvt.u64.u32 %rd8, %r21;\n");
    out.push_str("    cvt.u64.u32 %rd9, %r22;\n");
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");

    out.push_str("    mov.u32 %r1, %ctaid.x;\n");
    out.push_str(&format!("    mov.u32 %r2, {};\n", n / (8 * gr as i64 * nw as i64)));
    out.push_str("    div.u32 %r3, %r1, %r2;  // m_cta\n");
    out.push_str("    rem.u32 %r4, %r1, %r2;  // n_cta\n");
    out.push_str("    mov.u32 %r5, %tid.x;\n");
    out.push_str(&format!("    setp.ge.u32 %p1, %r5, {};\n", threads));
    out.push_str("    @%p1 ret;\n");
    out.push_str("    shr.u32 %r9, %r5, 5;    // warp = tid/32\n");
    out.push_str(&format!("    mov.u32 %r2, {};\n", nw));
    out.push_str("    div.u32 %r10, %r9, %r2;  // mh_warp = warp/nw\n");
    out.push_str("    rem.u32 %r11, %r9, %r2;  // ng_warp = warp%nw\n");
    out.push_str("    and.b32 %r7, %r5, 31;\n");
    out.push_str("    shr.u32 %r6, %r7, 2;    // g\n");
    out.push_str("    and.b32 %r8, %r7, 3;    // t\n");
    out.push_str("    shl.b32 %r8, %r8, 1;    // 2t\n");

    out.push_str("    mov.u32 %r2, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r2, %r2, {};\n", 16 * mhr as i64 * mw as i64 * a_row));
    out.push_str("    mul.wide.u32 %rd4, %r2, 1;\n");
    out.push_str(&format!("    mov.u64 %rd2, {};\n", a_off));
    out.push_str("    add.u64 %rd2, %rd1, %rd2;\n");
    out.push_str("    add.u64 %rd2, %rd2, %rd4;\n");
    out.push_str("    mov.u32 %r2, %r4;\n");
    out.push_str(&format!("    mul.lo.u32 %r2, %r2, {};\n", 16 * gr as i64 * nw as i64));
    out.push_str("    mul.wide.u32 %rd4, %r2, 1;\n");
    out.push_str(&format!("    mov.u64 %rd3, {};\n", b_off));
    out.push_str("    add.u64 %rd3, %rd1, %rd3;\n");
    out.push_str("    add.u64 %rd3, %rd3, %rd4;\n");
    out.push_str("    mov.u32 %r2, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r2, %r2, {};\n", 16 * mhr as i64 * mw as i64 * y_row));
    out.push_str("    mul.wide.u32 %rd4, %r2, 1;\n");
    out.push_str(&format!("    mov.u64 %rd6, {};\n", y_off));
    out.push_str("    add.u64 %rd6, %rd1, %rd6;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");
    out.push_str("    mov.u32 %r2, %r4;\n");
    out.push_str(&format!("    mul.lo.u32 %r2, %r2, {};\n", 8 * gr as i64 * nw as i64 * (y_elem as i64)));
    out.push_str("    mul.wide.u32 %rd4, %r2, 1;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");

    // === Helper closure: emit fill code for one tile ===
    // Instead of a closure (which can't be called multiple times in a loop
    // easily), we inline the fill logic as a Rust macro-style codegen helper.
    // We emit A-fill then B-fill, targeting smem at smem_base (a u32 reg holding
    // the shared base address for this buffer).

    // --- PROLOGUE: fill stages 0..stages-2, one commit group each ---
    // (the f16-acc y-tile zero pass runs AFTER the setup below — it needs
    // r6/r8/r10/r11; emitting it here read uninitialized registers and
    // scattered zeros into A/B: the 4096x4096x16 2.3e-1 failure.)

    for s in 0..stages - 1 {
        let stripe = s as i64 * 16;
        let a_off_s = s * asmem_buf;
        let b_off_s = s * bsmem_buf;
        if stripe >= k {
            // The stage's stripe does not exist (K < 16*(s+1)); the commit
            // still happens — wait_group counts groups, empty is legal.
            out.push_str("    cp.async.commit_group;\n");
            continue;
        }
        // A fill into stage s
        out.push_str("    mov.u32 %r12, %r21;\n");
        if a_off_s > 0 {
            out.push_str(&format!("    add.u32 %r12, %r12, {};\n", a_off_s));
        }
        // Coalesced fill mapping (2026-09-10): D = tid*4 + j*threads*4 —
        // consecutive lanes touch consecutive 4B, so each warp's 10-24
        // copies cover full 128B lines. The per-thread stride here forced
        // 4B reads out of separate 32B sectors.
        out.push_str("    mul.lo.u32 %r13, %r5, 4;\n");
        for j in 0..per_a {
            out.push_str("    mov.u32 %r14, %r13;\n");
            if j > 0 {
                out.push_str(&format!("    add.u32 %r14, %r14, {};\n", j * threads * 4));
            }
            out.push_str("    mov.u32 %r15, %r14;\n");
            out.push_str("    and.b32 %r16, %r14, 31;\n");
            out.push_str("    shr.u32 %r15, %r15, 5;\n");
            out.push_str(&format!("    mul.lo.u32 %r15, %r15, {};\n", a_row));
            out.push_str("    add.u32 %r15, %r15, %r16;\n");
            if stripe > 0 {
                out.push_str(&format!("    add.u32 %r15, %r15, {};\n", stripe * 2));
            }
            out.push_str("    mul.wide.u32 %rd5, %r15, 1;\n");
            out.push_str("    add.u64 %rd5, %rd2, %rd5;\n");
            out.push_str("    mov.u32 %r16, %r12;\n");
            out.push_str("    add.u32 %r16, %r16, %r14;\n");
            out.push_str("    cp.async.ca.shared.global [%r16], [%rd5], 4;\n");
        }
        // B fill into stage s. Layout contract (must mirror the B ldmatrix
        // reader below): the slab is K-MAJOR — smem byte D = slab*2048 +
        // k*128 + n*2 (pre-swizzle), 128-byte k-rows of 64 n-cols. A 4-byte
        // cp.async moves global B[stripe+k][n..n+1] (adjacent columns,
        // 4-aligned since D%4==0 keeps n even) to the same-shaped smem pair.
        // 16B chunks of each k-row are XOR-swizzled with k&7 so ldmatrix's
        // 16 rows touch 8 bank groups, not 1. The pre-rewrite n-major fill
        // could not be sourced by 4-byte cp.async at all — that inversion
        // was the 2026-09-10 fault.
        out.push_str("    mov.u32 %r12, %r22;\n");
        if b_off_s > 0 {
            out.push_str(&format!("    add.u32 %r12, %r12, {};\n", b_off_s));
        }
        out.push_str("    mul.lo.u32 %r13, %r5, 4;\n");
        for j in 0..per_b {
            out.push_str("    mov.u32 %r14, %r13;\n");
            if j > 0 {
                out.push_str(&format!("    add.u32 %r14, %r14, {};\n", j * threads * 4));
            }
            out.push_str("    mov.u32 %r15, %r14;\n");
            out.push_str(&format!("    shr.u32 %r15, %r15, {};\n", 8 + gr.trailing_zeros()));
            out.push_str("    mov.u32 %r16, %r14;\n");
            out.push_str(&format!("    and.b32 %r16, %r16, {};\n", gr * 256 - 1));
            out.push_str(&format!("    shr.u32 %r17, %r16, {};\n", 4 + gr.trailing_zeros()));
            out.push_str(&format!("    and.b32 %r19, %r16, {};\n", gr * 16 - 1));
            out.push_str("    shr.u32 %r19, %r19, 1;\n");
            // global: (stripe+k)*b_row + (slab*(8*gr)+n)*2  (the stripe constant
            // folds the prologue's kstep; %r2 is NOT kstep here — it still
            // holds a setup product)
            out.push_str(&format!("    mul.lo.u32 %r18, %r17, {};\n", b_row));
            if stripe > 0 {
                out.push_str(&format!(
                    "    add.u32 %r18, %r18, {};\n",
                    stripe * b_row
                ));
            }
            out.push_str(&format!("    mul.lo.u32 %r20, %r15, {};\n", 8 * gr));
            out.push_str("    add.u32 %r20, %r20, %r19;\n");
            out.push_str("    shl.b32 %r20, %r20, 1;\n");
            out.push_str("    add.u32 %r18, %r18, %r20;\n");
            out.push_str("    mul.wide.u32 %rd5, %r18, 1;\n");
            out.push_str("    add.u64 %rd5, %rd3, %rd5;\n");
            // smem dst: slab*(256*gr) + k*(16*gr) + ((n>>3)^(k&(gr-1)))*16 + (n&7)*2
            out.push_str(&format!("    shl.b32 %r19, %r15, {};\n", 8 + gr.trailing_zeros()));
            out.push_str(&format!("    shl.b32 %r20, %r17, {};\n", 4 + gr.trailing_zeros()));
            out.push_str("    add.u32 %r19, %r19, %r20;\n");
            out.push_str("    shr.u32 %r20, %r14, 4;\n");
            out.push_str(&format!("    and.b32 %r20, %r20, {};\n", gr - 1));
            out.push_str(&format!("    and.b32 %r16, %r17, {};\n", gr - 1));
            out.push_str("    xor.b32 %r20, %r20, %r16;\n");
            out.push_str("    shl.b32 %r20, %r20, 4;\n");
            out.push_str("    add.u32 %r19, %r19, %r20;\n");
            out.push_str("    and.b32 %r20, %r14, 14;\n");
            out.push_str("    add.u32 %r19, %r19, %r20;\n");
            out.push_str("    mov.u32 %r16, %r12;\n");
            out.push_str("    add.u32 %r16, %r16, %r19;\n");
            out.push_str("    cp.async.ca.shared.global [%r16], [%rd5], 4;\n");
        }
        out.push_str("    cp.async.commit_group;\n");
    }
    out.push_str(&format!("    cp.async.wait_group {};\n", stages - 2));
    // Async-write visibility (2026-09-10, driver 580.178 / sm_86): the
    // documented wait_group + bar.sync pattern alone let ldmatrix read
    // stale smem natively (serialized debuggers masked it). membar.cta
    // closes the ordering gap; verified on-device (fence.proxy.async
    // needs sm_90 so it is not available here).
    out.push_str("    membar.cta;\n");
    out.push_str("    bar.sync 0;\n");

    // === K LOOP: double-buffered pipeline ===
    // Structure per iteration:
    //   1. Start async fill of buffer (kstep/16+1)%2
    //   2. Compute on buffer (kstep/16)%2  (overlaps with fill)
    //   3. Wait for fill + barrier
    if f16_acc {
        emit_y_pass(&mut out, false); // zero the CTA-private y tile
    }

    // === K LOOP pipeline body (reconstructed 2026-09-10 after fault bisection) ===
    out.push_str("    mov.u32 %r2, 0;  // kstep\n");
    out.push_str("KLOOP:\n");
    out.push_str(&format!("    setp.ge.u32 %p1, %r2, {};\n", k));
    out.push_str("    @%p1 bra KEND;\n");

    // Compute stage = (kstep/16) & (stages-1); fill stage = stage+(stages-1).
    out.push_str("    shr.u32 %r9, %r2, 4;\n");
    out.push_str(&format!("    and.b32 %r9, %r9, {};\n", stages - 1));
    out.push_str(&format!("    add.u32 %r17, %r9, {};\n", stages - 1));
    out.push_str(&format!("    and.b32 %r17, %r17, {};\n", stages - 1));

    // Compute smem base for the FILL stage: asmem + stage * asmem_buf
    out.push_str("    mov.u32 %r12, %r21;\n");
    out.push_str(&format!("    mul.lo.u32 %r18, %r17, {};\n", asmem_buf));
    out.push_str("    add.u32 %r12, %r12, %r18;\n");

    // The fill prefetches stripe kstep+16*(stages-1) (consumed stages later —
    // an off-by-one here would reuse stripe kstep and silently skip the last
    // stripes; the seeded-data error 3.2e-3 slipped under the 5e-3 gate once).
    // On the final iterations the prefetch would read past K, so it is skipped.
    out.push_str(&format!("    setp.ge.u32 %p1, %r2, {};\n", (k - 16 * (stages as i64 - 1)).max(0)));
    out.push_str("    @%p1 bra FILL_DONE;\n");

    // Cooperative A fill into the fill stage (coalesced D mapping)
    out.push_str("    mul.lo.u32 %r13, %r5, 4;\n");
    for j in 0..per_a {
        out.push_str("    mov.u32 %r14, %r13;\n");
        if j > 0 {
            out.push_str(&format!("    add.u32 %r14, %r14, {};\n", j * threads * 4));
        }
        out.push_str("    mov.u32 %r15, %r14;\n");
        out.push_str("    and.b32 %r16, %r14, 31;\n");
        out.push_str("    shr.u32 %r15, %r15, 5;\n");
        out.push_str(&format!("    mul.lo.u32 %r15, %r15, {};\n", a_row));
        out.push_str("    add.u32 %r15, %r15, %r16;\n");
        out.push_str("    mov.u32 %r16, %r2;\n");
        out.push_str(&format!("    add.u32 %r16, %r16, {};\n", 16 * (stages as i64 - 1)));
        out.push_str("    mul.lo.u32 %r16, %r16, 2;\n");
        out.push_str("    add.u32 %r15, %r15, %r16;\n");
        out.push_str("    mul.wide.u32 %rd5, %r15, 1;\n");
        out.push_str("    add.u64 %rd5, %rd2, %rd5;\n");
        out.push_str("    mov.u32 %r16, %r12;\n");
        out.push_str("    add.u32 %r16, %r16, %r14;\n");
        out.push_str("    cp.async.ca.shared.global [%r16], [%rd5], 4;\n");
    }

    // Cooperative B fill into the fill stage. Same k-major swizzled contract
    // as the prologue fill (see there); row advance is (kstep+16*(S-1)+k)*b_row.
    out.push_str("    mov.u32 %r12, %r22;\n");
    out.push_str(&format!("    mul.lo.u32 %r18, %r17, {};\n", bsmem_buf));
    out.push_str("    add.u32 %r12, %r12, %r18;\n");
    out.push_str("    mul.lo.u32 %r13, %r5, 4;\n");
    for j in 0..per_b {
        out.push_str("    mov.u32 %r14, %r13;\n");
        if j > 0 {
            out.push_str(&format!("    add.u32 %r14, %r14, {};\n", j * threads * 4));
        }
        out.push_str("    mov.u32 %r15, %r14;\n");
        out.push_str(&format!("    shr.u32 %r15, %r15, {};\n", 8 + gr.trailing_zeros()));
        out.push_str("    mov.u32 %r16, %r14;\n");
        out.push_str(&format!("    and.b32 %r16, %r16, {};\n", gr * 256 - 1));
        out.push_str(&format!("    shr.u32 %r17, %r16, {};\n", 4 + gr.trailing_zeros()));
        out.push_str(&format!("    and.b32 %r19, %r16, {};\n", gr * 16 - 1));
        out.push_str("    shr.u32 %r19, %r19, 1;\n");
        // global: (kstep+16*(S-1)+k)*b_row + (slab*(8*gr)+n)*2
        out.push_str("    mov.u32 %r18, %r2;\n");
        out.push_str(&format!("    add.u32 %r18, %r18, {};\n", 16 * (stages as i64 - 1)));
        out.push_str("    add.u32 %r18, %r18, %r17;\n");
        out.push_str(&format!("    mul.lo.u32 %r18, %r18, {};\n", b_row));
        out.push_str(&format!("    mul.lo.u32 %r20, %r15, {};\n", 8 * gr));
        out.push_str("    add.u32 %r20, %r20, %r19;\n");
        out.push_str("    shl.b32 %r20, %r20, 1;\n");
        out.push_str("    add.u32 %r18, %r18, %r20;\n");
        out.push_str("    mul.wide.u32 %rd5, %r18, 1;\n");
        out.push_str("    add.u64 %rd5, %rd3, %rd5;\n");
        // smem dst: slab*(256*gr) + k*(16*gr) + ((n>>3)^(k&(gr-1)))*16 + (n&7)*2
        out.push_str(&format!("    shl.b32 %r19, %r15, {};\n", 8 + gr.trailing_zeros()));
        out.push_str(&format!("    shl.b32 %r20, %r17, {};\n", 4 + gr.trailing_zeros()));
        out.push_str("    add.u32 %r19, %r19, %r20;\n");
        out.push_str("    shr.u32 %r20, %r14, 4;\n");
        out.push_str(&format!("    and.b32 %r20, %r20, {};\n", gr - 1));
        out.push_str(&format!("    and.b32 %r16, %r17, {};\n", gr - 1));
        out.push_str("    xor.b32 %r20, %r20, %r16;\n");
        out.push_str("    shl.b32 %r20, %r20, 4;\n");
        out.push_str("    add.u32 %r19, %r19, %r20;\n");
        out.push_str("    and.b32 %r20, %r14, 14;\n");
        out.push_str("    add.u32 %r19, %r19, %r20;\n");
        out.push_str("    mov.u32 %r16, %r12;\n");
        out.push_str("    add.u32 %r16, %r16, %r19;\n");
        out.push_str("    cp.async.ca.shared.global [%r16], [%rd5], 4;\n");
    }
    out.push_str("FILL_DONE:\n");
    out.push_str("    cp.async.commit_group;\n");

    // === Compute on CURRENT buffer (overlaps with async fill above) ===
    // Register-trimmed scheduling (2026-09-10): A fragments load per-mh
    // (4 live, was 8) and each B fragment loads immediately before its mma
    // (2 live, was 16). 136 -> 128 regs natural, which is what lets
    // select_mw_nw fund 512-thread CTAs.
    // (An evening-2026-09-10 experiment hoisted the loop-invariant B base
    // and lane terms — instruction count halved, perf DROPPED 11%: the
    // redundant uniform math was soaking up issue slots that now stall on
    // ldmatrix/mma dependencies. Reverted; measured before removed.)
    out.push_str("    mov.u64 %rd7, %rd8;\n");
    out.push_str(&format!("    mul.lo.u32 %r18, %r9, {};\n", asmem_buf));
    out.push_str("    mul.wide.u32 %rd5b, %r18, 1;\n");
    out.push_str("    add.u64 %rd7, %rd7, %rd5b;\n");
    out.push_str("    mov.u32 %r12, %r10;\n");
    out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", mhr * 512));
    out.push_str("    mul.wide.u32 %rd5b, %r12, 1;\n");
    out.push_str("    add.u64 %rd7, %rd7, %rd5b;\n");
    for mh in 0..mhr {
        // A: warp mh_warp slice, rows 16*mh..16*mh+15 at +mh*512 bytes.
        // Lane addressing: rows (lane>>3>>1)*8 + lane&7, k-block (lane>>3)&1.
        out.push_str(&format!("    add.u64 %rd4, %rd7, {};\n", mh * 512));
        out.push_str("    mov.u32 %r13, %r7;\n");
        out.push_str("    shr.u32 %r12, %r13, 3;\n");
        out.push_str("    and.b32 %r13, %r13, 7;\n");
        out.push_str("    and.b32 %r14, %r12, 1;\n");
        out.push_str("    shr.u32 %r15, %r12, 1;\n");
        out.push_str("    mul.lo.u32 %r15, %r15, 8;\n");
        out.push_str("    add.u32 %r15, %r15, %r13;\n");
        out.push_str("    mul.lo.u32 %r15, %r15, 32;\n");
        out.push_str("    mul.lo.u32 %r14, %r14, 16;\n");
        out.push_str("    add.u32 %r16, %r15, %r14;\n");
        out.push_str("    mul.wide.u32 %rd5, %r16, 1;\n");
        out.push_str("    add.u64 %rd5, %rd4, %rd5;\n");
        out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a0, %a1, %a2, %a3}, [%rd5];\n");
        out.push_str("    mov.b32 %t0, %a1; mov.b32 %a1, %a2; mov.b32 %a2, %t0;\n");

        // B fragments from the k-major swizzled tile. Per col-group g (8
        // cols): lane 0-15 addresses k-rows k=lane of 16 bytes at byte g*16,
        // XOR-swizzled by k&7 — matching the fill's ((n>>3)^(k&7))*16 chunk
        // remap. x2.trans: lanes 0-7 -> b-lo (k 0-7), lanes 8-15 -> b-hi
        // (k 8-15), both n 0-7.
        //
        // f16-acc: the B loads software-pipeline — preload g0/g1 into the
        // two register pairs, then each mma(g) is independent of the
        // ld-ahead for g+2 (alternate pairs), so the ~30clk ldmatrix
        // latency hides under the mma instead of serializing every g.
        // f32: serial per-g (the 128-reg 2-CTA budget cannot fund 4 B regs).
        let emit_b_ld = |out: &mut String, g: usize, lo: &str, hi: &str| {
            out.push_str("    mov.u64 %rd4, %rd9;\n");
            out.push_str(&format!("    mul.lo.u32 %r18, %r9, {};\n", bsmem_buf));
            out.push_str("    mul.wide.u32 %rd5b, %r18, 1;\n");
            out.push_str("    add.u64 %rd4, %rd4, %rd5b;\n");
            out.push_str("    mov.u32 %r12, %r11;\n");
            out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", gr * 256));
            out.push_str("    mul.wide.u32 %rd5b, %r12, 1;\n");
            out.push_str("    add.u64 %rd4, %rd4, %rd5b;\n");
            out.push_str("    mov.u32 %r13, %r7;\n");
            out.push_str("    and.b32 %r12, %r13, 15;\n");
            out.push_str(&format!("    shl.b32 %r12, %r12, {};\n", 4 + gr.trailing_zeros()));
            out.push_str(&format!("    and.b32 %r14, %r13, {};\n", gr - 1));
            out.push_str(&format!("    xor.b32 %r14, %r14, {};\n", g));
            out.push_str("    shl.b32 %r14, %r14, 4;\n");
            out.push_str("    add.u32 %r17, %r12, %r14;\n");
            out.push_str("    mul.wide.u32 %rd5, %r17, 1;\n");
            out.push_str("    add.u64 %rd5, %rd4, %rd5;\n");
            out.push_str(&format!(
                "    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {{{}, {}}}, [%rd5];\n",
                lo, hi
            ));
        };
        if f16_acc {
            emit_b_ld(&mut out, 0, "%b0", "%b1");
            emit_b_ld(&mut out, 1, "%b2", "%b3");
            for g in 0..gr {
                let (lo, hi) = if g % 2 == 0 { ("%b0", "%b1") } else { ("%b2", "%b3") };
                let cb = 2 * (mh * gr + g);
                out.push_str(&format!(
                    "    mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {{%c{}, %c{}}}, {{%a0, %a1, %a2, %a3}}, {{{}, {}}}, {{%c{}, %c{}}};\n",
                    cb, cb + 1, lo, hi, cb, cb + 1
                ));
                if g + 2 < gr {
                    // Refill the pair mma(g) JUST released — it is next
                    // needed at g+2. The other pair still holds g+1's
                    // fragment (loading there clobbered it before its mma:
                    // every mma(g>=1) consumed group-(g+1)'s B — the
                    // 4096x4096x16 f16acc 2.3e-1 failure).
                    let (nlo, nhi) = if g % 2 == 0 { ("%b0", "%b1") } else { ("%b2", "%b3") };
                    emit_b_ld(&mut out, g + 2, nlo, nhi);
                }
            }
        } else {
            for g in 0..gr {
                emit_b_ld(&mut out, g, "%b0", "%b1");
                let cb = 4 * (mh * gr + g);
                out.push_str(&format!(
                    "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c{}, %c{}, %c{}, %c{}}}, {{%a0, %a1, %a2, %a3}}, {{%b0, %b1}}, {{%c{}, %c{}, %c{}, %c{}}};\n",
                    cb, cb + 1, cb + 2, cb + 3, cb, cb + 1, cb + 2, cb + 3
                ));
            }
        }
    }

    // f16-acc chunk promotion: every 16 iterations (256 k) fold the f16x2
    // chunk accs into the y tile and reset them. Uniform predicate.
    if f16_acc {
        // 32-iteration chunks (512 k): half the RMV rounds; f16 rounding
        // measured 8e-4 at 16-iter chunks — 32 stays far under the 1e-2 gate.
        let chunk_iters = 32usize;
        out.push_str("    mov.u32 %r14, %r2;\n");
        out.push_str("    add.u32 %r14, %r14, 16;\n");
        out.push_str(&format!(
            "    and.b32 %r14, %r14, {};\n",
            chunk_iters * 16 - 1
        ));
        out.push_str("    setp.ne.u32 %p1, %r14, 0;\n");
        out.push_str("    @%p1 bra PROMO_SKIP;\n");
        emit_y_pass(&mut out, true);
        out.push_str("PROMO_SKIP:\n");
    }

    // Wait until this iteration's stage is complete (stages-2 groups remain
    // in flight) + async-write visibility (see the prologue note).
    out.push_str(&format!("    cp.async.wait_group {};\n", stages - 2));
    out.push_str("    membar.cta;\n");
    out.push_str("    bar.sync 0;\n");

    out.push_str("    add.u32 %r2, %r2, 16;\n");
    out.push_str("    bra.uni KLOOP;\nKEND:\n");

    // Store C (f32-acc) / remainder chunk promotion (f16-acc when K is not
    // a multiple of the 16-iteration chunk — the in-loop promotion covered
    // whole chunks already).
    if f16_acc {
        if k % (32 * 16) != 0 {
            emit_y_pass(&mut out, true);
        }
        out.push_str("    ret;\n}\n");
        return out;
    }
    out.push_str("    mov.u32 %r12, %r10;\n");
    out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", 16 * mhr as i64 * y_row));
    out.push_str("    mov.u32 %r13, %r11;\n");
    out.push_str(&format!("    mul.lo.u32 %r13, %r13, {};\n", 8 * gr as i64 * (y_elem as i64)));
    out.push_str("    mov.u32 %r9, %r12;\n");
    out.push_str("    add.u32 %r9, %r9, %r13;\n");
    for mh in 0..mhr {
        for g in 0..gr {
            let cb = 4 * (mh * gr + g);
            for (coff, row_off, col_off) in [
                (0i64, 0i64, 0i64),
                (1, 0, 1),
                (2, 8, 0),
                (3, 8, 1),
            ] {
                let cname = format!("c{}", cb + coff as usize);
                out.push_str(&format!("    mov.u32 %r13, {};\n", (16 * mh as i64 + row_off) * y_row));
                out.push_str(&format!("    mul.lo.u32 %r14, %r6, {};\n", y_row));
                out.push_str("    add.u32 %r13, %r13, %r14;\n");
                out.push_str(&format!("    add.u32 %r13, %r13, {};\n", (g as i64) * 8 * (y_elem as i64)));
                out.push_str("    mov.u32 %r14, %r8;\n");
                out.push_str(&format!("    mul.lo.u32 %r14, %r14, {};\n", y_elem));
                out.push_str("    add.u32 %r13, %r13, %r14;\n");
                out.push_str(&format!("    add.u32 %r13, %r13, {};\n", col_off * (y_elem as i64)));
                out.push_str("    add.u32 %r13, %r13, %r9;\n");
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
mod r16_dump {
    use super::*;
    #[test]
    fn cubin_emission_elf() {
        // compile_cubin: offline ptxas produces a loadable ELF cubin from
        // the emitted PTX. Tolerant skip when no ptxas is installed — the
        // PTX-text fallback is the documented contract then.
        let ptx = tensor_gemm_ptx_smem_mw(64, 64, 64, 0, 8192, 16392, 2, 1, 1, false, 4, 2);
        match crate::backend::ptx::compile_cubin(&ptx, 128) {
            Some(bytes) => assert_eq!(&bytes[0..4], b"\x7fELF", "cubin magic"),
            None => eprintln!("ptxas unavailable — PTX-text fallback (ok)"),
        }
    }

    #[test]
    fn dump_mw_1x1() {
        /* M=64,N=64,K=64, CTA 32x64 (mw=1,nw=1). a@0,b@8192,y@16392 */
        let ptx = tensor_gemm_ptx_smem_mw(64, 64, 64, 0, 8192, 16392, 2, 1, 1, false, 4, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_1x1.ptx", &ptx).unwrap();
    }

    use super::*;
    #[test]
    fn dump_mw_128x128() {
        /* M=128,N=128,K=64, CTA 128x128 (mw=4,nw=2). a@0,b@16384,y@32776 */
        let ptx = tensor_gemm_ptx_smem_mw(128, 128, 64, 0, 16384, 32776, 2, 4, 2, false, 4, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_128x128.ptx", &ptx).unwrap();
    }
    #[test]
    fn dump_mw_256x128() {
        /* M=256,N=128,K=64, CTA 256x128 (mw=8,nw=2). a@0,b=256*64*2=32768,
           y@32768+128*64*2+8=49160 */
        let ptx = tensor_gemm_ptx_smem_mw(256, 128, 64, 0, 32768, 49160, 2, 8, 2, false, 4, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_256x128.ptx", &ptx).unwrap();
    }

    use super::*;
    #[test]
    fn dump_r16_4096() {
        /* gemm_h 4096^3: a@0, b=4096*4096*2=33554432, y=b+K*N*2+8 */
        let ptx = tensor_gemm_ptx_smem_r16(4096, 4096, 4096, 0, 33554432, 67108872, 2);
        std::fs::write("/tmp/opencode/tgemm_r16_4096.ptx", &ptx).unwrap();
    }

    use super::*;
    #[test]
    fn dump_r16_64() {
        /* M=64,N=64,K=64: a@0, b@8192, y@16392, f16-y */
        let ptx = tensor_gemm_ptx_smem_r16(64, 64, 64, 0, 8192, 16392, 2);
        std::fs::write("/tmp/opencode/tgemm_r16_64.ptx", &ptx).unwrap();
    }

    use super::*;
    #[test]
    fn dump_r16_128() {
        /* M=128,N=128,K=128: a@0, b@32768, y@65544 */
        let ptx = tensor_gemm_ptx_smem_r16(128, 128, 128, 0, 32768, 65544, 2);
        std::fs::write("/tmp/opencode/tgemm_r16_128.ptx", &ptx).unwrap();
    }

    /* ── S4 shape portfolio (2026-09-10) ────────────────────────────── */

    #[test]
    fn dump_mw_2048() {
        /* M=N=K=2048, select_mw_nw=(4,4) block=512 (128-reg budget).
           a@0, b=2048*2048*2=8388608, y=2*8388608+8=16777224 */
        let ptx = tensor_gemm_ptx_smem_mw(2048, 2048, 2048, 0, 8388608, 16777224, 2, 2, 4, false, 4, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_2048.ptx", &ptx).unwrap();
    }

    #[test]
    fn dump_mw_2048_f16acc() {
        let ptx = tensor_gemm_ptx_smem_mw(2048, 2048, 2048, 0, 8388608, 16777224, 2, 4, 4, true, 2, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_2048_f16acc.ptx", &ptx).unwrap();
    }

    #[test]
    fn dump_mw_8192_f16acc() {
        let ptx = tensor_gemm_ptx_smem_mw(8192, 8192, 8192, 0, 134217728, 268435464, 2, 4, 4, true, 2, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_8192_f16acc.ptx", &ptx).unwrap();
    }

    #[test]
    fn dump_mw_4096_k16_f16acc() {
        for k in [16i64, 32, 64, 128, 256, 512] {
            let b_off = 4096 * k * 2;
            let y_off = ((b_off + 4096 * k * 2 + 7) & !7) + 8;
            let ptx = tensor_gemm_ptx_smem_mw(
                4096, 4096, k, 0, b_off as u64, y_off as u64, 2, 4, 4, true, 2, 2,
            );
            std::fs::write(&format!("/tmp/opencode/tgemm_mw_4096_k{k}_f16acc.ptx"), &ptx).unwrap();
        }
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 16, 0, 131072, 262152, 2, 4, 4, true, 2, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_4096_k16_f16acc.ptx", &ptx).unwrap();
    }

    #[test]
    fn dump_mw_4096_f16acc() {
        /* f16-acc contract variant (ptx_tensor_f16acc=1): f16x2 chunk accs,
           8-kstep... 16-iteration promotion into the f16 y tile. Gate 1e-2. */
        /* production pairing: f16acc funds (4,4)@512T; 2 stages keep smem
           at 40KB so TWO CTAs co-reside (16 warps + 2 fill streams). */
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 4096, 0, 33554432, 67108872, 2, 4, 4, true, 2, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_4096_f16acc.ptx", &ptx).unwrap();
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 4096, 0, 33554432, 67108872, 2, 2, 4, true, 4, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_4096_f16acc_c24.ptx", &ptx).unwrap();
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 4096, 0, 33554432, 67108872, 2, 4, 4, true, 4, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_4096_f16acc_s4.ptx", &ptx).unwrap();
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 4096, 0, 33554432, 67108872, 2, 2, 8, true, 2, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_4096_f16acc_28.ptx", &ptx).unwrap();
        // Double-pump variant C (plan 2026-09-11-ptx-double-pump-warp-tile):
        // 64x32 warp (warp_mh=4) — A fragments shared across 4 mh-blocks,
        // B loaded 4x per kstep instead of 16x. (8,2) and (4,4) CTAs.
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 4096, 0, 33554432, 67108872, 2, 8, 2, true, 2, 4);
        std::fs::write("/tmp/opencode/tgemm_mw_4096_f16acc_dp82.ptx", &ptx).unwrap();
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 4096, 0, 33554432, 67108872, 2, 4, 4, true, 2, 4);
        std::fs::write("/tmp/opencode/tgemm_mw_4096_f16acc_dp44.ptx", &ptx).unwrap();
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 4096, 0, 33554432, 67108872, 2, 8, 2, true, 2, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_4096_f16acc_82.ptx", &ptx).unwrap();
    }

    #[test]
    fn dump_mw_4096() {
        /* M=N=K=4096, select_mw_nw=(2,8), block=512.
           a@0, b=4096*4096*2=33554432, y=2*33554432+8=67108872 */
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 4096, 0, 33554432, 67108872, 2, 2, 4, false, 4, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_4096.ptx", &ptx).unwrap();
    }

    #[test]
    fn dump_mw_8192() {
        /* M=N=K=8192, select_mw_nw=(2,8), block=512.
           a@0, b=8192*8192*2=134217728, y=2*134217728+8=268435464 */
        let ptx = tensor_gemm_ptx_smem_mw(8192, 8192, 8192, 0, 134217728, 268435464, 2, 2, 4, false, 4, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_8192.ptx", &ptx).unwrap();
    }

    #[test]
    fn dump_mw_4096_k16() {
        /* M=N=4096,K=16 (skinny-K), select_mw_nw=(2,8), block=512.
           a@0, b=4096*16*2=131072, y=2*131072+8=262152 */
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 16, 0, 131072, 262152, 2, 2, 4, false, 4, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_4096_k16.ptx", &ptx).unwrap();
    }
}

#[cfg(test)]
mod smem_tests {
    use super::*;

    #[test]
    fn smem_geometry_and_mma() {
        let ptx = tensor_gemm_ptx_smem(64, 32, 32, 0, 8192, 32768, 4);
        assert!(ptx.contains(".entry main"), "entry");
        assert!(ptx.contains(".shared .align 16 .b8 asmem[1024];"), "asmem decl");
        assert!(ptx.contains(".shared .align 16 .b8 bsmem[512];"), "bsmem decl");
        assert_eq!(ptx.matches("mma.sync.aligned.m16n8k16").count(), 4, "mma count");
        assert_eq!(ptx.matches("ldmatrix.sync.aligned.m8n8.x4").count(), 2, "A ldmatrix x4 x2");
        assert_eq!(ptx.matches("ldmatrix.sync.aligned.m8n8.x2.trans").count(), 2, "B ldmatrix x2.trans x2");
        assert_eq!(ptx.matches("bar.sync 0").count(), 2, "barrier per kstep (fill + MMA)");
        assert!(ptx.contains("KLOOP:"), "k loop");
        assert!(ptx.contains("setp.ge.u32 %p1, %r10, 32;"), "K=32 bound");
    }

    #[test]
    fn smem_multi_kstep_fill_strides() {
        // The kstep strides: A fill advances 2 bytes (one f16 column) and the
        // B fill b_row bytes (one K row) per kstep — NOT k_elem-scaled strides
        // that read OOB for K > 16.
        let ptx = tensor_gemm_ptx_smem(32, 16, 32, 0, 2048, 3072, 4);
        assert!(ptx.contains("    mul.lo.u32 %r13, %r13, 2;\n"), "A fill kstep*2: {ptx}");
        assert!(ptx.contains("    mul.lo.u32 %r14, %r14, 32;\n"), "B fill kstep*b_row(N=16): {ptx}");
        // Each thread copies 8 b32 (32 B = 16 f16) for A and 4 b32 (16 B) for B.
        assert_eq!(ptx.matches("ld.global.b32 %t0, [%rd5]; st.shared.b32 [%rd4], %t0;").count(), 12,
            "A(8) + B(4) smem fill stores: {ptx}");
    }

    #[test]
    fn smem_b_blocked_fill_layout() {
        // The B tile is stored as 2x2 8x8 blocks so the x2.trans col-shifted
        // second 8x8 yields the mma's rows-8-15 fragment (the 2026-09-09 fix):
        //   bsmem[r<8][c] = B[r][c], bsmem[r<8][8+c] = B[8+r][c] (ng=0),
        //   bsmem[8+r][c] = B[r][8+c], bsmem[8+r][8+c] = B[8+r][8+c] (ng=1).
        // The fill uses row%8 (not row) for the B row and +16 bytes for
        // row>=8 (the ng=1 column block); the ldmatrix bases are bsmem (ng=0)
        // and bsmem+256 (ng=1).
        let ptx = tensor_gemm_ptx_smem(64, 32, 16, 0, 2048, 8192, 2);
        assert!(ptx.contains("    and.b32 %r15, %r12, 7;    // row%8\n"), "row%8: {ptx}");
        assert!(ptx.contains("    shr.u32 %r15, %r12, 3;    // row>=8 ? 1 : 0\n"), "row/8: {ptx}");
        assert!(ptx.contains("    add.u64 %rd4, %rd4, 0;\n") ||
                ptx.contains("    add.u64 %rd4, %rd4, 256;\n"),
            "B ldmatrix bases 0/256: {ptx}");
        assert!(ptx.contains("add.u64 %rd4, %rd4, 256;\n"), "ng=1 base bsmem+256: {ptx}");
        assert!(!ptx.contains("add.u64 %rd4, %rd4, 16;\n"), "old ng*16 base removed: {ptx}");
    }

    #[test]
    fn smem_proj_bases_include_rd1() {
        let ptx = tensor_gemm_ptx_smem(64, 32, 32, 0, 8192, 32768, 4);
        assert!(ptx.contains("add.u64 %rd2, %rd1, %rd2;"), "a base += proj");
        assert!(ptx.contains("add.u64 %rd3, %rd1, %rd3;"), "b base += proj");
        assert!(ptx.contains("add.u64 %rd6, %rd1, %rd6;"), "y base += proj");
    }

    #[test]
    fn smem_guards_extra_lanes() {
        let ptx = tensor_gemm_ptx_smem(32, 16, 16, 0, 1024, 4096, 4);
        assert!(ptx.contains("setp.ge.u32 %p1, %r5, 32;"), "lane guard");
        assert!(ptx.contains("@%p1 ret;"), "early return");
    }

    #[test]
    fn smem_f16_y_emits_cvt() {
        let ptx = tensor_gemm_ptx_smem(32, 16, 16, 0, 1024, 4096, 2);
        assert!(ptx.contains("cvt.rn.f16.f32 %t0, %c0;"), "f16 store cvt");
        assert!(ptx.contains("st.global.u16 [%rd5], %t0;"), "f16 store");
    }

    #[test]
    fn smem_f16_y_stride_is_n_elem_not_n4() {
        let ptx = tensor_gemm_ptx_smem(32, 16, 16, 0, 1024, 4096, 2);
        assert!(ptx.contains("mul.lo.u32 %r14, %r6, 32;"), "y_row=N*2=32: {ptx}");
        assert!(ptx.contains("mul.lo.u32 %r9, %r9, 32;"), "n_cta col = 16*2: {ptx}");
    }

    #[test]
    fn smem_cstore_uses_tile_accs() {
        let ptx = tensor_gemm_ptx_smem(32, 32, 16, 0, 2048, 8192, 4);
        for cb in [0, 4, 8, 12] {
            assert!(ptx.contains(&format!("st.global.f32 [%rd5], %c{};", cb)),
                "tile store c{}", cb);
        }
    }
}
