// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//!
//! Briev's body prover, extracted so it can be used without the compiler.
//!
//! This crate holds the AST the prover reasons over and the machinery that walks
//! it. It has no parser and no backend: a consumer builds AST nodes directly,
//! which is what lets tools verify contracts without shipping the compiler.

pub mod ast;
pub mod smt;
pub mod span;
pub mod symbolic;
pub mod verify;

pub use span::Span;
