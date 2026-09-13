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

/// E5a shared emitter: gr x ldmatrix.x2.trans reads from %rd5c (the caller
/// sets it to the slab base of the stage being loaded) into the B register
/// set at `set_off`. Per-g address math uses the E4b precomputed lane terms
/// (%r25 b_l15, %r26 b_l3) — identical to the in-kstep schedule's loader.
fn emit_e5a_b_set(out: &mut String, gr: usize, set_off: usize) {
    for g in 0..gr {
        out.push_str(&format!("    xor.b32 %r14, %r26, {};\n", g));
        out.push_str("    shl.b32 %r14, %r14, 4;\n");
        out.push_str("    add.u32 %r17, %r25, %r14;\n");
        out.push_str("    mul.wide.u32 %rd5, %r17, 1;\n");
        out.push_str("    add.u64 %rd5, %rd5c, %rd5;\n");
        out.push_str(&format!(
            "    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {{%b{}, %b{}}}, [%rd5];\n",
            set_off + 2 * g,
            set_off + 2 * g + 1
        ));
    }
}

/// E5a shared emitter: the B-slab base (%rd5c = %rd9 + stage·bsmem_buf +
/// ng_warp·gr·256) from a stage held in PTX reg `stage_reg`.
fn emit_e5a_b_base(
    out: &mut String,
    stage_reg: &str,
    bsmem_buf: usize,
    gr: usize,
) {
    out.push_str("    mov.u64 %rd5c, %rd9;\n");
    out.push_str(&format!(
        "    mul.lo.u32 %r12, {}, {};\n",
        stage_reg, bsmem_buf
    ));
    out.push_str("    mul.wide.u32 %rd5b, %r12, 1;\n");
    out.push_str("    add.u64 %rd5c, %rd5c, %rd5b;\n");
    out.push_str("    mov.u32 %r12, %r11;\n");
    out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", gr * 256));
    out.push_str("    mul.wide.u32 %rd5b, %r12, 1;\n");
    out.push_str("    add.u64 %rd5c, %rd5c, %rd5b;\n");
}

/// E5a (2026-09-13, REJECTED on-device — see ptx_tensor_b_lookahead): the
/// three lookahead emission points, kept as the reference implementation
/// and instrument for deeper-pipeline variants.
/// Prologue: preload stage 0's B into set0 after the initial barrier.
fn emit_e5a_prologue(out: &mut String, gr: usize, bsmem_buf: usize) {
    emit_e5a_b_base(out, "0", bsmem_buf, gr);
    emit_e5a_b_set(out, gr, 0);
    out.push_str("    mov.u32 %r27, 0;\n");
}

/// Compute phase: the 16 mma consume the prefetched set selected by the
/// %r27 parity (0 → set0 = %b{2g}.., 1 → set1 = %b{2*gr+2g}..).
fn emit_e5a_mma_branch(out: &mut String, mhr: usize, gr: usize) {
    let emit_mma_set = |out: &mut String, set_off: usize| {
        for mh in 0..mhr {
            for g in 0..gr {
                let (lo, hi) = (
                    format!("%b{}", set_off + 2 * g),
                    format!("%b{}", set_off + 2 * g + 1),
                );
                let (a0, a1, a2, a3) = (
                    format!("%a{}", 4 * mh),
                    format!("%a{}", 4 * mh + 1),
                    format!("%a{}", 4 * mh + 2),
                    format!("%a{}", 4 * mh + 3),
                );
                let cb = 2 * (mh * gr + g);
                out.push_str(&format!(
                    "    mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {{%c{}, %c{}}}, {{{}, {}, {}, {}}}, {{{}, {}}}, {{%c{}, %c{}}};\n",
                    cb, cb + 1, a0, a1, a2, a3, lo, hi, cb, cb + 1
                ));
            }
        }
    };
    out.push_str("    setp.ne.u32 %p2, %r27, 0;\n");
    out.push_str("    @%p2 bra MMA_ODD;\n");
    emit_mma_set(out, 0);
    out.push_str("    bra MMA_DONE;\n");
    out.push_str("MMA_ODD:\n");
    emit_mma_set(out, 2 * gr);
    out.push_str("MMA_DONE:\n");
}

/// Tail (after the iteration barrier): load the NEXT stage's B into the
/// set opposite to %r27, then flip the parity. Skipped on the final kstep.
fn emit_e5a_tail_prefetch(
    out: &mut String,
    k: i64,
    stages: usize,
    gr: usize,
    bsmem_buf: usize,
) {
    out.push_str("    add.u32 %r28, %r2, 16;\n");
    out.push_str(&format!("    setp.ge.u32 %p2, %r28, {};\n", k));
    out.push_str("    @%p2 bra BSKIP;\n");
    out.push_str("    add.u32 %r28, %r9, 1;\n");
    out.push_str(&format!("    and.b32 %r28, %r28, {};\n", stages - 1));
    emit_e5a_b_base(out, "%r28", bsmem_buf, gr);
    out.push_str("    setp.ne.u32 %p2, %r27, 0;\n");
    out.push_str("    @%p2 bra LD_ODD;\n");
    emit_e5a_b_set(out, gr, 2 * gr);
    out.push_str("    mov.u32 %r27, 1;\n");
    out.push_str("    bra LD_DONE;\n");
    out.push_str("LD_ODD:\n");
    emit_e5a_b_set(out, gr, 0);
    out.push_str("    mov.u32 %r27, 0;\n");
    out.push_str("LD_DONE:\n");
    out.push_str("BSKIP:\n");
}

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
    // E5a (2026-09-13): the cross-kstep B lookahead is a config knob — the
    // dump tests call tensor_gemm_ptx_smem_mw_opt directly to A/B it.
    let b_lookahead = f16_acc && crate::config_tuning::ir_lowering().ptx_tensor_b_lookahead;
    tensor_gemm_ptx_smem_mw_opt(
        m, n, k, a_off, b_off, y_off, y_elem, mw, nw, f16_acc, stages, warp_mh, b_lookahead,
    )
}

