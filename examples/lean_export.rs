// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Run with: cargo run --example lean_export -- source.bv function fresh-directory
use briev_compiler::backend::lean::{MODEL, TOOLCHAIN, export_source};
use std::{error::Error, fs, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: lean_export source.bv function fresh-output-directory".into());
    }
    let source = fs::read_to_string(&args[0])?;
    let export = export_source(&args[0], &source, &args[1])?;
    let output = Path::new(&args[2]);
    fs::create_dir(output)?;
    for (name, text) in [
        ("Program.lean", export.program.as_str()),
        ("source.bv", export.source.as_str()),
        ("typed.bv", export.typed_source.as_str()),
        ("IntegerVC.lean", MODEL),
        ("lean-toolchain", TOOLCHAIN),
    ] {
        fs::write(output.join(name), text)?;
    }
    fs::write(
        output.join("manifest.json"),
        serde_json::to_string_pretty(&export.manifest)?,
    )?;
    println!("Generated unchecked export: {}", output.display());
    Ok(())
}
