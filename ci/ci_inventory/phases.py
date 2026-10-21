"""Explicit exceptional selections and stock harness commands for test lanes."""

import pathlib

from .catalog import lane_selection


PRISTINE_TARGET = "database-migration-lambda::bin::database-migration-lambda"
PRISTINE_CASE = "tests::should_bootstrap_roles_and_apply_root_schema_as_migrator"
CONTEXT_TARGET = "test-api::test::isolated_context"
CONTEXT_CASE = "should_verify_managed_context_isolation_and_recovery"
UNMANAGED_TARGETS = {"test-api::test::aws", "test-api::test::managed_environment"}
RUNNER_ENV = ("AURA_TEST_ENV_MANAGED", "AURA_TEST_ENV_CONFIG", "NEXTEST", "NEXTEST_RUN_ID",
              "NEXTEST_TEST_GLOBAL_SLOT", "NEXTEST_EXECUTION_MODE")
ORDINARY_FILTER = (
    "not ((package(=test-api) and (binary(=aws) or binary(=managed_environment) or "
    "(binary(=isolated_context) and test(=should_verify_managed_context_isolation_and_recovery)))) or "
    "(package(=database-migration-lambda) and binary(=database-migration-lambda) and "
    "test(=tests::should_bootstrap_roles_and_apply_root_schema_as_migrator)))"
)


def combined_selection(manifest, expected, lanes):
    if not lanes or len(lanes) != len(set(lanes)):
        raise ValueError("Select distinct, nonempty test lanes")
    selections = [lane_selection(manifest, expected, lane) for lane in sorted(lanes)]
    packages = sorted(name for selection in selections for name in selection["packages"])
    targets = sorted(identity for selection in selections for identity in selection["targets"])
    if len(packages) != len(set(packages)) or len(targets) != len(set(targets)):
        raise ValueError("Combined lanes have duplicated ownership")
    return {"lanes": sorted(lanes), "packages": packages, "targets": targets,
            "package_args": [argument for name in packages for argument in ("--package", name)],
            "doctest_targets": sorted(identity for selection in selections for identity in selection["doctest_targets"])}


def suite_identity(suite):
    return f"{suite['package-name']}::{suite['kind']}::{suite['binary-name']}"


def validate_special_cases(selection, suites):
    required = {}
    if "test-api" in selection["packages"]:
        required.update({CONTEXT_TARGET: CONTEXT_CASE, **{identity: None for identity in UNMANAGED_TARGETS}})
    if "database-migration-lambda" in selection["packages"]:
        required[PRISTINE_TARGET] = PRISTINE_CASE
    for identity, case in required.items():
        suite = suites.get(identity)
        if suite is None or not suite["testcases"]:
            raise ValueError(f"Required exceptional target is absent or empty: {identity}")
        if case is not None and (case not in suite["testcases"] or suite["testcases"][case]["ignored"]):
            raise ValueError(f"Required exceptional case is absent or ignored: {identity}::{case}")
        if case is None and all(case["ignored"] for case in suite["testcases"].values()):
            raise ValueError(f"Required exceptional target has no active cases: {identity}")


def cargo_args(selection, features):
    return ["--locked", "--all-features", *selection["package_args"],
            *[argument for feature in features for argument in ("--features", feature)]]


def nextest_command(selection, slots, features):
    return ["cargo", "nextest", "run", *cargo_args(selection, features), "--all-targets",
            "--test-threads", str(slots), "--retries", "0", "--no-tests", "fail", "--no-fail-fast", "-E", ORDINARY_FILTER]


def libtest_commands(identity, suite, slots):
    if identity in UNMANAGED_TARGETS:
        return []
    names = sorted(suite["testcases"])
    excluded = {PRISTINE_TARGET: PRISTINE_CASE, CONTEXT_TARGET: CONTEXT_CASE}.get(identity)
    selected = [name for name in names if name != excluded]
    environment = [name for name in selected if identity == "crawler::lib::crawler"
                   and name.startswith(("runtime::", "logging::"))]
    ordinary = [name for name in selected if name not in environment]
    binary = suite["binary-path"]
    commands = []
    if ordinary:
        commands.append([binary, *ordinary, "--exact", f"--test-threads={slots}"])
    elif not names:
        commands.append([binary, f"--test-threads={slots}"])
    if environment:
        commands.append([binary, *environment, "--exact", "--test-threads=1"])
    return commands


def unmanaged_commands(selection, features):
    if "test-api" not in selection["packages"]:
        return []
    selected = dict(selection, package_args=["--package", "test-api"])
    cargo = ["cargo", "test", *cargo_args(selected, features)]
    return [[*cargo, "--test", "aws", "--test", "managed_environment", "--", "--test-threads=1"],
            [*cargo, "--test", "isolated_context", CONTEXT_CASE, "--", "--exact", "--test-threads=1"]]


def unmanaged_environment(environment):
    return {key: value for key, value in environment.items() if key not in RUNNER_ENV}


def coverage_environment(environment, phase):
    result = dict(environment)
    if directory := result.get("AURA_CI_COVERAGE_DIR"):
        profiles = pathlib.Path(directory) / "profiles" / phase
        profiles.mkdir(parents=True, exist_ok=True)
        result["LLVM_PROFILE_FILE"] = str(profiles / "%b-%4m.profraw")
    return result
