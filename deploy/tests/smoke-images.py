#!/usr/bin/env python3
"""R2 native image smoke; provider doubles, not provider acceptance.
Canary checks cover probe bodies and retained logs (1 MiB), not whole-lifetime logs.
Unconfirmed Docker commands stop the run; printed recovery identities need manual review.
"""
import argparse
import collections
import datetime
import difflib
import itertools
import hashlib
import http.client
import http.server
import json

from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid

SHA = "672bdcefdeaabc6cd9f78461ec1bf859c31dc443"
SOURCE = "https://github.com/aura-historia/backend"
OWNER_LABEL = "org.aura-historia.r2-smoke-owner"
TRUSTED_ENV = dict(PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                   HOME="/home/aura", COMMIT_SHA=SHA)
SYSTEM_ATTRIBUTES = ["ApproximateReceiveCount", "SentTimestamp", "ApproximateFirstReceiveTimestamp"]
LOG_LIMIT = 1024 * 1024
PG_IMAGE = "sha256:1ef4f65fa354b5771def2872dc765c5cafb5c3f1e56ce1d394a6ad4af33279be"
SCOPES = ("product-listing-opensearch", "product-listing-normalization")
BINS = dict(api="aura-historia-api", worker="aura-historia-worker", cron="aura-historia-cron", crawler="server")
PORTS = dict(api=9080, worker=8081, cron=8082, crawler=9083)
SECRET = "smoke-secret-canary"
ENDPOINT = "http://127.0.0.1:18090"
SCRIPT = "/smoke/smoke-images.py"
SIGNALS = {signal.SIGINT, signal.SIGTERM, signal.SIGALRM}


class Failure(Exception):
    pass


def require(condition, code):
    if not condition:
        raise Failure(code)


def until(check, seconds, code):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if check():
            return
        time.sleep(0.25)
    raise Failure(code)


def attributes(scope, dlq, broken=False):
    def arn(dead):
        return f"arn:aws:sqs:eu-central-1:000000000000:aura-worker-{scope}{'-dlq' if dead else ''}-test"
    value = {
        "QueueArn": arn(dlq), "FifoQueue": "false", "SqsManagedSseEnabled": "true",
        "MessageRetentionPeriod": "1" if broken else ("1209600" if dlq else "604800"),
        "VisibilityTimeout": "60" if scope == SCOPES[0] else "300",
        "ReceiveMessageWaitTimeSeconds": "20",
        "Policy": json.dumps({"Statement": [{"Effect": "Deny", "Principal": "*", "Action": "sqs:*",
                    "Resource": arn(dlq), "Condition": {"Bool": {"aws:SecureTransport": "false"}}}]}),
        "RedriveAllowPolicy": json.dumps({"redrivePermission": "byQueue", "sourceQueueArns": [arn(False)]}
                                         if dlq else {"redrivePermission": "denyAll"}),
    }
    if not dlq:
        value["RedrivePolicy"] = json.dumps({"deadLetterTargetArn": arn(True), "maxReceiveCount": 5})
    return {"Attributes": value}


