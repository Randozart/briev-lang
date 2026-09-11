// ── Tuning Configuration (plan §8.1 / §8.2, frontend-driven-dispatch) ─
//
// 2026-07-31: Phase 3 — moves hardcoded codegen tuning constants out of the
// compiler into config, so silent x86_64 assumptions never apply to unknown
// targets and per-target/per-program knobs can evolve without recompiling.
//
// Two files:
//   - `config/targets.dbvl` `target.<triple-prefix>` entries → TargetSettings
//     (per-target: float_registers, dense_compute_density, vector_min_width).
//     Looked up by matching the compiler's target_triple PREFIX. An unknown
//     prefix falls back to x86_64 defaults + `warn_unknown_target` so the
//     fallback is never silent.
//   - `config/ir-lowering.dbvl` → IrLoweringSettings (global tuning knobs:
//     arena budget/size, stack threshold, SROA chunking, SSO/SVO caps, inline
//     weight). SSO's 6-byte default is derived from the String handle
//     representation (align 8 − 2 tag bits); the config entry is an override.
//
// 2026-08-03 (Phase 3, data-briev-config plan): both files migrated from TOML
// to the flat .dbvl line-table form, still baked at compile time via include_str!
// and cached with LazyLock.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Per-target codegen tuning (plan §8.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetSettings {
    /// Register-pressure budget for vector-phi promotion.
    pub float_registers: usize,
    /// Cross-float-ops-per-field threshold for the `#11 → #0` downgrade.
    pub dense_compute_density: f64,
    /// Minimum isomorphic-group width for vector-phi promotion.
    pub vector_min_width: usize,
}

