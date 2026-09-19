"""Pure, serial R3 regressions: no sockets, Docker, builds, cloud, or stateful fixtures."""
import collections
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import re
import threading
import unittest
from unittest.mock import Mock, patch


spec = importlib.util.spec_from_file_location("compose_provider", Path(__file__).with_name("compose-provider.py"))
p = importlib.util.module_from_spec(spec)
spec.loader.exec_module(p)
ROOT = Path(__file__).resolve().parents[2]


def queue_url(scope, dlq=False):
    return f"{p.ENDPOINT}/000000000000/aura-worker-{scope}{'-dlq' if dlq else ''}-test"


def receive(scope, dlq=False):
    return dict(QueueUrl=queue_url(scope, dlq), WaitTimeSeconds=20,
                MaxNumberOfMessages=1, VisibilityTimeout=p.SCOPES[scope],
                MessageSystemAttributeNames=p.r2.SYSTEM_ATTRIBUTES.copy())


def handler(operation="", data=None, method="POST", path="/", raw=None, headers=None):
    value = object.__new__(p.Provider)
    value.command = method
    value.path = path
    raw = json.dumps(data).encode() if raw is None and data is not None else (raw or b"")
    value.headers = {"x-amz-target": operation, "Content-Length": str(len(raw))}
    value.headers.update(headers or {})
    value.rfile = io.BytesIO(raw)
    value.reply = Mock()
    value.close_connection = False
    return value