class Provider(http.server.BaseHTTPRequestHandler):
    counts = collections.Counter()
    lock = threading.Lock()
    bad_queue = False
    bad_jwks = False
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass  # Provider requests contain synthetic credentials; never print bodies/headers.

    def send_error(self, *_args, **_kwargs):
        self.count("unexpected")
        self.close_connection = True

    def setup(self):
        super().setup()
        self.connection.settimeout(2)

    @classmethod
    def count(cls, key, amount=1):
        with cls.lock:
            cls.counts[key] += amount

    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/x-amz-json-1.0")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        self.wfile.write(data)

    def handle_request(self):
        # Witness attempts before body reads, validation, delays, or response writes.
        operation = self.headers.get("x-amz-target", "")
        if self.command == "POST" and self.path == "/token":
            self.count("token_attempt")
        if operation == "AmazonSQS.ReceiveMessage":
            self.count("receive_attempt")
        try:
            require(not self.headers.get("Transfer-Encoding"), "BODY_ENCODING")
            size = int(self.headers.get("Content-Length", "0"))
            require(0 <= size <= 16384, "BODY_SIZE")
            data = json.loads(self.rfile.read(size)) if size else None
            if self.command == "GET" and self.path == "/_state":
                with self.lock:
                    state = dict(self.counts)
                self.reply(200, state)
            elif self.command == "PUT" and self.path == "/_fault":
                require(isinstance(data, dict) and set(data) <= {"bad_queue", "bad_jwks"}, "FAULT")
                require(all(type(v) is bool for v in data.values()), "FAULT")
                Provider.bad_queue = data.get("bad_queue", False)
                Provider.bad_jwks = data.get("bad_jwks", False)
                self.reply(200, {"bad_queue": self.bad_queue, "bad_jwks": self.bad_jwks})
            elif self.command == "GET" and self.path == "/jwks":
                self.count("bad_jwks" if self.bad_jwks else "jwks")
                self.reply(200, {"keys": [] if self.bad_jwks else [
                    {"kid": "smoke", "alg": "RS256", "n": "AQAB", "e": "AQAB"}]})
            elif self.command == "POST" and self.path == "/token":
                require(data == {"grant_type": "refresh_token", "client_id": "smoke-client",
                    "client_secret": SECRET, "refresh_token": SECRET,
                    "scopes": "https://www.googleapis.com/auth/cloud-platform"}, "TOKEN_REQUEST")
                self.reply(200, {"access_token": SECRET, "token_type": "Bearer", "expires_in": 3600})
                self.count("token_complete")
            elif self.command == "POST" and self.path == "/":
                require(isinstance(data, dict), "SQS_BODY")
                queues = {f"{ENDPOINT}/000000000000/aura-worker-{s}{'-dlq' if d else ''}-test": (s, d)
                          for s in SCOPES for d in (False, True)}
                require(data.get("QueueUrl") in queues, "QUEUE_IDENTITY")
                scope, dlq = queues[data["QueueUrl"]]
                if operation == "AmazonSQS.GetQueueAttributes":
                    self.count("bad_source_queue" if self.bad_queue and not dlq else "attributes")
                    self.reply(200, attributes(scope, dlq, self.bad_queue and not dlq))
                else:
                    require(operation == "AmazonSQS.ReceiveMessage" and not dlq
                            and data.get("WaitTimeSeconds") == 20 and data.get("MaxNumberOfMessages") == 1
                            and data.get("VisibilityTimeout") == (60 if scope == SCOPES[0] else 300)
                            and data.get("MessageSystemAttributeNames") == SYSTEM_ATTRIBUTES,
                            "UNEXPECTED_SQS_OPERATION")
                    self.count("active")
                    try:
                        time.sleep(20)
                        self.reply(200, {})
                        self.count("receive_complete")
                    finally:
                        self.count("active", -1)
            else:
                raise Failure("UNEXPECTED_HTTP_OPERATION")
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            pass  # Owned application may cancel an outstanding poll during shutdown.
        except Exception:
            self.count("unexpected")
            self.close_connection = True

    do_GET = do_POST = do_PUT = do_CONNECT = do_DELETE = do_HEAD = handle_request


class ProviderServer(http.server.ThreadingHTTPServer):
    daemon_threads = True
    slots = threading.BoundedSemaphore(16)

    def process_request(self, request, address):
        if not self.slots.acquire(blocking=False):
            Provider.count("unexpected")
            self.shutdown_request(request)
            return
        super().process_request(request, address)

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            self.slots.release()

    def handle_error(self, *_args):
        Provider.count("unexpected")


def http_request(port, path, method, body):
    require(port in {7878, 8080, 8081, 8082, 9080, 9083, 9200, 18090} and path.startswith("/"), "HTTP_TARGET")
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
    try:
        require(len(body) <= 65536, "HTTP_BODY")
        connection.request(method, path, body=body or None, headers={"Content-Type": "application/json"})
        response = connection.getresponse()
        raw = response.read(65537)
        require(len(raw) <= 65536, "HTTP_RESPONSE_SIZE")
        try:
            value = json.loads(raw)
        except (ValueError, UnicodeError):
            value = raw.decode("utf-8", "replace")
        return {"status": response.status, "cache": response.getheader("Cache-Control", ""), "body": value}
    except ConnectionRefusedError:
        return {"status": 0, "error": "refused", "cache": "", "body": None}
    except (OSError, http.client.HTTPException):
        raise Failure("HTTP_TRANSPORT_UNCONFIRMED") from None
    finally:
        connection.close()