/// Global (target-independent) IR lowering tuning (plan §8.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IrLoweringSettings {
    /// Below this `--optimize-budget`, skip the bump arena (direct malloc).
    pub arena_min_budget: u32,
    /// Initial per-txn bump arena size.
    pub arena_initial_size: u64,
    /// Stack-allocation threshold for transient collections.
    pub stack_threshold: u64,
    /// %StateChunk<N> field cap so SROA can decompose each chunk.
    pub max_fields_per_alloca: usize,
    /// SSO small-string inline payload cap (derived: align 8 − 2 tag bits).
    pub sso_max_bytes: usize,
    /// SVO small-vector inline element cap.
    /// Weighted body-cost threshold for callable-txn auto-inline.
    pub callable_inline_weight_threshold: u32,
    /// Accel auto-tuning probe: full-map runs per lane (Phase 7).
    pub accel_probe_k: u64,
    /// Accel probe output-equality tolerance (relative per element).
    pub accel_probe_tolerance: f64,
    /// Accel probe commit margin: GPU must beat CPU by 1 + margin.
    pub accel_probe_margin: f64,
    /// 2026-08-31 (VITRIOL GEMM comparison O1): SPIR-V kernel foreach
    /// unroll factor for constant trip counts (0 disables unrolling).
    pub spirv_unroll: u32,
    /// 2026-09-01 (plan 2026-09-01-cooperative-row-kernels): cooperative row
    /// kernels (lane-strided accumulation + OpGroupNonUniformFAdd). OFF by
    /// default: the emitted kernel passes spirv-val and the minimal subgroup
    /// probe verifies on device, but the full GEMV integration produced
    /// wrong rows on the RTX 3060 — re-enable after the integration bug is
    /// root-caused (see the plan's outcome section).
    pub spirv_row_cooperative: bool,
    /// 2026-09-01 (plan 2026-09-01-m2-tensor-cores): lower Float16-operand
    /// GEMMs to VK_KHR_cooperative_matrix tensor-core fragments (fp16 in,
    /// fp32 accumulate). Build-time choice: the TARGET device must expose
    /// the extension, else the runtime falls back to CPU for that blob —
    /// the exact tiled kernel stays available with the knob off.
    pub spirv_coopmat: bool,
    /// 2026-09-02 (plan 2026-09-02-tensor-tier-run, B-reuse rung): tile-ROWS
    /// (16-row strips) per coopmat workgroup. R > 1 loads each B fragment
    /// once and reuses it across R A fragments — B DRAM traffic ÷ R (the
    /// tensor tier is B-traffic-bound: 10.5 GB total ≈ 29 ms floor at
    /// ~360 GB/s; measured min 41 ms at R=1). A/B history (RTX 3060,
    /// 4096³, all rel 2.442e-04): with unlocked clocks R=4 was bimodal
    /// (9.9ms boost-mode / 70ms idle-downclock mode — the GPU dropped to
    /// 139MHz between fence-waited launches; GPU_IDLE throttle flags, not
    /// thermal). After LOCKING clocks (`nvidia-smi -lgc 1800,1837 -pm 1`,
    /// user-run 2026-09-02), R=4 is stable: unfed mins 9.56-9.78ms =
    /// **14.3 TFLOP/s, 1.14× the ggml anchor** — R=4 is the default.
    /// Deployment note: fed/bursty both hold with locked clocks; without
    /// the lock, bursty callers should prefer R=2 (stable 13.3ms) or feed
    /// the queue. Capped at 8: R=16 was slower AND miscomputed on device
    /// (VERDICT: rejected — suspected coopmat fragment spill past 256
    /// acc regs/lane). Shapes not divisible by 16R clamp down the
    /// power-of-two ladder (kernel + runner share the clamp).
    pub spirv_coopmat_tile_rows: u32,
    /// 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): image
    /// storage strategy — realize texel-formatted write buffers as device
    /// storage images when the kernel's index math yields dimensions.
    /// Opt-in until measured (coopmat precedent).
    pub spirv_image_storage: bool,
    /// 2026-09-02 (plan 2026-09-02-cuda-race B3): the FP16-accumulate
    /// tensor tier — coopmat<f16> accumulators (Ampere double-pumped mma,
    /// 2x the FP32-acc ceiling). SEPARATE numerics tier: gate rel <= 1e-2
    /// vs the f32-acc tier (the default + correctness reference).
    pub spirv_coopmat_f16acc: bool,
    /// 2026-09-02 (plan 2026-09-02-cuda-race B2): subgroups per tensor-tier
    /// workgroup. S=1 (default) = one 32-lane warp per workgroup (16k
    /// workgroups of 32 lanes underfill the 3060's 28 SMs); S>1 packs S
    /// subgroups per workgroup, each owning its own tile_n slice via the
    /// SubgroupId builtin — fewer, fatter workgroups.
    pub spirv_coopmat_subgroups: u32,
    /// 2026-09-04 (perf-blocks plan): smem double-buffer staging for the
    /// tensor tier. 1 (default) = Workgroup staging arrays + coopmat loads
    /// from smem; 0 = direct SSBO coopmat loads (pre-smem form). A/B lever
    /// for the staging experiment. (The dbvl declared this key since the
    /// smem commit but no setting consumed it — dead-knob fix.)
    pub spirv_coopmat_smem: bool,
    /// 2026-09-04 (beyond-coopmat Stage 1, D3): the smem fill pairs the
    /// per-element half loads/stores into u32 loads/stores (adjacent flats
    /// = adjacent DRAM halves = adjacent smem slots). Halves the fill's
    /// instruction count — the fill is ~8× the mma instruction volume.
    /// 0 = scalar half fill (the historical form).
    pub spirv_coopmat_fill_pairs: bool,
    /// 2026-09-04 (beyond-coopmat Stage 1, D3b): the paired fill iterates
    /// f16×4 quads — two v2f16 loads/stores per loop iteration (half the
    /// pairs-mode instruction count again). Requires fill_pairs (same
    /// v2f16 member view). 0 = pairs-mode loop shape.
    pub spirv_coopmat_fill_quad: bool,
    /// 2026-09-04 (beyond-coopmat Stage 1, D2): the smem refill's DRAM
    /// loads issue into registers during the mma phase; the smem stores
    /// happen after the first barrier. Requires fill_pairs. 0 = fused
    /// post-barrier refill.
    pub spirv_coopmat_fill_prefetch: bool,
    /// 2026-09-05 (beyond-coopmat Stage 1, D5): rotate each workgroup's
    /// K-panel start (wgid % 8 buckets) so co-resident workgroups'
    /// fill/mma barrier phases desynchronize — the anti-phase-locking
    /// lever. Output values are order-independent (each tile sums all
    /// K panels; only the f16 rounding walk changes).
    pub spirv_coopmat_stagger: bool,
    /// 2026-09-04 (beyond-coopmat Stage 1, D1): panels per double-buffer
    /// stage — 2 halves the barrier count per panel and doubles the mma
    /// per barrier pair. Falls back to 1 when (K/16) is odd (the tail
    /// pair would double-count a clamped panel).
    pub spirv_coopmat_panels_per_stage: u32,
    /// 2026-09-10 (PTX tier, f16-acc contract): the mw tensor kernel's mma
    /// accumulates in f16x2 pairs with an 8-kstep chunk promotion into the
    /// f16 y tile via CTA-private read-modify-write (y zeroed by the kernel
    /// prologue). Numerics: f16 rounding per chunk, f32 across chunks —
    /// tier gate 1e-2 (the f32-acc default keeps the 5e-3 gate). 0 =
    /// f32-acc (historical form).
    pub ptx_tensor_f16acc: bool,
    /// 2026-09-11 (cubin shipping): compile the emitted PTX through offline
    /// ptxas and ship cubin bytes as the kernel blob. The driver JIT is
    /// avoided entirely: its CU_JIT_MAX_REGISTERS is ignored (166 vs the
    /// requested 128 → 1 CTA/SM, −27%), it rejects the `.maxnreg` directive
    /// text, and its internal compiler state wedges after fault storms.
    /// ptxas lookup: `$TRITON_PTXAS`, PATH, then the triton install layout.
    /// ptxas unavailable or failing → PTX-text fallback (JIT path).
    pub ptx_emit_cubin: bool,
    /// 2026-09-07 (single-buffer rung): smem stages per buffer — 1 halves
    /// the smem footprint (2× the resident WGs/SM at 48KB) at the cost of
    /// a strictly serial fill→mma pipeline per workgroup. 2 = the
    /// historical double-buffer. The fill is barrier-serialized either
    /// way, so the spare stage buys no overlap — only smem it occupies.
    pub spirv_coopmat_stages: u32,
    /// CIRCT: state arrays at/above this depth default to the seq.firmem
    /// memory macro (below: register files). 2026-08-25, seq-firmem plan.
    pub firmem_min_depth: usize,
    /// CIRCT: max distinct read sites per mem-lowered array (macro ports).
    pub firmem_max_ports: usize,
    /// CIRCT FSM clock frequency in Hz — converts time-unit watchdog bounds
    /// (`within 10ms`) to cycle counts. 0 = unset: time-unit watchdogs stay
    /// capability errors. 2026-08-26.
    pub clock_hz: u64,
}

