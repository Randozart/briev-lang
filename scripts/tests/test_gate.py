# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
"""Behavioral controls: a successful child alone never establishes a passing gate."""

import contextlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import gate
from gate_evidence import Evidence
from gate_protocol import (GateError, cargo_list, cargo_results, inventory,
                           nextest_list, nextest_results)
from gate_selection import (SOLVER_TESTS, TYPED_BOUNDARY_TESTS, Suite, cargo_command,
                            execution_batches, nextest_command, phases)


def prepared(suite, names=("answer",), ignored=()):
    record = inventory(suite, names, ignored)
    record["tests"] = dict.fromkeys(record["selected"], "unreported")
    return record


def cargo_output(result="ok", passed=1, failed=0):
    verdict = "ok" if failed == 0 else "FAILED"
    return (f"running 1 test\ntest answer ... {result}\n"
            f"test result: {verdict}. {passed} passed; {failed} failed; 0 ignored; "
            "0 measured; 0 filtered out; finished in 0.01s\n")


def nextest_events(suite, result="ok"):
    meta = {"crate": suite.package, "test_binary": suite.name, "kind": suite.kind}
    name = suite.wire_name + "$answer"
    return [
        {"type": "suite", "event": "started", "test_count": 1, "nextest": meta},
        {"type": "test", "event": "started", "name": name},
        {"type": "test", "event": result, "name": name},
        {"type": "suite", "event": result, "passed": int(result == "ok"),
         "failed": int(result == "failed"), "ignored": 0, "measured": 0,
         "filtered_out": 0, "nextest": meta},
    ]


def encode(events):
    return "\n".join(json.dumps(event) for event in events)


class SelectionControls(unittest.TestCase):
    def test_both_runners_select_every_required_binary_and_solver_case(self):
        groups = phases("quick")
        names = {suite.name for group in groups for suite in group}
        self.assertTrue({"unit_test_blocks", "interpreter_corpus_parity", "hir_reclamation_parity",
                         "reduction_scripts", "verify_report", "gate_runner"} <= names)
        for group in groups:
            nextest = nextest_command(group, "run")
            for suite in group:
                cargo = cargo_command(suite, "run")
                self.assertEqual(nextest[nextest.index("--") + 1:], suite.filters())
                self.assertEqual(cargo[-len(suite.filters()):] if suite.filters() else [], suite.filters())
                self.assertIn(suite.package, cargo)
        solver = next(suite for group in groups for suite in group if suite.name == "solver_boundary")
        self.assertEqual(set(solver.only), set(SOLVER_TESTS))
        self.assertEqual(len(prepared(solver, SOLVER_TESTS, SOLVER_TESTS)["selected"]), 3)

    def test_call_fact_regressions_are_required_by_both_gates(self):
        for mode in ("quick", "verifier"):
            matches = [suite for group in phases(mode) for suite in group
                       if suite.package == "briev-verify" and suite.name == "call_fact_scope"]
            self.assertEqual(len(matches), 1)
            self.assertEqual(matches[0].filters(), [])

    def test_typed_regression_is_required_without_enabling_unrelated_ignored_cases(self):
        required = "floating_increment_is_not_proven_by_integer_bitvectors"
        for mode in ("quick", "verifier"):
            suite = next(s for group in phases(mode) for s in group if s.name == "typed_boundary")
            names = (*TYPED_BOUNDARY_TESTS, "optional_device_probe")
            ignored = (required, "optional_device_probe")
            record = prepared(suite, names, ignored)
            self.assertIn(required, record["selected"])
            self.assertEqual(len(record["selected"]), 6)
            self.assertEqual(record["excluded"], {"optional_device_probe": "not_explicitly_selected"})
            for runner in ("cargo", "nextest"):
                self.assertEqual(execution_batches([suite], runner, {suite.identity: record}), [("cargo", [suite])])
            with self.assertRaises(GateError):
                inventory(suite, [name for name in names if name != required], ["optional_device_probe"])

    def test_discovery_retains_exclusions_and_requires_named_tests(self):
        suite = Suite("p", "lib", "p", skip=("conformance_sweep",))
        record = inventory(suite, ["answer", "optional", "conformance_sweep"], ["optional"])
        self.assertEqual(record["selected"], ["answer"])
        self.assertEqual(record["excluded"], {"optional": "ignored", "conformance_sweep": "quick_conformance_exclusion"})
        with self.assertRaises(GateError):
            inventory(Suite("p", "test", "solver", only=("missing",)), ["answer"], [])

    def test_empty_duplicate_and_corrupt_discovery_is_rejected(self):
        suite = Suite("p", "test", "cases")
        for text in ["answer: test\nanswer: test", "answer: bench", "not a test list"]:
            with self.subTest(text=text), self.assertRaises(GateError):
                cargo_list(text)
        with self.assertRaises(GateError):
            inventory(suite, cargo_list(""), [])
        with self.assertRaises(GateError):
            inventory(suite, ["answer"], ["missing"])
        with self.assertRaises(GateError):
            nextest_list('{"rust-suites": {}}', [suite])

    def test_runner_discoveries_have_identical_identity_and_exclusions(self):
        suite = Suite("p", "lib", "p", skip=("conformance_sweep",))
        names = ("answer", "optional", "conformance_sweep")
        data = {"test-count": len(names), "rust-suites": {"p": {"package-name": "p", "binary-name": "p", "kind": "lib",
                "status": "listed", "testcases": {name: {"kind": "test", "ignored": name == "optional"}
                                                   for name in names}}}}
        expected = inventory(suite, cargo_list("\n".join(name + ": test" for name in names)), ["optional"])
        self.assertEqual(nextest_list(json.dumps(data), [suite]), {suite.identity: expected})
        for total in (2, True, -1):
            with self.subTest(total=total), self.assertRaises(GateError):
                nextest_list(json.dumps(dict(data, **{"test-count": total})), [suite])


