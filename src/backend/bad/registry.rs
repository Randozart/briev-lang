// ── .bad config registries — bad-isa.dbvl + bad-registers.dbvl ────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-09-21 (bad-dialect plan): baked-in registries for the .bad dialect.
// All target knowledge lives in these two config files (data, never Rust);
// the lowerer only interprets rows. Row shapes are documented at the top
// of each config file.
//
// To undo: delete this module with the rest of src/backend/bad/.

use std::collections::HashMap;

/// One core-ISA lowering for one target.
///
/// Immediate handling (`imm`):
/// - `Shared` — no `|` in the row: the reg form takes immediates too
///   (`movq $1, %rax`, `mov x0, #42`).
/// - `Form(t)` — `|t`: a distinct immediate form (`mv|li`, `add|addi`).
/// - `Illegal` — `|-`: the hardware has no immediate form; an immediate
///   operand is a loud capability error (`mul` on aarch64/riscv64).
#[derive(Debug, Clone)]
pub enum ImmHandling {
    Shared,
    Form(String),
    Illegal,
}

#[derive(Debug, Clone)]
pub struct BadIsaLowering {
    pub target: String,
    pub reg_form: String,
    pub imm: ImmHandling,
}

#[derive(Debug, Clone)]
pub struct BadIsaOp {
    pub arity: usize,
    pub lowerings: Vec<BadIsaLowering>,
    /// Rows marked `"sym"` take a label/symbol operand — unresolvable
    /// names there substitute verbatim; everywhere else an unresolvable
    /// name is a loud error (the `movq msg, %rdi` loads-memory footgun).
    pub sym: bool,
}

/// config/bad-isa.dbvl — the portable core ISA. `Op: "<arity>";
/// "target:reg-form|imm-form"; ...` (quoted mode).
#[derive(Debug, Clone)]
pub struct BadIsa {
    ops: HashMap<String, BadIsaOp>,
}

impl BadIsa {
    pub fn load() -> Self {
        let content = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/config/bad-isa.dbvl"));
        let db = crate::dbriev::config_db::ConfigDb::from_quoted_str(content)
            .unwrap_or_else(|e| panic!("config/bad-isa.dbvl parse error: {}", e));
        let mut ops = HashMap::new();
        for key in db.keys() {
            let Some(arity) = db.field_string(&key, 0).and_then(|s| s.parse::<usize>().ok())
            else {
                continue;
            };
            let (lowerings, sym) = parse_isa_row(&db, &key);
            ops.insert(
                key,
                BadIsaOp { arity, lowerings, sym },
            );
        }
        BadIsa { ops }
    }

    /// The lowering for `op` on `family`, or None (unknown op OR no row for
    /// this target — the caller distinguishes for diagnostics).
    pub fn lookup(&self, op: &str, family: &str) -> Option<&BadIsaLowering> {
        self.ops.get(op)?.lowerings.iter()
            .find(|l| family.starts_with(l.target.as_str()))
    }

    /// Whether the op is a core ISA mnemonic at all (vs a defn call or typo).
    pub fn is_core(&self, op: &str) -> bool {
        self.ops.contains_key(op)
    }

    pub fn arity(&self, op: &str) -> Option<usize> {
        self.ops.get(op).map(|o| o.arity)
    }

    /// Whether the op takes a label/symbol operand (branch targets,
    /// address-of). Unresolvable names substitute verbatim there.
    pub fn is_sym(&self, op: &str) -> bool {
        self.ops.get(op).map(|o| o.sym).unwrap_or(false)
    }

    /// Sorted core mnemonics — for diagnostics.
    pub fn known_ops(&self) -> Vec<String> {
        let mut v: Vec<String> = self.ops.keys().cloned().collect();
        v.sort();
        v
    }
}


/// One ISA row: fields 1.. are `"target:reg-form|imm-form"` lowerings; a
/// trailing `"sym"` field marks ops that take a label/symbol operand.
fn parse_isa_row(db: &crate::dbriev::config_db::ConfigDb, key: &str) -> (Vec<BadIsaLowering>, bool) {
    let mut lowerings = Vec::new();
    let mut sym = false;
    let mut idx = 1;
    while let Some(field) = db.field_string(key, idx) {
        if field == "sym" {
            sym = true;
            idx += 1;
            continue;
        }
        // Split the target prefix on the FIRST colon; the optional imm
        // form on the LAST `|` (`-` = immediates illegal on this target).
        if let Some((target, forms)) = field.split_once(':') {
            let (reg_form, imm) = match forms.rsplit_once('|') {
                Some((r, "-")) => (r.to_string(), ImmHandling::Illegal),
                Some((r, i)) => (r.to_string(), ImmHandling::Form(i.to_string())),
                None => (forms.to_string(), ImmHandling::Shared),
            };
            lowerings.push(BadIsaLowering {
                target: target.trim().to_string(),
                reg_form,
                imm,
            });
        }
        idx += 1;
    }
    (lowerings, sym)
}


