// ── Briev Compiler CLI Entry Point ────────────────────────────────────
// 2026-07-12: Phase 7 — Clean CLI dispatch.
// Flat code: max 2 nesting. No unqualified unwraps.
// 2026-07-14: Add --llvm, --out, --optimize-budget flags to build.

mod compile;
mod deps;

use std::collections::HashMap;
use std::env;
use std::path::Path;

use briev_compiler::library;
use briev_compiler::target::{BackendKind, TargetConfig, get_extension};
use briev_compiler::vocab;
use compile::BeastFilter;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage(&args[0]);
        return;
    }

    let result = match args[1].as_str() {
        "build" => run_build(&args[2..]),
        "run" => {
            // brievc run x.abv — compile + drive the GPU runtime in-process.
            let mut run_args: Vec<String> = vec!["--run".into()];
            run_args.extend(args[2..].iter().cloned());
            run_build(&run_args)
        }
        "check" => run_check(&args[2..]),
        "derive" => run_derive(&args[2..]),
        "accept" => run_accept(&args[2..]),
        "library" | "lib" => library::run_library_mode(&args[2..]),
        "export" => run_export(&args[2..]),
        "bindings" => run_bindings(&args[2..]),
        "extension" => run_extension(&args[2..]),
        "doc" => run_doc(&args[2..]),
        "link" => run_link(&args[2..]),
        "audit" => run_audit_cmd(&args[2..]),
        "memcheck" => run_memcheck_cmd(&args[2..]),
        "ownership" => run_ownership_cmd(&args[2..]),
        "config" => run_config(&args[2..]),
        "init" => run_init(args.get(2).map(|s| s.as_str())),
        "bounty" => run_bounty(&args[2..]),
        "registry" => run_registry(&args[2..]),
        "register" => run_register(&args[2..]),
        "vocab" => run_vocab(&args[2..]),
        "grammar" => run_grammar(&args[2..]),
        "fmt" => run_fmt(&args[2..]),
        "install-deps" => deps::install_all(),
        "install-highlighter" => run_install_highlighter(&args[2..]),
        "help" | "--help" | "-h" => { print_usage(&args[0]); Ok(()) }
        _ => {
            // Default: compile the file
            if args[1].ends_with(".bv") || args[1].ends_with(".rbv") || args[1].ends_with(".abv") {
                run_build(&args[1..])
            } else {
                eprintln!("unknown command: {}", args[1]);
                print_usage(&args[0]);
                Ok(())
            }
        }
    };

    if let Err(e) = result {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

fn print_usage(program: &str) {
    let name = Path::new(program).file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("briev-compiler");
    eprintln!("Briev Compiler");
    eprintln!("Usage:");
    eprintln!("  {} build <file.bv>                 Compile a Briev source file", name);
    eprintln!("  {} build <file.bv> --llvm           Emit LLVM IR only, no binary", name);
    eprintln!("  {} build <file.bv> --config-dir <d>  Set config directory", name);
    eprintln!("  {} build <file.bv> --out <dir>      Set output directory", name);
    eprintln!("  {} build <file.bv> --backend <name> Select backend: llvm, circt, webstack, gpu", name);
    eprintln!("  {} build <file.bv> --emit-beast [ast|mid|post|all]  Emit BEAST snapshots (default: all)", name);
    eprintln!("  {} build <file.bv> --no-std          Disable prelude (equivalent to --disable-plugin prelude)", name);
    eprintln!("  {} build <file.bv> --stdlib-path <p>   Set stdlib search path", name);
    eprintln!("  {} build <file.bv> --disable-plugin <name>  Disable a system plugin by name", name);
    eprintln!("  {} build <file.bv> --enable-plugin <name>   Enable only specific plugins", name);
    eprintln!("  {} build <file.bv> --macro-budget <n>       Set instruction limit for macros (0 = unlimited)", name);
    eprintln!("  {} build <file.bv> --allow-read             Allow macros to read files", name);
    eprintln!("  {} build <file.bv> --allow-write            Allow macros to write files", name);
    eprintln!("  {} build <file.bv> --allow-run              Allow macros to execute shell commands", name);
    eprintln!("  {} build <file.bv> --allow-sys-query        Allow macros to query host hardware", name);
    eprintln!("  {} build <file.bv> --allow-net              Allow macros network access", name);
    eprintln!("  {} build <file.bv> --dump-vfs               Print virtual filesystem contents after build", name);
    eprintln!("  {} build <file.bv> --dump-traces            Print macro expansion traces after build", name);
    eprintln!("  {} build <file.bv> --diff                   Show macro changes (dry-run, no output)", name);
    eprintln!("  {} build <file.bv> --target <name>           Build for a specific target profile from briev.toml", name);
    eprintln!("  {} build <file.bv> --accel-cpu-fallback <n>  Min work items for GPU dispatch; below n runs the CPU loop", name);
    eprintln!("  {} build <file.bv> --sysquery <key=value>    Override a SysQuery$ result (repeatable, highest priority)", name);
    eprintln!("  {} build <file.bv> --sysquery-file <path>    Load SysQuery$ overrides from a key=value file", name);
    eprintln!("  {} build <file.bv> --update-lockfile        Regenerate macro-lock.toml from plugin files", name);
    eprintln!("  {} audit [file.bv]                Scan for $ intrinsic usage and capability requirements", name);
    eprintln!("  {} check <file.bv>               Type-check only", name);
    eprintln!("  {} derive <file.bv>              Synthesize derivation blocks", name);
    eprintln!("  {} derive --stochastic <file.bv> Synthesize + MCMC superoptimize (writes .opt.bv)", name);
    eprintln!("  {} accept <file.bv>              Fold synthesized bodies into source", name);
    eprintln!("  {} library <file.bv>             Compile to .a library", name);
    eprintln!("  {} export <file.bv> <lang> [--out <dir>]  Generate a GLUE bridge for <lang>", name);
    eprintln!("  {} doc <file.bv>                  Generate HTML documentation from doc comments", name);
    eprintln!("  {} link <library.so/a/o>         Analyze a foreign library for frgn declarations", name);
    eprintln!("  {} config list                   List available config profiles", name);
    eprintln!("  {} config show                   Show active config profile", name);
    eprintln!("  {} config set <name>             Switch to a config profile", name);
    eprintln!("  {} config init <name>            Create a new config profile", name);
    eprintln!("  {} bounty <file.bv>              Package a .bounty for install-time compilation", name);
    eprintln!("  {} init <name>                   Create a new project", name);
    eprintln!("  {} install-deps                 Download optional deps (z3, dwarfdump)", name);
    eprintln!("  {} install-highlighter [--vsix-only]  Build & install the syntax highlighter .vsix for VS Code / VSCodium", name);
    eprintln!("  {} vocab [path]                 Emit the canonical language vocabulary manifest (default: stdout)", name);
    eprintln!("  {} grammar <path>               Regenerate the TextMate grammar keyword/type patterns from the vocab", name);
    eprintln!("  {} fmt <file> [--stdout|--check]  Format a source file canonically (round-trip safe)", name);
    eprintln!("  {} help                          Show this help", name);
}

/// 2026-08-05 (Phase 1): emit the canonical language vocabulary manifest for
/// tooling (LSP/highlighter generation, CI parity checks). `brievc vocab`
/// prints TOML to stdout; `brievc vocab <path>` writes the file.
fn run_vocab(args: &[String]) -> Result<(), String> {
    let vocab = vocab::LanguageVocab::canonical();
    let text = vocab::serialize_vocab(&vocab).map_err(|e| e.to_string())?;
    match args.first() {
        Some(path) => {
            std::fs::write(path, text).map_err(|e| format!("failed to write vocab: {}", e))
        }
        None => {
            print!("{}", text);
            Ok(())
        }
    }
}

/// 2026-08-05 (Phase 1): regenerate the TextMate grammar's keyword/type
/// patterns from the canonical vocab so the highlighter cannot drift.
/// `brievc grammar <path-to-briev.tmLanguage.json>`.
fn run_grammar(args: &[String]) -> Result<(), String> {
    let path = args
        .first()
        .ok_or("usage: brievc grammar <path/to/briev.tmLanguage.json>")?;
    vocab::regenerate_highlighter_grammar(std::path::Path::new(path))
}

/// 2026-08-05 (Phase 2): canonical formatting. `brievc fmt <file>` parses,
/// formats with the canonical formatter, and writes the file back.
/// `--stdout` prints instead of writing; `--check` verifies idempotence.
fn run_fmt(args: &[String]) -> Result<(), String> {
    let mut stdout = false;
    let mut check = false;
    let mut file: Option<&str> = None;
    for arg in args {
        match arg.as_str() {
            "--stdout" => stdout = true,
            "--check" => check = true,
            other => file = Some(other),
        }
    }
    let file = file.ok_or("usage: brievc fmt <file.bv> [--stdout|--check]")?;
    let source = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read '{}': {}", file, e))?;
    let tokens = briev_compiler::lexer::tokenize(&source)
        .map_err(|e| format!("lex failed: {}", e))?;
    let mut parser = briev_compiler::parser::Parser::new(tokens, &source);
    let items = parser
        .parse_program()
        .map_err(|e| format!("parse failed: {}", e))?;
    let formatted = briev_compiler::ast::format_program(&items);

    if check {
        // Round-trip: reformat the formatted output and require a fixed point.
        let tokens2 = briev_compiler::lexer::tokenize(&formatted)
            .map_err(|e| format!("re-lex failed: {}", e))?;
        let mut parser2 = briev_compiler::parser::Parser::new(tokens2, &formatted);
        let items2 = parser2
            .parse_program()
            .map_err(|e| format!("re-parse of formatted output failed: {}", e))?;
        let reformatted = briev_compiler::ast::format_program(&items2);
        if formatted != reformatted {
            return Err(format!(
                "formatting is not idempotent for '{}'\n--- first pass:\n{}\n--- second pass:\n{}",
                file, formatted, reformatted
            ));
        }
        if formatted != source && formatted != format!("{}\n", source.trim_end()) {
            return Err(format!("'{}' is not canonically formatted", file));
        }
        return Ok(());
    }

    if stdout {
        print!("{}", formatted);
    } else {
        std::fs::write(file, formatted)
            .map_err(|e| format!("cannot write '{}': {}", file, e))?;
    }
    Ok(())
}

/// Parse `build` subcommand arguments into a `BuildOptions`.
///
/// Accepts:
///   <file.bv>               (positional, required)
///   --llvm                  emit IR only, no binary
///   --out <dir>             output directory
///   --optimize-budget <N>   simulation budget (default 256)
///   --backend <name>        select backend (llvm, circt, webstack, gpu)
///   --emit-beast [stage]     emit BEAST snapshots (ast, mid, post, all; default all)
fn parse_build_args(args: &[String]) -> Result<compile::BuildOptions, String> {
    let mut file_path: Option<String> = None;
    let mut emit_ir_only = false;
    let mut config_dir: Option<String> = None;
    let mut out_dir: Option<String> = None;
    let mut optimize_budget = 256u64;
    let mut shared = false;
    let mut library_mode = false;
    let mut emit_beast = Vec::new();
    let mut backend_override: Option<String> = None;
    let mut no_stdlib = false;
    let mut run_flag = false;
    let mut stdlib_path: Option<String> = None;
    let mut disable_plugins = Vec::new();
    let mut enable_plugins = Vec::new();
    let mut trg_unresolved_action = compile::TrgUnresolvedAction::Warn;
    let mut allow_read = false;
    let mut allow_write = false;
    let mut allow_run = false;
    let mut allow_sys_query = false;
    let mut allow_net = false;
    let mut macro_budget = 0u64;
    let mut dump_vfs = false;
    let mut update_lockfile = false;
    let mut dump_traces = false;
    let mut diff_mode = false;
    let mut target_name: Option<String> = None;
    let mut sysquery_pairs: Vec<(String, String)> = Vec::new();
    let mut sysquery_files: Vec<String> = Vec::new();
    let mut int_bits = 64u64;
    let mut accel_cpu_fallback: Option<u64> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--llvm" {
            emit_ir_only = true;
            i += 1;
        } else if arg == "--config-dir" {
            let val = args.get(i + 1).ok_or("--config-dir requires a directory argument")?;
            config_dir = Some(val.clone());
            i += 2;
        } else if arg == "--shared" {
            shared = true;
            i += 1;
        } else if arg == "--library" {
            library_mode = true;
            i += 1;
        } else if arg == "--out" {
            let val = args.get(i + 1).ok_or("--out requires a directory argument")?;
            out_dir = Some(val.clone());
            i += 2;
        } else if arg == "--optimize-budget" {
            let val = args.get(i + 1).ok_or("--optimize-budget requires a number argument")?;
            optimize_budget = val.parse()
                .map_err(|_| format!("invalid --optimize-budget value: '{}'", val))?;
            i += 2;
        } else if arg == "--emit-beast" {
            // --emit-beast [stage][.position] like "parse", "parse.before", "parse.after"
            let next = args.get(i + 1);
            let stage_str = next.filter(|s| !s.starts_with('-')).map(|s| s.as_str());
            let all_stages = vec![
                "parse", "resolve", "type-check", "normalize", "verify",
                "alloc", "provenance", "codegen", "optimize",
            ];
            match stage_str {
                Some("all") | None => {
                    for s in all_stages {
                        let filter: BeastFilter
                            = s.parse().map_err(|e: String| e)?;
                        emit_beast.push(filter);
                    }
                }
                Some(s) => {
                    let filter: BeastFilter
                        = s.parse().map_err(|e: String| e)?;
                    emit_beast.push(filter);
                }
            }
            i += if stage_str.is_some() { 2 } else { 1 };
        } else if arg == "--backend" {
            let name = args.get(i + 1).ok_or("--backend requires a name argument (llvm, circt, webstack, gpu, spirv, vm)")?;
            backend_override = Some(name.clone());
            i += 2;
        } else if arg == "--no-std" {
            no_stdlib = true;
            i += 1;
        } else if arg == "--run" {
            run_flag = true;
            i += 1;
        } else if arg == "--stdlib-path" {
            let val = args.get(i + 1).ok_or("--stdlib-path requires a path argument")?;
            stdlib_path = Some(val.clone());
            i += 2;
        // 2026-07-25: Target #Int protocol width. WASM uses 32 to emit
        // i32 instead of i64, eliminating BigInt overhead in JavaScript.
        } else if arg == "--int-bits" {
            let val = args.get(i + 1).ok_or("--int-bits requires a number argument (8, 16, 32, or 64)")?;
            int_bits = val.parse()
                .map_err(|_| format!("invalid --int-bits value: '{}'", val))?;
            if int_bits != 8 && int_bits != 16 && int_bits != 32 && int_bits != 64 {
                return Err(format!("--int-bits must be 8, 16, 32, or 64, got: {}", int_bits));
            }
            i += 2;
        } else if arg == "--accel-cpu-fallback" {
            let val = args.get(i + 1).ok_or("--accel-cpu-fallback requires a number argument (min work items for GPU)")?;
            accel_cpu_fallback = Some(val.parse()
                .map_err(|_| format!("invalid --accel-cpu-fallback value: '{}'", val))?);
            i += 2;
        } else if arg == "--disable-plugin" {
            let name = args.get(i + 1).ok_or("--disable-plugin requires a plugin name argument")?;
            disable_plugins.push(name.clone());
            i += 2;
        } else if arg == "--enable-plugin" {
            let name = args.get(i + 1).ok_or("--enable-plugin requires a plugin name argument")?;
            enable_plugins.push(name.clone());
            i += 2;
        } else if arg == "--allow-read" {
            allow_read = true;
            i += 1;
        } else if arg == "--allow-write" {
            allow_write = true;
            i += 1;
        } else if arg == "--allow-run" {
            allow_run = true;
            i += 1;
        } else if arg == "--allow-sys-query" {
            allow_sys_query = true;
            i += 1;
        } else if arg == "--allow-net" {
            allow_net = true;
            i += 1;
        } else if arg == "--dump-vfs" {
            dump_vfs = true;
            i += 1;
        } else if arg == "--update-lockfile" {
            update_lockfile = true;
            i += 1;
        } else if arg == "--dump-traces" {
            dump_traces = true;
            i += 1;
        } else if arg == "--diff" {
            diff_mode = true;
            i += 1;
        } else if arg == "--target" {
            let val = args.get(i + 1).ok_or("--target requires a target name argument")?;
            target_name = Some(val.clone());
            i += 2;
        } else if arg == "--sysquery" {
            let val = args.get(i + 1).ok_or("--sysquery requires a key=value argument")?;
            let parts: Vec<&str> = val.splitn(2, '=').collect();
            if parts.len() != 2 {
                return Err(format!("--sysquery: expected key=value, got '{}'", val));
            }
            sysquery_pairs.push((parts[0].to_string(), parts[1].to_string()));
            i += 2;
        } else if arg == "--sysquery-file" {
            let val = args.get(i + 1).ok_or("--sysquery-file requires a file path argument")?;
            sysquery_files.push(val.clone());
            i += 2;
        } else if arg == "--macro-budget" {
            let val = args.get(i + 1).ok_or("--macro-budget requires a number argument")?;
            macro_budget = val.parse()
                .map_err(|_| format!("invalid --macro-budget value: '{}'", val))?;
            i += 2;
        } else if arg == "--warn-unresolved-trg" {
            trg_unresolved_action = compile::TrgUnresolvedAction::Warn;
            i += 1;
        } else if arg == "--error-unresolved-trg" {
            trg_unresolved_action = compile::TrgUnresolvedAction::Error;
            i += 1;
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag: {}", arg));
        } else if file_path.is_some() {
            return Err(format!("unexpected positional argument: '{}'", arg));
        } else {
            file_path = Some(arg.clone());
            i += 1;
        }
    }

    let file_path = file_path.ok_or("missing file argument")?;

    // Resolve backend: CLI override or extension lookup
    let backend = match &backend_override {
        Some(name) => TargetConfig::resolve(name)?,
        None => {
            let ext = get_extension(&file_path);
            let config = load_target_config(config_dir.as_deref());
            match config.lookup(&ext) {
                // 2026-07-31: backend is Option now (target-tuning tables in the
                // same file omit it); extension entries always set it, so a
                // missing backend falls back to the LLVM default.
                Some(entry) => TargetConfig::resolve(entry.backend.as_deref().unwrap_or("llvm"))?,
                None => BackendKind::Llvm,
            }
        }
    };

    Ok(compile::BuildOptions {
        run: run_flag,
        config_dir,
        file_path,
        emit_ir_only,
        out_dir,
        optimize_budget,
        emit_beast_stages: emit_beast,
        backend,
        no_stdlib,
        stdlib_path,
        disable_plugins,
        enable_plugins,
        trg_unresolved_action,
        extra_objects: vec![],
        shared,
        library_mode,
        glue_config: None,
        stack_threshold: 4096,
        int_bits,
        allow_read,
        allow_write,
        allow_run,
        allow_sys_query,
        allow_net,
        macro_budget,
        dump_vfs,
        update_lockfile,
        dump_traces,
        diff_mode,
        sysquery_overrides: HashMap::new(),
        target: target_name,
        sysquery_pairs,
        sysquery_files,
        style_css: None,
        view_html: None,
        view_bindings: vec![],
        ssr: false,
        dev: false,
        accel_cpu_fallback,
        isr_mechanism: None,
    })
}

