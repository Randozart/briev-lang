pub mod address_space;
pub mod casing;
pub mod defn_liveness;
pub mod electronics;
pub mod strict;
pub mod task_linear;
pub mod task_segments;
pub mod call_graph;
pub mod concurrency_gate;
pub mod causality;
pub mod cross_reference;
pub mod dataflow;
pub mod dependency_graph;
pub mod gpu_schedule;
pub mod dfa;
pub mod entry_point;
pub mod equality_saturation;
pub mod pgo;
pub mod narrow_slice;
pub mod provenance;
pub mod protocol;
pub mod global_lifetime;
pub mod range;
pub mod roofline;
pub mod struct_generator;
pub mod gpu_cost;
pub mod gpu_strategy;
pub mod accel;
pub mod region;
pub mod schema_validator;
pub mod transition_graph;
pub mod watchdog;
pub mod allocation;
pub mod frgn_dispatch;
pub mod frgn_guard;
pub mod export_abi;
pub mod needs_state_projection;
pub mod boundary_ownership;
pub mod string_concat;
pub mod boundary_marshalling;
pub mod slp_isomorphism;
pub mod soa_reorder;
pub mod soa_projection;
pub mod licm;
pub mod node_decompose;
pub mod loop_carried;
pub mod layout_optimizer;
pub mod protocol_graph;
pub mod swan_song;
pub mod loop_shape;
pub mod density;
pub mod image_storage;
pub mod modulo_partition;
pub mod inline_cost;
pub mod batch_shape;
pub mod coll_length;
pub mod spawn_pool;
pub mod component_instances;
pub mod termination;
/// Determines how a state field behaves in the %State struct layout.
/// Used by the Adaptive Layout Engine (Phase 1) to eliminate unused fields
/// and allocate cache slots for meld-backed deferred projections.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldMode {
    /// Field is used and accessed directly — always present in %State.
    Always,
    /// Field has deferred projections (e.g. `strlen#` for CString .#Size)
    /// that are cached. Only used when both lenses are active in a hot loop.
    LazyCached {
        /// Index into the cache slot area appended after regular fields.
        cache_index: usize,
    },
    /// Field is never accessed through any lens — eliminated from %State.
    Never,
}

/// Expand top-level items into their effective transactions: a direct
/// `TopLevel::Transaction`, or the inner transaction of a `sync<group>` /
/// `export` wrapper. 2026-08-09 (Bug 2): sync-group-wrapped nodes were being
/// skipped by every pipeline that iterated `TopLevel::Transaction` directly —
/// the transition graph missed their fields (dead-field elimination dropped
/// them → undefined `@field` globals) and the reactor dispatch was empty
/// (nothing fired). Analyses/backends MUST iterate the effective transactions,
/// not the raw items.
pub fn effective_txns<'a>(items: &'a [crate::ast::TopLevel]) -> Vec<&'a crate::ast::Transaction> {
    let mut out = Vec::new();
    for item in items {
        match item {
            crate::ast::TopLevel::Transaction(t) => out.push(t),
            crate::ast::TopLevel::SyncGroup { item: inner, .. } => {
                if let crate::ast::TopLevel::Transaction(t) = inner.as_ref() {
                    out.push(t);
                }
            }
            crate::ast::TopLevel::Export(e) => {
                if let crate::ast::TopLevel::Transaction(t) = e.inner.as_ref() {
                    out.push(t);
                }
            }
            _ => {}
        }
    }
    out
}