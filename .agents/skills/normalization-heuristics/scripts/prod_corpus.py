#!/usr/bin/env python3
"""Read-only access to the production normalization diagnostic corpus.

Runs on the prod host, by a user in the `docker` group. Each query runs through `psql` inside the
prod PostgreSQL container, in a read-only transaction as the runtime role, and its output comes
straight back over the `docker exec` pipe. Nothing is logged, staged or uploaded; the corpus lands
only in a private local file. No credential is read: the container's local socket authenticates.

    prod_corpus.py size    --work DIR             # corpus volume by evaluator, outcome and reason
    prod_corpus.py export  --work DIR             # DIR/corpus.jsonl
    prod_corpus.py sources --work DIR             # DIR/sources.tsv: listing source id, slug, host, cases
    prod_corpus.py query   --work DIR FILE.sql    # one reviewed aggregate query
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

# The prod Compose project's database (host/prod/compose.yaml).
CONTAINER = "aura-prod-postgres-1"
DB_NAME = "aura"
QUERY_ROLE = "aura_runtime"
TIMEOUT_SECONDS = 1800

EXPORT_SQL = """
SELECT jsonb_build_object(
  'caseId', c.case_id::text,
  'entryPoint', c.entry_point,
  'inputSha256', encode(c.input_sha256, 'hex'),
  'listingSourceId', c.listing_source_id::text,
  'recordedOutcome', c.outcome,
  'recordedIssues', c.issues,
  'evidence', CASE
    WHEN r.product_listing_raw_revision_id IS NULL THEN
      jsonb_build_object('kind', 'UNAVAILABLE', 'reason', 'RAW_REVISION_MISSING')
    ELSE jsonb_build_object(
      'kind', 'RAW_REVISION',
      'productListingRawRevisionId', r.product_listing_raw_revision_id::text,
      'input', jsonb_build_object(
        'operation', r.operation,
        'payloadFormat', r.payload_format,
        'payloadSchemaVersion', r.payload_schema_version,
        'rawValuesSchemaVersion', r.raw_values_schema_version,
        'sourcePayload', r.source_payload,
        'rawValues', r.raw_values,
        'normalizationContext', r.normalization_context))
  END)::text
FROM product_listing_normalization_diagnostic_cases AS c
LEFT JOIN product_listing_raw_revisions AS r
  ON r.product_listing_raw_revision_id = c.product_listing_raw_revision_id
ORDER BY c.generation;
"""

SIZE_SQL = """
SELECT 'cases_total', count(*)::text FROM product_listing_normalization_diagnostic_cases
UNION ALL SELECT 'evaluator:v' || evaluator_version, count(*)::text
  FROM product_listing_normalization_diagnostic_cases GROUP BY evaluator_version
UNION ALL SELECT 'outcome:' || outcome, count(*)::text
  FROM product_listing_normalization_diagnostic_cases GROUP BY outcome
UNION ALL SELECT 'distinct_sources', count(DISTINCT listing_source_id)::text
  FROM product_listing_normalization_diagnostic_cases
UNION ALL SELECT 'export_MiB_estimate', round(sum(pg_column_size(r.source_payload)
    + pg_column_size(r.raw_values) + pg_column_size(r.normalization_context)) / 1048576.0, 1)::text
  FROM product_listing_normalization_diagnostic_cases c
  JOIN product_listing_raw_revisions r USING (product_listing_raw_revision_id)
UNION ALL SELECT 'first_recorded', min(first_recorded_at)::text FROM product_listing_normalization_diagnostic_cases
UNION ALL SELECT 'last_recorded', max(last_recorded_at)::text FROM product_listing_normalization_diagnostic_cases;
SELECT 'v' || c.evaluator_version || ' ' || (i->>'field') || '/' || (i->>'reason'), count(*)::text
  FROM product_listing_normalization_diagnostic_cases c, jsonb_array_elements(c.issues) i
  GROUP BY 1 ORDER BY 1;
"""

SOURCES_SQL = """
SELECT s.listing_source_id::text, s.listing_source_slug_id,
       coalesce(substring(s.url from '^https?://([^/]+)'), '-'), count(*)