/// `brievc bounty <file.bv>` — package a .bounty for install-time compilation.
fn run_bounty(args: &[String]) -> Result<(), String> {
    let file_path = args.first().ok_or("usage: brievc bounty <file.bv>")?;
    let source = std::fs::read_to_string(file_path)
        .map_err(|e| format!("cannot read '{}': {}", file_path, e))?;

    let opts = compile::BuildOptions {
        run: false,
        config_dir: None,
        file_path: file_path.clone(),
        emit_ir_only: false,
        out_dir: None,
        optimize_budget: 256,
        emit_beast_stages: vec![],
        backend: briev_compiler::target::BackendKind::Vm,
        // 2026-08-23 (Plan 1 HCALL slice): the tamer archive must not carry
        // the LLVM-oriented stdlib prelude — those helpers are outside the
        // VM's declared surface (they'd fail the capability gate mid-stream)
        // and the compile tail provides its own runtime
        // (lib/runtime/briev_rt.c host services).
        no_stdlib: true,
        stdlib_path: None,
        disable_plugins: vec![],
        enable_plugins: vec![],
        trg_unresolved_action: briev_compiler::backend::llvm::TrgUnresolvedAction::Warn,
        extra_objects: vec![],
        shared: false,
        library_mode: false,
        glue_config: None,
        stack_threshold: 4096,
        int_bits: 64,
        allow_read: false,
        allow_write: false,
        allow_run: false,
        allow_sys_query: false,
        allow_net: false,
        macro_budget: 256,
        dump_vfs: false,
        update_lockfile: false,
        dump_traces: false,
        diff_mode: false,
        sysquery_overrides: std::collections::HashMap::new(),
        target: None,
        sysquery_pairs: vec![],
        sysquery_files: vec![],
        style_css: None,
        view_html: None,
        view_bindings: vec![],
        ssr: false,
        dev: false,
        accel_cpu_fallback: None,
        isr_mechanism: None,
    };
    let source = std::fs::read_to_string(file_path)
        .map_err(|e| format!("cannot read '{}': {}", file_path, e))?;

    // 1. Compile to Typed stage
    eprintln!("[bounty] Compiling to Typed stage...");
    let (items, universe) = compile::compile_to_typed(file_path, &source, &opts)?;

    // 2. Generate obfuscation seed and obfuscate
    let noise_seed = 0xDEADBEEF; // MVP: fixed seed. Future: random.
    eprintln!("[bounty] Obfuscating identifiers...");
    let (obfuscated_items, _inverse_map) =
        briev_compiler::beastpack::obfuscate::obfuscate(&items, noise_seed);

    // 3. Serialize beastpack
    eprintln!("[bounty] Serializing .beastpack...");
    let beastpack = briev_compiler::beastpack::serialize(&obfuscated_items, &universe, noise_seed);
    eprintln!("[bounty]   .beastpack: {} bytes", beastpack.len());

    // 4. Pre-compile the tamer VM interpreter to .lair bytecode
    eprintln!("[bounty] Compiling tamer VM interpreter to .lair...");
    let tamer_source = std::fs::read_to_string("lib/tamer/main.bv")
        .map_err(|e| format!("cannot read lib/tamer/main.bv: {}", e))?;
    let (tamer_items, tamer_universe) = compile::compile_to_typed(
        "lib/tamer/main.bv", &tamer_source, &opts)?;
    let mut vm = briev_compiler::backend::vm::VmBackend::new();
    let tamer_lair = vm.generate(&tamer_items, &tamer_universe);
    eprintln!("[bounty]   tamer .lair: {} bytes", tamer_lair.len());

    // 5. Compile user program to .lair too (data for tamer to interpret)
    // 2026-08-23 (Plan 1 HCALL slice): the bounty path bypasses codegen()'s
    // capability gate (it constructs VmBackend directly), so validate here —
    // out-of-surface constructs must be compile errors, never silent
    // install-time traps.
    let cap_errors = briev_compiler::backend::capabilities::validate_program(
        &obfuscated_items, &briev_compiler::backend::vm::CAPABILITIES);
    if !cap_errors.is_empty() {
        return Err(cap_errors.join("\n"));
    }
    let mut vm2 = briev_compiler::backend::vm::VmBackend::new();
    let user_lair = vm2.generate(&obfuscated_items, &universe);
    if !vm2.errors.is_empty() {
        return Err(vm2.errors.join("\n"));
    }
    eprintln!("[bounty]   user .lair: {} bytes", user_lair.len());

    // 6. Assemble .bounty (4-section: tamer.lair + user.lair + beastpack + manifest)
    // 2026-08-23 (Plan 1 HCALL slice): ship the USER entry's bytecode offset
    // in the manifest — the tamer starts execution there instead of fn 0
    // (which is a prelude helper stub). User fns emit last, so the entry is
    // the fn with the largest bc_offset in the user .lair's fn table.
    let user_entry_bc = {
        use std::convert::TryInto;
        let hdr = |i: usize| -> u64 {
            u64::from_le_bytes(user_lair[16 + i * 8..16 + (i + 1) * 8].try_into().unwrap())
        };
        let _str_off = hdr(0);
        let fn_off = hdr(2) as usize;
        let fn_size = hdr(3) as usize;
        let bc_off = hdr(4);
        let n = fn_size / 20;
        // fn bc_offsets are RELATIVE to the bytecode section; the tamer
        // indexes from the .lair base, so ship the ABSOLUTE offset.
        bc_off
            + (0..n)
                .map(|k| {
                    u64::from_le_bytes(
                        user_lair[fn_off + k * 20 + 4..fn_off + k * 20 + 12]
                            .try_into()
                            .unwrap(),
                    )
                })
                .max()
                .unwrap_or(0)
    };
    let manifest = format!(
        r#"{{"version":1,"entry_point":"main","entry_bc":{},"noise_seed":{}}}"#,
        user_entry_bc, noise_seed
    );
    let bounty = briev_compiler::bounty::write_bounty_full(
        &tamer_lair, &user_lair, &beastpack, &manifest);

    // 6. Write .bounty file
    let output_path = file_path.replace(".bv", ".bounty");
    std::fs::write(&output_path, &bounty)
        .map_err(|e| format!("cannot write '{}': {}", output_path, e))?;
    eprintln!("[bounty] Written: {} ({} bytes)", output_path, bounty.len());
    eprintln!("[bounty] Distribute to any platform.");
    eprintln!("[bounty] Customer runs: `tamer {}`", output_path);

    Ok(())
}

