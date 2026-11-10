#!/usr/bin/env python3
"""Read-only access to the production normalization diagnostic corpus.

Throwaway Fargate psql tasks run in the application subnets. Only aggregates, counts and
hashes reach CloudWatch. The corpus itself leaves the VPC over ECS Exec straight into a
private local file, because the S3 gateway endpoint allows no uploads and listing text must
never be logged. Database credentials are injected into the task as ECS secrets; this script
never reads a secret value.

    prod_corpus.py setup    --work DIR   # roles, log group, network discovery -> DIR/state.json
    prod_corpus.py size     --work DIR   # corpus volume by evaluator, outcome and reason
    prod_corpus.py export   --work DIR   # DIR/corpus.jsonl over ECS Exec, SHA-256 verified
    prod_corpus.py sources  --work DIR   # DIR/sources.tsv: listing source id, slug, host, cases
    prod_corpus.py query    --work DIR FILE.sql   # one reviewed aggregate query
    prod_corpus.py teardown [--work DIR]         # delete every aura-normcorpus resource
    prod_corpus.py status                        # list remaining aura-normcorpus resources
"""

import argparse
import base64
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time

REGION = "eu-central-1"
CLUSTER = "aura-historia-scheduled-prod"
SECURITY_GROUP = "aura-historia-application-prod"
SUBNET_NAME = "*applicationSubnet*"
DB_INSTANCE = "aura-historia-postgres-prod"
DB_NAME = "aura_historia"
SECRET_NAME = "/aura-historia/prod/postgres/runtime"
IMAGE = "public.ecr.aws/docker/library/postgres:16-alpine"
PREFIX = "aura-normcorpus"
ROLE_PATH = "/aura-historia/operator/"
LOG_GROUP = f"/tmp/{PREFIX}"
CHUNK_LINES = 25_000
HOLD_SECONDS = 2700
REPO = Path(__file__).resolve().parents[4]
CA_BUNDLE = REPO / "infra/assets/rds-ca-layer/aura-historia/rds-ca/global-bundle.pem"

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

# The task already runs inside a read-only transaction; this guard also refuses anything that
# could leave it, write, or run psql meta-commands (`\!` would run a shell in the container).
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
    """Return SQL wrapped in a read-only transaction, refusing anything that could escape it."""
    without_comments = re.sub(r"--[^\n]*|/\*.*?\*/", " ", sql, flags=re.DOTALL)
    without_strings = re.sub(r"'(?:[^']|'')*'", "''", without_comments)
    if any(line.lstrip().startswith("\\") for line in without_strings.splitlines()):
        raise CorpusError("psql meta-commands are not allowed")
    match = _FORBIDDEN.search(without_strings)
    if match:
        raise CorpusError(f"read-only aggregate queries only; found `{match.group(0)}`")
    if not without_strings.strip():
        raise CorpusError("empty query")
    return f"BEGIN TRANSACTION READ ONLY;\n{sql.strip()}\nCOMMIT;\n"


def extract_chunk(output):
    """Lines between the BEGIN_CHUNK/END_CHUNK markers of one ECS Exec session, CR removed."""
    lines = output.replace("\r", "").split("\n")
    try:
        start = lines.index("BEGIN_CHUNK") + 1
        end = lines.index("END_CHUNK", start)
    except ValueError:
        return None
    return lines[start:end]


def parse_export_report(text):
    """Counts the export task logs before it waits: lines, gz_bytes, sha256, b64_lines."""
    report = {}
    for key, value in re.findall(r"\b(lines|gz_bytes|sha256|b64_lines)=([0-9a-f]+)", text):
        report[key] = value if key == "sha256" else int(value)
    return report


def aws(*args, parse=True, check=True):
    result = subprocess.run(
        ["aws", *args, "--region", REGION, "--output", "json"],
        capture_output=True, text=True,
    )
    if check and result.returncode != 0:
        raise CorpusError(f"aws {' '.join(args[:2])} failed: {result.stderr.strip()[:500]}")
    if not parse:
        return result
    return json.loads(result.stdout) if result.stdout.strip() else {}