fn tensor_gemm_ptx_smem_mw_opt(
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
    b_lookahead: bool,
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
    // b_lookahead is an f16-acc-only structure (the E5a emitters address the
    // f16acc B register sets); the wrapper never combines it with f32.
    debug_assert!(!b_lookahead || f16_acc);
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
    // Widened A fill (2026-09-12, plan 2026-09-11-ptx-mma-issue-ceiling
    // §Fill-gap): microbench variants a/8/c (identical loop, synchronous
    // 4B/8B/16B ld+st at 32B/thread) measured 31.6/37.0/35.4 TF — but that
    // overstates the transferable win because the real fill is already
    // async cp.async (the microbench win came largely from unblocking the
    // register roundtrip). On-kernel A/B at 4096³ f16acc (old 4B vs 8B
    // rung, interleaved ×4, same window): 28.97 → 29.41 TF (+0.44 TF,
    // new wins 4/4 rounds), correctness byte-identical (5.208e-3).
    // Ladder: per_a%4==0 → per_a/4 copies of 16B (.cg, the only L1-bypass
    // width), else per_a%2==0 → per_a/2 copies of 8B (.ca), else the 4B
    // loop. The production f16acc tile (mw=4, mhr=2, 512T) has per_a=2 →
    // one 8-byte copy per thread. Alignment: each chunk stays inside one
    // 32-byte smem row (D%32 + width ≤ 32); the global image row*a_row +
    // col + 2*koff is width-aligned because the kernel requires K%16==0
    // (a_row%32==0, koff%16==0) and col ∈ {0,8,16,24}. Coverage is an exact
    // bijection of the stage bytes at every rung. Non-divisible per_a keeps
    // the 4-byte loop. Undo: delete a_fill_rung and restore the single 4B
    // loop in the prologue + K-loop fills.
    let a_fill_rung = if per_a % 4 == 0 {
        Some((per_a / 4, 16, "cg"))
    } else if per_a % 2 == 0 {
        Some((per_a / 2, 8, "ca"))
    } else {
        None
    };
    // Widened B fill (2026-09-12): same rung ladder as the A fill — but B
    // needs no microbench preamble because the swizzle is exactly 16B
    // granular: a 16B global chunk (8 consecutive n of one k-row) IS one
    // swizzle unit, landing whole at ((n>>3)^(k&(gr-1)))*16. per_b=4 in
    // the production tile → one 16B cp.async.cg per thread (was 4×4B .ca).
    // Alignment: global (k)*b_row + (slab*64+n)*2 is 16B-aligned under the
    // kernel's N%8==0 precondition (b_row%16==0, n%8==0 at the 16B rung);
    // the smem chunk base is 16B-aligned by construction. Coverage is an
    // exact bijection of the stage bytes at every rung. Undo: delete
    // b_fill_rung and restore the single 4B loop in both B fill sites.
    let b_fill_rung = if per_b % 4 == 0 {
        Some((per_b / 4, 16, "cg", 0u32))
    } else if per_b % 2 == 0 {
        Some((per_b / 2, 8, "ca", 8u32))
    } else {
        None
    };
    // Shared B-fill emitter (2026-09-12 DRY merge of the prologue and
    // K-loop copies): the caller sets %r12 to the fill stage's smem base
    // (consuming %r17 = stage index BEFORE this emitter clobbers it) and
    // passes `r18_prelude` — the per-site lines computing r18 =
    // k_global*b_row (const stripe add in the prologue, dynamic kstep
    // block in the K loop). The last tuple element is the within-chunk
    // byte mask (0 at the 16B rung = swizzle-unit copies, no offset add).
    let mut emit_b_fill = |out: &mut String, r18_prelude: &str| {
        let (copies, width, modifier, chunk_mask) = match b_fill_rung {
            Some((c, w, m, mask)) => (c, w, m, mask),
            None => (per_b, 4, "ca", 14),
        };
        let glz = gr.trailing_zeros();
        out.push_str(&format!("    mul.lo.u32 %r13, %r5, {};\n", width));
        for j in 0..copies {
            out.push_str("    mov.u32 %r14, %r13;\n");
            if j > 0 {
                out.push_str(&format!("    add.u32 %r14, %r14, {};\n", j * threads * width));
            }
            out.push_str("    mov.u32 %r15, %r14;\n");
            out.push_str(&format!("    shr.u32 %r15, %r15, {};\n", 8 + glz));
            out.push_str("    mov.u32 %r16, %r14;\n");
            out.push_str(&format!("    and.b32 %r16, %r16, {};\n", gr * 256 - 1));
            out.push_str(&format!("    shr.u32 %r17, %r16, {};\n", 4 + glz));
            out.push_str(&format!("    and.b32 %r19, %r16, {};\n", gr * 16 - 1));
            out.push_str("    shr.u32 %r19, %r19, 1;\n");
            out.push_str(r18_prelude);
            out.push_str(&format!("    mul.lo.u32 %r20, %r15, {};\n", 8 * gr));
            out.push_str("    add.u32 %r20, %r20, %r19;\n");
            out.push_str("    shl.b32 %r20, %r20, 1;\n");
            out.push_str("    add.u32 %r18, %r18, %r20;\n");
            out.push_str("    mul.wide.u32 %rd5, %r18, 1;\n");
            out.push_str("    add.u64 %rd5, %rd3, %rd5;\n");
            // smem dst: slab*(256*gr) + k*(16*gr) + ((n>>3)^(k&(gr-1)))*16 [+ (n&mask)*2]
            out.push_str(&format!("    shl.b32 %r19, %r15, {};\n", 8 + glz));
            out.push_str(&format!("    shl.b32 %r20, %r17, {};\n", 4 + glz));
            out.push_str("    add.u32 %r19, %r19, %r20;\n");
            out.push_str("    shr.u32 %r20, %r14, 4;\n");
            out.push_str(&format!("    and.b32 %r20, %r20, {};\n", gr - 1));
            out.push_str(&format!("    and.b32 %r16, %r17, {};\n", gr - 1));
            out.push_str("    xor.b32 %r20, %r20, %r16;\n");
            out.push_str("    shl.b32 %r20, %r20, 4;\n");
            out.push_str("    add.u32 %r19, %r19, %r20;\n");
            if chunk_mask > 0 {
                out.push_str(&format!("    and.b32 %r20, %r14, {};\n", chunk_mask));
                out.push_str("    add.u32 %r19, %r19, %r20;\n");
            }
            out.push_str("    mov.u32 %r16, %r12;\n");
            out.push_str("    add.u32 %r16, %r16, %r19;\n");
            out.push_str(&format!(
                "    cp.async.{}.shared.global [%r16], [%rd5], {};\n",
                modifier, width
            ));
        }
    };
    // Shared A-fill emitter (2026-09-12 DRY merge of the prologue and
    // K-loop copies): the caller sets %r12 to the fill stage's smem base
    // and passes the per-site source-offset adjustment (`src_off`) — the
    // constant stripe add in the prologue, the dynamic kstep block in the
    // K loop.
    let mut emit_a_fill = |out: &mut String, src_off: &str| {
        match a_fill_rung {
            Some((copies, width, modifier)) => {
                out.push_str(&format!("    mul.lo.u32 %r13, %r5, {};\n", width));
                for j in 0..copies {
                    out.push_str("    mov.u32 %r14, %r13;\n");
                    if j > 0 {
                        out.push_str(&format!("    add.u32 %r14, %r14, {};\n", j * threads * width));
                    }
                    out.push_str("    mov.u32 %r15, %r14;\n");
                    out.push_str("    and.b32 %r16, %r14, 31;\n");
                    out.push_str("    shr.u32 %r15, %r15, 5;\n");
                    out.push_str(&format!("    mul.lo.u32 %r15, %r15, {};\n", a_row));
                    out.push_str("    add.u32 %r15, %r15, %r16;\n");
                    out.push_str(src_off);
                    out.push_str("    mul.wide.u32 %rd5, %r15, 1;\n");
                    out.push_str("    add.u64 %rd5, %rd2, %rd5;\n");
                    out.push_str("    mov.u32 %r16, %r12;\n");
                    out.push_str("    add.u32 %r16, %r16, %r14;\n");
                    out.push_str(&format!(
                        "    cp.async.{}.shared.global [%r16], [%rd5], {};\n",
                        modifier, width
                    ));
                }
            }
            None => {
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
                    out.push_str(src_off);
                    out.push_str("    mul.wide.u32 %rd5, %r15, 1;\n");
                    out.push_str("    add.u64 %rd5, %rd2, %rd5;\n");
                    out.push_str("    mov.u32 %r16, %r12;\n");
                    out.push_str("    add.u32 %r16, %r16, %r14;\n");
                    out.push_str("    cp.async.ca.shared.global [%r16], [%rd5], 4;\n");
                }
            }
        }
    };
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
    out.push_str("    .reg .u32  %r21, %r22, %r23, %r24, %r25, %r26;\n");
    // E5a lookahead control regs (%r27 = mma parity, %r28 = prefetch
    // scratch) + the extra predicate; declared only on the lookahead path
    // so the default PTX stays byte-identical.
    if b_lookahead {
        out.push_str("    .reg .u32  %r27, %r28;\n");
        out.push_str("    .reg .pred %p2;\n");
    }
    // Register scheduling (2026-09-11 E4a, plan 2026-09-11-ptx-mma-issue-ceiling):
    // f16-acc CLUSTER-PRELOADS — all 8 B fragments (%b0-%b15) and both A
    // blocks (%a0-%a7) issue back-to-back at the top of the compute phase,
    // so the ~30clk ldmatrix latencies overlap each other and the fill
    // instead of stalling every mma (the old g+2 ld-ahead hid only ~15 of
    // 30 clk per ld; the no-fill KLOOP ceiling measured 29.9 vs 53.5 for
    // fixed-address lds). Costs +16 regs over the trimmed schedule → ~80
    // regs ⇒ 1 CTA/SM at 512T — measured rational: E1b (no stalls) hit
    // 52.6 TF at 1 CTA/SM, occupancy only pays when stalls exist.
    // f32 keeps the trimmed 2-B-reg serial schedule byte-identical.
    let (aregs, bregs): (Vec<String>, Vec<String>) = if f16_acc {
        // The packed schedule addresses %a{4*mh}..%a{4*mh+3} per mh-block
        // and %b{2*g}..%b{2*g+1} per col-group, so the declarations must
        // scale with the warp shape (mhr=2 → a0-a7/b0-b15 as always; the
        // warp_mh=4 dumps previously declared %a<8> and failed ptxas with
        // unknown %a8-a15 — found 2026-09-12 in the E2 sweep).
        // E5a lookahead: a SECOND B set (%b{2*gr}..) receives the next
        // kstep's fragments across the barrier (+2*gr b32 regs).
        (
            (0..4 * mhr as usize).map(|i| format!("%a{}", i)).collect(),
            (0..if b_lookahead { 4 * gr } else { 2 * gr })
                .map(|i| format!("%b{}", i))
                .collect(),
        )
    } else {
        (
            vec!["%a0".to_string(), "%a1".to_string(), "%a2".to_string(), "%a3".to_string()],
            vec!["%b0".to_string(), "%b1".to_string()],
        )
    };
    out.push_str(&format!(
        "    .reg .b32  {}, {}, %t0;\n",
        aregs.join(", "),
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
    // y pass emitter. Three modes (2026-09-11): Accumulate folds the f16x2
    // chunk accs into y by read-modify-write (ld+add+st, resets accs);
    // Zero stores 0; StoreOnly writes the accs WITHOUT the global read —
    // the full-K final pass owns every y element (the kernel zeroed it
    // historically only because the RMV needed a operand; a pure store
    // needs nothing) and the cold-miss RMV reads measured 17 TFLOP/s of
    // end-of-kernel DRAM interference (45.4 stripped vs 28.0 with one
    // RMV round, three reps).
    let mut emit_y_pass = |out: &mut String, mode: u8| {
        let (accumulate, store_only) = (mode == 1, mode == 2);
        out.push_str("    mov.u32 %r12, %r10;\n");
        out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", 16 * mhr as i64 * y_row));
        out.push_str("    mov.u32 %r13, %r11;\n");
        out.push_str(&format!("    mul.lo.u32 %r13, %r13, {};\n", 8 * gr as i64 * (y_elem as i64)));
        out.push_str("    mov.u32 %r9, %r12;\n");
        out.push_str("    add.u32 %r9, %r9, %r13;\n");
        if mode == 0 {
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
                    if store_only {
                        out.push_str(&format!("    st.global.b32 [%rd5], {};\n", reg));
                    } else if accumulate {
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

    // Precompute lane-invariant address terms (2026-09-13 E4b):
    // The lane→row/k-block decomposition is identical across all mh (A)
    // and all g (B) iterations. Computing once saves ~80 ALU per kstep
    // per warp while preserving the mh/g-dependent offsets as
    // bubble-filler for ldmatrix latency.
    if f16_acc {
        // a_rf = (lane>>4)*8 + (lane&7) — row factor for A addressing
        out.push_str("    shr.u32 %r12, %r7, 4;\n");
        out.push_str("    shl.b32 %r23, %r12, 3;\n");
        out.push_str("    and.b32 %r12, %r7, 7;\n");
        out.push_str("    add.u32 %r23, %r23, %r12;\n");
        // a_cf = ((lane>>3)&1) * 16 — k-block factor for A addressing
        out.push_str("    shr.u32 %r12, %r7, 3;\n");
        out.push_str("    and.b32 %r12, %r12, 1;\n");
        out.push_str("    shl.b32 %r24, %r12, 4;\n");
        // b_l15 = (lane&15) << (4 + gr.trailing_zeros()) — B column factor
        out.push_str("    and.b32 %r25, %r7, 15;\n");
        out.push_str(&format!(
            "    shl.b32 %r25, %r25, {};\n",
            4 + gr.trailing_zeros()
        ));
        // b_l3 = lane & 3 — B XOR factor
        out.push_str("    and.b32 %r26, %r7, 3;\n");
    }

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
        // Coalesced fill mapping (2026-09-10): D = tid*width + j*threads*width
        // — consecutive lanes touch consecutive chunks, so each warp's copies
        // cover full 128B lines. The per-thread stride here forced 4B reads
        // out of separate 32B sectors. Widened rungs: see a_fill_rung above.
        let src_off = if stripe > 0 {
            format!("    add.u32 %r15, %r15, {};\n", stripe * 2)
        } else {
            String::new()
        };
        emit_a_fill(&mut out, &src_off);
        // B fill into stage s. Layout contract (must mirror the B ldmatrix
        // reader below): the slab is K-MAJOR — smem byte D = slab*2048 +
        // k*128 + n*2 (pre-swizzle), 128-byte k-rows of 64 n-cols. A fill
        // copy moves global B[stripe+k][n..] to the same-shaped smem pair;
        // the widened rung copies whole 16B swizzle units — see b_fill_rung.
        // 16B chunks of each k-row are XOR-swizzled with k&7 so ldmatrix's
        // 16 rows touch 8 bank groups, not 1. The pre-rewrite n-major fill
        // could not be sourced by 4-byte cp.async at all — that inversion
        // was the 2026-09-10 fault.
        out.push_str("    mov.u32 %r12, %r22;\n");
        if b_off_s > 0 {
            out.push_str(&format!("    add.u32 %r12, %r12, {};\n", b_off_s));
        }
        // global: (stripe+k)*b_row  (the stripe constant folds the
        // prologue's kstep; %r2 is NOT kstep here — it still holds a setup
        // product)
        let r18_prelude = {
            let mut s = format!("    mul.lo.u32 %r18, %r17, {};\n", b_row);
            if stripe > 0 {
                s.push_str(&format!("    add.u32 %r18, %r18, {};\n", stripe * b_row));
            }
            s
        };
        emit_b_fill(&mut out, &r18_prelude);
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

    // E5a prologue preload: stage 0's B fragments into set0 (%b0..). The
    // lane terms (%r25/%r26) are ready (computed above); %r11 (ng_warp)
    // and %rd9 (bsmem base) too. Stage 0 is the only stage whose B the
    // first iteration consumes without a prior in-loop prefetch.
    if b_lookahead {
        emit_e5a_prologue(&mut out, gr, bsmem_buf);
    }

    // === K LOOP: double-buffered pipeline ===
    // Structure per iteration:
    //   1. Start async fill of buffer (kstep/16+1)%2
    //   2. Compute on buffer (kstep/16)%2  (overlaps with fill)
    //   3. Wait for fill + barrier
    if f16_acc {
        // (The historical y-zeroing pass is gone with the RMV promo: the
        // final store-only pass overwrites every element the fragments
        // cover — the full CTA tile — so nothing needs a prior zero.)
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

    // Cooperative A fill into the fill stage (coalesced D mapping; widened
    // rung per a_fill_rung above — identical address decomposition, fewer
    // copies). Source offset advances by the prefetch distance
    // kstep + 16*(stages-1).
    let koff_src_off = {
        let mut s = String::from("    mov.u32 %r16, %r2;\n");
        s.push_str(&format!("    add.u32 %r16, %r16, {};\n", 16 * (stages as i64 - 1)));
        s.push_str("    mul.lo.u32 %r16, %r16, 2;\n");
        s.push_str("    add.u32 %r15, %r15, %r16;\n");
        s
    };
    emit_a_fill(&mut out, &koff_src_off);

    // Cooperative B fill into the fill stage. Same k-major swizzled contract
    // as the prologue fill (see there); row advance is (kstep+16*(S-1)+k)*b_row.
    // Widened rung per b_fill_rung above; %r12 consumes %r17 (stage) before
    // the emitter clobbers it.
    out.push_str("    mov.u32 %r12, %r22;\n");
    out.push_str(&format!("    mul.lo.u32 %r18, %r17, {};\n", bsmem_buf));
    out.push_str("    add.u32 %r12, %r12, %r18;\n");
    // global: (kstep+16*(S-1)+k)*b_row
    let koff_r18_prelude = {
        let mut s = String::from("    mov.u32 %r18, %r2;\n");
        s.push_str(&format!("    add.u32 %r18, %r18, {};\n", 16 * (stages as i64 - 1)));
        s.push_str("    add.u32 %r18, %r18, %r17;\n");
        s.push_str(&format!("    mul.lo.u32 %r18, %r18, {};\n", b_row));
        s
    };
    emit_b_fill(&mut out, &koff_r18_prelude);
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
    // B-fragment address+ldmatrix emitter (shared by both schedules).
    // Per col-group g (8 cols): lane 0-15 addresses k-rows k=lane of 16
    // bytes at byte g*16, XOR-swizzled by k&7 — matching the fill's
    // ((n>>3)^(k&7))*16 chunk remap. x2.trans: lanes 0-7 -> b-lo (k 0-7),
    // lanes 8-15 -> b-hi (k 8-15), both n 0-7.
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
        // E4b precomputed-lane schedule (2026-09-13): lane→row/k-block
        // decomposition done once at kernel entry (%r23=a_rf, %r24=a_cf,
        // %r25=b_l15, %r26=b_l3). Per-mh A load: 5 ALU + ldmatrix + swap
        // = 8 instr (was 13+1+2=16). Per-g B load: 6 ALU + ldmatrix = 7
        // instr (was 17). B base (stage+n_warp) computed once per kstep
        // before the g loop. Total savings ~80 ALU per kstep per warp.
        for mh in 0..mhr {
            out.push_str(&format!("    add.u64 %rd4, %rd7, {};\n", mh * 512));
            out.push_str("    shl.b32 %r16, %r23, 5;\n");
            out.push_str("    add.u32 %r16, %r16, %r24;\n");
            out.push_str("    mul.wide.u32 %rd5, %r16, 1;\n");
            out.push_str("    add.u64 %rd5, %rd4, %rd5;\n");
            out.push_str(&format!(
                "    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%a{}, %a{}, %a{}, %a{}}}, [%rd5];\n",
                4 * mh, 4 * mh + 1, 4 * mh + 2, 4 * mh + 3
            ));
            out.push_str(&format!(
                "    mov.b32 %t0, %a{}; mov.b32 %a{}, %a{}; mov.b32 %a{}, %t0;\n",
                4 * mh + 1, 4 * mh + 1, 4 * mh + 2, 4 * mh + 2
            ));
        }
        // E5a lookahead: the mma consumes the B set prefetched across the
        // previous iteration's barrier (%r27 = 0 → set0, 1 → set1); the
        // in-kstep B loads and B-base math move to the tail prefetch. No
        // smem B reads happen in the compute phase at all — ldmatrix
        // latency hides entirely behind the previous kstep's mma stream.
        if b_lookahead {
            emit_e5a_mma_branch(&mut out, mhr, gr);
        } else {
            // B base (stage + n_warp offset) — invariant across g, computed once.
            out.push_str("    mov.u64 %rd5c, %rd9;\n");
            out.push_str(&format!(
                "    mul.lo.u32 %r18, %r9, {};\n",
                bsmem_buf
            ));
            out.push_str("    mul.wide.u32 %rd5b, %r18, 1;\n");
            out.push_str("    add.u64 %rd5c, %rd5c, %rd5b;\n");
            out.push_str("    mov.u32 %r12, %r11;\n");
            out.push_str(&format!(
                "    mul.lo.u32 %r12, %r12, {};\n",
                gr * 256
            ));
            out.push_str("    mul.wide.u32 %rd5b, %r12, 1;\n");
            out.push_str("    add.u64 %rd5c, %rd5c, %rd5b;\n");
            // Per-g B loads with precomputed lane terms.
            for g in 0..gr {
                let (lo, hi) = (format!("%b{}", 2 * g), format!("%b{}", 2 * g + 1));
                out.push_str(&format!(
                    "    xor.b32 %r14, %r26, {};\n",
                    g
                ));
                out.push_str("    shl.b32 %r14, %r14, 4;\n");
                out.push_str("    add.u32 %r17, %r25, %r14;\n");
                out.push_str("    mul.wide.u32 %rd5, %r17, 1;\n");
                out.push_str("    add.u64 %rd5, %rd5c, %rd5;\n");
                out.push_str(&format!(
                    "    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {{{}, {}}}, [%rd5];\n",
                    lo, hi
                ));
            }
            for mh in 0..mhr {
                for g in 0..gr {
                    let (lo, hi) = (format!("%b{}", 2 * g), format!("%b{}", 2 * g + 1));
                    let (a0, a1, a2, a3) = (
                        format!("%a{}", 4 * mh),
                        format!("%a{}", 4 * mh + 1),
                        format!("%a{}", 4 * mh + 2),
                        format!("%a{}", 4 * mh + 3),
                    );
                    let cb = 2 * (mh * gr + g);
                    out.push_str(&format!(
                        "    mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {{%c{}, %c{}}}, {{{}, {}, {}, {}}}, {{{}, {}}}, {{%c{}, %c{}}};\n",
                        cb, cb + 1, a0, a1, a2, a3, lo, hi, cb, cb + 1
                    ));
                }
            }
        }
    } else {
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
            // f32: serial per-g (the 128-reg 2-CTA budget cannot fund 4 B
            // regs — the f32 path stays byte-identical through E4a).
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

    // f16-acc promotion: FULL-K register accumulation (2026-09-11, the
    // night's decisive probe) — the every-512-k y-RMV round cost 24 TFLOP/s
    // at 4096^3 (21.4 with vs 45.8 stripped, three reps): 138 pipeline
    // drains of 128 global-RMV instructions each. cuBLAS's structure is
    // full-K f16 accumulation with ONE epilogue; the f16x2 chain's random-
    // walk rounding at K=4096 measures ~5e-3 — inside the 1e-2 contract.
    // The chunk predicate therefore fires only on the final iteration
    // (chunk covers the whole K loop); the KEND remainder-promo handles
    // K not divisible by 16. Tier boundary: like the coopmat f16acc tier,
    // the f16 chain breaks the 1e-2 gate around K≈12288 (the documented
    // K-budget) — larger K belongs on the f32-acc tier.
    // (The final y store lives AFTER KEND — 2026-09-11: the in-loop
    // predicated promo structure, even firing once, measured 28 vs 45 TF
    // with the identical body made unconditional; the loop tail must stay
    // clean for ptxas's scheduling. The store-only pass overwrites every
    // element the fragments cover.)

    // Wait until this iteration's stage is complete (stages-2 groups remain
    // in flight) + async-write visibility (see the prologue note).
    out.push_str(&format!("    cp.async.wait_group {};\n", stages - 2));
    out.push_str("    membar.cta;\n");
    out.push_str("    bar.sync 0;\n");

    // E5a tail prefetch: the barrier above makes the NEXT stage's smem
    // visible — load its B fragments into the set the next iteration's mma
    // will consume, then flip the parity. Skipped on the final kstep (no
    // next B exists). The fill issued this iteration targets exactly this
    // stage (fill stage = (compute stage + stages-1) & mask = compute+1 at
    // stages=2), so the data is the kstep+16 stripe.
    if b_lookahead {
        emit_e5a_tail_prefetch(&mut out, k, stages, gr, bsmem_buf);
    }

    out.push_str("    add.u32 %r2, %r2, 16;\n");
    out.push_str("    bra.uni KLOOP;\nKEND:\n");

    // Store C (f32-acc) / the f16-acc final store-only y pass (2026-09-11:
    // full-K register accumulation — one straight-line store pass after
    // the K loop, no in-loop predicate, no RMV reads).
    if f16_acc {
        emit_y_pass(&mut out, 2);
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

    /// f32-tier warp_mh A/B artifacts (2026-09-12): the dispatch-true
    /// configs — select_mw_nw + ptx_warp_mh exactly as the dispatcher
    /// computes them, so what is benched is what the dispatch emits
    /// (BUGS.md 2026-09-12 rule).
    #[test]
    fn dump_f32_warp_mh_ab() {
        for mh in [2usize, 4usize] {
            let (mw, nw) = crate::backend::ptx::select_mw_nw(4096, 4096, 256, mh);
            let ptx = tensor_gemm_ptx_smem_mw(
                4096, 4096, 4096, 0, 33554432, 67108872, 4, mw, nw, false, 4, mh,
            );
            std::fs::write(format!("/tmp/opencode/tgemm_f32_mh{mh}.ptx"), &ptx).unwrap();
        }
        // Stage-depth candidate (2026-09-13, plan 2026-09-13): f32 at
        // stages=2 halves smem to 16384 → 2 CTAs/SM at 128 regs (s4 runs
        // 1 CTA). Same dispatch-true tile as the mh2 arm above.
        let (mw, nw) = crate::backend::ptx::select_mw_nw(4096, 4096, 256, 2);
        let ptx = tensor_gemm_ptx_smem_mw(
            4096, 4096, 4096, 0, 33554432, 67108872, 4, mw, nw, false, 2, 2,
        );
        std::fs::write("/tmp/opencode/tgemm_f32_s2.ptx", &ptx).unwrap();
    }

    /// warp_mh=4 portfolio (2026-09-12): the shapes the f16acc dispatch
    /// now emits (select_mw_nw lands (4,4) for 2048³/4096³/8192³ at mhr=4),
    /// for on-device correctness at the production warp shape.
    #[test]
    fn dump_mh4_f16acc_portfolio() {
        for (m, k, tag) in [(2048i64, 2048i64, "2048"), (4096, 4096, "4096"), (8192, 8192, "8192")] {
            let b_off = m * k * 2;
            let y_off = ((b_off + m * k * 2 + 7) & !7) + 8;
            let ptx = tensor_gemm_ptx_smem_mw(
                m, m, k, 0, b_off as u64, y_off as u64, 2, 4, 4, true, 2, 4,
            );
            std::fs::write(&format!("/tmp/opencode/tgemm_mh4_{tag}.ptx"), &ptx).unwrap();
        }
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 16, 0, 131072, 262152, 2, 4, 4, true, 2, 4);
        std::fs::write("/tmp/opencode/tgemm_mh4_k16.ptx", &ptx).unwrap();
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

    /// E5a (2026-09-13): cross-kstep B-fragment lookahead — mma consumes a
    /// register B set prefetched across the previous iteration's barrier.
    /// Same (2,4)@256T pairing as E4c; +2*gr B regs (72 → 3 CTAs/SM).
    #[test]
    fn dump_e5a_b_lookahead() {
        for (m, k, tag) in [
            (2048i64, 2048i64, "2048"),
            (4096, 4096, "4096"),
            (8192, 8192, "8192"),
            (4096, 16, "k16"),
        ] {
            let b_off = m * k * 2;
            let y_off = ((b_off + m * k * 2 + 7) & !7) + 8;
            let ptx = tensor_gemm_ptx_smem_mw_opt(
                m, m, k, 0, b_off as u64, y_off as u64, 2, 2, 4, true, 2, 4, true,
            );
            std::fs::write(&format!("/tmp/opencode/tgemm_e5a_{tag}.ptx"), &ptx).unwrap();
        }
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
        // (2,4)@256T stages-2: the post-pipeline-fix occupancy candidate —
        // 64 regs x 256T fits 4 CTAs/SM (plan 2026-09-11 probe round 2).
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 4096, 0, 33554432, 67108872, 2, 2, 4, true, 2, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_4096_f16acc_24s2.ptx", &ptx).unwrap();
        // E4c (2026-09-13): the new f16acc pairing — (2,4)@256T warp_mh=4
        // (128x128 tile, 16KB smem, 4 CTAs/SM). select_mw_nw lands here for
        // every large square shape; these dumps + ptx_gemm_bench gate it.
        for (m, k, tag) in [
            (2048i64, 2048i64, "2048"),
            (4096, 4096, "4096"),
            (8192, 8192, "8192"),
            (4096, 16, "k16"),
        ] {
            let b_off = m * k * 2;
            let y_off = ((b_off + m * k * 2 + 7) & !7) + 8;
            let ptx = tensor_gemm_ptx_smem_mw(m, m, k, 0, b_off as u64, y_off as u64, 2, 2, 4, true, 2, 4);
            std::fs::write(&format!("/tmp/opencode/tgemm_mw2_mh4_{tag}.ptx"), &ptx).unwrap();
        }
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

    /// E1 microbench (plan 2026-09-11-ptx-mma-issue-ceiling): pure mma
    /// issue rate. N independent f16x2 accumulator chains per warp, register
    /// operands, no smem/fills — isolates the tensor-core issue+dependency
    /// ceiling from everything else. chains 4..64: does the ISA need 32
    /// chains (CUTLASS 64x64 warp tiles) to reach the dense f16-acc peak,
    /// or is our 16-chain schedule already at it?
    #[test]
    fn dump_mma_microbench() {
        for &chains in &[4usize, 8, 16, 32, 64] {
            let mut out = String::new();
            out.push_str("// E1 pure-mma issue microbench\n");
            out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n\n");
            out.push_str(".visible .entry main (.param .b64 proj_param) {\n");
            out.push_str("    .reg .b32 %r<8>;\n    .reg .b64 %rd<8>;\n    .reg .pred %p<2>;\n");
            out.push_str(&format!("    .reg .b32 %c<{}>;\n", chains * 2 + 1));
            out.push_str("    .reg .b32 %a<5>;\n    .reg .b32 %b<3>;\n");
            // tid/lane-derived operands: non-zero, chain-independent.
            out.push_str("    mov.u32 %r1, %tid.x;\n");
            out.push_str("    mov.u32 %r2, %ctaid.x;\n");
            out.push_str("    add.u32 %r2, %r2, 1;\n");
            out.push_str("    and.b32 %r3, %r1, 31;\n");
            out.push_str("    add.u32 %a0, %r3, %r2;\n");     // a0: lane+cta+1
            out.push_str("    add.u32 %a1, %a0, 17;\n");
            out.push_str("    add.u32 %a2, %a0, 34;\n");
            out.push_str("    add.u32 %a3, %a0, 51;\n");
            out.push_str("    add.u32 %b0, %a0, 7;\n");
            out.push_str("    add.u32 %b1, %a0, 13;\n");
            // zero all accumulator chains
            for i in 0..chains * 2 {
                out.push_str(&format!("    mov.u32 %c{}, 0;\n", i));
            }
            // loop: R iterations of the chain block. R chosen so total
            // FLOP ≈ 130 GFLOP: R = 130e9 / (chains*4096*warps), warps=448.
            let warps = 448u64;
            let iters = (130_000_000_000f64 / (chains as f64 * 4096.0 * warps as f64)).round() as u64;
            let iters = (iters / 8 * 8).max(64); // multiple of 8, sane floor
            out.push_str("    mov.u32 %r4, %ntid.x;\n");
            out.push_str("    mul.lo.u32 %r4, %r4, %r2;\n");
            out.push_str("    add.u32 %r5, %r1, %r4;\n"); // global thread id (diag sweep fodder)
            out.push_str(&format!("    mov.u32 %r6, {};\n", iters));
            out.push_str("LOOP:\n");
            for i in 0..chains {
                out.push_str(&format!(
                    "    mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {{%c{}, %c{}}}, {{%a0, %a1, %a2, %a3}}, {{%b0, %b1}}, {{%c{}, %c{}}};\n",
                    2 * i, 2 * i + 1, 2 * i, 2 * i + 1
                ));
            }
            out.push_str("    add.u32 %r6, %r6, -1;\n");
            out.push_str("    setp.ne.u32 %p1, %r6, 0;\n");
            out.push_str("    @%p1 bra LOOP;\n");
            // DCE guard: fold the first chain into a single global store.
            // This also caps the f16 rounding drift — the microbench
            // measures throughput, not numerics.
            out.push_str("    ld.param.u64 %rd1, [proj_param];\n");
            out.push_str("    cvta.to.global.u64 %rd1, %rd1;\n");
            out.push_str("    mul.wide.u32 %rd2, %r5, 4;\n");
            out.push_str("    add.u64 %rd2, %rd1, %rd2;\n");
            // fold EVERY chain into the store — an mma whose accumulator
            // is never read is side-effect-free and ptxas deletes it
            // (14-reg "ceiling" artifacts otherwise).
            out.push_str("    add.u32 %r7, %c0, %c1;\n");
            for i in 1..chains {
                out.push_str(&format!("    add.u32 %r7, %r7, %c{};\n", 2 * i));
                out.push_str(&format!("    add.u32 %r7, %r7, %c{};\n", 2 * i + 1));
            }
            out.push_str("    st.global.b32 [%rd2], %r7;\n");
            out.push_str("    ret;\n}\n");
            let path = format!("/tmp/opencode/mb_chains_{}.ptx", chains);
            std::fs::write(&path, &out).unwrap();
        }
    }
    /// E1b/E1c (plan 2026-09-11-ptx-mma-issue-ceiling): decompose the 2.3x
    /// between the pure-issue ceiling (52.8 TF) and the shipped kernel
    /// (21.0). E1b = ldmatrix(x4 A + 8x x2.trans B) + 16 mma per kstep from
    /// FIXED hoisted addresses — the serial-order mainloop best case, no
    /// address math. E1c = E1b + the real kernel's ~150 support-ALU ops per
    /// kstep replayed on a live register chain. The gap E1→E1b isolates
    /// exposed ldmatrix latency; E1b→E1c isolates the support-math issue
    /// load. smem: 4KB dynamic (two 512B A slabs, eight 256B B slabs).
    #[test]
    fn dump_mma_mix_microbench() {
        for &variant in &["b", "c", "d", "f", "g"] {
            let chains = 16usize; // the shipped schedule's chain count
            let mut out = String::new();
            out.push_str(&format!("// E1{} ldmatrix+mma mix microbench\n", variant));
            out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n\n");
            out.push_str("    .extern .shared .align 16 .b8 dsmem[];\n");
            out.push_str(".visible .entry main (.param .b64 proj_param) {\n");
            out.push_str("    .reg .b32 %r<16>;\n    .reg .b64 %rd<16>;\n    .reg .pred %p<2>;\n");
            out.push_str("    .reg .b32 %c<33>;\n    .reg .b32 %a<5>;\n    .reg .b32 %b<17>;\n");
            out.push_str("    ld.param.u64 %rd1, [proj_param];\n");
            out.push_str("    cvta.to.global.u64 %rd1, %rd1;\n");
            out.push_str("    mov.u32 %r1, %tid.x;\n    mov.u32 %r2, %ctaid.x;\n");
            out.push_str("    and.b32 %r3, %r1, 31;\n    add.u32 %r2, %r2, 1;\n");
            // dynamic smem base (shared-window u32, widened for u64 math)
            out.push_str("    .reg .b32 %sas<1>;\n    .reg .b64 %rdS<1>;\n");
            out.push_str("    .reg .b64 %rdA;\n    .reg .b64 %rdB0, %rdB1, %rdB2, %rdB3, %rdB4, %rdB5, %rdB6, %rdB7;\n");
            out.push_str("    mov.u32 %sas0, dsmem;\n");
            out.push_str("    cvt.u64.u32 %rdS0, %sas0;\n");
            // A slab lane addressing (conflict-free, matches the kernel):
            // row r = (l>>4)*8 + (l&7), addr = r*32 + ((l>>3)&1)*16
            out.push_str("    shr.u32 %r4, %r3, 4;\n");          // l>>4
            out.push_str("    mul.lo.u32 %r4, %r4, 8;\n");
            out.push_str("    and.b32 %r5, %r3, 7;\n");
            out.push_str("    add.u32 %r4, %r4, %r5;\n");        // r
            out.push_str("    mul.lo.u32 %r4, %r4, 32;\n");
            out.push_str("    shr.u32 %r5, %r3, 3;\n    and.b32 %r5, %r5, 1;\n");
            out.push_str("    mul.lo.u32 %r5, %r5, 16;\n");
            out.push_str("    add.u32 %r4, %r4, %r5;\n");
            out.push_str("    mul.wide.u32 %rd4, %r4, 1;\n");
            out.push_str("    add.u64 %rdA, %rdS0, %rd4;\n");
            // B slab: 8 groups at +g*256, lane addr ((l>>3)&1)*128 + ((l&7)^((l>>3)&1))*16
            out.push_str("    shr.u32 %r5, %r3, 3;\n    and.b32 %r5, %r5, 1;\n");
            out.push_str("    mul.lo.u32 %r6, %r5, 128;\n");
            out.push_str("    and.b32 %r7, %r3, 7;\n    xor.b32 %r7, %r7, %r5;\n");
            out.push_str("    mul.lo.u32 %r7, %r7, 16;\n");
            out.push_str("    add.u32 %r6, %r6, %r7;\n");
            // pre-hoist all 8 B lane addresses + the A pair addresses
            for g in 0..8 {
                out.push_str(&format!("    mul.wide.u32 %rd6, %r6, 1;\n"));
                out.push_str(&format!("    add.u64 %rdB{}, %rdS0, %rd6;\n", g));
                out.push_str(&format!("    add.u64 %rdB{}, %rdB{}, {};\n", g, g, 512 + g * 256));
            }
            // zero the 16 acc chains
            for i in 0..chains * 2 {
                out.push_str(&format!("    mov.u32 %c{}, 0;\n", i));
            }
            // operand seeds for the mma (register-held)
            out.push_str("    add.u32 %a0, %r3, %r2;\n");
            out.push_str("    mov.u32 %b0, 7;\n    mov.u32 %b1, 13;\n");
            let warps = 448u64;
            let iters = (130_000_000_000f64 / (chains as f64 * 4096.0 * warps as f64)).round() as u64;
            let iters = (iters / 8 * 8).max(64);
            out.push_str(&format!("    mov.u32 %r8, {};\n", iters));
            out.push_str("LOOP:\n");
            // A fragment (x4) + fragment swap, then 8 B fragments (x2.trans)
            out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a0, %a1, %a2, %a3}, [%rdA];\n");
            out.push_str("    mov.b32 %r9, %a1; mov.b32 %a1, %a2; mov.b32 %a2, %r9;\n");
            for g in 0..8 {
                out.push_str(&format!(
                    "    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {{%b{}, %b{}}}, [%rdB{}];\n",
                    2 * g, 2 * g + 1, g % 8
                ));
            }
            if variant == "c" {
                // ~150 dead ALU ops per kstep on a live chain (the shipped
                // kernel's per-fragment address recomputation magnitude)
                out.push_str("    add.u32 %r10, %r3, %r2;\n");
                for _ in 0..150 {
                    out.push_str("    add.u32 %r10, %r10, 3;\n");
                }
            }
            let streaming = variant == "f" || variant == "g";
            let wait_slack = if variant == "g" { 0 } else { 1 };
            if variant == "d" || variant == "f" || variant == "g" {
                // E1d: + the real kernel's fill/wait/barrier rhythm — 6
                // cp.async.ca 4B per thread (12KB/CTA), commit, then after
                // the mma phase wait_group 1 + membar.cta + bar.sync.
                // E1f: the fill source STREAMS (iteration-scaled offset into
                // the 100MB state) instead of hammering one L2-resident
                // 24KB window — models the real kernel's DRAM traffic.
                // E1g: E1f with wait_group 0 — the shipped kernel's FULL
                // drain (stages=2 ⇒ stages-2=0); isolates the drain stall.
                // cp.async.ca 4B per thread (12KB/CTA), commit, then after
                // the mma phase wait_group 1 + membar.cta + bar.sync.
                // E1f: the fill source STREAMS (iteration-scaled offset into
                // the 100MB state) instead of hammering one L2-resident
                // 24KB window — models the real kernel's DRAM traffic.
                // iteration-scaled offset: %r8 (iter counter) * 12288, kept
                // inside the 96MB state via masking with the iteration
                // stride folded in — streaming, not L2-resident.
                if streaming {
                    out.push_str("    mul.lo.u32 %r11, %r8, 6144;\n");
                    out.push_str("    and.b32 %r11, %r11, 25161728;\n");
                    out.push_str("    mul.wide.u32 %rd9, %r11, 1;\n");
                    out.push_str("    add.u64 %rd10, %rd1, %rd9;\n");
                }
                for j in 0..6 {
                    out.push_str("    mul.wide.u32 %rd8, %r3, 4;\n");
                    if streaming {
                        out.push_str("    add.u64 %rd8, %rd10, %rd8;\n");
                    } else {
                        out.push_str("    add.u64 %rd8, %rd1, %rd8;\n");
                    }
                    out.push_str(&format!("    add.u64 %rd8, %rd8, {};\n", j * 2048));
                    out.push_str(&format!(
                        "    cp.async.ca.shared.global [%rdB{}], [%rd8], 4;\n",
                        j
                    ));
                }
                out.push_str("    cp.async.commit_group;\n");
            }
            for i in 0..chains {
                out.push_str(&format!(
                    "    mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {{%c{}, %c{}}}, {{%a0, %a1, %a2, %a3}}, {{%b0, %b1}}, {{%c{}, %c{}}};\n",
                    2 * i, 2 * i + 1, 2 * i, 2 * i + 1
                ));
            }
            if variant == "d" || streaming {
                out.push_str(&format!("    cp.async.wait_group {};\n", wait_slack));
                out.push_str("    membar.cta;\n");
                out.push_str("    bar.sync 0;\n");
            }
            out.push_str("    add.u32 %r8, %r8, -1;\n");
            out.push_str("    setp.ne.u32 %p1, %r8, 0;\n");
            out.push_str("    @%p1 bra LOOP;\n");
            // DCE guard: fold every chain + the ALU chain
            out.push_str("    mul.wide.u32 %rd2, %r1, 4;\n");
            out.push_str("    add.u64 %rd2, %rd1, %rd2;\n");
            out.push_str("    add.u32 %r7, %c0, %c1;\n");
            for i in 1..chains {
                out.push_str(&format!("    add.u32 %r7, %r7, %c{};\n", 2 * i));
                out.push_str(&format!("    add.u32 %r7, %r7, %c{};\n", 2 * i + 1));
            }
            if variant == "c" {
                out.push_str("    add.u32 %r7, %r7, %r10;\n");
            }
            out.push_str("    st.global.b32 [%rd2], %r7;\n");
            out.push_str("    ret;\n}\n");
            let path = format!("/tmp/opencode/mb_mix_{}.ptx", variant);
            std::fs::write(&path, &out).unwrap();
        }
    }

    /// Fill-gap diagnosis (plan 2026-09-11-ptx-mma-issue-ceiling, §Fill-gap):
    /// isolate the 29.3 → ~41 TF gap between the shipped kernel and E1f.
    ///
    /// Variants:
    ///   e = E1f baseline (clean fill + ldmatrix + mma) — should reproduce ~41 TF
    ///   s = swizzled fill + ldmatrix + mma — isolates the XOR swizzle
    ///   f = clean fill only (no ldmatrix/mma in loop) — fill throughput
    ///   m = mma only from pre-filled smem — mma-from-smem throughput
    ///   d = double fill (2× B fill + ldmatrix + mma) — tests fill throughput wall
    ///   p = pipeline overlap (double-buffered: fill buf(i+1) while mma buf(i))
    ///   a = A fill + B fill + ldmatrix + mma — full kernel fill pattern
    ///   w = A fill only (no B fill) — isolates A fill overhead
    ///   i = interleave A+B fills (alternating groups)
    ///   t = 3-stage pipeline (triple-buffered: fill buf(i+1) while mma buf(i))
    ///   c = widened A fill (16-byte ld+st instead of 4-byte)
    ///   8 = widened A fill (8-byte ld+st instead of 4-byte)
    #[test]
    fn dump_mma_fill_microbench() {
        let chains = 16usize;
        let warps = 448u64;
        let iters = (130_000_000_000f64 / (chains as f64 * 4096.0 * warps as f64)).round() as u64;
        let iters = (iters / 8 * 8).max(64);

        for &variant in &["e", "s", "f", "m", "d", "p", "a", "w", "i", "t", "c", "8", "sp"] {
            let mut out = String::new();
            let label = match variant {
                "e" => "E1f baseline (clean fill + ldmatrix + mma)",
                "s" => "swizzled fill + ldmatrix + mma",
                "f" => "clean fill only (no mma in loop)",
                "m" => "mma only from pre-filled smem",
                "d" => "double fill (2x B fill + ldmatrix + mma)",
                "p" => "pipeline overlap (fill buf(i+1) while mma buf(i))",
                "a" => "A fill + B fill + ldmatrix + mma (full kernel pattern)",
                "w" => "A fill only (no B fill) — isolates A fill overhead",
                "i" => "interleave A+B fills (alternating groups)",
                "t" => "3-stage pipeline (triple-buffered)",
                "c" => "widened A fill (16-byte ld+st)",
                "8" => "widened A fill (8-byte ld+st)",
                "sp" => "warp-split fills (A warps 0-7, B warps 8-15, both 16B)",
                _ => unreachable!(),
            };
            out.push_str(&format!("// Fill-gap diagnosis: {}\n", label));
            out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n\n");
            if variant == "p" {
                out.push_str("    .extern .shared .align 16 .b8 dsmem[4096];\n");
            } else if variant == "t" {
                out.push_str("    .extern .shared .align 16 .b8 dsmem[6144];\n");
            } else {
                out.push_str("    .extern .shared .align 16 .b8 dsmem[];\n");
            }
            out.push_str(".visible .entry main (.param .b64 proj_param) {\n");
            // All register declarations at the top
            out.push_str("    .reg .b32 %r<16>;\n");
            out.push_str("    .reg .b64 %rd<16>;\n");
            out.push_str("    .reg .pred %p<3>;\n");
            out.push_str("    .reg .b32 %c<33>;\n");
            out.push_str("    .reg .b32 %a<5>;\n");
            out.push_str("    .reg .b32 %b<17>;\n");
            out.push_str("    .reg .b32 %sas<1>;\n");
            out.push_str("    .reg .b64 %rdS<1>;\n");
            out.push_str("    .reg .b64 %rdA;\n");
            // Load state pointer
            out.push_str("    ld.param.u64 %rd1, [proj_param];\n");
            out.push_str("    cvta.to.global.u64 %rd1, %rd1;\n");
            out.push_str("    mov.u32 %r1, %tid.x;\n");
            out.push_str("    mov.u32 %r2, %ctaid.x;\n");
            out.push_str("    and.b32 %r3, %r1, 31;\n");
            out.push_str("    add.u32 %r2, %r2, 1;\n");
            // smem base
            out.push_str("    mov.u32 %sas0, dsmem;\n");
            out.push_str("    cvt.u64.u32 %rdS0, %sas0;\n");
            // A lane addressing: row = (lane>>4)*8 + (lane&7), col-block = (lane>>3)&1
            out.push_str("    shr.u32 %r4, %r3, 4;\n");
            out.push_str("    mul.lo.u32 %r4, %r4, 8;\n");
            out.push_str("    and.b32 %r5, %r3, 7;\n");
            out.push_str("    add.u32 %r4, %r4, %r5;\n");
            out.push_str("    mul.lo.u32 %r4, %r4, 32;\n");
            out.push_str("    shr.u32 %r5, %r3, 3;\n");
            out.push_str("    and.b32 %r5, %r5, 1;\n");
            out.push_str("    mul.lo.u32 %r5, %r5, 16;\n");
            out.push_str("    add.u32 %r4, %r4, %r5;\n");
            out.push_str("    mul.wide.u32 %rd4, %r4, 1;\n");
            out.push_str("    add.u64 %rdA, %rdS0, %rd4;\n");
            // B lane offsets: r8 = clean, r9 = swizzled
            // ng = (lane>>3)&1
            out.push_str("    shr.u32 %r5, %r3, 3;\n");
            out.push_str("    and.b32 %r5, %r5, 1;\n");
            out.push_str("    mul.lo.u32 %r6, %r5, 128;\n"); // r6 = ng*128
            out.push_str("    and.b32 %r7, %r3, 7;\n");
            out.push_str("    mul.lo.u32 %r7, %r7, 16;\n");
            out.push_str("    add.u32 %r8, %r6, %r7;\n"); // r8 = clean offset
            out.push_str("    and.b32 %r7, %r3, 7;\n");
            out.push_str("    xor.b32 %r7, %r7, %r5;\n");
            out.push_str("    mul.lo.u32 %r7, %r7, 16;\n");
            out.push_str("    add.u32 %r9, %r6, %r7;\n"); // r9 = swizzled offset
            // zero accs
            for i in 0..chains * 2 {
                out.push_str(&format!("    mov.u32 %c{}, 0;\n", i));
            }
            // operand seeds
            out.push_str("    add.u32 %a0, %r3, %r2;\n");
            out.push_str("    mov.u32 %b0, 7;\n");
            out.push_str("    mov.u32 %b1, 13;\n");
            // prefill smem (variant m: fill once before loop; variant p: fill buffer0; variant t: fill buffer0+1)
            if variant == "m" || variant == "p" || variant == "t" {
                for g in 0..8 {
                    out.push_str(&format!("    mov.u32 %r12, {};\n", g * 256));
                    out.push_str("    add.u32 %r12, %r12, %r8;\n");
                    out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                    out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                    out.push_str("    ld.global.b32 %r10, [%rd1];\n");
                    out.push_str("    st.shared.b32 [%rd5], %r10;\n");
                }
                if variant == "t" {
                    // Also fill buffer1 (offset 2048)
                    for g in 0..8 {
                        out.push_str(&format!("    mov.u32 %r12, {};\n", g * 256 + 2048));
                        out.push_str("    add.u32 %r12, %r12, %r8;\n");
                        out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                        out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                        out.push_str("    ld.global.b32 %r10, [%rd1];\n");
                        out.push_str("    st.shared.b32 [%rd5], %r10;\n");
                    }
                }
                out.push_str("    bar.sync 0;\n");
            }
            out.push_str(&format!("    mov.u32 %r10, {};\n", iters));
            out.push_str("LOOP:\n");
            // ---- FILL PHASE ----
            if variant == "p" {
                // Pipeline overlap: fill buffer (iter+1)%2 while mma reads from buffer iter%2
                out.push_str("    and.b32 %r12, %r10, 1;\n");
                out.push_str("    xor.b32 %r12, %r12, 1;\n");
                out.push_str("    shl.b32 %r12, %r12, 11;\n"); // (parity^1) * 2048
                for g in 0..8 {
                    out.push_str(&format!("    mov.u32 %r13, {};\n", g * 256));
                    out.push_str("    add.u32 %r13, %r13, %r8;\n");
                    out.push_str("    add.u32 %r13, %r13, %r12;\n");
                    out.push_str("    mul.wide.u32 %rd5, %r13, 1;\n");
                    out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                    out.push_str("    ld.global.b32 %r11, [%rd1];\n");
                    out.push_str("    st.shared.b32 [%rd5], %r11;\n");
                }
            } else if variant == "t" {
                // 3-stage: fill buffer (iter%3 + 1) % 3
                // r10 counts DOWN from iters; stage = (iters - r10) % 3
                // fill_buf = ((iters - r10 + 1) % 3) * 2048
                // For simplicity: use r10 % 3 since iters is multiple of 8
                // If r10 % 3 == 0: fill buf 1; if 1: fill buf 2; if 2: fill buf 0
                out.push_str("    rem.u32 %r12, %r10, 3;\n");
                out.push_str("    add.u32 %r12, %r12, 1;\n");
                out.push_str("    rem.u32 %r12, %r12, 3;\n"); // r12 = (r10%3 + 1) % 3
                out.push_str("    shl.b32 %r12, %r12, 11;\n"); // r12 * 2048
                for g in 0..8 {
                    out.push_str(&format!("    mov.u32 %r13, {};\n", g * 256));
                    out.push_str("    add.u32 %r13, %r13, %r8;\n");
                    out.push_str("    add.u32 %r13, %r13, %r12;\n");
                    out.push_str("    mul.wide.u32 %rd5, %r13, 1;\n");
                    out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                    out.push_str("    ld.global.b32 %r11, [%rd1];\n");
                    out.push_str("    st.shared.b32 [%rd5], %r11;\n");
                }
            } else if variant == "sp" {
                // Warp-split fills: warps 0-7 (tid < 256) fill A with four
                // 16-byte v4 ops (64B per A-warp thread); warps 8-15 fill B
                // the same way. Same total bytes/thread as variant a, but no
                // thread touches both fills and every op is 16-byte.
                out.push_str("    setp.ge.u32 %p2, %r1, 256;\n");
                out.push_str("    @%p2 bra BFILL_SP;\n");
                for g in 0..4 {
                    out.push_str(&format!("    mov.u32 %r12, {};\n", g * 256));
                    out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                    out.push_str("    add.u64 %rd5, %rdA, %rd5;\n");
                    out.push_str("    ld.global.v4.b32 {%r11, %r13, %r14, %r15}, [%rd1];\n");
                    out.push_str("    st.shared.v4.b32 [%rd5], {%r11, %r13, %r14, %r15};\n");
                }
                out.push_str("    bra FILLDONE_SP;\n");
                out.push_str("BFILL_SP:\n");
                for g in 0..4 {
                    out.push_str(&format!("    mov.u32 %r12, {};\n", g * 256));
                    out.push_str("    add.u32 %r12, %r12, %r8;\n");
                    out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                    out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                    out.push_str("    ld.global.v4.b32 {%r11, %r13, %r14, %r15}, [%rd1];\n");
                    out.push_str("    st.shared.v4.b32 [%rd5], {%r11, %r13, %r14, %r15};\n");
                }
                out.push_str("FILLDONE_SP:\n");
            } else if variant == "a" || variant == "w" || variant == "i" || variant == "c" || variant == "8" {
                // A fill
                if variant == "c" || variant == "8" {
                    // Widened A fill: 16-byte (c) or 8-byte (8) ld+st. The
                    // ops per thread halve/quarter at the same 32B/thread and
                    // the stride scales to keep variant a's 512-byte
                    // footprint: c = 2 v4 ops at stride 256, 8 = 4 v2 ops at
                    // stride 128.
                    let (ops, reg_v, stride) = if variant == "c" {
                        (2, "v4", 256)
                    } else {
                        (4, "v2", 128)
                    };
                    for g in 0..ops {
                        out.push_str(&format!("    mov.u32 %r12, {};\n", g * stride));
                        out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                        out.push_str("    add.u64 %rd5, %rdA, %rd5;\n");
                        if reg_v == "v4" {
                            out.push_str("    ld.global.v4.b32 {%r11, %r13, %r14, %r15}, [%rd1];\n");
                            out.push_str("    st.shared.v4.b32 [%rd5], {%r11, %r13, %r14, %r15};\n");
                        } else {
                            out.push_str("    ld.global.v2.b32 {%r11, %r13}, [%rd1];\n");
                            out.push_str("    st.shared.v2.b32 [%rd5], {%r11, %r13};\n");
                        }
                    }
                } else {
                    // A fill: 8 stores to A smem region
                    for g in 0..8 {
                        out.push_str(&format!("    mov.u32 %r12, {};\n", g * 64));
                        out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                        out.push_str("    add.u64 %rd5, %rdA, %rd5;\n");
                        out.push_str("    ld.global.b32 %r11, [%rd1];\n");
                        out.push_str("    st.shared.b32 [%rd5], %r11;\n");
                    }
                }
                // B fill (skip for variant w)
                if variant != "w" {
                    for g in 0..8 {
                        out.push_str(&format!("    mov.u32 %r12, {};\n", g * 256));
                        out.push_str("    add.u32 %r12, %r12, %r8;\n");
                        out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                        out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                        out.push_str("    ld.global.b32 %r11, [%rd1];\n");
                        out.push_str("    st.shared.b32 [%rd5], %r11;\n");
                    }
                }
            } else if variant != "m" {
                // Variants e, s, f, d: B fill only
                let b_off = if variant == "s" { "%r9" } else { "%r8" };
                for g in 0..8 {
                    out.push_str(&format!("    mov.u32 %r12, {};\n", g * 256));
                    out.push_str(&format!("    add.u32 %r12, %r12, {};\n", b_off));
                    out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                    out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                    out.push_str("    ld.global.b32 %r11, [%rd1];\n");
                    out.push_str("    st.shared.b32 [%rd5], %r11;\n");
                }
                if variant == "d" {
                    for g in 0..8 {
                        out.push_str(&format!("    mov.u32 %r12, {};\n", g * 256));
                        out.push_str(&format!("    add.u32 %r12, %r12, {};\n", b_off));
                        out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                        out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                        out.push_str("    ld.global.b32 %r11, [%rd1];\n");
                        out.push_str("    st.shared.b32 [%rd5], %r11;\n");
                    }
                }
            }
            // ---- LDMATRIX + MMA PHASE ----
            if variant == "p" {
                // Pipeline: mma reads from buffer (parity) * 2048
                out.push_str("    and.b32 %r12, %r10, 1;\n");
                out.push_str("    shl.b32 %r12, %r12, 11;\n"); // r12 = parity * 2048
                out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a0, %a1, %a2, %a3}, [%rdA];\n");
                out.push_str("    mov.b32 %r11, %a1;\n");
                out.push_str("    mov.b32 %a1, %a2;\n");
                out.push_str("    mov.b32 %a2, %r11;\n");
                for g in 0..8 {
                    out.push_str(&format!("    mov.u32 %r13, {};\n", g * 256));
                    out.push_str("    add.u32 %r13, %r13, %r8;\n");
                    out.push_str("    add.u32 %r13, %r13, %r12;\n");
                    out.push_str("    mul.wide.u32 %rd5, %r13, 1;\n");
                    out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                    out.push_str(&format!(
                        "    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {{%b{}, %b{}}}, [%rd5];\n",
                        2 * g, 2 * g + 1
                    ));
                }
                for i in 0..chains {
                    let bg = i / 8;
                    out.push_str(&format!(
                        "    mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {{%c{}, %c{}}}, {{%a0, %a1, %a2, %a3}}, {{%b{}, %b{}}}, {{%c{}, %c{}}};\n",
                        2 * i, 2 * i + 1, 2 * bg, 2 * bg + 1, 2 * i, 2 * i + 1
                    ));
                }
            } else if variant == "t" {
                // 3-stage: mma reads from buffer (r10 % 3) * 2048
                out.push_str("    rem.u32 %r12, %r10, 3;\n");
                out.push_str("    shl.b32 %r12, %r12, 11;\n"); // r12 = (r10%3) * 2048
                out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a0, %a1, %a2, %a3}, [%rdA];\n");
                out.push_str("    mov.b32 %r11, %a1;\n");
                out.push_str("    mov.b32 %a1, %a2;\n");
                out.push_str("    mov.b32 %a2, %r11;\n");
                for g in 0..8 {
                    out.push_str(&format!("    mov.u32 %r13, {};\n", g * 256));
                    out.push_str("    add.u32 %r13, %r13, %r8;\n");
                    out.push_str("    add.u32 %r13, %r13, %r12;\n");
                    out.push_str("    mul.wide.u32 %rd5, %r13, 1;\n");
                    out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                    out.push_str(&format!(
                        "    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {{%b{}, %b{}}}, [%rd5];\n",
                        2 * g, 2 * g + 1
                    ));
                }
                for i in 0..chains {
                    let bg = i / 8;
                    out.push_str(&format!(
                        "    mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {{%c{}, %c{}}}, {{%a0, %a1, %a2, %a3}}, {{%b{}, %b{}}}, {{%c{}, %c{}}};\n",
                        2 * i, 2 * i + 1, 2 * bg, 2 * bg + 1, 2 * i, 2 * i + 1
                    ));
                }
            } else if variant != "f" {
                out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a0, %a1, %a2, %a3}, [%rdA];\n");
                out.push_str("    mov.b32 %r11, %a1;\n");
                out.push_str("    mov.b32 %a1, %a2;\n");
                out.push_str("    mov.b32 %a2, %r11;\n");
                let b_off = if variant == "s" { "%r9" } else { "%r8" };
                for g in 0..8 {
                    out.push_str(&format!("    mov.u32 %r12, {};\n", g * 256));
                    out.push_str(&format!("    add.u32 %r12, %r12, {};\n", b_off));
                    out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                    out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                    out.push_str(&format!(
                        "    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {{%b{}, %b{}}}, [%rd5];\n",
                        2 * g, 2 * g + 1
                    ));
                }
                for i in 0..chains {
                    let bg = i / 8;
                    out.push_str(&format!(
                        "    mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {{%c{}, %c{}}}, {{%a0, %a1, %a2, %a3}}, {{%b{}, %b{}}}, {{%c{}, %c{}}};\n",
                        2 * i, 2 * i + 1, 2 * bg, 2 * bg + 1, 2 * i, 2 * i + 1
                    ));
                }
            }
            out.push_str("    bar.sync 0;\n");
            out.push_str("    add.u32 %r10, %r10, -1;\n");
            out.push_str("    setp.ne.u32 %p1, %r10, 0;\n");
            out.push_str("    @%p1 bra LOOP;\n");
            // DCE guard
            out.push_str("    mul.wide.u32 %rd2, %r1, 4;\n");
            out.push_str("    add.u64 %rd2, %rd1, %rd2;\n");
            out.push_str("    add.u32 %r11, %c0, %c1;\n");
            for i in 1..chains {
                out.push_str(&format!("    add.u32 %r11, %r11, %c{};\n", 2 * i));
                out.push_str(&format!("    add.u32 %r11, %r11, %c{};\n", 2 * i + 1));
            }
            out.push_str("    st.global.b32 [%rd2], %r11;\n");
            out.push_str("    ret;\n}\n");
            let path = format!("/tmp/opencode/mb_fill_{}.ptx", variant);
            std::fs::write(&path, &out).unwrap();
        }
    }

    /// DRAM-real fill microbench (2026-09-13, plan 2026-09-13): the sync
    /// fill microbenches above read one broadcast address — zero DRAM
    /// traffic — so they model smem/instruction cost only (the E1 lesson).
    /// These variants use the REAL per-CTA tile addressing and the REAL
    /// fill byte patterns at production geometry (warp_mh=4 (4,4)@512T,
    /// 256x128 CTA tiles, 512 CTAs, 256 k-stripes of 16): A = one 16-byte
    /// cp.async.cg per thread (D = tid*16; global row = D>>5 of the CTA's
    /// 256 rows, col = D&31, src = a_base + row*8192 + col + stripe*32),
    /// B = one 8-byte cp.async.ca (D = tid*8; global k = stripe*16 + D>>8,
    /// col = D&255, src = b_base + k*8192 + col — bijective over the CTA's
    /// 256-byte B col span). Consumer is a LIGHT ldmatrix+xor — documented
    /// caveat: the real kernel's mma pressure is absent, so absolute TF
    /// sits above production; the decomposition across variants is the
    /// measurement.
    ///
    /// Variants:
    ///   n = fills only (no consumer) — pure DRAM fill ceiling
    ///   a = A stream only
    ///   b = B stream only
    ///   f = full (A + B fills + consumer) — calibrate against production
    #[test]
    fn dump_mma_dram_microbench() {
        for &variant in &["n", "a", "b", "f"] {
            let fill_a = variant != "b";
            let fill_b = variant != "a";
            let consume = variant == "f";
            let mut out = String::new();
            out.push_str(&format!("// DRAM-real fill microbench: {variant}\n"));
            out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n\n");
            out.push_str("    .extern .shared .align 16 .b8 dsmem[];\n");
            out.push_str(".visible .entry main (.param .b64 proj_param) {\n");
            out.push_str("    .reg .b32 %r<24>;\n");
            out.push_str("    .reg .b64 %rd<8>;\n");
            out.push_str("    .reg .pred %p<2>;\n");
            out.push_str("    .reg .b32 %c<3>;\n");
            out.push_str("    .reg .b32 %a<5>;\n");
            out.push_str("    .reg .b32 %b<3>;\n");
            out.push_str("    .reg .b32 %sas<1>;\n");
            out.push_str("    .reg .b64 %rdS<1>;\n");
            out.push_str("    .reg .b64 %rdA;\n");
            out.push_str("    ld.param.u64 %rd1, [proj_param];\n");
            out.push_str("    cvta.to.global.u64 %rd1, %rd1;\n");
            out.push_str("    mov.u32 %r1, %tid.x;\n");
            out.push_str("    mov.u32 %r2, %ctaid.x;\n");
            // cta_m = ctaid >> 5 (16 m-tiles of 256 rows), cta_n = ctaid & 31
            out.push_str("    shr.u32 %r3, %r2, 5;\n");
            out.push_str("    and.b32 %r4, %r2, 31;\n");
            // a_base = state + cta_m*256*a_row (a_row = 8192)
            out.push_str("    mul.lo.u32 %r5, %r3, 2097152;\n");
            out.push_str("    mul.wide.u32 %rd2, %r5, 1;\n");
            out.push_str("    add.u64 %rd2, %rd1, %rd2;\n");
            // b_base = state + 33554432 + cta_n*256
            out.push_str("    mul.lo.u32 %r5, %r4, 256;\n");
            out.push_str("    mul.wide.u32 %rd3, %r5, 1;\n");
            out.push_str("    mov.u64 %rd4, 33554432;\n");
            out.push_str("    add.u64 %rd4, %rd1, %rd4;\n");
            out.push_str("    add.u64 %rd3, %rd4, %rd3;\n");
            // smem base + A ldmatrix lane offsets (real kernel's formula)
            out.push_str("    mov.u32 %r21, dsmem;\n");
            out.push_str("    mov.u32 %sas0, dsmem;\n");
            out.push_str("    cvt.u64.u32 %rdS0, %sas0;\n");
            out.push_str("    and.b32 %r6, %r1, 31;\n");
            out.push_str("    shr.u32 %r7, %r6, 4;\n");
            out.push_str("    mul.lo.u32 %r7, %r7, 8;\n");
            out.push_str("    and.b32 %r8, %r6, 7;\n");
            out.push_str("    add.u32 %r7, %r7, %r8;\n");
            out.push_str("    mul.lo.u32 %r7, %r7, 32;\n");
            out.push_str("    shr.u32 %r8, %r6, 3;\n");
            out.push_str("    and.b32 %r8, %r8, 1;\n");
            out.push_str("    mul.lo.u32 %r8, %r8, 16;\n");
            out.push_str("    add.u32 %r7, %r7, %r8;\n");
            out.push_str("    mul.wide.u32 %rd5, %r7, 1;\n");
            out.push_str("    add.u64 %rdA, %rdS0, %rd5;\n");
            // B consume lane offsets: (lane&31)>>3 * 1024 + (lane&7)*16
            out.push_str("    and.b32 %r8, %r1, 31;\n");
            out.push_str("    shr.u32 %r9, %r8, 3;\n");
            out.push_str("    mul.lo.u32 %r9, %r9, 1024;\n");
            out.push_str("    and.b32 %r8, %r8, 7;\n");
            out.push_str("    mul.lo.u32 %r8, %r8, 16;\n");
            out.push_str("    add.u32 %r8, %r8, %r9;\n");
            out.push_str("    mov.u32 %c0, 0;\n");
            out.push_str("    mov.u32 %c1, 0;\n");
            // Prologue: fill stage 0 (the streams this variant owns), one
            // commit group — mirrors the real kernel's prologue.
            {
                let s_off_a = "0";
                let s_off_b = "16384";
                if fill_a {
                    out.push_str(&format!("    mov.u32 %r12, {s_off_a};\n"));
                    out.push_str("    mul.lo.u32 %r13, %r1, 16;\n");
                    out.push_str("    add.u32 %r16, %r12, %r13;\n");
                    out.push_str("    shr.u32 %r14, %r13, 5;\n");
                    out.push_str("    and.b32 %r15, %r13, 31;\n");
                    out.push_str("    shl.b32 %r17, %r14, 13;\n");
                    out.push_str("    mov.u32 %r18, %r15;\n");
                    out.push_str("    add.u32 %r18, %r18, %r17;\n");
                    out.push_str("    mul.wide.u32 %rd5, %r18, 1;\n");
                    out.push_str("    add.u64 %rd5, %rd2, %rd5;\n");
                    out.push_str("    mov.u32 %r19, %r21;\n");
                    out.push_str("    add.u32 %r19, %r19, %r16;\n");
                    out.push_str("    cp.async.cg.shared.global [%r19], [%rd5], 16;\n");
                }
                if fill_b {
                    out.push_str(&format!("    mov.u32 %r12, {s_off_b};\n"));
                    out.push_str("    mul.lo.u32 %r13, %r1, 8;\n");
                    out.push_str("    add.u32 %r16, %r12, %r13;\n");
                    out.push_str("    shr.u32 %r14, %r13, 8;\n");
                    out.push_str("    and.b32 %r15, %r13, 255;\n");
                    out.push_str("    shl.b32 %r18, %r14, 13;\n");
                    out.push_str("    add.u32 %r18, %r18, %r15;\n");
                    out.push_str("    mul.wide.u32 %rd5, %r18, 1;\n");
                    out.push_str("    add.u64 %rd5, %rd3, %rd5;\n");
                    out.push_str("    mov.u32 %r19, %r21;\n");
                    out.push_str("    add.u32 %r19, %r19, %r16;\n");
                    out.push_str("    cp.async.ca.shared.global [%r19], [%rd5], 8;\n");
                }
                out.push_str("    cp.async.commit_group;\n");
            }
            // K-loop over stripes 1..256: fill stage s^1 (async) while
            // consuming stage s — the real kernel's overlap structure.
            // wait_group 1 completes the PREVIOUS iteration's fill (the
            // stage being consumed); bar makes all threads' fills visible.
            out.push_str("    mov.u32 %r10, 1;\n");
            out.push_str("KLOOP:\n");
            out.push_str("    setp.ge.u32 %p1, %r10, 256;\n");
            out.push_str("    @%p1 bra KEND;\n");
            out.push_str("    and.b32 %r11, %r10, 1;\n");
            out.push_str("    xor.b32 %r20, %r11, 1;\n");
            if fill_a {
                // fill stage s^1: dst = dsmem + (s^1)*8192 + tid*16; src =
                // a_base + row*8192 + col + stripe*32
                out.push_str("    shl.b32 %r12, %r20, 13;\n");
                out.push_str("    mul.lo.u32 %r13, %r1, 16;\n");
                out.push_str("    add.u32 %r16, %r12, %r13;\n");
                out.push_str("    shr.u32 %r14, %r13, 5;\n");
                out.push_str("    and.b32 %r15, %r13, 31;\n");
                out.push_str("    shl.b32 %r17, %r14, 13;\n");
                out.push_str("    shl.b32 %r18, %r10, 5;\n");
                out.push_str("    add.u32 %r18, %r18, %r15;\n");
                out.push_str("    add.u32 %r18, %r18, %r17;\n");
                out.push_str("    mul.wide.u32 %rd5, %r18, 1;\n");
                out.push_str("    add.u64 %rd5, %rd2, %rd5;\n");
                out.push_str("    mov.u32 %r19, %r21;\n");
                out.push_str("    add.u32 %r19, %r19, %r16;\n");
                out.push_str("    cp.async.cg.shared.global [%r19], [%rd5], 16;\n");
            }
            if fill_b {
                // fill stage s^1: dst = dsmem + 16384 + (s^1)*4096 + tid*8;
                // src = b_base + (stripe*16 + D>>8)*8192 + D&255
                out.push_str("    shl.b32 %r12, %r20, 12;\n");
                out.push_str("    add.u32 %r12, %r12, 16384;\n");
                out.push_str("    mul.lo.u32 %r13, %r1, 8;\n");
                out.push_str("    add.u32 %r16, %r12, %r13;\n");
                out.push_str("    shr.u32 %r14, %r13, 8;\n");
                out.push_str("    and.b32 %r15, %r13, 255;\n");
                out.push_str("    shl.b32 %r17, %r10, 4;\n");
                out.push_str("    add.u32 %r17, %r17, %r14;\n");
                out.push_str("    shl.b32 %r18, %r17, 13;\n");
                out.push_str("    add.u32 %r18, %r18, %r15;\n");
                out.push_str("    mul.wide.u32 %rd5, %r18, 1;\n");
                out.push_str("    add.u64 %rd5, %rd3, %rd5;\n");
                out.push_str("    mov.u32 %r19, %r21;\n");
                out.push_str("    add.u32 %r19, %r19, %r16;\n");
                out.push_str("    cp.async.ca.shared.global [%r19], [%rd5], 8;\n");
            }
            out.push_str("    cp.async.commit_group;\n");
            if consume {
                // Complete the previous iteration's fill (stage r20), make
                // it visible, then read it.
                out.push_str("    cp.async.wait_group 1;\n");
                out.push_str("    membar.cta;\n");
                out.push_str("    bar.sync 0;\n");
                // A ldmatrix x4 + xor at rdA + (s^1)*8192
                out.push_str("    shl.b32 %r12, %r20, 13;\n");
                out.push_str("    mul.wide.u32 %rd5, %r12, 1;\n");
                out.push_str("    add.u64 %rd5, %rdA, %rd5;\n");
                out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a0, %a1, %a2, %a3}, [%rd5];\n");
                out.push_str("    xor.b32 %c0, %a0, %a1;\n");
                out.push_str("    xor.b32 %c0, %c0, %a2;\n");
                out.push_str("    xor.b32 %c0, %c0, %a3;\n");
                // B ldmatrix x2.trans x8 groups + xor at bsmem + (s^1)*4096
                out.push_str("    shl.b32 %r12, %r20, 12;\n");
                out.push_str("    add.u32 %r12, %r12, 16384;\n");
                for g in 0..8 {
                    out.push_str(&format!("    mov.u32 %r13, {};\n", g * 512));
                    out.push_str("    add.u32 %r13, %r13, %r8;\n");
                    out.push_str("    add.u32 %r13, %r13, %r12;\n");
                    out.push_str("    mul.wide.u32 %rd5, %r13, 1;\n");
                    out.push_str("    add.u64 %rd5, %rdS0, %rd5;\n");
                    out.push_str("    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {%b0, %b1}, [%rd5];\n");
                    out.push_str("    xor.b32 %c1, %c1, %b0;\n");
                    out.push_str("    xor.b32 %c1, %c1, %b1;\n");
                }
            } else {
                // No consumer: bound the async queue per iteration.
                out.push_str("    cp.async.wait_group 0;\n");
                out.push_str("    membar.cta;\n");
                out.push_str("    bar.sync 0;\n");
            }
            out.push_str("    bar.sync 0;\n");
            out.push_str("    add.u32 %r10, %r10, 1;\n");
            out.push_str("    bra KLOOP;\n");
            out.push_str("KEND:\n");
            // DCE guard: y[ctaid*512 + tid] = acc (y @ 67108872, 512T grid)
            out.push_str("    mul.lo.u32 %r13, %r2, 512;\n");
            out.push_str("    add.u32 %r13, %r13, %r1;\n");
            out.push_str("    mul.wide.u32 %rd5, %r13, 4;\n");
            out.push_str("    add.u64 %rd5, %rd1, %rd5;\n");
            out.push_str("    mov.u64 %rd6, 67108872;\n");
            out.push_str("    add.u64 %rd5, %rd5, %rd6;\n");
            out.push_str("    add.u32 %c0, %c0, %c1;\n");
            out.push_str("    st.global.b32 [%rd5], %c0;\n");
            out.push_str("    ret;\n}\n");
            let path = format!("/tmp/opencode/mb_dram_{variant}.ptx");
            std::fs::write(&path, &out).unwrap();
        }
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
