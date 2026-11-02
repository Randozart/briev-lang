// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3a unified HIR fact projection. One build entry point feeds every ported
//! analysis; individual fact families remain independently testable.

use super::constructors::{build_constructor_facts, HirConstructorFacts};
use super::ownership::{build_value_facts, HirValueFacts};
use super::program::HirProgram;
use super::sinks::{build_sink_facts, HirSinkFacts};
use super::tail_calls::{build_tail_call_facts, HirTailCallFacts};
use super::type_facts::{build_type_facts, HirTypeFacts};

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
