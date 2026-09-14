"""Offline TLS harness guards. No Docker, sockets, images, or provider calls."""
import argparse
import contextlib
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("sequin_tls", Path(__file__).with_name("smoke-sequin-tls.py"))
tls = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tls)
IMAGE = "sha256:" + "a" * 64
TOKEN = "b" * 32
PROJECT = "aura-sequin-tls-" + TOKEN
CID = "c" * 64


def container():
    return dict(Id=CID, Name=f"/{PROJECT}-sequin-1", Config=dict(Labels={
        tls.OWNER: TOKEN, "com.docker.compose.project": PROJECT, "com.docker.compose.service": "sequin"}))


def network():
    return dict(Name=PROJECT + "_backend", Internal=True, Driver="bridge", Scope="local",
        Options={}, IPAM=dict(Driver="default", Options={}), Containers={CID: {}}, Labels={
            tls.OWNER: TOKEN, "com.docker.compose.project": PROJECT, "com.docker.compose.network": "backend"})


def stunnel_log(listener="source", error="SSL_connect: certificate verify failed: unable to get local issuer certificate"):
    return (f"2026.09.14 LOG5[7]: Service [{listener}] accepted connection from 127.0.0.1:40123\n"
        "2026.09.14 LOG5[7]: s_connect: connected 172.30.0.2:5432\n"
        f"2026.09.14 LOG3[7]: {error}\n")


