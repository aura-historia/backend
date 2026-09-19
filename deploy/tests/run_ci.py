#!/usr/bin/env python3
"""Run lightweight deployment checks; one documented helper-image test is deferred."""
import os
import sys
import unittest
from pathlib import Path

EXPECTED_SKIPPED_ID = (
    "test_search_ca_compose.SearchCaComposeTests."
    "test_actual_candidate_ca_bind_is_readable_and_readonly_as_uid10001"
)
EXPECTED_SKIP_REASON = "opt-in cached networkless helper only"


def environment_error(environ):
    if environ.get("AURA_TEST_COMPOSE_CONFIG") != "1":
        return "CI must enable the actual Compose-config check"
    if environ.get("AURA_TEST_CA_MOUNT") == "1":
        return "The C1 lightweight job must not start the cached-helper test"
    return None


def result_is_accepted(result, discovered):
    skipped = [(test.id(), reason) for test, reason in result.skipped]
    expected = [(EXPECTED_SKIPPED_ID, EXPECTED_SKIP_REASON)]
    return (
        skipped == expected
        and result.testsRun == discovered
        and result.wasSuccessful()
        and not result.unexpectedSuccesses
    )


def main() -> int:
    error = environment_error(os.environ)
    if error is not None:
        print(error, file=sys.stderr)
        return 2

    directory = Path(__file__).resolve().parent
    # Existing test modules import siblings using their current top-level names.
    sys.path.insert(0, str(directory))
    loader = unittest.TestLoader()
    suite = loader.discover(str(directory), pattern="test_*.py")
    discovered = suite.countTestCases()
    if discovered == 0 or loader.errors:
        print("Deployment test discovery failed or selected zero tests", file=sys.stderr)
        return 1

    result = unittest.TextTestRunner(verbosity=2).run(suite)
    skipped = [(test.id(), reason) for test, reason in result.skipped]
    if skipped != [(EXPECTED_SKIPPED_ID, EXPECTED_SKIP_REASON)]:
        print("Unexpected deployment-test skip set", file=sys.stderr)
    if result.testsRun != discovered:
        print("Deployment test execution count changed during the run", file=sys.stderr)
    if result.unexpectedSuccesses:
        print("Unexpected deployment-test successes", file=sys.stderr)
    if not result.wasSuccessful():
        print("Deployment tests reported failures or errors", file=sys.stderr)
    if not result_is_accepted(result, discovered):
        return 1

    print(
        "C1 lightweight checks passed; "
        "one explicitly deferred cached-helper mount test was not executed."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
