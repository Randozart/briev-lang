#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
"""Exercise the actual Briev exporter and Lean checker, including rejection controls."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys

CASES = {
    "cancel": "defn f(x: Int64) [true][term == x] -> Int64 { let y: Int64 = x + 1; y = y - 1; term y; };",
    "overflow": "defn f(x: Int64) [true][term > x] -> Int64 { term x + 1; };",
    "false": "defn f(x: Int64) [true][term == 0] -> Int64 { term x; };",
}
CONTROLS = {
    "hash-drift": "hash mismatch",
    "missing-report": "missing or duplicate",
    "forbidden-axiom": "unapproved proof dependency",
}


def run(command, log):
    result = subprocess.run(command, capture_output=True, text=True, timeout=180)
    log.write_text(result.stdout + result.stderr)
    return result


def contract_case(exporter, artifacts, checker, name, source):
    path = artifacts / (name + ".bv")
    path.write_text(source + "\n")
    directory = artifacts / name
    generation = run(
        [str(exporter), str(path), "f", str(directory)],
        artifacts / (name + "-generation.log"),
    )
    if generation.returncode:
        raise RuntimeError(f"export generation failed for {name}")
    result = run(
        [sys.executable, str(checker), str(directory)],
        artifacts / (name + "-check.log"),
    )
    return assert_contract_result(name, directory, result)


def assert_contract_result(name, directory, result):
    checked = (directory / "checked.json").exists()
    certified = result.returncode == 0 and checked
    if name == "cancel":
        if not certified:
            raise RuntimeError("positive exported contract was not checked")
        return "checked-exported-model"
    lean_log = (directory / "Program.lean.log").read_text()
    if (
        result.returncode != 1
        or "omega could not prove the goal" not in lean_log
        or checked
    ):
        raise RuntimeError(
            f"false contract {name} did not fail at its proof obligation"
        )
    return "proof-obligation-rejected"


def control_program(control, text):
    if control == "missing-report":
        return text.replace("#print axioms contract_checked", "")
    if control == "forbidden-axiom":
        return "import IntegerVC\nnamespace BrievExport\naxiom untrusted : False\ntheorem contract_checked : False := untrusted\n#print axioms contract_checked\nend BrievExport\n"
    return text + "\n-- modified after export\n"


def checker_control(artifacts, checker, control):
    directory = artifacts / control
    shutil.copytree(artifacts / "cancel", directory)
    program = directory / "Program.lean"
    program.write_text(control_program(control, program.read_text()))
    if control != "hash-drift":
        manifest_path = directory / "manifest.json"
        manifest = json.loads(manifest_path.read_text())
        manifest["program_sha256"] = hashlib.sha256(program.read_bytes()).hexdigest()
        manifest_path.write_text(json.dumps(manifest))
    result = run(
        [sys.executable, str(checker), str(directory)], artifacts / (control + ".log")
    )
    if (
        result.returncode != 1
        or CONTROLS[control] not in result.stderr
        or (directory / "checked.json").exists()
    ):
        raise RuntimeError(f"checker control failed: {control}")
    return "rejected"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exporter", type=Path)
    parser.add_argument("artifacts", type=Path)
    args = parser.parse_args()
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=False)
    checker = Path(__file__).with_name("check_export.py").resolve()
    exporter = args.exporter.resolve(strict=True)
    results = {
        name: contract_case(exporter, artifacts, checker, name, source)
        for name, source in CASES.items()
    }
    results.update(
        {name: checker_control(artifacts, checker, name) for name in CONTROLS}
    )
    (artifacts / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    print(
        "Export checks: 1 contract checked, 2 false contracts and 3 invalid-evidence controls rejected."
    )


if __name__ == "__main__":
    main()