fn run_build(args: &[String]) -> Result<(), String> {
    let opts = parse_build_args(args)?;

    // --config-dir must reach the IR-lowering tuning too (baked config is the
    // fallback): install the override BEFORE any compile step touches
    // `ir_lowering()` — the LazyLock initializes on first access.
    if let Some(dir) = &opts.config_dir {
        briev_compiler::config_tuning::set_ir_lowering_from_dir(std::path::Path::new(dir))?;
    }

    // 2026-07-28: Phase E.2 — doppelganger resolution: .opt.bv > .derive.bv > .bv
    // Read source from the doppelganger if it exists, but pass opts.file_path
    // to compile functions so error messages and output paths use the original name.
    let doppelganger_path = briev_compiler::derive::Doppelganger::resolve(std::path::Path::new(&opts.file_path));
    let source = if doppelganger_path != std::path::Path::new(&opts.file_path) {
        eprintln!("[derive] using {}", doppelganger_path.display());
        std::fs::read_to_string(&doppelganger_path)
            .map_err(|e| format!("cannot read '{}': {}", doppelganger_path.display(), e))?
    } else {
        std::fs::read_to_string(&opts.file_path)
            .map_err(|e| format!("cannot read '{}': {}", opts.file_path, e))?
    };

    // 2026-07-23: Resolve SysQuery$ overrides from three sources (low→high):
    //   1. --target <name> loads per-target overrides from briev.toml profiles
    //   2. --sysquery-file <path> loads key=value pairs from a text file
    //   3. --sysquery <key=value> CLI flags (highest priority)
    // Each source merges over the previous. If no overrides from any source,
    // SysQuery$ queries the real host (backward compatible).

    // Helper: merge a HashMap into the base, later values override earlier.
    let mut merge_overrides = |incoming: HashMap<String, String>, base: &mut HashMap<String, String>| {
        for (k, v) in incoming {
            base.insert(k, v);
        }
    };

    // Helper: read a key=value file (one pair per line, # comments, blank lines skipped).
    let load_sysquery_file = |path: &str| -> Result<HashMap<String, String>, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read sysquery file '{}': {}", path, e))?;
        let mut map = HashMap::new();
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = trimmed.splitn(2, '=').collect();
            if parts.len() != 2 {
                return Err(format!("{}:{}: expected key=value, got '{}'", path, lineno + 1, trimmed));
            }
            map.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
        }
        Ok(map)
    };

    // ── Determine what to build ──────────────────────────────────────
    // Each entry: (target_name, base_overrides_from_profile, isr_mechanism)
    let target_profiles: Vec<(String, HashMap<String, String>, Option<String>)> = if let Some(ref target_name) = opts.target {
        // --target <name>: single target from briev.toml
        let project_dir = std::path::Path::new(&opts.file_path)
            .parent().map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let manifest = briev_compiler::manifest::find_manifest(&project_dir)
            .and_then(|p| briev_compiler::manifest::Manifest::load(&p).ok());
        let manifest = manifest.as_ref().ok_or_else(|| {
            format!("--target '{}' requires a briev.toml with target profiles", target_name)
        })?;
        let profile = manifest.target.get(target_name).ok_or_else(|| {
            format!("target '{}' not found in briev.toml. Available targets: {}",
                target_name, manifest.target.keys().cloned().collect::<Vec<_>>().join(", "))
        })?;
        vec![(target_name.clone(), profile.sysquery_overrides(), profile.isr_mechanism.clone())]
    } else {
        // Single default build (no --target)
        vec![("default".to_string(), HashMap::new(), None)]
    };

    // ── Compile each target ───────────────────────────────────────────
    for (target_name, profile_overrides, profile_isr_mechanism) in &target_profiles {
        if target_profiles.len() > 1 {
            println!("Building for target: {}", target_name);
        }

        // Build sysquery_overrides with cascading precedence
        let mut overrides = profile_overrides.clone();           // lowest: target profile

        // Middle: --sysquery-file paths (load in order, later overrides earlier)
        for file_path in &opts.sysquery_files {
            let file_overrides = load_sysquery_file(file_path)?;
            merge_overrides(file_overrides, &mut overrides);
        }

        // Highest: --sysquery <key=value> CLI pairs (later overrides earlier)
        for (key, value) in &opts.sysquery_pairs {
            overrides.insert(key.clone(), value.clone());
        }

        let mut target_opts = opts.clone();
        target_opts.sysquery_overrides = overrides;
        // 2026-09-06 (ISR plan): the profile's ISR mechanism is the configured
        // default for mechanism-less `isr` declarations.
        target_opts.isr_mechanism = profile_isr_mechanism.clone();

        // Per-target output directory
        if *target_name != "default" {
            let orig = target_opts.out_dir.clone();
            target_opts.out_dir = Some(
                orig.map(|d| format!("{}/{}", d, target_name))
                    .unwrap_or_else(|| format!("bin/{}", target_name))
            );
        }

        compile::compile_source(&opts.file_path, &source, &target_opts)?;
    }

    Ok(())
}

