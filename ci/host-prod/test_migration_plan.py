"""Unit tests of host/prod/migration_plan.py: python3 -m unittest discover -s ci/host-prod"""

import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("migration_plan", ROOT / "host/prod/migration_plan.py")
migration_plan = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(migration_plan)

DIGEST = "ab" * 32


def checksum(sql):
    return hashlib.sha384(sql.encode()).hexdigest()


def inspection(*rows):
    ledger = [{"checksum": c, "success": s, "version": v} for v, c, s in rows]
    return {"ledger": ledger, "ledger_fingerprint": migration_plan.fingerprint(ledger)}


def transactional(**sql_by_version):
    return {int(v[1:]): migration_plan.Migration(checksum(sql), True) for v, sql in sql_by_version.items()}


class ReadMigrations(unittest.TestCase):
    def directory(self, files):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        for name, sql in files.items():
            (Path(temporary.name) / name).write_text(sql)
        return temporary.name

    def test_should_hash_each_migration_file_like_sqlx(self):
        directory = self.directory({"1_a.sql": "SELECT 1;\n", "2_b.sql": "-- no-transaction\nVACUUM;\n", "README.md": "x"})
        self.assertEqual(
            migration_plan.read_migrations(directory),
            {
                1: migration_plan.Migration(checksum("SELECT 1;\n"), True),
                2: migration_plan.Migration(checksum("-- no-transaction\nVACUUM;\n"), False),
            },
        )

    def test_should_refuse_reversible_migrations(self):
        directory = self.directory({"1_a.up.sql": "", "1_a.down.sql": ""})
        with self.assertRaises(migration_plan.PlanError):
            migration_plan.read_migrations(directory)

    def test_should_refuse_two_migrations_with_one_version(self):
        directory = self.directory({"1_a.sql": "", "1_b.sql": ""})
        with self.assertRaises(migration_plan.PlanError):
            migration_plan.read_migrations(directory)


class Plan(unittest.TestCase):
    migrations = transactional(v1="one", v2="two", v3="three")

    def test_should_plan_the_migrations_the_ledger_lacks(self):
        current = inspection((1, checksum("one"), True))
        self.assertEqual(
            migration_plan.plan(current, self.migrations, "deploy-1", DIGEST),
            {
                "request_id": "deploy-1",
                "bundle_digest": DIGEST,
                "expected_ledger_fingerprint": current["ledger_fingerprint"],
                "pending": [
                    {"version": 2, "checksum": checksum("two")},
                    {"version": 3, "checksum": checksum("three")},
                ],
            },
        )

    def test_should_plan_nothing_when_every_migration_is_applied(self):
        current = inspection(*[(v, m.checksum, True) for v, m in self.migrations.items()])
        self.assertIsNone(migration_plan.plan(current, self.migrations, "deploy-1", DIGEST))

    def test_should_plan_nothing_when_rolling_back_past_applied_migrations(self):
        current = inspection(*[(v, m.checksum, True) for v, m in self.migrations.items()], (4, checksum("four"), True))
        self.assertIsNone(migration_plan.plan(current, self.migrations, "deploy-1", DIGEST))

    def test_should_refuse_pending_migrations_next_to_unknown_applied_ones(self):
        current = inspection((1, checksum("one"), True), (4, checksum("four"), True))
        with self.assertRaisesRegex(migration_plan.PlanError, "lacks"):
            migration_plan.plan(current, self.migrations, "deploy-1", DIGEST)

    def test_should_refuse_a_migration_that_differs_from_the_applied_one(self):
        current = inspection((1, checksum("edited"), True))
        with self.assertRaisesRegex(migration_plan.PlanError, "differs"):
            migration_plan.plan(current, self.migrations, "deploy-1", DIGEST)

    def test_should_refuse_a_failed_ledger_row(self):
        current = inspection((1, checksum("one"), False))
        with self.assertRaisesRegex(migration_plan.PlanError, "failed"):
            migration_plan.plan(current, self.migrations, "deploy-1", DIGEST)

    def test_should_refuse_a_ledger_that_does_not_match_its_fingerprint(self):
        current = inspection((1, checksum("one"), True))
        current["ledger_fingerprint"] = "0" * 64
        with self.assertRaisesRegex(migration_plan.PlanError, "fingerprint"):
            migration_plan.plan(current, self.migrations, "deploy-1", DIGEST)

    def test_should_refuse_a_pending_migration_outside_a_transaction(self):
        migrations = {**self.migrations, 4: migration_plan.Migration(checksum("four"), False)}
        current = inspection(*[(v, m.checksum, True) for v, m in self.migrations.items()])
        with self.assertRaisesRegex(migration_plan.PlanError, "outside a transaction"):
            migration_plan.plan(current, migrations, "deploy-1", DIGEST)

    def test_should_refuse_a_malformed_bundle_digest(self):
        with self.assertRaises(migration_plan.PlanError):
            migration_plan.plan(inspection(), self.migrations, "deploy-1", "sha256:" + DIGEST)


class Fingerprint(unittest.TestCase):
    def test_should_hash_the_compact_json_rows_in_field_order(self):
        ledger = [{"version": 7, "success": True, "checksum": "ff"}]
        expected = hashlib.sha256(b'[{"checksum":"ff","success":true,"version":7}]').hexdigest()
        self.assertEqual(migration_plan.fingerprint(ledger), expected)


class ReadInspection(unittest.TestCase):
    def test_should_read_the_report_after_log_lines(self):
        report = inspection((1, checksum("one"), True))
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as file:
            file.write('{"level":"INFO","message":"connected"}\n' + json.dumps(report) + "\n")
        self.addCleanup(Path(file.name).unlink)
        self.assertEqual(migration_plan.read_inspection(file.name), report)


if __name__ == "__main__":
    unittest.main()