/// Enforcement policy for `axiom`-declared authority (plan 2026-08-26).
/// Config: config/axioms.dbv `policy` key (allow | warn | deny).
/// The `.s` strict report always renders the full axiom ledger regardless of
/// this setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxiomPolicy {
    /// Accepted; one info line per site in the warnings stream.
    Allow,
    /// Accepted; a prominent warning naming every site rides alongside.
    Warn,
    /// Any axiom site is a hard error: prove it or remove the shortcut.
    Deny,
}

impl AxiomPolicy {
    /// Parse from a string, defaulting to Allow on unknown values.
    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "warn" => AxiomPolicy::Warn,
            "deny" => AxiomPolicy::Deny,
            _ => AxiomPolicy::Allow,
        }
    }
}

/// Global axiom-policy and lemma-property vocabulary (plan 2026-08-26).
#[derive(Debug, Clone)]
pub struct AxiomSettings {
    pub policy: AxiomPolicy,
    /// Closed vocabulary of optimizer-exploitable properties (e.g. "commutative").
    /// Anything outside this list is rejected at parse validation.
    pub lemma_properties: Vec<String>,
}

/// Defaults: allow + empty lemma vocabulary.
const DEFAULT_AXIOM_SETTINGS: AxiomSettings = AxiomSettings {
    policy: AxiomPolicy::Allow,
    lemma_properties: Vec::new(),
};

