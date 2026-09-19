#!/usr/bin/env python3
"""Bounded R3 empty-fixture integration of the three checked-in Compose projects.

No builds, generated Compose, cloud credentials, queue custody or readiness claim.
Only --pull-caddy allows a network download, of one fixed official manifest.
Apps/platform have no external egress. Only Caddy has the project edge bridge:
pinned image, local CA, fixed internal upstream, no provider credentials/external URLs.
Unknown Docker outcomes retain the 0700 journal/config directory; no blind retry.
"""
import argparse
import hashlib
import http.client
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import ssl
import stat
import sys
import tempfile
import time
import traceback
import uuid

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("provider", Path(__file__).with_name("compose-provider.py"))
provider = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provider)
r2 = provider.r2
require = r2.require
ROOT = Path(__file__).resolve().parents[2]
deploy_loader = importlib.machinery.SourceFileLoader(
    "r4_deploy_for_compose", str(ROOT / "deploy/bin/deploy"))
deploy_spec = importlib.util.spec_from_loader(deploy_loader.name, deploy_loader)
deploy = importlib.util.module_from_spec(deploy_spec)
deploy_loader.exec_module(deploy)
compose_config_json = deploy.compose_config_json
CADDY = "caddy@sha256:98eb57d882ccd5213d1688764db10c1ca2c58a1ca3a6717a3411ad798f7a423a"
CADDYFILE = "{\n admin off\n auto_https disable_redirects\n}\nhttps://localhost {\n tls internal\n reverse_proxy api:8080\n}\n"
IMAGES = dict(
    api="sha256:7d0037cc85d6aba544be7f35dc2e2ac2be156c6dc697fdef38a1ae45e3451cfe",
    worker="sha256:963f0ea9bb704cf3b88dfe9791645bfc81a75bb159793bbdb4c54363bbd38284",
    cron="sha256:8e180232aad2995de05ce972ea12d30989c9010597a29c49c1aa28d3bb45be06",
    crawler="sha256:38d7d0067a645e6055aafc8efcdd0ac8f7a6d0119df4dbaf04a65e25f9e204d6",
    helper="sha256:adbdfc3fab194e4291b7e5db1eaa1dfa997ea8a51e5a50229bec60124374deb8",
    postgres=r2.PG_IMAGE,
    opensearch="sha256:0dd81b2051dc9ccd9e466596aa66b7764b55184886eeccd36c1cf17bdf5ed27d",
    redis="sha256:02419de7eddf55aa5bcf49efb74e88fa8d931b4d77c07eff8a6b2144472b6952",
    sequin="sha256:336759c1c632ebdc87939bcea70b26b46df6593e666088e1bf29058209437d3e")
SERVICES = dict(platform={"postgres", "opensearch", "redis", "sequin", "provider", "bootstrap"},
                application={"api", "cron", "crawler", *provider.SCOPES}, edge={"caddy"})
VOLUMES = dict(platform={"postgres-data", "opensearch-data", "redis-data"},
               application=set(), edge={"caddy-data", "caddy-config"})
VOLUME_MOUNTS = {
    "postgres": {"postgres-data": "/var/lib/postgresql/data"},
    "opensearch": {"opensearch-data": "/usr/share/opensearch/data"},
    "redis": {"redis-data": "/data"},
    "caddy": {"caddy-data": "/data", "caddy-config": "/config"},
}
SEARCH_WORKER_ENVFILES = {
    "product-listing-opensearch": "product-listing-opensearch.env",
    "search-filter-projection": "search-filter-projection.env",
    "search-filter-percolator": "search-filter-percolator.env",
}
SEARCH_IDENTITIES = {
    "api": "aura_reader",
    "api-candidate": "aura_reader",
    "product-listing-opensearch": "aura_product_projector",
    "search-filter-projection": "aura_filter_projector",
    "search-filter-percolator": "aura_percolator",
    "cron": "aura_cron",
}
SEARCH_PASSWORDS = {
    "api": "c2-synthetic-api-reader-password",
    "cron": "c2-synthetic-cron-password",
    "product-listing-opensearch": "c2-synthetic-product-projector-password",
    "search-filter-projection": "c2-synthetic-filter-projector-password",
    "search-filter-percolator": "c2-synthetic-percolator-password",
}
SEARCH_SECRET_FILES = frozenset(SEARCH_WORKER_ENVFILES.values())
BIND_MOUNTS = {
    "postgres": {"postgres": "/etc/postgresql"},
    "opensearch": {"opensearch.yml": "/usr/share/opensearch/config/opensearch.yml",
                   "opensearch-certs": "/usr/share/opensearch/config/certs"},
    "redis": {"redis.conf": "/usr/local/etc/redis/redis.conf"},
    "sequin": {"sequin.yaml": "/run/sequin/sequin.yaml"},
    "caddy": {"caddy": "/etc/caddy"},
    **{name: {"postgres-ca.pem": "/run/aura/postgres-ca.pem"} for name in SERVICES["application"]},
}
for _name in {"api", "cron", "crawler", *provider.GOOGLE_SCOPES}:
    BIND_MOUNTS[_name]["google-adc.json"] = provider.ADC_PATH
for _name in {"api", "cron", "product-listing-opensearch", "search-filter-projection", "search-filter-percolator"}:
    BIND_MOUNTS[_name]["opensearch-ca.pem"] = "/run/aura/opensearch-ca.pem"
PUBLIC_BINDS = {
    "opensearch": {ROOT / "opensearch/analysis": "/usr/share/opensearch/config/analysis"},
    "provider": {ROOT / "deploy/tests": "/smoke"},
}
ENV_FILES = {"postgres": ["postgres.env"], "sequin": ["sequin.env"],
             **{name: [name + ".env"] for name in ("api", "cron", "crawler")},
             **{name: ["worker.env"] + ([SEARCH_WORKER_ENVFILES[name]] if name in SEARCH_WORKER_ENVFILES else [])
                for name in provider.SCOPES},
             "notification-delivery": ["worker.env", "notification-delivery.env"]}
TMPFS = {**{name: "/tmp:rw,nosuid,nodev,noexec,size=64m,mode=1777" for name in SERVICES["application"]},
         "bootstrap": "/tmp:rw,nosuid,nodev,noexec,size=16m,mode=1777"}