class Guards(unittest.TestCase):
    def probe(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        with contextlib.redirect_stdout(io.StringIO()):
            return tls.Probe(Path(temporary.name), IMAGE)

    def test_single_file_local_logging_disables_compression(self):
        # Docker refuses container startup when compression has no rotated file.
        self.assertEqual(tls.LOGGING, dict(driver="local", options={
            "max-size": "1m", "max-file": "1", "compress": "false"}))

    def test_image_input_rejects_tags_registry_digests_options_and_short_ids(self):
        self.assertEqual(tls.image_id(IMAGE), IMAGE)
        for value in ("latest", "sequin:0.14.6", "registry/image@" + IMAGE, "a" * 64,
                      "sha256:123", "sha256:" + "A" * 64, IMAGE + "\n", "--privileged", ""):
            with self.subTest(value=value), self.assertRaises(argparse.ArgumentTypeError):
                tls.image_id(value)

    def test_missing_cli_image_does_not_create_fixtures(self):
        with patch.object(tls.tempfile, "mkdtemp") as create, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                tls.main([])
            create.assert_not_called()

    def test_container_requires_full_id_exact_name_service_project_and_owner(self):
        self.assertEqual(tls.owned_container(container(), PROJECT, TOKEN), "sequin")
        for key, value in (("Id", "c" * 12), ("Name", "/live-sequin-1")):
            item = container()
            item[key] = value
            with self.assertRaises(tls.Failure):
                tls.owned_container(item, PROJECT, TOKEN)
        for key in (tls.OWNER, "com.docker.compose.project", "com.docker.compose.service"):
            item = container()
            item["Config"]["Labels"][key] = "foreign"
            with self.assertRaises(tls.Failure):
                tls.owned_container(item, PROJECT, TOKEN)

    def test_runtime_requires_exact_pg_url_unset_wrapper_without_extra_arguments(self):
        probe = self.probe()
        for entrypoint, command in ((None, None), (["/scripts/start_commands.sh"], None),
                (["/usr/bin/env", "PG_URL=", "/scripts/start_commands.sh"], None),
                (["/usr/bin/env", "-u", "PG_PASSWORD", "/scripts/start_commands.sh"], None),
                (tls.SEQUIN_ENTRYPOINT + ["extra"], None), (tls.SEQUIN_ENTRYPOINT, ["extra"])):
            item = container()
            item["Image"] = probe.images["sequin"]
            item["Config"].update(Entrypoint=entrypoint, Cmd=command)
            probe.project, probe.token = PROJECT, TOKEN
            probe.call = Mock(side_effect=[CID, json.dumps([item])])
            with self.subTest(entrypoint=entrypoint, command=command), self.assertRaisesRegex(
                    tls.Failure, "^RUNTIME_SEQUIN_PG_URL_WRAPPER$"):
                probe.resources()
            self.assertEqual(probe.call.call_count, 2)

    def test_network_rejects_egress_foreign_member_driver_options_and_owner(self):
        tls.owned_network(network(), PROJECT, TOKEN, [CID])
        for key, value in (("Internal", False), ("Name", "live_backend"), ("Driver", "overlay"),
                           ("Options", {"com.docker.network.bridge.name": "live"}),
                           ("Containers", {"d" * 64: {}}), ("Labels", {})):
            item = network()
            item[key] = value
            with self.subTest(key=key), self.assertRaises(tls.Failure):
                tls.owned_network(item, PROJECT, TOKEN, [CID])

    def test_unknown_command_outcome_never_runs_cleanup(self):
        for code in ("COMMAND_UNCONFIRMED", "OUTPUT_LIMIT_UNCONFIRMED"):
            probe = self.probe()
            with patch.object(tls, "capture", side_effect=tls.Failure(code)):
                with self.assertRaises(tls.Failure):
                    probe.call("compose", "up")
            self.assertTrue(probe.uncertain)
            probe.resources = Mock()
            with self.assertRaisesRegex(tls.Failure, "UNKNOWN_OUTCOME"):
                probe.cleanup()
            probe.resources.assert_not_called()
            self.assertTrue(probe.directory.exists())

    def test_nonzero_daemon_mutations_retain_state_even_without_check(self):
        for args in (("compose", "up"), ("stop", CID), ("start", CID), ("rm", "--force", CID),
                     ("kill", "--signal", "HUP", CID), ("network", "rm", "owned-network")):
            for check in (True, False):
                probe = self.probe()
                with self.subTest(args=args, check=check), patch.object(tls, "capture", return_value=(1, "ignored")):
                    if check:
                        with self.assertRaises(tls.Failure):
                            probe.call(*args)
                    else:
                        self.assertEqual(probe.call(*args, check=False)[0], 1)
                self.assertTrue(probe.uncertain)
                probe.resources = Mock()
                with self.assertRaisesRegex(tls.Failure, "UNKNOWN_OUTCOME_RETAINED"):
                    probe.cleanup()
                probe.resources.assert_not_called()
                self.assertTrue(probe.directory.exists())

    def test_completed_application_nonzero_is_not_an_unknown_daemon_mutation(self):
        probe = self.probe()
        with patch.object(tls, "capture", return_value=(1, "not ready")):
            self.assertEqual(probe.call("exec", CID, "pg_isready", check=False), (1, "not ready"))
        self.assertFalse(probe.uncertain)

    def test_pinned_sources_reject_includes_extends_and_env_files_before_compose(self):
        originals = {name: (tls.ROOT / name).read_bytes() for name in tls.COMPOSE_SOURCES}
        tls.validate_sources([tls.ROOT / name for name in originals])
        probe = self.probe()
        root = probe.directory / "checkout"
        for name, raw in originals.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)
        probe.files = [root / name for name in originals]
        probe.call = Mock()
        injections = (b"\ninclude: [https://untrusted.invalid/compose.yml]\n",
            b"\nservices: {sequin: {extends: {file: /live/compose.yml, service: sequin}}}\n",
            b"\nservices: {sequin: {env_file: /live/credentials.env}}\n",
            b"\nservices: {sequin: {env_file: !override [/live/credentials.env]}}\n",
            b"\nx-other: &other {env_file: /live/credentials.env}\nservices: {sequin: {<<: *other}}\n")
        with patch.object(tls, "ROOT", root):
            tls.validate_sources(probe.files)
            for name, raw in originals.items():
                for injection in injections:
                    (root / name).write_bytes(raw + injection)
                    for operation in (probe.prepare, lambda: probe.compose("config", "--format", "json")):
                        with self.subTest(name=name), self.assertRaisesRegex(tls.Failure, "COMPOSE_SOURCE_NOT_REVIEWED"):
                            operation()
                    probe.call.assert_not_called()
                    (root / name).write_bytes(raw)
            with self.assertRaisesRegex(tls.Failure, "COMPOSE_SOURCE_PATHS"):
                tls.validate_sources(list(reversed(probe.files)))
            first = probe.files[0]
            first.unlink()
            first.symlink_to(probe.directory / "not-to-be-read")
            with self.assertRaisesRegex(tls.Failure, "INPUT_PATH"):
                probe.compose("config")
            probe.call.assert_not_called()

    def test_compose_rechecks_synthetic_env_files_and_marks_failed_up_uncertain(self):
        probe = self.probe()
        probe.env_inputs = {name: b"synthetic\n" for name in ("compose.env", "postgres.env", "sequin.env")}
        for name, raw in probe.env_inputs.items():
            probe.write(name, raw.decode())
        with patch.object(tls, "capture", return_value=(1, "ignored")) as capture:
            with self.assertRaisesRegex(tls.Failure, "DOCKER_FAILED_compose"):
                probe.compose("up", "-d", "--no-deps", "sequin")
            self.assertTrue(probe.uncertain)
            self.assertEqual(capture.call_count, 1)
        probe.uncertain = False
        probe.write("sequin.env", "PG_URL=postgres://not-fixture\n")
        with patch.object(tls, "capture") as capture:
            with self.assertRaisesRegex(tls.Failure, "ENV_INPUT_CHANGED"):
                probe.compose("config", "--format", "json")
            capture.assert_not_called()
        (probe.directory / "sequin.env").unlink()
        (probe.directory / "sequin.env").symlink_to(probe.directory / "not-to-be-read")
        with patch.object(tls, "capture") as capture:
            with self.assertRaisesRegex(tls.Failure, "INPUT_PATH"):
                probe.compose("config")
            capture.assert_not_called()

    def test_failed_ownership_scan_never_removes_anything(self):
        probe = self.probe()
        probe.resources = Mock(side_effect=tls.Failure("NETWORK_FOREIGN_MEMBER"))
        probe.call = Mock()
        with self.assertRaises(tls.Failure):
            probe.cleanup()
        probe.call.assert_not_called()
        self.assertTrue(probe.uncertain)
        self.assertTrue(probe.directory.exists())

    def test_failed_command_output_is_not_an_exception_or_console_payload(self):
        probe = self.probe()
        output = io.StringIO()
        with patch.object(tls, "capture", return_value=(1, tls.SECRET + " provider-body")), contextlib.redirect_stdout(output):
            with self.assertRaisesRegex(tls.Failure, "^DOCKER_FAILED_exec$"):
                probe.call("exec", CID, "example")
        self.assertEqual(output.getvalue(), "")

    def test_capture_strips_inherited_environment_and_bounds_output_and_time(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict(os.environ, AWS_SECRET_ACCESS_KEY="must-not-inherit", DOCKER_HOST="tcp://live:2375"):
                code, raw = tls.capture([sys.executable, "-c", "import os,json; print(json.dumps(dict(os.environ)))"], directory, 3)
            self.assertEqual(code, 0)
            env = json.loads(raw)
            self.assertNotIn("AWS_SECRET_ACCESS_KEY", env)
            self.assertNotIn("DOCKER_HOST", env)
            self.assertEqual(env["HOME"], directory)
            with self.assertRaisesRegex(tls.Failure, "OUTPUT_LIMIT_UNCONFIRMED"):
                tls.capture([sys.executable, "-c", "print('x' * 1100000)"], directory, 3)
            with self.assertRaisesRegex(tls.Failure, "COMMAND_UNCONFIRMED"):
                tls.capture([sys.executable, "-c", "import time; time.sleep(5)"], directory, .05)

    def test_certificate_error_requires_same_connection_attempt_and_listener(self):
        self.assertTrue(tls.trust_rejected(stunnel_log(), "source", "ca"))
        for raw in ("certificate verify failed: unable to get local issuer certificate",
                    stunnel_log("metadata"), stunnel_log().replace("LOG3[7]", "LOG3[8]"),
                    stunnel_log().replace("accepted connection", "configured"),
                    stunnel_log().replace("s_connect: connected", "DNS lookup failed"),
                    stunnel_log().replace("s_connect: connected", "s_connect: disconnected"),
                    stunnel_log(error="Connection refused"), stunnel_log(error="Connection timed out")):
            with self.subTest(raw=raw):
                self.assertFalse(tls.trust_rejected(raw, "source", "ca"))

    def test_trust_reasons_cannot_substitute_for_each_other(self):
        hostname = stunnel_log(error="CERT: Subject checks failed")
        unsupported = stunnel_log(error="PostgreSQL server rejected TLS")
        self.assertTrue(tls.trust_rejected(hostname, "source", "hostname"))
        self.assertTrue(tls.trust_rejected(unsupported, "source", "no-ssl"))
        self.assertFalse(tls.trust_rejected(hostname, "source", "ca"))
        self.assertFalse(tls.trust_rejected(stunnel_log(), "source", "hostname"))
        self.assertFalse(tls.trust_rejected(stunnel_log(), "source", "no-ssl"))

    def test_source_needs_separate_sql_and_wal_actual_connect_failures(self):
        database, replication = "a" * 36, "b" * 36
        sql = "Postgrex.Protocol (#PID<0.123.0>) failed to connect: ** (DBConnection.ConnectionError) tcp recv (idle): closed"
        wal = "[SlotProducer] replication connect failed: tcp recv (idle): closed " + f"database_id={database} replication_id={replication} "
        classify = lambda text: tls.source_failure_events(text, database, replication)
        self.assertEqual(classify(sql + "\n" + wal), ({"<0.123.0>"}, True))
        self.assertEqual(classify(sql), ({"<0.123.0>"}, False))
        self.assertEqual(classify(wal), (set(), True))
        for raw in (sql.replace("failed to connect", "disconnected"), sql.replace("closed", "timeout"),
                    wal.replace(database, "c" * 36), wal.replace(replication, replication + "extra"),
                    wal.replace("SlotProducer", "SlotProducerStatus"), wal.replace("connect failed", "disconnected"),
                    wal.replace("closed", "closed_unrecognized"), sql.replace("closed", "closed_unrecognized"),
                    wal.replace("tcp recv (idle): closed", "timeout"), wal.replace("database_id=", "other_database_id="),
                    "slot inactive; no webhook delivery", "SQL and WAL timed out", stunnel_log()):
            self.assertEqual(classify(raw), (set(), False))
        self.assertEqual(classify(wal.replace(" database_id=", "\ndatabase_id=")), (set(), False))

    def test_sql_failure_pid_requires_readonly_runtime_identity_not_just_log(self):
        probe = self.probe()
        probe.ids = {"sequin": CID}
        probe.call = Mock(return_value="TLS_SOURCE_SQL_UNCONFIRMED\n")
        self.assertFalse(probe.source_sql_failed(set()))
        probe.call.assert_not_called()
        for pids in ({"<0.1.0>;System.cmd(\"bad\",[])"}, {f"<0.{i}.0>" for i in range(17)}):
            with self.assertRaisesRegex(tls.Failure, "SQL_PID_INPUT"):
                probe.source_sql_failed(pids)
        probe.call.assert_not_called()
        self.assertFalse(probe.source_sql_failed({"<0.123.0>"}))
        args, kwargs = probe.call.call_args
        self.assertEqual(args[:4], ("exec", CID, "sequin-server", "rpc"))
        self.assertEqual(kwargs, {"seconds": 15})
        for text in (":sys.get_state", "s.mod == Postgrex.Protocol", "port: 15433", "database: \"tls_source\"", "ssl: false"):
            self.assertIn(text, args[4])
        self.assertNotIn("start_link", args[4])
        probe.call.return_value = "TLS_SOURCE_SQL_CONFIRMED\n"
        self.assertTrue(probe.source_sql_failed({"<0.123.0>"}))
        probe.call.return_value = "unexpected TLS_SOURCE_SQL_CONFIRMED\n"
        self.assertFalse(probe.source_sql_failed({"<0.123.0>"}))

    def test_native_config_has_no_slot_creation_backfill_or_direct_tls_bypass(self):
        config = tls.source_config()
        database = config["databases"][0]
        self.assertEqual((database["hostname"], database["port"], database["ssl"]), ("127.0.0.1", 15433, False))
        self.assertFalse(database["slot"]["create_if_not_exists"])
        self.assertFalse(database["publication"]["create_if_not_exists"])
        self.assertFalse(config["sinks"][0]["initial_backfill"])
        good = tls.stunnel_config()
        for listener in ("metadata", "source"):
            other = "source" if listener == "metadata" else "metadata"
            for reason in ("ca", "hostname"):
                changed = tls.stunnel_config(listener, reason)
                section = lambda text: text.split("[" + other + "]\n")[1].split("\n[")[0]
                self.assertEqual(section(changed), section(good))
                self.assertEqual(changed.count("verifyChain = yes"), 2)
                self.assertEqual(changed.count("protocol = pgsql"), 2)
                self.assertNotIn("accept = 0.0.0.0", changed)

    def test_config_reload_replaces_readonly_file_and_changes_visible_directory_entry(self):
        probe = self.probe()
        probe.write("sequin-postgres-tls/stunnel.conf", "first")
        probe.write("sequin-postgres-tls/stunnel.conf", "second")
        file = probe.directory / "sequin-postgres-tls/stunnel.conf"
        self.assertEqual(file.read_text(), "second")
        self.assertEqual(file.stat().st_mode & 0o777, 0o444)

    def test_webhook_import_does_not_need_repository_ancestors_or_host_helpers(self):
        namespace = {"__name__": "webhook_import", "__file__": "/smoke/smoke-sequin-tls.py"}
        exec(compile(Path(tls.__file__).read_text(), "/smoke/smoke-sequin-tls.py", "exec"), namespace)
        self.assertNotIn("r3", namespace)
        self.assertIn("Webhook", namespace)

    def test_webhook_retains_only_valid_synthetic_ids_and_rejects_oversized_body(self):
        handler = object.__new__(tls.Webhook)
        handler.path = "/cdc"
        handler.reply = Mock()
        tls.Webhook.ids = set()
        tls.Webhook.invalid = 0
        body = json.dumps(dict(action="insert", record=dict(id=1, marker="committed"),
            metadata=dict(table_schema="public", table_name="tls_events"))).encode()
        handler.headers = {"Content-Length": str(len(body))}
        handler.rfile = io.BytesIO(body)
        handler.do_POST()
        handler.reply.assert_called_with(200, {})
        self.assertEqual(tls.Webhook.ids, {1})
        handler.headers = {"Content-Length": "16385"}
        handler.rfile = Mock()
        handler.do_POST()
        handler.rfile.read.assert_not_called()
        handler.reply.assert_called_with(400, {})
        self.assertEqual(tls.Webhook.invalid, 1)
        self.assertEqual(tls.Webhook.ids, {1})

    def test_start_scope_refuses_search_and_all_applications_before_docker(self):
        probe = self.probe()
        probe.compose = Mock()
        for name in ("opensearch", "api", "crawler", "cron", "worker"):
            with self.assertRaisesRegex(tls.Failure, "START_SCOPE"):
                probe.up(name)
        probe.compose.assert_not_called()

    def test_model_rejects_live_mount_ports_network_cloud_and_metadata_url(self):
        directory = Path("/synthetic-fixture")
        images = {s: IMAGE for s in tls.SERVICES}
        model = dict(networks={"backend": dict(name=PROJECT + "_backend", internal=True, labels={tls.OWNER: TOKEN})}, services={})
        for name in tls.SERVICES:
            svc = dict(image=IMAGE, pull_policy="never", labels={tls.OWNER: TOKEN}, environment=tls.fixture_environment(name),
                logging=copy.deepcopy(tls.LOGGING), security_opt=list(tls.SECURITY), command=tls.COMMANDS.get(name),
                restart="no" if name == "webhook" else "unless-stopped", networks={"backend": {}},
                volumes=[dict(type="bind", source=s, target=t, read_only=True, bind={"create_host_path": False})
                         for s, t in tls.binds(directory)[name].items()])
            model["services"][name] = svc
        sidecar = model["services"][tls.TLS_SERVICE]
        sidecar.update(user="10001:10001", read_only=True, cap_drop=["ALL"], networks={"backend": {"aliases": ["sequin"]}})
        sequin = model["services"]["sequin"]
        sequin.pop("networks")
        sequin.update(network_mode="service:sequin-postgres-tls",
            entrypoint=["/usr/bin/env", "-u", "PG_URL", "/scripts/start_commands.sh"])
        sequin["environment"].update(PG_HOSTNAME="127.0.0.1", PG_PORT="15432", PG_SSL="false", PG_URL="")
        validate = lambda m: tls.validate_model(m, directory, PROJECT, TOKEN, images)
        validate(model)
        for key, value in (("entrypoint", None), ("entrypoint", []),
                ("entrypoint", ["/scripts/start_commands.sh"]),
                ("entrypoint", ["/usr/bin/env", "PG_URL=", "/scripts/start_commands.sh"]),
                ("entrypoint", ["/bin/sh", "-c", "unset PG_URL; /scripts/start_commands.sh"]),
                ("command", ["extra"])):
            bad = copy.deepcopy(model)
            bad["services"]["sequin"][key] = value
            with self.subTest(key=key, value=value), self.assertRaisesRegex(tls.Failure, "^SEQUIN_PG_URL_WRAPPER$"):
                validate(bad)
        for name in tls.SERVICES:
            for key, value in (("use_api_socket", True), ("post_start", [{"command": "id", "privileged": True}]),
                    ("pre_stop", [{"command": "id"}]), ("env_file", ["/live/credentials.env"]),
                    ("security_opt", ["seccomp:unconfined"]),
                    ("security_opt", tls.SECURITY + ["apparmor:unconfined"]),
                    ("logging", {"driver": "syslog", "options": {"syslog-address": "tcp://untrusted.invalid:514"}}),
                    ("logging", {"driver": "local", "options": {"max-size": "1m", "max-file": "1", "env": "PG_PASSWORD"}})):
                bad = copy.deepcopy(model)
                bad["services"][name][key] = value
                with self.subTest(name=name, key=key), self.assertRaises(tls.Failure):
                    validate(bad)
            for key in ("HTTP_PROXY", "LD_PRELOAD", "CONFIG_FILE_YAML", "PGOPTIONS", "ARBITRARY"):
                bad = copy.deepcopy(model)
                bad["services"][name]["environment"][key] = "not-fixture"
                with self.subTest(name=name, key=key), self.assertRaisesRegex(tls.Failure, "MODEL_FIXTURE_ENV"):
                    validate(bad)
            bad = copy.deepcopy(model)
            bad["services"][name].pop("environment")
            with self.assertRaises(tls.Failure):
                validate(bad)
        bad = copy.deepcopy(model)
        bad["volumes"] = {"postgres-data": {"external": True}}
        with self.assertRaisesRegex(tls.Failure, "MODEL_EXTRA_RESOURCES"):
            validate(bad)
        bad = copy.deepcopy(model)
        bad["networks"]["backend"]["internal"] = False
        with self.assertRaises(tls.Failure):
            validate(bad)
        for name, key, value in (("postgres", "ports", [5432]), ("postgres", "volumes", []),
                                ("redis", "network_mode", "host"), (tls.TLS_SERVICE, "read_only", False),
                                (tls.TLS_SERVICE, "user", "0"), ("sequin", "networks", {"backend": {}})):
            bad = copy.deepcopy(model)
            bad["services"][name][key] = value
            with self.subTest(key=key), self.assertRaises(tls.Failure):
                validate(bad)
        for key, value in (("PG_URL", "postgres://live"), ("PG_SSL", "true"), ("AWS_PROFILE", "live"), ("GOOGLE_API_KEY", "live")):
            bad = copy.deepcopy(model)
            bad["services"]["sequin"]["environment"][key] = value
            with self.subTest(key=key), self.assertRaises(tls.Failure):
                validate(bad)
        bad = copy.deepcopy(model)
        bad["services"]["postgres"]["volumes"][0]["source"] = "/var/lib/live-postgresql"
        with self.assertRaisesRegex(tls.Failure, "MODEL_BINDS"):
            validate(bad)


if __name__ == "__main__":
    unittest.main()