/// Cached axiom settings (baked config/axioms.dbv).
static AXIOM_SETTINGS: LazyLock<AxiomSettings> = LazyLock::new(load_axioms);

/// Return the global axiom settings.
pub fn axiom_settings() -> &'static AxiomSettings {
    &AXIOM_SETTINGS
}

/// x86_64 defaults — also the fallback for unknown target prefixes.
pub const DEFAULT_TARGET_SETTINGS: TargetSettings = TargetSettings {
    float_registers: 16,
    dense_compute_density: 4.0,
    vector_min_width: 4,
};

const DEFAULT_IR_LOWERING: IrLoweringSettings = IrLoweringSettings {
    arena_min_budget: 128,
    arena_initial_size: 65536,
    stack_threshold: 4096,
    max_fields_per_alloca: 15,
    sso_max_bytes: 6,
    callable_inline_weight_threshold: 40,
    accel_probe_k: 2,
    accel_probe_tolerance: 0.0001,
    accel_probe_margin: 0.05,
    spirv_unroll: 16,
    spirv_row_cooperative: false,
    spirv_coopmat: false,
    spirv_coopmat_tile_rows: 4,
    spirv_image_storage: false,
    spirv_coopmat_f16acc: false,
    spirv_coopmat_subgroups: 1,
    spirv_coopmat_smem: true,
    spirv_coopmat_fill_pairs: false,
    spirv_coopmat_fill_quad: false,
    spirv_coopmat_fill_prefetch: false,
    spirv_coopmat_stagger: false,
    spirv_coopmat_panels_per_stage: 2,
    ptx_tensor_f16acc: false,
    ptx_emit_cubin: true,
    spirv_coopmat_stages: 1,

    firmem_min_depth: 64,
    firmem_max_ports: 4,
    clock_hz: 0,
};

/// Per-target-prefix tuning tables, keyed by triple prefix (e.g. "x86_64").
static TARGET_SETTINGS: LazyLock<HashMap<String, TargetSettings>> =
    LazyLock::new(load_target_settings);

/// Global IR-lowering tuning (baked config/ir-lowering.toml).
static IR_LOWERING: LazyLock<IrLoweringSettings> = LazyLock::new(load_ir_lowering);

/// Return the global IR-lowering settings.
pub fn ir_lowering() -> &'static IrLoweringSettings {
    &IR_LOWERING
}

/// Resolve tuning settings for a target triple by longest-prefix match.
///
/// 2026-07-31: Matches `ctx.target_triple` against the `[target.<prefix>]`
/// keys. Falls back to the x86_64 defaults; `warn_unknown_target` reports the
/// fallback so unknown targets never silently inherit x86 assumptions.
pub fn target_settings_for(triple: &str) -> TargetSettings {
    let mut best: Option<(usize, &TargetSettings)> = None;
    for (prefix, settings) in TARGET_SETTINGS.iter() {
        if triple.starts_with(prefix.as_str()) && best.map_or(true, |(blen, _)| prefix.len() > blen) {
            best = Some((prefix.len(), settings));
        }
    }
    best.map(|(_, s)| *s).unwrap_or(DEFAULT_TARGET_SETTINGS)
}

/// Is the triple's prefix known to config/targets.dbvl?
pub fn known_target_triple(triple: &str) -> bool {
    TARGET_SETTINGS.keys().any(|p| triple.starts_with(p.as_str()))
}

/// Walk config/targets.dbvl's `target.<prefix>` rows.
///
/// 2026-08-03 (Phase 3, data-briev-config plan): migrated from the
/// `[target.<prefix>]` tables in targets.dbvl. Row shape:
/// `target.<prefix>: <float_registers>; <dense_compute_density>; <vector_min_width>;`.
fn load_target_settings() -> HashMap<String, TargetSettings> {
    let content = include_str!("../config/targets.dbvl");
    let db = match crate::dbriev::config_db::ConfigDb::from_str(content) {
        Ok(db) => db,
        Err(e) => panic!("config/targets.dbvl parse error: {}", e),
    };
    let mut out = HashMap::new();
    for key in db.keys() {
        let Some(prefix) = key.strip_prefix("target.") else { continue };
        let float_registers = db
            .field_int(&key, 0)
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_TARGET_SETTINGS.float_registers);
        let dense_compute_density = db
            .field_float(&key, 1)
            .unwrap_or(DEFAULT_TARGET_SETTINGS.dense_compute_density);
        let vector_min_width = db
            .field_int(&key, 2)
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_TARGET_SETTINGS.vector_min_width);
        out.insert(
            prefix.to_string(),
            TargetSettings {
                float_registers,
                dense_compute_density,
                vector_min_width,
            },
        );
    }
    out
}

