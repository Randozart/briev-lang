//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Boundary-protocol generation — the descriptor rendered into Briev.
//!
//! 2026-11-16 (S3/G1, FFI contract plan): the protocol half of a language adapter
//! is data, not hand-written Briev. This module turns `glue.dbv`'s `values:`
//! section into the `type`/`proto` declarations the casting graph consumes, so
//! that `lib/glue/<lang>/protocols.bv` is generated and reviewable instead of
//! typed by hand.
//!
//! Deliberate properties:
//!
//! - **No language knowledge.** The renderer walks the descriptor generically;
//!   names (`CStr`, `C_String`, `String`) come from data. Nothing here may grow an
//!   `if lang == …`.
//! - **Deterministic.** Keys are visited in sorted order, so re-running the
//!   generator produces the same bytes and a diff means a descriptor change.
//! - **Only classes with a lane render.** A `scalar` class is a base type; a class
//!   without both conversion ops has nothing to generate. That rule is derived
//!   from the class's own fields, not from its name.
//! - **Provenance is in the header.** Every emitted line traces to the descriptor
//!   key path that produced it (D6 as amended: path authoritative, line by lookup),
//!   because the fix for wrong generated glue is always "change that key".

use crate::glue::config::GlueTarget;

/// The generated file for one language, or an empty string when the descriptor
/// declares no renderable class.
pub fn render_protocols(target: &GlueTarget, descriptor_path: &str) -> String {
    let mut names: Vec<&String> = target.values.keys().collect();
    names.sort();

    let mut declarations = String::new();
    let mut protocols = String::new();
    let mut sources: Vec<String> = Vec::new();

    for name in names {
        let Some(class) = target.values.get(name) else {
            continue;
        };
        if !class.renders() {
            continue;
        }
        let (Some(boundary), Some(protocol), Some(base)) = (
            class.boundary.as_ref(),
            class.protocol.as_ref(),
            class.base.as_ref(),
        ) else {
            continue;
        };
        let (Some(to_op), Some(from_op)) = (class.to_op.as_ref(), class.from_op.as_ref()) else {
            continue;
        };
        declarations.push_str(&format!("type {boundary}: {base}<{protocol}> {{ }};\n"));
        protocols.push_str(&format!(
            "proto {protocol}: {base} {{\n    \
             axiom CastTo({base}<UTF8>) = {from_op}(#Lh);\n    \
             axiom CastFrom({base}<UTF8>) = {to_op}(#Lh);\n}};\n"
        ));
        sources.push(format!("values.{name}"));
    }

    if declarations.is_empty() {
        return String::new();
    }

    let mut header = String::new();
    header.push_str("// GENERATED — do not edit.\n");
    header.push_str(&format!(
        "// Source: {descriptor_path} (schema v{}, keys: {})\n",
        target.version,
        sources.join(", ")
    ));
    header.push_str(
        "// Regenerate: brievc render <lang>   —   Verify: brievc render <lang> --check\n",
    );
    header
        .push_str("//\n// Ownership of a specific call is declared per `frgn`, not here (D1).\n\n");
    // 2026-11-19 (issue #132): the axioms name the lane ops (`str_to_c` /
    // `cstr_to_briev` / …), which live in the descriptor's `types_module`. Import
    // it so a program importing this file directly resolves those names — before
    // this, a consumer got "use of undefined value '@str_to_c'" because nothing
    // declared the op the axiom calls. The ops are in the helpers module, NOT
    // the main adapter module, because the main adapter module imports this file
    // (a cycle if this file imported it back). Empty types_module (test targets
    // built via `target_with`) emits no import, preserving the old output.
    if !target.types_module.as_os_str().is_empty() {
        header.push_str(&format!(
            "import \"{}\";\n\n",
            target.types_module.to_string_lossy()
        ));
    }
    format!("{header}{declarations}\n{protocols}")
}