FIXTURE_DIRS = {"postgres": {"postgresql.conf"}, "opensearch-certs": set(), "caddy": {"Caddyfile"}}
HISTORY = "SELECT version||':'||encode(checksum,'hex')||':'||success::text FROM public._sqlx_migrations ORDER BY version;"
DELIVERY = {"S3_BUCKET_NAME_TEMPLATES", "NOTIFICATION_EMAIL_FROM", "NOTIFICATION_EMAIL_REPLY_TO",
            "AWS_ENDPOINT_URL_S3", "AWS_ENDPOINT_URL_SESV2"}
QUEUE_FIELDS = {"AURA_HISTORIA_WORKER_SCOPE", "AURA_HISTORIA_WORKER_QUEUE_URL"}
# Canonical ProductListingId: UUIDv7 00000000-0000-7000-8000-000000000001.
SAFE_GET = "/api/v1/product-listings/pl_0000000000e008000000000001"


def worker_environment():
    common, delivery = {}, {}
    for scope in provider.SCOPES:
        for key, value in provider.fixture_environment("worker", scope).items():
            if key in QUEUE_FIELDS:
                continue
            target = delivery if key in DELIVERY else common
            require(key not in target or target[key] == value, "WORKER_ENV_CONFLICT")
            target[key] = value
    require(set(delivery) == DELIVERY and not {"OPENSEARCH_USERNAME", "OPENSEARCH_PASSWORD"} & common.keys(),
            "DELIVERY_ENV")
    return common, delivery


def protected_write(directory, name, text, mode=0o444):
    path = directory / name
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_text(text)
    # Public synthetic contents only, except raw search credentials kept private even in fixtures.
    path.chmod(mode)


def env_text(values):
    require(all(re.fullmatch(r"[A-Z0-9_]+", k) and "\n" not in v for k, v in values.items()), "ENV_SHAPE")
    return "".join(f"{key}={value}\n" for key, value in sorted(values.items()))


def native_sequin(catalog):
    require({w["scope"] for w in catalog["workers"]} == set(provider.SCOPES), "CATALOG_SCOPES")
    return dict(account=dict(name="r3-empty-test"),
        api_tokens=[dict(name="test", token="r3-fixture-only-token")],
        databases=[dict(name="business", username="smoke_replication", password=r2.SECRET,
            hostname="postgres", port=5432, database="smoke_business", pool_size=3,
            slot=dict(name="r3_slot", create_if_not_exists=False),
            publication=dict(name="r3_pub", create_if_not_exists=False))],
        http_endpoints=[dict(name=w["scope"], url=f"http://{w['scope']}:8081/cdc/sequin")
                        for w in catalog["workers"]],
        sinks=[dict(name=w["scope"], database="business", source=dict(include_tables=["public." + w["table"]]),
                    actions=[op.lower() for op in w["operations"]], batch_size=1,
                                        initial_backfill=False, load_shedding_policy="pause_on_full",
                    destination=dict(type="webhook", http_endpoint=w["scope"], batch=False))
               for w in catalog["workers"]])


def app_environments():
    common, delivery = worker_environment()
    values = {name: provider.fixture_environment(name) for name in ("api", "cron", "crawler")}
    for name in ("api", "cron"):
        values[name].update(OPENSEARCH_USERNAME=SEARCH_IDENTITIES[name], OPENSEARCH_PASSWORD=SEARCH_PASSWORDS[name])
    values["crawler"].pop("OPENSEARCH_USERNAME", None)
    values["crawler"].pop("OPENSEARCH_PASSWORD", None)
    for scope in provider.SCOPES:
        scoped = provider.fixture_environment("worker", scope)
        values[scope] = dict(common, **{key: scoped[key] for key in QUEUE_FIELDS})
        if scope in SEARCH_WORKER_ENVFILES:
            values[scope].update(OPENSEARCH_USERNAME=SEARCH_IDENTITIES[scope], OPENSEARCH_PASSWORD=SEARCH_PASSWORDS[scope])
        if scope == "notification-delivery":
            values[scope].update(delivery)
    return values


def materialize(directory, projects, port, images, environments=None):
    catalog = json.loads((ROOT / "deploy/catalog.json").read_text())
    environments = app_environments() if environments is None else environments
    for kind in ("api", "cron", "crawler"):
        protected_write(directory, kind + ".env", env_text(environments[kind]))
    common, delivery = worker_environment()
    protected_write(directory, "worker.env", env_text(common))
    protected_write(directory, "notification-delivery.env", env_text(delivery))
    for scope, filename in SEARCH_WORKER_ENVFILES.items():
        credentials = {key: environments[scope][key] for key in ("OPENSEARCH_USERNAME", "OPENSEARCH_PASSWORD")}
        protected_write(directory, filename, env_text(credentials), mode=0o600)
    protected_write(directory, "google-adc.json", json.dumps(provider.fixture_adc()))
    protected_write(directory, "postgres-ca.pem", "unused synthetic STAGE=test CA mount\n")
    protected_write(directory, "opensearch-ca.pem", "unused synthetic STAGE=test CA mount\n")
    protected_write(directory, "postgres.env", env_text(dict(POSTGRES_USER="postgres", POSTGRES_PASSWORD=r2.SECRET)))
    protected_write(directory, "postgres/postgresql.conf", "listen_addresses='*'\nshared_preload_libraries='pg_ttl_index'\nwal_level=logical\nmax_replication_slots=10\nmax_wal_senders=10\nmax_connections=120\nssl=off\n")
    (directory / "postgres").chmod(0o755)
    protected_write(directory, "opensearch.yml", "cluster.name: r3-test\nnetwork.host: 0.0.0.0\ndiscovery.type: single-node\nplugins.security.disabled: true\n")
    (directory / "opensearch-certs").mkdir(mode=0o755)
    protected_write(directory, "redis.conf", "bind 0.0.0.0\nprotected-mode no\nappendonly yes\ndir /data\n")
    protected_write(directory, "sequin.env", env_text(dict(
        PG_URL=f"postgres://smoke_metadata:{r2.SECRET}@postgres:5432/smoke_sequin", PG_POOL_SIZE="3",
        REDIS_URL="redis://redis:6379", SERVER_PORT="7376", SERVER_HOST="sequin",
        SECRET_KEY_BASE="wDPLYus0pvD6qJhKJICO4vYl782Zjtpew5qRBDp7CZvbWtQmY0eB13If01234567",
        VAULT_KEY="2Sig69bIpuSm2kv0VQfDekET2qy8qUZGI8v3/h3ASiY=",
        TELEMETRY_ENABLED="false", CRASH_REPORTING_DISABLED="true")))
    # JSON is native YAML, not a Compose renderer.
    protected_write(directory, "sequin.yaml", json.dumps(native_sequin(catalog)))
    protected_write(directory, "caddy/Caddyfile", CADDYFILE)
    (directory / "caddy").chmod(0o755)
    env = {key.upper() + "_IMAGE": value for key, value in images.items()}
    env.update(AURA_CONFIG_DIR=str(directory), AURA_NETWORK=projects["platform"] + "_backend",
               AURA_NETWORK_INTERNAL="true", AURA_HTTPS_PORT=str(port))
    env.update({"QUEUE_" + scope.upper().replace("-", "_"):
                provider.fixture_environment("worker", scope)["AURA_HISTORIA_WORKER_QUEUE_URL"] for scope in provider.SCOPES})
    protected_write(directory, "compose.env", env_text(env))
    return catalog


