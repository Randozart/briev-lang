# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
"""CI cannot hide failed/skipped jobs or restore an incomplete/stale compiler."""

import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
from ci_summary import report


class SummaryControls(unittest.TestCase):
    def test_all_required_jobs_must_succeed(self):
        jobs = {"library": {"result": "success"}, "browser": {"result": "success"}}
        self.assertEqual(report(json.dumps(jobs))["outcome"], "pass")
        for result in ("failure", "cancelled", "skipped"):
            jobs["library"]["result"] = result
            data = report(json.dumps(jobs))
            self.assertEqual(data["outcome"], "fail")
            self.assertEqual(data["jobs"]["browser"], "success")
            self.assertEqual(data["jobs"]["library"], result)

    def test_empty_missing_malformed_and_duplicate_results_fail(self):
        for text in ("", "{}", "[]", "null", "not JSON", '{"job": {}}',
                     '{"job": {"result": "unknown"}}', '{"job": "success"}',
                     '{"job":{"result":"failure","result":"success"}}',
                     '{"job":{"result":"failure"},"job":{"result":"success"}}'):
            with self.subTest(text=text):
                self.assertEqual(report(text)["outcome"], "fail")

    def test_cli_retains_failure_and_success_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for result in ("success", "failure", "skipped"):
                out = root / result
                env = dict(os.environ, NEEDS_JSON=json.dumps({"required": {"result": result}}),
                           GITHUB_STEP_SUMMARY=str(root / "summary.md"), GITHUB_SHA="fixture-revision")
                child = subprocess.run([sys.executable, "-B", str(SCRIPTS / "ci_summary.py"),
                                        "--artifacts", str(out)], env=env, capture_output=True, timeout=10)
                data = json.loads((out / "required-jobs.json").read_text())
                self.assertEqual(child.returncode == 0, result == "success")
                self.assertEqual(data["jobs"], {"required": result})
                self.assertEqual(data["revision"], "fixture-revision")
            self.assertIn('"skipped"', (root / "summary.md").read_text())


class CompilerArtifactControls(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "scripts").mkdir()
        shutil.copy2(SCRIPTS / "ci_compiler_artifact.sh", self.root / "scripts/ci_compiler_artifact.sh")
        self.git("init", "--quiet")
        self.git("commit", "--quiet", "--allow-empty", "-m", "fixture")
        self.files = ("target/release/brievc", "target/compiler-in-briv/libbriev_accel_rt.a",
                      "target/compiler-in-briv/libbriev_gpu_rt.a")
        for name in self.files:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"fixture payload: " + name.encode())
        (self.root / self.files[0]).chmod(0o755)
        self.archive = self.root / "target/ci-compiler.tar.gz"

    def git(self, *args):
        subprocess.run(["git", "-c", "user.name=CI control", "-c", "user.email=ci@example.invalid",
                        "-c", "commit.gpgsign=false", *args], cwd=self.root, check=True,
                       capture_output=True, timeout=10)

    def artifact(self, operation):
        return subprocess.run(["bash", "scripts/ci_compiler_artifact.sh", operation], cwd=self.root,
                              text=True, capture_output=True, timeout=10)

    def pack(self):
        result = self.artifact("pack")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_roundtrip_preserves_executable_and_both_runtime_archives(self):
        expected = {name: (self.root / name).read_bytes() for name in self.files}
        self.pack()
        bundle = self.archive.read_bytes()
        shutil.rmtree(self.root / "target")
        self.archive.parent.mkdir()
        self.archive.write_bytes(bundle)
        result = self.artifact("unpack")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(os.access(self.root / self.files[0], os.X_OK))
        for name, payload in expected.items():
            self.assertEqual((self.root / name).read_bytes(), payload)

    def test_missing_runtime_archive_cannot_be_packaged(self):
        (self.root / self.files[1]).unlink()
        result = self.artifact("pack")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing", result.stderr)

    def test_nonexecutable_compiler_cannot_be_packaged(self):
        (self.root / self.files[0]).chmod(0o644)
        result = self.artifact("pack")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not executable", result.stderr)

    def test_wrong_revision_cannot_be_restored(self):
        self.pack()
        self.git("commit", "--quiet", "--allow-empty", "-m", "different revision")
        result = self.artifact("unpack")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("revision differs", result.stderr)

    def test_corrupted_compiler_cannot_be_restored(self):
        self.pack()
        self.replace_member(self.files[0], b"corruption")
        result = self.artifact("unpack")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum mismatch", result.stderr)

    def test_missing_checkout_revision_cannot_match_empty_artifact_revision(self):
        self.pack()
        self.replace_member("target/ci-compiler/revision", b"")
        shutil.rmtree(self.root / ".git")
        result = self.artifact("unpack")
        self.assertNotEqual(result.returncode, 0)

    def replace_member(self, name, replacement):
        output = io.BytesIO()
        with tarfile.open(self.archive, "r:gz") as source, tarfile.open(fileobj=output, mode="w:gz") as target:
            for member in source.getmembers():
                payload = replacement if member.name == name else source.extractfile(member).read()
                member.size = len(payload)
                target.addfile(member, io.BytesIO(payload))
        self.archive.write_bytes(output.getvalue())


if __name__ == "__main__":
    unittest.main(verbosity=2)