/// Parse config/ir-lowering.dbvl with per-key defaults.
///
/// 2026-08-03 (Phase 3, data-briev-config plan): migrated from ir-lowering.toml
/// to the flat .dbvl line-table form; still compile-time baked via include_str!.
/// Absent keys fall back to the hardcoded defaults, matching pre-migration.
fn load_ir_lowering() -> IrLoweringSettings {
    let content = include_str!("../config/ir-lowering.dbvl");
    let db = match crate::dbriev::config_db::ConfigDb::from_str(content) {
        Ok(db) => db,
        Err(e) => panic!("config/ir-lowering.dbvl parse error: {}", e),
    };
    IrLoweringSettings {
        arena_min_budget: db
            .field_int("arena_min_budget", 0)
            .unwrap_or(DEFAULT_IR_LOWERING.arena_min_budget as i64) as u32,
        firmem_min_depth: db
            .field_int("circt.firmem_min_depth", 0)
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_IR_LOWERING.firmem_min_depth),
        firmem_max_ports: db
            .field_int("circt.firmem_max_ports", 0)
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_IR_LOWERING.firmem_max_ports),
        clock_hz: db
            .field_int("circt.clock_hz", 0)
            .map(|v| v as u64)
            .unwrap_or(DEFAULT_IR_LOWERING.clock_hz),
        arena_initial_size: db
            .field_int("arena_initial_size", 0)
            .unwrap_or(DEFAULT_IR_LOWERING.arena_initial_size as i64) as u64,
        stack_threshold: db
            .field_int("stack_threshold", 0)
            .unwrap_or(DEFAULT_IR_LOWERING.stack_threshold as i64) as u64,
        max_fields_per_alloca: db
            .field_int("max_fields_per_alloca", 0)
            .unwrap_or(DEFAULT_IR_LOWERING.max_fields_per_alloca as i64) as usize,
        sso_max_bytes: db
            .field_int("sso_max_bytes", 0)
            .unwrap_or(DEFAULT_IR_LOWERING.sso_max_bytes as i64) as usize,
        callable_inline_weight_threshold: db
            .field_int("callable_inline_weight_threshold", 0)
            .unwrap_or(DEFAULT_IR_LOWERING.callable_inline_weight_threshold as i64) as u32,
        accel_probe_k: db
            .field_int("accel_probe_k", 0)
            .map(|v| v.max(1) as u64)
            .unwrap_or(DEFAULT_IR_LOWERING.accel_probe_k),
        accel_probe_tolerance: db
            .field_float("accel_probe_tolerance", 0)
            .unwrap_or(DEFAULT_IR_LOWERING.accel_probe_tolerance),
        accel_probe_margin: db
            .field_float("accel_probe_margin", 0)
            .unwrap_or(DEFAULT_IR_LOWERING.accel_probe_margin),
        spirv_unroll: db
            .field_int("spirv_unroll", 0)
            .map(|v| v.max(0) as u32)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_unroll),
        spirv_row_cooperative: db
            .field_int("spirv_row_cooperative", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_row_cooperative),
        spirv_coopmat: db
            .field_int("spirv_coopmat", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat),
        // NB: field_int's second arg is the FIELD INDEX (0 for scalars),
        // not a default — the default comes from DEFAULT_IR_LOWERING via
        // unwrap_or. (Passing 8 here silently read index 8 = None = the
        // R=8 default on every build, pinning the A/B to one config.)
        spirv_coopmat_tile_rows: db
            .field_int("spirv_coopmat_tile_rows", 0)
            .map(|v| v.max(1) as u32)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat_tile_rows),
        spirv_image_storage: db
            .field_int("spirv_image_storage", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_image_storage),
        spirv_coopmat_f16acc: db
            .field_int("spirv_coopmat_f16acc", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat_f16acc),
        spirv_coopmat_subgroups: db
            .field_int("spirv_coopmat_subgroups", 0)
            .map(|v| v.max(1).min(4) as u32)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat_subgroups),
        spirv_coopmat_smem: db
            .field_int("spirv_coopmat_smem", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat_smem),
        spirv_coopmat_fill_pairs: db
            .field_int("spirv_coopmat_fill_pairs", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat_fill_pairs),
        spirv_coopmat_fill_quad: db
            .field_int("spirv_coopmat_fill_quad", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat_fill_quad),
        spirv_coopmat_fill_prefetch: db
            .field_int("spirv_coopmat_fill_prefetch", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat_fill_prefetch),
        spirv_coopmat_stagger: db
            .field_int("spirv_coopmat_stagger", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat_stagger),
        spirv_coopmat_panels_per_stage: db
            .field_int("spirv_coopmat_panels_per_stage", 0)
            .map(|v| v.max(1).min(4) as u32)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat_panels_per_stage),
        ptx_tensor_f16acc: db
            .field_int("ptx_tensor_f16acc", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.ptx_tensor_f16acc),
        ptx_emit_cubin: db
            .field_int("ptx_emit_cubin", 0)
            .map(|v| v != 0)
            .unwrap_or(DEFAULT_IR_LOWERING.ptx_emit_cubin),
        spirv_coopmat_stages: db
            .field_int("spirv_coopmat_stages", 0)
            .map(|v| v.max(1).min(2) as u32)
            .unwrap_or(DEFAULT_IR_LOWERING.spirv_coopmat_stages),
    }
}

