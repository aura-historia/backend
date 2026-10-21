import json
import os
import pathlib
import subprocess
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "collect-coverage.sh"


class ExistingCoverageCommand(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        self.profiles = self.root / "profile directory"
        self.profiles.mkdir()
        self.binaries = self.root / "binary directory"
        self.report = self.root / "reports" / "lane.lcov"
        self.record = self.root / "arguments.json"
        executable = self.root / "grcov"
        executable.write_text('''#!/usr/bin/env python3
import json,os,pathlib,sys
pathlib.Path(os.environ['FIXTURE_ARGUMENTS']).write_text(json.dumps(sys.argv[1:]))
if os.environ.get('FIXTURE_EXIT'): raise SystemExit(int(os.environ['FIXTURE_EXIT']))
if not os.environ.get('FIXTURE_EMPTY'):
    pathlib.Path(sys.argv[sys.argv.index('-o')+1]).write_text('SF:src/example.rs\\nend_of_record\\n')
''')
        executable.chmod(0o755)
        self.environment = dict(os.environ, PATH=str(self.root) + os.pathsep + os.environ["PATH"],
                                FIXTURE_ARGUMENTS=str(self.record))

    def collect(self):
        return subprocess.run(["bash", str(SCRIPT), str(self.profiles), str(self.binaries), str(self.report)],
                              cwd=self.root, env=self.environment, text=True, capture_output=True)

    def test_export_keeps_existing_grcov_policy_and_handles_paths_with_spaces(self):
        (self.profiles / "case.profraw").write_bytes(b"profile")
        result = self.collect()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.record.read_text()), [str(self.profiles), "--binary-path", str(self.binaries),
            "-s", ".", "-t", "lcov", "--branch", "--ignore-not-existing", "--ignore", "/*",
            "--ignore", "*/target/*", "--ignore", "*/.cargo/*", "-o", str(self.report)])
        self.assertTrue(self.report.stat().st_size)

    def test_missing_profiles_fail_before_export(self):
        result = self.collect()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("No LLVM coverage profiles", result.stderr)
        self.assertFalse(self.record.exists())

    def test_exporter_failure_keeps_its_status(self):
        (self.profiles / "case.profraw").write_bytes(b"profile")
        self.environment["FIXTURE_EXIT"] = "37"
        self.assertEqual(self.collect().returncode, 37)

    def test_empty_export_is_rejected(self):
        (self.profiles / "case.profraw").write_bytes(b"profile")
        self.environment["FIXTURE_EMPTY"] = "1"
        self.assertNotEqual(self.collect().returncode, 0)


if __name__ == "__main__":
    unittest.main()