fn run_check(args: &[String]) -> Result<(), String> {
    let file_path = args.first().ok_or("missing file argument")?;
    let source = {
        let p = std::path::Path::new(file_path);
        let doppel = briev_compiler::derive::Doppelganger::resolve(p);
        if doppel != p {
            std::fs::read_to_string(&doppel)
                .map_err(|e| format!("cannot read '{}': {}", doppel.display(), e))?
        } else {
            std::fs::read_to_string(file_path)
                .map_err(|e| format!("cannot read '{}': {}", file_path, e))?
        }
    };
    // 2026-08-09 (Phase 13, SPEC 22.6): `briev check file.dbv|file.dbvl`
    // dispatches to the Data Briev check (parse + validate asserted schemas),
    // not the .bv source pipeline.
    let ext = get_extension(file_path);
    if ext == ".dbv" || ext == ".dbvl" {
        return compile::check_data_source(file_path, &source);
    }
    compile::check_source(file_path, &source)
}

fn run_memcheck_cmd(args: &[String]) -> Result<(), String> {
    let file_path = args.first().ok_or("usage: brievc memcheck <file.bv>")?;
    let source = std::fs::read_to_string(file_path)
        .map_err(|e| format!("cannot read '{}': {}", file_path, e))?;
    let tokens = briev_compiler::lexer::tokenize(&source)
        .map_err(|e| format!("lex failed: {}", e))?;
    let mut parser = briev_compiler::parser::Parser::new(tokens, &source);
    let items = parser.parse_program().map_err(|e| format!("parse failed: {}", e))?;
    let report = briev_compiler::macros::memcheck::run_memcheck(&items);
    briev_compiler::macros::memcheck::print_memcheck(&report);
    Ok(())
}

