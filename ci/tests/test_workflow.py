import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest
from unittest.mock import call, patch


SPEC = importlib.util.spec_from_file_location("collect_ci_run", pathlib.Path(__file__).resolve().parents[1] / "collect-ci-run.py")
COLLECTOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(COLLECTOR)


class WorkflowTiming(unittest.TestCase):
    def test_rerun_duration_uses_attempt_creation_and_the_same_attempt_jobs(self):
        attempt = {"status": "completed", "run_attempt": 2, "head_sha": "a" * 40,
                   "html_url": "https://github.com/example/repo/actions/runs/42",
                   "created_at": "2026-10-21T12:00:00Z", "run_started_at": "2026-10-21T12:00:05Z",
                   "updated_at": "2026-10-21T12:05:00Z", "conclusion": "success"}
        run = dict(attempt, created_at="2026-10-21T09:00:00Z")
        job = {"id": 7, "name": "baseline", "conclusion": "success", "runner_name": "runner",
               "labels": ["ubuntu-latest"], "created_at": "2026-10-21T12:00:05Z",
               "started_at": "2026-10-21T12:00:10Z", "completed_at": "2026-10-21T12:04:00Z", "steps": []}
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "workflow.json"
            with patch.object(sys, "argv", ["collect-ci-run.py", "42", "--repository", "example/repo", "--output", str(output)]), patch.object(
                    COLLECTOR, "api", side_effect=[run, attempt, {"jobs": [job]}]) as api:
                COLLECTOR.main()
            actual = json.loads(output.read_text())
        self.assertEqual(actual["workflow_seconds"], 300)
        self.assertEqual(actual["attempt"], 2)
        self.assertEqual(actual["jobs"][0]["scheduling_delay_seconds"], 5)
        self.assertEqual(api.call_args_list, [call("repos/example/repo/actions/runs/42"),
                         call("repos/example/repo/actions/runs/42/attempts/2"),
                         call("repos/example/repo/actions/runs/42/attempts/2/jobs?per_page=100&page=1")])


if __name__ == "__main__":
    unittest.main()