def private_dir(path):
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(0o700)
    return path


def load_state(work):
    state = work / "state.json"
    if not state.exists():
        raise CorpusError(f"{state} is missing; run `setup --work {work}` first")
    return json.loads(state.read_text())


def regional_ca():
    """The committed RDS bundle filtered to this region's roots (the full bundle exceeds ECS limits)."""
    certificates = re.findall(
        r"-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----", CA_BUNDLE.read_text(), re.DOTALL
    )
    kept = []
    for certificate in certificates:
        subject = subprocess.run(
            ["openssl", "x509", "-noout", "-subject"], input=certificate, capture_output=True, text=True
        ).stdout
        if REGION in subject:
            kept.append(certificate)
    if not kept:
        raise CorpusError(f"no {REGION} root in {CA_BUNDLE}")
    return "\n".join(kept) + "\n"


def setup(work):
    private_dir(work)
    account = aws("sts", "get-caller-identity")["Account"]
    group = aws("ec2", "describe-security-groups", "--filters", f"Name=group-name,Values={SECURITY_GROUP}")
    group = group["SecurityGroups"][0]
    subnets = aws("ec2", "describe-subnets", "--filters", f"Name=vpc-id,Values={group['VpcId']}",
                  f"Name=tag:Name,Values={SUBNET_NAME}")["Subnets"]
    if not subnets:
        raise CorpusError("no application subnets found")
    database = aws("rds", "describe-db-instances", "--db-instance-identifier", DB_INSTANCE)["DBInstances"][0]
    secret = aws("secretsmanager", "describe-secret", "--secret-id", SECRET_NAME)["ARN"]

    aws("logs", "create-log-group", "--log-group-name", LOG_GROUP, parse=False, check=False)
    aws("logs", "put-retention-policy", "--log-group-name", LOG_GROUP, "--retention-in-days", "1", parse=False)
    trust = json.dumps({"Version": "2012-10-17", "Statement": [{"Effect": "Allow",
        "Principal": {"Service": "ecs-tasks.amazonaws.com"}, "Action": "sts:AssumeRole"}]})
    execution = json.dumps({"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["secretsmanager:GetSecretValue"], "Resource": secret},
        {"Effect": "Allow", "Action": ["logs:CreateLogStream", "logs:PutLogEvents"],
         "Resource": f"arn:aws:logs:{REGION}:{account}:log-group:{LOG_GROUP}:*"}]})
    exec_channel = json.dumps({"Version": "2012-10-17", "Statement": [{"Effect": "Allow",
        "Action": ["ssmmessages:CreateControlChannel", "ssmmessages:CreateDataChannel",
                   "ssmmessages:OpenControlChannel", "ssmmessages:OpenDataChannel"], "Resource": "*"}]})
    roles = {}
    for name, policy in ((f"{PREFIX}-execution", execution), (f"{PREFIX}-task", exec_channel)):
        aws("iam", "create-role", "--role-name", name, "--path", ROLE_PATH,
            "--assume-role-policy-document", trust, parse=False, check=False)
        aws("iam", "put-role-policy", "--role-name", name, "--policy-name", "access",
            "--policy-document", policy, parse=False)
        roles[name] = aws("iam", "get-role", "--role-name", name)["Role"]["Arn"]
    state = {
        "account": account,
        "subnets": sorted(subnet["SubnetId"] for subnet in subnets),
        "securityGroup": group["GroupId"],
        "dbHost": database["Endpoint"]["Address"],
        "secretArn": secret,
        "executionRole": roles[f"{PREFIX}-execution"],
        "taskRole": roles[f"{PREFIX}-task"],
        "ca": regional_ca(),
    }
    (work / "state.json").write_text(json.dumps(state, indent=1))
    (work / "state.json").chmod(0o600)
    print(f"account {account}; subnets {state['subnets']}; security group {state['securityGroup']}")
    print(f"state written to {work / 'state.json'}")