class ResultControls(unittest.TestCase):
    def setUp(self):
        self.suite = Suite("p", "test", "cases")

    def test_valid_results_and_failed_tests_are_reported(self):
        for cargo_state, nextest_state in [("ok", "ok"), ("FAILED", "failed")]:
            expected = "passed" if cargo_state == "ok" else "failed"
            first, second = prepared(self.suite), prepared(self.suite)
            cargo_results(cargo_output(cargo_state, int(expected == "passed"), int(expected == "failed")), first)
            nextest_results(encode(nextest_events(self.suite, nextest_state)), [self.suite], {self.suite.identity: second})
            self.assertEqual(first["tests"], {"answer": expected})
            self.assertEqual(first, second)

    def test_cargo_rejects_false_success(self):
        good = cargo_output()
        mutations = ["", good.replace("test answer ... ok\n", ""), good + good,
                     good.replace("1 passed", "0 passed"), good.replace("answer", "other"),
                     good.replace("... ok", "... ignored"), good.replace("0 ignored", "1 ignored"),
                     good.replace("0 filtered", "1 filtered"), good.replace("result: ok", "result: FAILED")]
        for text in mutations:
            with self.subTest(text=text), self.assertRaises(GateError):
                cargo_results(text, prepared(self.suite))

    def test_cargo_accounts_for_optional_ignored_and_filtered_tests(self):
        suite = Suite("p", "test", "cases", skip=("filtered",))
        record = prepared(suite, ["answer", "optional", "filtered"], ["optional"])
        text = cargo_output().replace("test result:", "test optional ... ignored, requires device\ntest result:")
        cargo_results(text.replace("0 ignored", "1 ignored").replace("0 filtered", "1 filtered"), record)
        self.assertEqual(record["tests"], {"answer": "passed"})

    def test_cargo_expected_panic_annotation_preserves_identity_and_verdict(self):
        for result, passed, failed in [("ok", 1, 0), ("FAILED", 0, 1)]:
            record = prepared(self.suite)
            text = cargo_output(result, passed, failed).replace("test answer ...", "test answer - should panic ...")
            cargo_results(text, record)
            self.assertEqual(record["tests"], {"answer": "passed" if passed else "failed"})

    def test_nextest_rejects_missing_duplicate_foreign_and_malformed_events(self):
        good = nextest_events(self.suite)
        mutations = [[], good[:-1], good[1:], good[:2] + good[1:], good[:3] + good[2:],
                     [dict(good[0], test_count=0), *good[1:]],
                     [*good[:-1], dict(good[-1], passed=2)],
                     [*good[:-1], dict(good[-1], passed=True)],
                     [*good[:-1], dict(good[-1], ignored=1)],
                     [*good[:-1], dict(good[-1], filtered_out=False)],
                     [good[0], dict(good[1], name="foreign$answer"), *good[2:]],
                     [*good, {"type": "unknown"}]]
        for events in mutations:
            with self.subTest(events=events), self.assertRaises(GateError):
                nextest_results(encode(events), [self.suite], {self.suite.identity: prepared(self.suite)})
        with self.assertRaises(ValueError):
            nextest_results("not JSON", [self.suite], {self.suite.identity: prepared(self.suite)})

    def test_nextest_ignored_reporter_defect_is_rejected_even_with_successful_tests(self):
        good = nextest_events(self.suite)
        premature = dict(good[-1], passed=0, ignored=1)
        wrapped = dict(good[-1], ignored=1, filtered_out=18446744073709551615)
        observed = [*good[:2], premature, *good[:2], premature, good[2], wrapped]
        with self.assertRaises(GateError):
            nextest_results(encode(observed), [self.suite], {self.suite.identity: prepared(self.suite)})

    def test_duplicate_json_fields_cannot_hide_a_failure(self):
        text = encode(nextest_events(self.suite)).replace('"passed": 1', '"passed": 0, "passed": 1')
        with self.assertRaises(GateError):
            nextest_results(text, [self.suite], {self.suite.identity: prepared(self.suite)})