class Docker:
    def __init__(self, directory):
        self.config = str(directory / "docker")
        Path(self.config).mkdir(mode=0o700)
        self.owned = []
        self.recovery = []

    def call(self, *args, body=None, seconds=20, check=True):
        try:
            with tempfile.TemporaryFile() as output:
                result = subprocess.run(["/usr/bin/docker", "--host", "unix:///var/run/docker.sock",
                    "--config", self.config, *args], input=body, stdout=output,
                    stderr=subprocess.STDOUT, text=True, timeout=seconds, env={"PATH": "/usr/bin:/bin"})
                output.seek(0)
                raw = output.read(LOG_LIMIT + 1)
                require(len(raw) <= LOG_LIMIT, "DOCKER_OUTPUT_LIMIT")
                result.stdout = raw.decode("utf-8", "replace")
        except (OSError, subprocess.TimeoutExpired):
            raise Failure("DOCKER_COMMAND_UNCONFIRMED") from None
        require(not check or result.returncode == 0, "DOCKER_COMMAND_FAILED")
        return result.stdout if check else result

    def create(self, image, options, command=()):
        previous = signal.pthread_sigmask(signal.SIG_BLOCK, SIGNALS)
        try:
            name, token = "aura-r2-smoke-" + uuid.uuid4().hex, uuid.uuid4().hex
            recovery = f"create name={name} label={OWNER_LABEL}={token} (unconfirmed; no retry, manual recovery)"
            self.recovery.append(recovery)  # Retained even if create fails or returns no usable ID.
            cidfile = Path(self.config) / (name + ".cid")
            identifier = self.call("create", "--pull=never", "--restart=no", "--no-healthcheck",
                "--name", name, "--label", OWNER_LABEL + "=" + token, "--cidfile", str(cidfile),
                "--log-driver=json-file", "--log-opt=max-size=1m", "--log-opt=max-file=1",
                *options, image, *command).strip()
            require(re.fullmatch(r"[0-9a-f]{64}", identifier)
                    and cidfile.read_text().strip() == identifier, "CREATE_NO_OWNERSHIP")
            acquired = json.loads(self.call("container", "inspect", identifier))[0]
            require(acquired["Id"] == identifier and acquired["Name"] == "/" + name
                    and acquired["Config"]["Labels"].get(OWNER_LABEL) == token, "CREATE_IDENTITY")
            self.owned.append(identifier)
            self.recovery.remove(recovery)
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, previous)
        self.call("start", identifier)
        return identifier

    def state(self, identifier):
        return json.loads(self.call("inspect", "--format", "{{json .State}}", identifier))

    def remove(self, identifier):
        require(identifier in self.owned, "UNOWNED_CLEANUP")
        # An unknown rm result must not be retried by final cleanup.
        recovery = "cleanup ID=" + identifier + " (unconfirmed; no retry, manual recovery)"
        self.recovery.append(recovery)
        self.owned.remove(identifier)
        query = ("container", "ls", "--all", "--no-trunc", "--filter", "id=" + identifier, "--format", "{{.ID}}")
        present = self.call(*query).strip()
        require(present in ("", identifier), "CLEANUP_IDENTITY")
        if present:
            self.call("rm", "--force", identifier)
        require(not self.call(*query).strip(), "CLEANUP_UNCONFIRMED")
        self.recovery.remove(recovery)


def environment(kind, scope=None):
    env = dict(STAGE="test", POSTGRES_SSL_MODE="disable", POSTGRES_HOST="127.0.0.1", POSTGRES_PORT="5432",
        POSTGRES_DATABASE="smoke_business", POSTGRES_USERNAME="smoke_runtime", POSTGRES_PASSWORD=SECRET,
        POSTGRES_MAX_CONNECTIONS="2", LOG_LEVEL="info", AWS_REGION="eu-central-1",
        AWS_ACCESS_KEY_ID="smoke-key", AWS_SECRET_ACCESS_KEY=SECRET, AWS_EC2_METADATA_DISABLED="true",
        AWS_CONFIG_FILE="/dev/null", AWS_SHARED_CREDENTIALS_FILE="/dev/null",
        OPENSEARCH_ENDPOINT_URL="http://127.0.0.1:9200", VERTEX_AI_PROJECT_ID="smoke-project",
        VERTEX_AI_LOCATION="eu", VERTEX_AI_MODEL="smoke-model", GOOGLE_APPLICATION_CREDENTIALS="/smoke-adc.json")
    if kind == "api":
        env.update(AURA_HISTORIA_API_BIND_ADDR="127.0.0.1:8080", AURA_HISTORIA_API_OPERATIONS_BIND_ADDR="127.0.0.1:9080",
            AURA_HISTORIA_COGNITO_ISSUER=ENDPOINT + "/issuer", AURA_HISTORIA_COGNITO_JWKS_URL=ENDPOINT + "/jwks",
            AURA_HISTORIA_COGNITO_APP_CLIENT_IDS="smoke-client", AURA_HISTORIA_COGNITO_USER_POOL_ID="smoke-pool",
            OPENSEARCH_USERNAME="smoke", OPENSEARCH_PASSWORD=SECRET, STRIPE_API_KEY=SECRET,
            STRIPE_CHECKOUT_SUCCESS_URL=ENDPOINT + "/success", STRIPE_CHECKOUT_CANCEL_URL=ENDPOINT + "/cancel",
            STRIPE_PORTAL_RETURN_URL=ENDPOINT + "/return", ZOHO_LIST_KEY="smoke", ZOHO_CLIENT_ID="smoke-client",
            ZOHO_CLIENT_SECRET=SECRET, ZOHO_REFRESH_TOKEN=SECRET, ZOHO_ACCOUNTS_URL=ENDPOINT + "/zoho-accounts",
            ZOHO_CAMPAIGNS_URL=ENDPOINT + "/zoho-campaigns")
        for tier in ("PRO", "ULTIMATE"):
            for period in ("MONTHLY", "YEARLY"):
                env[f"STRIPE_{tier}_{period}_PRICE_ID"] = f"price_smoke_{tier}_{period}"
    elif kind == "worker":
        env.update(AWS_ENDPOINT_URL_SQS=ENDPOINT, AURA_HISTORIA_WORKER_SCOPE=scope,
            AURA_HISTORIA_WORKER_QUEUE_URL=f"{ENDPOINT}/000000000000/aura-worker-{scope}-test",
            AURA_HISTORIA_WORKER_HEALTH_BIND_ADDR="127.0.0.1:8081")
    elif kind == "cron":
        tomorrow = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(days=1)
        env.update(AURA_HISTORIA_CRON_ENABLED_JOBS="search-filter-periodic-match",
            AURA_HISTORIA_CRON_HEALTH_BIND_ADDR="127.0.0.1:8082",
            SEARCH_FILTER_PERIODIC_MATCH_CRON=f"0 0 15 {tomorrow.day} {tomorrow.month} * {tomorrow.year}")
    elif kind == "crawler":
        env.update(LOCAL_DB_URL=f"postgres://smoke_runtime:{SECRET}@127.0.0.1:5432/smoke_crawler",
            BUSINESS_DATABASE_URL=f"postgres://smoke_runtime:{SECRET}@127.0.0.1:5432/smoke_business",
            SPIDER_MAX_SIZE_BYTES="8388608", CRAWLER_VERTEX_AI_CHEAP_MODEL="smoke-model",
            CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL="smoke-model", CRAWLER_REVIEW_BIND_ADDR="127.0.0.1:7878",
            CRAWLER_REVIEW_AUTH_TOKEN=SECRET, CRAWLER_OPERATIONS_BIND_ADDR="127.0.0.1:9083")
    return env


