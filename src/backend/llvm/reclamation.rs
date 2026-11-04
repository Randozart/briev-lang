//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Consume frontend region proofs; never infer ownership from handle types.
//! 2026-11-04: unknown callables suspend inheritance. Explicit free/realloc
//! update private records, preserving borrowed storage and allocator ABI.
use crate::analysis::reclamation::ReclamationPlan;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

pub(super) fn lower(module: String, plan: &ReclamationPlan) -> String {
    if (!plan.entry_region && plan.regions.is_empty()) || module.contains("musttail ") || !has_heap_calls(&module) { return module; }
    let groups = EffectGroups::build(&module);
    let mut lowering = ModuleLowering { plan, groups: &groups, output: String::with_capacity(module.len()), mode: None, entry: false };
    for line in module.lines() { lowering.line(line); }
    lowering.output.push_str(&groups.definitions);
    for line in include_str!("reclamation.ll").lines().filter(|line| !line.starts_with(';')) { writeln!(lowering.output, "{line}").ok(); }
    lowering.output
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Region, Barrier, Inherit }

struct ModuleLowering<'a> {
    plan: &'a ReclamationPlan,
    groups: &'a EffectGroups,
    output: String,
    mode: Option<Mode>,
    entry: bool,
}

impl ModuleLowering<'_> {
    fn line(&mut self, line: &str) {
        if line.starts_with("define ") { self.definition(line); return; }
        if line == "}" { self.mode = None; }
        self.boundaries(line);
        let line = if self.mode.is_some() { rewrite_allocator_call(line) } else { line.to_string() };
        writeln!(self.output, "{line}").ok();
    }

    fn definition(&mut self, line: &str) {
        if line.split_whitespace().any(|token| token == "naked") {
            self.mode = None; self.entry = false; writeln!(self.output, "{line}").ok(); return;
        }
        let name = function_name(line);
        self.mode = name.map(|name| mode_for(self.plan, name));
        self.entry = self.mode.is_some_and(|mode| mode != Mode::Inherit);
        let changes_effects = self.entry || name.is_some_and(|name| self.plan.mutating.contains(name));
        let scoped = self.mode == Some(Mode::Region);
        let line = if scoped { strip_inline_attributes(line) } else { line.into() };
        let mut line = if changes_effects { self.groups.rewrite_reference(&line, scoped) } else { line };
        if scoped && !line.contains(" #") { line = line.replacen('{', "noinline {", 1); }
        writeln!(self.output, "{line}").ok();
    }

    fn boundaries(&mut self, line: &str) {
        if self.mode.is_some() && line.trim() == "; briev.program_end" {
            writeln!(self.output, "  call void @__briev_region_finish()").ok();
        }
        if self.entry && is_instruction(line) {
            emit_entry(&mut self.output, self.mode.unwrap()); self.entry = false;
        }
        if !line.trim_start().starts_with("ret ") { return; }
        if let Some(mode) = self.mode { emit_exit(&mut self.output, mode); }
    }
}

fn mode_for(plan: &ReclamationPlan, name: &str) -> Mode {
    // `main` is the compiler-owned process driver, not a source node name.
    if name == "main" && plan.entry_region { return Mode::Region; }
    if plan.regions.contains(name) { return Mode::Region; }
    if plan.barriers.contains(name) || !plan.known.contains(name) { return Mode::Barrier; }
    Mode::Inherit
}

fn function_name(line: &str) -> Option<&str> { line.split_once('@')?.1.split_once('(').map(|(name, _)| name) }

fn is_instruction(line: &str) -> bool {
    let line = line.split(';').next().unwrap_or("").trim();
    !line.is_empty() && !line.starts_with(';') && !line.ends_with(':')
}

#[derive(Default)]
struct EffectGroups {
    references: BTreeMap<u64, u64>,
    scoped_references: BTreeMap<u64, u64>,
    definitions: String,
}

impl EffectGroups {
    fn build(module: &str) -> Self {
        let ids: BTreeSet<_> = attribute_pattern().captures_iter(module)
            .filter_map(|captures| captures.get(1).and_then(|id| id.as_str().parse::<u64>().ok())).collect();
        let mut next = ids.iter().max().copied().unwrap_or(0) + 1;
        let mut result = Self::default();
        for id in &ids {
            result.references.insert(*id, next); result.scoped_references.insert(*id, next+1); next += 2;
        }
        let mut defined = BTreeSet::new();
        let mut inside = false;
        let mut scoped = String::new();
        let mut scoped_id = 0;
        for line in module.lines() {
            let mut header = None;
            if let Some(id) = attribute_id(line) {
                defined.insert(id); header = Some((id, result.references[&id]));
                scoped_id = result.scoped_references[&id]; inside = true;
            }
            if !inside { continue; }
            let mut line = strip_effect_attributes(line);
            let mut scoped_line = strip_inline_attributes(&line);
            if let Some((old, new)) = header { line = line.replacen(&format!("#{old}"), &format!("#{new}"), 1); }
            if let Some((old, _)) = header {
                scoped_line = scoped_line.replacen(&format!("#{old}"), &format!("#{scoped_id}"), 1).replacen('{', "{ noinline", 1);
            }
            writeln!(result.definitions, "{line}").ok();
            writeln!(scoped, "{scoped_line}").ok();
            if line.trim_end().ends_with('}') { inside = false; result.definitions.push_str(&scoped); scoped.clear(); }
        }
        for id in ids.difference(&defined) { result.neutral_groups(*id); }
        result
    }

