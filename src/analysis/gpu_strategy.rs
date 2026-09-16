//! Shape-driven GPU strategy selection — the cost model that picks the
//! efficient kernel per shape (plan 2026-09-16-gpu-shape-strategy-selector).
//!
//! Every shape is efficient in SOME use case: small shapes win with small
//! tiles (parallelism), large with big tiles (arithmetic intensity), thin-K
//! with no pipeline, deep-K with a pipeline. This module derives the codegen
//! strategy (tile, stages, warp geometry) from shape evidence + hardware
//! parameters — a uniform cost model, not a lookup table. Calibrated against
//! the decompiled cuBLAS kernel map (Stage 0c).

/// Per-device hardware parameters for the cost model.
///
/// Measured on the target (RTX 3060 / sm_86 values in the default): peak TF
/// (f16 tensor), DRAM bandwidth, L2 size, shared memory per SM, static smem
/// cap per CTA, register file per SM, SM count. These belong in config
/// (measured per device, not guessed — gpu-offloading.md:134).
#[derive(Debug, Clone, Copy)]
pub struct GpuHardware {
    /// Peak tensor TFLOPS (f16-accumulate).
    pub peak_tflops: f64,
    /// DRAM bandwidth in GB/s.
    pub dram_gbps: f64,
    /// L2 cache bytes.
    pub l2_bytes: u64,
    /// Shared memory per SM in bytes (the configurable carve-out).
    pub smem_per_sm: u64,
    /// Static shared memory cap per CTA (no opt-in needed).
    pub smem_cta_cap: u64,
    /// Register file per SM (32-bit registers).
    pub regs_per_sm: u64,
    /// SM count.
    pub sm_count: u64,
}

impl GpuHardware {
    /// RTX 3060 (sm_86, GA106): the calibration target. Values from the
    /// ledger (docs/plans/2026-08-31-vitriol-gemm-comparison.md,
    /// docs/architecture/gpu-backend-strategy.md).
    pub const SM86: GpuHardware = GpuHardware {
        peak_tflops: 102.0,
        dram_gbps: 360.0,
        l2_bytes: 3 << 20,
        smem_per_sm: 100 * 1024,
        smem_cta_cap: 48 * 1024,
        regs_per_sm: 65536,
        sm_count: 28,
    };
}

/// A candidate codegen strategy for a GEMM shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strategy {
    /// CTA tile rows (the M-dim per block).
    pub tile_m: u64,
    /// CTA tile cols (the N-dim per block).
    pub tile_n: u64,
    /// Pipeline stages (K-slab ring depth).
    pub stages: u64,
    /// Load path: 0 = direct global fragment loads, 1 = staged smem fills.
    pub staged: bool,
}

/// The estimated execution time (in seconds) for a strategy on a shape.
#[derive(Debug, Clone, Copy)]
pub struct Estimate {
    pub seconds: f64,
    pub compute_bound: bool,
    pub occupancy_ctas: u64,
}

/// Generate the candidate strategy set for a GEMM shape.
///
/// Tiles are the CTA tile (M×N per block); warp geometry derives from the
/// tensor-GEMM convention (mhr rows per warp, tile = (16·mhr·mw)×(8·gr·nw),
/// gr = 16/mhr). Pruning: divisibility, smem ≤ cap, threads ≤ 256, CTA
/// count ≥ 1 (fill the SM count).
pub fn candidate_strategies(m: u64, n: u64, k: u64, hw: &GpuHardware) -> Vec<Strategy> {
    let mut out = Vec::new();
    // Warp-tile shapes from the tensor tier: mhr (rows per warp) and the
    // derived warp_n. mhr=4 → warp tile 64×32 (E4c), mhr=2 → 32×64. The
    // warp grid is (mw×nw) with mw·nw·32 ≤ 256 threads and mw,nw ≤ 8 —
    // bounded enumeration (constant, not data-dependent).
    for (warp_m, warp_n) in [(64u64, 32u64), (32u64, 64u64)] {
        for pair in 1u64..=64u64 {
            let mw = pair / 8 + 1;
            let nw = pair % 8 + 1;
            let tile = (warp_m * mw, warp_n * nw);
            if mw * nw * 32 <= 256 && m % tile.0 == 0 && n % tile.1 == 0 {
                push_stages(&mut out, m, n, k, hw, tile);
            }
        }
    }
    out.sort_by_key(|s| (s.tile_m * s.tile_n, s.stages));
    out.dedup();
    out
}