class ComposeProviderTests(unittest.TestCase):
    def setUp(self):
        p.Provider.counts = collections.Counter()

    def reject(self, value):
        before = p.Provider.counts["unexpected"]
        with patch.object(p.time, "sleep") as sleep:
            value.handle_request()
        sleep.assert_not_called()
        value.reply.assert_not_called()
        self.assertTrue(value.close_connection)
        self.assertEqual(before + 1, p.Provider.counts["unexpected"])

    def test_catalog_and_runtime_scope_visibility_agreement(self):
        catalog = json.loads((ROOT / "deploy/catalog.json").read_text())
        self.assertEqual(10, len(catalog["workers"]))
        self.assertEqual(set(p.SCOPES), {row["scope"] for row in catalog["workers"]})
        self.assertEqual(set(p.r2.BINS.values()), {row["binary"] for row in catalog["native"]})
        runtime = (ROOT / "src/aura-historia-worker/src/lib.rs").read_text()
        scope_block = runtime.split("impl WorkerScope {", 1)[1].split("pub(crate) fn from_getter", 1)[0]
        names = dict(re.findall(r'Self::(\w+) => "([a-z-]+)"', scope_block))
        config = (ROOT / "src/aura-historia-worker/src/queue/config.rs").read_text()
        visibility = config.split("pub(super) fn visibility(", 1)[1].split("pub(super) fn execution_budget", 1)[0]
        actual = {}
        for variants, seconds in re.findall(r"((?:\s*WorkerScope::\w+\s*\|?)+)\s*=>\s*(\d+)", visibility):
            for variant in re.findall(r"WorkerScope::(\w+)", variants):
                actual[names[variant]] = int(seconds)
        self.assertEqual(p.SCOPES, actual)
        self.assertEqual(collections.Counter({60: 5, 300: 4, 360: 1}), collections.Counter(actual.values()))

    def test_all_twenty_queue_attributes_exact_and_completed_per_scope(self):
        self.assertEqual(20, len(p.QUEUES))
        for scope, seconds in p.SCOPES.items():
            for dlq in (False, True):
                with self.subTest(scope=scope, dlq=dlq):
                    arn = f"arn:aws:sqs:eu-central-1:000000000000:aura-worker-{scope}-test"
                    dead_arn = f"arn:aws:sqs:eu-central-1:000000000000:aura-worker-{scope}-dlq-test"
                    expected = dict(QueueArn=dead_arn if dlq else arn, FifoQueue="false",
                                    SqsManagedSseEnabled="true", VisibilityTimeout=str(seconds),
                                    MessageRetentionPeriod="1209600" if dlq else "604800",
                                    ReceiveMessageWaitTimeSeconds="20",
                                    Policy={"Statement": [{"Effect": "Deny", "Principal": "*", "Action": "sqs:*",
                                        "Resource": dead_arn if dlq else arn,
                                        "Condition": {"Bool": {"aws:SecureTransport": "false"}}}]},
                                    RedriveAllowPolicy={"redrivePermission": "byQueue", "sourceQueueArns": [arn]}
                                        if dlq else {"redrivePermission": "denyAll"})
                    if not dlq:
                        expected["RedrivePolicy"] = dict(deadLetterTargetArn=dead_arn, maxReceiveCount=5)
                    value = handler("AmazonSQS.GetQueueAttributes", dict(QueueUrl=queue_url(scope, dlq), AttributeNames=["All"]))
                    event = "attributes_dlq" if dlq else "attributes_source"

                    def reply(status, body):
                        self.assertEqual(200, status)
                        self.assertEqual(1, p.Provider.counts[f"{scope}.{event}_attempt"])
                        self.assertEqual(0, p.Provider.counts[f"{scope}.{event}_complete"])
                        attrs = body["Attributes"].copy()
                        for key in ("Policy", "RedrivePolicy", "RedriveAllowPolicy"):
                            if key in attrs:
                                attrs[key] = json.loads(attrs[key])
                        self.assertEqual(expected, attrs)

                    value.reply.side_effect = reply
                    value.handle_request()
                    value.reply.assert_called_once()
                    self.assertEqual(1, p.Provider.counts[f"{scope}.{event}_complete"])
        self.assertEqual(20, p.Provider.counts["attributes_attempt"])
        self.assertEqual(20, p.Provider.counts["attributes_complete"])
        self.assertEqual(0, p.Provider.counts["unexpected"])

    def test_attribute_requests_strict_for_every_queue(self):
        for url, (scope, dlq) in p.QUEUES.items():
            valid = dict(QueueUrl=url, AttributeNames=["All"])
            invalid = [{key: value for key, value in valid.items() if key != missing} for missing in valid]
            invalid += [dict(valid, AttributeNames=value) for value in ([], ["QueueArn"], "All", ["All", "All"], None)]
            invalid += [dict(valid, extra="not-allowed")]
            for data in invalid:
                with self.subTest(scope=scope, dlq=dlq, data=data):
                    self.reject(handler("AmazonSQS.GetQueueAttributes", data))
        self.assertEqual(0, p.Provider.counts["attributes_complete"])

    def test_all_scopes_empty_receive_waits_twenty_and_counts_completion_after_write(self):
        for scope in p.SCOPES:
            with self.subTest(scope=scope):
                value = handler("AmazonSQS.ReceiveMessage", receive(scope))

                def waiting(seconds):
                    self.assertEqual(20, seconds)
                    self.assertEqual(1, p.Provider.counts[f"{scope}.receive_attempt"])
                    self.assertEqual(1, p.Provider.counts[f"{scope}.active"])
                    self.assertEqual(0, p.Provider.counts[f"{scope}.receive_complete"])
                    value.reply.assert_not_called()

                def reply(status, body):
                    self.assertEqual((200, {}), (status, body))
                    self.assertEqual(0, p.Provider.counts[f"{scope}.receive_complete"])

                value.reply.side_effect = reply
                with patch.object(p.time, "sleep", side_effect=waiting) as sleep:
                    value.handle_request()
                sleep.assert_called_once_with(20)
                self.assertEqual(1, p.Provider.counts[f"{scope}.receive_complete"])
                self.assertEqual(0, p.Provider.counts[f"{scope}.active"])
        self.assertEqual(10, p.Provider.counts["receive_complete"])
        self.assertEqual(0, p.Provider.counts["active"])
        self.assertEqual(0, p.Provider.counts["unexpected"])

    def test_receive_validation_all_scopes_including_dlqs_and_python_numeric_coercion(self):
        for scope in p.SCOPES:
            valid = receive(scope)
            invalid = [{key: value for key, value in valid.items() if key != missing} for missing in valid]
            invalid += [dict(valid, **{key: bad}) for key in ("WaitTimeSeconds", "MaxNumberOfMessages", "VisibilityTimeout")
                        for bad in (None, True, False, 0, -1, "20", float(valid[key]), valid[key] + 1)]
            invalid += [dict(valid, MessageSystemAttributeNames=bad)
                        for bad in (None, [], ["All"], list(reversed(p.r2.SYSTEM_ATTRIBUTES)), p.r2.SYSTEM_ATTRIBUTES + ["SenderId"])]
            invalid += [dict(valid, MessageAttributeNames=["All"]), receive(scope, True)]
            for data in invalid:
                with self.subTest(scope=scope, data=data):
                    self.reject(handler("AmazonSQS.ReceiveMessage", data))
            self.assertGreater(p.Provider.counts[f"{scope}.receive_attempt"], 0)
            self.assertEqual(0, p.Provider.counts[f"{scope}.receive_complete"])

    def test_mutations_and_unknown_operations_never_succeed_for_any_queue(self):
        operations = ("SendMessage", "SendMessageBatch", "DeleteMessage", "DeleteMessageBatch",
                      "ChangeMessageVisibility", "ChangeMessageVisibilityBatch", "SetQueueAttributes",
                      "CreateQueue", "DeleteQueue", "PurgeQueue", "ListQueues", "GetQueueUrl", "Unknown")
        for url, (scope, dlq) in p.QUEUES.items():
            for operation in operations:
                with self.subTest(scope=scope, dlq=dlq, operation=operation):
                    self.reject(handler("AmazonSQS." + operation, dict(QueueUrl=url)))
            self.assertGreaterEqual(p.Provider.counts[f"{scope}.forbidden_attempt"], len(operations))
        self.assertEqual(20 * len(operations), p.Provider.counts["forbidden_attempt"])

    def test_unknown_queue_identity_and_malformed_http_fail_without_echo_or_io(self):
        scope = next(iter(p.SCOPES))
        url = queue_url(scope)
        invalid_urls = [url + "/", url + "?x=1", url + "#fragment", url.replace("-test", "-prod"),
                        url.replace("000000000000", "123456789012"), url.replace("provider", "localhost"),
                        url.replace("http:", "https:"), url.replace(scope, "unknown"), None, [], {}]
        for bad in invalid_urls:
            self.reject(handler("AmazonSQS.ReceiveMessage", dict(receive(scope), QueueUrl=bad)))
        for method, path in (("GET", "/"), ("POST", "/wrong"), ("PUT", "/_fault"), ("CONNECT", "provider:443")):
            self.reject(handler("AmazonSQS.ReceiveMessage", receive(scope), method, path))
        for path in ("/inference", "/zoho-accounts", "/mail", "/s3", "/_fault", "http://example.test/"):
            self.reject(handler(path=path))
        for raw, headers in ((b"{", {}), (b"[]", {}), (b"null", {}), (b"\xff", {}),
                             (b"{}", {"Content-Length": "-1"}), (b"{}", {"Content-Length": "16385"}),
                             (b"{}", {"Content-Length": "bad"}), (b"{}", {"Content-Length": "3"}),
                             (b"{}", {"Transfer-Encoding": "chunked"})):
            self.reject(handler("AmazonSQS.ReceiveMessage", raw=raw, headers=headers))
        output = io.StringIO()
        value = handler()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            value.log_message(p.r2.SECRET)
            value.send_error(500, p.r2.SECRET)
        self.assertEqual("", output.getvalue())
        self.assertTrue(value.close_connection)

    def test_attempts_survive_body_timeout_and_cancelled_responses_never_complete(self):
        for operation, event in (("GetQueueAttributes", "attributes"), ("ReceiveMessage", "receive")):
            value = handler("AmazonSQS." + operation, raw=b"x")
            value.rfile = Mock()
            value.rfile.read.side_effect = TimeoutError
            self.reject(value)
            self.assertEqual(1, p.Provider.counts[event + "_attempt"])
        p.Provider.counts.clear()
        for scope in p.SCOPES:
            for error in (BrokenPipeError, ConnectionResetError, TimeoutError):
                value = handler("AmazonSQS.ReceiveMessage", receive(scope))
                value.reply.side_effect = error
                with patch.object(p.time, "sleep"):
                    value.handle_request()
                value.reply.assert_called_once_with(200, {})
                self.assertEqual(0, p.Provider.counts[f"{scope}.receive_complete"])
                self.assertEqual(0, p.Provider.counts[f"{scope}.active"])
            for dlq in (False, True):
                value = handler("AmazonSQS.GetQueueAttributes", dict(QueueUrl=queue_url(scope, dlq), AttributeNames=["All"]))
                value.reply.side_effect = BrokenPipeError
                value.handle_request()
                self.assertEqual(0, p.Provider.counts[f"{scope}.attributes_{'dlq' if dlq else 'source'}_complete"])
        self.assertEqual(0, p.Provider.counts["unexpected"])
        self.assertEqual(0, p.Provider.counts["active"])

    def test_r2_adc_jwks_and_state_reused_without_mutating_r2(self):
        original = p.r2.Provider.counts.copy()
        adc = p.fixture_adc()
        self.assertEqual(dict(type="authorized_user", client_id="smoke-client", client_secret=p.r2.SECRET,
                              refresh_token=p.r2.SECRET, token_uri=p.ENDPOINT + "/token"), adc)
        data = {key: adc[key] for key in ("client_id", "client_secret", "refresh_token")}
        data.update(grant_type="refresh_token", scopes="https://www.googleapis.com/auth/cloud-platform")
        token = handler(data=data, path="/token")
        token.handle_request()
        token.reply.assert_called_once_with(200, dict(access_token=p.r2.SECRET, token_type="Bearer", expires_in=3600))
        self.assertEqual(1, p.Provider.counts["token_attempt"])
        self.assertEqual(1, p.Provider.counts["token_complete"])
        self.reject(handler(data=dict(data, grant_type="client_credentials"), path="/token"))
        for path in ("/jwks", "/_state"):
            self.reject(handler(method="GET", path=path, data={"unexpected": "body"}))
            self.reject(handler(method="GET", path=path, headers={"Transfer-Encoding": "chunked"}))
        jwks = handler(method="GET", path="/jwks")
        jwks.handle_request()
        jwks.reply.assert_called_once_with(200, {"keys": [{"kid": "smoke", "alg": "RS256", "n": "AQAB", "e": "AQAB"}]})
        state = handler(method="GET", path="/_state")
        state.handle_request()
        state.reply.assert_called_once_with(200, dict(p.Provider.counts))
        self.assertGreater(state.reply.call_args.args[1]["unexpected"], 0)
        self.assertNotIn(p.r2.SECRET, json.dumps(state.reply.call_args.args))
        self.assertEqual(original, p.r2.Provider.counts)
        self.assertEqual("http://127.0.0.1:18090", p.r2.ENDPOINT)
        self.assertEqual(("product-listing-opensearch", "product-listing-normalization"), p.r2.SCOPES)
        self.assertIs(p.Provider.reply, p.r2.Provider.reply)
        self.assertIs(p.Provider.setup, p.r2.Provider.setup)

    def test_environment_and_minimal_application_mounts_for_every_kind_and_scope(self):
        cases = [(kind, None) for kind in ("api", "cron", "crawler")] + [("worker", scope) for scope in p.SCOPES]
        google_scopes = {"search-filter-percolator", "product-embedding", "product-translation"}
        search_scopes = {"product-listing-opensearch", "search-filter-projection", "search-filter-percolator"}
        for kind, scope in cases:
            with self.subTest(kind=kind, scope=scope):
                with patch.dict("os.environ", {"POSTGRES_HOST": "real-host", "AWS_SECRET_ACCESS_KEY": "real-secret"}):
                    env = p.fixture_environment(kind, scope)
                self.assertTrue(all(isinstance(value, str) for value in env.values()))
                for key, value in dict(STAGE="test", POSTGRES_SSL_MODE="disable", POSTGRES_HOST="postgres",
                    POSTGRES_PORT="5432", POSTGRES_DATABASE="smoke_business", POSTGRES_USERNAME="smoke_runtime",
                    POSTGRES_PASSWORD=p.r2.SECRET, POSTGRES_MAX_CONNECTIONS="2", AWS_REGION="eu-central-1",
                    AWS_EC2_METADATA_DISABLED="true", AWS_CONFIG_FILE="/dev/null", AWS_SHARED_CREDENTIALS_FILE="/dev/null",
                    AWS_ACCESS_KEY_ID="smoke-key", AWS_SECRET_ACCESS_KEY=p.r2.SECRET,
                    COMMIT_SHA="672bdcefdeaabc6cd9f78461ec1bf859c31dc443").items():
                    self.assertEqual(value, env[key])
                for key in ("POSTGRES_SSL_ROOT_CERT", "PGSSLROOTCERT", "AWS_ENDPOINT_URL", "AWS_PROFILE", "AWS_SESSION_TOKEN"):
                    self.assertNotIn(key, env)
                google = kind != "worker" or scope in google_scopes
                self.assertEqual((p.ADC_PATH,) if google else (), p.fixture_mounts(kind, scope))
                self.assertEqual(google, "GOOGLE_APPLICATION_CREDENTIALS" in env)
                if google:
                    self.assertEqual("/run/aura/google-adc.json", env["GOOGLE_APPLICATION_CREDENTIALS"])
                self.assertEqual(kind in ("api", "cron") or scope in search_scopes, "OPENSEARCH_ENDPOINT_URL" in env)
                if "OPENSEARCH_ENDPOINT_URL" in env:
                    self.assertEqual("http://opensearch:9200", env["OPENSEARCH_ENDPOINT_URL"])
                if kind == "worker":
                    self.assertEqual(scope, env["AURA_HISTORIA_WORKER_SCOPE"])
                    self.assertEqual(queue_url(scope), env["AURA_HISTORIA_WORKER_QUEUE_URL"])
                    self.assertEqual(p.ENDPOINT, env["AWS_ENDPOINT_URL_SQS"])
                    self.assertEqual("0.0.0.0:8081", env["AURA_HISTORIA_WORKER_HEALTH_BIND_ADDR"])
                    self.assertEqual(google, "VERTEX_AI_PROJECT_ID" in env)
                    self.assertEqual(scope in {"search-filter-percolator", "product-translation"}, "VERTEX_AI_MODEL" in env)
                    self.assertEqual(scope == "notification-delivery", "S3_BUCKET_NAME_TEMPLATES" in env)
                self.assertNotIn("127.0.0.1", " ".join(value for key, value in env.items() if not key.endswith("_BIND_ADDR")))
        api = p.fixture_environment("api")
        self.assertEqual("0.0.0.0:8080", api["AURA_HISTORIA_API_BIND_ADDR"])
        self.assertEqual("127.0.0.1:9080", api["AURA_HISTORIA_API_OPERATIONS_BIND_ADDR"])
        self.assertEqual(p.ENDPOINT + "/jwks", api["AURA_HISTORIA_COGNITO_JWKS_URL"])
        self.assertEqual("127.0.0.1:8082", p.fixture_environment("cron")["AURA_HISTORIA_CRON_HEALTH_BIND_ADDR"])
        crawler = p.fixture_environment("crawler")
        self.assertEqual("127.0.0.1:9083", crawler["CRAWLER_OPERATIONS_BIND_ADDR"])
        self.assertEqual("127.0.0.1:7878", crawler["CRAWLER_REVIEW_BIND_ADDR"])
        for key, database in (("LOCAL_DB_URL", "smoke_crawler"), ("BUSINESS_DATABASE_URL", "smoke_business")):
            self.assertEqual(f"postgres://smoke_runtime:{p.r2.SECRET}@postgres:5432/{database}", crawler[key])
        delivery = p.fixture_environment("worker", "notification-delivery")
        self.assertEqual("smoke-templates", delivery["S3_BUCKET_NAME_TEMPLATES"])
        self.assertEqual("no-reply@example.test", delivery["NOTIFICATION_EMAIL_FROM"])
        self.assertEqual("contact@example.test", delivery["NOTIFICATION_EMAIL_REPLY_TO"])
        self.assertEqual(p.ENDPOINT, delivery["AWS_ENDPOINT_URL_S3"])
        self.assertEqual(p.ENDPOINT, delivery["AWS_ENDPOINT_URL_SESV2"])

    def test_unknown_kinds_scopes_and_cli_arguments_fail_closed(self):
        for kind, scope in (("unknown", None), ("aura-historia-api", None), ("worker", None), ("worker", "unknown"),
                            ("api", "product-embedding"), ("cron", ""), ("crawler", "notification-delivery")):
            for function in (p.fixture_environment, p.fixture_mounts):
                with self.subTest(kind=kind, scope=scope), self.assertRaises(p.r2.Failure):
                    function(kind, scope)
        for args in ([], ["_serve"], ["run"], ["serve", "extra"], ["serve", "--port", "1"], ["serve", "--host", "127.0.0.1"]):
            with self.subTest(args=args), patch.object(p, "ProviderServer") as server, contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    p.main(args)
                self.assertEqual(2, error.exception.code)
                server.assert_not_called()
        with patch.object(p, "ProviderServer") as server:
            p.main(["serve"])
            server.assert_called_once_with(("0.0.0.0", 18090), p.Provider)
            server.return_value.__enter__.return_value.serve_forever.assert_called_once_with()

    def test_server_capacity_and_errors_count_against_r3_not_r2(self):
        original = p.r2.Provider.counts.copy()
        server = object.__new__(p.ProviderServer)
        with patch.object(p.http.server.ThreadingHTTPServer, "__init__"):
            p.ProviderServer.__init__(server, p.BIND, p.Provider)
        self.assertEqual(32, server.request_queue_size)
        for _ in range(32):
            self.assertTrue(server.slots.acquire(blocking=False))
        self.assertFalse(server.slots.acquire(blocking=False))
        server.shutdown_request = Mock()
        server.process_request("socket", "address")
        server.shutdown_request.assert_called_once_with("socket")
        self.assertEqual(1, p.Provider.counts["unexpected"])
        server.slots = threading.BoundedSemaphore(1)
        with patch.object(p.r2.http.server.ThreadingHTTPServer, "process_request") as dispatch:
            server.process_request("socket", "address")
        dispatch.assert_called_once_with("socket", "address")
        self.assertFalse(server.slots.acquire(blocking=False))
        with patch.object(p.r2.http.server.ThreadingHTTPServer, "process_request_thread"):
            server.process_request_thread("socket", "address")
        self.assertTrue(server.slots.acquire(blocking=False))
        output = io.StringIO()
        with contextlib.redirect_stderr(output):
            server.handle_error(p.r2.SECRET)
        self.assertEqual("", output.getvalue())
        self.assertEqual(2, p.Provider.counts["unexpected"])
        self.assertEqual(original, p.r2.Provider.counts)


if __name__ == "__main__":
    unittest.main()
