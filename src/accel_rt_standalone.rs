// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Standalone staticlib wrapper for the accel orchestrator (Family K):
//! `rustc --crate-type staticlib --crate-name briev_accel_rt` on this file
//! yields `libbriev_accel_rt.a` exporting the `briev_accel_*` C ABI — the
//! archive runners and benchmark binaries link instead of the former
//! single-TU C source. Kept to this wrapper so the orchestrator module
//! builds both inside the compiler crate and standalone from ONE source.

#[path = "accel_rt.rs"]
mod accel_rt;