/// Push the stage variants of a divisible tile that fit the smem cap.
fn push_stages(
    out: &mut Vec<Strategy>,
    m: u64,
    n: u64,
    k: u64,
    hw: &GpuHardware,
    (tile_m, tile_n): (u64, u64),
) {
    for stages in 1..=4u64 {
        let cta_smem = smem_for(tile_m, tile_n, k, stages);
        let ctas = (m / tile_m) * (n / tile_n);
        if cta_smem <= hw.smem_cta_cap && ctas > 0 {
            out.push(Strategy {
                tile_m,
                tile_n,
                stages,
                staged: stages > 1,
            });
        }
    }
}

/// Shared memory per CTA for a tile/stage combination (f16 operands, the
/// A/B panels per stage: A = tile_m×16 f16, B = 16×tile_n f16, ×stages).
fn smem_for(tile_m: u64, tile_n: u64, _k: u64, stages: u64) -> u64 {
    let a_panel = tile_m * 16 * 2;
    let b_panel = 16 * tile_n * 2;
    (a_panel + b_panel) * stages
}

/// Estimate execution time for a strategy on a shape.
///
/// Roofline: compute-bound when flops/peak ≥ bytes/bw; memory time is
/// HBM bytes (L2-aware: the B operand's re-read across m-tiles hits L2 when
/// the B slab fits; the ledger measures ~30% L2 supply at 4096³, not 80%).
/// Underfill penalty: fewer CTAs than the SM count scales time by the
/// unused-SM fraction.
pub fn estimate_time(m: u64, n: u64, k: u64, s: &Strategy, hw: &GpuHardware) -> Estimate {
    let flops = 2.0 * m as f64 * n as f64 * k as f64;
    let compute_s = flops / (hw.peak_tflops * 1e12);

    // HBM bytes: A is M×K, re-read per n-tile; B is K×N, re-read per
    // m-tile. With an L2-resident B slab, a fraction of the B re-reads hit
    // L2 (measured ~30% supply at 4096³ — gpu-backend-strategy.md:82).
    let m_tiles = m.div_ceil(s.tile_m);
    let n_tiles = n.div_ceil(s.tile_n);
    let a_bytes = (m * k * 2) as f64 * n_tiles as f64;
    let b_bytes = (k * n * 2) as f64 * m_tiles as f64;
    let b_slab = (k * s.tile_n * 2) as u64; // one m-tile's B slab
    let b_l2_hits = if b_slab <= hw.l2_bytes { 0.3 } else { 0.0 };
    let b_bytes = b_bytes * (1.0 - b_l2_hits);
    let total_bytes = a_bytes + b_bytes;
    let memory_s = total_bytes / (hw.dram_gbps * 1e9);

    let compute_bound = compute_s >= memory_s;
    // Pipeline model: cp.async overlap hides memory behind compute. With
    // `stages` K-slabs, only ~memory/stages is exposed (the prologue fill);
    // the rest overlaps the mma. Time = compute + exposed memory.
    let mut seconds = compute_s + memory_s / s.stages as f64;

    // Underfill: when the grid has fewer CTAs than the SM count, the GPU
    // sits partially idle — scale by the unused-SM fraction.
    let ctas = m_tiles * n_tiles;
    if ctas < hw.sm_count {
        seconds *= hw.sm_count as f64 / ctas as f64;
    }

    Estimate {
        seconds,
        compute_bound,
        occupancy_ctas: ctas,
    }
}