fn run_audit_cmd(args: &[String]) -> Result<(), String> {
    let source_file = args.first().map(|s| s.as_str());
    let results = briev_compiler::macros::audit::run_audit(source_file)?;
    briev_compiler::macros::audit::print_audit(&results);
    Ok(())
}

/// `brievc ownership <file.bv>` — audit report of every FFI boundary's
/// ownership class (value / owned / zero-copy / borrowed / zero-cost).
/// 2026-08-31 (boundary ownership plan §5): the compiler's view of each
/// export/frgn parameter and return — is this boundary copy-free, and why.
fn run_ownership_cmd(args: &[String]) -> Result<(), String> {
    let file_path = args.first().ok_or("usage: brievc ownership <file.bv>")?;
    let source = std::fs::read_to_string(file_path)
        .map_err(|e| format!("cannot read '{}': {}", file_path, e))?;
    let (items, universe) = briev_compiler::library::parse_and_check(file_path, &source)?;
    let ownership = briev_compiler::analysis::boundary_ownership::compute_boundary_ownership(
        &items,
        Some(&universe),
        &|_| None,
    );

    println!("== Boundary ownership: {}", file_path);
    let mut export_names: Vec<&String> = ownership.exports.keys().collect();
    export_names.sort();
    for name in export_names {
        let e = &ownership.exports[name];
        let params: Vec<String> = e.params.iter().map(|o| o.as_str().to_string()).collect();
        let ret = e.ret.map(|o| o.as_str().to_string()).unwrap_or_else(|| "void".to_string());
        println!("  export {:<24} params: [{}]  return: {}", name, params.join(", "), ret);
    }
    let mut frgn_names: Vec<&String> = ownership.frgns.keys().collect();
    frgn_names.sort();
    for name in frgn_names {
        let e = &ownership.frgns[name];
        let params: Vec<String> = e.params.iter().map(|o| o.as_str().to_string()).collect();
        let ret = e.ret.map(|o| o.as_str().to_string()).unwrap_or_else(|| "void".to_string());
        println!("  frgn   {:<24} params: [{}]  return: {}", name, params.join(", "), ret);
    }
    Ok(())
}