def env_args(values):
    return [arg for key, value in values.items() for arg in ("--env", key + "=" + value)]


def safe_output(text):
    require(all(value not in text for value in (SECRET, "postgres://", "postgresql://")), "OUTPUT_REDACTION")


def no_attempts(before, after):
    require(all(after.get(key, 0) == before.get(key, 0) for key in ("token_attempt", "receive_attempt"))
            and not after.get("active"), "FORBIDDEN_PROVIDER_ATTEMPT")


def identity(kind, scope=None):
    if kind in ("api", "crawler"):
        return {"commit_sha": SHA, "state": "READY"}
    if kind == "worker":
        return dict(schema_version=1, component=BINS[kind], scope=scope, source_sha=SHA, local=True)
    return dict(schema_version=1, component=BINS[kind], stage="test", source_sha=SHA)


def fields_match(record, fields):
    return all(type(record.get(key)) is type(value) and record[key] == value for key, value in fields.items())


def normalization_completed(records):
    expected = dict(metric="product_listing_raw_normalization_reconciliation",
        job_type="product_listing_raw_normalization_reconciliation", outcome="completed", reconciliation_runs=1,
        processed_revisions=0, normalization_failures=0, pending_stream_page_count=0,
        reconciliation_page="global_initial", pending_stream_cursor_present=False,
        reconciliation_continuation_stream_count=0, unscheduled_continuation_count=0, suppressed_continuation_count=0)
    return any(record.get("level") == "INFO" and fields_match(record.get("fields", {}), expected) for record in records)


def mapping_readback(value):
    # Observed OpenSearch 3.1 readback omits object type and default enabled:true.
    # Preserve every other checked-in field, including nested and enabled:false.
    if isinstance(value, dict):
        redundant_object = value.get("type") == "object" and "properties" in value
        return {key: mapping_readback(child) for key, child in value.items()
                if not (redundant_object and (key == "type" or key == "enabled" and child is True))}
    if isinstance(value, list):
        return [mapping_readback(child) for child in value]
    return value


def verify_image(key, image, metadata, opensearch_digest):
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", image), "IMMUTABLE_LOCAL_IMAGE_REQUIRED")
    require(metadata["Id"] == image and metadata["Os"] == "linux" and metadata["Architecture"] == "amd64", "IMAGE_PLATFORM")
    config = metadata["Config"]
    volumes = {"postgres": {"/var/lib/postgresql/data"}, "opensearch": {"/usr/share/opensearch/data"}}
    require(set(config.get("Volumes") or {}) <= volumes.get(key, set()), "UNEXPECTED_IMAGE_VOLUME")
    if key == "postgres":
        require(image == PG_IMAGE, "POSTGRES_PIN")
    elif key == "opensearch":
        require(re.fullmatch(r"opensearchproject/opensearch@sha256:[0-9a-f]{64}", opensearch_digest)
                and opensearch_digest in metadata.get("RepoDigests", []), "OPENSEARCH_OFFICIAL_PIN")
    else:
        baked = dict(item.split("=", 1) for item in config.get("Env") or [])
        require(len(baked) == len(config.get("Env") or [])
                and all(baked.get(key) == value for key, value in TRUSTED_ENV.items())
                and all(key in TRUSTED_ENV or key in ("LANG", "LC_ALL") and value == "C.UTF-8"
                        for key, value in baked.items()), "UNTRUSTED_BAKED_ENVIRONMENT")
        labels = config.get("Labels") or {}
        require(labels.get("org.opencontainers.image.revision") == SHA
                and labels.get("org.opencontainers.image.source") == SOURCE, "IMAGE_PROVENANCE")
        entrypoint = "/usr/bin/python3" if key == "helper" else "/usr/local/bin/" + BINS[key]
        require(config["User"] == "10001:10001" and config["Entrypoint"] == [entrypoint]
                and not config.get("Cmd"), "IMAGE_USER_OR_ENTRYPOINT")