def validate_caddyfile(directory):
    config = directory / "caddy"
    require({p.name for p in config.iterdir()} == {"Caddyfile"}
            and (config / "Caddyfile").read_text() == CADDYFILE, "CADDY_FIXED_LOCAL_CONFIG")


def checked_path(path):
    require(path.is_absolute() and path.resolve(strict=True) == path, "BIND_SYMLINK_OR_ALIAS")
    for child in [path, *(path.rglob("*") if path.is_dir() else [])]:
        mode = child.lstat()
        require(stat.S_ISDIR(mode.st_mode) or stat.S_ISREG(mode.st_mode) and mode.st_nlink == 1,
                "BIND_SYMLINK_OR_SPECIAL_FILE")


def fixture_hashes(directory):
    checked_path(directory)
    root_stat = directory.stat()
    require(stat.S_IMODE(root_stat.st_mode) == 0o700 and root_stat.st_uid == os.getuid(), "FIXTURE_ROOT_PERMISSIONS")
    for name, children in FIXTURE_DIRS.items():
        path = directory / name
        require(path.is_dir() and {p.name for p in path.iterdir()} == children
                and stat.S_IMODE(path.stat().st_mode) == 0o755, "FIXTURE_DIRECTORY_SHAPE")
    names = {"compose.env", *(name for names in ENV_FILES.values() for name in names),
             *(name for mounts in BIND_MOUNTS.values() for name in mounts if name not in FIXTURE_DIRS),
             *(folder + "/" + name for folder, names in FIXTURE_DIRS.items() for name in names)}
    result = {}
    for name in names:
        path = directory / name
        expected_mode = 0o600 if name in SEARCH_SECRET_FILES else 0o444
        require(path.is_file() and stat.S_IMODE(path.stat().st_mode) == expected_mode, "FIXTURE_FILE_PERMISSIONS")
        result[name] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


def bind_pairs(service, directory):
    return {**{directory / source: target for source, target in BIND_MOUNTS.get(service, {}).items()},
            **PUBLIC_BINDS.get(service, {})}


def validate_mounts(service, mounts, project, directory, actual=False):
    binds = bind_pairs(service, directory)
    expected = {("bind", str(source), target, True) for source, target in binds.items()}
    expected |= {("volume", project + "_" + key if actual else key, target, False)
                 for key, target in VOLUME_MOUNTS.get(service, {}).items()}
    observed = []
    for mount in mounts:
        kind = mount.get("Type" if actual else "type")
        target = mount.get("Destination" if actual else "target")
        if actual and kind == "tmpfs":
            require(service in TMPFS and target == "/tmp" and mount.get("RW") is True, "ACTUAL_TMPFS")
            continue
        source = mount.get(("Name" if kind == "volume" else "Source") if actual else "source")
        require(type(mount.get("RW")) is bool if actual else type(mount.get("read_only", False)) is bool, "MOUNT_READONLY_TYPE")
        readonly = not mount["RW"] if actual else mount.get("read_only", False)
        observed.append((kind, source, target, readonly))
        if not actual:
            require(set(mount) <= {"type", "source", "target", "read_only", "bind", "volume"}, "MOUNT_OPTIONS")
            require(mount.get("volume", {}) == {} and
                    (mount.get("bind") == {"create_host_path": False} if kind == "bind" else "bind" not in mount), "MOUNT_OPTIONS")
        elif kind == "bind":
            require(mount.get("Propagation") == "rprivate", "BIND_PROPAGATION")
        elif kind == "volume":
            require(mount.get("Driver") == "local", "VOLUME_DRIVER")
    require(len(observed) == len(expected) and set(observed) == expected, "EXACT_SERVICE_MOUNTS_" + service)
    for source in binds:
        checked_path(source)


def validate_env_files(kind, model, directory):
    require(set(model["services"]) == SERVICES[kind], "COMPOSE_SERVICE_SET_" + kind)
    for name, service in model["services"].items():
        files = service.get("env_file", [])
        require(len(files) == len(ENV_FILES.get(name, [])), "ENV_FILE_SET_" + name)
        for value, filename in zip(files, ENV_FILES.get(name, [])):
            require(value.get("path") == str(directory / filename) and value.get("format") == "raw"
                    and value.get("required", True) is True
                    and set(value) <= {"path", "format", "required"}, "ENV_FILE_PATH_" + name)
            checked_path(directory / filename)


