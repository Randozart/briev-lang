#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
"""Check a generated export, retain logs, and bind the result to exact artifacts."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

INPUT_NAMES = [
    "manifest.json",
    "Program.lean",
    "source.bv",
    "typed.bv",
    "IntegerVC.lean",
    "lean-toolchain",
]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def input_hashes(directory):
    return {name: digest(directory / name) for name in INPUT_NAMES}


def validate_inputs(directory):
    manifest = json.loads((directory / "manifest.json").read_text())
    if (
        manifest.get("schema") != "briev-lean-export/v1"
        or manifest.get("status") != "generated-unchecked"
    ):
        raise ValueError("unsupported export manifest")
    keys = {
        "Program.lean": "program_sha256",
        "source.bv": "source_sha256",
        "typed.bv": "typed_source_sha256",
        "IntegerVC.lean": "model_sha256",
    }
    for name, key in keys.items():
        if digest(directory / name) != manifest[key]:
            raise ValueError(f"export hash mismatch: {name}")
    # Bind the import to the reviewed model, rather than a manifest authorizing
    # an arbitrary replacement model with an updated hash.
    if digest(directory / "IntegerVC.lean") != digest(
        Path(__file__).with_name("IntegerVC.lean")
    ):
        raise ValueError("export model differs from reviewed model")
    return input_hashes(directory)


def lean_context(directory, lean):
    environment = dict(os.environ, LEAN_PATH=str(directory))
    executable = str(Path(lean).resolve(strict=True))
    version = subprocess.run(
        [executable, "--version"],
        cwd=directory,
        env=environment,
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout.strip()
    if not re.search(r"version 4\.24\.0,", version):
        raise ValueError(f"unexpected Lean version: {version}")
    prefix = subprocess.run(
        [executable, "--print-prefix"],
        cwd=directory,
        env=environment,
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout.strip()
    return environment, executable, version, Path(prefix) / "bin" / "lean"


def run_lean(directory, executable, environment):
    logs = []
    for filename in ["IntegerVC.lean", "Program.lean"]:
        command = [
            executable,
            "-o",
            str(directory / (filename.removesuffix(".lean") + ".olean")),
            filename,
        ]
        result = subprocess.run(
            command,
            cwd=directory,
            env=environment,
            capture_output=True,
            text=True,
            timeout=120,
        )
        log = result.stdout + result.stderr
        (directory / (filename + ".log")).write_text(log)
        if result.returncode:
            raise ValueError(f"Lean rejected {filename}; inspect {filename}.log")
        logs.append(log)
    return logs


def contract_axioms(logs):
    pattern = r"'BrievExport\.contract_checked' (?:depends on axioms: \[([^]]*)\]|does not depend on any axioms)"
    reports = re.findall(pattern, logs[1])
    if len(reports) != 1:
        raise ValueError("missing or duplicate contract axiom report")
    axioms = set(reports[0].split(", ")) if reports[0] else set()
    forbidden = ["sorryAx", "Lean.ofReduceBool", "declaration uses 'sorry'"]
    if axioms - {"propext"} or any(token in "\n".join(logs) for token in forbidden):
        raise ValueError(f"unapproved proof dependency: {sorted(axioms)}")
    return sorted(axioms)


def check(directory, lean):
    directory = directory.resolve(strict=True)
    checked = directory / "checked.json"
    # A failed recheck must never leave a previous success marker behind.
    checked.unlink(missing_ok=True)
    inputs = validate_inputs(directory)
    environment, executable, version, tool = lean_context(directory, lean)
    tool_hash = digest(tool)
    axioms = contract_axioms(run_lean(directory, executable, environment))
    files = INPUT_NAMES + [
        "Program.olean",
        "IntegerVC.olean",
        "Program.lean.log",
        "IntegerVC.lean.log",
    ]
    if inputs != input_hashes(directory) or tool_hash != digest(tool):
        raise ValueError("inputs or Lean tool changed during verification")
    checked.write_text(
        json.dumps(
            {
                "status": "checked-exported-model",
                "lean": version,
                "theorem": "BrievExport.contract_checked",
                "axioms": axioms,
                "lean_binary_sha256": tool_hash,
                "files": {name: digest(directory / name) for name in files},
                "checker_sha256": digest(Path(__file__)),
                "exporter_refinement": "unproved",
                "source_native_refinement": "unproved",
            },
            indent=2,
        )
        + "\n"
    )
    print(
        "Checked exported-model contract; source/native and exporter refinement remain unproved."
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument(
        "--lean", default=shutil.which("lean") or str(Path.home() / ".elan/bin/lean")
    )
    args = parser.parse_args()
    try:
        check(args.directory, args.lean)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, f"Export not certified: {error}\n")


if __name__ == "__main__":
    main()
