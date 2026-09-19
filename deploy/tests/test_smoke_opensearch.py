"""Offline safety guards only. No Docker, sockets, real credentials, or Rust."""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("smoke_opensearch", Path(__file__).with_name("smoke-opensearch.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class SourceTests(unittest.TestCase):
    def test_reviewed_sources_match(self):
        smoke.source_guard()
        smoke.image_pins_guard()

    def test_helper_image_pin_rejects_malformed_id_before_docker(self):
        for bad in (smoke.PYTHON + "3", "sha256:" + "a" * 63, "sha256:" + "A" * 64, "python:latest"):
            probe = object.__new__(smoke.Probe)
            probe.call = Mock()
            with self.subTest(bad=bad), patch.object(smoke, "PYTHON", bad):
                with self.assertRaisesRegex(smoke.Failure, "MALFORMED_IMAGE_PIN"):
                    probe.prepare()
            probe.call.assert_not_called()

        valid_default = "sha256:" + "b" * 64
        for bad in ("sha256:" + "a" * 63, "sha256:" + "A" * 64, "python:latest",
                    "repo@sha256:" + "c" * 64, valid_default + "\n"):
            probe = object.__new__(smoke.Probe)
            probe.args = SimpleNamespace(helper_image=bad)
            probe.call = Mock()
            with self.subTest(explicit=bad), patch.object(smoke, "PYTHON", valid_default):
                with self.assertRaisesRegex(smoke.Failure, "MALFORMED_HELPER_IMAGE"):
                    probe.prepare()
            probe.call.assert_not_called()

    def test_explicit_helper_id_controls_default_python_specs(self):
        helper = "sha256:" + "d" * 64
        engine = "sha256:" + "e" * 64
        with tempfile.TemporaryDirectory() as folder:
            probe = smoke.Probe(Path(folder), SimpleNamespace(helper_image=helper), Mock())
            self.assertEqual(probe.helper_image, helper)
            self.assertEqual(probe.plain_spec()["image"], helper)
            self.assertEqual(probe.plain_spec(engine)["image"], engine)

    def test_engine_pin_rejects_malformed_reference_before_docker(self):
        snapshot = smoke.source_guard()
        for bad in (b"opensearchproject/opensearch:latest\n",
                    b"opensearchproject/opensearch:3.8.0-rc1@sha256:" + b"a" * 64 + b"\n",
                    b"opensearchproject/opensearch:3.8.0@sha256:" + b"a" * 63 + b"\n",
                    b"opensearchproject/opensearch:3.8.0@sha256:" + b"A" * 64 + b"\n"):
            altered = dict(snapshot, **{smoke.IMAGE_REF: bad})
            probe = object.__new__(smoke.Probe)
            probe.call = Mock()
            with self.subTest(bad=bad), patch.object(smoke, "source_guard", return_value=altered):
                with self.assertRaisesRegex(smoke.Failure, "^ENGINE_PIN_INVALID$"):
                    probe.prepare()
            probe.call.assert_not_called()

    def test_local_image_override_accepts_only_immutable_ids(self):
        self.assertEqual(smoke.local_image_id("sha256:" + "a" * 64), "sha256:" + "a" * 64)
        for value in ("opensearchproject/opensearch:3.8.0", "latest", "sha256:" + "A" * 64):
            with self.assertRaises(smoke.argparse.ArgumentTypeError):
                smoke.local_image_id(value)

    def test_source_change_rejected_before_any_docker_call(self):
        probe = object.__new__(smoke.Probe)
        probe.call = Mock(side_effect=AssertionError("must not invoke Docker"))
        with patch.object(smoke, "checked_bytes", return_value=b"include: [/unreviewed.yml]\n"):
            with self.assertRaisesRegex(smoke.Failure, "SOURCE_NOT_REVIEWED"):
                probe.prepare()
        probe.call.assert_not_called()

    def test_root_changes_after_capture_cannot_change_mounted_native_inputs(self):
        with tempfile.TemporaryDirectory() as folder, contextlib.redirect_stdout(io.StringIO()):
            root = Path(folder) / "checkout"
            for name, raw in smoke.source_guard().items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(raw)
            captured = smoke.source_guard(root)
            roles = "deploy/compose/opensearch/security/roles.yml"
            node = "deploy/compose/opensearch/opensearch.yml"
            (root / roles).write_text('aura_reader:\n  cluster_permissions: ["cluster:*"]\n')
            (root / node).write_text("plugins.security.disabled: true\n")
            directory = Path(folder) / "fixture"
            directory.mkdir()
            probe = smoke.Probe(directory, SimpleNamespace(cluster_name="owned-cluster", port=9200), Mock())
            with patch.object(smoke, "ROOT", root):
                probe.stage_sources(captured)
                probe.native_inputs()
            self.assertEqual((directory / "secrets/admin/security/roles.yml").read_bytes(), captured[roles])
            expected_node = captured[node].decode().replace("cluster.name: aura-dev-search", "cluster.name: owned-cluster") + "\nhttp.port: 9200\n"
            self.assertEqual((directory / "secrets/opensearch.yml").read_text(), expected_node)
            probe.freeze_generated()
            self.assertEqual(json.loads((directory / "generated-inputs.json").read_text()), probe.generated)
            self.assertEqual(probe.generated["admin/security/roles.yml"], smoke.PINS[roles])
            corrupted = dict(captured)
            corrupted[roles] = (root / roles).read_bytes()
            with self.assertRaisesRegex(smoke.Failure, "SNAPSHOT_CHANGED"):
                probe.stage_sources(corrupted)

    def test_generated_drift_refuses_before_ownership_helper_creation(self):
        with tempfile.TemporaryDirectory() as folder, contextlib.redirect_stdout(io.StringIO()):
            probe = smoke.Probe(Path(folder), SimpleNamespace(), Mock())
            path = probe.write("secrets/admin/security/roles.yml", "trusted-native-input")
            path.write_text('cluster_permissions: ["cluster:*"]')
            probe.create_plain = Mock()
            with self.assertRaisesRegex(smoke.Failure, "GENERATED_INPUT_CHANGED"):
                probe.permissions()
            probe.create_plain.assert_not_called()

    def test_symlink_source_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            source = Path(folder) / "source"
            source.write_text("synthetic")
            link = Path(folder) / "link"
            link.symlink_to(source)
            with self.assertRaisesRegex(smoke.Failure, "SOURCE_PATH"):
                smoke.checked_bytes(link)

    def test_no_implicit_real_run(self):
        with patch.object(smoke, "load_capture") as capture:
            with self.assertRaisesRegex(smoke.Failure, "PRE_REVIEW"):
                smoke.main([])
        capture.assert_not_called()

    def test_endpoint_inputs_cannot_escape_fixture(self):
        for value in ("https://example.com", "127.0.0.1", "remote.example", "x\nSECRET=y", "--host", "x/y"):
            with self.subTest(value=value), self.assertRaises(smoke.argparse.ArgumentTypeError):
                smoke.hostname(value)
        self.assertEqual(smoke.hostname("fixture-node"), "fixture-node")
        with self.assertRaisesRegex(smoke.Failure, "ENDPOINT_INPUT"):
            smoke.main(["--reviewed-run", "--wrong-hostname", "opensearch"])


class SecurityPreflightTests(unittest.TestCase):
    def whoami(self, **changes):
        body = {
            "dn": smoke.EXPECTED_ADMIN_DN,
            "is_admin": True,
            "is_node_certificate_request": False,
            "unused": "secret-canary",
        }
        body.update(changes)
        return {"status": 200, "body": body}

    def security_plugin(self, **changes):
        plugin = {
            "name": smoke.SECURITY_PLUGIN_NAME,
            "classname": smoke.SECURITY_PLUGIN_CLASSNAME,
            "version": "3.8.0.0",
            "unused": "secret-canary",
        }
        plugin.update(changes)
        return plugin

    def nodes(self, **changes):
        node = {
            "version": "3.8.0",
            "plugins": [
                self.security_plugin(),
                {"name": "some-other-plugin", "classname": "org.example.OtherPlugin", "version": "1.2.3.4",
                 "unused": "secret-canary"},
            ],
            "unused": "secret-canary",
        }
        node.update(changes)
        return {"status": 200, "body": {"nodes": {"node-id": node}, "unused": "secret-canary"}}

    def health(self, **changes):
        body = {"cluster_name": "owned-cluster", "status": "yellow", "timed_out": False,
            "unused": "secret-canary"}
        body.update(changes)
        return {"status": 200, "body": body}

    def invoke(self, responses):
        probe = object.__new__(smoke.Probe)
        probe.args = SimpleNamespace(hostname="opensearch", port=9200, cluster_name="owned-cluster")
        probe.engine_version = "3.8.0"
        probe.client = Mock(side_effect=responses)
        probe.event = Mock()
        output = io.StringIO()
        result, error = None, None
        with contextlib.redirect_stdout(output):
            try:
                result = probe.security_preflight()
            except Exception as caught:
                error = caught
        return probe, output.getvalue(), result, error

    def assert_get_only(self, probe):
        self.assertEqual([call.args[0]["path"] for call in probe.client.call_args_list],
            [smoke.SECURITY_WHOAMI, smoke.SECURITY_NODES, smoke.SECURITY_PREFLIGHT_HEALTH])
        for call in probe.client.call_args_list:
            case = call.args[0]
            self.assertEqual(case["method"], "GET")
            self.assertEqual(case["cert"], "admin")
            self.assertNotIn(case["method"], {"POST", "PUT", "DELETE", "PATCH"})

    def assert_failure(self, responses, code):
        probe, output, result, error = self.invoke(responses)
        self.assertIsNone(result)
        self.assertIsInstance(error, smoke.Failure)
        self.assertEqual(str(error), code)
        self.assertEqual(probe.event.call_count, 0)
        self.assertNotIn("secret-canary", output)
        self.assertNotIn("secret-canary", str(error))
        for call in probe.client.call_args_list:
            self.assertEqual(call.args[0]["method"], "GET")

    def test_success_is_fixed_safe_and_get_only(self):
        probe, output, facts, error = self.invoke([self.whoami(), self.nodes(), self.health()])
        self.assertIsNone(error)
        self.assertEqual(facts, {
            "whoami_status": 200,
            "admin": True,
            "node_certificate": False,
            "admin_dn_matches": True,
            "node_count": 1,
            "node_versions": ["3.8.0"],
            "security_plugin_present": True,
            "security_plugin_versions": ["3.8.0.0"],
            "cluster_health": "yellow",
            "cluster_timed_out": False,
        })
        self.assertEqual(output.count("SECURITY_PREFLIGHT "), 1)
        self.assertEqual(json.loads(output.split(" ", 1)[1]), facts)
        self.assertNotIn(smoke.EXPECTED_ADMIN_DN, output)
        self.assertNotIn("secret-canary", output)
        self.assertNotIn("some-other-plugin", output)
        self.assertNotIn("org.example.OtherPlugin", output)
        self.assertEqual(probe.event.call_args.args, ("security-preflight",))
        self.assertEqual(probe.event.call_args.kwargs, facts)
        self.assert_get_only(probe)

    def test_success_writes_only_fixed_values_to_safe_evidence(self):
        with tempfile.TemporaryDirectory() as folder, contextlib.redirect_stdout(io.StringIO()):
            probe = smoke.Probe(Path(folder), SimpleNamespace(cluster_name="owned-cluster"), Mock())
            probe.engine_version = "3.8.0"
            probe.client = Mock(side_effect=[self.whoami(), self.nodes(), self.health()])
            facts = probe.security_preflight()
            evidence = (Path(folder) / "evidence.json").read_text()
        self.assertIn('"check": "security-preflight"', evidence)
        self.assertIn('"cluster_health": "yellow"', evidence)
        self.assertNotIn(smoke.EXPECTED_ADMIN_DN, evidence)
        self.assertNotIn("secret-canary", evidence)
        self.assertNotIn("some-other-plugin", evidence)
        self.assertNotIn("org.example.OtherPlugin", evidence)
        self.assertEqual(facts["security_plugin_versions"], ["3.8.0.0"])

    def test_whoami_rejections_require_real_booleans_and_expected_identity(self):
        cases = [
            (self.whoami(is_admin=False), "SECURITY_PREFLIGHT_ADMIN"),
            (self.whoami(is_node_certificate_request=True), "SECURITY_PREFLIGHT_NODE_CERTIFICATE"),
            (self.whoami(is_admin="true"), "SECURITY_PREFLIGHT_WHOAMI_FLAGS"),
            (self.whoami(is_node_certificate_request=0), "SECURITY_PREFLIGHT_WHOAMI_FLAGS"),
            ({"status": 401, "body": {"unused": "secret-canary"}}, "SECURITY_PREFLIGHT_WHOAMI_STATUS"),
            ({"status": 200, "body": []}, "SECURITY_PREFLIGHT_WHOAMI_BODY"),
            (self.whoami(dn="wrong-secret-canary"), "SECURITY_PREFLIGHT_ADMIN_DN"),
            (self.whoami() | {"body": {"is_admin": True, "is_node_certificate_request": False}},
                "SECURITY_PREFLIGHT_ADMIN_DN"),
        ]
        for response, code in cases:
            with self.subTest(code=code):
                self.assert_failure([response], code)

    def test_node_info_rejections_are_strict(self):
        base = self.whoami()
        cases = []
        cases.append(({"status": 503, "body": {"unused": "secret-canary"}}, "SECURITY_PREFLIGHT_NODES_STATUS"))
        cases.append(({"status": 200, "body": []}, "SECURITY_PREFLIGHT_NODES_BODY"))
        cases.append(({"status": 200, "body": {"nodes": []}}, "SECURITY_PREFLIGHT_NODES_MAP"))
        cases.append(({"status": 200, "body": {"nodes": {}}}, "SECURITY_PREFLIGHT_NODE_COUNT"))
        cases.append((self.nodes(version="3.7.0"), "SECURITY_PREFLIGHT_ENGINE_VERSION"))
        cases.append((self.nodes(plugins=None), "SECURITY_PREFLIGHT_NODE_PLUGINS"))
        cases.append((self.nodes(plugins={}), "SECURITY_PREFLIGHT_NODE_PLUGINS"))
        cases.append((self.nodes(plugins=[{"name": "other", "classname": smoke.SECURITY_PLUGIN_CLASSNAME,
                                             "version": "3.8.0.0"}]),
            "SECURITY_PREFLIGHT_SECURITY_PLUGIN"))
        cases.append((self.nodes(plugins=[{"name": smoke.SECURITY_PLUGIN_NAME, "classname": "org.example.NotSecurity",
                                             "version": "3.8.0.0"}]),
            "SECURITY_PREFLIGHT_SECURITY_PLUGIN"))
        cases.append((self.nodes(plugins=[{"name": smoke.SECURITY_PLUGIN_NAME, "version": "3.8.0.0"}]),
            "SECURITY_PREFLIGHT_SECURITY_PLUGIN"))
        cases.append((self.nodes(plugins=[{"classname": smoke.SECURITY_PLUGIN_CLASSNAME, "version": "3.8.0.0"}]),
            "SECURITY_PREFLIGHT_SECURITY_PLUGIN"))
        cases.append((self.nodes(plugins=[self.security_plugin(), self.security_plugin()]),
            "SECURITY_PREFLIGHT_SECURITY_PLUGIN"))
        for plugin_version in ("3.8.0", "3.8", "latest", "3.8.0.0-SNAPSHOT", None, 3800):
            cases.append((self.nodes(plugins=[self.security_plugin(version=plugin_version)]),
                "SECURITY_PREFLIGHT_SECURITY_PLUGIN_VERSION"))
        for nodes, code in cases:
            with self.subTest(code=code, nodes=nodes):
                self.assert_failure([base, nodes], code)

    def test_health_records_all_exact_states_but_rejects_malformed_values(self):
        for status in ("green", "yellow", "red"):
            with self.subTest(status=status):
                probe, output, facts, error = self.invoke([self.whoami(), self.nodes(), self.health(status=status)])
                self.assertIsNone(error)
                self.assertEqual(facts["cluster_health"], status)
                self.assertFalse(facts["cluster_timed_out"])
                self.assert_get_only(probe)
                self.assertNotIn("secret-canary", output)
        for status in ("GREEN", "unknown", 1, None, []):
            with self.subTest(status=status):
                self.assert_failure([self.whoami(), self.nodes(), self.health(status=status)],
                    "SECURITY_PREFLIGHT_HEALTH_STATE")
        for timed_out in (None, "false", 0, 1, []):
            with self.subTest(timed_out=timed_out):
                self.assert_failure([self.whoami(), self.nodes(), self.health(timed_out=timed_out)],
                    "SECURITY_PREFLIGHT_HEALTH_TIMEOUT")
        self.assert_failure([self.whoami(), self.nodes(), self.health(cluster_name="other")],
            "SECURITY_PREFLIGHT_HEALTH_CLUSTER")


class MLReadinessTests(unittest.TestCase):
    def response(self, status="yellow"):
        return {"status": 200, "body": {"cluster_name": "owned-cluster", "timed_out": False, "status": status,
            "indices": {smoke.ML_INDEX: {"status": status, "number_of_shards": 1, "active_primary_shards": 1}}}}

    def test_exact_native_health_request_contract(self):
        self.assertEqual(smoke.ML_HEALTH, "/_cluster/health/.plugins-ml-config?level=indices&wait_for_status=yellow"
            "&timeout=60s&cluster_manager_timeout=5s&wait_for_active_shards=1")

    def test_yellow_empty_store_primary_zero_still_refuses(self):
        response = self.response("yellow")
        response["body"]["indices"][smoke.ML_INDEX]["active_primary_shards"] = 0
        self.assertFalse(smoke.ml_index_ready(response, "owned-cluster"))
        probe = object.__new__(smoke.Probe)
        probe.args = SimpleNamespace(cluster_name="owned-cluster")
        probe.event, probe.write = Mock(), Mock()
        probe.client = Mock(return_value=response)
        with self.assertRaisesRegex(smoke.Failure, "ML_CONFIG_INDEX_NOT_READY"):
            probe.ml_config_ready()
        self.assertEqual(probe.client.call_count, 1)
        self.assertFalse(probe.event.call_args.kwargs["ready"])

    def test_exact_named_index_health_predicate(self):
        for status in ("yellow", "green"):
            self.assertTrue(smoke.ml_index_ready(self.response(status), "owned-cluster"))
        changes = [lambda r: r.update(status=408), lambda r: r.update(status=503), lambda r: r.update(body=[]),
            lambda r: r["body"].update(cluster_name="other"), lambda r: r["body"].update(timed_out=True),
            lambda r: r["body"].pop("timed_out"), lambda r: r["body"].update(timed_out=0),
            lambda r: r["body"].update(status="red"), lambda r: r["body"].update(indices={}),
            lambda r: r["body"].update(indices=[]), lambda r: r["body"]["indices"].update(other={}),
            lambda r: r["body"]["indices"].update({smoke.ML_INDEX: None}),
            lambda r: r["body"]["indices"][smoke.ML_INDEX].update(status="red")]
        for key in ("number_of_shards", "active_primary_shards"):
            for value in (None, 0, 2, True, 1.0, "1"):
                bad = self.response()
                bad["body"]["indices"][smoke.ML_INDEX][key] = value
                self.assertFalse(smoke.ml_index_ready(bad, "owned-cluster"))
        for change in changes:
            bad = self.response()
            change(bad)
            self.assertFalse(smoke.ml_index_ready(bad, "owned-cluster"))
        for bad in (None, [], {}, {"transport": "peer_closed"}, {"tls": 20}):
            self.assertFalse(smoke.ml_index_ready(bad, "owned-cluster"))

    def test_gate_one_get_no_retry_and_safe_failure_evidence(self):
        probe = object.__new__(smoke.Probe)
        probe.args = SimpleNamespace(cluster_name="owned-cluster")
        probe.event, probe.write = Mock(), Mock()
        for response in (self.response(), {"status": 408, "body": {"secret": "secret-canary"}}, {"unknown": True}):
            probe.client = Mock(return_value=response)
            with patch.object(smoke.time, "sleep") as sleep:
                if response == self.response():
                    probe.ml_config_ready()
                else:
                    with self.assertRaisesRegex(smoke.Failure, "ML_CONFIG_INDEX_NOT_READY"):
                        probe.ml_config_ready()
                sleep.assert_not_called()
            probe.client.assert_called_once_with({"method": "GET", "path": smoke.ML_HEALTH, "cert": "admin"})
            probe.write.assert_called_with("ml-config-health.json", json.dumps(response))
        probe.client = Mock(side_effect=smoke.Failure("CLIENT_OUTCOME_UNKNOWN"))
        with self.assertRaisesRegex(smoke.Failure, "CLIENT_OUTCOME_UNKNOWN"):
            probe.ml_config_ready()
        self.assertEqual(probe.client.call_count, 1)
        self.assertNotIn("secret-canary", str(probe.event.call_args_list))

    def test_failed_health_response_is_private_and_exact_not_printed(self):
        response = {"status": 408, "body": {"unexpected": "secret-canary"}}
        with tempfile.TemporaryDirectory() as folder, contextlib.redirect_stdout(io.StringIO()) as output:
            probe = smoke.Probe(Path(folder), SimpleNamespace(cluster_name="owned-cluster"), Mock())
            probe.client = Mock(return_value=response)
            with self.assertRaisesRegex(smoke.Failure, "ML_CONFIG_INDEX_NOT_READY"):
                probe.ml_config_ready()
            path = Path(folder) / "ml-config-health.json"
            self.assertEqual(json.loads(path.read_text()), response)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertNotIn("secret-canary", output.getvalue())
            self.assertNotIn("secret-canary", (Path(folder) / "evidence.json").read_text())

    def test_gate_before_first_baseline_and_after_restart_before_verify(self):
        for failed_gate in (None, 1, 2):
            probe = object.__new__(smoke.Probe)
            probe.ids = {"opensearch": "owned-node"}
            probe.directory = Path("/unused-unit-fixture")
            calls = Mock()
            for name in ("prepare", "admin", "compose_create", "call", "wait_node", "security_preflight", "target", "ml_config_ready",
                    "unchanged", "operator", "grants", "denied_changes", "tombstones", "searches", "trust_tests",
                    "snapshot", "runtime", "request", "event"):
                method = Mock()
                setattr(probe, name, method)
                calls.attach_mock(method, name)
            probe.snapshot.return_value = "unchanged-full-state"
            probe.wait_node.side_effect = probe.target
            probe.runtime.side_effect = [{"State": {"StartedAt": "before"}}, {"State": {"Running": False}},
                {"State": {"StartedAt": "after"}}]
            probe.request.side_effect = lambda method, path, *args, **kw: (
                {"status": 404, "body": {}} if path == smoke.PIPELINE and method == "GET"
                else {"status": 200, "body": {"acknowledged": True}})
            if failed_gate:
                probe.ml_config_ready.side_effect = [None] * (failed_gate - 1) + [smoke.Failure("ML_CONFIG_INDEX_NOT_READY")]
            with self.subTest(failed_gate=failed_gate), patch.object(smoke, "checked_bytes", return_value=b"{}"):
                if failed_gate:
                    with self.assertRaisesRegex(smoke.Failure, "ML_CONFIG_INDEX_NOT_READY"):
                        probe.run()
                else:
                    probe.run()
            names = [call[0] for call in calls.mock_calls]
            first = names.index("ml_config_ready")
            preflight = names.index("security_preflight")
            self.assertEqual(names[preflight - 2:preflight + 2],
                ["wait_node", "target", "security_preflight", "admin"])
            self.assertEqual(names[first-2:first], ["admin", "target"])
            if failed_gate == 1:
                self.assertEqual(names[-1], "ml_config_ready")
                probe.snapshot.assert_not_called()
                probe.unchanged.assert_not_called()
                probe.request.assert_not_called()
                continue
            self.assertEqual(names[first+1], "unchanged")
            last = len(names) - 1 - names[::-1].index("ml_config_ready")
            self.assertEqual(names[last-3:last], ["wait_node", "target", "runtime"])
            if failed_gate == 2:
                self.assertEqual(names[-1], "ml_config_ready")
                self.assertEqual(probe.snapshot.call_count, 1)  # Original pre-stop snapshot only.
            else:
                self.assertEqual(names[last+1:last+3], ["operator", "snapshot"])
                self.assertEqual(probe.ml_config_ready.call_count, 2)
                self.assertEqual(probe.snapshot.call_count, 2)


class SecurityPreflightRunOrderTests(unittest.TestCase):
    def test_failed_preflight_stops_before_online_securityadmin(self):
        probe = object.__new__(smoke.Probe)
        probe.ids = {"opensearch": "owned-node"}
        calls = Mock()
        for name in ("prepare", "admin", "compose_create", "call", "wait_node", "security_preflight", "target"):
            method = Mock()
            setattr(probe, name, method)
            calls.attach_mock(method, name)
        probe.security_preflight.side_effect = smoke.Failure("SECURITY_PREFLIGHT_ADMIN")
        with self.assertRaisesRegex(smoke.Failure, "^SECURITY_PREFLIGHT_ADMIN$"):
            probe.run()
        names = [call[0] for call in calls.mock_calls]
        self.assertEqual(names[names.index("security_preflight") - 1], "wait_node")
        self.assertEqual(probe.admin.call_count, 1)
        self.assertEqual(probe.admin.call_args.kwargs, {"offline": True})
        self.assertEqual(probe.compose_create.call_count, 1)  # The node, not native admin.

    def test_valid_red_diagnostic_does_not_skip_or_repeat_native_admin(self):
        probe = object.__new__(smoke.Probe)
        probe.ids = {"opensearch": "owned-node"}
        probe.directory = Path("/unused-unit-fixture")
        calls = Mock()
        for name in ("prepare", "admin", "compose_create", "call", "wait_node", "security_preflight", "target",
                "ml_config_ready", "unchanged", "operator", "grants", "denied_changes", "tombstones", "searches",
                "trust_tests", "snapshot", "runtime", "request", "event"):
            method = Mock()
            setattr(probe, name, method)
            calls.attach_mock(method, name)
        probe.security_preflight.return_value = {"cluster_health": "red"}
        probe.snapshot.return_value = "unchanged-full-state"
        probe.runtime.side_effect = [{"State": {"StartedAt": "before"}}, {"State": {"Running": False}},
            {"State": {"StartedAt": "after"}}]
        probe.request.side_effect = lambda method, path, *args, **kw: (
            {"status": 404, "body": {}} if path == smoke.PIPELINE and method == "GET"
            else {"status": 200, "body": {"acknowledged": True}})
        with patch.object(smoke, "checked_bytes", return_value=b"{}"):
            probe.run()
        self.assertEqual(probe.admin.call_count, 2)
        self.assertEqual(probe.admin.call_args_list[0].kwargs, {"offline": True})
        self.assertEqual(probe.admin.call_args_list[1].args, ())
        self.assertEqual(probe.security_preflight.call_count, 1)


class ClientTests(unittest.TestCase):
    def test_only_exact_admin_ml_health_get_has_75s_socket(self):
        exact = {"host": "opensearch", "port": 9200, "method": "GET", "path": smoke.ML_HEALTH, "cert": "admin"}
        variants = [({}, 75), ({"path": "/"}, 8), ({"method": "POST"}, 8), ({"path": smoke.ML_HEALTH + "&extra=true"}, 8),
            ({"path": smoke.ML_HEALTH.removesuffix("&wait_for_active_shards=1")}, 8),
            ({"body": {}}, 8), ({"cert": None}, 8), ({"plain": True}, 8), ({"timeout": 999}, 75)]
        for options, timeout in variants:
            connection = Mock()
            connection.getresponse.return_value.read.return_value = b"{}"
            connection.getresponse.return_value.status = 200
            with self.subTest(options=options), patch.object(smoke.ssl, "create_default_context"), \
                    patch.object(smoke.http.client, "HTTPSConnection", return_value=connection) as https, \
                    patch.object(smoke.http.client, "HTTPConnection", return_value=connection) as http:
                smoke.client_request(exact | options)
            chosen = http if options.get("plain") else https
            self.assertEqual(chosen.call_args.kwargs["timeout"], timeout)
            self.assertEqual(connection.request.call_count, 1)
            connection.close.assert_called_once()

    def test_ml_socket_timeout_stays_unknown_with_existing_helper_exec_bounds(self):
        case = {"host": "opensearch", "port": 9200, "method": "GET", "path": smoke.ML_HEALTH, "cert": "admin"}
        connection = Mock()
        connection.getresponse.side_effect = TimeoutError("secret-canary")
        output = io.StringIO()
        with patch.object(smoke.ssl, "create_default_context"), \
                patch.object(smoke.http.client, "HTTPSConnection", return_value=connection), \
                patch.object(smoke.sys, "stdin", SimpleNamespace(buffer=io.BytesIO(json.dumps(case).encode()))), \
                patch.object(smoke.signal, "signal"), patch.object(smoke.signal, "alarm") as alarm, contextlib.redirect_stdout(output):
            self.assertEqual(smoke.client_main(), 1)
        alarm.assert_called_once_with(100)
        self.assertEqual(output.getvalue(), '{"unknown":true}\n')
        self.assertEqual(connection.request.call_count, 1)
        connection.close.assert_called_once()
        probe = object.__new__(smoke.Probe)
        probe.args, probe.ids = SimpleNamespace(hostname="opensearch", port=9200), {"client": "owned"}
        probe.runtime = Mock()
        probe.call = Mock(return_value=(1, output.getvalue()))
        with self.assertRaisesRegex(smoke.Failure, "CLIENT_OUTCOME_UNKNOWN"):
            probe.client(case)
        self.assertEqual(probe.call.call_count, 1)
        self.assertEqual(probe.call.call_args.kwargs["seconds"], 105)

    def test_raw_ndjson_is_not_json_quoted_and_has_native_content_type(self):
        raw = '{"delete":{"_index":"product-listings","_id":"synthetic"}}\n'
        connection = Mock()
        connection.getresponse.return_value.status = 200
        connection.getresponse.return_value.read.return_value = b'{"errors":true,"items":[]}'
        for payload, encoded, content_type in (({"ndjson": raw}, raw.encode(), "application/x-ndjson"),
                ({"body": {"synthetic": True}}, b'{"synthetic": true}', "application/json")):
            with self.subTest(payload=payload), patch.object(smoke.ssl, "create_default_context"), \
                    patch.object(smoke.http.client, "HTTPSConnection", return_value=connection):
                result = smoke.client_request({"host": "opensearch", "port": 9200, "method": "POST", "path": "/_bulk", **payload})
            self.assertEqual(result["status"], 200)
            connection.request.assert_called_with("POST", "/_bulk", body=encoded, headers={"Content-Type": content_type})
            connection.getresponse.return_value.read.assert_called_with(smoke.LIMIT + 1)
            connection.close.assert_called()

    def test_invalid_or_oversized_request_refuses_before_connection(self):
        for payload in ({"ndjson": {}}, {"ndjson": "{}"}, {"ndjson": "{}\n", "body": {}},
                {"ndjson": "é" * (smoke.LIMIT // 2) + "\n"}, {"body": "x" * smoke.LIMIT}):
            with self.subTest(kind=list(payload)), patch.object(smoke.http.client, "HTTPSConnection") as connection:
                with self.assertRaises(smoke.Failure):
                    smoke.client_request({"host": "opensearch", "port": 9200, **payload})
                connection.assert_not_called()

    def test_response_limit_and_client_failure_output_never_echo_payload(self):
        connection = Mock()
        connection.getresponse.return_value.read.return_value = b"x" * (smoke.LIMIT + 1)
        with patch.object(smoke.ssl, "create_default_context"), \
                patch.object(smoke.http.client, "HTTPSConnection", return_value=connection):
            with self.assertRaisesRegex(smoke.Failure, "HTTP_RESPONSE_LIMIT"):
                smoke.client_request({"host": "opensearch", "port": 9200})
        connection.close.assert_called_once()
        output = io.StringIO()
        with patch.object(smoke.sys, "stdin", SimpleNamespace(buffer=io.BytesIO(b'{"ndjson":"secret-canary"}'))), \
                patch.object(smoke, "client_request", side_effect=ValueError("secret-canary")), \
                patch.object(smoke.signal, "signal"), patch.object(smoke.signal, "alarm"), contextlib.redirect_stdout(output):
            self.assertEqual(smoke.client_main(), 1)
        self.assertEqual(output.getvalue(), '{"unknown":true}\n')


class ModelTests(unittest.TestCase):
    def setUp(self):
        self.engine_ref = smoke.selected_engine(smoke.checked_bytes(smoke.ROOT / smoke.IMAGE_REF))[0]
        self.expected = {"image": self.engine_ref, "networks": {"backend": None}}
        self.model = {"name": "owned", "services": {"opensearch": copy.deepcopy(self.expected)},
            "networks": {"backend": {"name": "owned", "external": True}},
            "volumes": {"opensearch-data": {"name": "owned-data", "external": True}}}

    def validate(self, model):
        smoke.validate_model(model, "opensearch", self.expected, "owned", "owned-data")

    def capture_admin_expected(self):
        with tempfile.TemporaryDirectory() as folder:
            probe = object.__new__(smoke.Probe)
            probe.directory = Path(folder)
            probe.engine_image = self.engine_ref
            probe.project, probe.token = "owned", "token"
            probe.guard, probe.journal = Mock(), Mock()
            probe.call = Mock(side_effect=[json.dumps({"services": {}}), "created"])
            probe.inspect = Mock(return_value={"Id": "a" * 64})
            probe.remember = Mock()
            with patch.object(smoke, "validate_model") as validate:
                probe.compose_create("opensearch-admin", ["-cd", "/operator/security", "-vc", "7"])
            return copy.deepcopy(validate.call_args.args[2])

    def admin_model(self):
        expected = self.capture_admin_expected()
        model = {"name": "owned", "services": {"opensearch-admin": copy.deepcopy(expected)},
            "networks": {"backend": {"name": "owned", "external": True}}}
        return expected, model

    def validate_admin(self, model, expected):
        smoke.validate_model(model, "opensearch-admin", expected, "owned")

    def test_exact_model_and_dangerous_changes(self):
        self.validate(self.model)
        self.model["services"]["opensearch"].update(entrypoint=None, command=None)
        self.validate(self.model)
        changes = [lambda m: m.update(include=["untrusted"]),
            lambda m: m["services"]["opensearch"].update(entrypoint=[]),
            lambda m: m["services"]["opensearch"].update(command=["unexpected"]),
            lambda m: m["services"]["opensearch"].update(ports=["9200:9200"]),
            lambda m: m["services"]["opensearch"].update(env_file=["/host/secret"]),
            lambda m: m["services"]["opensearch"].update(privileged=True),
            lambda m: m["networks"]["backend"].update(name="host-network"),
            lambda m: m["volumes"]["opensearch-data"].update(name="existing-data"),
            lambda m: m["services"].update(postgres={"image": self.engine_ref}),
            lambda m: m["services"].update(unreviewed={"image": self.engine_ref})]
        for change in changes:
            model = copy.deepcopy(self.model)
            change(model)
            with self.subTest(change=change), self.assertRaises(smoke.Failure):
                self.validate(model)

    def test_create_cli_selects_only_one_service_without_unsupported_no_deps(self):
        with tempfile.TemporaryDirectory() as folder:
            probe = object.__new__(smoke.Probe)
            probe.directory = Path(folder)
            probe.engine_image = self.engine_ref
            probe.project, probe.token = "owned", "token"
            probe.guard, probe.journal = Mock(), Mock()
            probe.call = Mock(side_effect=[json.dumps({"services": {}}), "created"])
            probe.inspect = Mock(return_value={"Id": "a" * 64})
            probe.remember = Mock()
            with patch.object(smoke, "validate_model") as validate:
                probe.compose_create("opensearch-admin", ["-cd", "/operator/security", "-vc", "7"])
            validate.assert_called_once()
            argv = probe.call.call_args.args
            self.assertEqual(argv[-5:], ("create", "--pull", "never", "--no-build", "opensearch-admin"))
            self.assertNotIn("--no-deps", argv)
            self.assertNotIn("depends_on", validate.call_args.args[2])
            expected = validate.call_args.args[2]
            self.assertEqual(expected["user"], "1000:1000")
            self.assertEqual(probe.remember.call_args.args[2]["user"], "1000:1000")
            self.assertEqual(expected["cap_drop"], ["ALL"])
            self.assertNotIn("cap_add", expected)
            self.assertEqual(expected["tmpfs"], smoke.ADMIN_TMPFS)
            self.assertEqual(expected["environment"], {"JAVA_TOOL_OPTIONS": smoke.ADMIN_JAVA_TOOL_OPTIONS})
            self.assertEqual(expected["volumes"], [smoke.bind(probe.directory / "secrets/admin", "/operator")])
            self.assertEqual(probe.plain_spec()["user"], "0:0")
            self.assertEqual(probe.plain_spec()["cap_drop"], ["ALL"])
            self.assertNotIn("cap_add", probe.plain_spec())

    def test_admin_model_requires_noexec_broad_tmp_and_private_executable_native_tmp(self):
        expected, model = self.admin_model()
        self.validate_admin(model, expected)
        self.assertEqual(expected["tmpfs"], [
            "/tmp:rw,nosuid,nodev,noexec,size=64m,mode=1777",
            "/securityadmin-native-tmp:rw,nosuid,nodev,exec,size=16m,mode=0700,uid=1000,gid=1000",
        ])
        self.assertEqual(expected["environment"], {"JAVA_TOOL_OPTIONS":
            "-Xms64m -Xmx256m -XX:ActiveProcessorCount=1 "
            "-Djava.io.tmpdir=/securityadmin-native-tmp "
            "-Djna.tmpdir=/securityadmin-native-tmp"})
        self.assertEqual(expected["volumes"], [smoke.bind(model["services"]["opensearch-admin"]["volumes"][0]["source"], "/operator")])
        cases = [
            lambda service: service["tmpfs"].__setitem__(0, "/tmp:rw,nosuid,nodev,exec,size=64m,mode=1777"),
            lambda service: service["tmpfs"].__setitem__(0, "/tmp:rw,nosuid,nodev,size=64m,mode=1777"),
            lambda service: service["tmpfs"].pop(),
            lambda service: service["tmpfs"].__setitem__(1, "/securityadmin-native-tmp:ro,nosuid,nodev,exec,size=16m,mode=0700,uid=1000,gid=1000"),
            lambda service: service["tmpfs"].__setitem__(1, "/securityadmin-native-tmp:rw,suid,nodev,exec,size=16m,mode=0700,uid=1000,gid=1000"),
            lambda service: service["tmpfs"].__setitem__(1, "/securityadmin-native-tmp:rw,nosuid,dev,exec,size=16m,mode=0700,uid=1000,gid=1000"),
            lambda service: service["tmpfs"].__setitem__(1, "/securityadmin-native-tmp:rw,nosuid,nodev,noexec,size=16m,mode=0700,uid=1000,gid=1000"),
            lambda service: service["tmpfs"].__setitem__(1, "/securityadmin-native-tmp:rw,nosuid,nodev,exec,size=64m,mode=0700,uid=1000,gid=1000"),
            lambda service: service["tmpfs"].__setitem__(1, "/securityadmin-native-tmp:rw,nosuid,nodev,exec,size=16m,mode=1777,uid=1000,gid=1000"),
            lambda service: service["tmpfs"].__setitem__(1, "/securityadmin-native-tmp:rw,nosuid,nodev,exec,size=16m,mode=0700,uid=0,gid=0"),
        ]
        for change in cases:
            altered = copy.deepcopy(model)
            change(altered["services"]["opensearch-admin"])
            with self.subTest(change=change), self.assertRaises(smoke.Failure):
                self.validate_admin(altered, expected)

    def test_admin_java_temp_paths_are_only_dedicated_native_tmp(self):
        expected, model = self.admin_model()
        for value in (
            "-Xms64m -Xmx256m -XX:ActiveProcessorCount=1 -Djava.io.tmpdir=/tmp -Djna.tmpdir=/tmp",
            "-Xms64m -Xmx256m -XX:ActiveProcessorCount=1 -Djava.io.tmpdir=/operator -Djna.tmpdir=/securityadmin-native-tmp",
            "-Xms64m -Xmx256m -XX:ActiveProcessorCount=1 -Djava.io.tmpdir=/securityadmin-native-tmp -Djna.tmpdir=/operator",
            "-Xms64m -Xmx256m -XX:ActiveProcessorCount=1 -Djava.io.tmpdir=/securityadmin-native-tmp",
            "-Xms64m -Xmx256m -XX:ActiveProcessorCount=1 -Djava.io.tmpdir=/securityadmin-native-tmp -Djna.tmpdir=/securityadmin-native-tmp -Djava.io.tmpdir=/tmp",
            "-Xms32m -Xmx256m -XX:ActiveProcessorCount=1 -Djava.io.tmpdir=/securityadmin-native-tmp -Djna.tmpdir=/securityadmin-native-tmp",
        ):
            altered = copy.deepcopy(model)
            altered["services"]["opensearch-admin"]["environment"]["JAVA_TOOL_OPTIONS"] = value
            with self.subTest(value=value), self.assertRaises(smoke.Failure):
                self.validate_admin(altered, expected)

        altered = copy.deepcopy(model)
        altered["services"]["opensearch-admin"]["volumes"][0]["read_only"] = False
        with self.assertRaises(smoke.Failure):
            self.validate_admin(altered, expected)

    def test_application_services_cannot_receive_admin_native_tmpfs(self):
        expected, model = self.admin_model()
        model["services"]["api"] = {"tmpfs": [smoke.ADMIN_TMPFS[1]]}
        with self.assertRaisesRegex(smoke.Failure, "MODEL_SERVICES"):
            self.validate_admin(model, expected)

        model["services"].pop("api")
        model["services"]["postgres"] = {
            "profiles": ["excluded-from-opensearch-test"],
            "tmpfs": [smoke.ADMIN_TMPFS[1]],
        }
        with self.assertRaisesRegex(smoke.Failure, "MODEL_EXCLUDED_SERVICE"):
            self.validate_admin(model, expected)

    def test_inactive_platform_services_cannot_keep_mounts(self):
        self.model["services"]["postgres"] = {"profiles": ["excluded-from-opensearch-test"]}
        self.validate(self.model)
        for field, value in (("volumes", ["host-data:/data"]), ("tmpfs", ["/securityadmin-native-tmp:rw"])):
            self.model["services"]["postgres"][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(smoke.Failure, "EXCLUDED_SERVICE"):
                self.validate(self.model)
            self.model["services"]["postgres"].pop(field)


class OwnershipTests(unittest.TestCase):
    def test_native_admin_ownership_keeps_separate_root_client_key_and_hash_guard(self):
        for corrupt in (False, True):
            with self.subTest(corrupt=corrupt), tempfile.TemporaryDirectory() as folder, contextlib.redirect_stdout(io.StringIO()):
                probe = smoke.Probe(Path(folder), SimpleNamespace(), Mock())
                probe.helper_image = "sha256:" + "h" * 64
                root = probe.directory / "secrets"
                for name in ("admin/admin-key.pem", "client/admin-key.pem", "opensearch-certs/node-key.pem"):
                    probe.write("secrets/" + name, "synthetic-key")
                probe.write("secrets/admin/security/roles.yml", "native-roles")
                probe.write("secrets/opensearch.yml", "native-config")
                probe.create_plain = Mock()
                probe.finish = Mock(return_value=(0, ""))
                probe.retire = Mock()
                probe.permissions()
                helper = probe.create_plain.call_args.args[1]
                self.assertEqual(helper["image"], probe.helper_image)
                self.assertEqual(helper["user"], "0:0")
                self.assertEqual(helper["cap_drop"], ["ALL"])
                self.assertEqual(helper["cap_add"], ["CHOWN", "FOWNER"])
                self.assertEqual(helper["network_mode"], "none")
                self.assertEqual(helper["volumes"], [smoke.bind(root, "/fixture") | {"read_only": False},
                    smoke.bind(probe.directory / "generated-inputs.json", "/expected.json")])
                script = helper["command"][1].replace("'/fixture'", repr(str(root))).replace(
                    "'/expected.json'", repr(str(probe.directory / "generated-inputs.json")))
                owners = {}
                def chown(path, uid, gid):
                    path = Path(path)
                    self.assertFalse(owners.get(path.parent) == (1000, 1000)
                        and path.parent.stat().st_mode & 0o777 == 0o700, "no DAC bypass to private parent")
                    owners[path] = (uid, gid)
                if corrupt:
                    (root / "admin/security/roles.yml").chmod(0o600)
                    (root / "admin/security/roles.yml").write_text("changed-after-freeze")
                with patch.object(smoke.os, "chown", side_effect=chown):
                    if corrupt:
                        with self.assertRaises(AssertionError):
                            exec(script, {})
                        self.assertNotIn((1000, 1000), owners.values())
                        continue
                    exec(script, {})
                for path in [root / "admin", *(root / "admin").rglob("*")]:
                    self.assertEqual(owners[path], (1000, 1000))
                    self.assertEqual(path.stat().st_mode & 0o777, 0o700 if path.is_dir() else 0o600)
                for path in [root / "client", root / "client/admin-key.pem"]:
                    self.assertEqual(owners[path], (0, 0))
                    self.assertEqual(path.stat().st_mode & 0o777, 0o700 if path.is_dir() else 0o600)
                self.assertNotEqual((root / "admin/admin-key.pem").stat().st_ino, (root / "client/admin-key.pem").stat().st_ino)
                self.assertEqual({str(p.relative_to(root)): smoke.digest(p.read_bytes()) for p in root.rglob("*") if p.is_file()}, probe.generated)
                probe.permissions(restore=True)
                restore = probe.create_plain.call_args.args[1]
                self.assertEqual(restore["image"], probe.helper_image)
                self.assertEqual(restore["volumes"], [smoke.bind(root, "/fixture") | {"read_only": False}])
                with patch.object(smoke.os, "chown", side_effect=chown):
                    exec(restore["command"][1].replace("'/fixture'", repr(str(root))), {})
                self.assertTrue(all(owner == (smoke.os.getuid(), smoke.os.getgid()) for owner in owners.values()))

    def test_network_rejects_foreign_members_and_egress(self):
        network = dict(Name="owned", Labels={smoke.OWNER: "token"}, Internal=True, Driver="bridge", Scope="local",
            IPAM={"Driver": "default"}, Containers={"known": {}})
        smoke.owned_network(network, "owned", "token", ["known"])
        for key, value in (("Internal", False), ("Driver", "host"), ("Labels", {}),
                ("Containers", {"foreign": {}}), ("Options", {"device": "host"})):
            with self.subTest(key=key), self.assertRaises(smoke.Failure):
                smoke.owned_network(network | {key: value}, "owned", "token", ["known"])

    def test_volume_rejects_unowned_driver_and_host_bind_options(self):
        volume = dict(Name="owned-data", Labels={smoke.OWNER: "token"}, Driver="local", Scope="local")
        smoke.owned_volume(volume, "owned-data", "token")
        for key, value in (("Name", "existing-data"), ("Labels", {}), ("Driver", "nfs"), ("Options", {"device": "/"})):
            with self.subTest(key=key), self.assertRaises(smoke.Failure):
                smoke.owned_volume(volume | {key: value}, "owned-data", "token")

    def test_runtime_rejects_extra_mounts_env_ports_namespaces_and_root_node(self):
        image_id = "sha256:" + "b" * 64
        image = {"Id": image_id, "Config": {"Env": ["PATH=/usr/bin"], "Entrypoint": ["node"], "Cmd": []}}
        spec = dict(image=image_id, user="1000:1000", environment={}, restart="unless-stopped",
            mem_limit=1024, cpus=1, pids_limit=32, networks={"backend": {"aliases": ["opensearch"]}},
            volumes=[smoke.bind("/owned/node", "/certs"),
                {"type": "volume", "source": "owned-data", "target": "/data"}])
        item = {"Name": "/owned-node", "Image": image_id,
            "Config": {"Labels": {smoke.OWNER: "token"}, "User": "1000:1000", "Entrypoint": ["node"], "Cmd": [], "Env": ["PATH=/usr/bin"]},
            "HostConfig": {"SecurityOpt": smoke.SECURITY, "IpcMode": "private", "ReadonlyRootfs": False,
                "Memory": 1024, "PidsLimit": 32, "NanoCpus": 1000000000, "RestartPolicy": {"Name": "unless-stopped"},
                "LogConfig": {"Type": "local", "Config": smoke.LOGGING["options"]}, "NetworkMode": "owned"},
            "NetworkSettings": {"Networks": {"owned": {"Aliases": ["opensearch"]}}},
            "Mounts": [{"Type": "bind", "Source": "/owned/node", "Destination": "/certs", "RW": False},
                {"Type": "volume", "Name": "owned-data", "Destination": "/data", "RW": True}]}
        def validate(value):
            smoke.runtime_guard(value, spec, image, "owned", "token", "owned-node")
        validate(item)
        item["HostConfig"]["Binds"] = ["owned-data:/data:rw"]
        validate(item)
        changes = [lambda v: v["Config"].update(User="0:0"),
            lambda v: v["HostConfig"].update(Binds=["foreign-data:/data:rw"]),
            lambda v: v["HostConfig"].update(Binds=["owned-data:/data:ro"]),
            lambda v: v["HostConfig"].update(Binds=["owned-data:/data:rw", "/host:/extra:ro"]),
            lambda v: v["Config"]["Env"].append("AWS_ACCESS_KEY_ID=canary"),
            lambda v: v["HostConfig"].update(PortBindings={"9200/tcp": [{"HostPort": "9200"}]}),
            lambda v: v["HostConfig"].update(NetworkMode="host"),
            lambda v: v["HostConfig"].update(Privileged=True),
            lambda v: v["HostConfig"].update(CapAdd=["SYS_ADMIN"]),
            lambda v: v["NetworkSettings"]["Networks"].update(bridge={}),
            lambda v: v["Mounts"][0].update(RW=True),
            lambda v: v["Mounts"][0].update(Source="/host/admin"),
            lambda v: v["Mounts"].append({"Type": "bind", "Source": "/var/run/docker.sock", "Destination": "/socket", "RW": True})]
        for change in changes:
            value = copy.deepcopy(item)
            change(value)
            with self.subTest(change=change), self.assertRaises(smoke.Failure):
                validate(value)
        spec["cap_add"] = ["CHOWN", "FOWNER"]
        for caps in (["CHOWN", "FOWNER"], ["CAP_CHOWN", "CAP_FOWNER"]):
            item["HostConfig"]["CapAdd"] = caps
            validate(item)
        item["HostConfig"]["CapAdd"] = ["CAP_CHOWN", "CAP_SYS_ADMIN"]
        with self.assertRaises(smoke.Failure):
            validate(item)


class SecurityAdminDiagnosticTests(unittest.TestCase):
    def invoke(self, raw, code=1, identity="admin", offline=False, finish_side_effect=None):
        with tempfile.TemporaryDirectory() as folder:
            output = io.StringIO()
            args = SimpleNamespace(hostname="opensearch", port=9200, cluster_name="owned-cluster")
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                probe = smoke.Probe(Path(folder), args, Mock())
                probe.target = Mock()
                probe.compose_create = Mock()
                probe.finish = (Mock(side_effect=finish_side_effect) if finish_side_effect is not None
                    else Mock(return_value=(code, raw)))
                probe.retire = Mock()
                caught = None
                try:
                    probe.admin(identity=identity, offline=offline)
                except Exception as error:
                    caught = error
            case = "offline" if offline else identity
            private_path = Path(folder) / smoke.ADMIN_OUTPUT_FILES[case]
            result_lines = [line for line in output.getvalue().splitlines()
                if line.startswith("SECURITYADMIN_RESULT ")]
            facts = json.loads(result_lines[-1].split(" ", 1)[1]) if result_lines else None
            return {
                "error": caught,
                "stdout": output.getvalue(),
                "facts": facts,
                "private": private_path.read_text() if private_path.is_file() else None,
                "private_mode": private_path.stat().st_mode & 0o777 if private_path.is_file() else None,
                "evidence": (Path(folder) / "evidence.json").read_text(),
                "target_calls": probe.target.call_count,
                "compose_calls": probe.compose_create.call_count,
                "finish_calls": probe.finish.call_count,
                "retire_calls": probe.retire.call_count,
                "retire_args": [call.args for call in probe.retire.call_args_list],
            }

    def test_nonzero_native_result_is_retained_with_safe_facts(self):
        observed = self.invoke("native failure without a classified marker\n", code=7)
        self.assertTrue(isinstance(observed["error"], smoke.Failure))
        self.assertEqual(str(observed["error"]), "SECURITYADMIN_FAILED_RETAINED")
        self.assertTrue(observed["private"] == "native failure without a classified marker\n")
        self.assertTrue(observed["private_mode"] == 0o600)
        self.assertEqual(observed["facts"]["case"], "admin")
        self.assertEqual(observed["facts"]["exit_code"], 7)
        self.assertTrue(observed["facts"]["unclassified_failure"])
        self.assertEqual(observed["compose_calls"], 1)
        self.assertEqual(observed["finish_calls"], 1)
        self.assertEqual(observed["retire_calls"], 0)

    def test_zero_exit_with_each_original_marker_still_fails_and_is_distinguished(self):
        for marker in smoke.ADMIN_MARKERS:
            with self.subTest(marker=marker):
                observed = self.invoke("native output " + marker + "\n", code=0)
                self.assertEqual(str(observed["error"]), "SECURITYADMIN_FAILED_RETAINED")
                self.assertEqual(observed["facts"]["exit_code"], 0)
                self.assertTrue(observed["facts"]["markers"][marker])
                self.assertEqual(observed["retire_calls"], 0)

    def test_clean_native_result_retires_owned_admin_once(self):
        observed = self.invoke("Security Admin v7\nDone with success\n", code=0)
        self.assertIsNone(observed["error"])
        self.assertTrue(observed["facts"]["done_with_success"])
        self.assertEqual(observed["retire_calls"], 1)
        self.assertEqual(observed["retire_args"], [("opensearch-admin",)])

    def test_offline_validation_has_its_own_case_without_online_success_marker(self):
        observed = self.invoke("config.yml OK\n", code=0, offline=True)
        self.assertIsNone(observed["error"])
        self.assertEqual(observed["facts"]["case"], "offline")
        self.assertFalse(observed["facts"]["done_with_success"])
        self.assertEqual(observed["target_calls"], 0)
        self.assertEqual(observed["retire_calls"], 1)
        self.assertEqual(observed["retire_args"], [("opensearch-admin",)])

    def test_certificate_rejections_keep_specific_predicate_and_generic_failure_does_not_pass(self):
        cases = {
            "node": "ERR: certificate is not an admin user\nSeems you use a node certificate.\n",
            "unregistered": "ERR: CN=unregistered is not an admin user\n",
        }
        for identity, raw in cases.items():
            with self.subTest(identity=identity):
                observed = self.invoke(raw, code=1, identity=identity)
                self.assertIsNone(observed["error"])
                self.assertEqual(observed["facts"]["case"], identity)
                self.assertEqual(observed["retire_calls"], 1)
        observed = self.invoke("connection timed out\n", code=1, identity="node")
        self.assertEqual(str(observed["error"]), "ADMIN_REJECTION_UNATTRIBUTED")
        self.assertEqual(observed["retire_calls"], 0)

    def test_secret_bearing_native_output_is_private_and_absent_from_public_facts(self):
        raw = ("ERR: secret-canary\npassword=secret-canary\n"
            "$2b$12$aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
            "-----BEGIN PRIVATE KEY-----\nsecret-canary\n-----END PRIVATE KEY-----\n"
            "\x1b[31mERROR\x1b[0m\n"
            '{"password":"secret-canary","type":"arbitrary-type"}\n')
        observed = self.invoke(raw, code=1)
        self.assertTrue(observed["private"] == raw)
        self.assertTrue(observed["private_mode"] == 0o600)
        public_values = (observed["stdout"], json.dumps(observed["facts"]), observed["evidence"])
        for value in public_values:
            self.assertFalse("secret-canary" in value)
            self.assertFalse("BEGIN PRIVATE KEY" in value)

    def test_diagnostic_matching_returns_only_fixed_types_and_exception_labels(self):
        raw = ("SUCC: Configuration for 'config' created or updated\n"
            "FAIL: Configuration for 'roles' failed for unknown reasons\n"
            "SUCC: Configuration for 'arbitrary-type' created or updated\n"
            "FAIL: Configuration for 'arbitrary-type' failed because of suffix-canary\n"
            "java.lang.UnlistedException: exception-suffix-canary\n"
            "OpenSearch Security Version: \"3.8.0.0\"\n"
            "OpenSearchStatusException\nDone with failures\n")
        observed = self.invoke(raw, code=1)
        facts = observed["facts"]
        self.assertEqual(facts["uploaded_types"], ["config"])
        self.assertEqual(facts["failed_types"], ["roles"])
        self.assertEqual(facts["exception_labels"], ["OpenSearchStatusException"])
        self.assertEqual(facts["security_plugin_versions"], ["3.8.0.0"])
        self.assertTrue(facts["done_with_failures"])
        public = observed["stdout"] + json.dumps(facts) + observed["evidence"]
        self.assertFalse("arbitrary-type" in public)
        self.assertFalse("suffix-canary" in public)
        self.assertFalse("exception-suffix-canary" in public)

    def test_known_upstream_error_prefixes_use_only_closed_categories(self):
        cases = {
            "parse_failure": "ERR: Parsing failed.  Reason: secret-canary\n",
            "socket_unreachable": "ERR: Seems there is no OpenSearch running on secret-canary:9200 - Will exit\n",
            "admin_not_registered": "ERR: \\\"CN=secret-canary,...\\\" is not an admin user\n",
            "node_certificate_as_admin": "ERR: Seems you use a node certificate which is also an admin certificate\n",
            "unexpected_exception": "ERR: An unexpected NullPointerException occurred: secret-canary\n",
            "cluster_state_fetch_failed": "ERR: Cannot retrieve cluster state due to: secret-canary\n",
            "cluster_state_timeout": "ERR: Timed out while waiting for a green or yellow cluster state.\n",
            "security_index_state_timeout": "ERR: Timed out while waiting for .opendistro_security index state.\n",
            "security_index_red": "ERR: .opendistro_security index state is RED.\n",
            "security_index_state_failed": "ERR: Cannot retrieve .opendistro_security index state state due to secret-canary.\n",
            "invalid_config_type": "ERR: Invalid type 'secret-canary'\n",
            "config_upload_failed": "ERR: cannot upload configuration, see errors above\n",
        }
        self.assertEqual(tuple(cases), smoke.ADMIN_ERROR_CATEGORIES)
        for category, raw in cases.items():
            with self.subTest(category=category):
                observed = self.invoke(raw, code=1)
                self.assertEqual(observed["facts"]["error_categories"], [category])
                public = observed["stdout"] + json.dumps(observed["facts"]) + observed["evidence"]
                self.assertNotIn("secret-canary", public)

    def test_admin_dn_rejection_category_hides_returned_dn(self):
        observed = self.invoke(
            'Connected as "CN=secret-canary,..."\nERR: "CN=secret-canary,..." is not an admin user\n', code=255)
        self.assertEqual(observed["facts"]["error_categories"], ["admin_not_registered"])
        self.assertTrue(observed["facts"]["connected_as_seen"])
        self.assertFalse(observed["facts"]["unclassified_failure"])
        self.assertNotIn("secret-canary", observed["stdout"] + json.dumps(observed["facts"]) + observed["evidence"])

    def test_node_certificate_category_is_exact_and_fixed(self):
        observed = self.invoke(
            "ERR: Seems you use a node certificate which is also an admin certificate\n", code=255)
        self.assertEqual(observed["facts"]["error_categories"], ["node_certificate_as_admin"])
        self.assertIsNone(observed["facts"]["unexpected_exception_class"])

    def test_cluster_state_failure_records_contact_progress_without_cluster_text(self):
        observed = self.invoke(
            "Contacting opensearch cluster 'secret-canary' and wait for YELLOW clusterstate ...\n"
            "ERR: Cannot retrieve cluster state due to: secret-canary\n", code=255)
        facts = observed["facts"]
        self.assertEqual(facts["error_categories"], ["cluster_state_fetch_failed"])
        self.assertTrue(facts["cluster_contact_seen"])
        self.assertFalse(facts["cluster_identity_seen"])
        self.assertNotIn("secret-canary", observed["stdout"] + json.dumps(facts) + observed["evidence"])

    def test_unexpected_exception_extracts_only_simple_class(self):
        observed = self.invoke(
            "ERR: An unexpected NullPointerException occurred: secret-canary\nTrace:\nsecret-canary\n", code=255)
        facts = observed["facts"]
        self.assertEqual(facts["error_categories"], ["unexpected_exception"])
        self.assertEqual(facts["unexpected_exception_class"], "NullPointerException")
        self.assertFalse(facts["unclassified_failure"])
        public = observed["stdout"] + json.dumps(facts) + observed["evidence"]
        self.assertNotIn("secret-canary", public)
        malicious = self.invoke(
            "ERR: An unexpected BadClass-suffix-canary occurred: secret-canary\n", code=255)
        self.assertEqual(malicious["facts"]["error_categories"], [])
        self.assertIsNone(malicious["facts"]["unexpected_exception_class"])
        self.assertTrue(malicious["facts"]["unclassified_failure"])
        self.assertNotIn("secret-canary", malicious["stdout"] + json.dumps(malicious["facts"]) + malicious["evidence"])

    def test_progress_markers_are_boolean_only(self):
        observed = self.invoke(
            'Connected as "secret-canary"\n'
            "Contacting opensearch cluster 'secret-canary' and wait for YELLOW clusterstate ...\n"
            "Clustername: secret-canary\n"
            ".opendistro_security index does not exists, attempt to create it ...\n"
            "Populate config from /secret-canary\n", code=0)
        facts = observed["facts"]
        for field in ("connected_as_seen", "cluster_contact_seen", "cluster_identity_seen",
                "populate_config_seen", "security_index_create_seen"):
            self.assertTrue(facts[field])
        self.assertNotIn("secret-canary", observed["stdout"] + json.dumps(facts) + observed["evidence"])

    def test_config_upload_category_retains_known_failed_type(self):
        observed = self.invoke(
            "FAIL: Configuration for 'roles' failed because of secret-canary\n"
            "ERR: cannot upload configuration, see errors above\n", code=255)
        self.assertEqual(observed["facts"]["error_categories"], ["config_upload_failed"])
        self.assertEqual(observed["facts"]["failed_types"], ["roles"])
        self.assertFalse(observed["facts"]["unclassified_failure"])
        self.assertNotIn("secret-canary", observed["stdout"] + json.dumps(observed["facts"]) + observed["evidence"])

    def test_unknown_err_stays_unclassified(self):
        observed = self.invoke("ERR: A future native path: secret-canary\n", code=255)
        self.assertEqual(observed["facts"]["error_categories"], [])
        self.assertTrue(observed["facts"]["unclassified_failure"])
        self.assertNotIn("secret-canary", observed["stdout"] + json.dumps(observed["facts"]) + observed["evidence"])

    def test_unknown_native_command_outcome_has_no_fabricated_result_or_retirement(self):
        observed = self.invoke("transport details must stay private\n", finish_side_effect=smoke.Failure("COMMAND_UNCONFIRMED"))
        self.assertEqual(str(observed["error"]), "COMMAND_UNCONFIRMED")
        self.assertIsNone(observed["facts"])
        self.assertIsNone(observed["private"])
        self.assertFalse("SECURITYADMIN_RESULT" in observed["stdout"])
        self.assertEqual(observed["retire_calls"], 0)

    def test_admin_cases_and_diagnostic_inputs_are_fixed(self):
        for case in ("offline", "admin", "node", "unregistered"):
            self.assertTrue(case in smoke.ADMIN_OUTPUT_FILES)
        with self.assertRaisesRegex(smoke.Failure, "ADMIN_DIAGNOSTIC_CASE"):
            smoke.admin_diagnostic("arbitrary", 1, "failure")
        with self.assertRaisesRegex(smoke.Failure, "ADMIN_DIAGNOSTIC_INPUT"):
            smoke.admin_diagnostic("admin", True, "failure")


class OperatorDiagnosticTests(unittest.TestCase):
    def result(self, code, outcome, phase, trace, output=None):
        return {
            "code": code,
            "trace": [list(entry) for entry in trace],
            "output": output if output is not None else f"opensearch outcome={outcome} phase={phase}\n",
        }

    def invoke(self, mode, success, result):
        probe = object.__new__(smoke.Probe)
        probe.args = SimpleNamespace(cluster_name="owned-cluster")
        probe.client = Mock(return_value=result)
        probe.event = Mock()
        output = io.StringIO()
        error = None
        with contextlib.redirect_stdout(output):
            try:
                probe.operator(mode, success)
            except smoke.Failure as caught:
                error = caught
        lines = [line for line in output.getvalue().splitlines()
            if line.startswith("OPENSEARCH_OPERATOR_RESULT ")]
        facts = json.loads(lines[-1].split(" ", 1)[1]) if lines else {}
        return probe, output.getvalue(), facts, error

    @staticmethod
    def target_trace():
        return [["GET", "/"]]

    @classmethod
    def verify_trace(cls):
        return cls.target_trace() + [
            ["GET", "/product-listings?flat_settings=true"],
            ["GET", "/user_search_filters?flat_settings=true"],
            ["GET", "/_search/pipeline/hybrid-search-pipeline"],
        ]

    @classmethod
    def absence_trace(cls):
        return cls.verify_trace()

    @classmethod
    def initialize_trace(cls):
        return cls.absence_trace() + [
            ["PUT", "/product-listings"],
            ["PUT", "/user_search_filters"],
            ["PUT", "/_search/pipeline/hybrid-search-pipeline"],
        ] + cls.verify_trace()[1:]

    def test_parser_accepts_fixed_status_and_all_phases(self):
        traces = self.verify_trace()
        for phase in sorted(smoke.OPERATOR_PHASES):
            with self.subTest(phase=phase):
                facts = smoke.operator_diagnostic("verify", False,
                    self.result(1, "no-writes-attempted", phase, traces))
                self.assertTrue(facts["status_parsed"])
                self.assertEqual(facts["phase"], phase)
                self.assertEqual(facts["outcome"], "no-writes-attempted")
                self.assertFalse(facts["writes_may_have_occurred"])

    def test_expected_missing_verify_failure_emits_safe_result_and_passes(self):
        result = self.result(1, "no-writes-attempted", "verify", self.verify_trace()[:2])
        probe, output, facts, error = self.invoke("verify", False, result)
        self.assertIsNone(error)
        self.assertEqual(facts["exit_code"], 1)
        self.assertEqual(facts["expected_exit_code"], 1)
        self.assertTrue(facts["matches_expected_exit"])
        self.assertTrue(facts["status_parsed"])
        self.assertEqual(facts["outcome"], "no-writes-attempted")
        self.assertEqual(facts["phase"], "verify")
        self.assertFalse(facts["writes_may_have_occurred"])
        self.assertEqual(facts["trace_steps"], ["target", "read-product-listings"])
        self.assertEqual(facts["write_trace_count"], 0)
        self.assertTrue(facts["trace_valid"])
        self.assertEqual(probe.client.call_count, 1)
        self.assertEqual(probe.event.call_args_list[0].args, ("operator-result",))
        self.assertEqual(probe.event.call_args_list[0].kwargs, facts)
        self.assertEqual(output.count("OPENSEARCH_OPERATOR_RESULT "), 1)

    def test_expected_partial_state_initialize_failure_is_get_only(self):
        result = self.result(1, "no-writes-attempted", "absence", self.absence_trace())
        probe, output, facts, error = self.invoke("initialize-fresh", False, result)
        self.assertIsNone(error)
        self.assertEqual(facts["phase"], "absence")
        self.assertEqual(facts["trace_count"], 4)
        self.assertEqual(facts["write_trace_count"], 0)
        self.assertEqual(facts["last_trace_step"], "read-pipeline")
        self.assertTrue(facts["trace_valid"])
        self.assertEqual(probe.client.call_count, 1)
        self.assertNotIn("secret-canary", output)

    def test_unexpected_target_failure_emits_before_retained_verdict(self):
        result = self.result(1, "no-writes-attempted", "target", self.target_trace())
        probe, output, facts, error = self.invoke("initialize-fresh", True, result)
        self.assertIsInstance(error, smoke.Failure)
        self.assertEqual(str(error), "OPERATOR_RESULT_RETAINED")
        self.assertFalse(facts["matches_expected_exit"])
        self.assertEqual(facts["phase"], "target")
        self.assertFalse(facts["writes_may_have_occurred"])
        self.assertEqual(probe.event.call_count, 1)
        self.assertIn("OPENSEARCH_OPERATOR_RESULT ", output)
        self.assertIn('"phase": "target"', output)

    def test_unexpected_write_failures_report_partial_risk_and_retain(self):
        cases = (
            ("create-product-listings", self.absence_trace() + [["PUT", "/product-listings"]], 1),
            ("create-user_search_filters", self.absence_trace() + [
                ["PUT", "/product-listings"], ["PUT", "/user_search_filters"]], 2),
            ("create-pipeline", self.absence_trace() + [
                ["PUT", "/product-listings"], ["PUT", "/user_search_filters"],
                ["PUT", "/_search/pipeline/hybrid-search-pipeline"]], 3),
        )
        for phase, trace, write_count in cases:
            with self.subTest(phase=phase):
                result = self.result(1, "partial-or-unknown-do-not-retry", phase, trace)
                probe, output, facts, error = self.invoke("initialize-fresh", True, result)
                self.assertIsInstance(error, smoke.Failure)
                self.assertEqual(str(error), "OPERATOR_RESULT_RETAINED")
                self.assertTrue(facts["writes_may_have_occurred"])
                self.assertEqual(facts["phase"], phase)
                self.assertEqual(facts["write_trace_count"], write_count)
                self.assertEqual(probe.client.call_count, 1)
                self.assertEqual(probe.event.call_count, 1)
                self.assertNotIn("secret-canary", output)

    def test_unexpected_verify_failure_after_writes_is_retained(self):
        trace = self.initialize_trace() + [["GET", "/product-listings?flat_settings=true"]]
        result = self.result(1, "partial-or-unknown-do-not-retry", "verify", trace)
        probe, output, facts, error = self.invoke("initialize-fresh", True, result)
        self.assertIsInstance(error, smoke.Failure)
        self.assertEqual(str(error), "OPERATOR_RESULT_RETAINED")
        self.assertTrue(facts["writes_may_have_occurred"])
        self.assertEqual(facts["phase"], "verify")
        self.assertEqual(facts["write_trace_count"], 3)
        self.assertEqual(probe.client.call_count, 1)
        self.assertEqual(probe.event.call_count, 1)
        self.assertNotIn("secret-canary", output)

    def test_clean_initialize_success_is_verified(self):
        result = self.result(0, "verified", "verify", self.initialize_trace())
        probe, output, facts, error = self.invoke("initialize-fresh", True, result)
        self.assertIsNone(error)
        self.assertEqual(facts["exit_code"], 0)
        self.assertTrue(facts["matches_expected_exit"])
        self.assertTrue(facts["status_parsed"])
        self.assertEqual(facts["outcome"], "verified")
        self.assertEqual(facts["phase"], "verify")
        self.assertFalse(facts["writes_may_have_occurred"])
        self.assertEqual(facts["write_trace_count"], 3)
        self.assertEqual(probe.client.call_count, 1)
        self.assertEqual(probe.event.call_args_list[0].args, ("operator-result",))
        self.assertEqual(output.count("OPENSEARCH_OPERATOR_RESULT "), 1)

    def test_malformed_status_is_fixed_and_not_published(self):
        valid = "opensearch outcome=verified phase=verify\n"
        cases = (
            "",
            valid + valid,
            "opensearch outcome=secret-canary phase=verify\n",
            "opensearch outcome=verified phase=secret-canary\n",
            valid + "request-body-secret-canary\n",
        )
        for output in cases:
            with self.subTest(output=output):
                result = self.result(0, "verified", "verify", self.verify_trace(), output=output)
                facts = smoke.operator_diagnostic("verify", True, result)
                self.assertFalse(facts["status_parsed"])
                self.assertNotIn("secret-canary", json.dumps(facts))

        result = self.result(0, "verified", "verify", self.verify_trace(), output=valid + "secret-canary\n")
        _probe, output, facts, error = self.invoke("verify", True, result)
        self.assertIsInstance(error, smoke.Failure)
        self.assertEqual(str(error), "OPERATOR_STATUS_UNPARSED")
        self.assertFalse(facts["status_parsed"])
        self.assertNotIn("secret-canary", output)

    def test_unknown_trace_is_not_printed_and_fails_closed(self):
        result = self.result(1, "no-writes-attempted", "target",
            self.target_trace() + [["DELETE", "/secret-canary"]])
        probe, output, facts, error = self.invoke("verify", False, result)
        self.assertIsInstance(error, smoke.Failure)
        self.assertEqual(str(error), "OPERATOR_TRACE_INVALID")
        self.assertFalse(facts["trace_valid"])
        self.assertEqual(facts["trace_steps"], ["target"])
        self.assertEqual(facts["trace_count"], 2)
        self.assertEqual(facts["write_trace_count"], 0)
        self.assertEqual(probe.client.call_count, 1)
        self.assertNotIn("DELETE", output)
        self.assertNotIn("secret-canary", output)

    def test_result_shape_and_types_are_strict(self):
        good = self.result(1, "no-writes-attempted", "target", self.target_trace())
        bad_results = (
            None,
            [],
            {"code": 1, "trace": [], "output": "", "extra": "secret-canary"},
            {"code": True, "trace": good["trace"], "output": good["output"]},
            {"code": 1, "trace": {}, "output": good["output"]},
            {"code": 1, "trace": good["trace"], "output": None},
        )
        for bad in bad_results:
            with self.subTest(bad=bad), self.assertRaisesRegex(smoke.Failure, "OPERATOR_DIAGNOSTIC_INPUT"):
                smoke.operator_diagnostic("verify", False, bad)
        with self.assertRaisesRegex(smoke.Failure, "OPERATOR_DIAGNOSTIC_MODE"):
            smoke.operator_diagnostic("other", False, good)
        with self.assertRaisesRegex(smoke.Failure, "OPERATOR_DIAGNOSTIC_EXPECTED_SUCCESS"):
            smoke.operator_diagnostic("verify", 1, good)

    def test_request_bodies_are_not_in_public_diagnostic(self):
        result = self.result(1, "partial-or-unknown-do-not-retry", "create-product-listings",
            self.absence_trace() + [["PUT", "/product-listings"]])
        _probe, output, facts, error = self.invoke("initialize-fresh", True, result)
        self.assertEqual(str(error), "OPERATOR_RESULT_RETAINED")
        self.assertTrue(facts["writes_may_have_occurred"])
        self.assertNotIn("secret-canary", output)
        self.assertNotIn("body", json.dumps(facts))


class EvidenceTests(unittest.TestCase):
    def test_bulk_denials_require_every_exact_item_not_just_http200(self):
        targets = [("delete", "product-listings", "product-0"), ("index", "user_search_filters", "filter-0"),
            ("index", "fixture-cross-index", "sentinel")]
        good = {"status": 200, "body": {"errors": True, "items": [{op: {"_index": index, "_id": identifier,
            "status": 403, "error": {"type": "security_exception", "reason": "secret-canary"}}} for op, index, identifier in targets]}}
        self.assertTrue(smoke.bulk_denials(good, targets))
        changes = [lambda r: r.update(status=403), lambda r: r.update(body=[]),
            lambda r: r["body"].update(errors=False), lambda r: r["body"].update(items=[]),
            lambda r: r["body"]["items"].append(r["body"]["items"][0]),
            lambda r: r["body"]["items"][0].update(index={}),
            lambda r: r["body"]["items"][1]["index"].update(status=201),
            lambda r: r["body"]["items"][1]["index"].update(status=401),
            lambda r: r["body"]["items"][1]["index"].update(error={"type": "parse_exception"}),
            lambda r: r["body"]["items"][2]["index"].update(_index="product-listings"),
            lambda r: r["body"]["items"][2]["index"].update(_id="other"),
            lambda r: r["body"]["items"].reverse()]
        for change in changes:
            bad = copy.deepcopy(good)
            change(bad)
            self.assertFalse(smoke.bulk_denials(bad, targets))
        self.assertFalse(smoke.bulk_denials({"unknown": True}, targets))
        self.assertFalse(smoke.bulk_denials({"status": 200, "body": {"errors": True, "items": []}}, []))
        self.assertNotIn("secret-canary", json.dumps(smoke.safe_result(good)))

    def test_projector_bulk_and_explicit_create_are_inside_unchanged_check(self):
        probe = object.__new__(smoke.Probe)
        probe.request, probe.event, probe.write = Mock(), Mock(), Mock()
        probe.snapshot = Mock(return_value="same-full-snapshot")
        cases = []
        def client(case):
            cases.append(case)
            if case["path"] == "/_plugins/_security/api/roles/forbidden":
                self.assertEqual(case["method"], "PUT")
                return {"status": 403, "body": {"status": "FORBIDDEN", "message": "No permission to access REST API: management disabled"}}
            if "ndjson" not in case:
                return {"status": 403, "body": {"error": {"type": "security_exception"}}}
            self.assertEqual(case["path"], "/_bulk?refresh=true")
            self.assertEqual(case["method"], "POST")
            self.assertTrue(case["ndjson"].endswith("\n"))
            lines = [json.loads(line) for line in case["ndjson"].splitlines()]
            self.assertEqual(len(lines), 5)
            own, other = smoke.INDICES if case["user"] == smoke.USERS[1] else smoke.INDICES[::-1]
            self.assertEqual(lines[0]["delete"]["_index"], own)
            self.assertEqual(lines[1]["index"]["_index"], other)
            self.assertEqual(lines[3]["index"], {"_index": "fixture-cross-index", "_id": "sentinel"})
            self.assertEqual(lines[2], {"projectionDeleted": True})
            self.assertEqual(lines[4], {"name": "forbidden"})
            return {"status": 200, "body": {"errors": True, "items": [{op: metadata | {"status": 403,
                "error": {"type": "security_exception", "reason": "secret-canary"}}}
                for line in (lines[0], lines[1], lines[3]) for op, metadata in line.items()]}}
        probe.client = client
        with patch.object(smoke, "security_api_denial", wraps=smoke.security_api_denial) as native:
            probe.denied_changes()
        self.assertEqual(native.call_count, len(smoke.USERS))
        self.assertTrue(all(len(call.args) == 1 for call in native.call_args_list))
        self.assertEqual(probe.write.call_count, len(cases))
        for call, case in zip(probe.write.call_args_list, cases):
            self.assertEqual(call.args[0], "last-denial-response.json")
            saved = json.loads(call.args[1])
            self.assertEqual(set(saved), {"user", "method", "path", "response"})
            self.assertEqual({key: saved[key] for key in ("user", "method", "path")},
                {key: case[key] for key in ("user", "method", "path")})
        self.assertEqual(probe.snapshot.call_count, 2)
        self.assertEqual([c["user"] for c in cases if "ndjson" in c], list(smoke.USERS[1:3]))
        self.assertEqual([c["user"] for c in cases if c["path"] == "/fixture-autocreate"], list(smoke.USERS))
        self.assertNotIn("secret-canary", str(probe.event.call_args_list))
        probe.event.assert_called_with("role-denials-unchanged", snapshot_sha256="same-full-snapshot")

    def test_query_insights_effective_precedence_all_five_and_all_indices(self):
        expected = {"search.insights.top_queries." + metric + ".enabled": "false" for metric in ("latency", "cpu", "memory")}
        expected.update({"search.insights.top_queries.exporter.type": "none", "search.query.metrics.enabled": "false"})
        probe = object.__new__(smoke.Probe)
        probe.event = Mock()
        def check(settings, indices):
            probe.request = Mock(side_effect=[{"body": settings}, {"body": indices}])
            probe.query_insights()
        check({"defaults": expected}, [{"index": "product-listings"}, {"index": ".opendistro_security"}])
        self.assertEqual(probe.request.call_args_list[1].args,
            ("GET", "/_cat/indices?format=json&h=index&expand_wildcards=all"))
        for key in expected:
            for bad in (None, "true", "local_index", False):
                with self.subTest(key=key, bad=bad), self.assertRaisesRegex(smoke.Failure, "QUERY_INSIGHTS_NOT_DISABLED"):
                    check({"defaults": expected, "persistent": {key: bad}}, [])
            with self.assertRaisesRegex(smoke.Failure, "QUERY_INSIGHTS_NOT_DISABLED"):
                check({"defaults": expected, "persistent": expected, "transient": {key: "true"}}, [])
            check({"defaults": {key: "true"}, "persistent": expected}, [])
        for indices in ([{"index": "top_queries-fixture"}], {"error": "secret-canary"}, [{}]):
            with self.assertRaisesRegex(smoke.Failure, "QUERY_INSIGHTS_(INDEX_APPEARED|INVENTORY_UNKNOWN)"):
                check({"defaults": expected}, indices)
        self.assertNotIn("secret-canary", str(probe.event.call_args_list))

    def test_pipeline_absence_accepts_only_verified_absence_shapes(self):
        self.assertTrue(smoke.pipeline_absence({"status": 404, "body": {}}))
        for response in ({"status": 200, "body": {}}, {"status": 503, "body": {}},
                {"status": 404}, {"status": 404, "body": []}, {"status": 404, "body": {"unexpected": True}},
                {"status": 404, "body": {"error": {"type": "index_not_found_exception"}}}, {"unknown": True}):
            with self.subTest(response=response):
                self.assertFalse(smoke.pipeline_absence(response))

    def test_snapshot_all_pipelines_accepts_empty_absence_not_unrelated_errors(self):
        snapshots = []
        for response in ({"status": 404, "body": {}}, {"status": 200, "body": {}},
                {"status": 404, "body": {"unexpected": True}}, {"status": 503, "body": {}}):
            probe = object.__new__(smoke.Probe)
            def request(method, path, **options):
                self.assertEqual(method, "GET")
                if path == "/_search/pipeline":
                    self.assertEqual(options, {"expected": (200, 404)})
                    return response
                if not path.startswith("/_"):
                    return {"status": 404, "body": {"error": {"type": "index_not_found_exception"}}}
                self.assertEqual(options, {})
                return {"status": 200, "body": {}}
            probe.request = request
            probe.event = Mock()
            probe.query_insights = Mock()
            if response["status"] == 200 or smoke.pipeline_absence(response):
                snapshots.append(probe.snapshot())
            else:
                with self.assertRaisesRegex(smoke.Failure, "PIPELINE_ABSENCE_UNCONFIRMED"):
                    probe.snapshot()
        self.assertEqual(len(snapshots), 2)
        self.assertEqual(snapshots[0], snapshots[1])

    def test_snapshot_hash_diagnostics_preserve_original_digest_and_hide_values(self):
        probe = object.__new__(smoke.Probe)
        probe.event = Mock()
        secret = {"private-user": {"hash": "password-hash-canary", "attributes": {"secret-field": "secret-value"}}}
        state = {index: None for index in (*smoke.INDICES, "fixture-cross-index", "fixture-autocreate")}
        state.update({path: {} for path in ("/_cluster/settings?flat_settings=true", "/_search/pipeline", "/_alias",
            "/_plugins/_security/api/roles", "/_plugins/_security/api/rolesmapping")})
        state["/_plugins/_security/api/internalusers"] = secret
        def request(method, path, **options):
            self.assertEqual(method, "GET")
            if not path.startswith("/_"):
                return {"status": 404, "body": {"error": {"type": "index_not_found_exception"}}}
            return {"status": 200, "body": state[path]}
        probe.request = request
        probe.query_insights = Mock()
        result = probe.snapshot()
        self.assertEqual(result, smoke.digest(json.dumps(state, sort_keys=True).encode()))
        probe.event.assert_called_once_with("snapshot", snapshot_sha256=result, field_sha256={
            field: smoke.digest(json.dumps(value, sort_keys=True).encode()) for field, value in state.items()})
        emitted = json.dumps(probe.event.call_args.kwargs)
        for sensitive in ("private-user", "password-hash-canary", "secret-field", "secret-value"):
            self.assertNotIn(sensitive, emitted)

    def test_drift_retains_before_after_field_hashes_without_normalizing_system_entries(self):
        probe = object.__new__(smoke.Probe)
        probe.event = Mock()
        aliases = {}
        def request(method, path, **options):
            self.assertEqual(method, "GET")
            if not path.startswith("/_"):
                return {"status": 404, "body": {"error": {"type": "index_not_found_exception"}}}
            return {"status": 200, "body": aliases if path == "/_alias" else {}}
        probe.request = request
        probe.query_insights = Mock()
        for index in ("top_queries-fixture", smoke.ML_INDEX):
            aliases.clear()
            probe.event.reset_mock()
            with self.subTest(index=index), self.assertRaisesRegex(smoke.Failure, "STATE_CHANGED:system-entry"):
                probe.unchanged("system-entry", lambda: aliases.update({index: {"aliases": {}}}))
            self.assertEqual([call.args for call in probe.event.call_args_list], [("snapshot",), ("snapshot",)])
            before, after = [call.kwargs for call in probe.event.call_args_list]
            self.assertNotEqual(before["snapshot_sha256"], after["snapshot_sha256"])
            self.assertEqual(set(before["field_sha256"]), set(after["field_sha256"]))
            self.assertEqual([field for field in before["field_sha256"]
                if before["field_sha256"][field] != after["field_sha256"][field]], ["/_alias"])
            self.assertNotIn(index, json.dumps([before, after]))

    def test_only_attributed_authorization_and_autocreate_policy(self):
        denied = {"status": 403, "body": {"error": {"type": "security_exception"}}}
        self.assertTrue(smoke.denial(denied))
        for result in ({"unknown": True}, {"transport": "peer_closed"}, {"tls": 20},
                denied | {"status": 401}, {"status": 403, "body": {"error": {"type": "parse_exception"}}}):
            self.assertFalse(smoke.denial(result))
        missing = {"status": 404, "body": {"error": {"type": "index_not_found_exception", "reason": "missing"}}}
        self.assertFalse(smoke.autocreate_policy(missing))
        missing["body"]["error"]["reason"] = "no such index and [action.auto_create_index] is [false]"
        self.assertTrue(smoke.autocreate_policy(missing))

    def test_native_security_rest_disabled_branch_does_not_require_username(self):
        prefix = "No permission to access REST API: "
        response = {"status": 403, "body": {"status": "FORBIDDEN", "message": prefix + "management disabled"}}
        self.assertTrue(smoke.security_api_denial(response))
        named = copy.deepcopy(response)
        named["body"]["message"] = prefix + "User aura_reader has no admin access"
        self.assertTrue(smoke.security_api_denial(named))
        for bad in (None, [], {}, {"transport": "peer_closed"}, {"tls": 20}, {"unknown": True},
                response | {"status": 200}, response | {"status": 401}, response | {"status": 503},
                response | {"body": None}, response | {"body": []}, response | {"body": "FORBIDDEN"}):
            self.assertFalse(smoke.security_api_denial(bad))
        for status in (None, "forbidden", "UNAUTHORIZED", 403):
            self.assertFalse(smoke.security_api_denial({"status": 403, "body": response["body"] | {"status": status}}))
        for message in (None, {}, [], 403, "", prefix, prefix + "  ", prefix.rstrip(),
                "No permission to access REST API:disabled", "no permission to access REST API: disabled",
                "unrelated " + prefix + "disabled", "aura_reader timeout"):
            self.assertFalse(smoke.security_api_denial({"status": 403, "body": response["body"] | {"message": message}}))

    def test_original_denial_response_saved_private_before_failure_without_request_payload(self):
        for bulk in (False, True):
            with self.subTest(bulk=bulk), tempfile.TemporaryDirectory() as folder, contextlib.redirect_stdout(io.StringIO()) as output:
                probe = smoke.Probe(Path(folder), SimpleNamespace(), Mock())
                probe.request = Mock()
                probe.snapshot = Mock(return_value="unchanged")
                bad = {"status": 200 if bulk else 400, "body": {"error": {"type": "parse_exception", "reason": "secret-canary"}}}
                def client(case):
                    if not bulk or "ndjson" in case:
                        return bad
                    return {"status": 403, "body": {"error": {"type": "security_exception"}}}
                probe.client = Mock(side_effect=client)
                with self.assertRaisesRegex(smoke.Failure, "BULK_ITEM_DENIAL_FAILED_RETAINED" if bulk else "LEAST_PRIVILEGE_FAILED_RETAINED"):
                    probe.denied_changes()
                path = Path(folder) / "last-denial-response.json"
                saved = json.loads(path.read_text())
                self.assertEqual(set(saved), {"user", "method", "path", "response"})
                self.assertEqual(saved["response"], bad)
                case = probe.client.call_args.args[0]
                self.assertEqual({key: saved[key] for key in ("user", "method", "path")},
                    {key: case[key] for key in ("user", "method", "path")})
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                self.assertNotIn("secret-canary", output.getvalue())
                self.assertNotIn("secret-canary", (Path(folder) / "evidence.json").read_text())
                self.assertEqual(probe.snapshot.call_count, 1)

    def test_securityadmin_generic_failure_is_not_admin_rejection(self):
        for text in ("connection timed out", "ERR: cluster unavailable", "SSL handshake failed", "unknown CA", "hostname mismatch"):
            for identity in ("node", "unregistered"):
                self.assertFalse(smoke.securityadmin_rejection(text, identity))
        self.assertTrue(smoke.securityadmin_rejection("CN=unregistered is not an admin user", "unregistered"))
        self.assertTrue(smoke.securityadmin_rejection("seems to be a node certificate", "node"))
        self.assertTrue(smoke.securityadmin_rejection("Seems you use a node certificate which is not an admin certificate", "node"))

    def test_plaintext_needs_same_peer_channel_and_tls_error(self):
        header = "[2026-09-14T12:00:00][WARN] Netty4HttpChannel{localAddress=/172.20.0.2:9200, remoteAddress=/172.20.0.3:45000}\n"
        error = "io.netty.handler.ssl.NotSslRecordException: not an SSL/TLS record\n"
        self.assertTrue(smoke.plaintext_rejection(header + error, "172.20.0.3", 9200))
        for raw in (header + "connection reset", error, header + error.replace("SSL/TLS", "OTHER"),
                header + "[2026-09-14T12:00:01][WARN] another channel\n" + error):
            self.assertFalse(smoke.plaintext_rejection(raw, "172.20.0.3", 9200))
        self.assertFalse(smoke.plaintext_rejection(header + error, "172.20.0.4", 9200))
        self.assertFalse(smoke.plaintext_rejection(header + error, "172.20.0.3", 9300))

    def test_safe_evidence_does_not_emit_response_credentials(self):
        value = {"status": 403, "body": {"password": "secret-canary", "error": {"type": "security_exception",
            "reason": "secret-canary no permissions for [cluster:monitor/main]"}}}
        safe = smoke.safe_result(value)
        self.assertNotIn("secret-canary", json.dumps(safe))
        self.assertEqual(safe["actions"], ["cluster:monitor/main"])

    def test_missing_cached_image_reports_only_reviewed_id(self):
        for field in ("engine_image", "helper_image"):
            missing = "sha256:" + ("c" if field == "engine_image" else "d") * 64
            probe = object.__new__(smoke.Probe)
            probe.engine_image = "sha256:" + "e" * 64
            probe.helper_image = "sha256:" + "f" * 64
            setattr(probe, field, missing)
            probe.call = Mock(return_value=(1, "Error: No such image: secret-canary"))
            with self.subTest(field=field), self.assertRaises(smoke.Failure) as caught:
                probe.inspect("image", missing)
            self.assertEqual(str(caught.exception), "CACHED_IMAGE_MISSING:" + missing)
            self.assertNotIn("secret-canary", str(caught.exception))
            probe.call.assert_called_once_with("image", "inspect", missing, check=False)

    def test_never_started_permission_failure_is_classified_without_raw_error(self):
        probe = object.__new__(smoke.Probe)
        probe.ids = {"hash": "owned-id"}
        probe.runtime = Mock(return_value={"State": {"Status": "created", "Running": False, "Pid": 0,
            "ExitCode": 126, "StartedAt": "0001-01-01T00:00:00Z", "Error": "secret-canary permission denied"}})
        probe.call = Mock(return_value=(1, "secret-canary"))
        probe.event = Mock()
        with self.assertRaisesRegex(smoke.Failure, "ONESHOT_EXEC_PERMISSION_DENIED"):
            probe.finish("hash")
        self.assertNotIn("secret-canary", str(probe.event.call_args))
        probe.runtime.return_value["State"]["StartedAt"] = "2026-09-14T12:00:00Z"
        with self.assertRaisesRegex(smoke.Failure, "ONESHOT_OUTCOME_UNKNOWN"):
            probe.finish("hash")

    def test_non_200_root_stops_without_retry_or_security_upload(self):
        for status in (401, 403, 503):
            probe = object.__new__(smoke.Probe)
            probe.deadline = time.monotonic() + 10
            probe.runtime = Mock(return_value={"State": {"Running": True}})
            probe.client = Mock(return_value={"status": status, "body": {}, "classification": "fixture-status"})
            probe.event = Mock()
            probe.compose_create = Mock()
            probe.args = SimpleNamespace(hostname="opensearch", port=9200, cluster_name="owned-cluster")
            with self.subTest(status=status), patch.object(smoke.time, "sleep") as sleep:
                with self.assertRaisesRegex(smoke.Failure, "TARGET_ROOT_NOT_200"):
                    probe.wait_node()
                sleep.assert_not_called()
                probe.client.assert_called_once()
                with self.assertRaisesRegex(smoke.Failure, "TARGET_ROOT_NOT_200"):
                    probe.admin()
                probe.compose_create.assert_not_called()
                self.assertEqual(probe.event.call_args.kwargs["result"]["status"], status)

    def test_snapshot_change_stops_without_success_evidence(self):
        probe = object.__new__(smoke.Probe)
        probe.snapshot = Mock(side_effect=["before", "after"])
        probe.event = Mock()
        action = Mock()
        with self.assertRaisesRegex(smoke.Failure, "STATE_CHANGED"):
            probe.unchanged("verify", action)
        action.assert_called_once()
        probe.event.assert_not_called()

    def test_failed_run_never_calls_cleanup_and_redacts_unknown_error(self):
        probe = Mock()
        probe.run.side_effect = OSError("secret-canary")
        output = io.StringIO()
        with tempfile.TemporaryDirectory(prefix="unit-stock-") as folder, \
                patch.object(smoke, "Probe", return_value=probe), \
                patch.object(smoke, "load_capture", return_value=Mock()), \
                patch.object(smoke.tempfile, "mkdtemp", return_value=folder), \
                patch.object(smoke.Path, "glob", return_value=[]), \
                contextlib.redirect_stdout(output):
            self.assertEqual(smoke.main(["--reviewed-run"]), 1)
        probe.cleanup.assert_not_called()
        self.assertIn("RETAINED", output.getvalue())
        self.assertNotIn("secret-canary", output.getvalue())

    def test_reused_capture_bounds_output_and_timeout_without_echo(self):
        capture = smoke.load_capture()
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaisesRegex(Exception, "OUTPUT_LIMIT_UNCONFIRMED"):
                capture([sys.executable, "-c", "print('secret-canary' * 100000)"], Path(folder), 5)
            with self.assertRaisesRegex(Exception, "COMMAND_UNCONFIRMED"):
                capture([sys.executable, "-c", "import time; time.sleep(2)"], Path(folder), .05)


if __name__ == "__main__":
    unittest.main()
