# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
"""The required quick/verifier test selections, independent of the Rust runner."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Suite:
    package: str
    kind: str
    name: str
    skip: tuple = ()
    only: tuple = ()

    @property
    def identity(self):
        return f"{self.package}/{self.kind}/{self.name}"

    @property
    def wire_name(self):
        return f"{self.package}::{self.name}"

    def target_args(self):
        return ["--lib"] if self.kind == "lib" else ["--test", self.name]

    def filters(self):
        if self.only:
            return ["--include-ignored", "--exact", *self.only]
        return [arg for name in self.skip for arg in ["--skip", name]]


# 2026-11-15: one list fixes the five-suite cargo fallback coverage loss.
# Extend this list for both runners; retire only if Cargo gains a shared suite manifest.
QUICK_INTEGRATIONS = (
    "diagnostics", "hir_snapshots", "hold_law", "publish_reclamation",
    "deterministic_transitions", "unit_test_blocks", "interpreter_corpus_parity",
    "hir_reclamation_parity", "reduction_scripts", "verify_report",
    "integer_wrapping", "cli_args_test", "gate_runner",
)
SOLVER_TESTS = (
    "solver_status_is_not_taken_from_model_or_error_text",
    "a_sat_model_named_unsat_cannot_prove_a_false_body_contract",
    "wrapping_addition_cannot_prove_unbounded_monotonicity",
)


def phases(mode):
    verifier = [
        [Suite("briev-verify", "lib", "briev_verify"),
         Suite("briev-verify", "test", "arithmetic_boundary")],
        [Suite("briev-verify", "test", "solver_boundary", only=SOLVER_TESTS)],
    ]
    if mode == "verifier":
        return verifier
    return [
        [Suite("briev-compiler", "lib", "briev_compiler", skip=("conformance_sweep",))],
        [Suite("briev-compiler", "test", name) for name in QUICK_INTEGRATIONS],
        *verifier,
    ]


def execution_batches(suites, runner, records):
    # TEMP: 2026-11-15: nextest 0.9.151's JSON reporter double-counts ignored
    # cases, whether enabled or skipped. Keep discovery on nextest and execute
    # affected binaries with Cargo; retire after both ignored-suite probes pass
    # on the pinned reporter. Unaffected binaries retain batched nextest runs.
    if runner != "nextest":
        return [("cargo", [suite]) for suite in suites]
    regular = [s for s in suites if not s.only and not records[s.identity]["ignored"]]
    batches = [(runner, regular)] if regular else []
    return batches + [("cargo", [suite]) for suite in suites if suite not in regular]


def cargo_command(suite, operation):
    command = ["cargo", "test", "--color", "never", "-p", suite.package,
               *suite.target_args(), "--"]
    if operation == "run":
        return command + ["--format", "pretty", "--color", "never",
                          "--test-threads=4", *suite.filters()]
    command += ["--list", "--format", "terse"]
    return command + (["--ignored"] if operation == "ignored" else [])


def nextest_command(suites, operation):
    first = suites[0]
    assert all((s.package, s.skip, s.only) == (first.package, first.skip, first.only)
               for s in suites), "a nextest phase must have one package/filter policy"
    targets = [arg for suite in suites for arg in suite.target_args()]
    command = ["cargo", "nextest", operation, "--color", "never", "-p", first.package,
               *targets, "--ignore-default-filter"]
    if operation == "list":
        return command + ["--message-format", "json"]
    return command + ["-j", "4", "--retries", "0", "--no-fail-fast", "--no-tests", "fail",
                      "--message-format", "libtest-json-plus",
                      "--message-format-version", "0.1", "--", *first.filters()]
