// 2026-07-25: VM backend — top-level item bytecode emission.
// Converts TopLevel items (Defn, Txn, TypeDef, etc.) into .lair functions.

use super::VmBackend;
use crate::ast::top::*;
use crate::ast::*;

impl VmBackend {
    pub(crate) fn emit_toplevel(&mut self, item: &TopLevel) {
        // 2026-07-25: Unwrap export wrappers to compile exported defns.
        let inner = match item {
            TopLevel::Export(e) => &e.inner,
            other => other,
        };
        match inner {
            TopLevel::Definition(d) => self.emit_definition(d),
            TopLevel::Transaction(t) => self.emit_transaction(t),
            TopLevel::TypeDef(_) => {}
            TopLevel::Constant(c) => self.emit_constant(c),
            _ => {}
        }
    }

    fn emit_definition(&mut self, d: &Definition) {
        // Map parameters to local slots
        self.local_slots.clear();
        self.ptr_slots.clear();
        self.next_local_slot = 0;
        self.current_fn = d.name.clone();
        for (name, ty) in &d.parameters {
            self.local_slots.insert(name.clone(), self.next_local_slot);
            // 2026-07-25: Track Ptr<Int> slots for pointer arithmetic scaling.
            if matches!(ty, Type::Ptr(_) | Type::PtrConst(_)) {
                self.ptr_slots.insert(self.next_local_slot);
            }
            self.next_local_slot += 1;
        }
        // 2026-07-25: Count total needed locals (params + let bindings in body)
        // by pre-scanning the body.
        let total_locals = count_let_bindings(&d.body) as u16 + d.parameters.len() as u16;

        // Also register the function name in fn_indices for call resolution
        self.fn_index_counter += 1;
        // The assembler tracks function indices in order of define_function calls

        self.asm.define_function(
            &d.name,
            total_locals,              // local_count (params + let bindings)
            d.parameters.len() as u16, // arg_count
        );

        // Emit body statements
        for stmt in &d.body {
            self.emit_stmt(stmt);
        }

        // If the function doesn't end with ret, add one
        // (Functions with no explicit term/return still need to return)
        self.asm.emit_ret();
    }

    fn emit_transaction(&mut self, t: &Transaction) {
        // Transactions are like definitions with contracts
        self.local_slots.clear();
        self.ptr_slots.clear();
        self.next_local_slot = 0;
        self.current_fn = t.name.clone();
        for (name, ty) in &t.parameters {
            self.local_slots.insert(name.clone(), self.next_local_slot);
            // 2026-07-25: Track Ptr<Int> slots for pointer arithmetic scaling.
            if matches!(ty, Type::Ptr(_) | Type::PtrConst(_)) {
                self.ptr_slots.insert(self.next_local_slot);
            }
            self.next_local_slot += 1;
        }
        // 2026-07-25: Count total needed locals (params + let bindings in body)
        let total_locals = count_let_bindings(&t.body) as u16 + t.parameters.len() as u16;

        self.asm
            .define_function(&t.name, total_locals, t.parameters.len() as u16);

        for stmt in &t.body {
            self.emit_stmt(stmt);
        }

        self.asm.emit_ret();
    }

    fn emit_constant(&mut self, c: &Constant) {
        // Constants are compile-time values. For the VM, emit a function
        // that returns the constant's value.
        self.local_slots.clear();
        self.next_local_slot = 0;
        self.current_fn = c.name.clone();

        self.asm.define_function(&c.name, 0, 0);
        self.emit_expr(&c.expr);
        self.asm.emit_ret();
    }
}

/// 2026-07-25: Count the number of let bindings in a statement body.
/// Used to pre-allocate sufficient local slots for functions.
fn count_let_bindings(body: &[crate::ast::top::Statement]) -> usize {
    body.iter()
        .filter(|s| matches!(s, crate::ast::top::Statement::Let { .. }))
        .count()
}
