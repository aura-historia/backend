import json
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import unittest

WORKFLOW = pathlib.Path(__file__).resolve().parents[2] / ".github/workflows/benchmark-lanes.yml"
SCRIPT = textwrap.dedent(WORKFLOW.read_text().split("python3 - <<'PYPLAN'\n", 1)[1].split("\n          PYPLAN", 1)[0])


class OptInLanePlan(unittest.TestCase):
    def plan(self, **values):
        environment = dict(os.environ, EVENT="workflow_dispatch", LABEL="", HEAD_SHA="", REVISION="a" * 40,
            LAYOUT="four", SCENARIO="FULL", REPETITION="1", HARNESS="nextest", SLOTS="4",
            GITHUB_RUN_NUMBER="17", GITHUB_WORKFLOW_SHA="b" * 40)
        environment.update(values)
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "output"
            result = subprocess.run([sys.executable, "-c", SCRIPT], env=dict(environment, GITHUB_OUTPUT=str(output)),
                                    text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            return result, dict(line.split("=", 1) for line in output.read_text().splitlines()) if output.exists() else {}

    def test_every_layout_covers_each_lane_once(self):
        for layout, count in (("four", 4), ("three", 3), ("one", 1)):
            with self.subTest(layout=layout):
                result, outputs = self.plan(LAYOUT=layout)
                self.assertEqual(result.returncode, 0, result.stderr)
                rows = json.loads(outputs["matrix"])
                self.assertEqual(len(rows), count)
                self.assertEqual(sorted(lane for row in rows for lane in row["lanes"].split()),
                                 ["API", "CRAWLER", "INFRASTRUCTURE", "REMAINING"])
                self.assertEqual(outputs["revision"], "a" * 40)

    def test_label_bootstrap_uses_actual_head_and_explicit_screen_defaults(self):
        for layout in ("four", "three", "one"):
            with self.subTest(layout=layout):
                result, outputs = self.plan(EVENT="pull_request", LABEL="ci/lanes-" + layout,
                    HEAD_SHA="c" * 40, REVISION="", LAYOUT="", SCENARIO="", REPETITION="", HARNESS="", SLOTS="")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual({name: outputs[name] for name in ("revision", "layout", "harness", "slots", "scenario", "repetition")},
                    dict(revision="c" * 40, layout=layout, harness="nextest", slots="4", scenario="FULL", repetition="17"))

    def test_invalid_source_or_configuration_fails_before_emitting_a_plan(self):
        for key, value in (("REVISION", "develop"), ("LAYOUT", "unknown"), ("SCENARIO", "unknown"),
                           ("REPETITION", "0"), ("HARNESS", "unknown"), ("SLOTS", "1024"), ("GITHUB_WORKFLOW_SHA", "unknown")):
            with self.subTest(key=key):
                result, outputs = self.plan(**{key: value})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(outputs, {})


if __name__ == "__main__":
    unittest.main()