/// `brievc registry {add,list,remove}` — manage the compiler registry.
/// 2026-07-26: Phase 1f — Per-user registry directory.
fn run_registry(args: &[String]) -> Result<(), String> {
    let sub = args.first().ok_or("expected 'add', 'list', or 'remove'")?;
    match sub.as_str() {
        "add" => {
            let path = args.get(1).ok_or("missing path argument for 'registry add'")?;
            let source = std::path::Path::new(path);
            let name = args.get(2).and_then(|s| {
                if s.starts_with("--name=") { Some(&s[7..]) } else { None }
            }).or_else(|| {
                source.file_stem().and_then(|s| s.to_str())
            }).ok_or("could not infer registry name from path; use --name=<name>")?;
            briev_compiler::registry::add(source, name)?;
            Ok(())
        }
        "list" => {
            let entries = briev_compiler::registry::list()?;
            if entries.is_empty() {
                println!("(registry is empty)");
            } else {
                println!("{:<30} {:<15} {:>10}", "Name", "Type", "Size");
                println!("{:-<30} {:-<15} {:-<10}", "", "", "");
                for (name, kind, size) in &entries {
                    let size_str = if *size > 0 {
                        if *size > 1024 {
                            format!("{}k", size / 1024)
                        } else {
                            format!("{}b", size)
                        }
                    } else {
                        "-".to_string()
                    };
                    println!("{:<30} {:<15} {:>10}", name, kind, size_str);
                }
            }
            Ok(())
        }
        "remove" => {
            let name = args.get(1).ok_or("missing name argument for 'registry remove'")?;
            briev_compiler::registry::remove(name)?;
            Ok(())
        }
        _ => Err(format!("unknown registry subcommand '{}'. Use 'add', 'list', or 'remove'", sub)),
    }
}

