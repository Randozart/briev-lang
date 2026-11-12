//! Roofline Analyzer — Throughput-matched optimization
//!
//! Uses physical hardware constraints (cache sizes, bandwidth, FLOPS) to guide
//! precompute/fold decisions. A precomputed LUT that spills out of cache runs
//! *slower* than the arithmetic loop — the roofline model prevents that.

use crate::target_spec::BottleneckSection;

/// Which cache tier a LUT fits in (if any)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTier {
    L1,
    L2,
    L3,
}

/// Result of a roofline performance analysis
#[derive(Debug, Clone)]
pub struct RooflineResult {
    /// Attained performance in FLOP/s (or ops/s)
    pub attained: f64,
    /// Whether the operation is compute-bound (vs memory-bound)
    pub is_compute_bound: bool,
    /// Ridge point: arithmetic intensity where bound shifts
    pub ridge_point: f64,
}

/// Analyzes hardware constraints to guide optimization decisions.
pub struct RooflineAnalyzer {
    bottlenecks: BottleneckSection,
}

impl RooflineAnalyzer {
    pub fn new(bottlenecks: BottleneckSection) -> Self {
        Self { bottlenecks }
    }

    /// Peak compute throughput (rough estimate: assume 8-wide SIMD FMA at clock rate).
    fn peak_flops(&self) -> f64 {
        // Conservative: 8 FLOP/cycle (one 256-bit FMA = 2 FLOP × 4-wide lane)
        // If FPGA clock is set, use that as a multiplier
        let base = 3.0e9; // ~3 GHz × 8 FLOP/cycle
        if self.bottlenecks.fpga_clock_mhz > 0.0 {
            base * (self.bottlenecks.fpga_clock_mhz / 3000.0)
        } else {
            base
        }
    }

    /// Peak memory bandwidth in bytes/s
    fn peak_bandwidth(&self) -> f64 {
        self.bottlenecks.system_ram_bandwidth_gbs * 1.0e9
    }

    /// Cache size in bytes by tier
    pub fn cache_size_bytes(&self, tier: CacheTier) -> u64 {
        match tier {
            CacheTier::L1 => self.bottlenecks.l1_cache_size_kb * 1024,
            CacheTier::L2 => self.bottlenecks.l2_cache_size_kb * 1024,
            CacheTier::L3 => self.bottlenecks.l3_cache_size_kb * 1024,
        }
    }

    /// Compute roofline: given arithmetic intensity (FLOP/byte), determine bottleneck.
    pub fn compute_roofline(&self, flops: f64, bytes_moved: f64) -> RooflineResult {
        let peak_compute = self.peak_flops();
        let peak_bandwidth = self.peak_bandwidth();
        let ridge_point = peak_compute / peak_bandwidth; // FLOP/byte
        let intensity = if bytes_moved > 0.0 {
            flops / bytes_moved
        } else {
            f64::MAX
        };
        let attained = (peak_compute).min(peak_bandwidth * intensity);
        let is_compute_bound = intensity > ridge_point;
        RooflineResult {
            attained,
            is_compute_bound,
            ridge_point,
        }
    }

    /// Check if a precomputed LUT of the given size fits in a cache tier.
    pub fn lut_fits_cache(&self, lut_size_bytes: u64) -> Option<CacheTier> {
        if lut_size_bytes <= self.cache_size_bytes(CacheTier::L1) {
            Some(CacheTier::L1)
        } else if lut_size_bytes <= self.cache_size_bytes(CacheTier::L2) {
            Some(CacheTier::L2)
        } else if lut_size_bytes <= self.cache_size_bytes(CacheTier::L3) {
            Some(CacheTier::L3)
        } else {
            None
        }
    }

    /// Should the compiler precompute a loop body as a LUT?
    /// Returns `true` if the LUT fits in cache and has sufficient reuse factor.
    pub fn should_precompute_as_lut(
        &self,
        iterations: u64,
        lut_size_bytes: u64,
        bytes_saved_per_iter: f64,
    ) -> bool {
        match self.lut_fits_cache(lut_size_bytes) {
            Some(CacheTier::L1) => true, // Free — L1 is essentially zero-cost
            Some(CacheTier::L2) => true, // Worth it — L2 is fast
            Some(CacheTier::L3) => {
                // Only if bandwidth savings are significant (10x reuse)
                let bandwidth_saved = iterations as f64 * bytes_saved_per_iter;
                bandwidth_saved > lut_size_bytes as f64 * 10.0
            }
            None => false, // Spills to RAM — reject; loop will be faster
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_bottlenecks() -> BottleneckSection {
        BottleneckSection {
            pcie_bandwidth_gbs: 15.75,
            system_ram_bandwidth_gbs: 40.0,
            l1_cache_size_kb: 32,
            l2_cache_size_kb: 256,
            l3_cache_size_kb: 8192,
            memory_port_width: 1,
            fpga_clock_mhz: 0.0,
        }
    }

    #[test]
    fn test_lut_fits_l1() {
        let roofline = RooflineAnalyzer::new(test_bottlenecks());
        assert_eq!(roofline.lut_fits_cache(1024), Some(CacheTier::L1));
        assert_eq!(roofline.lut_fits_cache(32 * 1024), Some(CacheTier::L1));
    }

    #[test]
    fn test_lut_fits_l2() {
        let roofline = RooflineAnalyzer::new(test_bottlenecks());
        assert_eq!(roofline.lut_fits_cache(64 * 1024), Some(CacheTier::L2));
        assert_eq!(roofline.lut_fits_cache(256 * 1024), Some(CacheTier::L2));
    }

    #[test]
    fn test_lut_spills_to_ram() {
        let roofline = RooflineAnalyzer::new(test_bottlenecks());
        assert_eq!(roofline.lut_fits_cache(16 * 1024 * 1024), None);
    }

    #[test]
    fn test_should_precompute_l1() {
        let roofline = RooflineAnalyzer::new(test_bottlenecks());
        assert!(roofline.should_precompute_as_lut(1000, 1024, 0.0));
    }

    #[test]
    fn test_should_not_precompute_ram() {
        let roofline = RooflineAnalyzer::new(test_bottlenecks());
        assert!(!roofline.should_precompute_as_lut(1000, 16 * 1024 * 1024, 0.0));
    }

    #[test]
    fn test_roofline_compute_bound() {
        let roofline = RooflineAnalyzer::new(test_bottlenecks());
        // High compute, low bytes → compute bound
        let result = roofline.compute_roofline(1.0e12, 1.0);
        assert!(result.is_compute_bound);
    }

    #[test]
    fn test_roofline_memory_bound() {
        let roofline = RooflineAnalyzer::new(test_bottlenecks());
        // Low compute, high bytes → memory bound
        let result = roofline.compute_roofline(1.0, 1.0e12);
        assert!(!result.is_compute_bound);
    }
}