def task_definition(state, family, sql, hold):
    psql = 'psql -v ON_ERROR_STOP=1 -h "$PGHOST" -p 5432 -d "$PGDATABASE" '
    prepare = "set -e; printf '%s' \"$CA_PEM\" > /tmp/ca.pem; echo \"$SQL_B64\" | base64 -d > /tmp/q.sql; "
    if hold:
        command = (prepare + psql + "-At -f /tmp/q.sql | grep -v -x -e BEGIN -e COMMIT -e '' "
                   "| gzip -9 > /tmp/out.jsonl.gz; "
                   "echo \"lines=$(gzip -dc /tmp/out.jsonl.gz | wc -l) gz_bytes=$(stat -c %s /tmp/out.jsonl.gz) "
                   "sha256=$(sha256sum /tmp/out.jsonl.gz | cut -d' ' -f1)\"; "
                   "base64 -w 76 /tmp/out.jsonl.gz > /tmp/out.b64; echo \"b64_lines=$(wc -l < /tmp/out.b64)\"; "
                   f"echo READY; sleep {HOLD_SECONDS}")
    else:
        command = prepare + psql + "-A -F '\t' -f /tmp/q.sql"
    container = {
        "name": "psql", "image": IMAGE, "essential": True, "entryPoint": ["/bin/sh", "-c"], "command": [command],
        "environment": [
            {"name": "PGHOST", "value": state["dbHost"]},
            {"name": "PGDATABASE", "value": DB_NAME},
            {"name": "PGSSLMODE", "value": "verify-full"},
            {"name": "PGSSLROOTCERT", "value": "/tmp/ca.pem"},
            {"name": "PGCONNECT_TIMEOUT", "value": "15"},
            {"name": "CA_PEM", "value": state["ca"]},
            {"name": "SQL_B64", "value": base64.b64encode(checked_sql(sql).encode()).decode()},
        ],
        "secrets": [
            {"name": "PGUSER", "valueFrom": state["secretArn"] + ":username::"},
            {"name": "PGPASSWORD", "valueFrom": state["secretArn"] + ":password::"},
        ],
        "logConfiguration": {"logDriver": "awslogs", "options": {
            "awslogs-group": LOG_GROUP, "awslogs-region": REGION, "awslogs-stream-prefix": family}},
    }
    return {"family": family, "networkMode": "awsvpc", "requiresCompatibilities": ["FARGATE"],
            "cpu": "1024", "memory": "4096", "executionRoleArn": state["executionRole"],
            "taskRoleArn": state["taskRole"], "containerDefinitions": [container]}


def run_task(state, family, sql, hold=False):
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
        json.dump(task_definition(state, family, sql, hold), handle)
    try:
        arn = aws("ecs", "register-task-definition", "--cli-input-json",
                  f"file://{handle.name}")["taskDefinition"]["taskDefinitionArn"]
    finally:
        os.unlink(handle.name)
    network = ("awsvpcConfiguration={subnets=[" + ",".join(state["subnets"]) + "],securityGroups=["
               + state["securityGroup"] + "],assignPublicIp=DISABLED}")
    extra = ["--enable-execute-command"] if hold else []
    # Freshly created roles can take a few seconds to become assumable.
    for attempt in range(8):
        result = aws("ecs", "run-task", "--cluster", CLUSTER, "--launch-type", "FARGATE",
                     "--task-definition", arn, "--network-configuration", network, *extra, check=False, parse=False)
        if result.returncode == 0 and json.loads(result.stdout)["tasks"]:
            return json.loads(result.stdout)["tasks"][0]["taskArn"]
        time.sleep(10)
    raise CorpusError(f"run-task failed: {result.stderr.strip()[:500] or result.stdout[:500]}")


def describe(task):
    return aws("ecs", "describe-tasks", "--cluster", CLUSTER, "--tasks", task)["tasks"][0]