/// `briev-compiler register <name>` — register a project/target schema.
/// 2026-07-15: Phase 7 — Stub implementation.
fn run_register(_args: &[String]) -> Result<(), String> {
    eprintln!("register: not yet implemented — schema registration is a future feature");
    Ok(())
}

fn run_derive(args: &[String]) -> Result<(), String> {
    let (config, positional) = briev_compiler::derive::parse_derive_flags(args)?;
    let file_path = positional.first().ok_or("missing file argument\nusage: briev derive [--stochastic] [--iterations N] [--temperature T] [--enumerative-depth N] <file.bv>")?;
    briev_compiler::derive::handle_derive_command(&config, file_path)
}

fn run_accept(args: &[String]) -> Result<(), String> {
    let use_opt = args.iter().any(|a| a == "--opt");
    let file_path = args.iter().find(|a| !a.starts_with("--"))
        .ok_or("missing file argument\nusage: briev accept [--opt] <file.bv>")?;
    briev_compiler::derive::handle_accept_command(file_path, use_opt)
}

/// Load TargetConfig with optional --config-dir override.
/// 2026-07-16: P1 — Respects runtime config directory when set.
fn load_target_config(config_dir: Option<&str>) -> TargetConfig {
    match config_dir {
        Some(dir) => {
            match briev_compiler::dbriev::config_db::resolve_config_file(std::path::Path::new(dir), "targets") {
                Some(path) => TargetConfig::load_from(&path).unwrap_or_else(|e| {
                    eprintln!("warning: cannot load '{}': {} — using baked fallback", path.display(), e);
                    TargetConfig::load()
                }),
                None => {
                    eprintln!("warning: no targets config found in '{}' — using baked fallback", dir);
                    TargetConfig::load()
                }
            }
        }
        None => TargetConfig::load(),
    }
}

/// `briev doc <file.bv>` — generate HTML documentation.
fn run_doc(args: &[String]) -> Result<(), String> {
    let file_path = args.first().ok_or("usage: briev doc <file.bv>")?;
    briev_compiler::doc::generate_doc(file_path)
}

/// `briev-compiler config <subcommand>` — manage config profiles.
/// Subcommands: list, show, set <name>, init <name>
fn run_config(args: &[String]) -> Result<(), String> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("show");
    match sub {
        "list" => {
            let profiles = briev_compiler::config_resolver::list_profiles()?;
            if profiles.is_empty() {
                println!("no profiles configured");
            } else {
                println!("available profiles:");
                for p in &profiles {
                    println!("  {}", p);
                }
            }
            Ok(())
        }
        "show" => briev_compiler::config_resolver::show_active_profile(),
        "set" => {
            let name = args.get(1).ok_or("usage: briev-compiler config set <profile-name>")?;
            briev_compiler::config_resolver::set_active_profile(name)
        }
        "init" => {
            let name = args.get(1).ok_or("usage: briev-compiler config init <profile-name>")?;
            briev_compiler::config_resolver::init_profile(name)
        }
        _ => Err(format!("unknown config subcommand '{}'. Use: list, show, set <name>, init <name>", sub)),
    }
}

/// `briev export <file.bv> <language> [--out <dir>]`
/// 2026-07-22: Generate a GLUE bridge for the target language.
fn run_export(args: &[String]) -> Result<(), String> {
    let file_path = args.first().ok_or("usage: briev export <file.bv> <language> [--out <dir>]")?;
    let language = args.get(1).ok_or("usage: briev export <file.bv> <language> [--out <dir>]")?;
    let mut out_dir = ".".to_string();
    let mut i = 2;
    while i < args.len() {
        if args[i] == "--out" {
            out_dir = args.get(i + 1).ok_or("--out requires a directory argument")?.clone();
            i += 2;
        } else {
            return Err(format!("unknown flag: {}", args[i]));
        }
    }
    briev_compiler::glue::export::run_export_cli(file_path, language, &out_dir)
}