/// Load axiom settings from config/axioms.dbv (structured Data Briev —
/// human-editable config, house rule 2026-08-27). Parsed with the full v2
/// parser (quoted mode), same pattern as backend/metadata.rs.
/// Absent fields fall back to the hardcoded defaults (allow, no lemma
/// properties). Panics on parse failure — the .dbv is a compile-time invariant.
fn load_axioms() -> AxiomSettings {
    let content = include_str!("../config/axioms.dbv");
    let doc = crate::dbriev::v2::parse_document_quoted(content)
        .expect("config/axioms.dbv: parse failed — check .dbv syntax");
    let mut settings = AxiomSettings {
        policy: DEFAULT_AXIOM_SETTINGS.policy,
        lemma_properties: Vec::new(),
    };
    for group in &doc.data_groups {
        if group.schema_name.as_deref() != Some("AxiomSettings") {
            continue;
        }
        for entry in &group.entries {
            let key = match entry.key {
                Some(ref k) => k.to_ascii_lowercase(),
                None => continue,
            };
            match key.as_str() {
                "policy" => {
                    if let Some(crate::dbriev::v2::DataField::Positional(
                        crate::dbriev::v2::DataValue::String(s),
                    )) = entry.fields.first()
                    {
                        settings.policy = AxiomPolicy::from_str_loose(s);
                    }
                }
                "lemma_properties" => {
                    if let Some(crate::dbriev::v2::DataField::Positional(v)) = entry.fields.first() {
                        settings.lemma_properties = lemma_vocab_values(v);
                    }
                }
                _ => {}
            }
        }
    }
    settings
}