FROM product_listing_normalization_diagnostic_cases c JOIN listing_sources s USING (listing_source_id)
GROUP BY 1, 2, 3 ORDER BY 4 DESC;
"""

# The wrapper runs every query inside a read-only transaction as the runtime role; this guard also
# refuses anything that could leave it, write, or run psql meta-commands (`\!` would run a shell
# in the database container).
_FORBIDDEN = re.compile(
    r"\b(begin|commit|rollback|abort|savepoint|start\s+transaction|insert|update|delete|merge|"
    r"upsert|alter|drop|create|truncate|grant|revoke|copy|vacuum|reindex|lock|call|do|"
    r"reset|discard|listen|notify|prepare|execute|set\s+(session|transaction|role|local)|"
    r"pg_terminate_backend|pg_cancel_backend|dblink|lo_import|lo_export)\b",
    re.IGNORECASE,
)


class CorpusError(RuntimeError):
    pass


def checked_sql(sql):
    """Return SQL wrapped in a read-only transaction as the runtime role, refusing anything that
    could escape it."""
    without_comments = re.sub(r"--[^\n]*|/\*.*?\*/", " ", sql, flags=re.DOTALL)
    without_strings = re.sub(r"'(?:[^']|'')*'", "''", without_comments)
    if any(line.lstrip().startswith("\\") for line in without_strings.splitlines()):
        raise CorpusError("psql meta-commands are not allowed")
    match = _FORBIDDEN.search(without_strings)
    if match:
        raise CorpusError(f"read-only aggregate queries only; found `{match.group(0)}`")
    if not without_strings.strip():
        raise CorpusError("empty query")
    return f"BEGIN TRANSACTION READ ONLY;\nSET LOCAL ROLE {QUERY_ROLE};\n{sql.strip()}\nCOMMIT;\n"


def psql_command(headers):
    """`psql` in the database container: unaligned and tab-separated, quiet, stopping at the first
    error. Without headers it prints only the rows."""
    return ["docker", "exec", "-i", "-u", "postgres", CONTAINER, "psql", "-X", "-q", "-A",
            "-F", "\t", *([] if headers else ["-t"]), "-v", "ON_ERROR_STOP=1", "-d", DB_NAME, "-f", "-"]


def run_sql(sql, headers=False, output=subprocess.PIPE):
    """Runs one checked query; the query text travels on stdin, never in arguments. Returns its
    output lines, or None when `output` is a file that received them."""
    try:
        result = subprocess.run(psql_command(headers), input=checked_sql(sql), stdout=output,
                                stderr=subprocess.PIPE, text=True, timeout=TIMEOUT_SECONDS)
    except FileNotFoundError as error:
        raise CorpusError("the docker CLI is missing") from error
    except subprocess.TimeoutExpired as error:
        raise CorpusError(f"the query ran longer than {TIMEOUT_SECONDS} s") from error
    if result.returncode != 0:
        raise CorpusError(f"psql exited {result.returncode}: {result.stderr.strip()[:500]}")
    if result.stdout is None:
        return None
    return [line for line in result.stdout.split("\n") if line]


def private_dir(path):
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(0o700)
    return path


def private_file(path):
    """`path` emptied and readable only by its owner, before anything is written to it."""
    path.touch(mode=0o600, exist_ok=True)
    path.chmod(0o600)
    return path.open("w", encoding="utf-8")


def query(work, sql_file):
    private_dir(work)
    print("\n".join(run_sql(Path(sql_file).read_text(), headers=True)))


def size(work):
    private_dir(work)
    print("\n".join(run_sql(SIZE_SQL, headers=True)))


def sources(work):
    rows = [line for line in run_sql(SOURCES_SQL) if line.count("\t") == 3]
    target = private_dir(work) / "sources.tsv"
    with private_file(target) as handle:
        handle.writelines(f"{row}\n" for row in rows)
    print(f"{len(rows)} sources written to {target}")


def export(work):
    """Streams the corpus into the file, so its size never has to fit in memory."""
    corpus = private_dir(work) / "corpus.jsonl"
    with private_file(corpus) as handle:
        run_sql(EXPORT_SQL, output=handle)
    with corpus.open(encoding="utf-8") as handle:
        count = sum(1 for line in handle if line.strip())
    print(f"{count} cases written to {corpus}")


def argument_parser():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["size", "export", "sources", "query"])
    parser.add_argument("sql_file", nargs="?")
    parser.add_argument("--work", type=Path, required=True)
    return parser


def main(argv=None):
    parser = argument_parser()
    # Intermixed parsing accepts `query --work DIR FILE.sql` as well as `query FILE.sql --work DIR`.
    args = parser.parse_intermixed_args(argv)
    if args.command == "query" and not args.sql_file:
        parser.error("query needs a SQL file")
    try:
        {
            "size": lambda: size(args.work),
            "export": lambda: export(args.work),
            "sources": lambda: sources(args.work),
            "query": lambda: query(args.work, args.sql_file),
        }[args.command]()
    except CorpusError as error:
        print(f"prod_corpus: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