    fn neutral_groups(&mut self, id: u64) {
        // Reserve referenced-but-undefined IDs too: assigning a new group to
        // one would silently change every old reference (including recursion).
        writeln!(self.definitions, "attributes #{id} = {{ }}").ok();
        writeln!(self.definitions, "attributes #{} = {{ }}", self.references[&id]).ok();
        writeln!(self.definitions, "attributes #{} = {{ noinline }}", self.scoped_references[&id]).ok();
    }

    fn rewrite_reference(&self, line: &str, scoped: bool) -> String {
        let references = if scoped { &self.scoped_references } else { &self.references };
        attribute_pattern().replace_all(line, |captures: &regex::Captures<'_>| {
            let Some(id) = captures.get(1) else { return captures[0].to_string() };
            let id = id.as_str().parse::<u64>().unwrap();
            format!("#{}", references.get(&id).unwrap_or(&id))
        }).into_owned()
    }
}

fn attribute_pattern() -> &'static regex::Regex {
    static REFERENCE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    REFERENCE.get_or_init(|| regex::Regex::new(r#""(?:\\.|[^"\\])*"|;[^\n]*|#([0-9]+)"#).unwrap())
}

fn attribute_id(line: &str) -> Option<u64> { line.strip_prefix("attributes #")?.split_whitespace().next()?.parse().ok() }

fn strip_inline_attributes(line: &str) -> String {
    // 2026-11-04: retaining a region's private frame in its own call prevents
    // an inlined TLS-visible alloca from disabling the caller's tail loop.
    // Retire noinline only with a lowering whose frame lifetime survives TCE.
    static INLINE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    INLINE.get_or_init(|| regex::Regex::new(r#""(?:\\.|[^"\\])*"|\b(?:alwaysinline|inlinehint|noinline)\b"#).unwrap()).replace_all(line, |captures: &regex::Captures<'_>| {
        let value = captures.get(0).unwrap().as_str();
        if value.starts_with('"') { value.to_string() } else { String::new() }
    }).into_owned()
}

fn strip_effect_attributes(line: &str) -> String {
    // Region calls invalidate memory-effect summaries, not CPU features,
    // inlining policy or ABI. Quoted attribute values are preserved verbatim.
    static EFFECT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let expression = r#""(?:\\.|[^"\\])*"|\bmemory\([^)]*\)|\b(?:nofree|nosync|speculatable|readnone|readonly|argmemonly|inaccessiblememonly|inaccessiblemem_or_argmemonly|nocallback)\b"#;
    EFFECT.get_or_init(|| regex::Regex::new(expression).unwrap()).replace_all(line, |captures: &regex::Captures<'_>| {
        let value = captures.get(0).unwrap().as_str();
        if value.starts_with('"') { value.to_string() } else { String::new() }
    }).into_owned()
}

fn allocator_pattern() -> &'static regex::Regex {
    static CALL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CALL.get_or_init(|| regex::Regex::new(r"^(\s*(?:%[^\s=]+ = )?(?:(?:tail|notail) )?call (?:noalias )?(?:ptr|void) )@(malloc|free|realloc)\(").unwrap())
}

fn has_heap_calls(module: &str) -> bool {
    module.lines().any(|line| allocator_pattern().captures(line).is_some_and(|captures| &captures[2] == "malloc"))
}

fn rewrite_allocator_call(line: &str) -> String {
    allocator_pattern().replace(line, |captures: &regex::Captures<'_>| {
        format!("{}@__briev_region_{}(", &captures[1], &captures[2])
    }).into_owned()
}

fn emit_entry(output: &mut String, mode: Mode) {
    match mode {
        Mode::Region => {
            writeln!(output, "  %briev.region.frame = alloca {{ ptr, ptr, ptr }}, align 8").ok();
            writeln!(output, "  call void @__briev_region_enter(ptr %briev.region.frame)").ok();
        }
        Mode::Barrier => { writeln!(output, "  %briev.region.saved = call ptr @__briev_region_suspend()").ok(); }
        Mode::Inherit => {}
    }
}

fn emit_exit(output: &mut String, mode: Mode) {
    match mode {
        Mode::Region => { writeln!(output, "  call void @__briev_region_leave(ptr %briev.region.frame)").ok(); }
        Mode::Barrier => { writeln!(output, "  call void @__briev_region_restore(ptr %briev.region.saved)").ok(); }
        Mode::Inherit => {}
    }
}

