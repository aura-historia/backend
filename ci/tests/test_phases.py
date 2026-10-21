import pathlib
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from ci_inventory.phases import (CONTEXT_CASE, CONTEXT_TARGET, ORDINARY_FILTER, PRISTINE_CASE,
                                 PRISTINE_TARGET, UNMANAGED_TARGETS, combined_selection,
                                 libtest_commands, nextest_command, unmanaged_commands,
                                 unmanaged_environment, validate_special_cases)


def suite(names):
    return {"binary-path": "/fixture/test-binary", "testcases": {name: {"ignored": False} for name in names}}


class LanePhaseCommands(unittest.TestCase):
    def setUp(self):
        self.selection = {"packages": ["database-migration-lambda", "test-api"],
            "package_args": ["--package", "database-migration-lambda", "--package", "test-api"]}
        self.suites = {CONTEXT_TARGET: suite([CONTEXT_CASE, "ordinary_context_case"]),
                       PRISTINE_TARGET: suite([PRISTINE_CASE, "ordinary_migration_case"]),
                       **{identity: suite(["control_case"]) for identity in UNMANAGED_TARGETS}}

    def test_mandatory_exceptional_cases_cannot_silently_match_zero_or_become_ignored(self):
        validate_special_cases(self.selection, self.suites)
        for identity, case in ((CONTEXT_TARGET, CONTEXT_CASE), (PRISTINE_TARGET, PRISTINE_CASE)):
            for mutation in ("absent_target", "absent_case", "ignored_case"):
                with self.subTest(identity=identity, mutation=mutation):
                    changed = {key: dict(value, testcases=dict(value["testcases"])) for key, value in self.suites.items()}
                    if mutation == "absent_target":
                        del changed[identity]
                    elif mutation == "absent_case":
                        del changed[identity]["testcases"][case]
                    else:
                        changed[identity]["testcases"][case] = {"ignored": True}
                    with self.assertRaisesRegex(ValueError, "Required exceptional"):
                        validate_special_cases(self.selection, changed)

    def test_whole_unmanaged_targets_are_required_and_never_run_as_ordinary_libtest(self):
        for identity in UNMANAGED_TARGETS:
            with self.subTest(identity=identity):
                self.assertEqual(libtest_commands(identity, self.suites[identity], 2), [])
                changed = dict(self.suites, **{identity: suite([])})
                with self.assertRaisesRegex(ValueError, "absent or empty"):
                    validate_special_cases(self.selection, changed)
                changed[identity] = {"testcases": {"control_case": {"ignored": True}}}
                with self.assertRaisesRegex(ValueError, "no active cases"):
                    validate_special_cases(self.selection, changed)

    def test_exact_pristine_and_context_controls_are_partitioned_out_of_normal_cases(self):
        for identity, excluded, ordinary in ((CONTEXT_TARGET, CONTEXT_CASE, "ordinary_context_case"),
                                              (PRISTINE_TARGET, PRISTINE_CASE, "ordinary_migration_case")):
            with self.subTest(identity=identity):
                command = libtest_commands(identity, self.suites[identity], 2)[0]
                self.assertEqual(command, ["/fixture/test-binary", ordinary, "--exact", "--test-threads=2"])
                self.assertNotIn(excluded, command)

    def test_legacy_teardown_contracts_remain_ordinary_managed_cases(self):
        for identity, name in (
                ("test-api::test::postgres_business_schema", "should_clear_business_rows_on_plain_postgres"),
                ("test-api::test::sqs", "should_purge_only_its_source_and_dlq_including_invisible_messages")):
            with self.subTest(identity=identity):
                self.assertIn(name, libtest_commands(identity, suite([name]), 4)[0])

    def test_crawler_library_partitions_exact_prefix_names_without_skipping_llm_runtime(self):
        names = ["runtime::tests::case", "logging::tests::case", "llm_runtime::tests::case", "ordinary"]
        commands = libtest_commands("crawler::lib::crawler", suite(names), 4)
        self.assertEqual(commands, [
            ["/fixture/test-binary", "llm_runtime::tests::case", "ordinary", "--exact", "--test-threads=4"],
            ["/fixture/test-binary", "logging::tests::case", "runtime::tests::case", "--exact", "--test-threads=1"]])
        self.assertCountEqual([name for command in commands for name in command[1:-2]], names)

    def test_new_crawler_target_cases_are_retained_without_a_target_count_assumption(self):
        names = ["should_read_reviewer_page", "should_preserve_repository_case"]
        command = libtest_commands("crawler::test::crawler_review_reviewer_pages", suite(names), 2)[0]
        self.assertEqual(command[1:-2], sorted(names))

    def test_zero_case_harness_still_executes_without_selecting_other_cases(self):
        self.assertEqual(libtest_commands("root::lib::root", suite([]), 1),
                         [["/fixture/test-binary", "--test-threads=1"]])

    def test_nextest_command_keeps_locked_features_owner_arguments_and_explicit_exceptions(self):
        command = nextest_command(self.selection, 2, ["test-api/s3"])
        self.assertEqual(command, ["cargo", "nextest", "run", "--locked", "--all-features",
            "--package", "database-migration-lambda", "--package", "test-api", "--features", "test-api/s3",
            "--all-targets", "--test-threads", "2", "--retries", "0", "--no-tests", "fail", "--no-fail-fast", "-E", ORDINARY_FILTER])
        self.assertIn(f"binary(=isolated_context) and test(={CONTEXT_CASE})", ORDINARY_FILTER)
        self.assertIn(f"binary(=database-migration-lambda) and test(={PRISTINE_CASE})", ORDINARY_FILTER)

    def test_control_commands_run_explicitly_and_preserve_exact_context_selection(self):
        commands = unmanaged_commands(self.selection, [])
        self.assertEqual(commands[0], ["cargo", "test", "--locked", "--all-features", "--package", "test-api",
                                       "--test", "aws", "--test", "managed_environment", "--", "--test-threads=1"])
        self.assertEqual(commands[1][-6:], ["--test", "isolated_context", CONTEXT_CASE,
                                          "--", "--exact", "--test-threads=1"])
        self.assertEqual(unmanaged_commands({"packages": ["crawler"]}, []), [])

    def test_unmanaged_environment_retains_coverage_and_fixed_credentials(self):
        preserved = {"LLVM_PROFILE_FILE": "/profiles/%p-%m.profraw", "RUSTFLAGS": "-C instrument-coverage",
                     "RUSTDOCFLAGS": "coverage", "AWS_ACCESS_KEY_ID": "test", "PATH": "/tools"}
        environment = dict(preserved, AURA_TEST_ENV_MANAGED="1", AURA_TEST_ENV_CONFIG="/managed",
                           NEXTEST="1", NEXTEST_RUN_ID="invocation", NEXTEST_TEST_GLOBAL_SLOT="2",
                           NEXTEST_EXECUTION_MODE="process-per-test")
        self.assertEqual(unmanaged_environment(environment), preserved)

    def test_combining_lanes_is_deterministic_and_rejects_repeated_ownership(self):
        values = {
            "INFRASTRUCTURE": {"packages": ["z"], "targets": ["z::lib::z"], "doctest_targets": ["z::lib::z"]},
            "REMAINING": {"packages": ["a"], "targets": ["a::lib::a"], "doctest_targets": []}}
        with patch("ci_inventory.phases.lane_selection", side_effect=lambda manifest, expected, lane: values[lane]):
            selected = combined_selection({}, {}, ["REMAINING", "INFRASTRUCTURE"])
            self.assertEqual(selected["package_args"], ["--package", "a", "--package", "z"])
            self.assertEqual(selected["targets"], ["a::lib::a", "z::lib::z"])
            values["REMAINING"] = values["INFRASTRUCTURE"]
            with self.assertRaisesRegex(ValueError, "duplicated ownership"):
                combined_selection({}, {}, ["REMAINING", "INFRASTRUCTURE"])
        for lanes in ([], ["REMAINING", "REMAINING"]):
            with self.subTest(lanes=lanes), self.assertRaisesRegex(ValueError, "distinct, nonempty"):
                combined_selection({}, {}, lanes)


if __name__ == "__main__":
    unittest.main()
