#!/usr/bin/env python3
"""Plans the migrations a deploy applies, as the plan `ops migrate --plan` accepts.

Reads the migration ledger from the output of `ops inspect-schema`, whose report is its last
line, and the migrations of the checkout being deployed, which are the ones built into its image. Prints the plan when migrations are pending
and nothing when none are; fails on a ledger that does not belong to this history. The
migrator re-validates the plan against the migrations in the image under its lock, so a wrong
plan is refused, never applied.

Usage: migration_plan.py --inspection <inspect-schema JSON> --migrations <dir>
                         --request-id <id> --bundle-digest <64 hex digits>
"""

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import NamedTuple

MIGRATION_FILE = re.compile(r"^(\d+)_.+\.sql$")
REVERSIBLE_FILE = re.compile(r"\.(up|down)\.sql$")
BUNDLE_DIGEST = re.compile(r"^[0-9a-f]{64}$")
# SQLx runs a migration outside a transaction when its file starts with this line.
NO_TRANSACTION = b"-- no-transaction"


class PlanError(Exception):
    pass


class Migration(NamedTuple):
    checksum: str
    transactional: bool


def read_migrations(directory):
    """Version to SQLx checksum (SHA-384 of the file) and transaction mode of every migration."""
    migrations = {}
    for path in sorted(Path(directory).iterdir()):
        if REVERSIBLE_FILE.search(path.name):
            raise PlanError(f"{path.name}: reversible migrations are not supported")
        match = MIGRATION_FILE.match(path.name)
        if not match:
            continue
        version = int(match.group(1))
        if version in migrations:
            raise PlanError(f"two migrations have version {version}")
        sql = path.read_bytes()
        migrations[version] = Migration(hashlib.sha384(sql).hexdigest(), not sql.startswith(NO_TRANSACTION))
    if not migrations:
        raise PlanError(f"no migrations in {directory}")
    return migrations


def fingerprint(ledger):
    """The ledger fingerprint the migrator computes: SHA-256 of its compact JSON rows."""
    rows = [{"checksum": row["checksum"], "success": row["success"], "version": row["version"]} for row in ledger]
    return hashlib.sha256(json.dumps(rows, separators=(",", ":")).encode()).hexdigest()


def plan(inspection, migrations, request_id, bundle_digest):
    """The plan for the pending migrations, or None when the ledger already has them all."""
    if not request_id:
        raise PlanError("the request ID is empty")
    if not BUNDLE_DIGEST.match(bundle_digest):
        raise PlanError("the bundle digest must be 64 lowercase hex digits")
    ledger = inspection["ledger"]
    if fingerprint(ledger) != inspection["ledger_fingerprint"]:
        raise PlanError("the inspected ledger does not match its fingerprint")
    applied = set()
    unknown = []
    for row in ledger:
        version = row["version"]
        if not row["success"]:
            raise PlanError(f"migration {version} failed earlier; repair the ledger by hand")
        if version not in migrations:
            unknown.append(version)
        elif migrations[version].checksum != row["checksum"]:
            raise PlanError(f"migration {version} differs from the one the database applied")
        applied.add(version)
    pending = [{"version": version, "checksum": migrations[version].checksum}
               for version in sorted(migrations) if version not in applied]
    if pending and unknown:
        raise PlanError(
            f"the database has migrations this revision lacks ({', '.join(map(str, unknown))}) "
            f"and this revision has pending ones ({', '.join(str(m['version']) for m in pending)})"
        )
    outside_transaction = [m["version"] for m in pending if not migrations[m["version"]].transactional]
    if outside_transaction:
        raise PlanError(
            f"migration {', '.join(map(str, outside_transaction))} runs outside a transaction, which the "
            "migrator refuses; it needs a reviewed manual procedure"
        )
    if not pending:
        return None
    return {
        "request_id": request_id,
        "bundle_digest": bundle_digest,
        "expected_ledger_fingerprint": inspection["ledger_fingerprint"],
        "pending": pending,
    }


def read_inspection(path):
    """The `ops inspect-schema` report: the last line of its output, after any log lines."""
    lines = [line for line in Path(path).read_text(encoding="utf-8").splitlines() if line.strip()]
    if not lines:
        raise PlanError("the schema inspection printed nothing")
    try:
        inspection = json.loads(lines[-1])
    except json.JSONDecodeError as error:
        raise PlanError("the schema inspection's last line is not its JSON report") from error
    if not isinstance(inspection, dict) or not isinstance(inspection.get("ledger"), list) \
            or not isinstance(inspection.get("ledger_fingerprint"), str):
        raise PlanError("the schema inspection has no ledger")
    return inspection


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--inspection", required=True)
    parser.add_argument("--migrations", required=True)
    parser.add_argument("--request-id", required=True)
    parser.add_argument("--bundle-digest", required=True)
    args = parser.parse_args(argv)
    try:
        inspection = read_inspection(args.inspection)
        result = plan(inspection, read_migrations(args.migrations), args.request_id, args.bundle_digest)
    except (KeyError, TypeError) as error:
        print(f"migration plan: malformed ledger row ({error!r})", file=sys.stderr)
        return 1
    except PlanError as error:
        print(f"migration plan: {error}", file=sys.stderr)
        return 1
    if result is None:
        print("migration plan: nothing pending", file=sys.stderr)
    else:
        versions = ", ".join(str(m["version"]) for m in result["pending"])
        print(f"migration plan: pending {versions}", file=sys.stderr)
        json.dump(result, sys.stdout)
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