#[cfg(test)]
pub(super) fn contains_heap_allocation(ir: &str, bytes: u64) -> bool {
    let mut implementation = false;
    let size = format!("i64 {bytes})");
    for line in ir.lines() {
        if line.starts_with("define ") { implementation = function_name(line).is_some_and(|name| name.starts_with("__briev_region_")); }
        if implementation { continue; }
        let normalized = line.replace("@__briev_region_malloc(", "@malloc(");
        let Some(call) = allocator_pattern().captures(&normalized) else { continue };
        if &call[2] == "malloc" && normalized[call.get(0).unwrap().end()..].starts_with(&size) { return true; }
    }
    false
}

#[cfg(test)]
pub(super) fn permits_raw_memory(ir: &str, name: &str) -> bool {
    let name = name.trim_start_matches('@');
    let definition = ir.lines().find(|line| line.starts_with("define ") && function_name(line) == Some(name)).unwrap();
    let mut effects = String::new();
    for reference in attribute_pattern().captures_iter(definition).filter_map(|captures| captures.get(1)) {
        let prefix = format!("attributes #{} =", reference.as_str());
        let mut lines = ir.lines().skip_while(|line| !line.starts_with(&prefix));
        let first = lines.next().expect("referenced attribute group must be defined");
        effects.push_str(first);
        if first.contains('}') { continue; }
        effects.push_str(&lines.take_while(|line| !line.contains('}')).collect::<Vec<_>>().join(" "));
    }
    if effects.contains("readnone") || effects.contains("readonly") || effects.contains("argmemonly") { return false; }
    !effects.contains("memory(") || effects.contains("memory(readwrite)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undefined_attribute_references_cannot_collide_with_new_groups() {
        let module = r#"declare ptr @malloc(i64)
define void @owned() #1 {
entry:
  %value = call ptr @malloc(i64 8)
  ret void
}
attributes #0 = { norecurse }
"#;
        let mut plan = ReclamationPlan::default();
        plan.known.insert("owned".into()); plan.regions.insert("owned".into());
        let lowered = lower(module.into(), &plan);
        assert!(lowered.contains("define void @owned() #5"));
        assert!(lowered.contains("attributes #1 = { }"));
        assert!(lowered.contains("attributes #5 = { noinline }"));
        assert!(!lowered.contains("attributes #5 = { norecurse"));
    }
    #[test]
    fn pure_attributes_and_quoted_values_survive_scoped_cloning() {
        let module = "declare ptr @malloc(i64)\ndefine i64 @pure() #0 {\nentry:\n  ret i64 1\n}\ndefine void @owned() #0 {\nentry:\n  %value = call ptr @malloc(i64 8)\n  ret void\n}\nattributes #0 = { alwaysinline nounwind memory(none) \"custom\"=\"#1 nofree\" }\n";
        let mut plan = ReclamationPlan::default();
        plan.known.extend(["pure".into(), "owned".into()]);
        plan.regions.insert("owned".into());
        let lowered = lower(module.into(), &plan);
        assert!(lowered.contains("define i64 @pure() #0"));
        assert!(lowered.contains("attributes #0 = { alwaysinline nounwind memory(none)"));
        assert!(lowered.contains("define void @owned() #2"));
        assert!(lowered.contains("attributes #2 = { noinline"));
        assert_eq!(lowered.matches("\"custom\"=\"#1 nofree\"").count(), 3);
    }

    #[test]
    fn allocator_text_alone_does_not_enable_regions() {
        let module = "@message = constant [17 x i8] c\"call ptr @malloc(\"\n";
        let mut plan = ReclamationPlan::default();
        plan.regions.insert("owned".into());
        assert_eq!(lower(module.into(), &plan), module);
    }
    #[test]
    fn rewriting_allocator_calls_preserves_constants_and_inline_assembly() {
        assert_eq!(rewrite_allocator_call("  %value = call ptr @malloc(i64 32)"),
            "  %value = call ptr @__briev_region_malloc(i64 32)");
        let assembly = "  call void asm sideeffect \"call ptr @malloc(\", \"\"()";
        assert_eq!(rewrite_allocator_call(assembly), assembly);
        let constant = "@message = constant [17 x i8] c\"call ptr @malloc(\"";
        assert_eq!(rewrite_allocator_call(constant), constant);
    }

    #[test]
    fn allocator_effects_preserve_target_and_inlining_attributes() {
        let group = "attributes #2 = { nofree nosync nounwind alwaysinline memory(argmem: read) \"target-cpu\"=\"x86-64\" }";
        let rewritten = strip_effect_attributes(group);
        assert!(!rewritten.contains("nofree"));
        assert!(!rewritten.contains("memory("));
        assert!(rewritten.contains("nounwind alwaysinline"));
        assert!(rewritten.contains("\"target-cpu\"=\"x86-64\""));
    }
}
