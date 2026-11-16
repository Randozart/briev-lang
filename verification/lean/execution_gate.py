#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
"""Finite interpreter/Lean/native comparison; this is not native refinement proof."""

import argparse
import hashlib
from collections import Counter
import itertools
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

BOUNDARIES = (0, 1, -1, 2, -2, 2**63 - 1, -(2**63), 2**63 - 2, -(2**63) + 1)
CASES = {
    "add": ("term x + y;", "x + y", lambda x, y, z: x + y),
    "subtract": ("term x - y;", "x - y", lambda x, y, z: x - y),
    "multiply": ("term x * y;", "x * y", lambda x, y, z: x * y),
    "negate": ("term -x;", "-x", lambda x, y, z: -x),
    "snapshot": (
        "let a: Int64 = x; let b: Int64 = a; a = y; term b;",
        "x",
        lambda x, y, z: x,
    ),
    "mutation": (
        "let a: Int64 = x + y; a = a * z; let b: Int64 = a - x; a = z; term b;",
        "(x + y) * z - x",
        lambda x, y, z: (x + y) * z - x,
    ),
    "polynomial": (
        "term (x * y + z) * (x - y);",
        "(x * y + z) * (x - y)",
        lambda x, y, z: (x * y + z) * (x - y),
    ),
    "alias": ("term x + y;", "x + y", lambda x, y, z: x + y),
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def signed(value):
    return (value + 2**63) % 2**64 - 2**63


def run(command, directory, log, environment, expected=0):
    result = subprocess.run(
        [str(arg) for arg in command],
        cwd=directory,
        env=environment,
        capture_output=True,
        text=True,
        timeout=180,
    )
    log.write_text(result.stdout + result.stderr)
    if result.returncode != expected:
        raise RuntimeError(f"unexpected exit {result.returncode}: inspect {log}")
    return result


def definition(name):
    body, post, _ = CASES[name]
    ty = "Scalar" if name == "alias" else "Int64"
    prefix = "type Scalar: Int64;\n" if name == "alias" else ""
    return (
        prefix
        + f"defn f(x: {ty}, y: {ty}, z: {ty}) [true][term == {post}] -> {ty} {{ {body} }};\n"
    )


def native_source(source, name):
    word_type = "Scalar" if name == "alias" else "Int64"
    return (
        'import { print_int, print_str } from "std/io";\n'
        + source
        + """
let done: Bool = false;
node entry [!done][done] {
    let x: Int = get_env_int!("LEAN_X");
    let y: Int = get_env_int!("LEAN_Y");
    let z: Int = get_env_int!("LEAN_Z");
    print_int(f(x as SampleInput, y as SampleInput, z as SampleInput) as Int); print_str("\\n");
    done = true;
    endprogram;
};
""".replace("SampleInput", word_type)
    )


def compare_values(actual, expected, context):
    if len(actual) != len(expected):
        raise RuntimeError(f"result count differs: {context}")
    for index, (value, wanted) in enumerate(zip(actual, expected)):
        if value != wanted:
            raise RuntimeError(
                f"value mismatch: {context}, sample {index}: {value} != {wanted}"
            )


def check_lean(directory, inputs, expected, environment):
    from check_export import lean_context

    context, lean, _, _ = lean_context(directory, environment["LEAN_BIN"])
    lines = ["import Program", "namespace BrievExecution"]
    for index, (arguments, value) in enumerate(zip(inputs, expected)):
        args = " ".join(f"({x % 2**64} : BrievExport.Word)" for x in arguments)
        lines += [
            f"theorem sample_{index} : BrievExport.program {args} = ({value % 2**64} : BrievExport.Word) := by decide",
            f"#print axioms sample_{index}",
        ]
    (directory / "Samples.lean").write_text(
        "\n".join(lines + ["end BrievExecution", ""])
    )
    result = run([lean, "Samples.lean"], directory, directory / "samples.log", context)
    reports = re.findall(
        r"'BrievExecution\.sample_(\d+)' does not depend on any axioms", result.stdout
    )
    if Counter(reports) != Counter(str(i) for i in range(len(inputs))):
        raise RuntimeError(
            "missing, duplicate or axiom-dependent sampled theorem report"
        )
    wrong = signed(expected[0] + 1) % 2**64
    arguments = " ".join(f"({x % 2**64} : BrievExport.Word)" for x in inputs[0])
    (directory / "WrongSample.lean").write_text(
        f"import Program\nexample : BrievExport.program {arguments} = ({wrong} : BrievExport.Word) := by decide\n"
    )
    rejected = run(
        [lean, "WrongSample.lean"],
        directory,
        directory / "wrong-sample.log",
        context,
        expected=1,
    )
    if (
        "Tactic `decide` proved that the proposition" not in rejected.stdout
        or "is false" not in rejected.stdout
    ):
        raise RuntimeError("wrong result did not fail for the intended logical reason")


def native_values(binary, inputs, environment, directory):
    values = []
    for index, arguments in enumerate(inputs):
        env = dict(
            environment,
            **dict(zip(("LEAN_X", "LEAN_Y", "LEAN_Z"), map(str, arguments))),
        )
        result = run([binary], directory, directory / f"sample-{index}.log", env)
        values.append(int(result.stdout.strip()))
    return values


def check_native(config, name, directory, inputs, expected):
    path = directory / "native.bv"
    path.write_text(native_source(definition(name), name))
    binaries = {}
    for hir, level in itertools.product(("0", "1"), ("0", "3")):
        variant = directory / f"hir-{hir}-O{level}"
        variant.mkdir()
        environment = dict(config.environment, BRIEV_HIR_EMIT=hir)
        ir_dir = directory / f"ir-{hir}"
        if not ir_dir.exists():
            run(
                [config.compiler, "build", path, "--llvm", "--out", ir_dir],
                config.repo,
                directory / f"build-{hir}.log",
                environment,
            )
        binary = variant / "program"
        run(
            [
                config.clang,
                f"-O{level}",
                ir_dir / "native.ll",
                config.repo / "lib/runtime/briev_rt.c",
                "-lm",
                "-ldl",
                "-o",
                binary,
            ],
            config.repo,
            variant / "link.log",
            environment,
        )
        compare_values(
            native_values(binary, inputs, environment, variant), expected, str(variant)
        )
        binaries[str(binary.relative_to(directory))] = digest(binary)
    return binaries


def check_case(config, name, inputs):
    directory = config.artifacts / name
    directory.mkdir()
    source = definition(name)
    path = directory / "source.bv"
    path.write_text(source)
    expected = [signed(CASES[name][2](*arguments)) for arguments in inputs]
    (directory / "expected.json").write_text(json.dumps(expected))
    vectors = directory / "inputs.json"
    vectors.write_text(json.dumps(inputs))
    export = directory / "export"
    run(
        [config.exporter, path, "f", export],
        config.repo,
        directory / "export.log",
        config.environment,
    )
    run(
        [
            sys.executable,
            Path(__file__).with_name("check_export.py"),
            export,
            "--lean",
            config.environment["LEAN_BIN"],
        ],
        config.repo,
        directory / "contract.log",
        config.environment,
    )
    check_lean(export, inputs, expected, config.environment)
    actual = run(
        [config.interpreter, path, "f", vectors],
        config.repo,
        directory / "interpreter.log",
        config.environment,
    )
    compare_values(json.loads(actual.stdout), expected, f"{name}: interpreter")
    binaries = check_native(config, name, directory, inputs, expected)
    print(
        f"{name}: contract, {len(inputs)} Lean/interpreter samples and four native variants agree",
        flush=True,
    )
    return {
        "samples": len(inputs),
        "native_variants": 4,
        "native_binaries": binaries,
        "false_lean_result": "rejected",
    }


def native_negative_control(config, inputs):
    directory = config.artifacts / "wrong-native"
    directory.mkdir()
    expected = [signed(x + y) for x, y, _ in inputs]
    # A valid subtracting program must fail against the unchanged addition oracle
    # at execution comparison, not during compilation.
    try:
        check_native(config, "subtract", directory, inputs, expected)
    except RuntimeError as error:
        if str(error).startswith("value mismatch:"):
            (directory / "rejection.txt").write_text(str(error) + "\n")
            return "native-value-mismatch-rejected"
        raise
    raise RuntimeError("altered native program was accepted")


def main():
    from check_export import lean_context

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("compiler", "exporter", "interpreter", "artifacts"):
        parser.add_argument(name, type=Path)
    parser.add_argument("--clang", default=shutil.which("clang"))
    parser.add_argument(
        "--lean", default=shutil.which("lean") or str(Path.home() / ".elan/bin/lean")
    )
    config = parser.parse_args()
    config.repo = Path(__file__).resolve().parents[2]
    config.artifacts = config.artifacts.resolve()
    for name in ("compiler", "exporter", "interpreter"):
        setattr(config, name, getattr(config, name).resolve(strict=True))
    if config.clang is None:
        raise FileNotFoundError("clang is not installed")
    config.clang = str(Path(config.clang).resolve(strict=True))
    config.environment = dict(
        os.environ,
        BRIEV_HOME=str(config.repo),
        BRIEV_STDLIB_PATH=str(config.repo / "lib"),
        LEAN_BIN=config.lean,
    )
    config.artifacts.mkdir(parents=True, exist_ok=False)
    _, _, _, lean = lean_context(config.artifacts, config.lean)
    tools = {
        name: {"path": str(path), "sha256": digest(path)}
        for name, path in {
            "compiler": config.compiler,
            "exporter": config.exporter,
            "interpreter": config.interpreter,
            "clang": Path(config.clang),
            "lean": lean,
            "gate": Path(__file__),
            "checker": Path(__file__).with_name("check_export.py"),
            "interpreter_source": config.repo / "examples/lean_interpret.rs",
        }.items()
    }
    inputs = [
        [x, y, BOUNDARIES[(i * 3 + j * 5) % len(BOUNDARIES)]]
        for i, x in enumerate(BOUNDARIES)
        for j, y in enumerate(BOUNDARIES)
    ]
    results = {name: check_case(config, name, inputs) for name in CASES}
    control = native_negative_control(config, inputs)
    if any(digest(Path(tool["path"])) != tool["sha256"] for tool in tools.values()):
        raise RuntimeError("tool or source changed during execution checks")
    record = {
        "schema": "briev-execution-comparison/v1",
        "status": "finite-differential-checks",
        "cases": results,
        "native_negative_control": control,
        "tools": tools,
        "source_native_refinement": "unproved",
        "ast_serialization_refinement": "unproved",
    }
    (config.artifacts / "results.json").write_text(json.dumps(record, indent=2) + "\n")
    hashes = {
        str(path.relative_to(config.artifacts)): digest(path)
        for path in config.artifacts.rglob("*")
        if path.is_file()
    }
    (config.artifacts / "artifact-hashes.json").write_text(
        json.dumps(hashes, indent=2) + "\n"
    )


if __name__ == "__main__":
    sys.dont_write_bytecode = True
    try:
        main()
    except FileNotFoundError as error:
        print(f"Execution comparison unavailable: {error}", file=sys.stderr)
        sys.exit(2)
