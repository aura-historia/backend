"""Pure contract tests. No sockets, certificates, containers or live mutations."""
import contextlib
import copy
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import ssl
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
loader = importlib.machinery.SourceFileLoader("opensearch_bootstrap", str(ROOT / "deploy/bin/opensearch"))
spec = importlib.util.spec_from_loader(loader.name, loader)
bootstrap = importlib.util.module_from_spec(spec)
loader.exec_module(bootstrap)


def index_response(name, definition):
    return {name: {"mappings": bootstrap.mapping(definition["mappings"]),
                   "settings": bootstrap.settings(definition["settings"]) | {
                       "index.uuid": "server-generated", "index.number_of_shards": "1"},
                   "aliases": {}}}


class Cluster:
    def __init__(self, fresh=False):
        self.cluster = "isolated-test"
        self.calls = []
        definitions, pipeline = bootstrap.assets()
        self.target = {"cluster_name": self.cluster,
                       "version": {"distribution": "opensearch", "number": "3.1.0"}}
        self.values = {} if fresh else {
            "/" + name + "?flat_settings=true": index_response(name, value) for name, value in definitions}
        if not fresh:
            self.values[bootstrap.PIPELINE_PATH] = {bootstrap.PIPELINE: pipeline}

    def request(self, method, path, body=None, missing=None):
        self.calls.append((method, path, copy.deepcopy(body)))
        if path == "/":
            return self.target
        if method == "PUT":
            if path == bootstrap.PIPELINE_PATH:
                self.values[path] = {bootstrap.PIPELINE: body}
                return {"acknowledged": True}
            name = path[1:]
            self.values[path + "?flat_settings=true"] = index_response(name, body)
            return {"acknowledged": True, "index": name}
        if path not in self.values:
            if missing:
                return None
            raise bootstrap.Failure("MISSING")
        return self.values[path]


class FlowTests(unittest.TestCase):
    def run_flow(self, mode, client):
        operation = bootstrap.Bootstrap()
        with patch.object(bootstrap, "Client", return_value=client):
            operation.run(mode)
        return operation

    def test_exact_fresh_sequence_and_local_bodies(self):
        client = Cluster(fresh=True)
        operation = self.run_flow("initialize-fresh", client)
        definitions, pipeline = bootstrap.assets()
        self.assertEqual([(m, p) for m, p, _ in client.calls], [
            ("GET", "/"), ("GET", "/product-listings?flat_settings=true"),
            ("GET", "/user_search_filters?flat_settings=true"), ("GET", bootstrap.PIPELINE_PATH),
            ("PUT", "/product-listings"), ("PUT", "/user_search_filters"),
            ("PUT", bootstrap.PIPELINE_PATH), ("GET", "/product-listings?flat_settings=true"),
            ("GET", "/user_search_filters?flat_settings=true"), ("GET", bootstrap.PIPELINE_PATH)])
        self.assertEqual([body for m, _, body in client.calls if m == "PUT"],
                         [d for _, d in definitions] + [pipeline])
        self.assertTrue(operation.write_attempted)

    def test_verify_only_metadata_gets_and_no_changes(self):
        client = Cluster()
        before = copy.deepcopy(client.values)
        self.assertFalse(self.run_flow("verify", client).write_attempted)
        self.assertEqual(client.values, before)
        self.assertEqual(len(client.calls), 4)
        self.assertTrue(all(m == "GET" and body is None for m, _, body in client.calls))

    def test_each_existing_asset_rejects_before_any_put(self):
        existing = Cluster().values
        for path, value in existing.items():
            client = Cluster(fresh=True)
            client.values[path] = value
            with self.subTest(path=path), self.assertRaisesRegex(bootstrap.Failure, "NOT_FRESH"):
                self.run_flow("initialize-fresh", client)
            self.assertTrue(all(m == "GET" for m, _, _ in client.calls))

    def test_target_cluster_distribution_version_must_match_before_other_calls(self):
        for target in ({"cluster_name": "wrong"},
                       {"version": {"distribution": "other", "number": "3.1.0"}},
                       {"version": {"distribution": "opensearch", "number": "3.2.0"}}):
            client = Cluster(fresh=True)
            client.target.update(target)
            with self.assertRaises(bootstrap.Failure):
                self.run_flow("initialize-fresh", client)
            self.assertEqual(len(client.calls), 1)

    def test_verify_missing_and_pipeline_drift_fail_without_writes(self):
        for path in Cluster().values:
            client = Cluster()
            del client.values[path]
            with self.assertRaises(bootstrap.Failure):
                self.run_flow("verify", client)
            self.assertTrue(all(m == "GET" for m, _, _ in client.calls))
        client = Cluster()
        client.values[bootstrap.PIPELINE_PATH][bootstrap.PIPELINE]["extra"] = True
        with self.assertRaisesRegex(bootstrap.Failure, "PIPELINE_MISMATCH"):
            self.run_flow("verify", client)

    def test_put_failure_stops_without_retry_or_cleanup_and_reports_unknown(self):
        for path in ("/product-listings", "/user_search_filters", bootstrap.PIPELINE_PATH):
            client = Cluster(fresh=True)
            original = client.request
            def fail(method, request_path, *args, **kwargs):
                result = original(method, request_path, *args, **kwargs)
                if method == "PUT" and request_path == path:
                    raise OSError("secret response, endpoint, password")
                return result
            client.request = fail
            output = io.StringIO()
            with patch.object(bootstrap, "Client", return_value=client), contextlib.redirect_stderr(output):
                self.assertEqual(bootstrap.main(["initialize-fresh"]), 1)
            self.assertEqual(client.calls[-1][:2], ("PUT", path))
            self.assertIn("partial-or-unknown-do-not-retry", output.getvalue())
            self.assertIn("phase=create-", output.getvalue())
            self.assertNotIn("secret", output.getvalue())
            with self.assertRaisesRegex(bootstrap.Failure, "NOT_FRESH"):
                self.run_flow("initialize-fresh", client)

    def test_unacknowledged_index_or_pipeline_stops(self):
        for failed_path in ("/product-listings", bootstrap.PIPELINE_PATH):
            client = Cluster(fresh=True)
            original = client.request
            def unconfirmed(method, path, *args, **kwargs):
                result = original(method, path, *args, **kwargs)
                return {"acknowledged": False} if method == "PUT" and path == failed_path else result
            client.request = unconfirmed
            with self.assertRaisesRegex(bootstrap.Failure, "WRITE_UNCONFIRMED"):
                self.run_flow("initialize-fresh", client)
            self.assertEqual(client.calls[-1][:2], ("PUT", failed_path))


