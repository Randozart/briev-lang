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
) -> String {
    debug_assert!(m % (32 * mw as i64) == 0 && n % (64 * nw as i64) == 0 && k % 16 == 0);
    let a_row = k * 2;
    let b_row = n * 2;
    let y_row = n * (y_elem as i64);
    let ng = 8;
    let threads = (mw * nw * 32) as i64;
    // Per-thread fill amounts (b32).
    let per_a = (mw as i64 * 256) / threads; // A tile b32 / threads
    let per_b = (nw as i64 * 512) / threads; // B tile b32 / threads
    let mut out = String::new();

    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n");
    out.push_str(".visible .entry main (.param .b64 proj_param)\n{\n");
    out.push_str("    .reg .b64  %rd1, %rd2, %rd3, %rd4, %rd5, %rd6, %rd7, %rd5b, %rd5c;\n");
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
    out.push_str(&format!("    .reg .f32  {};\n", cregs.join(", ")));
    out.push_str("    .reg .pred %p1;\n");
    out.push_str(&format!("    .shared .align 16 .b8 asmem[{}];\n", mw * 1024));
    out.push_str(&format!("    .shared .align 16 .b8 bsmem[{}];\n", nw * 2048));
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");

    // Block decode from ctaid.x: n/(nw*64) blocks per M-row.
    out.push_str("    mov.u32 %r1, %ctaid.x;\n");
    out.push_str(&format!("    mov.u32 %r2, {};\n", n / (64 * nw as i64)));
    out.push_str("    div.u32 %r3, %r1, %r2;  // m_cta\n");
    out.push_str("    rem.u32 %r4, %r1, %r2;  // n_cta\n");
    // Lane / warp decode.
    out.push_str("    mov.u32 %r5, %tid.x;\n");
    out.push_str(&format!("    setp.ge.u32 %p1, %r5, {};\n", threads));
    out.push_str("    @%p1 ret;\n");
    out.push_str("    shr.u32 %r9, %r5, 5;    // warp = tid/32\n");
    out.push_str(&format!("    mov.u32 %r2, {};\n", nw));
    out.push_str("    div.u32 %r10, %r9, %r2;  // mh_warp = warp/nw\n");
    out.push_str("    rem.u32 %r11, %r9, %r2;  // ng_warp = warp%nw\n");
    out.push_str("    and.b32 %r7, %r5, 31;\n"); // lane = tid%32
    out.push_str("    shr.u32 %r6, %r7, 2;    // g\n");
    out.push_str("    and.b32 %r8, %r7, 3;    // t\n");
    out.push_str("    shl.b32 %r8, %r8, 1;    // 2t\n");

    // rd2 = a_off + m_cta*(mw*32)*a_row
    out.push_str("    mov.u32 %r2, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r2, %r2, {};\n", 32 * mw as i64 * a_row));
    out.push_str("    mul.wide.u32 %rd4, %r2, 1;\n");
    out.push_str(&format!("    mov.u64 %rd2, {};\n", a_off));
    out.push_str("    add.u64 %rd2, %rd1, %rd2;\n");
    out.push_str("    add.u64 %rd2, %rd2, %rd4;\n");
    // rd3 = b_off + n_cta*(nw*64)*2
    out.push_str("    mov.u32 %r2, %r4;\n");
    out.push_str(&format!("    mul.lo.u32 %r2, %r2, {};\n", 128 * nw as i64));
    out.push_str("    mul.wide.u32 %rd4, %r2, 1;\n");
    out.push_str(&format!("    mov.u64 %rd3, {};\n", b_off));
    out.push_str("    add.u64 %rd3, %rd1, %rd3;\n");
    out.push_str("    add.u64 %rd3, %rd3, %rd4;\n");
    // rd6 = y_off + m_cta*(mw*32)*y_row + n_cta*(nw*64)*y_elem
    out.push_str("    mov.u32 %r2, %r3;\n");
    out.push_str(&format!("    mul.lo.u32 %r2, %r2, {};\n", 32 * mw as i64 * y_row));
    out.push_str("    mul.wide.u32 %rd4, %r2, 1;\n");
    out.push_str(&format!("    mov.u64 %rd6, {};\n", y_off));
    out.push_str("    add.u64 %rd6, %rd1, %rd6;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");
    out.push_str("    mov.u32 %r2, %r4;\n");
    out.push_str(&format!("    mul.lo.u32 %r2, %r2, {};\n", 64 * nw as i64 * (y_elem as i64)));
    out.push_str("    mul.wide.u32 %rd4, %r2, 1;\n");
    out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");

    // Zero accumulators.
    for i in 0..4 * 2 * ng {
        out.push_str(&format!("    mov.f32 %c{}, 0f00000000;\n", i));
    }

    // K loop.
    out.push_str("    mov.u32 %r2, 0;  // kstep\nKLOOP:\n");
    out.push_str(&format!("    setp.ge.u32 %p1, %r2, {};\n", k));
    out.push_str("    @%p1 bra KEND;\n");

    // Cooperative A fill: thread fills per_a b32 of the mw*32×16 tile.
    // Tile b32 idx = t*per_a + j; byte D = idx*4; row = D/32; chunk = (D%32)/4.
    out.push_str("    mov.u32 %r12, %r5;\n");
    out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", per_a as u32 * 4));
    for j in 0..per_a {
        out.push_str("    mov.u32 %r13, %r12;\n");
        out.push_str(&format!("    add.u32 %r13, %r13, {};\n", j * 4));
        out.push_str("    mov.u32 %r14, %r13;\n");
        out.push_str("    and.b32 %r15, %r13, 31;\n"); // col chunk byte within row
        out.push_str("    shr.u32 %r14, %r14, 5;\n"); // row = D/32
        out.push_str(&format!("    mul.lo.u32 %r14, %r14, {};\n", a_row));
        out.push_str("    add.u32 %r14, %r14, %r15;\n");
        out.push_str("    mov.u32 %r16, %r2;\n");
        out.push_str("    mul.lo.u32 %r16, %r16, 2;\n");
        out.push_str("    add.u32 %r14, %r14, %r16;\n"); // + kstep*2
        out.push_str("    mul.wide.u32 %rd4, %r14, 1;\n");
        out.push_str("    add.u64 %rd5, %rd2, %rd4;\n");
        out.push_str("    mov.u64 %rd4, asmem;\n");
        out.push_str("    mul.wide.u32 %rd5b, %r13, 1;\n");
        out.push_str("    add.u64 %rd4, %rd4, %rd5b;\n");
        out.push_str("    ld.global.b32 %t0, [%rd5]; st.shared.b32 [%rd4], %t0;\n");
    }

    // Cooperative B fill: thread fills per_b b32 of the nw*64-col blocked tile.
    // Tile b32 idx = t*per_b + j; byte D = idx*4.
    //   slice = D/2048; w = D%2048; block b = w/256; w2 = w%256;
    //   row r = w2/32; half = (w2%32)>=16; c2 = (w2%32)%16;
    //   B_row = r + 8*half; B_col = slice*64 + b*8 + c2%8.
    out.push_str("    mov.u32 %r12, %r5;\n");
    out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", per_b as u32 * 4));
    for j in 0..per_b {
        out.push_str("    mov.u32 %r13, %r12;\n");
        out.push_str(&format!("    add.u32 %r13, %r13, {};\n", j * 4));
        // slice = D/2048
        out.push_str("    mov.u32 %r14, %r13;\n");
        out.push_str("    shr.u32 %r14, %r14, 11;\n");
        // w = D%2048 ; block b = w/256 ; w2 = w%256
        out.push_str("    mov.u32 %r15, %r13;\n");
        out.push_str("    and.b32 %r15, %r15, 2047;\n");
        out.push_str("    shr.u32 %r16, %r15, 8;\n"); // b = w/256
        out.push_str("    and.b32 %r17, %r15, 255;\n"); // w2
        // r = w2/32
        out.push_str("    shr.u32 %r18, %r17, 5;\n");
        // c_dest = (w2%32)/2 (f16 col within the row); half = c_dest>=8
        out.push_str("    mov.u32 %r19, %r17;\n");
        out.push_str("    and.b32 %r19, %r19, 31;\n");
        out.push_str("    shr.u32 %r19, %r19, 1;\n");
        out.push_str("    setp.ge.u32 %p1, %r19, 8;\n");
        out.push_str("    mov.u32 %r20, 0;\n");
        out.push_str("    @%p1 mov.u32 %r20, 8;\n"); // +8 rows if half
        out.push_str("    add.u32 %r18, %r18, %r20;\n"); // B_row
        out.push_str("    and.b32 %r19, %r19, 7;\n"); // c_dest%8
        // global byte = rd3 + kstep*b_row + B_row*b_row + (slice*64 + b*8 + c2%8)*2
        // row terms are already bytes (b_row = N*2); only the column part needs *2.
        out.push_str("    mov.u32 %r20, %r2;\n");
        out.push_str(&format!("    mul.lo.u32 %r20, %r20, {};\n", b_row));
        out.push_str(&format!("    mul.lo.u32 %r15, %r18, {};\n", b_row));
        out.push_str("    add.u32 %r20, %r20, %r15;\n");
        // Column part in f16-element count, then *2 to bytes.
        out.push_str("    mul.lo.u32 %r15, %r14, 64;\n");
        out.push_str("    mul.lo.u32 %r17, %r16, 8;\n");
        out.push_str("    add.u32 %r15, %r15, %r17;\n");
        out.push_str("    add.u32 %r15, %r15, %r19;\n");
        out.push_str("    mul.lo.u32 %r15, %r15, 2;\n");
        out.push_str("    add.u32 %r20, %r20, %r15;\n");
        out.push_str("    mul.wide.u32 %rd4, %r20, 1;\n");
        out.push_str("    add.u64 %rd5, %rd3, %rd4;\n");
        // dest = bsmem + D
        out.push_str("    mov.u64 %rd4, bsmem;\n");
        out.push_str("    mul.wide.u32 %rd5b, %r13, 1;\n");
        out.push_str("    add.u64 %rd4, %rd4, %rd5b;\n");
        out.push_str("    ld.global.b32 %t0, [%rd5]; st.shared.b32 [%rd4], %t0;\n");
    }

    out.push_str("    bar.sync 0;\n");

    // A fragments: warp mh_warp slice at asmem + mh_warp*1024.
    out.push_str("    mov.u64 %rd4, asmem;\n");
    out.push_str("    mov.u32 %r12, %r10;\n");
    out.push_str("    mul.lo.u32 %r12, %r12, 1024;\n");
    out.push_str("    mul.wide.u32 %rd5b, %r12, 1;\n");
    out.push_str("    add.u64 %rd4, %rd4, %rd5b;\n");
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
    out.push_str("    add.u64 %rd5, %rd5, 512;\n");
    out.push_str("    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%a4, %a5, %a6, %a7}, [%rd5];\n");
    out.push_str("    mov.b32 %t0, %a1; mov.b32 %a1, %a2; mov.b32 %a2, %t0;\n");
    out.push_str("    mov.b32 %t0, %a5; mov.b32 %a5, %a6; mov.b32 %a6, %t0;\n");

    // B fragments: warp ng_warp slice at bsmem + ng_warp*2048, block g at +g*256.
    for g in 0..ng {
        let (b0, b1) = (&bregs[2 * g], &bregs[2 * g + 1]);
        out.push_str("    mov.u64 %rd4, bsmem;\n");
        out.push_str("    mov.u32 %r12, %r11;\n");
        out.push_str("    mul.lo.u32 %r12, %r12, 2048;\n");
        out.push_str("    mul.wide.u32 %rd5b, %r12, 1;\n");
        out.push_str("    add.u64 %rd4, %rd4, %rd5b;\n");
        out.push_str(&format!("    add.u64 %rd4, %rd4, {};\n", g * 256));
        out.push_str("    mov.u32 %r13, %r7;\n");
        out.push_str("    shr.u32 %r12, %r13, 4;\n");
        out.push_str("    and.b32 %r13, %r13, 15;\n");
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

    // 16 mma per warp.
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
    out.push_str("    add.u32 %r2, %r2, 16;\n");
    out.push_str("    bra.uni KLOOP;\nKEND:\n");

    // Store C: warp tile at rd6 + mh_warp*32*y_row + ng_warp*64*y_elem, then
    // the 2 mh × 8 ng sub-tiles.
    out.push_str("    mov.u32 %r12, %r10;\n");
    out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", 32 * y_row));
    out.push_str("    mov.u32 %r13, %r11;\n");
    out.push_str(&format!("    mul.lo.u32 %r13, %r13, {};\n", 64 * (y_elem as i64)));
    out.push_str("    mov.u32 %r9, %r12;\n");
    out.push_str("    add.u32 %r9, %r9, %r13;\n");
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
    fn dump_mw_1x1() {
        /* M=64,N=64,K=64, CTA 32x64 (mw=1,nw=1). a@0,b@8192,y@16392 */
        let ptx = tensor_gemm_ptx_smem_mw(64, 64, 64, 0, 8192, 16392, 2, 1, 1);
        std::fs::write("/tmp/opencode/tgemm_mw_1x1.ptx", &ptx).unwrap();
    }

    use super::*;
    #[test]
    fn dump_mw_128x128() {
        /* M=128,N=128,K=64, CTA 128x128 (mw=4,nw=2). a@0,b@16384,y@32776 */
        let ptx = tensor_gemm_ptx_smem_mw(128, 128, 64, 0, 16384, 32776, 2, 4, 2);
        std::fs::write("/tmp/opencode/tgemm_mw_128x128.ptx", &ptx).unwrap();
    }
    #[test]
    fn dump_mw_256x128() {
        /* M=256,N=128,K=64, CTA 256x128 (mw=8,nw=2). a@0,b=256*64*2=32768,
           y@32768+128*64*2+8=49160 */
        let ptx = tensor_gemm_ptx_smem_mw(256, 128, 64, 0, 32768, 49160, 2, 8, 2);
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
        /* M=N=K=2048, select_mw_nw=(2,8), block=512.
           a@0, b=2048*2048*2=8388608, y=2*8388608+8=16777224 */
        let ptx = tensor_gemm_ptx_smem_mw(2048, 2048, 2048, 0, 8388608, 16777224, 2, 2, 8);
        std::fs::write("/tmp/opencode/tgemm_mw_2048.ptx", &ptx).unwrap();
    }

    #[test]
    fn dump_mw_4096() {
        /* M=N=K=4096, select_mw_nw=(2,8), block=512.
           a@0, b=4096*4096*2=33554432, y=2*33554432+8=67108872 */
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 4096, 0, 33554432, 67108872, 2, 2, 8);
        std::fs::write("/tmp/opencode/tgemm_mw_4096.ptx", &ptx).unwrap();
    }

    #[test]
    fn dump_mw_8192() {
        /* M=N=K=8192, select_mw_nw=(2,8), block=512.
           a@0, b=8192*8192*2=134217728, y=2*134217728+8=268435464 */
        let ptx = tensor_gemm_ptx_smem_mw(8192, 8192, 8192, 0, 134217728, 268435464, 2, 2, 8);
        std::fs::write("/tmp/opencode/tgemm_mw_8192.ptx", &ptx).unwrap();
    }

    #[test]
    fn dump_mw_4096_k16() {
        /* M=N=4096,K=16 (skinny-K), select_mw_nw=(2,8), block=512.
           a@0, b=4096*16*2=131072, y=2*131072+8=262152 */
        let ptx = tensor_gemm_ptx_smem_mw(4096, 4096, 16, 0, 131072, 262152, 2, 2, 8);
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
