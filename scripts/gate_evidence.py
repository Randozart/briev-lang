# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
"""Retained, bounded subprocess evidence for the existing compiler test runners."""

import datetime
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import uuid

from gate_protocol import GateError


def timestamp():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


class Evidence:
    def __init__(self, root, base, timeout, mode, runner):
        self.root = Path(root)
        self.directory = Path(base) / (time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()) + "-" + uuid.uuid4().hex[:12])
        self.directory.mkdir(parents=True, exist_ok=False)
        self.timeout = timeout
        self.env = dict(os.environ, CARGO_TERM_COLOR="never", NEXTEST_EXPERIMENTAL_LIBTEST_JSON="1")
        self.data = {"schema": "briev-gate/v1", "mode": mode, "runner": runner,
                     "started_at": timestamp(), "outcome": "not_run", "tools": {},
                     "commands": [], "suites": {}, "command_timeout_seconds": timeout,
                     "working_directory": str(self.root),
                     "environment": self.environment()}
        self.save()

    def environment(self):
        names = ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET", "CARGO_BUILD_JOBS",
                 "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_RELEASE_DEBUG", "CARGO_INCREMENTAL",
                 "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUST_TEST_THREADS",
                 "BRIEV_HOME", "BRIEV_STDLIB_PATH", "BRIEV_HIR_EMIT",
                 "XDG_DATA_HOME",
                 "BRIEV_SIM_FAMILY", "BRIEV_SIM_CASES", "BRIEV_SIM_SEED", "BRIEV_SIM_REPLAY",
                 "BRIEV_SIM_DEMO_FAULT", "BRIEV_SIM_TYPED_REFERENCE", "BRIEV_SIM_ARTIFACTS")
        # Record invocation knobs, never registry credentials or unrelated environment.
        return {key: self.env[key] for key in names if key in self.env}

    def save(self):
        records = self.data["suites"].values()
        outcomes = [value for record in records for value in record["tests"].values()]
        self.data["counts"] = {name: outcomes.count(name)
                               for name in ("passed", "failed", "not_run", "unreported")}
        self.data["counts"]["selected"] = len(outcomes)
        self.data["counts"]["excluded"] = sum(len(r["excluded"]) for r in self.data["suites"].values())
        temp = self.directory / "manifest.tmp"
        temp.write_text(json.dumps(self.data, indent=2) + "\n")
        temp.replace(self.directory / "manifest.json")

    def command(self, argv, label):
        index = len(self.data["commands"])
        record = {"argv": list(map(str, argv)), "label": label, "started_at": timestamp(),
                  "outcome": "running", "exit_code": None,
                  "stdout": f"{index:03d}.stdout.log", "stderr": f"{index:03d}.stderr.log"}
        self.data["commands"].append(record)
        self.save()
        start = time.monotonic()
        try:
            record["exit_code"] = self.spawn(record)
            record["outcome"] = "pass" if record["exit_code"] == 0 else "fail"
        except OSError as error:
            record["outcome"] = "infrastructure_error"
            raise GateError(f"{label}: {error}") from error
        except subprocess.TimeoutExpired as error:
            record["outcome"] = "timeout"
            raise GateError(f"{label}: exceeded {self.timeout}s", "timeout") from error
        except KeyboardInterrupt:
            record["outcome"] = "interrupted"
            raise
        finally:
            record["seconds"] = round(time.monotonic() - start, 3)
            self.save()
        return record

    def spawn(self, record):
        with (self.directory / record["stdout"]).open("wb") as stdout, \
                (self.directory / record["stderr"]).open("wb") as stderr:
            child = subprocess.Popen(record["argv"], cwd=self.root, env=self.env,
                                     stdout=stdout, stderr=stderr, start_new_session=True)
            try:
                return child.wait(timeout=self.timeout)
            except BaseException:
                stop_group(child)
                raise

    def text(self, record, stream="stdout"):
        return (self.directory / record[stream]).read_text()

    def checked(self, argv, label):
        record = self.command(argv, label)
        if record["exit_code"] != 0:
            raise GateError(f"{label}: exit {record['exit_code']}; see {record['stderr']}")
        return self.text(record)


def stop_group(child):
    # 2026-11-15: cargo/clang/test grandchildren must not outlive a failed gate.
    # Keep process-group cleanup until the runner supplies equivalent containment.
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait()
