// ── Program Diff for Dry-Run ──────────────────────────────────────────
// 2026-07-23: Produces a human-readable diff between two Vec<TopLevel>
// programs. Used by --diff to show what macros changed without committing.

use crate::ast::TopLevel;
use std::collections::{HashMap, HashSet};

/// A single diff entry between two programs.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffEntry {
    /// Item appears in `after` but not in `before`. Second field is index in `after`.
    Added(usize, String),
    /// Item appears in `before` but not in `after`. Second field is index in `before`.
    Removed(usize, String),
    /// Item appears in both with a different body. Indices in `before` and `after`.
    Modified(usize, usize, String, String),
}

/// Build a stable key for a TopLevel item. Two items with the same key are
/// considered "the same item" for diff purposes. Relies on name or path.
fn item_key(tl: &TopLevel) -> String {
    use TopLevel::*;
    match tl {
        Import(i) => format!("import:{}", i.path()),
        Definition(d) => format!("defn:{}", d.name),
        TypeDefOperator(d) => format!("op:{}", d.name),
        Transaction(t) => format!("txn:{}", t.name),
        Budget(b) => format!("budget@{:?}", b.span),
        // 2026-09-22 (Slice B): participation facts.
        Unpop(d) => format!("unpop:{}", d.instance),
        ShortCircuit(d) => format!("shortcircuit:{}", d.instance),
        // 2026-09-22 (Slice C): the static when law — keyed by the guard.
        WhenLaw(w) => format!("when-law@{}", w.guard),
        Cell(c) => format!("cell:{}", c.name),
        ForeignBinding(f) => format!("frgn:{}", f.effective_briev_name()),
        Export(e) => format!("export:{}", e.export_name.as_deref().unwrap_or("_")),
        Constant(c) => format!("constant:{}", c.name),
        Obj(s) => format!("obj:{}", s.name),
        StaticStruct(s) => format!("struct:{}", s.name),
        Enum(e) => format!("enum:{}", e.name),
        TriggerBinding { name, .. } => format!("trigger-binding:{}", name),
        Trigger(t) => format!("trigger:{}", t.name),
        StateDecl(s) => format!("state:{}", s.name),
        Signature(s) => format!("signature:{}", s.name),
        LinkDependency(l) => format!("link-dep:{}", l.path),
        ResourceDecl(r) => format!("resource:{}", r.name),
        TypeDef(t) => format!("typedef:{}", t.name),
        Trait(t) => format!("trait:{}", t.name),
        Impl(i) => format!("impl:{}", i.target),
        Codec(c) => format!("codec:{}", c.name),
        Assertion { .. } => "assertion".into(),
        Fuzzed { .. } => "fuzzed".into(),
        Statement(_) | Stylesheet(_) | SvgComponent { .. } | SyncGroup { .. } | StageBlock(_) | RenderBlock(_) | Cfg(_) => {
            format!("{:?}", tl)
        }
        AsmFn(a) => format!("asm:<{}> {}", a.target, a.name),
        BadFn(bf) => format!("bad {}", bf.name),
        IsrHandler(h) => format!("isr:{}", h.name),
        CompileTimeDefn(d) => format!("$defn:{}", d.name),
        CompileTimeTxn(t) => format!("$txn:{}", t.name),
        CompileTimeLet(name, _) => format!("$let:{}", name),
        CompileTimeConst(name, _) => format!("$const:{}", name),
        ProtocolDef(p) => format!("protocol:{}", p.name),
        ModuleMetadata(_) => "module-metadata".into(),
        Init(i) => format!("init:{}", i.name),
    }
}

/// Build a short human-readable summary for display.
pub fn item_summary(tl: &TopLevel) -> String {
    use TopLevel::*;
    match tl {
        Import(i) => format!("import \"{}\"", i.path()),
        Definition(d) => format!("defn {}", d.name),
        TypeDefOperator(d) => format!("op {}", d.name),
        Transaction(t) => format!("txn {}", t.name),
        Budget(_) => "budget <source-pin limit>".to_string(),
        // 2026-09-22 (Slice B): participation facts.
        Unpop(d) => format!("unpop {}", d.instance),
        ShortCircuit(d) => format!("shortcircuit {}", d.instance),
        // 2026-09-22 (Slice C): the static when law.
        WhenLaw(_) => "when <forced-fact law>".to_string(),
        Cell(c) => format!("cell {}", c.name),
        ForeignBinding(f) => format!("frgn {}", f.foreign_name),
        Export(e) => format!("export {}", e.export_name.as_deref().unwrap_or("_")),
        Constant(c) => format!("constant {}", c.name),
        Obj(s) => format!("obj {}", s.name),
        StaticStruct(s) => format!("struct {}", s.name),
        Enum(e) => format!("enum {}", e.name),
        TriggerBinding { name, .. } => format!("trigger-binding {}", name),
        Trigger(t) => format!("trigger {}", t.name),
        StateDecl(s) => format!("state {}", s.name),
        Signature(s) => format!("signature {}", s.name),
        LinkDependency(l) => format!("link-dependency {}", l.path),
        ResourceDecl(r) => format!("resource {}", r.name),
        TypeDef(t) => format!("typedef {}", t.name),
        Trait(t) => format!("trait {}", t.name),
        Impl(i) => format!("impl {}", i.target),
        Codec(c) => format!("codec {}", c.name),
        Assertion { .. } => "assertion".into(),
        Fuzzed { .. } => "fuzzed".into(),
        Statement(_) => "statement".into(),
        StageBlock(sb) => format!("$({:?})", sb.stage),
        RenderBlock(_) => "render".into(),
        Stylesheet(_) => "stylesheet".into(),
        SvgComponent { name, .. } => format!("svg {}", name),
        SyncGroup { .. } => "sync-group".into(),
        Cfg(_) => "cfg".into(),
        CompileTimeDefn(d) => format!("$defn {}", d.name),
        CompileTimeTxn(t) => format!("$txn {}", t.name),
        CompileTimeLet(name, _) => format!("$let {}", name),
        CompileTimeConst(name, _) => format!("$const {}", name),
        AsmFn(a) => format!("asm:<{}> {}", a.target, a.name),
        BadFn(bf) => format!("bad {}", bf.name),
        IsrHandler(h) => format!("isr:{}", h.name),
        ProtocolDef(p) => format!("proto {}: #{}", p.name, p.category),
        ModuleMetadata(meta) => format!("module metadata ({} keys)", meta.len()),
        Init(i) => format!("init {}", i.name),
    }
}

