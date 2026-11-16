#!/usr/bin/env python3
# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
"""Require every GitHub Actions dependency to succeed and retain its verdict."""

import argparse
import json
import os
from pathlib import Path

from gate_evidence import timestamp
from gate_protocol import GateError, json_object, require


def job_results(text):
    jobs = json.loads(text, object_pairs_hook=json_object)
    require(isinstance(jobs, dict) and jobs, "required job results must be a nonempty object")
    results = {}
    for name, job in jobs.items():
        require(isinstance(job, dict), f"{name}: missing job result")
        result = job.get("result")
        require(result in ("success", "failure", "cancelled", "skipped"),
                f"{name}: unknown or missing job result: {result!r}")
        results[name] = result
    return results


def report(text):
    data = {"schema": "briev-ci/v1", "outcome": "fail", "jobs": {},
            "revision": os.environ.get("GITHUB_SHA"),
            "run_id": os.environ.get("GITHUB_RUN_ID"),
            "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
            "finished_at": timestamp()}
    try:
        data["jobs"] = job_results(text)
        failed = [f"{name} ({result})" for name, result in data["jobs"].items() if result != "success"]
        require(not failed, "required jobs did not succeed: " + ", ".join(failed))
        data["outcome"] = "pass"
    except (GateError, ValueError, TypeError) as error:
        data["error"] = str(error)
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, default=Path("target/ci-evidence"))
    args = parser.parse_args()
    data = report(os.environ.get("NEEDS_JSON", ""))
    text = json.dumps(data, indent=2) + "\n"
    args.artifacts.mkdir(parents=True, exist_ok=True)
    (args.artifacts / "required-jobs.json").write_text(text)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(summary).open("a") as stream:
            stream.write("## Required CI jobs\n\n```json\n" + text + "```\n")
    print(text, end="")
    return 0 if data["outcome"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
