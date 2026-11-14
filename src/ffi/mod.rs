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

//! Metropolitan FFI
//!
//! Two mechanisms:
//! - **GLUE** (compile-time bridge generation): `lib/glue/<lang>/glue.dbv` + `src/glue/`
//! - **Metropipe** (runtime shared memory IPC): `src/ffi/metropipe.rs`

pub mod error;
pub mod metropipe; // Metropipe — shared memory IPC runtime
pub mod metropipe_cli; // `briev metrod connect` CLI

pub use error::{ErrVariant, ErrorConventions, generate_bounds_check, generate_null_check};
pub use metropipe::{MetroStatus, MetropolitanChannel, MetropolitanHub, SharedRegion};