/// Compute a diff between two programs.
pub fn compute_diff(before: &[TopLevel], after: &[TopLevel]) -> Vec<DiffEntry> {
    let mut result = Vec::new();
    let before_keys: HashMap<String, (usize, &TopLevel)> = before.iter()
        .enumerate()
        .map(|(i, tl)| (item_key(tl), (i, tl)))
        .collect();
    let mut seen_in_after: HashSet<usize> = HashSet::new();

    for (after_idx, after_item) in after.iter().enumerate() {
        let key = item_key(after_item);
        if let Some(&(before_idx, before_item)) = before_keys.get(&key) {
            seen_in_after.insert(before_idx);
            let before_debug = format!("{:?}", before_item);
            let after_debug = format!("{:?}", after_item);
            if before_debug != after_debug {
                result.push(DiffEntry::Modified(
                    before_idx,
                    after_idx,
                    item_summary(before_item),
                    item_summary(after_item),
                ));
            }
        } else {
            result.push(DiffEntry::Added(after_idx, item_summary(after_item)));
        }
    }

    for (before_idx, before_item) in before.iter().enumerate() {
        if !seen_in_after.contains(&before_idx) {
            result.push(DiffEntry::Removed(before_idx, item_summary(before_item)));
        }
    }

    result.sort_by_key(|e| match e {
        DiffEntry::Added(i, _) => *i,
        DiffEntry::Removed(i, _) => *i,
        DiffEntry::Modified(_, i, _, _) => *i,
    });
    result
}

/// Print a diff in human-readable format.
pub fn print_diff(diff: &[DiffEntry]) {
    if diff.is_empty() {
        println!("(no changes)");
        return;
    }
    for entry in diff {
        match entry {
            DiffEntry::Added(idx, summary) => {
                println!("  + [{}] {}", idx, summary);
            }
            DiffEntry::Removed(idx, summary) => {
                println!("  - [{}] {}", idx, summary);
            }
            DiffEntry::Modified(before_idx, after_idx, before_summary, after_summary) => {
                println!("  ~ [{}→{}] {} → {}", before_idx, after_idx, before_summary, after_summary);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    fn make_import(path: &str) -> TopLevel {
        TopLevel::Import(Import::literal(path, vec![]))
    }

    fn make_defn(name: &str) -> TopLevel {
        TopLevel::Definition(Definition {
            name: name.into(),
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body: vec![],
            metadata: Default::default(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })
    }

    #[test]
    fn test_diff_identical() {
        let before = vec![make_import("std/io.bv")];
        let after = vec![make_import("std/io.bv")];
        let diff = compute_diff(&before, &after);
        assert!(diff.is_empty(), "identical programs should have no diff");
    }

    #[test]
    fn test_diff_added() {
        let before = vec![make_import("std/io.bv")];
        let after = vec![
            make_import("std/io.bv"),
            make_import("std/foo.bv"),
        ];
        let diff = compute_diff(&before, &after);
        assert_eq!(diff.len(), 1, "one addition expected");
        match &diff[0] {
            DiffEntry::Added(idx, summary) => {
                assert_eq!(*idx, 1);
                assert!(summary.contains("foo"), "summary should mention added item");
            }
            other => panic!("expected Added, got {:?}", other),
        }
    }

    #[test]
    fn test_diff_removed() {
        let before = vec![
            make_import("std/io.bv"),
            make_import("std/net.bv"),
        ];
        let after = vec![make_import("std/io.bv")];
        let diff = compute_diff(&before, &after);
        assert_eq!(diff.len(), 1, "one removal expected");
        match &diff[0] {
            DiffEntry::Removed(idx, summary) => {
                assert_eq!(*idx, 1);
                assert!(summary.contains("net"), "summary should mention removed item");
            }
            other => panic!("expected Removed, got {:?}", other),
        }
    }

    #[test]
    fn test_diff_modified() {
        let before = vec![make_defn("foo")];
        let mut after_item = make_defn("foo");
        if let TopLevel::Definition(d) = &mut after_item {
            d.contract = Contract::new(Expr::Bool(false), Expr::Bool(true));
        }
        let after = vec![after_item];
        let diff = compute_diff(&before, &after);
        assert!(!diff.is_empty(), "modified program should have a diff");
        let has_modified = diff.iter().any(|e| matches!(e, DiffEntry::Modified(..)));
        assert!(has_modified, "should detect modification");
    }

    #[test]
    fn test_diff_ordering() {
        let before = vec![make_import("std/a.bv"), make_import("std/b.bv")];
        let after = vec![
            make_import("std/c.bv"),
            make_import("std/a.bv"),
        ];
        let diff = compute_diff(&before, &after);
        let adds: Vec<_> = diff.iter().filter(|e| matches!(e, DiffEntry::Added(..))).collect();
        let removes: Vec<_> = diff.iter().filter(|e| matches!(e, DiffEntry::Removed(..))).collect();
        assert_eq!(adds.len(), 1, "one addition");
        assert_eq!(removes.len(), 1, "one removal");
    }
}