/// One register/scalar row: fields are `"target:token:prop"` (register
/// rows) or `"target:value"` (imm/comment rows).
fn parse_register_row(
    db: &crate::dbriev::config_db::ConfigDb, key: &str,
) -> (Vec<BadRegEntry>, Vec<(String, String)>) {
    let mut entries = Vec::new();
    let mut pairs = Vec::new();
    let mut idx = 0;
    while let Some(field) = db.field_string(key, idx) {
        if let Some((target, rest)) = field.split_once(':') {
            let target = target.trim().to_string();
            match rest.split_once(':') {
                Some((token, prop)) => entries.push(BadRegEntry {
                    target,
                    token: token.to_string(),
                    prop: parse_prop(prop.trim()),
                }),
                None => pairs.push((target, rest.to_string())),
            }
        }
        idx += 1;
    }
    (entries, pairs)
}

/// The proof-relevant property of one portable register on one target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegProp {
    Caller,
    Callee,
    ReadOnly,
    None,
}

#[derive(Debug, Clone)]
pub struct BadRegEntry {
    pub target: String,
    pub token: String,
    pub prop: RegProp,
}

/// config/bad-registers.dbvl — portable register mapping + per-target
/// scalar rows (`imm` prefix, `comment` prefix).
#[derive(Debug, Clone)]
pub struct BadRegisters {
    regs: HashMap<String, Vec<BadRegEntry>>,
    scalars: HashMap<String, Vec<(String, String)>>,
}

impl BadRegisters {
    pub fn load() -> Self {
        let content = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/config/bad-registers.dbvl"));
        let db = crate::dbriev::config_db::ConfigDb::from_quoted_str(content)
            .unwrap_or_else(|e| panic!("config/bad-registers.dbvl parse error: {}", e));
        let mut regs = HashMap::new();
        let mut scalars = HashMap::new();
        for key in db.keys() {
            let (entries, pairs) = parse_register_row(&db, &key);
            // Rows whose fields are "target:value" scalars (not
            // "target:token:prop" register entries).
            if matches!(
                key.as_str(),
                "imm" | "comment" | "abi_args" | "push_width" | "dynamic_linker"
                    | "cross_as" | "cross_ld" | "syscall_nums"
            ) {
                scalars.insert(key, pairs);
            } else {
                regs.insert(key, entries);
            }
        }
        BadRegisters { regs, scalars }
    }

    /// The assembler token for portable register `reg` on `family`.
    pub fn resolve(&self, reg: &str, family: &str) -> Option<&str> {
        self.regs.get(reg)?.iter()
            .find(|e| family.starts_with(e.target.as_str()))
            .map(|e| e.token.as_str())
    }

    /// Width-qualified resolution: the `$N.w8`-style template refs. Looks
    /// up the `.wN` row first (x86 %al, aarch64 w0, ...), falling back to
    /// the base row (riscv stores low bits of the full register).
    pub fn resolve_w(&self, reg: &str, family: &str, width: u8) -> Option<&str> {
        let wide = self.regs.get(&format!("{reg}.w{width}"))?.iter()
            .find(|e| family.starts_with(e.target.as_str()))
            .map(|e| e.token.as_str());
        wide.or_else(|| self.resolve(reg, family))
    }

    /// The proof property for `reg` on `family`.
    pub fn property(&self, reg: &str, family: &str) -> Option<RegProp> {
        self.regs.get(reg)?.iter()
            .find(|e| family.starts_with(e.target.as_str()))
            .map(|e| e.prop)
    }

    /// Whether the register exists on `family` at all.
    pub fn exists(&self, reg: &str, family: &str) -> bool {
        self.resolve(reg, family).is_some()
    }

    /// Every target this register maps to — diagnostics for unmapped regs.
    pub fn known_targets(&self, reg: &str) -> Vec<String> {
        self.regs.get(reg)
            .map(|es| es.iter().map(|e| e.target.clone()).collect())
            .unwrap_or_default()
    }

