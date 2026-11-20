"""The read-only guard and the psql invocation, without a database."""

import unittest

from pathlib import Path

from prod_corpus import (
    EXPORT_SQL, SIZE_SQL, SOURCES_SQL, CorpusError, argument_parser, checked_sql, psql_command,
)


class CheckedSqlTest(unittest.TestCase):
    def test_should_wrap_the_bundled_queries_in_a_read_only_transaction_as_the_runtime_role(self):
        for sql in (EXPORT_SQL, SIZE_SQL, SOURCES_SQL):
            wrapped = checked_sql(sql)
            self.assertTrue(wrapped.startswith("BEGIN TRANSACTION READ ONLY;\nSET LOCAL ROLE aura_runtime;\n"))
            self.assertTrue(wrapped.endswith("COMMIT;\n"))

    def test_should_allow_forbidden_words_inside_literals_identifiers_and_comments(self):
        checked_sql("SELECT 'update', count(*) AS updated, created FROM t -- delete later\nWHERE x = 'do'")
        checked_sql("SELECT CASE WHEN a THEN 1 ELSE 0 END FROM product_listings")

    def test_should_refuse_writes_transaction_control_and_meta_commands(self):
        for sql in (
            "SELECT 1; COMMIT; DELETE FROM users",
            "UPDATE users SET language = 'de'",
            "SET TRANSACTION READ WRITE",
            "set role aura_migrator",
            "RESET ROLE",
            "\\! rm -rf /",
            "SELECT 1;\n  \\copy users TO '/tmp/x'",
            "SELECT pg_terminate_backend(1)",
            "DO $$ BEGIN END $$",
            "   ",
        ):
            with self.assertRaises(CorpusError, msg=sql):
                checked_sql(sql)


class PsqlCommandTest(unittest.TestCase):
    def test_should_read_the_query_from_stdin_and_stop_at_the_first_error(self):
        command = psql_command(headers=False)
        self.assertEqual(["docker", "exec", "-i"], command[:3])
        self.assertEqual(["-f", "-"], command[-2:])
        self.assertIn("ON_ERROR_STOP=1", command)
        self.assertIn("-t", command)
        self.assertNotIn("-t", psql_command(headers=True))


class ArgumentsTest(unittest.TestCase):
    def test_should_accept_the_work_directory_before_or_after_the_sql_file(self):
        for argv in (["query", "--work", "/w", "q.sql"], ["query", "q.sql", "--work", "/w"],
                     ["--work", "/w", "query", "q.sql"]):
            args = argument_parser().parse_intermixed_args(argv)
            self.assertEqual(("query", "q.sql", Path("/w")), (args.command, args.sql_file, args.work), argv)


if __name__ == "__main__":
    unittest.main()
