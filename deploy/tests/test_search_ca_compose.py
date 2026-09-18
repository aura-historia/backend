"""Opt-in real Compose config and isolated cached-helper mount checks; no app startup."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
import uuid

import test_deploy as host_tests
import test_smoke_compose as compose_tests

smoke = compose_tests.smoke


class SearchCaComposeTests(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="aura-search-ca-compose-"))
        self.docker = None
        self.addCleanup(self.cleanup)
        projects = {kind: "aura-search-ca-" + uuid.uuid4().hex + "-" + kind for kind in smoke.SERVICES}
        images = dict(smoke.IMAGES, caddy=smoke.CADDY)
        smoke.materialize(self.directory, projects, 12345, images)
        (self.directory / "state").mkdir(mode=0o700)
        self.host = host_tests.deploy.Host(dict(stage="test", application_project=projects["application"],
            edge_project=projects["edge"], config_dir=str(self.directory),
            compose_env=str(self.directory / "compose.env"), state_dir=str(self.directory / "state"),
            https_port=12345, https_ca=str(self.directory / "postgres-ca.pem")))
        self.selected = dict(source_sha=host_tests.A["source_sha"],
                             images={kind: images[kind] for kind in host_tests.deploy.BINS})

    def cleanup(self):
        if self.docker is not None:
            try:
                for identifier in list(self.docker.owned):
                    self.docker.remove(identifier)
            finally:
                if self.docker.recovery:
                    # Exact ownership evidence only; do not discard an uncertain acquisition.
                    self.fail("retained fixture " + str(self.directory) + ": " + "; ".join(self.docker.recovery))
        shutil.rmtree(self.directory)
        self.assertFalse(self.directory.exists(), "owned CA fixture directory remained")

    @unittest.skipUnless(os.environ.get("AURA_TEST_COMPOSE_CONFIG") == "1", "opt-in local Compose config only")
    def test_actual_compose_config_inherits_exact_ca_mount_for_candidate(self):
        before = smoke.fixture_hashes(self.directory)
        sources = [host_tests.deploy.COMPOSE / name for name in ("compose.application.yml", "compose.replace.yml")]
        source_bytes = [path.read_bytes() for path in sources]
        model = self.host.model(self.selected)  # Actual two Compose config calls; output/env remain captured.
        receivers = {"api", "api-candidate", "cron", "product-listing-opensearch",
                     "search-filter-projection", "search-filter-percolator"}
        observed = set()
        for name, service in model["services"].items():
            mounts = [m for m in service["volumes"] if m["target"] == "/run/aura/opensearch-ca.pem"]
            if name in receivers:
                self.assertEqual(len(mounts), 1)
                mount = mounts[0]
                self.assertEqual(mount["source"], str(self.directory / "opensearch-ca.pem"))
                self.assertEqual(mount["type"], "bind")
                self.assertIs(mount["read_only"], True)
                self.assertEqual(mount["bind"], {"create_host_path": False})
                observed.add(name)
            else:
                self.assertFalse(mounts, "non-search service received CA")
            self.assertTrue("OPENSEARCH_SSL_ROOT_CERT" not in service["environment"], "HTTP test fixture enabled CA input")
        self.assertEqual(observed, receivers)
        self.assertEqual(smoke.fixture_hashes(self.directory), before)
        self.assertTrue([path.read_bytes() for path in sources] == source_bytes, "Compose sources changed")

    @unittest.skipUnless(os.environ.get("AURA_TEST_COMPOSE_CONFIG") == "1", "opt-in local Compose config only")
    def test_actual_compose_config_resolves_scoped_raw_credentials(self):
        cases = (
            ("single-dollar", "$cash"),
            ("double-dollar", "$$"),
            ("braced-literal", "${SHOULD_STAY_LITERAL}"),
            ("mixed-punctuation", 'C2-literal-$cash-#hash-"double"-\'single\'-=tail'),
        )
        product_filename = smoke.SEARCH_WORKER_ENVFILES["product-listing-opensearch"]
        product_path = self.directory / product_filename
        for case, special in cases:
            credential_text = "\n".join((
                "OPENSEARCH_USERNAME=aura_product_projector",
                "OPENSEARCH_PASSWORD=" + special,
                "",
            ))
            smoke.protected_write(self.directory, product_filename, credential_text, mode=0o600)
            expected_bytes = credential_text.encode()
            resolved = self.host.model(self.selected)
            raw = host_tests.deploy.compose_config_json(self.host.compose(
                "application", self.selected, "config", "--no-env-resolution", "--format", "json"))
            scoped = {
                "product-listing-opensearch": (product_filename, "aura_product_projector", special),
                "search-filter-projection": ("search-filter-projection.env", "aura_filter_projector",
                                              smoke.SEARCH_PASSWORDS["search-filter-projection"]),
                "search-filter-percolator": ("search-filter-percolator.env", "aura_percolator",
                                              smoke.SEARCH_PASSWORDS["search-filter-percolator"]),
            }
            for name, (filename, username, password) in scoped.items():
                with self.subTest(case=case, service=name):
                    entries = raw["services"][name]["env_file"]
                    expected_entries = [(str(self.directory / "worker.env"), "raw"),
                                        (str(self.directory / filename), "raw")]
                    self.assertTrue([(entry["path"], entry["format"]) for entry in entries] == expected_entries,
                                    "raw env-file contract changed")
                    environment = resolved["services"][name]["environment"]
                    self.assertTrue(environment.get("OPENSEARCH_USERNAME") == username,
                                    "search username changed")
                    self.assertTrue(environment.get("OPENSEARCH_PASSWORD") == password,
                                    "literal search password changed")
                    self.assertTrue(all(environment.get("OPENSEARCH_PASSWORD") != other
                                        for other in smoke.SEARCH_PASSWORDS.values() if other != password),
                                    "search credential crossed scope")
            for name, username, password in (("api", "aura_reader", smoke.SEARCH_PASSWORDS["api"]),
                                              ("api-candidate", "aura_reader", smoke.SEARCH_PASSWORDS["api"]),
                                              ("cron", "aura_cron", smoke.SEARCH_PASSWORDS["cron"])):
                environment = resolved["services"][name]["environment"]
                with self.subTest(case=case, service=name):
                    self.assertTrue(environment.get("OPENSEARCH_USERNAME") == username,
                                    "non-projector username changed")
                    self.assertTrue(environment.get("OPENSEARCH_PASSWORD") == password,
                                    "non-projector password changed")
            non_search = (set(smoke.provider.SCOPES) - set(smoke.SEARCH_WORKER_ENVFILES)) | {"crawler"}
            for name in non_search:
                environment = resolved["services"][name]["environment"]
                with self.subTest(case=case, service=name):
                    self.assertTrue("OPENSEARCH_USERNAME" not in environment and
                                    "OPENSEARCH_PASSWORD" not in environment,
                                    "non-search worker received credentials")
            self.assertTrue(product_path.read_bytes() == expected_bytes, "raw credential bytes changed")
            self.assertEqual(product_path.stat().st_mode & 0o777, 0o600)

        if os.environ.get("AURA_TEST_LITERAL_ENV") == "1":
            self.literal_environment_witness()

    def literal_environment_witness(self):
        helper = os.environ.get("AURA_TEST_HELPER_IMAGE")
        self.assertTrue(isinstance(helper, str) and helper.startswith("sha256:") and len(helper) == 71
                        and all(character in "0123456789abcdef" for character in helper[7:]),
                        "prepared helper image ID is required")
        self.docker = smoke.r2.Docker(self.directory)
        literals = {
            "LITERAL_SINGLE": "$cash",
            "LITERAL_DOUBLE": "$$",
            "LITERAL_BRACED": "${SHOULD_STAY_LITERAL}",
            "LITERAL_MIXED": 'x$y-$$-${ALSO_LITERAL}-#-"-\'-,=\\',
        }
        raw_path = self.directory / "literal.env"
        smoke.protected_write(self.directory, raw_path.name, smoke.env_text(literals), mode=0o600)
        expected_path = self.directory / "literal-expected.json"
        smoke.protected_write(expected_path.parent, expected_path.name, json.dumps(literals), mode=0o444)
        witness = smoke.ROOT / "deploy/tests/literal-env-witness.py"
        fixture = smoke.ROOT / "deploy/tests/literal-env.fixture.yml"
        project = "aura-literal-" + uuid.uuid4().hex
        token = uuid.uuid4().hex
        compose_env = self.directory / "literal-compose.env"
        smoke.protected_write(self.directory, compose_env.name, smoke.env_text({
            "EXPECTED_FILE": str(expected_path),
            "FIXTURE_TOKEN": token,
            "HELPER_IMAGE": helper,
            "RAW_ENV_FILE": str(raw_path),
            "WITNESS_FILE": str(witness),
        }), mode=0o600)
        docker_args = ("compose", "--project-name", project, "--env-file", str(compose_env),
                       "-f", str(fixture))
        image = json.loads(self.docker.call("image", "inspect", helper))[0]
        config_raw = self.docker.call(*docker_args, "config", "--format", "json")
        model = host_tests.deploy.compose_config_json(config_raw)
        service = model["services"]["literal-env"]
        self.assertTrue(service["image"] == helper and service["network_mode"] == "none"
                        and not service.get("ports") and service["read_only"] is True
                        and service["user"] == "10001:10001" and service["cap_drop"] == ["ALL"],
                        "literal witness service boundary changed")
        entries = service["env_file"]
        self.assertTrue([(entry["path"], entry["format"]) for entry in entries]
                        == [(str(raw_path), "raw")], "literal witness raw env contract changed")
        self.assertTrue(all(service["environment"].get(key) == value for key, value in literals.items()),
                        "Compose literal environment changed")
        self.assertTrue(image["Id"] == helper and image["Os"] == "linux"
                        and image["Architecture"] == "amd64", "helper image platform changed")
        baked = dict(value.split("=", 1) for value in image["Config"].get("Env") or [])
        recovery = "compose project=" + project + " token=" + token + " (unconfirmed; no retry, manual recovery)"
        self.docker.recovery.append(recovery)
        self.docker.call(*docker_args, "create", "--pull", "never", "--no-build", "literal-env")
        identifiers = self.docker.call(
            "container", "ls", "-aq", "--no-trunc",
            "--filter", "label=com.docker.compose.project=" + project,
            "--filter", "label=org.aura-historia.literal-env=" + token,
        ).split()
        self.assertEqual(len(identifiers), 1, "literal witness ownership was not unique")
        identifier = identifiers[0]
        self.assertTrue(len(identifier) == 64 and all(character in "0123456789abcdef" for character in identifier),
                        "literal witness container ID was not immutable")
        self.docker.owned.append(identifier)
        self.docker.recovery.remove(recovery)
        item = json.loads(self.docker.call("container", "inspect", identifier))[0]
        self.assertTrue(item["Id"] == identifier and item["Name"] == "/" + project + "-literal-env-1"
                        and item["Image"] == helper
                        and item["Config"]["Labels"].get("org.aura-historia.literal-env") == token
                        and item["Config"]["User"] == "10001:10001"
                        and item["HostConfig"]["NetworkMode"] == "none"
                        and item["HostConfig"]["ReadonlyRootfs"] is True
                        and item["HostConfig"].get("CapAdd") in (None, [])
                        and item["HostConfig"].get("CapDrop") == ["ALL"]
                        and not item["HostConfig"].get("PortBindings"),
                        "literal witness runtime boundary changed")
        result = self.docker.call("start", "--attach", identifier, seconds=30, check=False)
        self.assertEqual(result.returncode, 0, "literal witness command failed")
        self.assertTrue(result.stdout.strip() == "LITERAL_ENV_OK", "literal witness marker missing")
        item = json.loads(self.docker.call("container", "inspect", identifier))[0]
        actual = dict(value.split("=", 1) for value in item["Config"]["Env"])
        expected = dict(baked, **service["environment"])
        self.assertTrue(actual == expected, "literal container environment drift")

    @unittest.skipUnless(os.environ.get("AURA_TEST_CA_MOUNT") == "1", "opt-in cached networkless helper only")
    def test_actual_candidate_ca_bind_is_readable_and_readonly_as_uid10001(self):
        model = self.host.model(self.selected)
        mount = next(m for m in model["services"]["api-candidate"]["volumes"]
                     if m["target"] == "/run/aura/opensearch-ca.pem")
        source = Path(mount["source"])
        public_ca = (smoke.ROOT / "src/aura-historia-worker/src/postgres-test-ca.crt").read_bytes()
        source.chmod(0o600)
        source.write_bytes(public_ca)
        # Public synthetic CA only, inside our 0700 directory. Writable mode makes
        # EROFS prove the bind is read-only rather than merely Unix file permissions.
        source.chmod(0o666)
        digest = hashlib.sha256(public_ca).hexdigest()
        self.docker = smoke.r2.Docker(self.directory)
        image = json.loads(self.docker.call("image", "inspect", smoke.IMAGES["helper"]))[0]
        self.assertEqual(image["Id"], smoke.IMAGES["helper"])
        self.assertEqual(image["Config"]["User"], "10001:10001")
        self.assertFalse(image["Config"].get("Volumes"), "helper declares extra mounts")
        code = """import errno, hashlib, os, pathlib, sys