def log_lines(family, task):
    stream = f"{family}/psql/{task.rsplit('/', 1)[-1]}"
    lines, token = [], None
    while True:
        args = ["logs", "get-log-events", "--log-group-name", LOG_GROUP, "--log-stream-name", stream,
                "--start-from-head"]
        if token:
            args += ["--next-token", token]
        page = aws(*args, check=False, parse=False)
        if page.returncode != 0:
            return lines
        page = json.loads(page.stdout)
        lines += [event["message"] for event in page.get("events", [])]
        if page.get("nextForwardToken") in (None, token):
            return lines
        token = page["nextForwardToken"]


def wait_stopped(task):
    while (described := describe(task))["lastStatus"] != "STOPPED":
        time.sleep(10)
    return described


def deregister(family):
    for arn in aws("ecs", "list-task-definitions", "--family-prefix", family)["taskDefinitionArns"]:
        aws("ecs", "deregister-task-definition", "--task-definition", arn, parse=False)
        aws("ecs", "delete-task-definitions", "--task-definitions", arn, parse=False, check=False)


def run_query(work, name, sql):
    state = load_state(work)
    family = f"{PREFIX}-{name}"
    task = run_task(state, family, sql)
    described = wait_stopped(task)
    lines = log_lines(family, task)
    deregister(family)
    exit_code = described["containers"][0].get("exitCode")
    if exit_code != 0:
        raise CorpusError(f"query task exited {exit_code}: {described.get('stoppedReason')}\n" + "\n".join(lines))
    return [line for line in lines if line not in ("BEGIN", "COMMIT")]


def query(work, sql_file):
    print("\n".join(run_query(work, "query", Path(sql_file).read_text())))


def size(work):
    print("\n".join(run_query(work, "size", SIZE_SQL)))


def sources(work):
    rows = [line for line in run_query(work, "sources", SOURCES_SQL) if line.count("\t") == 3]
    target = work / "sources.tsv"
    target.write_text("\n".join(rows) + "\n")
    target.chmod(0o600)
    print(f"{len(rows) - 1} sources written to {target}")


def exec_chunk(task, start, end, timeout=600):
    """Read one chunk over ECS Exec.

    The Session Manager plugin ends the session as soon as its stdin reaches EOF, which truncates
    large outputs, so stdin stays open until the END_CHUNK marker has arrived.
    """
    command = f"sh -c 'echo BEGIN_CHUNK; sed -n {start},{end}p /tmp/out.b64; echo END_CHUNK'"
    process = subprocess.Popen(
        ["aws", "ecs", "execute-command", "--cluster", CLUSTER, "--task", task, "--container", "psql",
         "--interactive", "--region", REGION, "--command", command],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
    )
    timer = threading.Timer(timeout, process.kill)
    timer.start()
    lines = []
    try:
        for line in process.stdout:
            lines.append(line)
            if line.replace("\r", "").rstrip("\n") == "END_CHUNK":
                break
    finally:
        timer.cancel()
        process.stdin.close()
        try:
            process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            process.kill()
    return extract_chunk("".join(lines))