class ReadbackTests(unittest.TestCase):
    def test_only_observed_object_defaults_are_normalized(self):
        for name, desired in bootstrap.assets()[0]:
            actual = index_response(name, desired)
            bootstrap.compatible_index(name, desired, actual)
            self.assertEqual(bootstrap.mapping(desired["mappings"])["properties"]["embedding"],
                             desired["mappings"]["properties"]["embedding"])
        node = {"type": "object", "enabled": True, "properties": {"type": {"type": "keyword"}}}
        self.assertEqual(bootstrap.mapping(node), {"properties": {"type": {"type": "keyword"}}})
        self.assertEqual(bootstrap.mapping({"type": "nested", "enabled": True, "properties": {}}),
                         {"type": "nested", "enabled": True, "properties": {}})

    def test_complete_mapping_drift_including_false_enabled_is_rejected(self):
        name, desired = bootstrap.assets()[0][0]
        for field, value in (("extra", {"type": "keyword"}), ("title", {"enabled": False}),
                             ("embedding", {"type": "knn_vector", "dimension": 1}),
                             ("productListingId", {"type": "text"})):
            actual = index_response(name, desired)
            actual[name]["mappings"]["properties"][field] = value
            with self.assertRaisesRegex(bootstrap.Failure, "MAPPING_MISMATCH"):
                bootstrap.compatible_index(name, desired, actual)
        actual = index_response(name, desired)
        del actual[name]["mappings"]["properties"]["projectionDeleted"]
        with self.assertRaises(bootstrap.Failure):
            bootstrap.compatible_index(name, desired, actual)

    def test_declared_flat_settings_and_all_analysis_compare(self):
        name, desired = bootstrap.assets()[0][0]
        expected = bootstrap.settings(desired["settings"])
        self.assertEqual(expected["index.knn"], "true")
        self.assertEqual(expected["index.knn.algo_param.ef_search"], "128")
        self.assertEqual(expected["index.analysis.filter.french_elision.articles"],
                         desired["settings"]["analysis"]["filter"]["french_elision"]["articles"])
        for key, value in (("index.knn", "false"), ("index.knn.algo_param.ef_search", "127"),
                           ("index.analysis.filter.english_synonyms.synonyms_path", "remote.txt"),
                           ("index.analysis.analyzer.english_with_synonyms.filter", ["lowercase"]),
                           ("index.analysis.analyzer.unexpected.type", "standard")):
            actual = index_response(name, desired)
            actual[name]["settings"][key] = value
            with self.assertRaises(bootstrap.Failure):
                bootstrap.compatible_index(name, desired, actual)
        actual = index_response(name, desired)
        del actual[name]["settings"]["index.knn"]
        with self.assertRaises(bootstrap.Failure):
            bootstrap.compatible_index(name, desired, actual)

    def test_alias_resolution_and_extra_indices_are_not_accepted(self):
        name, desired = bootstrap.assets()[0][0]
        actual = index_response(name, desired)
        actual[name]["aliases"] = {"alias": {}}
        with self.assertRaisesRegex(bootstrap.Failure, "INDEX_ALIASES"):
            bootstrap.compatible_index(name, desired, actual)
        with self.assertRaisesRegex(bootstrap.Failure, "INDEX_IDENTITY"):
            bootstrap.compatible_index(name, desired, {"other": actual[name]})