def run(args, directory, docker):
    root = Path(__file__).resolve().parents[2]
    images = {key: getattr(args, key) for key in (*BINS, "helper", "postgres", "opensearch")}
    for key, image in images.items():
        require(re.fullmatch(r"sha256:[0-9a-f]{64}", image), "IMMUTABLE_LOCAL_IMAGE_REQUIRED")
        metadata = json.loads(docker.call("image", "inspect", image))[0]
        verify_image(key, image, metadata, args.opensearch_digest)
        print(f"IMAGE {key} {image}", flush=True)
    adc = directory / "adc.json"
    adc.write_text(json.dumps(dict(type="authorized_user", client_id="smoke-client", client_secret=SECRET,
                                  refresh_token=SECRET, token_uri=ENDPOINT + "/token")))
    adc.chmod(0o444)  # Public synthetic canary only; parent directory remains 0700.
    mounts = ["--mount", f"type=bind,src={Path(__file__).resolve()},dst={SCRIPT},readonly"]
    helper = docker.create(images["helper"], ["--network=none", "--read-only", "--user=10001:10001",
        "--cap-drop=ALL", "--security-opt=no-new-privileges", "--pids-limit=64", "--memory=256m", "--memory-swap=256m",
        "--cpus=1", "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=16m,mode=1777", *mounts,
        "--entrypoint=/usr/bin/python3"], [SCRIPT, "_serve"])
    shared = ["--network=container:" + helper, "--security-opt=no-new-privileges"]

    def request(port, path, method="GET", body=None):
        raw = docker.call("exec", "-i", helper, "/usr/bin/python3", SCRIPT, "_http", str(port), path, method,
                          body=json.dumps(body) if body is not None else "", seconds=10)
        safe_output(raw)
        return json.loads(raw)

    def stats():
        response = request(18090, "/_state")
        require(response["status"] == 200, "PROVIDER_HELPER_FAILED")
        require(not response["body"].get("unexpected"), "UNEXPECTED_PROVIDER_OPERATION")
        return response["body"]

    def probe(kind, path, expected=200):
        value = request(PORTS[kind], path)
        require(value["status"] == expected and "no-store" in value["cache"], "PROBE_CONTRACT")
        return value["body"]

    until(lambda: request(18090, "/_state")["status"] == 200, 15, "HELPER_START")
    pg = docker.create(images["postgres"], [*shared, "--memory=1g", "--memory-swap=1g", "--cpus=2", "--pids-limit=256",
        "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=32m,mode=1777",
        "--tmpfs=/var/run/postgresql:rw,nosuid,nodev,noexec,size=16m,mode=1777",
        "--tmpfs=/var/lib/postgresql/data:rw,nosuid,nodev,noexec,size=512m",
        *env_args(dict(POSTGRES_USER="postgres", POSTGRES_PASSWORD=SECRET, POSTGRES_DB="postgres"))],
        ["postgres", "-c", "shared_preload_libraries=pg_ttl_index"])

    def sql(database, query):
        return docker.call("exec", "-i", pg, "psql", "-X", "-v", "ON_ERROR_STOP=1", "-At",
                           "-U", "postgres", "-d", database, body=query, seconds=15).strip()

    until(lambda: docker.call("exec", pg, "pg_isready", "-h", "127.0.0.1", "-U", "postgres", check=False).returncode == 0, 45, "POSTGRES_START")
    require(160000 <= int(sql("postgres", "SHOW server_version_num;")) < 170000, "POSTGRES_VERSION")
    for database in ("smoke_business", "smoke_crawler", "smoke_empty"):
        sql("postgres", f"CREATE DATABASE {database} TEMPLATE template0;")
    sql("smoke_business", "CREATE EXTENSION pg_ttl_index WITH SCHEMA public; SELECT ttl_start_worker();")
    require(sql("smoke_business", "SELECT extversion FROM pg_extension WHERE extname='pg_ttl_index';") == "3.0.0", "TTL_VERSION")
    history_query = "SELECT version||':'||encode(checksum,'hex')||':'||success::text FROM public._sqlx_migrations ORDER BY version;"
    histories = {}
    for target in ("business", "crawler"):
        key = "BUSINESS_DATABASE_URL" if target == "business" else "LOCAL_DB_URL"
        setup = env_args({"STAGE": "test", "POSTGRES_SSL_MODE": "disable",
                          key: f"postgres://postgres:{SECRET}@127.0.0.1:5432/smoke_{target}"})
        for mode, expected in (("--initialize-fresh", "INITIALIZED_"), ("--verify", "VERIFIED_")):
            output = docker.call("exec", *setup, helper, "/usr/local/bin/bootstrap-local", mode, target, seconds=100, check=False)
            require(output.returncode == 0 and output.stdout.strip() == expected + target.upper(), "BOOTSTRAP_FAILED_OR_UNKNOWN_NO_RETRY")
        sources = root / ("migrations" if target == "business" else "src/crawler/migrations")
        expected = "\n".join(f"{p.name.split('_')[0]}:{hashlib.sha384(p.read_bytes()).hexdigest()}:true"
                             for p in sorted(sources.glob("*.sql")))
        require(expected == sql("smoke_" + target, history_query), "GENUINE_SQLX_HISTORY")
        histories[target] = expected
    sql("postgres", f"CREATE ROLE smoke_runtime LOGIN PASSWORD '{SECRET}' NOSUPERUSER NOCREATEDB NOCREATEROLE;")
    for database in ("smoke_business", "smoke_crawler", "smoke_empty"):
        sql(database, "GRANT USAGE ON SCHEMA public TO smoke_runtime; GRANT SELECT ON ALL TABLES IN SCHEMA public TO smoke_runtime;")
    sql("smoke_crawler", "GRANT UPDATE(crawl_enabled, updated) ON listing_sources TO smoke_runtime;")
    search = docker.create(images["opensearch"], [*shared, "--memory=2g", "--memory-swap=2g", "--cpus=2", "--pids-limit=512",
        "--cap-drop=ALL", "--tmpfs=/tmp:rw,nosuid,nodev,size=128m,mode=1777",
        "--tmpfs=/usr/share/opensearch/data:rw,nosuid,nodev,noexec,size=512m,uid=1000,gid=1000",
        "--tmpfs=/usr/share/opensearch/logs:rw,nosuid,nodev,noexec,size=32m,uid=1000,gid=1000",
        "--mount", f"type=bind,src={root / 'opensearch/analysis'},dst=/usr/share/opensearch/config/analysis,readonly",
        "--ulimit=nofile=65536:65536", *env_args({"discovery.type": "single-node", "DISABLE_SECURITY_PLUGIN": "true",
        "DISABLE_INSTALL_DEMO_CONFIG": "true", "OPENSEARCH_JAVA_OPTS": "-Xms512m -Xmx512m"})])
    until(lambda: request(9200, "/")["status"] == 200, 120, "OPENSEARCH_START")
    version = request(9200, "/")["body"]["version"]
    require(version["distribution"] == "opensearch" and version["number"] == "3.1.0", "OPENSEARCH_VERSION")
    pipeline = {"phase_results_processors": [{"score-ranker-processor": {"combination": {"technique": "rrf"}}}]}
    require(request(9200, "/_search/pipeline/hybrid-search-pipeline", "PUT", pipeline)["status"] == 200, "SEARCH_PIPELINE")
    for filename, index in (("product_listings.json", "product-listings"), ("user_search_filters.json", "user_search_filters")):
        mapping = json.loads((root / "opensearch/mappings" / filename).read_text())
        require(request(9200, "/" + index, "PUT", mapping)["status"] == 200, "SEARCH_MAPPING")
        actual_mapping = request(9200, "/" + index + "/_mapping")["body"][index]["mappings"]
        if actual_mapping != mapping_readback(mapping["mappings"]):
            # Fresh fixture definitions only: no documents, secrets or provider error bodies.
            difference = difflib.unified_diff(json.dumps(mapping_readback(mapping["mappings"]), indent=2, sort_keys=True).splitlines(),
                                              json.dumps(actual_mapping, indent=2, sort_keys=True).splitlines(), n=0)
            print("MAPPING_DEFINITION_DIFF " + index + "\n" + "\n".join(itertools.islice(difference, 30)), flush=True)
            raise Failure("SEARCH_MAPPING_READBACK")
        result = request(9200, "/" + index + "/_search", "POST", {"query": {"match_all": {}}})
        require(result["status"] == 200 and result["body"]["hits"]["total"]["value"] == 0, "SEARCH_EMPTY_QUERY")
    require(request(9200, "/_search/pipeline/hybrid-search-pipeline")["body"]["hybrid-search-pipeline"] == pipeline, "PIPELINE_READBACK")
    print("PASS isolated fixtures and genuine SQLx/search definitions", flush=True)

    def start(kind, env, command=()):
        return docker.create(images[kind], [*shared, "--read-only", "--cap-drop=ALL", "--pids-limit=256",
            "--memory=1g", "--memory-swap=1g", "--cpus=2", "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=64m,mode=1777", "--mount", f"type=bind,src={adc},dst=/smoke-adc.json,readonly",
            *env_args(env)], command)

    def logs(identifier):
        text = docker.call("logs", identifier)  # Only the retained 1 MiB, not lifetime coverage.
        safe_output(text)
        return [json.loads(line) for line in text.splitlines() if line.startswith("{")]

    def absent(kind):
        for port in [PORTS[kind]] + ({"api": [8080], "crawler": [7878]}.get(kind, [])):
            require(request(port, "/health").get("error") == "refused", "LISTENER_ABSENCE_UNCONFIRMED")

    def finished(identifier, expected, seconds, negative=None, preflight=None):
        def exited():
            if preflight:
                absent(preflight)
            if negative:
                require(request(PORTS[negative], "/ready")["status"] not in (200, 204), "NEGATIVE_READINESS_OBSERVED")
                if negative == "api":
                    require(request(8080, "/health").get("error") == "refused", "NEGATIVE_PUBLIC_START")
            return not docker.state(identifier)["Running"]
        until(exited, seconds, "PROCESS_EXIT_DEADLINE")
        state = docker.state(identifier)
        require(state["ExitCode"] == expected and not state["OOMKilled"], "PROCESS_EXIT_STATUS")
        records = logs(identifier)
        require(expected != 0 or not any(record.get("level") == "ERROR" for record in records), "UNEXPECTED_RUNTIME_ERROR")
        if negative:
            require(not any(record.get("fields", {}).get("state") in ("READY", "RUNNING") for record in records)
                    and not normalization_completed(records), "NEGATIVE_RETAINED_START_EVENT")
        docker.remove(identifier)
        return records

    cases = [("api", None), ("worker", SCOPES[0]), ("worker", SCOPES[1]), ("cron", None), ("crawler", None)]
    for kind, scope in cases:
        env = environment(kind, scope)
        label = scope or kind
        before = stats()
        preflight = start(kind, env, ["--check-config"])
        finished(preflight, 0, 70, preflight=kind)
        no_attempts(before, stats())
        absent(kind)
        for sig in ("SIGTERM", "SIGINT"):
            before = stats()
            app = start(kind, env)
            def ready():
                require(docker.state(app)["Running"], "EXIT_BEFORE_READY")
                stats()
                return request(PORTS[kind], "/ready")["status"] == (204 if kind == "api" else 200)
            until(ready, 75, "READY_DEADLINE")
            expected_identity = identity(kind, scope)
            require(probe(kind, "/health") == (expected_identity if kind == "crawler" else "ok\n"), "HEALTH_BODY")
            require(probe(kind, "/ready", 204 if kind == "api" else 200)
                    == ("" if kind == "api" else expected_identity if kind == "crawler" else "ready\n"), "READY_BODY")
            actual_identity = probe(kind, "/ops/version" if kind in ("cron", "crawler") else "/version")
            require(set(actual_identity) == set(expected_identity)
                    and fields_match(actual_identity, expected_identity), "OPERATIONAL_IDENTITY")
            process = docker.call("exec", app, "cat", "/proc/1/status")
            require(re.search(r"^Uid:\s+10001\s+10001\s+10001\s+10001$", process, re.M), "PID1_NONROOT")
            libraries = docker.call("exec", app, "ldd", "/usr/local/bin/" + BINS[kind])
            require("not found" not in libraries, "RUNTIME_LIBRARIES")
            if kind == "worker":
                require(probe(kind, "/admission") == "accepting\n", "ADMISSION_BODY")
                expected_state = dict(schema_version=1, lifecycle="RUNNING", ingress_admission=True,
                    consumer_live=True, consumer_ready=True, identity=expected_identity,
                    budgets=dict(drain_seconds=270, external_stop_seconds=300,
                                 execution_seconds=45 if scope == SCOPES[0] else 240, http_seconds=20))
                require(probe(kind, "/state") == expected_state, "WORKER_STATE")
                until(lambda: stats().get("receive_complete", 0) > before.get("receive_complete", 0), 30, "REAL_EMPTY_POLL")
                if scope == SCOPES[1]:
                    require(normalization_completed(logs(app)), "NORMALIZER_RECONCILIATION")
            else:
                until(lambda: stats().get("token_complete", 0) >= before.get("token_complete", 0) + (3 if kind == "crawler" else 1), 15, "ADC_REFRESH")
                if kind == "cron":
                    require(probe(kind, "/ops/status") == dict(expected_identity, state="ready", accepting=True,
                        active_executions=0, drain_seconds=300, stop_seconds=330, execution_seconds=7200,
                        cleanup_seconds=5, runtime_teardown_seconds=1), "CRON_IDLE_IDENTITY_BUDGETS")
                if kind == "api":
                    for path in ("/health", "/ready", "/version"):
                        require(request(8080, path)["status"] == 404, "PUBLIC_PROBE_LEAK")
                if kind == "crawler":
                    require(request(7878, "/health")["status"] == 200, "REVIEW_LISTENER")
            time.sleep(2)
            require(ready(), "READY_NOT_STABLE")
            docker.call("kill", "--signal=" + sig, app)
            finished(app, 0, {"api": 60, "worker": 300, "cron": 330, "crawler": 330}[kind])
            absent(kind)
            until(lambda: sql("postgres", "SELECT count(*) FROM pg_stat_activity WHERE usename='smoke_runtime';") == "0", 5, "PG_SESSIONS_RETAINED")
            until(lambda: not stats().get("active"), 25, "PROVIDER_POLL_NOT_FINISHED")
            print(f"PASS {label} READY/source/nonroot/libs/{sig}", flush=True)
        bad = dict(env, POSTGRES_DATABASE="smoke_empty")
        if kind == "crawler":
            bad["LOCAL_DB_URL"] = f"postgres://smoke_runtime:{SECRET}@127.0.0.1:5432/smoke_empty"
        before = stats()
        finished(start(kind, bad), 1, 70, negative=kind)
        no_attempts(before, stats())
        if kind in ("api", "worker"):
            fault = dict(bad_jwks=kind == "api", bad_queue=kind == "worker")
            acknowledgement = request(18090, "/_fault", "PUT", fault)
            require(acknowledgement["status"] == 200 and acknowledgement["body"] == fault, "FAULT_NOT_ACKNOWLEDGED")
            before = stats()
            records = finished(start(kind, env), 1, 70, negative=kind)
            after = stats()
            no_attempts(before, after)
            witness = "bad_jwks" if kind == "api" else "bad_source_queue"
            require(after.get(witness, 0) > before.get(witness, 0), "FAULT_REQUEST_NOT_WITNESSED")
            category = {"reason": "STARTUP_OR_PREFLIGHT_FAILED"} if kind == "api" else {"category": "SQS_CONTRACT_OR_DEPENDENCY"}
            require(any(record.get("level") == "ERROR" and fields_match(record.get("fields", {}), category)
                        for record in records), "EXPECTED_SAFE_FAILURE_CATEGORY")
            acknowledgement = request(18090, "/_fault", "PUT", {})
            require(acknowledgement["status"] == 200 and acknowledgement["body"] == dict(bad_queue=False, bad_jwks=False), "FAULT_RESET_NOT_ACKNOWLEDGED")
        if kind != "worker":
            before = stats()
            finished(start(kind, dict(env, GOOGLE_APPLICATION_CREDENTIALS="/missing-smoke-adc")), 1, 70, negative=kind)
            no_attempts(before, stats())
        absent(kind)
        print(f"PASS {label} startup negatives: readiness not observed; no token/receive attempts", flush=True)
    stats()
    for target, history in histories.items():
        require(sql("smoke_" + target, history_query) == history, "RUNTIME_CHANGED_HISTORY")
        require(sql("smoke_" + target, "SELECT count(*) FROM listing_sources;") == "0", "IDLE_SOURCE_MUTATION")
    require(sql("smoke_empty", "SELECT count(*) FROM pg_tables WHERE schemaname='public';") == "0", "RUNTIME_CREATED_SCHEMA")
    require(docker.state(pg)["Running"] and docker.state(search)["Running"], "FIXTURE_DIED")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in (*BINS, "helper", "opensearch"):
        parser.add_argument("--" + key, required=True, help="already cached full sha256 image ID")
    parser.add_argument("--postgres", default=PG_IMAGE, help="must equal the approved cached PG16/TTL3 image pin")
    parser.add_argument("--opensearch-digest", required=True,
                        help="integrator-verified official 3.1.0 opensearchproject/opensearch@sha256 digest")
    args = parser.parse_args()
    require(sys.platform == "linux" and Path("/var/run/docker.sock").is_socket(), "LOCAL_LINUX_DOCKER_REQUIRED")
    def interrupted(_signal, _frame):
        raise Failure("INTERRUPTED")
    for sig in SIGNALS:
        signal.signal(sig, interrupted)
    signal.alarm(1800)
    with tempfile.TemporaryDirectory(prefix="aura-r2-smoke-") as name:
        directory = Path(name)
        docker = Docker(directory)
        try:
            run(args, directory, docker)
        finally:
            signal.alarm(0)
            for sig in SIGNALS:
                signal.signal(sig, signal.SIG_IGN)
            for identifier in list(reversed(docker.owned)):
                try:
                    docker.remove(identifier)
                except Exception:
                    pass  # remove retained the exact authorized ID; do not retry.
            if docker.recovery:
                for recovery in docker.recovery:
                    print("MANUAL_RECOVERY " + recovery, file=sys.stderr, flush=True)
                raise Failure("DOCKER_UNCONFIRMED_NOT_RESOLVED")
    print("PASS four native images; isolated real stores; provider doubles only; owned cleanup confirmed; retained-log canary only")


if __name__ == "__main__":
    try:
        if sys.argv[1:] == ["_serve"]:
            require([name for _, name in socket.if_nameindex()] == ["lo"], "LOOPBACK_ONLY_REQUIRED")
            ProviderServer(("127.0.0.1", 18090), Provider).serve_forever()
        elif sys.argv[1:2] == ["_http"]:
            print(json.dumps(http_request(int(sys.argv[2]), sys.argv[3], sys.argv[4], sys.stdin.read(65537))))
        else:
            main()
    except Failure as error:
        print("FAIL " + str(error), file=sys.stderr)
        sys.exit(1)
    except Exception:
        print("FAIL INTERNAL (details suppressed)", file=sys.stderr)
        sys.exit(1)