def export(work):
    if not shutil.which("session-manager-plugin"):
        raise CorpusError("session-manager-plugin is missing; install it as described in the SKILL")
    state = load_state(work)
    family = f"{PREFIX}-export"
    task = run_task(state, family, EXPORT_SQL, hold=True)
    try:
        report = {}
        while "b64_lines" not in report:
            if describe(task)["lastStatus"] == "STOPPED":
                raise CorpusError("export task stopped early:\n" + "\n".join(log_lines(family, task)))
            report = parse_export_report("\n".join(log_lines(family, task)))
            time.sleep(10)
        print(f"staged: {report['lines']} cases, {report['gz_bytes']} gz bytes, sha256 {report['sha256']}")
        while not any(agent["name"] == "ExecuteCommandAgent" and agent.get("lastStatus") == "RUNNING"
                      for agent in describe(task)["containers"][0].get("managedAgents", [])):
            time.sleep(5)
        encoded = []
        total = report["b64_lines"]
        for start in range(1, total + 1, CHUNK_LINES):
            end = min(start + CHUNK_LINES - 1, total)
            for attempt in range(4):
                chunk = exec_chunk(task, start, end)
                if chunk is not None and len(chunk) == end - start + 1:
                    break
                print(f"retrying lines {start}-{end} (attempt {attempt + 1})", file=sys.stderr)
            else:
                raise CorpusError(f"could not transfer lines {start}-{end}")
            encoded += chunk
            print(f"transferred {end}/{total} lines", file=sys.stderr)
        compressed = base64.b64decode("".join(encoded))
        if hashlib.sha256(compressed).hexdigest() != report["sha256"]:
            raise CorpusError("SHA-256 mismatch; transfer corrupted")
        corpus = work / "corpus.jsonl"
        corpus.write_bytes(gzip.decompress(compressed))
        corpus.chmod(0o600)
        count = sum(1 for _ in corpus.open())
        if count != report["lines"]:
            raise CorpusError(f"expected {report['lines']} cases, wrote {count}")
        print(f"{count} cases written to {corpus} (sha256 verified)")
    finally:
        aws("ecs", "stop-task", "--cluster", CLUSTER, "--task", task, "--reason", "corpus retrieved",
            parse=False, check=False)
        deregister(family)


def remaining():
    families = sorted({arn.rsplit("/", 1)[-1].rsplit(":", 1)[0] for arn in
                       aws("ecs", "list-task-definitions", "--family-prefix", PREFIX)["taskDefinitionArns"]})
    roles = [role["RoleName"] for role in aws("iam", "list-roles", "--path-prefix", ROLE_PATH)["Roles"]
             if role["RoleName"].startswith(PREFIX)]
    groups = [group["logGroupName"] for group in
              aws("logs", "describe-log-groups", "--log-group-name-prefix", LOG_GROUP)["logGroups"]]
    tasks = [task for family in families for task in
             aws("ecs", "list-tasks", "--cluster", CLUSTER, "--family", family)["taskArns"]]
    return {"taskDefinitionFamilies": families, "runningTasks": tasks, "roles": roles, "logGroups": groups}


def status():
    print(json.dumps(remaining(), indent=1))


def teardown(work):
    found = remaining()
    for task in found["runningTasks"]:
        aws("ecs", "stop-task", "--cluster", CLUSTER, "--task", task, "--reason", "teardown", parse=False, check=False)
    for family in found["taskDefinitionFamilies"]:
        deregister(family)
    for role in found["roles"]:
        for policy in aws("iam", "list-role-policies", "--role-name", role)["PolicyNames"]:
            aws("iam", "delete-role-policy", "--role-name", role, "--policy-name", policy, parse=False)
        aws("iam", "delete-role", "--role-name", role, parse=False)
    for group in found["logGroups"]:
        aws("logs", "delete-log-group", "--log-group-name", group, parse=False)
    if work and (work / "state.json").exists():
        (work / "state.json").unlink()
    left = remaining()
    if any(left.values()):
        raise CorpusError(f"resources remain: {json.dumps(left)}")
    print("all aura-normcorpus resources deleted")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["setup", "size", "export", "sources", "query", "teardown", "status"])
    parser.add_argument("sql_file", nargs="?")
    parser.add_argument("--work", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command in ("setup", "size", "export", "sources", "query") and not args.work:
            parser.error("--work is required")
        if args.command == "query" and not args.sql_file:
            parser.error("query needs a SQL file")
        {
            "setup": lambda: setup(args.work),
            "size": lambda: size(args.work),
            "export": lambda: export(args.work),
            "sources": lambda: sources(args.work),
            "query": lambda: query(args.work, args.sql_file),
            "teardown": lambda: teardown(args.work),
            "status": status,
        }[args.command]()
    except CorpusError as error:
        print(f"prod_corpus: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