def validate_model(kind, model, images, projects, directory, port, environments):
    project = projects[kind]
    network = projects["platform"] + "_backend"
    edge_network = projects["edge"] + "_edge"
    require(set(model["services"]) == SERVICES[kind], "COMPOSE_SERVICE_SET_" + kind)
    require(set(model.get("volumes", {})) == VOLUMES[kind], "COMPOSE_VOLUME_SET_" + kind)
    for key, volume in model.get("volumes", {}).items():
        require(volume.get("name") == project + "_" + key and volume.get("driver", "local") == "local"
                and set(volume) <= {"name", "driver"}, "VOLUME_DECLARATION_" + key)
    require(set(model["networks"]) == ({"backend", "edge"} if kind == "edge" else {"backend"}), "EXACT_NETWORK_SET")
    if kind == "edge":
        edge = model["networks"]["edge"]
        require(edge_network is not None and edge["name"] == edge_network
                and not edge.get("external") and not edge.get("internal")
                and edge.get("driver", "bridge") == "bridge"
                and edge.get("ipam", {}) == {}
                and set(edge) <= {"name", "driver", "internal", "ipam"}, "EDGE_PRIVATE_BRIDGE")
    net = model["networks"]["backend"]
    require(net["name"] == network and net.get("ipam", {}) == {}, "INTERNAL_NETWORK_GATE")
    if kind == "platform":
        require(net.get("internal") is True and net.get("driver", "bridge") == "bridge"
                and set(net) <= {"name", "internal", "driver", "ipam"}, "INTERNAL_NETWORK_GATE")
    else:
        require(net.get("external") is True and set(net) <= {"name", "external", "ipam"}, "INTERNAL_NETWORK_GATE")
    for name, service in model["services"].items():
        key = "worker" if name in provider.SCOPES else "helper" if name in {"bootstrap", "provider"} else name
        require(service["image"] == images[key] and service.get("pull_policy") == "never" and not service.get("build"), "IMAGE_GATE_" + name)
        require(not any(service.get(key) for key in ("privileged", "devices", "extra_hosts", "volumes_from", "secrets", "configs", "use_api_socket")), "ISOLATION_GATE")
        require(service.get("tmpfs", []) == ([TMPFS[name]] if name in TMPFS else []), "TMPFS_QUOTING_" + name)
        validate_mounts(name, service.get("volumes", []), project, directory)
        if name == "bootstrap":
            require(service.get("network_mode") == "service:postgres" and not service.get("networks"), "BOOTSTRAP_NAMESPACE")
        else:
            require(set(service.get("networks", {})) == ({"backend", "edge"} if name == "caddy" else {"backend"})
                    and not service.get("network_mode"), "NETWORK_GATE")
        if name == "caddy":
            require(not any(service.get(key) for key in ("environment", "env_file", "secrets", "configs", "command", "entrypoint")), "CADDY_NO_CREDENTIALS_OR_OVERRIDE")

        ports = service.get("ports", [])
        require(ports == [] if name != "caddy" else len(ports) == 1 and ports[0]["host_ip"] == "127.0.0.1"
                and str(ports[0]["published"]) == str(port) and ports[0]["target"] == 443, "PORT_GATE")
        if kind == "application":
            env = service["environment"]
            require(env.get("STAGE") == "test" and env.get("POSTGRES_SSL_MODE") == "disable"
                    and service["user"] == "10001:10001" and service["read_only"], "APP_TEST_GATE")
            require(env == environments[name] and not service.get("command") and not service.get("entrypoint"), "APP_ENV_OR_COMMAND_DRIFT_" + name)


def owned_volume(volume, kind, project):
    labels = volume.get("Labels") or {}
    key = labels.get("com.docker.compose.volume")
    require(key in VOLUMES[kind] and volume.get("Name") == project + "_" + key
            and labels.get("com.docker.compose.project") == project
            and volume.get("Driver") == "local" and volume.get("Scope") == "local"
            and volume.get("Options") in (None, {}), "VOLUME_OWNERSHIP")


def owned_network(net, kind, project, allowed_ids):
    key = {"platform": "backend", "edge": "edge"}.get(kind)
    labels = net.get("Labels") or {}
    require(key is not None and net["Name"] == project + "_" + key
            and net["Internal"] is (kind == "platform") and net["Driver"] == "bridge"
            and net["Scope"] == "local" and net.get("Options") in (None, {})
            and net.get("IPAM", {}).get("Driver") == "default"
            and net.get("IPAM", {}).get("Options") in (None, {})
            and labels.get("com.docker.compose.project") == project
            and labels.get("com.docker.compose.network") == key, "NETWORK_OWNERSHIP")
    require(set(net.get("Containers", {})) <= allowed_ids, "NETWORK_UNOWNED_MEMBER")


def owned_container(item, project, services):
    labels = item["Config"].get("Labels") or {}
    service = labels.get("com.docker.compose.service")
    require(labels.get("com.docker.compose.project") == project and service in services
            and item["Name"] == f"/{project}-{service}-1"
            and re.fullmatch(r"[0-9a-f]{64}", item["Id"]), "CONTAINER_OWNERSHIP")
    return service


# Runs inside the existing provider container; responses stay captured by Docker.call.
HTTP = """import http.client,json,sys
host,port,path,method=sys.argv[1:]
c=http.client.HTTPConnection(host,int(port),timeout=3)
try:
 c.request(method,path,body=sys.stdin.read() or None,headers={'Content-Type':'application/json'})
 r=c.getresponse(); raw=r.read(65537)
 if len(raw)>65536: raise ValueError('size')
 try: body=json.loads(raw)
 except ValueError: body=raw.decode()
 print(json.dumps({'status':r.status,'body':body}))
except ConnectionRefusedError: print(json.dumps({'status':0}))
finally: c.close()
"""


