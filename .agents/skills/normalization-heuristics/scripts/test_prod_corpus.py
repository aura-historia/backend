"""The read-only guard and the transfer parsing, without AWS."""

import unittest

from prod_corpus import (
    EXPORT_SQL, SIZE_SQL, SOURCES_SQL, CorpusError, checked_sql, extract_chunk, parse_export_report,
)


class CheckedSqlTest(unittest.TestCase):
    def test_should_wrap_the_bundled_queries_in_a_read_only_transaction(self):
        for sql in (EXPORT_SQL, SIZE_SQL, SOURCES_SQL):
            wrapped = checked_sql(sql)
            self.assertTrue(wrapped.startswith("BEGIN TRANSACTION READ ONLY;\n"))
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
            "\\! rm -rf /",
            "SELECT 1;\n  \\copy users TO '/tmp/x'",
            "SELECT pg_terminate_backend(1)",
            "DO $$ BEGIN END $$",
            "   ",
        ):
            with self.assertRaises(CorpusError, msg=sql):
                checked_sql(sql)


class TransferParsingTest(unittest.TestCase):
    def test_should_extract_only_the_lines_between_the_markers(self):
        output = ("\nThe Session Manager plugin was installed successfully.\r\n"
                  "Starting session with SessionId: ecs-x\r\nBEGIN_CHUNK\r\nAAAA\r\nBBBB\r\nEND_CHUNK\r\n"
                  "Exiting session with sessionId: ecs-x.\r\n")
        self.assertEqual(["AAAA", "BBBB"], extract_chunk(output))

    def test_should_report_a_truncated_session_as_missing(self):
        self.assertIsNone(extract_chunk("BEGIN_CHUNK\r\nAAAA\r\n"))
        self.assertIsNone(extract_chunk("Cannot perform start session: EOF"))

    def test_should_parse_the_export_report_without_confusing_line_counters(self):
        report = parse_export_report(
            "lines=37741 gz_bytes=23829386 sha256=0ae19032b975e9744c096915c1f28b91\nb64_lines=418060\nREADY")
        self.assertEqual({"lines": 37741, "gz_bytes": 23829386,
                          "sha256": "0ae19032b975e9744c096915c1f28b91", "b64_lines": 418060}, report)


if __name__ == "__main__":
    unittest.main()
