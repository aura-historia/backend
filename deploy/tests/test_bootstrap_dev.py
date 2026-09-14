"""Offline guards only. No Docker, certificates, sockets, cloud or image builds."""
import argparse
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("bootstrap_dev", Path(__file__).with_name("smoke-bootstrap-dev.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
IMAGE = "sha256:" + "a" * 64
SHA = "13a4b822673e0a33ed0e90ddee4f90889a097d4a"
CID = "b" * 64
NET = "c" * 64


def model(probe, environments, commands, ca=smoke.CA):
    return dict(name=probe.project, networks={"backend": dict(name=probe.project, external=True)}, services={
        "bootstrap-" + t: dict(image=IMAGE, pull_policy="never", restart="no", read_only=True,
            user="10001:10001", cap_drop=["ALL"], security_opt=["no-new-privileges:true"],
            networks={"backend": {"ipv4_address": probe.peers[t]}},
            mem_limit=536870912, cpus=1.0, pids_limit=64, stop_grace_period="1m40s", command=commands[t],
            environment=environments[t] | {"POSTGRES_SSL_ROOT_CERT": ca},
            volumes=[dict(type="bind", source=str(probe.directory / "init/postgres-ca.pem"), target=smoke.CA,
                read_only=True, bind=dict(create_host_path=False))], logging=smoke.LOGGING,
            labels={smoke.OWNER: probe.token}, container_name=probe.project + "-" + t) for t in smoke.TARGETS})


class Guards(unittest.TestCase):
    def probe(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        with contextlib.redirect_stdout(io.StringIO()):
            probe = smoke.Probe(Path(temporary.name), IMAGE, SHA)
        probe.peers = dict(business="172.20.0.254", crawler="172.20.0.253")
        return probe

    def test_required_immutable_cli_inputs_fail_before_fixture_creation(self):
        self.assertEqual(smoke.image_id(IMAGE), IMAGE)
        self.assertEqual(smoke.source_sha(SHA), SHA)
        for value in ("latest", "repo@" + IMAGE, "sha256:abc", IMAGE + "\n", "--privileged"):
            with self.assertRaises(argparse.ArgumentTypeError):
                smoke.image_id(value)
        for value in ("main", "a" * 40, SHA.upper(), SHA + "\n", "--help"):
            with self.assertRaises(argparse.ArgumentTypeError):
                smoke.source_sha(value)
        for argv in ([], ["--image", IMAGE], ["--source-sha", SHA]):
            with patch.object(smoke.tempfile, "mkdtemp") as create, contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    smoke.main(argv)
                create.assert_not_called()

    def test_exact_checked_in_source_is_guarded_before_compose(self):
        smoke.source_guard()
        probe = self.probe()
        source = smoke.helpers.checked_bytes(smoke.COMPOSE)
        self.assertEqual(smoke.hashlib.sha256(source).hexdigest(), smoke.COMPOSE_SHA256)
        path = probe.write("compose.yml", source.decode())
        probe.call = Mock()
        for suffix in ("\ninclude: [/host/live.yml]\n", "\nservices: {x: {env_file: /host/credentials}}\n"):
            path.write_text(source.decode() + suffix)
            with patch.object(smoke, "COMPOSE", path), self.assertRaisesRegex(smoke.Failure, "COMPOSE_SOURCE_NOT_REVIEWED"):
                probe.compose("create")
            probe.call.assert_not_called()
        path.unlink()
        path.symlink_to(probe.directory / "unread-secret")
        with patch.object(smoke, "COMPOSE", path), self.assertRaisesRegex(smoke.Failure, "INPUT_PATH"):
            probe.compose("config")
        probe.call.assert_not_called()

    def test_model_rejects_unsafe_mount_network_environment_and_execution(self):
        probe = self.probe()
        env = {t: probe.environment(t) for t in smoke.TARGETS}
        commands = {t: ["--help"] for t in smoke.TARGETS}
        original = model(probe, env, commands)
        def validate(value):
            smoke.validate_model(value, probe.directory, probe.project, probe.token, IMAGE, env, commands, smoke.CA, probe.peers)
        validate(original)
        normalized = copy.deepcopy(original)
        normalized["networks"]["backend"]["ipam"] = {}
        normalized["services"]["bootstrap-business"]["mem_limit"] = "536870912"
        normalized["services"]["bootstrap-crawler"]["entrypoint"] = None
        validate(normalized)
        normalized["networks"]["backend"]["ipam"] = {"config": [{"subnet": "10.0.0.0/8"}]}
        with self.assertRaisesRegex(smoke.Failure, "MODEL_NETWORK"):
            validate(normalized)
        mutations = [("ports", ["5432:5432"]), ("network_mode", "host"), ("privileged", True),
            ("build", "."), ("entrypoint", ["docker"]), ("entrypoint", []), ("env_file", "/host/credentials"),
            ("environment", env["business"] | {"AWS_PROFILE": "live"}), ("networks", {"default": {}}),
            ("networks", {"backend": None}), ("networks", {"backend": {"ipv4_address": probe.peers["crawler"]}}),
            ("volumes", [dict(type="bind", source="/var/run/docker.sock", target="/socket")]),
            ("read_only", False), ("user", "0"), ("image", "unreviewed:latest"), ("cap_add", ["SYS_ADMIN"]),
            ("mem_limit", "536870913"), ("mem_limit", "unlimited")]
        for key, value in mutations:
            changed = copy.deepcopy(original)
            changed["services"]["bootstrap-business"][key] = value
            with self.subTest(key=key), self.assertRaises(smoke.Failure):
                validate(changed)
        changed = copy.deepcopy(original)
        changed["networks"]["backend"]["name"] = "live"
        with self.assertRaises(smoke.Failure):
            validate(changed)
        changed = copy.deepcopy(original)
        changed["services"]["bootstrap-business"]["volumes"][0]["read_only"] = False
        with self.assertRaises(smoke.Failure):
            validate(changed)

    def test_runtime_rejects_foreign_identity_socket_mount_and_root_before_start(self):
        probe = self.probe()
        probe.recorded = {CID: "business"}
        env = probe.environment("business") | {"POSTGRES_SSL_ROOT_CERT": smoke.CA}
        item = dict(Id=CID, Name="/" + probe.project + "-business", Image=IMAGE,
            Config=dict(User="10001:10001", Entrypoint=smoke.ENTRYPOINT, Cmd=["--help"],
                Env=[k + "=" + v for k, v in env.items()], Labels={smoke.OWNER: probe.token,
                    "com.docker.compose.service": "bootstrap-business", "com.docker.compose.project": probe.project}),
            HostConfig=dict(ReadonlyRootfs=True, CapDrop=["ALL"], SecurityOpt=["no-new-privileges:true"],
                NetworkMode=probe.project, IpcMode="private", Memory=536870912, PidsLimit=64),
            NetworkSettings=dict(Networks={probe.project: {"IPAMConfig": {"IPv4Address": probe.peers["business"]}}}),
            Mounts=[dict(Type="bind", RW=False, Source=str(probe.directory / "init/postgres-ca.pem"), Destination=smoke.CA)])
        probe.inspect = Mock(return_value=item)
        probe.network_guard = Mock()
        probe.runtime(CID, "business", env, ["--help"])
        for rendered in (None, "POSTGRES_SSL_ROOT_CERT", "POSTGRES_SSL_ROOT_CERT=", "UNREVIEWED"):
            unset = copy.deepcopy(item)
            unset["Config"]["Env"] = [e for e in item["Config"]["Env"] if not e.startswith("POSTGRES_SSL_ROOT_CERT=")]
            if rendered is not None:
                unset["Config"]["Env"].append(rendered)
            probe.inspect.return_value = unset
            if rendered in (None, "POSTGRES_SSL_ROOT_CERT"):
                probe.runtime(CID, "business", env | {"POSTGRES_SSL_ROOT_CERT": None}, ["--help"])
            else:
                with self.assertRaisesRegex(smoke.Failure, "RUNTIME_ENV"):
                    probe.runtime(CID, "business", env | {"POSTGRES_SSL_ROOT_CERT": None}, ["--help"])
        for section, key, value in (("Config", "User", "0"), ("Config", "Labels", {}),
                ("HostConfig", "PortBindings", {"5432/tcp": []}), ("HostConfig", "NetworkMode", "host")):
            changed = copy.deepcopy(item)
            changed[section][key] = value
            probe.inspect.return_value = changed
            with self.assertRaises(smoke.Failure):
                probe.runtime(CID, "business", env, ["--help"])
        for address in (None, probe.peers["crawler"]):
            changed = copy.deepcopy(item)
            changed["NetworkSettings"]["Networks"][probe.project]["IPAMConfig"] = {"IPv4Address": address}
            probe.inspect.return_value = changed
            with self.assertRaisesRegex(smoke.Failure, "RUNTIME_NETWORK"):
                probe.runtime(CID, "business", env, ["--help"])
        changed = copy.deepcopy(item)
        changed["Mounts"][0]["Source"] = "/var/run/docker.sock"
        probe.inspect.return_value = changed
        with self.assertRaisesRegex(smoke.Failure, "RUNTIME_MOUNTS"):
            probe.runtime(CID, "business", env, ["--help"])

    def test_fixture_peers_are_reserved_in_actual_owned_subnet_not_discovered_after_exit(self):
        network = {"IPAM": {"Config": [{"Subnet": "172.20.0.0/24", "Gateway": "172.20.0.1"}]}}
        self.assertEqual(smoke.fixture_peers(network), dict(business="172.20.0.254", crawler="172.20.0.253"))
        for config in ([], [{"Subnet": "172.20.0.0/30"}], [{"Subnet": "fd00::/64"}],
                [{"Subnet": "172.20.0.0/24", "Gateway": "172.20.0.254"}],
                [{"Subnet": "172.20.0.0/24", "IPRange": "172.20.0.0/25"}]):
            with self.assertRaises(smoke.Failure):
                smoke.fixture_peers({"IPAM": {"Config": config}})

    def test_synthetic_inputs_are_frozen_before_compose(self):
        probe = self.probe()
        probe.write("case.json", "{}")
        probe.inputs = {"case.json": b"{}"}
        probe.write("case.json", '{"include":"/live"}')
        probe.call = Mock()
        with self.assertRaisesRegex(smoke.Failure, "INPUT_CHANGED"):
            probe.compose("create")
        probe.call.assert_not_called()

    def test_unknown_subprocess_and_daemon_failures_retain_without_cleanup(self):
        for response in (smoke.Failure("COMMAND_UNCONFIRMED"), (1, "private-password")):
            probe = self.probe()
            kwargs = {"side_effect": response} if isinstance(response, Exception) else {"return_value": response}
            with patch.object(smoke, "capture", **kwargs), self.assertRaises(smoke.Failure):
                probe.call("create", mutation=True)
            self.assertTrue(probe.uncertain)
            probe.call = Mock()
            with self.assertRaisesRegex(smoke.Failure, "UNKNOWN_OUTCOME_RETAINED"):
                probe.cleanup()
            probe.call.assert_not_called()
            self.assertTrue(probe.directory.exists())

    def test_fixed_local_docker_sanitized_capture_and_redacted_errors(self):
        probe = self.probe()
        with patch.object(smoke, "capture", return_value=(1, "password provider-body")) as capture:
            with self.assertRaisesRegex(smoke.Failure, "^DOCKER_COMMAND_FAILED$"):
                probe.call("image", "inspect", IMAGE)
        argv = capture.call_args.args[0]
        self.assertEqual(argv[:5], ["/usr/bin/docker", "--host", "unix:///var/run/docker.sock", "--config", str(probe.directory / "docker")])
        self.assertFalse(probe.uncertain)

    def test_owned_cleanup_rejects_unrecorded_container_before_any_removal(self):
        probe = self.probe()
        probe.call = Mock(return_value=CID + "\n")
        with self.assertRaisesRegex(smoke.Failure, "UNRECORDED_CONTAINER_RETAINED"):
            probe.cleanup()
        self.assertEqual(probe.call.call_count, 1)
        self.assertTrue(probe.directory.exists())
        self.assertTrue(probe.uncertain)

    def test_network_guard_rejects_egress_foreign_members_and_owner(self):
        probe = self.probe()
        probe.network, probe.ids = NET, {"postgres": CID}
        item = dict(Id=NET, Name=probe.project, Labels={smoke.OWNER: probe.token}, Internal=True,
            Driver="bridge", Scope="local", Options={}, EnableIPv6=False, IPAM=dict(Driver="default"), Containers={CID: {}})
        probe.inspect = Mock(return_value=item)
        probe.network_guard()
        for key, value in (("Internal", False), ("Labels", {}), ("Id", "c" * 12),
                           ("Containers", {"d" * 64: {}}), ("Options", {"bridge.name": "host"})):
            probe.inspect.return_value = item | {key: value}
            with self.subTest(key=key), self.assertRaises(smoke.Failure):
                probe.network_guard()

    def test_tls_evidence_requires_attributed_encrypted_selected_connection(self):
        line = "2026-09-14 [12] LOG: connection authorized: user=postgres database=smoke_business application_name=crawler-bootstrap-dev SSL enabled (protocol=TLSv1.3, cipher=TLS_AES_256_GCM_SHA384, bits=256)\n"
        self.assertTrue(smoke.tls_evidence(line, "business"))
        for raw in ("", "connection timed out", line.replace(" SSL enabled", ""),
                    line.replace("crawler-bootstrap-dev", "psql"), line.replace("smoke_business", "smoke_crawler"),
                    line + line.replace("smoke_business", "smoke_crawler"),
                    line + line.replace(" SSL enabled", " plaintext").replace("smoke_business", "smoke_crawler")):
            self.assertFalse(smoke.tls_evidence(raw, "business"))

    def test_rejection_requires_case_peer_same_pid_and_specific_reason(self):
        received = "2026-09-14 [12] LOG:  connection received: host=172.20.0.3 port=45678\n"
        reasons = {"wrong-ca": "LOG:  could not accept SSL connection: tlsv1 alert unknown ca",
            "wrong-hostname": "LOG:  could not accept SSL connection: sslv3 alert bad certificate",
            "wrong-password": 'FATAL:  password authentication failed for user "postgres"'}
        for reason, message in reasons.items():
            error = "2026-09-14 [12] " + message + "\n"
            self.assertTrue(smoke.rejection_evidence(received + error, "172.20.0.3", reason))
            for raw in (error, received, received + error.replace("[12]", "[13]"),
                    received.replace("172.20.0.3", "172.20.0.4") + error,
                    received + "2026-09-14 [12] LOG:  connection timed out\n",
                    received + error + "2026-09-14 [12] LOG:  connection authorized: user=postgres\n"):
                with self.subTest(reason=reason):
                    self.assertFalse(smoke.rejection_evidence(raw, "172.20.0.3", reason))
            for other in set(reasons) - {reason}:
                self.assertFalse(smoke.rejection_evidence(received + error, "172.20.0.3", other))
        self.assertTrue(smoke.rejection_evidence(received, "172.20.0.3", "plaintext"))
        self.assertFalse(smoke.rejection_evidence("", "172.20.0.3", "plaintext"))
        self.assertFalse(smoke.rejection_evidence(received + '2026-09-14 [12] FATAL:  authentication timeout\n', "172.20.0.3", "plaintext"))
        self.assertFalse(smoke.rejection_evidence(received + '2026-09-14 [12] LOG:  connection authenticated: identity="postgres"\n', "172.20.0.3", "plaintext"))

    def test_main_retains_post_initialize_history_mismatch_outside_attempt(self):
        probe = self.probe()
        probe.ids, probe.recorded = {"postgres": CID}, {CID: "postgres"}
        probe.journal()
        probe.run = Mock(side_effect=smoke.Failure("SOURCE_SQLX_HISTORY"))
        probe.call = Mock()
        output = io.StringIO()
        with patch.object(smoke, "Probe", return_value=probe), patch.object(smoke.tempfile, "mkdtemp", return_value=str(probe.directory)), patch.object(smoke.signal, "signal"), patch.object(smoke.os, "umask"), contextlib.redirect_stdout(output):
            self.assertEqual(smoke.main(["--image", IMAGE, "--source-sha", SHA]), 1)
        self.assertTrue(probe.uncertain)
        self.assertTrue(probe.directory.exists())
        self.assertEqual(json.loads((probe.directory / "ownership.json").read_text())["active"], {"postgres": CID})
        probe.call.assert_not_called()
        self.assertIn("FAIL SOURCE_SQLX_HISTORY", output.getvalue())
        self.assertIn("RETAINED", output.getvalue())
        self.assertNotIn("CLEANUP confirmed", output.getvalue())

    def test_dump_normalization_preserves_real_schema_data_and_history_changes(self):
        a = "\\restrict abc123\nCREATE TABLE t (x int);\nCOPY t FROM stdin;\n1\n\\.\n\\unrestrict abc123\n"
        self.assertEqual(smoke.logical_dump(a), smoke.logical_dump(a.replace("abc123", "random456")))
        for changed in (a.replace("1\n", "2\n"), a.replace("int", "text"), a + "CREATE TABLE _sqlx_migrations();\n"):
            self.assertNotEqual(smoke.logical_dump(a), smoke.logical_dump(changed))

    def test_every_known_rejection_requires_full_unchanged_snapshots(self):
        for changed in (False, True):
            probe = self.probe()
            probe.ids = {"postgres": CID}
            probe.write("root.pem", "synthetic CA")
            probe.write("init/postgres-ca.pem", "old CA", 0o444)
            probe.write("init/postgres-ca.pem", "replacement CA", 0o444)
            self.assertEqual((probe.directory / "init/postgres-ca.pem").read_text(), "replacement CA")
            probe.write("compose.env", "synthetic inputs")
            before = dict(postgres="p", smoke_business="b", smoke_crawler="c", globals="g")
            after = before | ({"smoke_crawler": "mutated"} if changed else {})
            probe.snapshot = Mock(side_effect=[before, after])
            env = {t: probe.environment(t) for t in smoke.TARGETS}
            env["business"]["STAGE"] = "prod"
            commands = dict(business=["--initialize-fresh", "business"], crawler=["--help"])
            probe.compose = Mock(side_effect=[json.dumps(model(probe, env, commands)), ""])
            probe.inspect = Mock(return_value={"Id": "d" * 64})
            probe.runtime = Mock()
            probe.owned = Mock(return_value={"State": {"Status": "exited", "ExitCode": 3}})
            probe.call = Mock(side_effect=["", "", "UNSUPPORTED_STAGE\n"])
            probe.remove = Mock()
            with contextlib.redirect_stdout(io.StringIO()):
                if changed:
                    with self.assertRaisesRegex(smoke.Failure, "REJECTION_OR_READONLY_MUTATED_DATABASE"):
                        probe.attempt("business", "--initialize-fresh", (3, "UNSUPPORTED_STAGE"), {"STAGE": "prod"})
                    probe.remove.assert_not_called()
                    self.assertEqual(json.loads((probe.directory / "last-snapshot.json").read_text())["after"], after)
                else:
                    probe.attempt("business", "--initialize-fresh", (3, "UNSUPPORTED_STAGE"), {"STAGE": "prod"})
                    probe.remove.assert_called_once_with("business")
            self.assertEqual(probe.snapshot.call_count, 2)


if __name__ == "__main__":
    unittest.main()