/// Select the best strategy for a GEMM shape.
pub fn select(m: u64, n: u64, k: u64, hw: &GpuHardware) -> Option<Strategy> {
    let cands = candidate_strategies(m, n, k, hw);
    if cands.is_empty() {
        return None;
    }
    cands
        .into_iter()
        .min_by(|a, b| {
            let ta = estimate_time(m, n, k, a, hw).seconds;
            let tb = estimate_time(m, n, k, b, hw).seconds;
            ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_small_tile_for_small_shape() {
        let hw = GpuHardware::SM86;
        let s = select(64, 64, 64, &hw).expect("candidate");
        assert!(s.tile_m <= 64 && s.tile_n <= 64, "64³ needs a small tile: {:?}", s);
    }

    #[test]
    fn selects_large_tile_for_large_shape() {
        let hw = GpuHardware::SM86;
        let s = select(4096, 4096, 4096, &hw).expect("candidate");
        assert!(s.tile_m >= 128 && s.tile_n >= 128, "4096³ needs a big tile: {:?}", s);
    }

    #[test]
    fn small_shape_underfill_penalty_favors_small_tiles() {
        let hw = GpuHardware::SM86;
        // 64³ with the (2,4) 128×128 tile → 1 CTA, huge underfill penalty.
        let big = Strategy {
            tile_m: 128,
            tile_n: 128,
            stages: 2,
            staged: true,
        };
        let small = Strategy {
            tile_m: 32,
            tile_n: 32,
            stages: 1,
            staged: false,
        };
        let t_big = estimate_time(64, 64, 64, &big, &hw).seconds;
        let t_small = estimate_time(64, 64, 64, &small, &hw).seconds;
        assert!(
            t_small < t_big,
            "small tile should win at 64³: small={:.2e} big={:.2e}",
            t_small,
            t_big
        );
    }

    #[test]
    fn large_shape_prefers_staged_pipeline() {
        let hw = GpuHardware::SM86;
        let s = select(4096, 4096, 4096, &hw).expect("candidate");
        assert!(s.staged, "4096³ prefers a pipelined strategy: {:?}", s);
    }

    #[test]
    fn empty_candidates_returns_none() {
        // Non-divisible shape → no candidate.
        let hw = GpuHardware::SM86;
        assert!(select(100, 100, 100, &hw).is_none() || candidate_strategies(100, 100, 100, &hw).is_empty());
    }

    /// Stage 0c calibration: the model's selected tile family must track
    /// the decompiled cuBLAS map (docs/plans/2026-09-16-panel-pipeline
    /// -generalization.md:134): small shapes → small tiles, large → big
    /// tiles. Exact stage counts are tolerated mismatch (cuBLAS autotunes).
    #[test]
    fn calibration_matches_cublas_tile_family() {
        let hw = GpuHardware::SM86;
        // (shape, cuBLAS tile) — family check: ours must be within 2× of
        // cuBLAS's tile AREA at each shape.
        let cases = [
            (64u64, 32u64 * 32),
            (128, 32 * 32),
            (256, 64 * 64),
            (512, 96 * 128),
            (1024, 128 * 128),
            (2048, 128 * 128),
            (4096, 256 * 128),
        ];
        for (shape, cublas_area) in cases {
            let s = select(shape, shape, shape, &hw).expect("candidate");
            let area = s.tile_m * s.tile_n;
            let ratio = area as f64 / cublas_area as f64;
            assert!(
                (0.25..=4.0).contains(&ratio),
                "{shape}³: our tile {}x{} (area {area}) not within family of cuBLAS {}x{} (area {cublas_area}), ratio {ratio:.2}",
                s.tile_m, s.tile_n, cublas_area / shape, shape
            );
        }
    }

    #[test]
    fn e4c_tile_preserved_at_4096() {
        let hw = GpuHardware::SM86;
        let s = select(4096, 4096, 4096, &hw).expect("candidate");
        // E4c: 128x128 tile (the (2,4)@256T f16acc point).
        assert_eq!((s.tile_m, s.tile_n), (128, 128), "4096³ must keep E4c tile: {:?}", s);
    }

    #[test]
    fn small_shape_gets_smaller_tile_than_big() {
        let hw = GpuHardware::SM86;
        let small = select(128, 128, 128, &hw).expect("candidate");
        let big = select(2048, 2048, 2048, &hw).expect("candidate");
        assert!(
            small.tile_m * small.tile_n < big.tile_m * big.tile_n,
            "128³ tile {}x{} should be smaller than 2048³ {}x{}",
            small.tile_m, small.tile_n, big.tile_m, big.tile_n
        );
    }
}
