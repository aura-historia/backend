from __future__ import annotations

import copy

import json
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest.mock import patch

import check_baseline


class CommonDecompositionCheckerTests(unittest.TestCase):
    baseline: dict[str, Any]

    def setUp(self) -> None:
        self.baseline = json.loads(check_baseline.BASELINE_PATH.read_text())

    def run_exact_check(
        self,
        *,
        features=None,
        public_modules=None,
        dependencies=None,
        forwarded=None,
    ) -> None:
        with (
            patch.object(check_baseline, "cargo_metadata", return_value={}),
            patch.object(
                check_baseline,
                "actual_common_features",
                return_value=features or self.baseline["common_features"],
            ),
            patch.object(
                check_baseline,
                "actual_public_modules",
                return_value=public_modules or self.baseline["common_public_modules"],
            ),
            patch.object(
                check_baseline,
                "actual_dependencies",
                return_value=dependencies or self.baseline["direct_dependencies"],
            ),
            patch.object(
                check_baseline,
                "actual_forwarded_common_features",
                return_value=forwarded or self.baseline["forwarded_common_features"],
            ),
            patch.object(sys, "argv", ["check_baseline.py"]),
        ):
            check_baseline.main()

    def run_against_base(
        self, baseline: dict[str, Any], previous: dict[str, Any]
    ):
        result = check_baseline.subprocess.CompletedProcess(
            args=["git", "show"],
            returncode=0,
            stdout=json.dumps(previous),
            stderr="",
        )
        return patch.object(check_baseline.subprocess, "run", return_value=result)

    def test_exact_current_state_passes(self) -> None:
        self.run_exact_check()

    def test_removing_dependency_and_shrinking_baseline_passes(self) -> None:
        current = copy.deepcopy(self.baseline)
        del current["direct_dependencies"]["normal"]["billing-stripe"]
        current["common_public_modules"].remove("batch")

        with self.run_against_base(current, self.baseline):
            check_baseline.check_baseline_only_shrinks(current, "base")

    def test_new_direct_common_consumer_fails(self) -> None:
        dependencies = copy.deepcopy(self.baseline["direct_dependencies"])
        dependencies["normal"]["new-canonical-service"] = []

        with self.assertRaises(SystemExit):
            self.run_exact_check(dependencies=dependencies)

    def test_new_consumer_with_matching_allowlist_entry_still_fails_against_base(
        self,
    ) -> None:
        current = copy.deepcopy(self.baseline)
        current["direct_dependencies"]["normal"]["new-canonical-service"] = []

        with self.run_against_base(current, self.baseline):
            with self.assertRaises(SystemExit):
                check_baseline.check_baseline_only_shrinks(current, "base")

    def test_new_common_feature_fails(self) -> None:
        features = [*self.baseline["common_features"], "new-feature"]

        with self.assertRaises(SystemExit):
            self.run_exact_check(features=features)

    def test_new_forwarded_feature_fails(self) -> None:
        forwarded = copy.deepcopy(self.baseline["forwarded_common_features"])
        forwarded["existing-consumer"] = {"new-feature": ["api"]}

        with self.assertRaises(SystemExit):
            self.run_exact_check(forwarded=forwarded)

    def test_new_public_top_level_module_fails(self) -> None:
        modules = [*self.baseline["common_public_modules"], "new_module"]

        with self.assertRaises(SystemExit):
            self.run_exact_check(public_modules=modules)

    def test_new_public_root_reexport_fails(self) -> None:
        modules = [*self.baseline["common_public_modules"], "__public_reexport__"]

        with self.assertRaises(SystemExit):
            self.run_exact_check(public_modules=modules)

    def test_bootstrap_with_pinned_final_baseline_passes(self) -> None:
        with patch.object(
            check_baseline.subprocess,
            "run",
            return_value=check_baseline.subprocess.CompletedProcess(
                args=["git", "show"], returncode=1, stdout="", stderr="missing"
            ),
        ):
            check_baseline.check_baseline_only_shrinks(self.baseline, "base-before-baseline")

    def test_bootstrap_with_modified_baseline_fails(self) -> None:
        modified = copy.deepcopy(self.baseline)
        modified["common_features"].append("new-feature")

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "baseline.json"
            path.write_text(json.dumps(modified))
            with (
                patch.object(check_baseline, "BASELINE_PATH", path),
                patch.object(
                    check_baseline.subprocess,
                    "run",
                    return_value=check_baseline.subprocess.CompletedProcess(
                        args=["git", "show"], returncode=1, stdout="", stderr="missing"
                    ),
                ),
            ):
                with self.assertRaises(SystemExit):
                    check_baseline.check_baseline_only_shrinks(modified, "base-before-baseline")


if __name__ == "__main__":
    unittest.main()