/// Flatten a lemma_properties value into the lowercase vocabulary list.
/// Accepts one bare token ("commutative"), a comma-separated token
/// ("commutative, identity"), or a Vec[...] value.
fn lemma_vocab_values(value: &crate::dbriev::v2::DataValue) -> Vec<String> {
    let mut out = Vec::new();
    match value {
        crate::dbriev::v2::DataValue::List(items) => {
            for item in items {
                out.extend(lemma_vocab_values(item));
            }
        }
        crate::dbriev::v2::DataValue::String(s) => {
            out.extend(
                s.split(|c: char| c == ',' || c == ' ')
                    .filter(|t| !t.is_empty())
                    .map(|t| t.trim().to_ascii_lowercase()),
            );
        }
        _ => {}
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Pre-migration config/ir-lowering.toml, frozen as the golden reference
    /// for parity_ir_lowering_dbvl_matches_toml. 2026-08-03: the .toml file is
    /// deleted; edits to config/ir-lowering.dbvl must keep this test green.
    const IR_LOWERING_TOML_GOLDEN: &str = r#"
arena_min_budget = 128
arena_initial_size = 65536
stack_threshold = 4096
max_fields_per_alloca = 15
sso_max_bytes = 6
svo_max_elements = 3
callable_inline_weight_threshold = 40
accel_probe_k = 2
accel_probe_tolerance = 0.0001
accel_probe_margin = 0.05
"#;

    /// Pre-migration config/targets.toml, frozen as the golden reference for
    /// parity_target_settings_dbvl_matches_toml (the `[target.<prefix>]`
    /// tables only). 2026-08-03: the .toml file is deleted; edits to the
    /// `target.*` rows in config/targets.dbvl must keep this test green.
    const TARGET_SETTINGS_TOML_GOLDEN: &str = r#"
[target.x86_64]
float_registers = 16
dense_compute_density = 4.0
vector_min_width = 4

[target.aarch64]
float_registers = 32
dense_compute_density = 4.0
vector_min_width = 4

[target.arm64]
float_registers = 32
dense_compute_density = 4.0
vector_min_width = 4

[target.wasm32]
float_registers = 4294967295
dense_compute_density = 4.0
vector_min_width = 4

[target.wasm64]
float_registers = 4294967295
dense_compute_density = 4.0
vector_min_width = 4

[target.spirv64]
float_registers = 32
dense_compute_density = 4.0
vector_min_width = 4
"#;

    #[test]
    fn test_ir_lowering_defaults_match_hardcoded() {
        let s = ir_lowering();
        assert_eq!(s.arena_min_budget, 128);
        assert_eq!(s.arena_initial_size, 65536);
        assert_eq!(s.stack_threshold, 4096);
        assert_eq!(s.max_fields_per_alloca, 15);
        assert_eq!(s.sso_max_bytes, 6);
        assert_eq!(s.callable_inline_weight_threshold, 40);
    }

    #[test]
    fn parity_ir_lowering_dbvl_matches_toml() {
        // Phase 3 migration gate: config/ir-lowering.dbvl must produce exactly
        // the values the TOML it replaces produced. 2026-08-03: the .toml is
        // deleted; this is now a GOLDEN test — the pre-migration TOML is baked
        // below and re-parsed.
        let s = ir_lowering();
        let raw: toml::Value = toml::from_str(IR_LOWERING_TOML_GOLDEN).unwrap();
        let i64_of = |k: &str| raw.get(k).and_then(toml::Value::as_integer).unwrap();
        assert_eq!(s.arena_min_budget as i64, i64_of("arena_min_budget"));
        assert_eq!(s.arena_initial_size as i64, i64_of("arena_initial_size"));
        assert_eq!(s.stack_threshold as i64, i64_of("stack_threshold"));
        assert_eq!(s.max_fields_per_alloca as i64, i64_of("max_fields_per_alloca"));
        assert_eq!(s.sso_max_bytes as i64, i64_of("sso_max_bytes"));
        assert_eq!(s.callable_inline_weight_threshold as i64, i64_of("callable_inline_weight_threshold"));
    }

    #[test]
    fn test_target_settings_x86_64() {
        let s = target_settings_for("x86_64-unknown-linux-gnu");
        assert_eq!(s.float_registers, 16);
        assert_eq!(s.dense_compute_density, 4.0);
        assert_eq!(s.vector_min_width, 4);
        assert!(known_target_triple("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn parity_target_settings_dbvl_matches_toml() {
        // Phase 3 migration gate (targets): config/targets.dbvl's `target.*`
        // rows must produce exactly the per-prefix tuning the targets.toml
        // `[target.<prefix>]` tables produced. 2026-08-03: the .toml is
        // deleted; this is now a GOLDEN test — the pre-migration TOML tables
        // are baked below and re-parsed.
        let db_map = load_target_settings();
        let raw: toml::Value = toml::from_str(TARGET_SETTINGS_TOML_GOLDEN).unwrap();
        let toml_targets = raw.get("target").and_then(toml::Value::as_table).unwrap();

        assert_eq!(db_map.len(), toml_targets.len(),
            "target-prefix count diverges between .dbvl and .toml");
        for (prefix, value) in toml_targets {
            let t = value.as_table().unwrap();
            let settings = db_map.get(prefix)
                .unwrap_or_else(|| panic!("prefix '{}' missing from targets.dbvl", prefix));
            assert_eq!(settings.float_registers as i64,
                t.get("float_registers").and_then(toml::Value::as_integer).unwrap(),
                "float_registers for '{}' diverges", prefix);
            assert_eq!(settings.dense_compute_density,
                t.get("dense_compute_density").and_then(toml::Value::as_float).unwrap(),
                "dense_compute_density for '{}' diverges", prefix);
            assert_eq!(settings.vector_min_width as i64,
                t.get("vector_min_width").and_then(toml::Value::as_integer).unwrap(),
                "vector_min_width for '{}' diverges", prefix);
        }
    }

    #[test]
    fn test_target_settings_aarch64() {
        let s = target_settings_for("aarch64-unknown-linux-gnu");
        assert_eq!(s.float_registers, 32);
    }

    #[test]
    fn test_target_settings_wasm_unlimited() {
        let s = target_settings_for("wasm32-unknown-wasi");
        assert_eq!(s.float_registers, 4294967295);
    }

    #[test]
    fn test_unknown_target_falls_back_to_x86() {
        let s = target_settings_for("mips-unknown-linux");
        assert_eq!(s, DEFAULT_TARGET_SETTINGS);
        assert!(!known_target_triple("mips-unknown-linux"));
    }

    #[test]
    fn test_longest_prefix_wins() {
        // A more specific prefix beats a shorter one (both present).
        let s = target_settings_for("x86_64-foo");
        assert_eq!(s.float_registers, 16);
}

    // ── Axiom settings (plan 2026-08-26-axiom-facility) ──

    #[test]
    fn axioms_dbv_parses() {
        let s = load_axioms();
        // config/axioms.dbv currently ships policy: allow.
        assert_eq!(s.policy, AxiomPolicy::Allow);
        // The baked vocabulary declares commutative.
        assert!(s.lemma_properties.iter().any(|p| p == "commutative"));
    }

    #[test]
    fn axiom_policy_from_str_loose() {
        assert_eq!(AxiomPolicy::from_str_loose("allow"), AxiomPolicy::Allow);
        assert_eq!(AxiomPolicy::from_str_loose("Warn"), AxiomPolicy::Warn);
        assert_eq!(AxiomPolicy::from_str_loose(" deny "), AxiomPolicy::Deny);
        // Unknown values fall back to allow, never deny.
        assert_eq!(AxiomPolicy::from_str_loose("sometimes"), AxiomPolicy::Allow);
        assert_eq!(AxiomPolicy::from_str_loose(""), AxiomPolicy::Allow);
    }

    #[test]
    fn lemma_vocab_values_splits_tokens() {
        use crate::dbriev::v2::DataValue;
        // Bare token.
        assert_eq!(
            lemma_vocab_values(&DataValue::String("commutative".into())),
            vec!["commutative".to_string()]
        );
        // Comma-separated token (bare-token parser preserves spaces).
        assert_eq!(
            lemma_vocab_values(&DataValue::String("commutative, identity".into())),
            vec!["commutative".to_string(), "identity".to_string()]
        );
        // Vec[...] value flattens.
        assert_eq!(
            lemma_vocab_values(&DataValue::List(vec![
                DataValue::String("commutative".into()),
                DataValue::String("IDENTITY".into()),
            ])),
            vec!["commutative".to_string(), "identity".to_string()]
        );
        // Non-string values contribute nothing.
        assert!(lemma_vocab_values(&DataValue::Int(7)).is_empty());
    }
}
