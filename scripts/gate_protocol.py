# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
"""Strict adapters for nextest JSON 0.1 and stable Cargo/libtest text output."""

import json
import re


class GateError(Exception):
    def __init__(self, message, outcome="infrastructure_error"):
        super().__init__(message)
        self.outcome = outcome


def require(condition, message):
    if not condition:
        raise GateError(message)


def json_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def count(value):
    require(type(value) is int and value >= 0, "invalid test count")
    return value


def inventory(suite, names, ignored):
    names, ignored = set(names), set(ignored)
    require(all(isinstance(name, str) and name for name in names), "invalid test identity")
    require(ignored <= names, f"{suite.identity}: unknown ignored test")
    require(set(suite.only) <= names, f"{suite.identity}: a required named test is missing")
    excluded = {name: exclusion(suite, name, ignored) for name in sorted(names)}
    selected = [name for name, reason in excluded.items() if reason is None]
    require(selected, f"{suite.identity}: zero selected tests")
    return {"package": suite.package, "kind": suite.kind, "binary": suite.name,
            "selected": selected, "ignored": sorted(ignored),
            "excluded": {name: reason for name, reason in excluded.items() if reason},
            "tests": {name: "not_run" for name in selected}}


def exclusion(suite, name, ignored):
    if suite.only:
        return None if name in suite.only else "not_explicitly_selected"
    if any(part in name for part in suite.skip):
        return "quick_conformance_exclusion"
    return "ignored" if name in ignored else None


def cargo_list(text):
    names = []
    for line in text.splitlines():
        if not line.strip():
            continue
        require(line.endswith(": test"), f"unrecognized libtest listing: {line!r}")
        names.append(line[:-6])
    require(len(names) == len(set(names)), "duplicate libtest test identity")
    return names


def nextest_list(text, suites):
    data = json.loads(text, object_pairs_hook=json_object)
    listed = data["rust-suites"]
    by_wire = {suite.wire_name: suite for suite in suites}
    found = {}
    for value in listed.values():
        suite = by_wire.get(f"{value['package-name']}::{value['binary-name']}")
        require(suite is not None, "nextest listed an unexpected binary")
        require(value["status"] == "listed" and value["kind"] == suite.kind,
                f"{suite.identity}: nextest did not list the expected target")
        require(suite.identity not in found, "duplicate nextest binary")
        cases = value["testcases"]
        require(all(case["kind"] == "test" and type(case["ignored"]) is bool
                    for case in cases.values()), "invalid libtest case")
        ignored = [name for name, case in cases.items() if case["ignored"]]
        found[suite.identity] = inventory(suite, cases, ignored)
    require(set(found) == {suite.identity for suite in suites}, "nextest omitted a required binary")
    require(count(data["test-count"]) == sum(len(r["selected"]) + len(r["excluded"]) for r in found.values()),
            "nextest discovery total mismatch")
    return found


def terminal(record, name, result):
    require(name in record["tests"], f"unexpected terminal test result: {name}")
    require(record["tests"][name] == "unreported", f"duplicate terminal result: {name}")
    record["tests"][name] = result


def complete(record, passed, failed):
    passed, failed = count(passed), count(failed)
    values = list(record["tests"].values())
    require(values.count("passed") == passed and values.count("failed") == failed,
            "suite totals disagree with terminal test records")
    require(passed + failed == len(values), "missing terminal test results")


def cargo_results(text, record):
    summaries = []
    ignored = set()
    for line in text.splitlines():
        match = re.fullmatch(r"test (.+?)(?: - should panic)? \.\.\. (ok|FAILED|ignored(?:,.*)?)", line)
        if match:
            cargo_terminal(record, match, ignored)
        if line.startswith("test result:"):
            summaries.append(line)
    require(len(summaries) == 1, "expected exactly one libtest summary")
    summary = re.fullmatch(
        r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; "
        r"0 measured; (\d+) filtered out; finished in [0-9.]+s", summaries[0])
    require(summary is not None, "malformed libtest summary")
    passed, failed, skipped, filtered = map(int, summary.groups()[1:])
    require((summary[1] == "ok") == (failed == 0), "inconsistent libtest suite verdict")
    require(skipped == len(ignored), "libtest ignored total mismatch")
    require(skipped + filtered == len(record["excluded"]), "libtest exclusion total mismatch")
    complete(record, passed, failed)


def cargo_terminal(record, match, ignored):
    name, result = match.groups()
    if not result.startswith("ignored"):
        terminal(record, name, "passed" if result == "ok" else "failed")
        return
    require(record["excluded"].get(name) == "ignored", "a required test was ignored")
    require(name not in ignored, "duplicate ignored test")
    ignored.add(name)


def nextest_results(text, suites, records):
    by_wire = {suite.wire_name: records[suite.identity] for suite in suites}
    started, finished, tests = set(), set(), set()
    for line in text.splitlines():
        event = json.loads(line, object_pairs_hook=json_object)
        if event["type"] == "suite":
            nextest_suite(event, by_wire, started, finished)
            continue
        require(event["type"] == "test", "unknown nextest event type")
        wire, separator, name = event["name"].partition("$")
        require(separator and wire in started and wire not in finished, "test outside active suite")
        record = by_wire[wire]
        require(name in record["tests"], "nextest ran an unselected test")
        if event["event"] == "started":
            require(event["name"] not in tests, "duplicate nextest test start")
            tests.add(event["name"])
            continue
        require(event["event"] in ("ok", "failed") and event["name"] in tests,
                "unexpected nextest test result")
        terminal(record, name, "passed" if event["event"] == "ok" else "failed")
    require(started == finished == set(by_wire), "missing nextest suite events")


def nextest_suite(event, records, started, finished):
    metadata = event["nextest"]
    wire = f"{metadata['crate']}::{metadata['test_binary']}"
    require(wire in records and metadata["kind"] == records[wire]["kind"], "unknown nextest suite")
    record = records[wire]
    if event["event"] == "started":
        require(wire not in started, "duplicate nextest suite start")
        require(count(event["test_count"]) == len(record["selected"]),
                "nextest selection count mismatch")
        started.add(wire)
        return
    require(event["event"] in ("ok", "failed") and wire in started and wire not in finished,
            "unexpected nextest suite result")
    require(count(event["ignored"]) == count(event["measured"]) == 0, "nextest skipped a required test")
    require(count(event["filtered_out"]) == len(record["excluded"]), "nextest exclusion total mismatch")
    require((event["event"] == "ok") == (event["failed"] == 0), "inconsistent nextest suite verdict")
    complete(record, event["passed"], event["failed"])
    finished.add(wire)