p = pathlib.Path(sys.argv[1])
assert os.getuid() == os.getgid() == 10001
assert {x.name for x in pathlib.Path('/sys/class/net').iterdir()} == {'lo'}
assert hashlib.sha256(p.read_bytes()).hexdigest() == sys.argv[2]
try:
    with p.open('ab') as f:
        f.write(b'forbidden')
except OSError as error:
    assert error.errno == errno.EROFS
else:
    raise SystemExit(1)
print('CA_MOUNT_OK')
"""
        identifier = self.docker.create(image["Id"], ["--network=none", "--user=10001:10001",
            "--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--pids-limit=16",
            "--memory=64m", "--cpus=1", "--entrypoint=/usr/bin/python3", "--mount",
            f"type=bind,source={source},target={mount['target']},readonly"],
            ["-I", "-c", code, mount["target"], digest])
        self.assertEqual(self.docker.call("wait", identifier, seconds=15).strip(), "0", "CA helper failed")
        self.assertTrue(self.docker.call("logs", identifier).strip() == "CA_MOUNT_OK", "CA helper witness missing")
        actual = json.loads(self.docker.call("container", "inspect", identifier))[0]
        self.assertEqual(actual["Config"]["User"], "10001:10001")
        self.assertEqual(actual["HostConfig"]["NetworkMode"], "none")
        self.assertIs(actual["HostConfig"]["ReadonlyRootfs"], True)
        self.assertEqual(len(actual["Mounts"]), 1)
        bound = actual["Mounts"][0]
        self.assertEqual((bound["Type"], bound["Source"], bound["Destination"], bound["RW"]),
                         ("bind", str(source), mount["target"], False))
        self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), digest)


if __name__ == "__main__":
    unittest.main()
