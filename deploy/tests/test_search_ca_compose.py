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
