"""The pristine migration case has a separate PostgreSQL environment."""


PRISTINE_TARGET = "database-migration-lambda::bin::database-migration-lambda"
PRISTINE_CASE = "tests::should_bootstrap_roles_and_apply_root_schema_as_migrator"
RUNNER_ENV = ("AURA_TEST_ENV_MANAGED", "AURA_TEST_ENV_CONFIG", "NEXTEST", "NEXTEST_RUN_ID",
              "NEXTEST_TEST_GLOBAL_SLOT", "NEXTEST_EXECUTION_MODE")
PRISTINE_FILTER = (
    "package(=database-migration-lambda) and binary(=database-migration-lambda) and "
    f"test(={PRISTINE_CASE})")


def cargo_args(selection, features):
    return ["--locked", "--all-features", *selection["package_args"],
            *[argument for feature in features for argument in ("--features", feature)]]


def nextest_command(selection, slots, features):
    return ["cargo", "nextest", "run", *cargo_args(selection, features), "--all-targets",
            "--test-threads", str(slots), "--retries", "0", "--no-tests", "fail", "--no-fail-fast",
            "-E", f"not ({PRISTINE_FILTER})"]


def standalone_environment(environment):
    return {key: value for key, value in environment.items() if key not in RUNNER_ENV}