class Harness:
    def __init__(self, directory):
        self.directory = directory
        self.projects = {kind: "aura-r3-" + uuid.uuid4().hex + "-" + kind for kind in SERVICES}
        self.docker = r2.Docker(directory)
        self.deadline = time.monotonic() + 1800
        self.images = {}
        self.image_envs = {}
        self.environments = app_environments()
        self.models = {}
        self.fixture_inputs = None
        self.uncertain = False
        self.mutated = False
        self.phase = "setup"
        self.ids = {}
        self.compose_inputs = [ROOT / f"deploy/compose/compose.{kind}.yml" for kind in SERVICES] + [ROOT / "deploy/tests/compose.fixture.yml"]
        self.compose_hashes = [hashlib.sha256(path.read_bytes()).hexdigest() for path in self.compose_inputs]
        self.journal()
        print("PROJECTS " + json.dumps(self.projects, sort_keys=True), flush=True)
        print("JOURNAL " + str(directory / "journal.json"), flush=True)

    def journal(self):
        path = self.directory / "journal.json"
        with path.open("w") as output:
            os.chmod(path, 0o600)
            json.dump(dict(projects=self.projects, phase=self.phase, uncertain=self.uncertain), output, indent=2)
            output.flush()
            os.fsync(output.fileno())

    def call(self, *args, seconds=20, **kwargs):
        remaining = self.deadline - time.monotonic()
        require(remaining > 0, "TOTAL_DEADLINE")
        try:
            check = kwargs.pop("check", True)
            result = self.docker.call(*args, seconds=min(seconds, remaining), check=False, **kwargs)
            if check and result.returncode:
                if args[0] == "compose":
                    diagnostic = "\n".join(result.stdout.splitlines()[-2:])
                    diagnostic = diagnostic.replace(str(self.directory), "<fixture>")
                    r2.safe_output(diagnostic)
                    print("COMPOSE_DIAGNOSTIC " + diagnostic, flush=True)
                categories = [word for word in ("unhealthy", "permission denied", "does not exist", "syntax error", "connection refused", "pg_hba", "address already in use", "not found") if word in result.stdout.lower()]
                raise r2.Failure("DOCKER_FAILED_" + args[0] + "_" + str(result.returncode) + "_" + ",".join(categories))
            return result.stdout if check else result
        except BaseException as error:
            if not isinstance(error, r2.Failure) or str(error) in {"DOCKER_COMMAND_UNCONFIRMED", "DOCKER_OUTPUT_LIMIT"} or str(error).startswith("INTERRUPTED_"):
                self.uncertain = True
                self.journal()
            raise

    def compose(self, kind, *args, seconds=30, **kwargs):
        require([hashlib.sha256(path.read_bytes()).hexdigest() for path in self.compose_inputs] == self.compose_hashes, "COMPOSE_CHANGED_DURING_RUN")
        require(self.fixture_inputs is not None and fixture_hashes(self.directory) == self.fixture_inputs, "FIXTURE_CHANGED_DURING_RUN")
        if args[0] != "config":
            require(kind in self.models, "UNVERIFIED_COMPOSE_MUTATION")
            require(self.model(kind) == self.models[kind], "COMPOSE_MODEL_CHANGED")
        if kind == "edge":
            validate_caddyfile(self.directory)
        files = ["-f", str(ROOT / f"deploy/compose/compose.{kind}.yml")]
        if kind == "platform":
            files += ["-f", str(ROOT / "deploy/tests/compose.fixture.yml")]
        return self.call("compose", "--project-name", self.projects[kind], "--env-file", str(self.directory / "compose.env"),
                         *files, *args, seconds=seconds, **kwargs)

    def model(self, kind):
        raw = compose_config_json(self.compose(kind, "config", "--no-env-resolution", "--format", "json"))
        validate_env_files(kind, raw, self.directory)
        resolved = compose_config_json(self.compose(kind, "config", "--format", "json"))
        validate_model(kind, resolved, self.images, self.projects, self.directory, self.port, self.environments)
        return resolved

    def phase_start(self, phase):
        self.phase = phase
        self.journal()
        print("STEP " + phase, flush=True)

    def resources(self, kind):
        project = self.projects[kind]
        ids = self.call("container", "ls", "-a", "--no-trunc", "--filter", "label=com.docker.compose.project=" + project,
                        "--format", "{{.ID}}").split()
        containers = json.loads(self.call("container", "inspect", *ids)) if ids else []
        found = {}
        for item in containers:
            service = owned_container(item, project, SERVICES[kind])
            require(service not in found, "DUPLICATE_SERVICE")
            found[service] = item
        # Compose down addresses declared volume names too: catch unlabeled collisions.
        names = self.call("volume", "ls", "--filter", "name=" + project + "_", "--format", "{{.Name}}").split()
        volumes = json.loads(self.call("volume", "inspect", *names)) if names else []
        for volume in volumes:
            owned_volume(volume, kind, project)
        networks = self.call("network", "ls", "--filter", "name=" + project + "_", "--format", "{{.ID}}").split()
        nets = json.loads(self.call("network", "inspect", *networks)) if networks else []
        for net in nets:
            allowed = {item["Id"] for name, item in found.items() if name == "caddy"} if kind == "edge" else set(self.all_container_ids())
            owned_network(net, kind, project, allowed)
        return found, volumes, nets

    def all_container_ids(self):
        result = []
        for project in self.projects.values():
            result.extend(self.call("container", "ls", "-a", "--no-trunc", "--filter", "label=com.docker.compose.project=" + project,
                                    "--format", "{{.ID}}").split())
        return result

    def refresh(self, kind, expected):
        found, volumes, nets = self.resources(kind)
        require(set(found) == expected, "ACTUAL_SERVICE_SET_" + kind)
        require(len(volumes) == len(VOLUMES[kind]), "PERSISTENT_VOLUMES_" + kind)
        require(len(nets) == (0 if kind == "application" else 1), "ACTUAL_PROJECT_NETWORK_SET")
        network = self.projects["platform"] + "_backend"
        for service, item in found.items():
            require(item["State"]["Running"] and item["RestartCount"] == 0, "SERVICE_NOT_STABLE_" + service)
            key = "worker" if service in provider.SCOPES else "helper" if service in {"provider", "bootstrap"} else service
            require(item["Image"] == self.images[key], "ACTUAL_IMAGE_" + service)
            attached = set(item["NetworkSettings"]["Networks"])
            if service == "bootstrap":
                require(not attached and item["HostConfig"]["NetworkMode"] == "container:" + found["postgres"]["Id"], "ACTUAL_NAMESPACE_bootstrap")
            else:
                expected_networks = {network, self.projects["edge"] + "_edge"} if service == "caddy" else {network}
                require(attached == expected_networks and item["HostConfig"]["NetworkMode"] in expected_networks, "ACTUAL_NAMESPACE_" + service)
            validate_mounts(service, item["Mounts"], self.projects[kind], self.directory, actual=True)
            expected_tmpfs = {"/tmp": TMPFS[service].split(":", 1)[1]} if service in TMPFS else {}
            require((item["HostConfig"].get("Tmpfs") or {}) == expected_tmpfs, "ACTUAL_TMPFS")
            if kind == "application":
                actual_env = item["Config"]["Env"]
                expected_env = dict(self.image_envs[key], **self.environments[service])
                require(len(actual_env) == len(expected_env) and dict(value.split("=", 1) for value in actual_env) == expected_env, "ACTUAL_APP_ENV_DRIFT")
            if service == "caddy":
                require(all(item["Config"].get(key) == self.caddy_config.get(key) for key in ("Env", "Cmd", "Entrypoint")), "CADDY_PINNED_RUNTIME_CONFIG")
                validate_caddyfile(self.directory)
            require(not item["HostConfig"]["PortBindings"] if service != "caddy" else
                    item["HostConfig"]["PortBindings"] == {"443/tcp": [{"HostIp": "127.0.0.1", "HostPort": str(self.port)}]}, "ACTUAL_PORT_BINDINGS")
            self.ids[service] = item["Id"]
        return found, volumes, nets

    def sql(self, db, query):
        return self.call("exec", "-i", self.ids["postgres"], "psql", "-X", "-v", "ON_ERROR_STOP=1", "-At",
                         "-U", "postgres", "-d", db, body=query, seconds=20).strip()

    def request(self, host, port, path, method="GET", body=None):
        raw = self.call("exec", "-i", self.ids["provider"], "/usr/bin/python3", "-c", HTTP, host, str(port), path, method,
                        body=json.dumps(body) if body is not None else "", seconds=10)
        r2.safe_output(raw)
        return json.loads(raw)

    def stats(self):
        result = self.request("provider", 18090, "/_state")
        require(result["status"] == 200, "PROVIDER_STATE")
        counters = result["body"]
        require(not any(value for key, value in counters.items() if "unexpected" in key or "forbidden" in key), "UNEXPECTED_PROVIDER_OPERATION")
        return counters

    def probe(self, name, port, path):
        command = f"exec 3<>/dev/tcp/127.0.0.1/{port}; printf 'GET {path} HTTP/1.1\\r\\nHost: localhost\\r\\nConnection: close\\r\\n\\r\\n' >&3; cat <&3"
        result = self.call("exec", self.ids[name], "timeout", "5", "bash", "-c", command, seconds=8, check=False)
        require(result.returncode != 124, "PRIVATE_PROBE_TIMEOUT_" + name)
        if result.returncode:
            return {"status": 0}
        r2.safe_output(result.stdout)
        class Socket:
            def makefile(self, *_args):
                return io.BytesIO(result.stdout.encode())
        response = http.client.HTTPResponse(Socket())
        response.begin()
        raw = response.read(65537)
        require(len(raw) <= 65536 and "no-store" in response.getheader("Cache-Control", ""), "PROBE_CONTRACT_" + name)
        try:
            body = json.loads(raw)
        except ValueError:
            body = raw.decode()
        return dict(status=response.status, body=body)

    def apps_ready(self):
        self.refresh("application", SERVICES["application"])
        for name in sorted(SERVICES["application"]):
            kind = "worker" if name in provider.SCOPES else name
            port = r2.PORTS[kind]
            r2.until(lambda: self.probe(name, port, "/ready")["status"] == (204 if kind == "api" else 200), 90, "READY_" + name)
            version = self.probe(name, port, "/ops/version" if kind in {"cron", "crawler"} else "/version")
            require(version["status"] == 200 and version["body"] == r2.identity(kind, name if kind == "worker" else None), "IDENTITY_" + name)
        self.refresh("application", SERVICES["application"])

    def witnesses(self, before):
        def complete():
            counts = self.stats()
            return all(counts.get(scope + "." + event, 0) > before.get(scope + "." + event, 0)
                       for scope in provider.SCOPES for event in ("attributes_source_complete", "attributes_dlq_complete", "receive_complete"))
        r2.until(complete, 60, "ALL_TEN_SCOPE_WITNESSES")
        print("PASS 10 scopes: source/DLQ attributes and empty receive completions", flush=True)

    def initialize(self, catalog):
        self.phase_start("fresh PostgreSQL/bootstrap/OpenSearch")
        self.compose("platform", "up", "-d", "--no-build", "--pull", "never", "postgres", "opensearch", "redis", "provider", "bootstrap", seconds=120)
        self.refresh("platform", SERVICES["platform"] - {"sequin"})
        require(160000 <= int(self.sql("postgres", "SHOW server_version_num;")) < 170000, "PG16")
        self.sql("postgres", f"CREATE ROLE smoke_metadata LOGIN PASSWORD '{r2.SECRET}' NOSUPERUSER NOCREATEDB NOCREATEROLE; CREATE ROLE smoke_replication LOGIN REPLICATION PASSWORD '{r2.SECRET}' NOSUPERUSER NOCREATEDB NOCREATEROLE; CREATE ROLE smoke_runtime LOGIN PASSWORD '{r2.SECRET}' NOSUPERUSER NOCREATEDB NOCREATEROLE;")
        for database in ("smoke_business", "smoke_crawler", "smoke_sequin"):
            owner = "smoke_metadata" if database == "smoke_sequin" else "postgres"
            self.sql("postgres", f"CREATE DATABASE {database} OWNER {owner} TEMPLATE template0;")
        self.sql("smoke_business", "CREATE EXTENSION pg_ttl_index WITH SCHEMA public; SELECT ttl_start_worker();")
        require(self.sql("smoke_business", "SELECT extversion FROM pg_extension WHERE extname='pg_ttl_index';") == "3.0.0", "TTL3")
        self.histories = {}
        for target in ("business", "crawler"):
            key = "BUSINESS_DATABASE_URL" if target == "business" else "LOCAL_DB_URL"
            setup = r2.env_args(dict(STAGE="test", POSTGRES_SSL_MODE="disable", **{key: f"postgres://postgres:{r2.SECRET}@127.0.0.1:5432/smoke_{target}"}))
            for mode, expected in (("--initialize-fresh", "INITIALIZED_"), ("--verify", "VERIFIED_")):
                output = self.call("exec", *setup, self.ids["bootstrap"], "/usr/local/bin/bootstrap-local", mode, target, seconds=100)
                require(output.strip() == expected + target.upper(), "BOOTSTRAP_NO_RETRY_" + target)
            sources = ROOT / ("migrations" if target == "business" else "src/crawler/migrations")
            expected = "\n".join(f"{p.name.split('_')[0]}:{hashlib.sha384(p.read_bytes()).hexdigest()}:true" for p in sorted(sources.glob("*.sql")))
            require(self.sql("smoke_" + target, HISTORY) == expected, "GENUINE_HISTORY_" + target)
            self.histories[target] = expected
            self.sql("smoke_" + target, "REVOKE CONNECT ON DATABASE smoke_" + target + " FROM PUBLIC; GRANT CONNECT ON DATABASE smoke_" + target + " TO smoke_runtime; GRANT USAGE ON SCHEMA public TO smoke_runtime; GRANT SELECT ON ALL TABLES IN SCHEMA public TO smoke_runtime;")
        self.sql("smoke_crawler", "GRANT UPDATE(crawl_enabled, updated) ON listing_sources TO smoke_runtime;")
        tables = sorted({w["table"] for w in catalog["workers"]})
        require(len(tables) == 5 and all(re.fullmatch(r"[a-z_]+", t) for t in tables), "PUBLICATION_TABLES")
        self.tables = tables
        joined = ",".join("public." + t for t in tables)
        self.sql("smoke_business", f"GRANT CONNECT ON DATABASE smoke_business TO smoke_replication; GRANT USAGE ON SCHEMA public TO smoke_replication; GRANT SELECT ON {joined} TO smoke_replication; CREATE PUBLICATION r3_pub FOR TABLE {joined} WITH (publish_via_partition_root=true); SELECT pg_create_logical_replication_slot('r3_slot','pgoutput');")
        require(self.sql("smoke_business", "SELECT tablename FROM pg_publication_tables WHERE pubname='r3_pub' ORDER BY tablename;") == "\n".join(tables), "EXACT_PUBLICATION")
        r2.until(lambda: self.request("opensearch", 9200, "/")["status"] == 200, 120, "OPENSEARCH_START")
        require(self.request("opensearch", 9200, "/")["body"]["version"]["number"] == "3.1.0", "OS_VERSION")
        pipeline = {"phase_results_processors": [{"score-ranker-processor": {"combination": {"technique": "rrf"}}}]}
        require(self.request("opensearch", 9200, "/_search/pipeline/hybrid-search-pipeline", "PUT", pipeline)["status"] == 200, "PIPELINE")
        for entry in catalog["search"]:
            mapping = json.loads((ROOT / entry["definition"]).read_text())
            index = entry["physical_baseline"]
            require(self.request("opensearch", 9200, "/" + index, "PUT", mapping)["status"] == 200, "MAPPING")
            actual = self.request("opensearch", 9200, "/" + index + "/_mapping")["body"][index]["mappings"]
            require(actual == r2.mapping_readback(mapping["mappings"]), "MAPPING_READBACK")
        print("PASS fresh 3 databases, genuine SQLx histories, TTL3, five-table slot/publication, search mappings", flush=True)

    def sequin_ready(self, catalog):
        r2.until(lambda: self.request("sequin", 7376, "/health")["status"] == 200, 120, "SEQUIN_HEALTH")
        r2.until(lambda: self.sql("smoke_business", "SELECT active FROM pg_replication_slots WHERE slot_name='r3_slot';") == "t", 120, "SEQUIN_ACTIVE_SLOT")
        require(self.sql("smoke_business", "SELECT usename FROM pg_stat_replication WHERE pid=(SELECT active_pid FROM pg_replication_slots WHERE slot_name='r3_slot');") == "smoke_replication", "SEQUIN_RESTRICTED_SOURCE_ROLE")
        # Read actual persisted config, not the input file or a count in generated YAML.
        sinks = json.loads(self.sql("smoke_sequin", "SELECT coalesce(json_agg(t),'[]') FROM sequin_config.sink_consumers t;"))
        endpoints = json.loads(self.sql("smoke_sequin", "SELECT coalesce(json_agg(t),'[]') FROM sequin_config.http_endpoints t;"))
        require(len(sinks) == 10 and {s["name"] for s in sinks} == set(provider.SCOPES), "SEQUIN_LOADED_CONSUMERS")
        require(len(endpoints) == 10 and {e["name"] for e in endpoints} == set(provider.SCOPES), "SEQUIN_LOADED_ENDPOINTS")
        by_id = {e["id"]: e for e in endpoints}
        for sink in sinks:
            endpoint = by_id[sink["sink"]["http_endpoint_id"]]
            require(endpoint["name"] == sink["name"] and endpoint["host"] == sink["name"]
                    and endpoint["port"] == 8081 and endpoint["path"] == "/cdc/sequin"
                    and endpoint["scheme"] == "http" and sink["status"] == "active", "SEQUIN_CONSUMER_ENDPOINT_BINDING")
            require(sink.get("load_shedding_policy") == "pause_on_full", "SEQUIN_LOAD_SHEDDING_POLICY")
        require(self.sql("smoke_sequin", "SELECT count(*) FROM sequin_config.backfills;") == "0", "SEQUIN_UNEXPECTED_BACKFILL")
        print("PASS Sequin health, active PG slot, 10 loaded real worker endpoint bindings; pause_on_full; no backfills", flush=True)

    def tls_get(self, port):
        r2.until(lambda: self.call("exec", self.ids["caddy"], "test", "-f", "/data/caddy/pki/authorities/local/root.crt", check=False).returncode == 0, 20, "CADDY_LOCAL_CA")
        direct = self.request("api", 8080, SAFE_GET)
        require(direct["status"] == 404, "API_SAFE_GET_STATUS_" + str(direct["status"]))
        root = self.call("exec", self.ids["caddy"], "cat", "/data/caddy/pki/authorities/local/root.crt")
        context = ssl.create_default_context(cadata=root)
        connection = http.client.HTTPSConnection("localhost", port, timeout=5, context=context)
        try:
            connection.request("GET", SAFE_GET)
            response = connection.getresponse()
            body = response.read(65537)
            require(len(body) <= 65536 and response.status == 404, "CADDY_PROXIED_SAFE_GET")
            r2.safe_output(body.decode())
            require(json.loads(body) == direct["body"], "CADDY_NOT_API_RESPONSE")
        except ConnectionRefusedError:
            ports = json.loads(self.call("container", "inspect", "--format", "{{json .NetworkSettings.Ports}}", self.ids["caddy"]))
            print("CADDY_RUNTIME_PORTS " + json.dumps(ports, sort_keys=True), flush=True)
            raise r2.Failure("CADDY_LOOPBACK_CONNECTION_REFUSED") from None
        finally:
            connection.close()
        # Same loopback endpoint must fail with the ordinary trust store.
        untrusted = http.client.HTTPSConnection("localhost", port, timeout=5, context=ssl.create_default_context())
        try:
            untrusted.request("GET", SAFE_GET)
        except ssl.SSLCertVerificationError:
            pass
        else:
            raise r2.Failure("TLS_UNTRUSTED_CA_ACCEPTED")
        finally:
            untrusted.close()
        print("PASS Caddy proxied safe GET 404; local CA/hostname verified; untrusted CA rejected", flush=True)

    def empty_and_unchanged(self, catalog):
        for target, expected in self.histories.items():
            require(self.sql("smoke_" + target, HISTORY) == expected, "HISTORY_CHANGED")
        for table in self.tables:
            require(self.sql("smoke_business", f"SELECT count(*) FROM public.{table};") == "0", "BUSINESS_NOT_EMPTY")
        require(self.sql("smoke_crawler", "SELECT count(*) FROM listing_sources;") == "0", "CRAWLER_NOT_EMPTY")
        for entry in catalog["search"]:
            result = self.request("opensearch", 9200, "/" + entry["physical_baseline"] + "/_count")
            require(result["status"] == 200 and result["body"]["count"] == 0, "SEARCH_NOT_EMPTY")

    def run(self, pull_caddy):
        self.phase_start("immutable image and Compose gates")
        require(self.call("compose", "version", "--short").strip(), "COMPOSE_MISSING")
        for key, image in IMAGES.items():
            metadata = json.loads(self.call("image", "inspect", image))[0]
            self.image_envs[key] = dict(value.split("=", 1) for value in metadata["Config"].get("Env") or [])
            if key not in {"redis", "sequin"}:
                r2.verify_image(key, image, metadata, "opensearchproject/opensearch@" + IMAGES["opensearch"])
            else:
                require(metadata["Id"] == image and metadata["Os"] == "linux" and metadata["Architecture"] == "amd64", "IMAGE_PLATFORM")
                require(set(metadata["Config"].get("Volumes") or {}) <= ({"/data"} if key == "redis" else set()), "UNEXPECTED_IMAGE_VOLUME")
        if pull_caddy:
            self.call("pull", "--platform", "linux/amd64", CADDY, seconds=120)
            print("PASS exact public Caddy digest pull (120s bound)", flush=True)
        caddy = json.loads(self.call("image", "inspect", CADDY))[0]
        require(CADDY in caddy["RepoDigests"] and caddy["Os"] == "linux" and caddy["Architecture"] == "amd64", "CADDY_OFFICIAL_PIN")
        require(set(caddy["Config"].get("Volumes") or {}) <= {"/data", "/config"}, "UNEXPECTED_IMAGE_VOLUME")
        images = dict(IMAGES, caddy=caddy["Id"])
        self.images = images
        self.caddy_config = caddy["Config"]
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        self.port = port
        catalog = materialize(self.directory, self.projects, port, images, self.environments)
        self.fixture_inputs = fixture_hashes(self.directory)
        for kind in SERVICES:
            self.models[kind] = self.model(kind)
            found, volumes, nets = self.resources(kind)
            require(not found and not volumes and not nets, "PROJECT_ALREADY_EXISTS")
        self.mutated = True
        self.journal()
        self.initialize(catalog)
        before = self.stats()
        self.phase_start("all 13 applications plus Sequin and Caddy")
        self.compose("application", "up", "-d", "--no-build", "--pull", "never", seconds=120)
        self.apps_ready()
        self.empty_and_unchanged(catalog)
        self.compose("platform", "up", "-d", "--no-build", "--pull", "never", "sequin", seconds=120)
        self.sequin_ready(catalog)
        # Bind failure is a test failure. Never take over another listener.
        self.compose("edge", "up", "-d", "--no-build", "--pull", "never", seconds=60)
        self.apps_ready()
        self.witnesses(before)
        self.sequin_ready(catalog)
        platform, volumes, _ = self.refresh("platform", SERVICES["platform"])
        edge, edge_volumes, _ = self.refresh("edge", SERVICES["edge"])
        print("PASS apps/platform internal-only; only credential-free Caddy has edge egress, fixed local-CA/internal-upstream config", flush=True)
        self.tls_get(port)
        self.empty_and_unchanged(catalog)
        self.phase_start("application-only stop/start; retain platform and edge")
        self.compose("application", "stop", "--timeout", "340", seconds=360)
        stopped, _, _ = self.resources("application")
        require(set(stopped) == SERVICES["application"] and all(not item["State"]["Running"] and item["State"]["ExitCode"] == 0 for item in stopped.values()), "APP_STOP_NOT_CLEAN")
        require(self.sql("postgres", "SELECT count(*) FROM pg_stat_activity WHERE usename='smoke_runtime';") == "0", "APP_SESSIONS_REMAIN")
        for kind, original, old_volumes in (("platform", platform, volumes), ("edge", edge, edge_volumes)):
            now, new_volumes, _ = self.refresh(kind, SERVICES[kind])
            require({n: (v["Id"], v["State"]["StartedAt"]) for n, v in now.items()} ==
                    {n: (v["Id"], v["State"]["StartedAt"]) for n, v in original.items()} and new_volumes == old_volumes, "STATEFUL_IDENTITY_CHANGED")
        before = self.stats()
        self.compose("application", "start", seconds=60)
        self.apps_ready()
        self.witnesses(before)
        for kind, original, old_volumes in (("platform", platform, volumes), ("edge", edge, edge_volumes)):
            now, new_volumes, _ = self.refresh(kind, SERVICES[kind])
            require({n: (v["Id"], v["State"]["StartedAt"]) for n, v in now.items()} ==
                    {n: (v["Id"], v["State"]["StartedAt"]) for n, v in original.items()} and new_volumes == old_volumes, "STATEFUL_IDENTITY_CHANGED")
        self.sequin_ready(catalog)
        self.tls_get(port)
        self.empty_and_unchanged(catalog)
        self.stats()
        print("PASS all 13 restarted; platform/edge container IDs, start times and persisted volumes unchanged; empty stores", flush=True)

    def cleanup(self):
        require(not self.uncertain, "UNKNOWN_OUTCOME_RETAINED")
        if not self.mutated:
            return
        self.phase_start("exact owned cleanup")
        # Verify all projects before any cleanup, including no unknown network members.
        for kind in SERVICES:
            self.resources(kind)
        for kind in ("application", "edge", "platform"):
            self.compose(kind, "down", "--volumes", "--timeout", "45", seconds=120)
            found, volumes, nets = self.resources(kind)
            require(not found and not volumes and not nets, "CLEANUP_NOT_CONFIRMED_" + kind)
        print("PASS exact owned containers, volumes and network absent", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--pull-caddy", action="store_true")
    args = parser.parse_args()
    directory = Path(tempfile.mkdtemp(prefix="aura-r3-test-"))
    harness = Harness(directory)
    def interrupted(signum, _frame):
        raise r2.Failure("INTERRUPTED_" + str(signum))
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupted)
    passed = False
    try:
        harness.run(args.pull_caddy)
        passed = True
    except Exception as error:
        code = str(error) if isinstance(error, r2.Failure) else type(error).__name__
        frames = [(Path(frame.filename).name, frame.lineno) for frame in traceback.extract_tb(error.__traceback__)]
        print("FAIL phase=" + harness.phase + " code=" + code + " frames=" + json.dumps(frames), flush=True)
    finally:
        try:
            harness.cleanup()
            shutil.rmtree(directory)
        except Exception as error:
            passed = False
            harness.uncertain = True
            harness.journal()
            code = str(error) if isinstance(error, r2.Failure) else type(error).__name__
            print("RETAINED " + str(directory) + " projects=" + json.dumps(harness.projects) + " code=" + code, flush=True)
    print("PASS R3 empty-fixture Compose integration" if passed else "FAIL R3 (no acceptance claim)", flush=True)
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
