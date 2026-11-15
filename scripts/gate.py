#!/usr/bin/env python3
# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
"""Account for the existing quick/verifier gates; never equate zero tests with pass."""

import argparse
from dataclasses import asdict
import os
from pathlib import Path
import platform
import shutil
import signal
import sys

from gate_evidence import Evidence, timestamp
from gate_protocol import (GateError, cargo_list, cargo_results, inventory,
                           nextest_list, nextest_results, require)
from gate_selection import cargo_command, execution_batches, nextest_command, phases


def options():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", nargs="?", choices=("quick", "verifier"), default="quick")
    parser.add_argument("--runner", choices=("auto", "nextest", "cargo"), default="auto")
    parser.add_argument("--artifacts", type=Path, default=Path("target/gate-results"))
    parser.add_argument("--timeout-seconds", type=int, default=1800)
    args = parser.parse_args()
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")
    return args


def choose_runner(requested):
    if requested != "auto":
        return requested
    return "nextest" if shutil.which("cargo-nextest") else "cargo"


def preflight(evidence):
    evidence.data["host"] = platform.platform()
    evidence.data["tools"]["python"] = sys.version
    evidence.data["revision"] = evidence.checked(["git", "rev-parse", "HEAD"], "revision").strip()
    evidence.data["working_tree"] = evidence.checked(
        ["git", "status", "--porcelain=v1"], "working tree").splitlines()
    conflict_guard(evidence)
    tools = {"cargo": ["cargo", "--version"], "rustc": ["rustc", "-vV"],
             "z3": [solver_path(), "--version"]}
    if evidence.data["mode"] == "quick":
        tools.update({name: [name, "--version"] for name in ("clang", "opt", "llc", "timeout")})
    if evidence.data["runner"] == "nextest":
        tools["nextest"] = ["cargo", "nextest", "--version"]
    for name, argv in tools.items():
        evidence.data["tools"][name] = evidence.checked(argv, f"required tool {name}").strip()
    evidence.save()


def solver_path():
    path = shutil.which("z3")
    if path:
        return path
    # 2026-11-15: mirror briev_verify::smt's managed install resolution, including
    # XDG_DATA_HOME; retire this copy when the verifier exposes a tool-info command.
    base = Path(os.environ["XDG_DATA_HOME"]) if "XDG_DATA_HOME" in os.environ else Path.home() / ".local/share"
    managed = base / "briev-compiler/bin/z3"
    return str(managed) if managed.is_file() else "z3"


def conflict_guard(evidence):
    record = evidence.command(
        ["git", "grep", "-nE", r"^(<<<<<<< |>>>>>>> |=======$)", "--", ":!target"],
        "conflict markers")
    if record["exit_code"] == 0:
        raise GateError(f"conflict markers found; see {record['stdout']}", "fail")
    require(record["exit_code"] == 1, "conflict-marker scan failed; absence was not established")
    record["outcome"] = "pass"
    record["expected_exit_code"] = 1
    evidence.save()


def discover(evidence, suites):
    if evidence.data["runner"] == "nextest":
        text = evidence.checked(nextest_command(suites, "list"), "nextest discovery")
        return nextest_list(text, suites)
    found = {}
    for suite in suites:
        names = cargo_list(evidence.checked(cargo_command(suite, "list"), f"list {suite.identity}"))
        ignored = cargo_list(evidence.checked(cargo_command(suite, "ignored"), f"ignored {suite.identity}"))
        found[suite.identity] = inventory(suite, names, ignored)
    return found


def run_group(evidence, suites, runner):
    require(runner == "nextest" or len(suites) == 1, "Cargo execution needs one target per command")
    records = evidence.data["suites"]
    for suite in suites:
        record = records[suite.identity]
        record["tests"] = dict.fromkeys(record["selected"], "unreported")
        record.update(execution_runner=runner, evidence_valid=False)
    if runner == "nextest":
        command = nextest_command(suites, "run")
    else:
        command = cargo_command(suites[0], "run")
    run = evidence.command(command, "execute " + ", ".join(s.identity for s in suites))
    try:
        parse_results(evidence, suites, run, runner)
    except (GateError, ValueError, KeyError, TypeError, AttributeError, IndexError) as error:
        outcome = "fail" if run["exit_code"] else "infrastructure_error"
        raise GateError(f"incomplete or invalid test evidence: {error}", outcome) from error
    for suite in suites:
        records[suite.identity]["evidence_valid"] = True
    require(run["exit_code"] == 0 or any(
        "failed" in records[s.identity]["tests"].values() for s in suites),
        f"test process exited {run['exit_code']} without reporting a failed test")
    if run["exit_code"] or any("failed" in records[s.identity]["tests"].values() for s in suites):
        raise GateError("required tests failed; see execution logs", "fail")
    evidence.save()


def parse_results(evidence, suites, run, runner):
    records = evidence.data["suites"]
    if runner == "nextest":
        nextest_results(evidence.text(run), suites, records)
        return
    cargo_results(evidence.text(run), records[suites[0].identity])


def execute(evidence):
    groups = phases(evidence.data["mode"])
    evidence.data["selection"] = [asdict(suite) for group in groups for suite in group]
    preflight(evidence)
    for group in groups:
        found = discover(evidence, group)
        for record in found.values():
            record.update(discovery_runner=evidence.data["runner"], execution_runner=None,
                          evidence_valid=False)
        evidence.data["suites"].update(found)
        evidence.save()
    for group in groups:
        for runner, batch in execution_batches(group, evidence.data["runner"], evidence.data["suites"]):
            print("gate: " + ", ".join(s.identity for s in batch), flush=True)
            run_group(evidence, batch, runner)
    require(evidence.data["counts"]["selected"] > 0, "zero tests executed")
    require(evidence.data["counts"]["passed"] == evidence.data["counts"]["selected"],
            "not every required test passed")


def finish(evidence):
    try:
        execute(evidence)
        evidence.data["outcome"] = "pass"
        return 0
    except GateError as error:
        evidence.data.update(outcome=error.outcome, error=str(error))
        return 1
    except (OSError, ValueError, KeyError, TypeError, AttributeError, IndexError) as error:
        evidence.data.update(outcome="infrastructure_error", error=str(error))
        return 1
    except KeyboardInterrupt:
        evidence.data.update(outcome="interrupted", error="gate interrupted")
        return 130
    finally:
        evidence.data["finished_at"] = timestamp()
        evidence.save()
        print(f"gate: {evidence.data['outcome']}; {evidence.directory / 'manifest.json'}", flush=True)
        if "error" in evidence.data:
            print(evidence.data["error"], file=sys.stderr)


def interrupted(_signal, _frame):
    raise KeyboardInterrupt


def main():
    args = options()
    root = Path(__file__).resolve().parent.parent
    runner = choose_runner(args.runner)
    evidence = Evidence(root, args.artifacts, args.timeout_seconds, args.mode, runner)
    evidence.data["requested_runner"] = args.runner
    signal.signal(signal.SIGTERM, interrupted)
    return finish(evidence)


if __name__ == "__main__":
    sys.exit(main())