class ExecutionControls(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def evidence(self, runner="cargo", timeout=10):
        return Evidence(self.root, self.root / "results", timeout, "verifier", runner)

    def finish_with(self, evidence, action):
        with patch.object(gate, "execute", side_effect=action), contextlib.redirect_stdout(io.StringIO()), \
                contextlib.redirect_stderr(io.StringIO()):
            status = gate.finish(evidence)
        manifest = json.loads((evidence.directory / "manifest.json").read_text())
        return status, manifest

    def test_missing_tool_retains_infrastructure_failure(self):
        evidence = self.evidence()
        status, manifest = self.finish_with(evidence, lambda e: e.checked([str(self.root / "missing")], "required tool"))
        self.assertNotEqual(status, 0)
        self.assertEqual(manifest["outcome"], "infrastructure_error")
        self.assertEqual(manifest["commands"][0]["outcome"], "infrastructure_error")

    def test_solver_preflight_finds_xdg_managed_install(self):
        managed = self.root / "briev-compiler/bin/z3"
        managed.parent.mkdir(parents=True)
        managed.touch()
        with patch.dict(os.environ, {"XDG_DATA_HOME": str(self.root)}), \
                patch.object(gate.shutil, "which", return_value=None):
            self.assertEqual(gate.solver_path(), str(managed))
        with patch.object(gate.shutil, "which", return_value="/usr/bin/z3"):
            self.assertEqual(gate.solver_path(), "/usr/bin/z3")

    def test_failed_child_cannot_pass_and_logs_are_retained(self):
        evidence = self.evidence()
        code = "import sys; print('before failure'); sys.exit(17)"
        status, manifest = self.finish_with(evidence, lambda e: e.checked([sys.executable, "-c", code], "failing child"))
        self.assertNotEqual(status, 0)
        self.assertEqual(manifest["commands"][0]["exit_code"], 17)
        self.assertIn("before failure", evidence.text(manifest["commands"][0]))

    def test_timeout_is_not_a_successful_empty_run(self):
        evidence = self.evidence(timeout=0.1)
        status, manifest = self.finish_with(evidence, lambda e: e.checked(
            [sys.executable, "-c", "import time; print('started', flush=True); time.sleep(20)"], "hung child"))
        self.assertNotEqual(status, 0)
        self.assertEqual(manifest["outcome"], "timeout")
        self.assertEqual(manifest["commands"][0]["outcome"], "timeout")
        self.assertIn("started", evidence.text(manifest["commands"][0]))

    def test_interruption_and_malformed_protocol_retain_nonpass_manifest(self):
        for error, outcome in [(KeyboardInterrupt(), "interrupted"), (ValueError("malformed"), "infrastructure_error")]:
            evidence = self.evidence()
            status, manifest = self.finish_with(evidence, error)
            self.assertNotEqual(status, 0)
            self.assertEqual(manifest["outcome"], outcome)

    def test_zero_exit_with_no_test_output_fails_actual_adapter(self):
        suite = Suite("p", "test", "cases")
        evidence = self.evidence()
        evidence.data["suites"][suite.identity] = prepared(suite)
        with patch.object(gate, "cargo_command", return_value=[sys.executable, "-c", "pass"]):
            status, manifest = self.finish_with(evidence, lambda e: gate.run_group(e, [suite], "cargo"))
        self.assertNotEqual(status, 0)
        self.assertEqual(manifest["outcome"], "infrastructure_error")
        self.assertEqual(manifest["counts"]["unreported"], 1)

    def test_positive_control_runs_through_both_adapters(self):
        suite = Suite("p", "test", "cases")
        for runner, text in [("cargo", cargo_output()), ("nextest", encode(nextest_events(suite)))]:
            evidence = self.evidence(runner)
            evidence.data["suites"][suite.identity] = prepared(suite)
            command = [sys.executable, "-c", f"print({text!r})"]
            with patch.object(gate, runner + "_command", return_value=command):
                status, manifest = self.finish_with(evidence, lambda e: gate.run_group(e, [suite], runner))
            self.assertEqual(status, 0)
            self.assertEqual(manifest["counts"]["passed"], 1)

    def test_required_ignored_suites_execute_with_cargo_and_record_actual_runner(self):
        suite = Suite("p", "test", "cases", only=("answer",))
        for requested in ("cargo", "nextest"):
            evidence = self.evidence(requested)
            evidence.data["suites"][suite.identity] = prepared(suite, ["answer"], ["answer"])
            command = [sys.executable, "-c", f"print({cargo_output()!r})"]
            runner, batch = execution_batches([suite], requested, evidence.data["suites"])[0]
            with patch.object(gate, "cargo_command", return_value=command):
                status, manifest = self.finish_with(evidence, lambda e: gate.run_group(e, batch, runner))
            self.assertEqual(status, 0)
            self.assertEqual(manifest["counts"]["passed"], 1)
            self.assertEqual(manifest["suites"][suite.identity]["execution_runner"], "cargo")
            self.assertTrue(manifest["suites"][suite.identity]["evidence_valid"])

    def test_optional_ignored_tests_use_cargo_without_enabling_them(self):
        suite, ordinary = Suite("p", "test", "cases"), Suite("p", "test", "ordinary")
        evidence = self.evidence("nextest")
        evidence.data["suites"] = {suite.identity: prepared(suite, ["answer", "optional"], ["optional"]),
                                   ordinary.identity: prepared(ordinary)}
        text = cargo_output().replace("test result:", "test optional ... ignored, optional probe\ntest result:")
        text = text.replace("0 ignored", "1 ignored")
        batches = execution_batches([suite, ordinary], "nextest", evidence.data["suites"])
        self.assertEqual(batches[0], ("nextest", [ordinary]))
        runner, batch = batches[1]
        with patch.object(gate, "cargo_command", return_value=[sys.executable, "-c", f"print({text!r})"]):
            status, manifest = self.finish_with(evidence, lambda e: gate.run_group(e, batch, runner))
        self.assertEqual(status, 0)
        self.assertEqual(manifest["suites"][suite.identity]["tests"], {"answer": "passed"})
        self.assertEqual(manifest["suites"][suite.identity]["excluded"], {"optional": "ignored"})

    def test_conflict_scan_error_does_not_establish_absence(self):
        evidence = self.evidence()
        for code in (0, 2):
            with patch.object(evidence, "command", return_value={"exit_code": code, "stdout": "log"}), self.assertRaises(GateError):
                gate.conflict_guard(evidence)
        with patch.object(evidence, "command", return_value={"exit_code": 1}):
            gate.conflict_guard(evidence)

    def test_artifacts_are_unique_and_credentials_are_not_recorded(self):
        with patch.dict(os.environ, {"CARGO_REGISTRY_TOKEN": "secret", "CARGO_BUILD_JOBS": "1"}):
            first, second = self.evidence(), self.evidence()
        self.assertNotEqual(first.directory, second.directory)
        self.assertNotIn("CARGO_REGISTRY_TOKEN", first.data["environment"])
        self.assertEqual(first.data["environment"]["CARGO_BUILD_JOBS"], "1")


if __name__ == "__main__":
    unittest.main(verbosity=2)
