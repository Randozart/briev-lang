// ── .bld intrinsic registry loader ────────────────────────────────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-10-08 (BILLD plan M5): parse config/bld-intrinsics.dbvl — one row
// per engine verb: arity, result flag, per-target `.bad` instruction
// sequences. Pure data (Rules 3/15/23): the lowerer consults this table
// generically; nothing in Rust knows an individual verb's name.
//
// To undo: delete this file and config/bld-intrinsics.dbvl.

use std::collections::HashMap;

/// One target's inline lowering: a `;`-separated sequence of `.bad`
/// instruction lines with `$N` / `$N!` argument slots.
#[derive(Debug, Clone)]
pub struct IntrinsicLowering {
    pub target: String,
    pub seq: String,
}

/// One engine verb.
#[derive(Debug, Clone)]
pub struct BldIntrinsic {
    pub arity: usize,
    /// The sequence leaves the result in r0 (the .bld return convention).
    pub ret: bool,
    pub lowerings: Vec<IntrinsicLowering>,
}

pub struct BldIntrinsics {
    intrinsics: HashMap<String, BldIntrinsic>,
}

impl BldIntrinsics {
    pub fn load() -> Self {
        let content =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/config/bld-intrinsics.dbvl"));
        let db = crate::dbriev::config_db::ConfigDb::from_quoted_str(content)
            .unwrap_or_else(|e| panic!("config/bld-intrinsics.dbvl parse error: {}", e));
        let mut intrinsics = HashMap::new();
        for key in db.keys() {
            let Some(arity) = db.field_string(&key, 0).and_then(|s| s.parse::<usize>().ok())
            else {
                continue;
            };
            let ret = matches!(db.field_string(&key, 1).map(|s| s.trim()), Some("ret"));
            let lowerings = parse_lowerings(&db, &key);
            intrinsics.insert(
                key,
                BldIntrinsic { arity, ret, lowerings },
            );
        }
        BldIntrinsics { intrinsics }
    }

    /// The lowering for `name` on `family`, or None (unknown verb OR no
    /// row for this target — the caller distinguishes for diagnostics).
    pub fn lookup(&self, name: &str, family: &str) -> Option<&IntrinsicLowering> {
        self.intrinsics.get(name)?.lowerings.iter()
            .find(|l| family.starts_with(l.target.as_str()))
    }

    /// Whether the name is an engine verb at all (vs a defn or typo).
    pub fn is_intrinsic(&self, name: &str) -> bool {
        self.intrinsics.contains_key(name)
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.intrinsics.get(name).map(|i| i.arity)
    }

    pub fn returns(&self, name: &str) -> Option<bool> {
        self.intrinsics.get(name).map(|i| i.ret)
    }

    /// The targets a verb provides, sorted — for capability errors.
    pub fn targets(&self, name: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .intrinsics
            .get(name)
            .map(|i| i.lowerings.iter().map(|l| l.target.clone()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }
}

/// Fields 2.. are `"target:instr; instr"` lowerings.
fn parse_lowerings(
    db: &crate::dbriev::config_db::ConfigDb,
    key: &str,
) -> Vec<IntrinsicLowering> {
    let mut lowerings = Vec::new();
    let mut idx = 2;
    while let Some(field) = db.field_string(key, idx) {
        if let Some((target, seq)) = field.split_once(':') {
            lowerings.push(IntrinsicLowering {
                target: target.trim().to_string(),
                seq: seq.trim().to_string(),
            });
        }
        idx += 1;
    }
    lowerings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_load_with_arity_and_flag() {
        let r = BldIntrinsics::load();
        assert_eq!(r.arity("Halt"), Some(0));
        assert_eq!(r.returns("Halt"), Some(false));
        assert_eq!(r.arity("ReadControlReg"), Some(1));
        assert_eq!(r.returns("ReadControlReg"), Some(true));
        assert_eq!(r.arity("WriteControlReg"), Some(2));
    }

    #[test]
    fn lookup_resolves_by_family_prefix() {
        let r = BldIntrinsics::load();
        assert!(r.lookup("Halt", "x86_64-unknown-none").is_some());
        assert!(r.lookup("Halt", "aarch64").is_some());
        assert!(r.lookup("ReadControlReg", "x86_64").is_some());
        assert!(r.lookup("ReadControlReg", "riscv64").is_some());
    }

    #[test]
    fn missing_target_row_is_none() {
        let r = BldIntrinsics::load();
        assert!(r.lookup("ReadControlReg", "aarch64").is_none());
        assert!(r.lookup("FarJump", "riscv64").is_none());
        assert!(r.lookup("FarJump", "aarch64").is_none());
        assert!(r.lookup("LoadDescriptorTable", "riscv64").is_none());
    }

    #[test]
    fn targets_are_sorted_for_diagnostics() {
        let r = BldIntrinsics::load();
        assert_eq!(r.targets("ReadControlReg"), vec!["riscv64", "x86_64"]);
    }
}
