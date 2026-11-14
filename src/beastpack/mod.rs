// 2026-07-25: Beastpack binary serialization module.
// Wraps the existing BEAST text format in a binary container with
// compression, checksums, and metadata stripping for distribution.
//
// The .beastpack file is the portable distribution format for
// Briev's install-time compilation pipeline. It contains the typed
// AST after the $(Typed) pipeline stage, with Source$/Comment$
// metadata stripped and internal identifiers optionally obfuscated.
//
// See docs/architecture/bounty-architecture.md for the full spec.

pub mod deserialize;
pub mod obfuscate;
pub mod serialize;
pub mod strip;

#[cfg(test)]
mod tests;

pub use deserialize::deserialize;
pub use serialize::serialize;
