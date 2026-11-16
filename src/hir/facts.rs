// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3a unified HIR fact projection. One build entry point feeds every ported
//! analysis; individual fact families remain independently testable.

use super::constructors::{HirConstructorFacts, build_constructor_facts};
use super::ownership::{HirValueFacts, build_value_facts};
use super::program::HirProgram;
use super::sinks::{HirSinkFacts, build_sink_facts};
use super::tail_calls::{HirTailCallFacts, build_tail_call_facts};
use super::type_facts::{HirTypeFacts, build_type_facts};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HirFacts {
    pub values: HirValueFacts,
    pub sinks: HirSinkFacts,
    pub types: HirTypeFacts,
    pub constructors: HirConstructorFacts,
    pub tail_calls: HirTailCallFacts,
}

pub fn build_facts(program: &HirProgram) -> HirFacts {
    HirFacts {
        values: build_value_facts(program),
        sinks: build_sink_facts(program),
        types: build_type_facts(program),
        constructors: build_constructor_facts(program),
        tail_calls: build_tail_call_facts(program),
    }
}
