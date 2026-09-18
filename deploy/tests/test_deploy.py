"""Bounded pure host-command regressions. No Docker/provider operations."""
import contextlib
import copy
import fcntl
import hashlib
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch


def load_deploy(path=None):
    path = path or Path(__file__).resolve().parents[1] / "bin/deploy"
    loader = importlib.machinery.SourceFileLoader("r4_deploy", str(path))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


deploy = load_deploy()
A = dict(source_sha="672bdcefdeaabc6cd9f78461ec1bf859c31dc443",
         images={key: "sha256:" + str(i) * 64 for i, key in enumerate(deploy.BINS, 1)})
B = dict(source_sha="08173849aa0a5f5849a340a1030b94af2162aee8",
         images={key: "sha256:" + str(i) * 64 for i, key in enumerate(deploy.BINS, 5)})


def running(selected, slot):
    return dict(release=selected, api_slot=slot, containers={name: hashlib.sha256(
        (selected["source_sha"] + name).encode()).hexdigest() for name in (*deploy.WORKERS, *deploy.JOBS, slot)})


class DeployTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="aura-r4-unit-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        (self.directory / "state").mkdir(mode=0o700)
        (self.directory / "caddy").mkdir(mode=0o700)
        for name in ("compose.env", "ca.pem", "caddy/Caddyfile"):
            (self.directory / name).write_text("synthetic only\n")
            (self.directory / name).chmod(0o600)
        self.config = dict(stage="test", application_project="r4-unit-app", edge_project="r4-unit-edge",
            config_dir=str(self.directory), compose_env=str(self.directory / "compose.env"),
            state_dir=str(self.directory / "state"), https_port=12345, https_ca=str(self.directory / "ca.pem"))
        self.host = deploy.Host(self.config)
        self.host.acquire = Mock()
        self.host.inputs = Mock(return_value={})
        self.host.images = Mock(return_value={})
        self.host.model = Mock(return_value={})
        self.host.compose = Mock()
        self.host.ready = Mock()
        self.current = running(A, "api")
        self.target = running(B, "api-candidate")
        self.host.converged = Mock(return_value=self.current)
        self.output = io.StringIO()
        self.enterContext(contextlib.redirect_stdout(self.output))

    def execute(self, action="apply", confirm=False, selected=None, slot="api"):
        self.host.execute(action, copy.deepcopy(selected or (B if action == "apply" else A)), slot, confirm)

    def marker(self, phase="candidate-start"):
        return {"from": self.current, "previous": None, "target": B, "api_slot": "api-candidate", "phase": phase}

    def test_release_schema_and_immutable_ids(self):
        self.assertEqual(deploy.release(copy.deepcopy(A)), A)
        cases = [None, {}, dict(A, extra=True), dict(A, source_sha="a" * 40), dict(A, source_sha="HEAD"),
                 dict(A, images={"api": A["images"]["api"]})]
        for value in ("image:latest", "repo@sha256:" + "a" * 64, "sha256:" + "g" * 64, 1):
            bad = copy.deepcopy(A)
            bad["images"]["api"] = value
            cases.append(bad)
        for value in cases:
            with self.subTest(value=type(value).__name__), self.assertRaises(deploy.Failure):
                deploy.release(value)

    def test_trusted_json_and_host_schema(self):
        path = self.directory / "release.json"
        deploy.atomic(path, '{"source_sha":1,"source_sha":2}')
        with self.assertRaisesRegex(deploy.Failure, "DUPLICATE_JSON_KEY"):
            deploy.read_json(path)
        path.chmod(0o666)
        with self.assertRaisesRegex(deploy.Failure, "UNTRUSTED_FILE"):
            deploy.read_json(path)
        with self.assertRaisesRegex(deploy.Failure, "UNTRUSTED_PATH"):
            deploy.trusted("relative.json")
        for config in (dict(self.config, extra=True), dict(self.config, https_port=True),
                       dict(self.config, edge_project=self.config["application_project"])):
            with self.assertRaises(deploy.Failure):
                deploy.Host(config)
        (self.directory / "state").chmod(0o755)
        with self.assertRaisesRegex(deploy.Failure, "STATE_PERMISSIONS"):
            deploy.Host(self.config)

    def application_models(self):
        # Reuse R3's pure model fixture; these durations came from real Compose config.
        from test_smoke_compose import SmokeComposeTests
        fixture = SmokeComposeTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        fixture.images.update(A["images"])
        model = fixture.model("application")
        model["services"]["api-candidate"] = copy.deepcopy(model["services"]["api"])
        raw = {"services": {}}
        for name, service in model["services"].items():
            component = deploy.kind(name)
            service.update(restart="unless-stopped", stop_grace_period={
                "api": "1m0s", "worker": "5m0s", "cron": "5m30s", "crawler": "5m30s"}[component])
            filenames = [component + ".env"]
            if name in deploy.SEARCH_WORKER_ENVFILES:
                filenames.append(deploy.SEARCH_WORKER_ENVFILES[name])
            if name == "notification-delivery":
                filenames.append("notification-delivery.env")
            raw["services"][name] = {"env_file": [{"path": str(fixture.directory / f)} for f in filenames]}
        self.host.files = fixture.directory
        return raw, model

    def test_compose_canonical_stop_budgets_are_accepted(self):
        raw, model = self.application_models()
        self.host.compose.side_effect = [json.dumps(raw), json.dumps(model)]
        self.assertEqual(deploy.Host.model(self.host, A), model)

    def test_search_ca_exact_receivers_and_mount_denials(self):
        raw, model = self.application_models()
        receivers = {"api", "api-candidate", "cron", "product-listing-opensearch",
                     "search-filter-projection", "search-filter-percolator"}
        ca = dict(type="bind", source=str(self.host.files / "opensearch-ca.pem"),
                  target="/run/aura/opensearch-ca.pem", read_only=True,
                  bind={"create_host_path": False})
        self.assertEqual({name for name, service in model["services"].items()
                          if ca in service["volumes"]}, receivers)
        for name, service in model["services"].items():
            mounts = service["volumes"]
            variants = [mounts + [ca]]  # Duplicate for receivers; forbidden extra for others.
            if name in receivers:
                without = [m for m in mounts if m != ca]
                variants += [without, without + [dict(ca, read_only=False)],
                             without + [dict(ca, target="/run/aura/wrong-ca.pem")],
                             without + [dict(ca, source=str(self.host.files / "postgres-ca.pem"))]]
            for index, changed in enumerate(variants):
                altered = copy.deepcopy(model)
                altered["services"][name]["volumes"] = changed
                self.host.compose.side_effect = [json.dumps(raw), json.dumps(altered)]
                with self.subTest(service=name, variant=index), self.assertRaisesRegex(deploy.Failure, "^APPLICATION_MOUNTS$"):
                    deploy.Host.model(self.host, A)

    def test_real_stage_requires_scoped_search_identity(self):
        raw, model = self.application_models()
        receivers = {"api", "api-candidate", "cron", "product-listing-opensearch",
                     "search-filter-projection", "search-filter-percolator"}
        self.host.config["stage"] = "dev"
        for name, service in model["services"].items():
            service["environment"].update(STAGE="dev", POSTGRES_SSL_MODE="verify-full",
                                           POSTGRES_SSL_ROOT_CERT="/run/aura/postgres-ca.pem")
            if name in receivers:
                service["environment"]["OPENSEARCH_SSL_ROOT_CERT"] = "/run/aura/opensearch-ca.pem"
        self.host.compose.side_effect = [json.dumps(raw), json.dumps(model)]
        deploy.Host.model(self.host, A)
        for name in deploy.SEARCH_IDENTITIES:
            wrong_username = ("aura_product_projector"
                              if deploy.SEARCH_IDENTITIES[name] == "aura_reader" else "aura_reader")
            for key, value in (("OPENSEARCH_USERNAME", wrong_username),
                               ("OPENSEARCH_PASSWORD", ""), ("OPENSEARCH_PASSWORD", " ")):
                altered = copy.deepcopy(model)
                altered["services"][name]["environment"][key] = value
                self.host.compose.side_effect = [json.dumps(raw), json.dumps(altered)]
                with self.subTest(service=name, field=key), self.assertRaisesRegex(
                        deploy.Failure, "^SEARCH_RUNTIME_IDENTITY$"):
                    deploy.Host.model(self.host, A)
        for name in deploy.SEARCH_IDENTITIES:
            altered = copy.deepcopy(model)
            altered["services"][name]["environment"].pop("OPENSEARCH_PASSWORD", None)
            self.host.compose.side_effect = [json.dumps(raw), json.dumps(altered)]
            with self.subTest(service=name, field="missing password"), self.assertRaisesRegex(
                    deploy.Failure, "^SEARCH_RUNTIME_IDENTITY$"):
                deploy.Host.model(self.host, A)

    def test_scoped_secret_files_require_private_mode_and_exact_paths(self):
        raw, model = self.application_models()
        for filename in deploy.SEARCH_SECRET_FILES:
            path = self.host.files / filename
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        product = self.host.files / deploy.SEARCH_WORKER_ENVFILES["product-listing-opensearch"]
        product.chmod(0o644)
        with self.assertRaisesRegex(deploy.Failure, "^ENV_FILE_PERMISSIONS$"):
            deploy.Host.inputs(self.host)
        product.chmod(0o600)
        original = product.read_text()
        product.write_text(original + "STAGE=test\n")
        self.host.compose.side_effect = [json.dumps(raw), json.dumps(model)]
        with self.assertRaisesRegex(deploy.Failure, "^SEARCH_SECRET_FIELDS$"):
            deploy.Host.model(self.host, A)
        product.write_text(original)
        original_stat = Path.stat
        other_owner = 1 if os.geteuid() != 1 else 2
        def foreign_owner(candidate, *args, **kwargs):
            info = original_stat(candidate, *args, **kwargs)
            if candidate == product:
                values = list(info)
                values[4] = other_owner
                return os.stat_result(values)
            return info
        with patch.object(Path, "stat", foreign_owner):
            with self.assertRaisesRegex(deploy.Failure, "^UNTRUSTED_FILE$"):
                deploy.Host.inputs(self.host)
        product.unlink()
        with self.assertRaisesRegex(deploy.Failure, "^UNTRUSTED_FILE$"):
            deploy.Host.inputs(self.host)
        raw, model = self.application_models()
        altered = copy.deepcopy(raw)
        entries = altered["services"]["product-listing-opensearch"]["env_file"]
        entries[1]["path"] = str(self.host.files / "search-filter-projection.env")
        self.host.compose.side_effect = [json.dumps(altered), json.dumps(model)]
        with self.assertRaisesRegex(deploy.Failure, "^ENV_FILE_LOCATION$"):
            deploy.Host.model(self.host, A)

    def test_real_stage_requires_exact_search_ca_path_only_for_receivers(self):
        raw, model = self.application_models()
        receivers = {"api", "api-candidate", "cron", "product-listing-opensearch",
                     "search-filter-projection", "search-filter-percolator"}
        key, path = "OPENSEARCH_SSL_ROOT_CERT", "/run/aura/opensearch-ca.pem"
        self.host.call = Mock()
        for stage in ("dev", "prod"):
            # Test the pure model gate, not real-stage Host/root authority or execution.
            self.host.config["stage"] = stage
            for name, service in model["services"].items():
                env = service["environment"]
                env.update(STAGE=stage, POSTGRES_SSL_MODE="verify-full",
                           POSTGRES_SSL_ROOT_CERT="/run/aura/postgres-ca.pem")
                env.pop(key, None)
                if name in receivers:
                    env[key] = path
            self.host.compose.side_effect = [json.dumps(raw), json.dumps(model)]
            deploy.Host.model(self.host, A)
            for name in sorted(receivers):
                for value in (None, "", "/run/aura/postgres-ca.pem", "/run/aura/wrong-ca.pem", " " + path, path + " "):
                    altered = copy.deepcopy(model)
                    env = altered["services"][name]["environment"]
                    env.pop(key)
                    if value is not None:
                        env[key] = value
                    self.host.compose.side_effect = [json.dumps(raw), json.dumps(altered)]
                    with self.subTest(stage=stage, service=name), self.assertRaisesRegex(deploy.Failure, "^SEARCH_CA_REQUIRED$"):
                        deploy.Host.model(self.host, A)
            self.host.call.assert_not_called()
            self.host.ready.assert_not_called()

    def test_current_and_incomplete_guards(self):
        with self.assertRaisesRegex(deploy.Failure, "ADOPT_RUNNING_RELEASE_FIRST"):
            self.execute()
        self.host.record("current", self.current)
        with self.assertRaisesRegex(deploy.Failure, "ALREADY_ADOPTED"):
            self.execute("adopt")
        self.host.record("incomplete", {"phase": "preflight"})
        with self.assertRaisesRegex(deploy.Failure, "INCOMPLETE_REQUIRES_RECOVERY"):
            self.execute()
        self.host.compose.assert_not_called()
        self.host.converged.assert_not_called()

    def test_real_os_flock_contention(self):
        holder = os.open(self.host.state / "lock", os.O_CREAT | os.O_RDWR, 0o600)
        self.addCleanup(os.close, holder)
        fcntl.flock(holder, fcntl.LOCK_EX | fcntl.LOCK_NB)
        contender = deploy.Host(self.config)
        try:
            with self.assertRaisesRegex(deploy.Failure, "DEPLOYMENT_BUSY"):
                contender.acquire()
        finally:
            if contender.lock is not None:
                os.close(contender.lock)

    def test_failed_stop_prevents_successor(self):
        self.host.record("current", self.current)
        self.host.containers = Mock(return_value={n: {"Id": identifier}
            for n, identifier in self.current["containers"].items()})
        self.host.call = Mock(return_value=(0, json.dumps([dict(State=dict(
            Running=True, Restarting=False, Pid=123, OOMKilled=False, ExitCode=0))])))
        with self.assertRaisesRegex(deploy.Failure, "STOP_NOT_CLEAN"):
            self.execute()
        marker = self.host.stored("incomplete")
        self.assertEqual(marker["phase"], "stop-" + deploy.WORKERS[0])
        starts = [call.args[-1] for call in self.host.compose.call_args_list if "up" in call.args]
        self.assertEqual(starts, ["api-candidate"])
        self.assertEqual(self.host.stored("current"), self.current)
        self.assertIsNone(self.host.stored("previous"))
        self.assertEqual(self.host.call.call_args_list[0].args,
            ("stop", "--time", "300", self.current["containers"][deploy.WORKERS[0]]))
        self.assertFalse(any("stop" in call.args for call in self.host.compose.call_args_list))

    def test_changed_stop_target_prevents_mutation(self):
        self.host.containers = Mock(return_value={"api": {"Id": "replacement"}})
        self.host.call = Mock()
        with self.assertRaisesRegex(deploy.Failure, "STOP_TARGET_CHANGED"):
            self.host.stop_remove("api", "captured-old-id", A)
        self.host.compose.assert_not_called()
        self.host.call.assert_not_called()
        self.assertIsNone(self.host.stored("incomplete"))

    def test_unknown_docker_result_leaves_incomplete_and_blocks_retry(self):
        self.host.record("current", self.current)
        self.host.compose.side_effect = deploy.Failure("DOCKER_OUTCOME_UNKNOWN")
        with self.assertRaisesRegex(deploy.Failure, "DOCKER_OUTCOME_UNKNOWN"):
            self.execute()
        self.assertEqual(self.host.stored("incomplete")["phase"], "preflight-api-candidate")
        with self.assertRaisesRegex(deploy.Failure, "INCOMPLETE_REQUIRES_RECOVERY"):
            self.execute()
        self.assertEqual(self.host.compose.call_count, 1)

    def test_interruption_in_preflight_preserves_uncertain_marker(self):
        self.host.record("current", self.current)
        self.host.compose.side_effect = deploy.Failure("INTERRUPTED_OUTCOME_UNKNOWN")
        with self.assertRaisesRegex(deploy.Failure, "INTERRUPTED_OUTCOME_UNKNOWN"):
            self.execute()
        self.assertEqual(self.host.stored("incomplete")["phase"], "preflight-api-candidate")
        self.host.ready.assert_not_called()

    def test_recover_needs_confirmation_and_refuses_mixed_state(self):
        self.host.record("current", self.current)
        self.host.record("incomplete", self.marker())
        with self.assertRaisesRegex(deploy.Failure, "RECOVERY_REQUIRES_FINISHED_COMMANDS_CONFIRMATION"):
            self.execute("recover")
        self.host.converged.assert_not_called()
        self.host.converged.side_effect = deploy.Failure("MIXED_APPLICATION_STATE")
        with self.assertRaisesRegex(deploy.Failure, "MIXED_APPLICATION_STATE"):
            self.execute("recover", confirm=True)
        self.assertEqual(self.host.stored("incomplete"), self.marker())
        self.host.compose.assert_not_called()

    def test_converged_rejects_extra_slot_before_probes(self):
        self.host.containers = Mock(return_value={n: {} for n in deploy.SERVICES})
        with self.assertRaisesRegex(deploy.Failure, "MIXED_APPLICATION_STATE"):
            deploy.Host.converged(self.host, A, "api")
        self.host.ready.assert_not_called()

    def test_recover_malformed_json_retains_state(self):
        deploy.atomic(self.host.state / "incomplete", '{"phase":')
        with self.assertRaises(json.JSONDecodeError):
            self.execute("recover", confirm=True)
        self.assertTrue((self.host.state / "incomplete").exists())
        self.host.converged.assert_not_called()

    def test_recover_only_records_verified_convergence(self):
        marker = self.marker()
        self.host.record("current", self.current)
        self.host.record("incomplete", marker)
        self.execute("recover", confirm=True)
        self.assertEqual(self.host.stored("last-incomplete"), marker)
        self.assertEqual(self.host.stored("current"), self.current)
        self.assertIsNone(self.host.stored("previous"))
        self.assertIsNone(self.host.stored("incomplete"))
        self.host.compose.assert_not_called()

    def test_successful_json_stdout_excludes_captured_stderr(self):
        host = deploy.Host(self.config)
        def warned(*args, **kwargs):
            kwargs["stdout"].write(b'{"admin": {}}')
            kwargs["stderr"].write(b"synthetic-secret warning: config formatting\n")
            return subprocess.CompletedProcess(args, 0)
        with patch.object(deploy.subprocess, "run", side_effect=warned):
            code, output = host.call("exec", "fixture", "caddy", "adapt")
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(output), {"admin": {}})
        self.assertNotIn("synthetic-secret", output + self.output.getvalue())

    def test_subprocess_failure_and_timeout_hide_payload(self):
        secret = "synthetic-secret-do-not-print"
        host = deploy.Host(self.config)
        def failed(*args, **kwargs):
            kwargs["stdout"].write(secret.encode())
            return subprocess.CompletedProcess(args, 1)
        with patch.object(deploy.subprocess, "run", side_effect=failed):
            with self.assertRaisesRegex(deploy.Failure, "^DOCKER_FAILED_INCOMPLETE$"):
                host.call("inspect", "fixture")
        with patch.object(deploy.subprocess, "run", side_effect=subprocess.TimeoutExpired(secret, 1, output=secret)):
            with self.assertRaisesRegex(deploy.Failure, "^DOCKER_OUTCOME_UNKNOWN$"):
                host.call("inspect", "fixture")
        config = self.directory / "host.json"
        deploy.atomic(config, json.dumps(self.config))
        stderr = io.StringIO()
        previous = {sig: signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
        try:
            with patch.object(deploy.Host, "execute", side_effect=ValueError(secret)), contextlib.redirect_stderr(stderr):
                self.assertEqual(deploy.main(["--config", str(config), "status"]), 1)
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)
        self.assertEqual(json.loads(stderr.getvalue())["code"], "INPUT_OR_DEPENDENCY_FAILED")
        self.assertNotIn(secret, stderr.getvalue() + self.output.getvalue())

    def test_frozen_inputs_reject_bytes_mode_and_owner_drift_before_mutation(self):
        static = self.directory / "static"
        static.mkdir()
        (self.directory / "deploy").mkdir()
        names = ("api.env", "worker.env", "notification-delivery.env", "cron.env", "crawler.env",
                 "product-listing-opensearch.env", "search-filter-projection.env", "search-filter-percolator.env",
                 "postgres-ca.pem", "opensearch-ca.pem", "google-adc.json")
        for name in names:
            content = ("OPENSEARCH_USERNAME=aura_reader\nOPENSEARCH_PASSWORD=synthetic\n"
                       if name in deploy.SEARCH_SECRET_FILES else "synthetic\n")
            deploy.atomic(self.directory / name, content)
        deploy.atomic(self.directory / "deploy/catalog.json", "{}")
        for name in ("compose.application.yml", "compose.replace.yml", "compose.edge.yml", "Caddyfile.replace"):
            deploy.atomic(static / name, "synthetic\n")
        self.host.inputs = lambda: deploy.Host.inputs(self.host)
        self.host.call = Mock()
        self.host.marker = self.marker()
        self.host.record("incomplete", self.host.marker)
        marker_bytes = (self.host.state / "incomplete").read_bytes()
        def refused():
            with self.assertRaisesRegex(deploy.Failure, "HOST_INPUTS_CHANGED"):
                self.host.phase("candidate-start")
            with self.assertRaisesRegex(deploy.Failure, "HOST_INPUTS_CHANGED"):
                deploy.Host.compose(self.host, "application", B, "up", "api-candidate")
            self.host.call.assert_not_called()
            self.assertEqual((self.host.state / "incomplete").read_bytes(), marker_bytes)
        with patch.object(deploy, "ROOT", self.directory), patch.object(deploy, "COMPOSE", static):
            self.host.snapshot = self.host.inputs()
            self.assertEqual(len(self.host.snapshot), 18)
            ca_path = self.directory / "opensearch-ca.pem"
            self.assertIn(str(ca_path), self.host.snapshot)
            self.assertEqual(self.host.snapshot[str(ca_path)][2], hashlib.sha256(ca_path.read_bytes()).hexdigest())
            for filename in self.host.snapshot:
                path = Path(filename)
                old = path.read_text()
                with self.subTest(path=path.name):
                    deploy.atomic(path, old + "changed")
                    self.assertNotEqual(self.host.inputs()[filename], self.host.snapshot[filename])
                    refused()
                    deploy.atomic(path, old)
            path = ca_path
            path.chmod(0o640)
            refused()
            path.chmod(0o600)
            original_stat = Path.stat
            def other_owner(candidate, *args, **kwargs):
                info = original_stat(candidate, *args, **kwargs)
                if candidate == path:
                    values = list(info)
                    values[4] = 0 if os.geteuid() else 1
                    return os.stat_result(values)
                return info
            if os.geteuid():
                with patch.object(Path, "stat", other_owner):
                    refused()

    def test_actual_process_drift_rejected_before_readiness_probe(self):
        mount = dict(type="bind", source=str(self.directory / "ca.pem"), target="/run/aura/postgres-ca.pem")
        model = {"services": {"api": {"volumes": [mount]}}}
        item = dict(Image=A["images"]["api"], Id=self.current["containers"]["api"], RestartCount=0,
            State=dict(Running=True, OOMKilled=False, Restarting=False),
            Config=dict(User="10001:10001", Entrypoint=["/usr/local/bin/aura-historia-api"], Cmd=None, StopTimeout=60),
            HostConfig=dict(ReadonlyRootfs=True, Privileged=False, CapAdd=None, CapDrop=["ALL"], SecurityOpt=["no-new-privileges:true"]),
            Mounts=[dict(Type="bind", Source=mount["source"], Destination=mount["target"], RW=False, Propagation="rprivate")])
        self.host.images.return_value = {"api": {}}
        self.host.probe = Mock()
        self.host.call = Mock()
        deploy.Host.actual_process(self.host, item, "api", model)
        changes = [("Config", "User", "0:0"), ("Config", "Entrypoint", ["/bin/sh"]),
            ("Config", "Cmd", ["--other"]), ("Config", "StopTimeout", 1),
            ("HostConfig", "ReadonlyRootfs", False), ("HostConfig", "Privileged", True),
            ("HostConfig", "CapAdd", ["SYS_ADMIN"]), ("HostConfig", "CapDrop", []),
            ("HostConfig", "SecurityOpt", []), ("HostConfig", "Devices", [{"PathOnHost": "/dev/test"}]),
            ("HostConfig", "ExtraHosts", ["api:127.0.0.1"])]
        for section, key, value in changes:
            bad = copy.deepcopy(item)
            bad[section][key] = value
            self.host.containers = Mock(return_value={"api": bad})
            with self.subTest(field=key), self.assertRaisesRegex(deploy.Failure, "RUNNING_PROCESS_DRIFT"):
                deploy.Host.ready(self.host, "api", A, model)
        for key, value in (("Source", "/unowned/ca"), ("RW", True), ("Propagation", "shared"), ("Type", "volume")):
            bad = copy.deepcopy(item)
            bad["Mounts"][0][key] = value
            self.host.containers = Mock(return_value={"api": bad})
            with self.subTest(mount=key), self.assertRaisesRegex(deploy.Failure, "RUNNING_MOUNT_DRIFT"):
                deploy.Host.ready(self.host, "api", A, model)
        self.host.probe.assert_not_called()
        self.host.call.assert_not_called()

    def edge_fixture(self, slot="api"):
        backend = "r4-unit-backend"
        edge_net = self.config["edge_project"] + "_edge"
        image, identifier, peer = "sha256:" + "e" * 64, "e" * 64, "f" * 64
        api = running(A, slot)["containers"][slot]
        text = deploy.caddyfile(slot, A["source_sha"])
        deploy.atomic(self.host.caddy, text, mode=0o644)
        volumes = {self.config["edge_project"] + "_" + name: dict(Driver="local", Options=None,
            Labels={"com.docker.compose.project": self.config["edge_project"]}) for name in ("caddy-data", "caddy-config")}
        item = dict(Id=identifier, Image=image, State={"Running": True},
            NetworkSettings={"Networks": {backend: {}, edge_net: {}}},
            HostConfig={"PortBindings": {"443/tcp": [{"HostIp": "127.0.0.1", "HostPort": "12345"}]}},
            Mounts=[dict(Type="bind", Source=str(self.host.caddy.parent), Destination="/etc/caddy", RW=False),
                dict(Type="volume", Name=self.config["edge_project"] + "_caddy-data", Destination="/data", RW=True),
                dict(Type="volume", Name=self.config["edge_project"] + "_caddy-config", Destination="/config", RW=True)])
        state = dict(item=item, volumes=volumes, image=image, container_text=text, active={"admin": {}},
            peers={api: {"Name": "/" + self.config["application_project"] + "-" + slot + "-1",
                         "NetworkSettings": {"Networks": {backend: {"Aliases": [slot], "DNSNames": [slot]}}}},
                   peer: {"Name": "/unrelated-worker", "NetworkSettings": {"Networks": {backend: {"Aliases": [], "DNSNames": []}}}},
                   identifier: {"Name": "/" + self.config["edge_project"] + "-caddy-1",
                                "NetworkSettings": {"Networks": {backend: {"Aliases": ["caddy"]}, edge_net: {"Aliases": ["caddy"]}}}}})
        self.host.containers = Mock(side_effect=lambda target: {"caddy": state["item"]} if target == "edge" else {slot: {"Id": api}})
        self.host.compose = Mock(side_effect=lambda *args: json.dumps(dict(services={"caddy": {"image": state["image"]}}, networks={"backend": {"name": backend}})))
        def call(*args, **kwargs):
            if args[:2] == ("image", "inspect"):
                value = [{"Id": image}]
            elif args[:2] == ("volume", "inspect"):
                value = [state["volumes"][args[2]]]
            elif args[:2] == ("network", "inspect"):
                value = [{"Containers": {i: {} for i in ((api, peer, identifier) if args[2] == backend else (identifier,))}}]
            elif args[0] == "inspect":
                value = [state["peers"][args[1]]]
            elif args[:3] == ("exec", identifier, "cat"):
                return 0, state["container_text"]
            elif args[:4] == ("exec", identifier, "caddy", "adapt"):
                value = {"admin": {}}
            elif args[:3] == ("exec", identifier, "wget"):
                value = state["active"]
            else:
                self.fail("unexpected edge command")
            return 0, json.dumps(value)
        self.host.call = Mock(side_effect=call)
        response = Mock(status=404)
        response.getheader.return_value = A["source_sha"]
        response.read.return_value = b"{}"
        connection = Mock()
        connection.getresponse.return_value = response
        return state, backend, peer, identifier, connection, response

    def test_edge_image_mount_volume_alias_and_config_drift_rejected(self):
        state, backend, peer, identifier, connection, response = self.edge_fixture()
        pristine = copy.deepcopy(state)
        text = state["container_text"]
        changes = [(('image',), 'caddy:latest', 'EDGE_IMAGE_PIN'),
            (('item', 'Image'), 'sha256:' + 'd' * 64, 'EDGE_IMAGE_DRIFT'),
            (('item', 'HostConfig', 'ExtraHosts'), ['api:127.0.0.1'], 'EDGE_NETWORK_DRIFT'),
            (('item', 'HostConfig', 'Dns'), ['127.0.0.1'], 'EDGE_NETWORK_DRIFT'),
            (('item', 'NetworkSettings', 'Networks'), {backend: {}}, 'EDGE_NETWORK_DRIFT'),
            (('item', 'Mounts', 0, 'Source'), '/unowned/config', 'EDGE_MOUNT_DRIFT'),
            (('volumes', self.config['edge_project'] + '_caddy-data', 'Driver'), 'nfs', 'EDGE_VOLUME_DRIFT'),
            (('volumes', self.config['edge_project'] + '_caddy-data', 'Options'), {'device': '/unowned', 'o': 'bind'}, 'EDGE_VOLUME_DRIFT'),
            (('peers', peer, 'NetworkSettings', 'Networks', backend, 'Aliases'), ['api'], 'EDGE_UPSTREAM_ALIAS_CONFLICT'),
            (('container_text',), text + '\n', 'EDGE_CONTAINER_CONFIG_DRIFT'),
            (('active',), {'admin': {'disabled': True}}, 'EDGE_ACTIVE_DRIFT')]
        with patch.object(deploy.ssl, "create_default_context"), patch.object(deploy.http.client, "HTTPSConnection", return_value=connection) as https:
            self.assertEqual(self.host.edge("api", A), identifier)
            for path, value, error in changes:
                state.clear()
                state.update(copy.deepcopy(pristine))
                target = state
                for part in path[:-1]:
                    target = target[part]
                target[path[-1]] = value
                https.reset_mock()
                with self.subTest(error=error), self.assertRaisesRegex(deploy.Failure, error):
                    self.host.edge("api", A)
                https.assert_not_called()
            state.clear()
            state.update(copy.deepcopy(pristine))
            response.getheader.return_value = B["source_sha"]
            with self.assertRaisesRegex(deploy.Failure, "EDGE_SMOKE_FAILED"):
                self.host.edge("api", A)
        self.assertTrue(all(call.args[0] not in {"stop", "rm"} for call in self.host.call.call_args_list))
        self.assertTrue(all(call.args == ("edge", A, "config", "--format", "json") for call in self.host.compose.call_args_list))

    def test_edge_empty_alias_dns_and_container_name_conflicts_for_both_slots(self):
        for slot in ("api", "api-candidate"):
            state, backend, peer, identifier, connection, _ = self.edge_fixture(slot)
            pristine = copy.deepcopy(state)
            with patch.object(deploy.ssl, "create_default_context"), patch.object(deploy.http.client, "HTTPSConnection", return_value=connection) as https:
                # The selected peer claims its name through both fields, but counts once.
                self.assertEqual(self.host.edge(slot, A), identifier)
                for field in ("DNSNames", "Name"):
                    for aliases in ([], None):
                        with self.subTest(slot=slot, field=field, aliases=aliases):
                            state.clear()
                            state.update(copy.deepcopy(pristine))
                            unrelated = state["peers"][peer]
                            endpoint = unrelated["NetworkSettings"]["Networks"][backend]
                            endpoint["Aliases"] = aliases
                            if field == "DNSNames":
                                endpoint["DNSNames"] = [slot]
                            else:
                                unrelated["Name"] = "/" + slot
                            https.reset_mock()
                            self.host.call.reset_mock()
                            with self.assertRaisesRegex(deploy.Failure, "EDGE_UPSTREAM_ALIAS_CONFLICT"):
                                self.host.edge(slot, A)
                            https.assert_not_called()
                            self.assertTrue(all(call.args[0] in {"image", "volume", "network", "inspect"}
                                                for call in self.host.call.call_args_list))

    def test_edge_dns_search_options_and_links_rejected_for_both_slots(self):
        for slot in ("api", "api-candidate"):
            state, _, _, _, connection, _ = self.edge_fixture(slot)
            pristine = copy.deepcopy(state)
            for field, value in (("DnsSearch", ["example.test"]), ("DnsOptions", ["ndots:5"]),
                                 ("Links", ["/unrelated:/" + slot])):
                with self.subTest(slot=slot, field=field):
                    state.clear()
                    state.update(copy.deepcopy(pristine))
                    state["item"]["HostConfig"][field] = value
                    self.host.call.reset_mock()
                    with patch.object(deploy.http.client, "HTTPSConnection", return_value=connection) as https:
                        with self.assertRaisesRegex(deploy.Failure, "EDGE_NETWORK_DRIFT"):
                            self.host.edge(slot, A)
                        https.assert_not_called()
                    self.assertEqual([call.args[0] for call in self.host.call.call_args_list], ["image"])

    def test_candidate_rechecked_before_switch_and_before_old_api_retirement(self):
        for failed_check, phase in ((2, "start-crawler"), (3, "proxy-switch")):
            with self.subTest(failed_check=failed_check):
                self.host.record("current", self.current)
                (self.host.state / "incomplete").unlink(missing_ok=True)
                self.host.stop_remove = Mock()
                self.host.call = Mock()
                self.host.edge = Mock(return_value="e" * 64)
                candidate_checks = 0
                def ready(name, *args):
                    nonlocal candidate_checks
                    if name == "api-candidate":
                        candidate_checks += 1
                        if candidate_checks == failed_check:
                            raise deploy.Failure("PROCESS_NOT_STABLE")
                self.host.ready.side_effect = ready
                with self.assertRaisesRegex(deploy.Failure, "PROCESS_NOT_STABLE"):
                    self.execute()
                self.assertEqual(self.host.stored("incomplete")["phase"], phase)
                self.assertEqual(self.host.stored("current"), self.current)
                self.assertNotIn("api", [call.args[0] for call in self.host.stop_remove.call_args_list])
                reloads = [call for call in self.host.call.call_args_list if "reload" in call.args]
                self.assertEqual(len(reloads), 0 if failed_check == 2 else 1)

    def test_recover_original_preserves_previous_and_rejects_unrelated_release(self):
        marker = dict(self.marker(), **{"from": self.target, "previous": self.current, "target": A, "api_slot": "api"})
        self.host.record("current", self.target)
        self.host.record("previous", self.current)
        self.host.record("incomplete", marker)
        unrelated = dict(A, source_sha="1234567890abcdef1234567890abcdef12345678")
        with self.assertRaisesRegex(deploy.Failure, "RECOVERY_TARGET_MISMATCH"):
            self.execute("recover", confirm=True, selected=unrelated)
        self.host.converged.assert_not_called()
        self.assertEqual(self.host.stored("incomplete"), marker)
        self.host.converged.return_value = self.target
        self.execute("recover", confirm=True, selected=B, slot="api-candidate")
        self.assertEqual(self.host.stored("current"), self.target)
        self.assertEqual(self.host.stored("previous"), self.current)
        self.host.compose.assert_not_called()

    def test_interrupted_finish_restores_snapshots_for_original_or_target(self):
        original = self.target
        selected = dict(A, source_sha="1234567890abcdef1234567890abcdef12345678")
        target = running(selected, "api")
        record = self.host.record
        unlink = Path.unlink
        for previous in (None, self.current):
            for point in ("previous", "current", "unlink"):
                for recover_target in (False, True):
                    with self.subTest(previous=previous is not None, point=point, recover_target=recover_target):
                        record("current", original)
                        if previous is None:
                            (self.host.state / "previous").unlink(missing_ok=True)
                        else:
                            record("previous", previous)
                        marker = dict(self.marker(), **{"from": original, "previous": previous, "target": selected, "api_slot": "api"})
                        self.host.marker = copy.deepcopy(marker)
                        record("incomplete", marker)
                        def interrupted_record(name, value):
                            if name == point:
                                raise deploy.Failure("INTERRUPTED_OUTCOME_UNKNOWN")
                            record(name, value)
                        def interrupted_unlink(path, *args, **kwargs):
                            if path == self.host.state / "incomplete":
                                raise deploy.Failure("INTERRUPTED_OUTCOME_UNKNOWN")
                            return unlink(path, *args, **kwargs)
                        with contextlib.ExitStack() as stack:
                            stack.enter_context(patch.object(self.host, "record", side_effect=interrupted_record))
                            if point == "unlink":
                                stack.enter_context(patch.object(Path, "unlink", interrupted_unlink))
                            with self.assertRaisesRegex(deploy.Failure, "INTERRUPTED_OUTCOME_UNKNOWN"):
                                self.host.finish(target, original)
                        saved = self.host.stored("incomplete")
                        self.assertEqual(saved, dict(marker, phase="commit-state"))
                        observed = target if recover_target else original
                        self.host.converged.return_value = observed
                        self.execute("recover", confirm=True, selected=observed["release"], slot=observed["api_slot"])
                        self.assertEqual(self.host.stored("current"), observed)
                        self.assertEqual(self.host.stored("previous"), original if recover_target else previous)
                        self.assertEqual(self.host.stored("last-incomplete"), saved)
                        self.assertIsNone(self.host.stored("incomplete"))
        self.host.compose.assert_not_called()

    def test_stored_schema_corruption_is_rejected_without_secret_output(self):
        config = self.directory / "host.json"
        deploy.atomic(config, json.dumps(self.config))
        secret = "synthetic-secret-do-not-print"
        handlers = {sig: signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
        try:
            for name in ("current", "previous", "incomplete"):
                for value in ({"unexpected": secret}, None, [secret]):
                    with self.subTest(name=name, shape=type(value).__name__):
                        for stored in ("current", "previous", "incomplete"):
                            (self.host.state / stored).unlink(missing_ok=True)
                        deploy.atomic(self.host.state / name, json.dumps(value))
                        out, err = io.StringIO(), io.StringIO()
                        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err), patch.object(deploy.subprocess, "run") as process:
                            self.assertEqual(deploy.main(["--config", str(config), "status"]), 1)
                        self.assertEqual(out.getvalue(), "")
                        self.assertEqual(json.loads(err.getvalue())["code"], "INCOMPLETE_FIELDS" if name == "incomplete" else "STATE_FIELDS")
                        self.assertNotIn(secret, err.getvalue())
                        process.assert_not_called()
            self.host.record("current", {"secret": secret})
            self.host.record("incomplete", self.marker())
            with self.assertRaisesRegex(deploy.Failure, "STATE_FIELDS"):
                self.execute("recover", confirm=True)
            self.host.converged.assert_not_called()
        finally:
            for sig, handler in handlers.items():
                signal.signal(sig, handler)

    def test_image_cache_reuses_inspect_but_revalidates_source(self):
        def inspect(*args, **kwargs):
            component = next(k for k, image in A["images"].items() if image == args[2])
            return 0, json.dumps([dict(Id=args[2], Os="linux", Architecture="amd64", Config=dict(
                User="10001:10001", Entrypoint=["/usr/local/bin/" + deploy.BINS[component]],
                Labels={"org.opencontainers.image.revision": A["source_sha"], "org.opencontainers.image.source": "https://github.com/aura-historia/backend"},
                Env=["COMMIT_SHA=" + A["source_sha"]]))])
        self.host.call = Mock(side_effect=inspect)
        first = deploy.Host.images(self.host, A)
        self.assertEqual(deploy.Host.images(self.host, A), first)
        self.assertEqual(self.host.call.call_count, 4)
        with self.assertRaisesRegex(deploy.Failure, "IMAGE_SOURCE"):
            deploy.Host.images(self.host, dict(A, source_sha=B["source_sha"]))
        self.assertEqual(self.host.call.call_count, 4)

    def test_atomic_public_caddy_does_not_relax_state_permissions(self):
        deploy.atomic(self.directory / "public-caddy", "public only", mode=0o644)
        self.host.record("current", self.current)
        self.assertEqual((self.directory / "public-caddy").stat().st_mode & 0o777, 0o644)
        self.assertEqual((self.host.state / "current").stat().st_mode & 0o777, 0o600)

    def test_private_install_preserves_source_and_r3_fixture_seals(self):
        spec = importlib.util.spec_from_file_location("installed_smoke", Path(__file__).with_name("smoke-deploy.py"))
        smoke = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(smoke)
        from test_smoke_compose import SmokeComposeTests
        fixture = SmokeComposeTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        before = {name: ((smoke.r3.ROOT / name).read_bytes(), (smoke.r3.ROOT / name).stat().st_mode,
                         (smoke.r3.ROOT / name).stat().st_uid) for name in smoke.TOOLING_FILES}
        sealed = smoke.r3.fixture_hashes(fixture.directory)
        root, hashes = smoke.install_tooling(fixture.directory)
        self.assertEqual(smoke.r3.fixture_hashes(fixture.directory), sealed)
        installed = load_deploy(root / "deploy/bin/deploy")
        self.assertEqual(installed.ROOT, root)
        self.assertEqual(installed.COMPOSE, root / "deploy/compose")
        self.assertEqual(installed.CATALOG, deploy.CATALOG)
        for name, (content, mode, uid) in before.items():
            source = smoke.r3.ROOT / name
            self.assertEqual((source.read_bytes(), source.stat().st_mode, source.stat().st_uid), (content, mode, uid))
            self.assertEqual((root / name).read_bytes(), content)
            self.assertEqual(hashes[name], hashlib.sha256(content).hexdigest())
            self.assertEqual(installed.trusted(root / name), root / name)
        host = installed.Host(dict(self.config, config_dir=str(fixture.directory), compose_env=str(fixture.directory / "compose.env")))
        snapshot = host.inputs()
        for name in smoke.TOOLING_FILES[1:]:
            self.assertEqual(snapshot[str(root / name)][2], hashes[name])
        self.assertTrue(all(not key.startswith(str(smoke.r3.ROOT) + "/") for key in snapshot))
        script = root / "deploy/bin/deploy"
        script.chmod(0o775)
        with self.assertRaisesRegex(smoke.r2.Failure, "INSTALLED_FILE_MODE"):
            smoke.check_tooling(root, hashes)
        script.chmod(0o755)
        script.write_bytes(script.read_bytes() + b"\n")
        with self.assertRaisesRegex(smoke.r2.Failure, "INSTALLED_TOOLING_BYTES"):
            smoke.check_tooling(root, hashes)


if __name__ == "__main__":
    unittest.main()