/// `briev extension <file.bv> <language> [--out <dir>]` — build a native
/// host-language extension module (e.g. a CPython C-extension, no ctypes).
fn run_extension(args: &[String]) -> Result<(), String> {
    let file_path = args.first().ok_or("usage: briev extension <file.bv> <language> [--out <dir>]")?;
    let language = args.get(1).ok_or("usage: briev extension <file.bv> <language> [--out <dir>]")?;
    let mut out_dir = ".".to_string();
    let mut i = 2;
    while i < args.len() {
        if args[i] == "--out" {
            out_dir = args.get(i + 1).ok_or("--out requires a directory argument")?.clone();
            i += 2;
        } else {
            return Err(format!("unknown flag: {}", args[i]));
        }
    }
    briev_compiler::glue::export::run_extension_cli(file_path, language, &out_dir)
}

/// `briev bindings <file.bv> <language> [--out <dir>]` — render only the
/// language's config-driven bindings templates (e.g. briev_types.h).
fn run_bindings(args: &[String]) -> Result<(), String> {
    let file_path = args.first().ok_or("usage: briev bindings <file.bv> <language> [--out <dir>]")?;
    let language = args.get(1).ok_or("usage: briev bindings <file.bv> <language> [--out <dir>]")?;
    let mut out_dir = ".".to_string();
    let mut i = 2;
    while i < args.len() {
        if args[i] == "--out" {
            out_dir = args.get(i + 1).ok_or("--out requires a directory argument")?.clone();
            i += 2;
        } else {
            return Err(format!("unknown flag: {}", args[i]));
        }
    }
    briev_compiler::glue::export::run_bindings_cli(file_path, language, &out_dir)
}

/// `briev link <library.so/a/o>`
/// 2026-07-22: Analyze a foreign library and generate frgn declarations.
fn run_link(args: &[String]) -> Result<(), String> {
    let lib_path = args.first().ok_or("usage: briev link <library.so/a/o>")?;
    let result = briev_compiler::glue::link::analyze_library(std::path::Path::new(lib_path))?;
    briev_compiler::glue::link::print_link_summary(&result);
    let bridge_bv = briev_compiler::glue::link::generate_bridge_bv(&result);
    println!("{}", bridge_bv);
    Ok(())
}

fn run_init(name: Option<&str>) -> Result<(), String> {
    let name = name.unwrap_or("my_project");
    let dir = Path::new(name);
    std::fs::create_dir_all(dir.join("src"))
        .map_err(|e| format!("cannot create project: {}", e))?;
    let main_bv = format!(r#"defn main() -> Int {{
    term 0;
}};
"#);
    std::fs::write(dir.join("src").join("main.bv"), main_bv)
        .map_err(|e| format!("cannot write main.bv: {}", e))?;
    println!("Created project '{}'", name);
    Ok(())
}

/// `briev install-highlighter [--vsix-only]`
/// 2026-07-25: Build & install the VS Code / VSCodium syntax highlighter .vsix.
/// Detects the highlighter directory relative to the executable or CWD.
/// Detects `codium` or `code` CLI for automatic installation.
/// --vsix-only: just build the .vsix, don't install.
fn run_install_highlighter(args: &[String]) -> Result<(), String> {
    let mut vsix_only = false;
    for arg in args {
        if arg == "--vsix-only" {
            vsix_only = true;
        }
    }

    // Find the syntax-highlighter directory.
    // Strategy: check executable-relative first, then CWD, then parent of CWD.
    let hl_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            let mut p = exe.clone();
            p.pop(); // target/release/ or target/debug/
            p.pop(); // target/
            p.pop(); // project root
            p.push("syntax-highlighter");
            if p.join("package.json").exists() { Some(p) } else { None }
        })
        .or_else(|| {
            let cwd = std::env::current_dir().ok()?;
            let candidates = vec![
                cwd.join("syntax-highlighter"),
                cwd.clone(),
                cwd.parent()?.join("syntax-highlighter"),
            ];
            candidates.into_iter().find(|p| p.join("package.json").exists())
        })
        .ok_or_else(|| "cannot find syntax-highlighter/ directory (looked relative to executable and CWD)".to_string())?;

    eprintln!("Building highlighter .vsix in: {}", hl_dir.display());

    // Run `npx vsce package` (or plain `vsce` if on PATH)
    let vsce_cmd = if std::process::Command::new("vsce").arg("--version").output().is_ok() {
        "vsce"
    } else {
        "npx"
    };
    let mut cmd = std::process::Command::new(vsce_cmd);
    if vsce_cmd == "npx" {
        cmd.args(["--yes", "vsce", "package"]);
    } else {
        cmd.arg("package");
    }
    cmd.current_dir(&hl_dir);

    let status = cmd.status()
        .map_err(|e| format!("failed to run {}: {}", vsce_cmd, e))?;
    if !status.success() {
        return Err("vsce package failed — see output above".into());
    }

    // Find the generated .vsix
    let vsix_path = std::fs::read_dir(&hl_dir)
        .map_err(|e| format!("cannot read highlighter dir: {}", e))?
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().map(|x| x == "vsix").unwrap_or(false))
        .map(|e| e.path())
        .ok_or_else(|| "no .vsix file found after build".to_string())?;

    eprintln!("Built: {}", vsix_path.display());

    if vsix_only {
        return Ok(());
    }

    // Detect editor CLI: prefer codium over code
    let editor = ["codium", "code", "code-insiders"].into_iter()
        .find(|bin| std::process::Command::new(bin).arg("--version").output().is_ok())
        .ok_or_else(|| {
            "no editor CLI found (tried: codium, code, code-insiders). "
                .to_string() + "Install the .vsix manually or use --vsix-only"
        })?;

    let status = std::process::Command::new(&editor)
        .args(["--install-extension", &vsix_path.to_string_lossy()])
        .status()
        .map_err(|e| format!("failed to run {}: {}", editor, e))?;

    if status.success() {
        eprintln!("Installed highlighter via {} — restart the editor to apply.", editor);
    } else {
        return Err(format!("{} --install-extension failed", editor));
    }

    Ok(())
}
