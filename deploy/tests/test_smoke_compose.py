"""Pure R3 guard regressions; no Docker or live provider calls."""
import contextlib
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import socket
import tempfile
import unittest
import uuid
from unittest.mock import Mock

spec = importlib.util.spec_from_file_location("smoke_compose", Path(__file__).with_name("smoke-compose.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class SmokeComposeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.projects = {kind: "aura-r3-" + uuid.uuid4().hex + "-" + kind for kind in smoke.SERVICES}
        self.images = dict(smoke.IMAGES, caddy=smoke.CADDY)
        self.environments = smoke.app_environments()
        smoke.materialize(self.directory, self.projects, 12345, self.images, self.environments)

    def validate(self, kind, model):
        smoke.validate_model(kind, model, self.images, self.projects, self.directory, 12345, self.environments)

    def mounts(self, service, actual=False):
        mounts = [dict(type="bind", source=str(source), target=target, read_only=True,
                       bind={"create_host_path": False}) for source, target in smoke.bind_pairs(service, self.directory).items()]
        mounts += [dict(type="volume", source=key, target=target) for key, target in smoke.VOLUME_MOUNTS.get(service, {}).items()]
        if not actual:
            return mounts
        kind = next(kind for kind, services in smoke.SERVICES.items() if service in services)
        return [dict(Type=m["type"], Source=m["source"], Destination=m["target"], RW=not m.get("read_only", False),
                     **({"Propagation": "rprivate"} if m["type"] == "bind" else
                        {"Name": self.projects[kind] + "_" + m["source"], "Driver": "local"})) for m in mounts]

    def model(self, kind):
        services = {}
        for name in smoke.SERVICES[kind]:
            key = "worker" if name in smoke.provider.SCOPES else "helper" if name in {"provider", "bootstrap"} else name
            service = dict(image=self.images[key], pull_policy="never", volumes=self.mounts(name))
            if name == "bootstrap":
                service["network_mode"] = "service:postgres"
            else:
                service["networks"] = {"backend": {}}
            if name in smoke.TMPFS:
                service["tmpfs"] = [smoke.TMPFS[name]]
            if kind == "application":
                service.update(environment=dict(self.environments[name]), user="10001:10001", read_only=True)
            if name == "caddy":
                service["networks"]["edge"] = {}
                service["ports"] = [dict(host_ip="127.0.0.1", published="12345", target=443)]
            services[name] = service
        backend = dict(name=self.projects["platform"] + "_backend", **({"internal": True} if kind == "platform" else {"external": True}))
        networks = dict(backend=backend)
        if kind == "edge":
            networks["edge"] = dict(name=self.projects["edge"] + "_edge", ipam={})
        return dict(services=services, volumes={v: {"name": self.projects[kind] + "_" + v} for v in smoke.VOLUMES[kind]}, networks=networks)

    def test_safe_get_uses_canonical_product_listing_typeid_with_uuid7(self):
        identifier = smoke.SAFE_GET.rsplit("/", 1)[1]
        self.assertRegex(identifier, r"^pl_[0-7][0-9a-hjkmnp-tv-z]{25}$")
        alphabet = "0123456789abcdefghjkmnpqrstvwxyz"
        value = 0
        for character in identifier[3:]:
            value = value * 32 + alphabet.index(character)
        decoded = uuid.UUID(int=value)
        self.assertEqual(decoded.version, 7)
        self.assertEqual(decoded.variant, uuid.RFC_4122)
        self.assertEqual(str(decoded), "00000000-0000-7000-8000-000000000001")

    def test_worker_env_is_union_without_scope_or_delivery_leak(self):
        common, delivery = smoke.worker_environment()
        self.assertFalse((smoke.DELIVERY | smoke.QUEUE_FIELDS) & common.keys())
        self.assertEqual(set(delivery), smoke.DELIVERY)
        for scope in smoke.provider.SCOPES:
            expected = smoke.provider.fixture_environment("worker", scope)
            for key, value in expected.items():
                if key not in smoke.QUEUE_FIELDS:
                    self.assertEqual((delivery if key in smoke.DELIVERY else common)[key], value)
        self.assertIn("VERTEX_AI_MODEL", common)
        self.assertIn("OPENSEARCH_ENDPOINT_URL", common)
        self.assertEqual(common["STAGE"], "test")

    def test_search_identities_are_scope_specific_and_non_search_workers_clean(self):
        expected = {
            "api": "aura_reader",
            "api-candidate": "aura_reader",
            "product-listing-opensearch": "aura_product_projector",
            "search-filter-projection": "aura_filter_projector",
            "search-filter-percolator": "aura_percolator",
            "cron": "aura_cron",
        }
        self.assertEqual(smoke.SEARCH_IDENTITIES, expected)
        for name, username in expected.items():
            values = self.environments["api" if name == "api-candidate" else name]
            self.assertTrue(values.get("OPENSEARCH_USERNAME") == username)
            self.assertTrue(values.get("OPENSEARCH_PASSWORD"))
        non_search = (set(smoke.provider.SCOPES) - set(smoke.SEARCH_WORKER_ENVFILES)) | {"crawler"}
        for name in non_search:
            values = self.environments[name]
            self.assertNotIn("OPENSEARCH_USERNAME", values)
            self.assertNotIn("OPENSEARCH_PASSWORD", values)
        common, _ = smoke.worker_environment()
        self.assertNotIn("OPENSEARCH_USERNAME", common)
        self.assertNotIn("OPENSEARCH_PASSWORD", common)
        for name in smoke.SEARCH_WORKER_ENVFILES.values():
            path = self.directory / name
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            keys = {line.split("=", 1)[0] for line in path.read_text().splitlines()}
            self.assertEqual(keys, {"OPENSEARCH_USERNAME", "OPENSEARCH_PASSWORD"})

    def test_native_sequin_catalog_not_compose(self):
        catalog = json.loads((smoke.ROOT / "deploy/catalog.json").read_text())
        config = smoke.native_sequin(catalog)
        self.assertNotIn("services", config)
        self.assertEqual(len(config["sinks"]), 10)
        self.assertEqual(len(config["http_endpoints"]), 10)
        self.assertEqual(len({s["source"]["include_tables"][0] for s in config["sinks"]}), 5)
        database = config["databases"][0]
        self.assertEqual(database["username"], "smoke_replication")
        self.assertFalse(database["slot"]["create_if_not_exists"])
        self.assertFalse(database["publication"]["create_if_not_exists"])
        for sink, worker in zip(config["sinks"], catalog["workers"]):
            self.assertEqual(sink["name"], worker["scope"])
            self.assertEqual(sink["actions"], [op.lower() for op in worker["operations"]])
            self.assertEqual(sink["destination"]["http_endpoint"], worker["scope"])
            self.assertIs(sink["initial_backfill"], False)
            self.assertEqual(sink["load_shedding_policy"], "pause_on_full")
        self.assertEqual({e["name"]: e["url"] for e in config["http_endpoints"]},
                         {scope: f"http://{scope}:8081/cdc/sequin" for scope in smoke.provider.SCOPES})

    def test_setup_records_projects_before_any_docker_call(self):
        with contextlib.redirect_stdout(io.StringIO()):
            harness = smoke.Harness(self.directory)
        journal = json.loads((self.directory / "journal.json").read_text())
        self.assertEqual(journal["projects"], harness.projects)
        self.assertFalse(harness.mutated)
        self.assertEqual(len(set(harness.projects.values())), 3)
        self.assertEqual((self.directory / "journal.json").stat().st_mode & 0o777, 0o600)
        for kind, project in harness.projects.items():
            self.assertRegex(project, "^aura-r3-[0-9a-f]{32}-" + kind + "$")

    def test_unknown_or_interrupted_docker_call_never_cleans_up(self):
        for code in ("DOCKER_COMMAND_UNCONFIRMED", "DOCKER_OUTPUT_LIMIT", "INTERRUPTED_15"):
            with self.subTest(code=code), tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
                harness = smoke.Harness(Path(directory))
                harness.docker.call = Mock(side_effect=smoke.r2.Failure(code))
                with self.assertRaisesRegex(smoke.r2.Failure, code):
                    harness.call("compose", "up")
                self.assertTrue(harness.uncertain)
                with self.assertRaisesRegex(smoke.r2.Failure, "UNKNOWN_OUTCOME_RETAINED"):
                    harness.cleanup()
                self.assertEqual(harness.docker.call.call_count, 1)
                self.assertTrue(json.loads((Path(directory) / "journal.json").read_text())["uncertain"])

    def test_cleanup_never_uses_unverified_or_changed_declarations(self):
        with contextlib.redirect_stdout(io.StringIO()):
            harness = smoke.Harness(self.directory)
        harness.mutated = True
        harness.fixture_inputs = smoke.fixture_hashes(self.directory)
        harness.resources = Mock(return_value=({}, [], []))
        harness.call = Mock(side_effect=AssertionError("Docker must not be called"))
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(smoke.r2.Failure, "UNVERIFIED_COMPOSE_MUTATION"):
            harness.cleanup()
        harness.models = {kind: self.model(kind) for kind in smoke.SERVICES}
        changed = copy.deepcopy(harness.models["platform"])
        changed["volumes"]["postgres-data"]["name"] = "unowned-data"
        harness.model = Mock(return_value=changed)
        with self.assertRaisesRegex(smoke.r2.Failure, "COMPOSE_MODEL_CHANGED"):
            harness.compose("platform", "down", "--volumes")
        harness.compose_hashes[0] = "changed"
        with self.assertRaisesRegex(smoke.r2.Failure, "COMPOSE_CHANGED_DURING_RUN"):
            harness.compose("platform", "down", "--volumes")
        harness.compose_hashes = [smoke.hashlib.sha256(p.read_bytes()).hexdigest() for p in harness.compose_inputs]
        env = self.directory / "compose.env"
        env.chmod(0o600)
        env.write_text(env.read_text().replace("AURA_NETWORK_INTERNAL=true", "AURA_NETWORK_INTERNAL=false"))
        env.chmod(0o444)
        with self.assertRaisesRegex(smoke.r2.Failure, "FIXTURE_CHANGED_DURING_RUN"):
            harness.compose("platform", "down", "--volumes")
        harness.call.assert_not_called()

    def test_container_ownership_requires_exact_project_service_name_and_full_id(self):
        project = self.projects["application"]
        item = dict(Id="b" * 64, Name="/" + project + "-api-1", Config=dict(Labels={
            "com.docker.compose.project": project, "com.docker.compose.service": "api"}))
        self.assertEqual(smoke.owned_container(item, project, {"api"}), "api")
        for key, value in (("Name", "/another-api-1"), ("Id", "b" * 12), ("Config", {"Labels": {}})):
            with self.assertRaisesRegex(smoke.r2.Failure, "CONTAINER_OWNERSHIP"):
                smoke.owned_container(dict(item, **{key: value}), project, {"api"})
        with self.assertRaisesRegex(smoke.r2.Failure, "CONTAINER_OWNERSHIP"):
            smoke.owned_container(item, project, {"cron"})

    def test_known_models_pass_without_deriving_authority_from_model(self):
        for kind in smoke.SERVICES:
            self.validate(kind, self.model(kind))

    def test_volume_declarations_reject_names_external_and_host_bind_drivers(self):
        for kind in ("platform", "edge"):
            for key in smoke.VOLUMES[kind]:
                for change in ({"name": "unowned-data"}, {"name": self.projects[kind] + "_other"},
                               {"external": True}, {"external": False}, {"driver": "nfs"},
                               {"driver_opts": {}}, {"driver_opts": {"type": "none", "o": "bind", "device": "/"}}):
                    with self.subTest(kind=kind, key=key, change=change):
                        model = self.model(kind)
                        model["volumes"][key].update(change)
                        with self.assertRaisesRegex(smoke.r2.Failure, "VOLUME_DECLARATION"):
                            self.validate(kind, model)

    def test_actual_volumes_require_local_empty_options_scope_and_labels(self):
        project = self.projects["platform"]
        volume = dict(Name=project + "_postgres-data", Driver="local", Scope="local", Options=None,
                      Labels={"com.docker.compose.project": project, "com.docker.compose.volume": "postgres-data"})
        for options in (None, {}):
            smoke.owned_volume(dict(volume, Options=options), "platform", project)
        for change in ({"Name": "unowned"}, {"Driver": "nfs"}, {"Scope": "global"}, {"Labels": {}},
                       {"Options": {"type": "none", "o": "bind", "device": "/"}}, {"Options": []}):
            with self.assertRaisesRegex(smoke.r2.Failure, "VOLUME_OWNERSHIP"):
                smoke.owned_volume(dict(volume, **change), "platform", project)

    def test_fixed_mount_inventory_and_actual_readonly_pairs(self):
        self.assertEqual(smoke.BIND_MOUNTS["postgres"], {"postgres": "/etc/postgresql"})
        self.assertEqual(smoke.BIND_MOUNTS["sequin"], {"sequin.yaml": "/run/sequin/sequin.yaml"})
        self.assertEqual(set(smoke.PUBLIC_BINDS), {"opensearch", "provider"})
        self.assertEqual({name for name, mounts in smoke.BIND_MOUNTS.items() if "opensearch-ca.pem" in mounts},
                         {"api", "cron", "product-listing-opensearch", "search-filter-projection", "search-filter-percolator"})
        for mounts in smoke.BIND_MOUNTS.values():
            if "opensearch-ca.pem" in mounts:
                self.assertEqual(mounts["opensearch-ca.pem"], "/run/aura/opensearch-ca.pem")
        self.assertEqual({n for n in smoke.SERVICES["application"] if "google-adc.json" in smoke.BIND_MOUNTS[n]},
                         {"api", "cron", "crawler", "search-filter-percolator", "product-embedding", "product-translation"})
        for kind, services in smoke.SERVICES.items():
            for name in services:
                for actual in (False, True):
                    smoke.validate_mounts(name, self.mounts(name, actual), self.projects[kind], self.directory, actual)

    def test_binds_reject_outside_alias_wrong_target_extra_and_writable_mounts(self):
        for kind, services in smoke.SERVICES.items():
            for service in services:
                for actual in (False, True):
                    mounts = self.mounts(service, actual)
                    for index, mount in enumerate(mounts):
                        if mount.get("Type" if actual else "type") != "bind":
                            continue
                        source_key, target_key = ("Source", "Destination") if actual else ("source", "target")
                        for change in ({source_key: "/outside/innocent-name"}, {source_key: str(self.directory / ".." / self.directory.name / "google-adc.json")},
                                       {target_key: "/other"}, {"RW": True} if actual else {"read_only": False}):
                            changed = copy.deepcopy(mounts)
                            changed[index].update(change)
                            with self.assertRaises(smoke.r2.Failure):
                                smoke.validate_mounts(service, changed, self.projects[kind], self.directory, actual)
                    with self.assertRaises(smoke.r2.Failure):
                        smoke.validate_mounts(service, mounts + [dict(Type="bind", Source="/outside/socket-alias", Destination="/extra", RW=False, Propagation="rprivate") if actual else
                            dict(type="bind", source="/outside/socket-alias", target="/extra", read_only=True, bind={"create_host_path": False})], self.projects[kind], self.directory, actual)

    def test_symlink_hardlink_and_socket_at_allowed_source_are_rejected(self):
        source = self.directory / "google-adc.json"
        other = tempfile.TemporaryDirectory()
        self.addCleanup(other.cleanup)
        outside = Path(other.name) / "synthetic-outside-file"
        outside.write_text("not a credential")
        source.unlink()
        source.symlink_to(outside)
        with self.assertRaises(smoke.r2.Failure):
            smoke.validate_mounts("api", self.mounts("api"), self.projects["application"], self.directory)
        source.unlink()
        os.link(outside, source)
        with self.assertRaises(smoke.r2.Failure):
            smoke.validate_mounts("api", self.mounts("api"), self.projects["application"], self.directory)
        source.unlink()
        with socket.socket(socket.AF_UNIX) as sock:
            sock.bind(str(source))
            with self.assertRaises(smoke.r2.Failure):
                smoke.validate_mounts("api", self.mounts("api"), self.projects["application"], self.directory)
        source.unlink()

    def test_directory_bind_rejects_nested_and_parent_symlink_escapes(self):
        config = self.directory / "postgres/postgresql.conf"
        config.unlink()
        config.symlink_to(self.directory / "worker.env")
        with self.assertRaises(smoke.r2.Failure):
            smoke.validate_mounts("postgres", self.mounts("postgres"), self.projects["platform"], self.directory)
        config.unlink()
        (self.directory / "postgres").rmdir()
        (self.directory / "postgres").symlink_to(self.directory / "caddy", target_is_directory=True)
        with self.assertRaises(smoke.r2.Failure):
            smoke.validate_mounts("postgres", self.mounts("postgres"), self.projects["platform"], self.directory)

    def test_mount_options_and_unknown_volume_targets_fail(self):
        for change in ({"source": "other-data"}, {"target": "/other"}, {"volume": {"subpath": "host"}}, {"read_only": True}):
            mounts = self.mounts("postgres")
            mounts[-1].update(change)
            with self.assertRaises(smoke.r2.Failure):
                smoke.validate_mounts("postgres", mounts, self.projects["platform"], self.directory)
        mounts = self.mounts("postgres")
        mounts[0]["bind"]["propagation"] = "rshared"
        with self.assertRaisesRegex(smoke.r2.Failure, "MOUNT_OPTIONS"):
            smoke.validate_mounts("postgres", mounts, self.projects["platform"], self.directory)

    def test_env_file_paths_checked_before_resolution(self):
        for kind in smoke.SERVICES:
            model = self.model(kind)
            for name, service in model["services"].items():
                service["env_file"] = [dict(path=str(self.directory / filename), format="raw", required=True) for filename in smoke.ENV_FILES.get(name, [])]
            smoke.validate_env_files(kind, model, self.directory)
            for name in model["services"]:
                altered = copy.deepcopy(model)
                files = altered["services"][name]["env_file"]
                if files:
                    files[0]["path"] = "/outside/credentials"
                else:
                    files.append(dict(path="/outside/credentials", format="raw", required=True))
                with self.assertRaises(smoke.r2.Failure):
                    smoke.validate_env_files(kind, altered, self.directory)

    def test_all_app_env_command_and_mount_drift_rejected(self):
        for name in smoke.SERVICES["application"]:
            for field, value in (("POSTGRES_HOST", "outside"), ("POSTGRES_PASSWORD", "not-fixture"),
                                 ("AWS_ENDPOINT_URL_SQS", "https://outside.example.test"), ("HTTPS_PROXY", "http://outside.example.test")):
                model = self.model("application")
                model["services"][name]["environment"][field] = value
                with self.assertRaisesRegex(smoke.r2.Failure, "APP_ENV_OR_COMMAND_DRIFT"):
                    self.validate("application", model)
            for field in ("command", "entrypoint", "volumes_from", "secrets", "configs"):
                model = self.model("application")
                model["services"][name][field] = ["unexpected"]
                with self.assertRaises(smoke.r2.Failure):
                    self.validate("application", model)
            model = self.model("application")
            model["services"][name]["volumes"] = []
            with self.assertRaisesRegex(smoke.r2.Failure, "EXACT_SERVICE_MOUNTS"):
                self.validate("application", model)

    def test_refresh_checks_actual_mounts_and_environment(self):
        harness = smoke.Harness.__new__(smoke.Harness)
        harness.projects, harness.directory, harness.images = self.projects, self.directory, self.images
        harness.environments, harness.ids = self.environments, {}
        harness.image_envs = {key: dict(smoke.r2.TRUSTED_ENV) for key in smoke.IMAGES}
        found = {}
        for name in smoke.SERVICES["application"]:
            key = "worker" if name in smoke.provider.SCOPES else name
            found[name] = dict(Id=uuid.uuid4().hex * 2, Image=self.images[key], RestartCount=0,
                State=dict(Running=True), Mounts=self.mounts(name, True),
                NetworkSettings=dict(Networks={self.projects["platform"] + "_backend": {}}),
                HostConfig=dict(NetworkMode=self.projects["platform"] + "_backend", PortBindings={},
                                Tmpfs={"/tmp": smoke.TMPFS[name].split(":", 1)[1]}),
                Config=dict(Env=[k + "=" + v for k, v in dict(harness.image_envs[key], **self.environments[name]).items()]))
        harness.resources = Mock(return_value=(found, [], []))
        harness.refresh("application", smoke.SERVICES["application"])
        original = copy.deepcopy(found["api"])
        found["api"]["Mounts"][0]["Source"] = "/outside/alias"
        with self.assertRaisesRegex(smoke.r2.Failure, "EXACT_SERVICE_MOUNTS"):
            harness.refresh("application", smoke.SERVICES["application"])
        found["api"] = original
        found["api"]["Config"]["Env"].append("HTTPS_PROXY=http://outside.example.test")
        with self.assertRaisesRegex(smoke.r2.Failure, "ACTUAL_APP_ENV_DRIFT"):
            harness.refresh("application", smoke.SERVICES["application"])

    def test_model_requires_immutable_images_internal_network_and_no_ports(self):
        for change in (lambda m: m["services"]["postgres"].update(image="postgres:latest"),
                       lambda m: m["networks"]["backend"].update(internal=False),
                       lambda m: m["services"]["provider"].update(ports=[dict(target=18090, published="18090")]),
                       lambda m: m["services"]["bootstrap"].update(network_mode="host"),
                       lambda m: m["services"]["provider"].update(build=".")):
            model = self.model("platform")
            change(model)
            with self.assertRaises(smoke.r2.Failure):
                self.validate("platform", model)

    def test_only_caddy_may_join_private_project_edge_bridge(self):
        for change in ({"external": True}, {"name": "shared_bridge"}, {"driver": "host"}):
            model = self.model("edge")
            model["networks"]["edge"].update(change)
            with self.assertRaises(smoke.r2.Failure):
                self.validate("edge", model)
        for field in ("environment", "env_file", "secrets", "configs", "command", "entrypoint"):
            model = self.model("edge")
            model["services"]["caddy"][field] = {"fixture": "unexpected"} if field == "environment" else ["unexpected"]
            with self.assertRaises(smoke.r2.Failure):
                self.validate("edge", model)
        for kind in smoke.SERVICES:
            for name in smoke.SERVICES[kind] - {"bootstrap"}:
                model = self.model(kind)
                model["services"][name]["networks"]["other"] = {}
                with self.assertRaisesRegex(smoke.r2.Failure, "NETWORK_GATE"):
                    self.validate(kind, model)
        model = self.model("platform")
        model["networks"]["edge"] = dict(name=self.projects["edge"] + "_edge")
        with self.assertRaisesRegex(smoke.r2.Failure, "EXACT_NETWORK_SET"):
            self.validate("platform", model)

    def test_bridge_ipam_only_empty_normalization_no_driver_options(self):
        for kind, key in (("platform", "backend"), ("edge", "edge")):
            model = self.model(kind)
            model["networks"][key]["ipam"] = {}
            self.validate(kind, model)
            for change in ({"ipam": None}, {"ipam": []}, {"ipam": False}, {"ipam": {"driver": "default"}},
                           {"ipam": {"config": []}}, {"ipam": {"config": [{"subnet": "192.0.2.0/24"}]}},
                           {"ipam": {"options": {}}}, {"driver_opts": {"com.docker.network.bridge.enable_ip_masquerade": "true"}},
                           {"driver": "host"}):
                altered = copy.deepcopy(model)
                altered["networks"][key].update(change)
                with self.assertRaises(smoke.r2.Failure):
                    self.validate(kind, altered)

    def test_network_ownership_rejects_other_members_options_and_networks(self):
        caddy_id = "a" * 64
        for kind, key in (("edge", "edge"), ("platform", "backend")):
            project = self.projects[kind]
            net = dict(Name=project + "_" + key, Internal=kind == "platform", Driver="bridge", Scope="local",
                       Options={}, IPAM=dict(Driver="default", Options=None),
                       Labels={"com.docker.compose.project": project, "com.docker.compose.network": key}, Containers={caddy_id: {}})
            smoke.owned_network(net, kind, project, {caddy_id})
            for change in ({"Internal": kind != "platform"}, {"Name": "unowned"}, {"Driver": "host"}, {"Labels": {}},
                           {"Options": {"unsafe": "true"}}, {"IPAM": {"Driver": "custom"}},
                           {"IPAM": {"Driver": "default", "Options": {"unsafe": "true"}}},
                           {"Containers": {caddy_id: {}, "b" * 64: {}}}):
                with self.assertRaises(smoke.r2.Failure):
                    smoke.owned_network(dict(net, **change), kind, project, {caddy_id})
            with self.assertRaises(smoke.r2.Failure):
                smoke.owned_network(net, "application", project, {caddy_id})

    def test_caddyfile_is_exact_local_ca_fixed_internal_upstream(self):
        smoke.validate_caddyfile(self.directory)
        for text in (smoke.CADDYFILE.replace("tls internal", "tls operator@example.test"),
                     smoke.CADDYFILE.replace("api:8080", "https://external.example.test")):
            (self.directory / "caddy/Caddyfile").chmod(0o600)
            smoke.protected_write(self.directory, "caddy/Caddyfile", text)
            with self.assertRaisesRegex(smoke.r2.Failure, "CADDY_FIXED_LOCAL_CONFIG"):
                smoke.validate_caddyfile(self.directory)
        (self.directory / "caddy/Caddyfile").chmod(0o600)
        smoke.protected_write(self.directory, "caddy/Caddyfile", smoke.CADDYFILE)
        smoke.protected_write(self.directory, "caddy/credentials.env", "synthetic\n")
        with self.assertRaisesRegex(smoke.r2.Failure, "CADDY_FIXED_LOCAL_CONFIG"):
            smoke.validate_caddyfile(self.directory)

    def test_unquoted_or_expanded_tmpfs_rejected(self):
        for tmpfs in (["/tmp:rw", "nosuid", "nodev", "mode=1777"], ["/tmp:rw,size=4g"]):
            model = self.model("platform")
            model["services"]["bootstrap"]["tmpfs"] = tmpfs
            with self.assertRaisesRegex(smoke.r2.Failure, "TMPFS_QUOTING_bootstrap"):
                self.validate("platform", model)

    def test_materialization_is_protected_synthetic_test_only(self):
        self.assertEqual(self.directory.stat().st_mode & 0o777, 0o700)
        hashes = smoke.fixture_hashes(self.directory)
        self.assertEqual(hashes, smoke.fixture_hashes(self.directory))
        ca = self.directory / "opensearch-ca.pem"
        self.assertIn(ca.name, hashes)
        self.assertEqual(ca.read_text(), "unused synthetic STAGE=test CA mount\n")
        self.assertEqual(ca.stat().st_mode & 0o777, 0o444)
        ca.chmod(0o600)
        smoke.protected_write(self.directory, ca.name, "changed synthetic CA mount\n")
        self.assertNotEqual(hashes[ca.name], smoke.fixture_hashes(self.directory)[ca.name])
        ca.chmod(0o600)
        smoke.protected_write(self.directory, ca.name, "unused synthetic STAGE=test CA mount\n")
        for kind in ("api", "worker", "cron", "crawler"):
            env = dict(line.split("=", 1) for line in (self.directory / (kind + ".env")).read_text().splitlines())
            self.assertEqual(env["STAGE"], "test")
            self.assertEqual(env["POSTGRES_SSL_MODE"], "disable")
            self.assertNotIn("POSTGRES_SSL_ROOT_CERT", env)
            self.assertTrue("OPENSEARCH_SSL_ROOT_CERT" not in env, "HTTP test fixture enabled CA input")
            if "OPENSEARCH_ENDPOINT_URL" in env:
                self.assertTrue(env["OPENSEARCH_ENDPOINT_URL"].startswith("http://"))
        self.assertIn("AURA_NETWORK_INTERNAL=true\n", (self.directory / "compose.env").read_text())
        self.assertFalse(list(self.directory.glob("compose.*.yml")))
        self.directory.chmod(0o755)
        with self.assertRaisesRegex(smoke.r2.Failure, "FIXTURE_ROOT_PERMISSIONS"):
            smoke.fixture_hashes(self.directory)
        self.directory.chmod(0o700)


if __name__ == "__main__":
    unittest.main()