/// Write (or check) one language's generated protocol file.
///
/// 2026-11-16 (S3/G2): `check == false` writes the rendered text; `check == true`
/// compares it with what is on disk and reports staleness **without writing**, so
/// CI can refuse a descriptor change that was not regenerated. Returns whether the
/// file on disk already matched.
pub fn render_and_write(
    target: &GlueTarget,
    descriptor_path: &str,
    out_path: &std::path::Path,
    check: bool,
) -> Result<bool, String> {
    let rendered = render_protocols(target, descriptor_path);
    let existing = std::fs::read_to_string(out_path).ok();
    match existing.as_deref() {
        Some(on_disk) if on_disk == rendered => return Ok(true),
        // A missing file is a first generation, not a staleness: check mode
        // reports what it would write instead of refusing (the descriptor is
        // valid; nothing has drifted).
        None if check => return Ok(true),
        _ => {}
    }
    if check {
        return Err(format!(
            "{} is stale — regenerate it with `brievc render <lang>` (the \
             descriptor changed and the generated protocols did not)",
            out_path.display()
        ));
    }
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(out_path, &rendered)
        .map_err(|e| format!("cannot write {}: {e}", out_path.display()))?;
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glue::config::ValueClass;

    /// A target with the given classes, versioned as the loader would produce.
    fn target_with(values: &[(&str, ValueClass)]) -> GlueTarget {
        let mut t = GlueTarget {
            version: 1,
            ..GlueTarget::default()
        };
        for (name, class) in values {
            t.values.insert((*name).to_string(), class.clone());
        }
        t
    }

    /// A string-like class with a full lane, as the C adapter declares it.
    fn str_class() -> ValueClass {
        ValueClass {
            class: "borrowed_bytes".to_string(),
            zero_copy: true,
            to_op: Some("str_to_c".to_string()),
            from_op: Some("cstr_to_briev".to_string()),
            from_copy: true,
            boundary: Some("CStr".to_string()),
            protocol: Some("C_String".to_string()),
            base: Some("String".to_string()),
            ..ValueClass::default()
        }
    }

    #[test]
    fn renders_a_boundary_type_and_its_axioms() {
        let out = render_protocols(
            &target_with(&[("String", str_class())]),
            "lib/glue/c/glue.dbv",
        );
        assert!(out.contains("type CStr: String<C_String> { };"), "{out}");
        assert!(out.contains("proto C_String: String {"), "{out}");
        assert!(
            out.contains("axiom CastTo(String<UTF8>) = cstr_to_briev(#Lh);"),
            "CastTo is the inbound op, got:\n{out}"
        );
        assert!(
            out.contains("axiom CastFrom(String<UTF8>) = str_to_c(#Lh);"),
            "CastFrom is the outbound op, got:\n{out}"
        );
        assert!(
            out.contains("GENERATED"),
            "the file must be marked generated"
        );
        assert!(
            out.contains("values.String"),
            "provenance must name the descriptor key, got:\n{out}"
        );
    }

    #[test]
    fn scalars_and_laneless_classes_do_not_render() {
        let scalar = ValueClass {
            class: "scalar".to_string(),
            layout: Some("i8".to_string()),
            ..ValueClass::default()
        };
        let lane_less = ValueClass {
            class: "opaque_handle".to_string(),
            boundary: Some("Handle".to_string()),
            protocol: Some("H".to_string()),
            base: Some("Data".to_string()),
            ..ValueClass::default()
        };
        let out = render_protocols(
            &target_with(&[("Bool", scalar), ("Thing", lane_less)]),
            "lib/glue/x/glue.dbv",
        );
        assert!(out.is_empty(), "nothing renderable, got:\n{out}");
    }

    #[test]
    fn rendering_is_deterministic_and_sorted() {
        // Two classes: insertion order must not reach the output.
        let b = ValueClass {
            boundary: Some("BStr".to_string()),
            protocol: Some("B_String".to_string()),
            ..str_class()
        };
        let forward = render_protocols(
            &target_with(&[("AAA", str_class()), ("ZZZ", b.clone())]),
            "d",
        );
        let reverse = render_protocols(&target_with(&[("ZZZ", b), ("AAA", str_class())]), "d");
        assert_eq!(
            forward, reverse,
            "map iteration order must not reach output"
        );
        let first = forward.find("CStr").unwrap_or(usize::MAX);
        let second = forward.find("BStr").unwrap_or(usize::MAX);
        assert!(first < second, "sorted by Briev protocol name:\n{forward}");
    }

    #[test]
    fn check_mode_refuses_a_stale_file_without_writing() {
        let dir = std::env::temp_dir().join(format!("glue_gen_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let out = dir.join("protocols.bv");
        let target = target_with(&[("String", str_class())]);

        // Missing file: a first generation, not a staleness — check mode
        // accepts it and writes nothing.
        assert!(render_and_write(&target, "d", &out, true).unwrap());
        assert!(!out.exists(), "check mode must not write");

        // Write, then check: fresh.
        assert!(!render_and_write(&target, "d", &out, false).unwrap());
        assert!(render_and_write(&target, "d", &out, true).unwrap());

        // Stale file: check refuses and leaves the bytes alone.
        std::fs::write(&out, "// stale\n").unwrap();
        assert!(render_and_write(&target, "d", &out, true).is_err());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "// stale\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_shipped_c_adapter_renders_its_string_lane() {
        let target = crate::glue::config::load_glue_language("c").expect("c adapter loads");
        let out = render_protocols(&target, "lib/glue/c/glue.dbv");
        assert!(
            out.contains("type CStr: String<C_String> { };\n\nproto C_String: String {"),
            "the shipped descriptor must produce the hand-written declaration, got:\n{out}"
        );
        assert!(out.contains("str_to_c(#Lh)"), "{out}");
        assert!(out.contains("cstr_to_briev(#Lh)"), "{out}");
        // 2026-11-19 (issue #132): the axioms name ops defined in the
        // descriptor's types_module; the rendered file must import it, or a
        // consumer importing protocols.bv alone gets `use of undefined value
        // '@str_to_c'`.
        assert!(
            out.contains("import \"glue/c/types.bv\";"),
            "the generated file must import the ops module, got:\n{out}"
        );
    }
}
