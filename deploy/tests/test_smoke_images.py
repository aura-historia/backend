"""Pure regressions only: no Docker, sockets, or provider operations."""
import collections
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("smoke", Path(__file__).with_name("smoke-images.py"))
s = importlib.util.module_from_spec(spec)
spec.loader.exec_module(s)

class SmokeRegressions(unittest.TestCase):
    def test_create_and_cleanup_uncertainty_retain_authority_without_retry(self):
        for result in (s.Failure("DOCKER_COMMAND_UNCONFIRMED"), s.Failure("DOCKER_COMMAND_FAILED"), "", "f" * 64):
            with self.subTest(result=type(result).__name__), tempfile.TemporaryDirectory() as directory:
                docker = s.Docker(Path(directory))
                docker.call = Mock(side_effect=result) if isinstance(result, Exception) else Mock(return_value=result)
                with self.assertRaises((s.Failure, FileNotFoundError)):
                    docker.create("image", [])
                args = docker.call.call_args_list[0].args
                self.assertIn("--cidfile", args)
                self.assertIn("--no-healthcheck", args)
                self.assertIn(args[args.index("--name") + 1], docker.recovery[0])
                self.assertIn(args[args.index("--label") + 1], docker.recovery[0])
                self.assertEqual([], docker.owned)
                self.assertEqual(1, docker.call.call_count)
        with tempfile.TemporaryDirectory() as directory:
            docker = s.Docker(Path(directory)); identifier = "a" * 64
            docker.owned.append(identifier)
            docker.call = Mock(side_effect=[identifier, s.Failure("DOCKER_COMMAND_UNCONFIRMED")])
            with self.assertRaises(s.Failure): docker.remove(identifier)
            with self.assertRaises(s.Failure): docker.remove(identifier)
            self.assertEqual(2, docker.call.call_count)
            self.assertIn(identifier, docker.recovery[0])

    def test_attempts_precede_body_io_and_response_completion(self):
        handler = object.__new__(s.Provider)
        handler.command = "POST"; handler.headers = {"Content-Length": "1"}
        handler.path = "/token"; handler.rfile = Mock(); handler.rfile.read.side_effect = TimeoutError
        s.Provider.counts = collections.Counter()
        handler.handle_request()
        self.assertEqual(1, s.Provider.counts["token_attempt"])
        for scope, visibility in zip(s.SCOPES, (60, 300)):
            for bad in (None, "MessageSystemAttributeNames", "VisibilityTimeout"):
                handler.path = "/"; handler.headers = {"x-amz-target": "AmazonSQS.ReceiveMessage", "Content-Length": "1"}
                body = dict(QueueUrl=f"{s.ENDPOINT}/000000000000/aura-worker-{scope}-test", WaitTimeSeconds=20,
                            MaxNumberOfMessages=1, VisibilityTimeout=visibility, MessageSystemAttributeNames=s.SYSTEM_ATTRIBUTES)
                if bad: body[bad] = "invalid"
                handler.rfile = io.BytesIO(json.dumps(body).encode()); handler.headers["Content-Length"] = str(len(handler.rfile.getvalue()))
                handler.reply = Mock(side_effect=BrokenPipeError)
                with patch.object(s.time, "sleep"): handler.handle_request()
                self.assertEqual(not bad, handler.reply.called)
        self.assertEqual(6, s.Provider.counts["receive_attempt"])
        self.assertEqual(0, s.Provider.counts["receive_complete"])
        with self.assertRaises(s.Failure): s.no_attempts({}, dict(token_attempt=1))
        with self.assertRaises(s.Failure): s.no_attempts({}, dict(receive_attempt=1))

    def test_only_connection_refusal_proves_absence(self):
        for error in (ConnectionRefusedError(), TimeoutError(), ConnectionResetError(), s.http.client.BadStatusLine("bad")):
            with patch.object(s.http.client, "HTTPConnection") as connection:
                connection.return_value.request.side_effect = error
                if isinstance(error, ConnectionRefusedError):
                    self.assertEqual("refused", s.http_request(9080, "/health", "GET", "")["error"])
                else:
                    with self.assertRaises(s.Failure): s.http_request(9080, "/health", "GET", "")

    def test_pins_source_and_environment_values_are_checked(self):
        image = "sha256:" + "a" * 64
        config = dict(User="10001:10001", Entrypoint=["/usr/bin/python3"], Env=[f"{k}={v}" for k, v in s.TRUSTED_ENV.items()],
                      Labels={"org.opencontainers.image.source": s.SOURCE, "org.opencontainers.image.revision": s.SHA})
        metadata = dict(Id=image, Os="linux", Architecture="amd64", Config=config)
        s.verify_image("helper", image, metadata, "")
        for key in ("PATH", "HOME", "COMMIT_SHA"):
            with patch.dict(config, Env=[f"{k}={('bad' if k == key else v)}" for k, v in s.TRUSTED_ENV.items()]):
                with self.assertRaises(s.Failure): s.verify_image("helper", image, metadata, "")
        with patch.dict(config, Labels={}):
            with self.assertRaises(s.Failure): s.verify_image("helper", image, metadata, "")
        for key in ("postgres", "opensearch"):
            with self.assertRaises(s.Failure): s.verify_image(key, image, metadata, "")

    def test_mapping_readback_only_omits_redundant_object_type(self):
        mapping = dict(properties=dict(obj=dict(type="object", properties=dict(value=dict(type="keyword"))),
                                       nested=dict(type="nested", properties=dict(value=dict(type="text")))))
        result = s.mapping_readback(mapping)
        self.assertNotIn("type", result["properties"]["obj"])
        self.assertEqual("object", mapping["properties"]["obj"]["type"])
        self.assertEqual(mapping["properties"]["nested"], result["properties"]["nested"])
        self.assertEqual(dict(type="object"), s.mapping_readback(dict(type="object")))
        self.assertEqual(dict(properties={}), s.mapping_readback(dict(type="object", enabled=True, properties={})))
        self.assertEqual(dict(enabled=False, properties={}), s.mapping_readback(dict(type="object", enabled=False, properties={})))
        self.assertEqual(dict(type="nested", enabled=True, properties={}), s.mapping_readback(dict(type="nested", enabled=True, properties={})))

    def test_structured_identity_completion_and_canaries(self):

        self.assertFalse(s.fields_match(dict(schema_version=True), dict(schema_version=1)))
        self.assertFalse(s.normalization_completed([dict(level="INFO", fields=dict(message="raw normalization reconciliation turn completed"))]))
        fields = dict.fromkeys(("processed_revisions", "normalization_failures", "pending_stream_page_count",
            "reconciliation_continuation_stream_count", "unscheduled_continuation_count", "suppressed_continuation_count"), 0)
        fields.update(metric="product_listing_raw_normalization_reconciliation", job_type="product_listing_raw_normalization_reconciliation",
            outcome="completed", reconciliation_runs=1, reconciliation_page="global_initial", pending_stream_cursor_present=False)
        self.assertTrue(s.normalization_completed([dict(level="INFO", fields=fields)]))
        fields["normalization_failures"] = 1
        self.assertFalse(s.normalization_completed([dict(level="INFO", fields=fields)]))
        for canary in (s.SECRET, "postgres://", "postgresql://"):
            with self.assertRaises(s.Failure): s.safe_output(json.dumps(dict(body=canary)))

if __name__ == "__main__":
    unittest.main()
