#![allow(unused)]
#![allow(unused_variables)]
// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Runtime Exception for Use as a Language:
// When the Work or any Derivative Work thereof is used to generate code
// ("generated code"), such generated code shall not be subject to the
// terms of this License, provided that the generated code itself is not
// a Derivative Work of the Work. This exception does not apply to code
// that is itself a compiler, interpreter, or similar tool that incorporates
// or embeds the Work.

pub mod accel_rt;
pub mod annotator;
pub mod address_resolver;
pub mod casting;
pub mod assertion_verify;
pub mod ast;
pub mod analysis;
pub mod archive;
pub mod backend;
pub mod beast;
pub mod bounty;
pub mod beastpack;
pub mod config;
pub mod config_resolver;
pub mod config_tuning;
pub mod target;
pub mod conformance;
pub mod vocab;
pub mod type_universe;
pub mod cache;
pub mod dbriev;
pub mod derive;
pub mod desugarer;
pub mod encoding_registry;
pub mod errors;
pub mod ffi;
pub mod fuzz_checker;
pub mod glue;
pub mod gpu_rt;
#[cfg(test)]
pub mod fuzzing;
pub mod hardware;
pub mod hardware_validator;
pub mod import_resolver;
pub mod intrinsic_signatures;
pub mod interpreter;
pub mod layout;
pub mod lexer;
pub mod library;
pub mod lifetime;
pub mod linkage;
pub mod lsp;
pub mod manifest;
pub mod memory_spec;
pub mod parser;
pub mod pipeline;
pub mod normalize_types;
pub mod plugin;
pub mod proof_engine;
pub mod protocol_verify;
pub mod rbv;
pub mod reactor;
pub mod resolver;
pub mod scheduler;
pub mod signal_graph;
pub mod ssr;
pub mod symbolic;
pub mod target_spec;
pub mod typechecker;
pub mod view_compiler;
pub mod macros;
pub mod watch;
pub mod wrapper;

pub mod doc;

pub mod optimizer;
pub mod registry;