class InputAndTransportTests(unittest.TestCase):
    def test_https_origin_only(self):
        self.assertEqual(bootstrap.endpoint("https://opensearch:9200"), ("opensearch", 9200))
        self.assertEqual(bootstrap.endpoint("https://[::1]:9200"), ("::1", 9200))
        for value in ("http://opensearch", "https://u:p@opensearch", "https://opensearch/",
                      "https://opensearch/path", "https://opensearch?", "https://opensearch#",
                      "https://opensearch?q=x", "https://opensearch#x", "https://opensearch:",
                      "https://opensearch:0", "https://opensearch:65536", "https://opensearch:bad",
                      "https://open\nsearch", " https://opensearch", "https://", "https://opensearch\\x",
                      "https://opensearch%2fanything", "https://[::1]ignored:9200",
                      "https://[::1]ignored", "https://opensearch:09200"):
            with self.subTest(value=value), self.assertRaises((bootstrap.Failure, ValueError)):
                bootstrap.endpoint(value)

    def test_stage_and_env_fail_before_ssl(self):
        for stage in ("prod", "local", "DEV", "dev ", ""):
            with patch.dict(os.environ, {"STAGE": stage}, clear=True), patch.object(bootstrap.ssl, "create_default_context") as ctx:
                with self.assertRaises(bootstrap.Failure):
                    bootstrap.Client()
                ctx.assert_not_called()

    def test_inherited_tls_key_logging_is_rejected_before_context_creation(self):
        with patch.dict(os.environ, {"STAGE": "dev", "SSLKEYLOGFILE": "private"}, clear=True), patch.object(bootstrap.ssl, "create_default_context") as ctx:
            with self.assertRaisesRegex(bootstrap.Failure, "TLS_KEYLOG_FORBIDDEN"):
                bootstrap.Client()
            ctx.assert_not_called()

    def test_all_assets_are_read_before_target_access(self):
        with patch.object(bootstrap, "read_asset", side_effect=[{}, {}, ValueError("private JSON")]), patch.object(bootstrap, "Client") as client:
            with self.assertRaises(ValueError):
                bootstrap.Bootstrap().run("initialize-fresh")
            client.assert_not_called()

    def test_key_regular_bounded_owner_only_readable(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "key"
            path.write_bytes(b"not a real key")
            path.chmod(0o600)
            self.assertEqual(bootstrap.checked_file(path, secret=True), path)
            path.chmod(0o640)
            with self.assertRaisesRegex(bootstrap.Failure, "KEY_PERMISSIONS"):
                bootstrap.checked_file(path, secret=True)
            with self.assertRaises(bootstrap.Failure):
                bootstrap.checked_file(directory)
            path.write_bytes(b"x" * (bootstrap.LIMIT + 1))
            with self.assertRaises(bootstrap.Failure):
                bootstrap.checked_file(path)

    def test_default_tls_validation_and_no_password_prompt(self):
        environment = {"STAGE": "dev", "OPENSEARCH_CLUSTER_NAME": "isolated-test",
                       "OPENSEARCH_ENDPOINT_URL": "https://opensearch:9200",
                       "OPENSEARCH_SSL_ROOT_CERT": "ca", "OPENSEARCH_ADMIN_CERT": "cert",
                       "OPENSEARCH_ADMIN_KEY": "key"}
        context = ssl.create_default_context()
        with patch.dict(os.environ, environment, clear=True), patch.object(bootstrap, "checked_file", side_effect=lambda p, **kw: Path(p)), patch.object(bootstrap.ssl, "create_default_context", return_value=context) as create, patch.object(context, "load_cert_chain") as load:
            client = bootstrap.Client()
        create.assert_called_once_with(cafile="ca")
        self.assertTrue(client.context.check_hostname)
        self.assertEqual(client.context.verify_mode, ssl.CERT_REQUIRED)
        self.assertEqual(load.call_args.args, ("cert", "key"))
        with self.assertRaisesRegex(bootstrap.Failure, "KEY_MUST_BE_UNENCRYPTED"):
            load.call_args.kwargs["password"]()

    def request(self, status, raw, missing=None, path="/", method="GET"):
        client = object.__new__(bootstrap.Client)
        client.host, client.port, client.context = "opensearch", 9200, object()
        connection = Mock()
        connection.getresponse.return_value.status = status
        connection.getresponse.return_value.read.return_value = raw
        with patch.object(bootstrap.http.client, "HTTPSConnection", return_value=connection) as ctor:
            try:
                return client.request(method, path, missing=missing)
            finally:
                ctor.assert_called_once_with("opensearch", 9200, context=client.context, timeout=10)
                self.assertEqual(connection.request.call_count, 1)
                connection.close.assert_called_once()
                connection.getresponse.return_value.read.assert_called_once_with(bootstrap.LIMIT + 1)

    def test_no_redirect_auth_error_or_untyped_404_counts_as_absence(self):
        for status in (301, 302, 307, 308, 401, 403, 500, 503):
            with self.assertRaises(bootstrap.Failure):
                self.request(status, b'{"private":"secret"}', "index_not_found_exception")
        for body in (b'{}', b'{"status":404,"error":"missing"}',
                     b'{"status":404,"error":{"type":"security_exception"}}'):
            with self.assertRaises(bootstrap.Failure):
                self.request(404, body, "index_not_found_exception")
        for kind in ("index_not_found_exception", "resource_not_found_exception"):
            self.assertIsNone(self.request(404, json.dumps({"status": 404, "error": {"type": kind}}).encode(), kind))
        with self.assertRaises(bootstrap.Failure):
            self.request(404, b'{}')

    def test_stock_empty_pipeline_404_is_absent_only_for_fixed_request(self):
        self.assertIsNone(self.request(404, b'{}', "resource_not_found_exception", bootstrap.PIPELINE_PATH))
        for path in ("/", "/_search/pipeline", "/_search/pipeline/other",
                     bootstrap.PIPELINE_PATH + "/", bootstrap.PIPELINE_PATH + "?pretty=true",
                     "/product-listings?flat_settings=true", "/user_search_filters?flat_settings=true"):
            with self.subTest(path=path), self.assertRaises(bootstrap.Failure):
                self.request(404, b'{}', "resource_not_found_exception", path)
        for missing in (None, "index_not_found_exception", "other", True):
            with self.subTest(missing=missing), self.assertRaises(bootstrap.Failure):
                self.request(404, b'{}', missing, bootstrap.PIPELINE_PATH)
        for method in ("PUT", "HEAD", "DELETE"):
            with self.subTest(method=method), self.assertRaises(bootstrap.Failure):
                self.request(404, b'{}', "resource_not_found_exception", bootstrap.PIPELINE_PATH, method)

    def test_empty_index_404_still_requires_typed_absence(self):
        for name, _ in bootstrap.INDICES:
            path = "/" + name + "?flat_settings=true"
            with self.subTest(index=name), self.assertRaises(bootstrap.Failure):
                self.request(404, b'{}', "index_not_found_exception", path)
            body = b'{"status":404,"error":{"type":"index_not_found_exception"}}'
            self.assertIsNone(self.request(404, body, "index_not_found_exception", path))

    def test_pipeline_other_status_or_body_is_not_empty_absence(self):
        for status in (204, 301, 302, 307, 308, 400, 401, 403, 500, 503):
            with self.subTest(status=status), self.assertRaises(bootstrap.Failure):
                self.request(status, b'{}', "resource_not_found_exception", bootstrap.PIPELINE_PATH)
        # A successful empty response is a present value, never the absence sentinel.
        self.assertEqual(self.request(200, b'{}', "resource_not_found_exception", bootstrap.PIPELINE_PATH), {})
        for body in (b'', b'null', b'[]', b'{"unexpected":true}', b'{"status":404}',
                     b'{"status":404,"error":{"type":"index_not_found_exception"}}',
                     b'{"status":404,"error":{"type":"security_exception"}}'):
            with self.subTest(body=body), self.assertRaises((bootstrap.Failure, ValueError)):
                self.request(404, body, "resource_not_found_exception", bootstrap.PIPELINE_PATH)

    def test_response_bounds_shape_and_invalid_json(self):
        for body in (b"x" * (bootstrap.LIMIT + 1), b"[]", b"null", b"not JSON"):
            with self.assertRaises((bootstrap.Failure, ValueError)):
                self.request(200, body)
        self.assertEqual(self.request(200, b'{"ok":true}'), {"ok": True})

    def test_usage_errors_never_echo_argv_or_touch_target(self):
        output = io.StringIO()
        with patch.object(bootstrap, "Client") as client, contextlib.redirect_stderr(output):
            self.assertEqual(bootstrap.main(["--password=secret"]), 2)
            client.assert_not_called()
        self.assertNotIn("secret", output.getvalue())

    def test_deadline_and_early_error_redaction(self):
        with self.assertRaises(bootstrap.Failure):
            bootstrap.interrupted(None, None)
        output = io.StringIO()
        with patch.object(bootstrap, "Client", side_effect=OSError("private file path")), patch.object(bootstrap.signal, "alarm") as alarm, contextlib.redirect_stderr(output):
            self.assertEqual(bootstrap.main(["verify"]), 1)
        self.assertEqual([c.args for c in alarm.call_args_list], [(90,), (0,)])
        self.assertEqual(output.getvalue(), "opensearch outcome=no-writes-attempted phase=inputs\n")


class NativeExamplesTests(unittest.TestCase):
    def test_credentials_and_default_grants_absent(self):
        directory = ROOT / "deploy/compose/opensearch/security"
        self.assertFalse((directory / "internal_users.yml").exists())
        example = (directory / "internal_users.yml.example").read_text()
        self.assertEqual(example.count("hash: null"), 5)
        self.assertNotIn("$2", example)
        roles = (directory / "roles.yml").read_text()
        active = "\n".join(line for line in roles.splitlines() if not line.startswith("#"))
        self.assertNotIn("indices:admin", active)
        self.assertNotIn("cluster:", active.replace('"cluster:monitor/main"', ""))
        self.assertNotIn('index_patterns: ["*"]', active)
        self.assertEqual(active.count("cluster_permissions:"), 5)
        for role in ("aura_reader", "aura_product_projector", "aura_filter_projector", "aura_percolator", "aura_cron"):
            permissions = ["cluster:monitor/main"]
            if role in ("aura_product_projector", "aura_filter_projector"):
                permissions.append("indices:data/write/bulk")
            self.assertIn(role + ":\n  cluster_permissions: " + json.dumps(permissions) + "\n", active)
        self.assertEqual(active.count('"indices:data/write/bulk"'), 2)
        self.assertEqual(active.count('"indices:data/write/bulk[s]"'), 2)
        self.assertNotIn("indices:data/write/bulk*", active)
        for role, index in (("aura_product_projector", "product-listings"),
                            ("aura_filter_projector", "user_search_filters")):
            section = active.split(role + ":\n", 1)[1].split("  tenant_permissions:", 1)[0]
            section = section.split("  index_permissions:\n", 1)[1]
            actions = ["indices:data/write/index*", "indices:data/write/bulk[s]",
                       "indices:data/read/get*", "indices:data/read/search*"]
            expected = "- index_patterns: " + json.dumps([index]) + " allowed_actions: " + json.dumps(actions)
            self.assertEqual("".join(section.split()).replace(",]", "]"), "".join(expected.split()))
        for filename, kind in (("action_groups.yml", "actiongroups"), ("tenants.yml", "tenants"), ("nodes_dn.yml", "nodesdn")):
            lines = [line for line in (directory / filename).read_text().splitlines() if not line.startswith("#")]
            self.assertEqual(lines, ["_meta:", "  type: " + kind, "  config_version: 2"])

    def test_pipeline_matches_existing_rrf_definition(self):
        self.assertEqual(bootstrap.assets()[1], {
            "description": "Hybrid BM25+kNN search pipeline using Reciprocal Rank Fusion",
            "phase_results_processors": [{"score-ranker-processor": {"combination": {"technique": "rrf"}}}]})


if __name__ == "__main__":
    unittest.main()