    fn scalar(&self, key: &str, family: &str) -> Option<&str> {
        self.scalars.get(key)?.iter()
            .find(|(t, _)| family.starts_with(t.as_str()))
            .map(|(_, v)| v.as_str())
    }

    /// C-ABI argument-register order (portable names) per target.
    pub fn abi_args(&self, family: &str) -> Vec<String> {
        self.scalar("abi_args", family)
            .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_default()
    }

    /// The portable `push` stack decrement per target.
    pub fn push_width(&self, family: &str) -> i64 {
        self.scalar("push_width", family).and_then(|s| s.parse().ok()).unwrap_or(16)
    }

    /// The dynamic-linker path for --with-libc runs.
    pub fn dynamic_linker(&self, family: &str) -> Option<&'static str> {
        self.scalar("dynamic_linker", family).map(leak_static)
    }

    /// The assembler binary for `family` (host `as` for x86_64, prefixed
    /// cross-binutils for the others).
    pub fn cross_as(&self, family: &str) -> Option<&'static str> {
        self.scalar("cross_as", family).map(leak_static)
    }

    /// The linker binary for `family`.
    pub fn cross_ld(&self, family: &str) -> Option<&'static str> {
        self.scalar("cross_ld", family).map(leak_static)
    }

    /// Kernel-call number for a NAME on `family` (`write` → 1 on x86_64,
    /// 64 on aarch64). Unknown name = None (caller raises loudly).
    pub fn syscall_number(&self, family: &str, name: &str) -> Option<i64> {
        self.scalar("syscall_nums", family)?
            .split(',')
            .find_map(|pair| {
                let (n, v) = pair.split_once('=')?;
                (n.trim() == name).then(|| v.trim().parse().ok())?
            })
    }

    /// Every syscall name known for `family` — diagnostics.
    pub fn known_syscalls(&self, family: &str) -> Vec<String> {
        self.scalar("syscall_nums", family)
            .map(|s| {
                let mut v: Vec<String> = s.split(',')
                    .filter_map(|p| p.split_once('=').map(|(n, _)| n.trim().to_string()))
                    .collect();
                v.sort();
                v
            })
            .unwrap_or_default()
    }

    /// Immediate-literal prefix per target (`$` / `#` / empty).
    pub fn imm_prefix(&self, family: &str) -> &'static str {
        match self.scalar("imm", family) {
            Some(p) => leak_static(p),
            None => "",
        }
    }

    /// GAS comment prefix per target.
    pub fn comment_prefix(&self, family: &str) -> &'static str {
        match self.scalar("comment", family) {
            Some(p) => leak_static(p),
            None => "#",
        }
    }
}

fn parse_prop(s: &str) -> RegProp {
    match s {
        "caller" => RegProp::Caller,
        "callee" => RegProp::Callee,
        "ro" => RegProp::ReadOnly,
        _ => RegProp::None,
    }
}

/// Scalar values are compile-time-baked config rows; the strings live for
/// the program's whole life. Promoting to 'static avoids threading a
/// lifetime through every call site for data that outlives everything.
fn leak_static(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_isa_loads_addr_sym_marker() {
        let isa = BadIsa::load();
        assert!(isa.is_core("addr"), "addr row missing");
        assert_eq!(isa.arity("addr"), Some(2));
        assert!(isa.is_sym("addr"), "addr row must carry the sym marker");
        assert!(isa.is_sym("jmp") && isa.is_sym("jz") && isa.is_sym("jnz") && isa.is_sym("call"));
        assert!(!isa.is_sym("mov"));
        let l = isa.lookup("addr", "x86_64").expect("addr x86_64 lowering");
        assert!(l.reg_form.contains("leaq"));
    }

    #[test]
    fn bad_registers_load() {
        let regs = BadRegisters::load();
        assert_eq!(regs.resolve("r0", "x86_64"), Some("%rax"));
        assert_eq!(regs.resolve("r14", "x86_64"), None, "r14 has no x86_64 slot");
        assert_eq!(regs.resolve("r14", "aarch64"), Some("x14"));
        assert_eq!(regs.property("r10", "x86_64"), Some(RegProp::Callee));
        assert_eq!(regs.property("r0", "x86_64"), Some(RegProp::Caller));
        assert_eq!(regs.imm_prefix("x86_64"), "$");
        assert_eq!(regs.imm_prefix("aarch64"), "#");
        assert_eq!(regs.imm_prefix("riscv64"), "");
    }
}
