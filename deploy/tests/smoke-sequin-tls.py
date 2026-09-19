#!/usr/bin/env python3
"""Isolated Sequin 0.14.6 TLS acceptance; synthetic SQL/WAL, not worker custody.

Requires cached smoke-compose pins and --tls-image sha256:<64 lowercase hex>.
Uses platform + integrator's compose.sequin-tls.yml + sequin-tls.fixture.yml.
No pulls/builds. 540s work + at most 60s owned cleanup. No raw subprocess output.
Unknown command/ownership outcomes retain the 0700 fixture directory for review.
Source negatives require separate real Sequin SQL/WAL connection-failure logs;
missing/unrecognized evidence fails closed, never counts as a trust rejection.
"""
import argparse
import datetime
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid

sys.dont_write_bytecode = True
# Host-only pins/helpers. The webhook's /smoke mount has no repository ancestors.
ROOT = Path(__file__).resolve().parent.parent.parent
OWNER = "org.aura-historia.sequin-tls-test"
SERVICES = {"postgres", "redis", "sequin-postgres-tls", "sequin", "webhook"}
LIMIT = 1024 * 1024
SECRET = "tls-fixture-only-password"
CLOUD = dict(AWS_ACCESS_KEY_ID="", AWS_SECRET_ACCESS_KEY="", AWS_SESSION_TOKEN="", AWS_PROFILE="",
    AWS_CONFIG_FILE="/dev/null", AWS_SHARED_CREDENTIALS_FILE="/dev/null", AWS_EC2_METADATA_DISABLED="true",
    GOOGLE_APPLICATION_CREDENTIALS="/dev/null", GOOGLE_CLOUD_PROJECT="", GCP_PROJECT="",
    GCE_METADATA_HOST="127.0.0.1:9")
CLOUD_PREFIXES = ("AWS_", "GOOGLE_", "GCP_", "GCE_", "CLOUDSDK_", "AZURE_")
TLS_SERVICE = "sequin-postgres-tls"
# Empty PG_URL is truthy in Sequin's runtime.exs; the process must not inherit it.
SEQUIN_ENTRYPOINT = ["/usr/bin/env", "-u", "PG_URL", "/scripts/start_commands.sh"]
# Review and re-pin source changes, including comments. Never learn trust from current files.
# Exact bytes avoid interpreting Compose include/extends/env_file before rejecting changes.
COMPOSE_SOURCES = {
    "deploy/compose/compose.platform.yml": "08c96e0d3b459612bf3092a0930fa6f860b4ef2366a41891ec40baaa40e7a31b",
    "deploy/compose/compose.sequin-tls.yml": "1714c530ab99d6368bfc5494ebfd6a4e931177aebb086f6d4d4ac926dc69d1d2",
    "deploy/tests/sequin-tls.fixture.yml": "65743910517d48e991b17006177a7c6d5d50527761b6dbfb3fc60bc27728ccde",
}
SERVICE_FIELDS = {"image", "pull_policy", "restart", "labels", "networks", "network_mode",
    "security_opt", "logging", "stop_grace_period", "mem_limit", "cpus", "pids_limit", "environment",
    "command", "entrypoint", "volumes", "healthcheck", "depends_on", "user", "read_only", "cap_drop", "tmpfs"}
LOGGING = dict(driver="local", options={"max-size": "1m", "max-file": "1", "compress": "false"})
SECURITY = ["no-new-privileges:true"]
POSTGRES_ENV = dict(POSTGRES_USER="postgres", POSTGRES_PASSWORD=SECRET)
METADATA_ENV = dict(PG_DATABASE="tls_metadata", PG_USERNAME="tls_metadata", PG_PASSWORD=SECRET,
    PG_POOL_SIZE="3", REDIS_URL="redis://redis:6379", SERVER_PORT="7376", SERVER_HOST="sequin",
    SECRET_KEY_BASE="wDPLYus0pvD6qJhKJICO4vYl782Zjtpew5qRBDp7CZvbWtQmY0eB13If01234567",
    VAULT_KEY="2Sig69bIpuSm2kv0VQfDekET2qy8qUZGI8v3/h3ASiY=", TELEMETRY_ENABLED="false", CRASH_REPORTING_DISABLED="true")
COMMANDS = {"postgres": ["postgres", "-c", "config_file=/etc/postgresql/postgresql.conf"],
    "redis": ["redis-server", "/usr/local/etc/redis/redis.conf"],
    "webhook": ["/smoke/smoke-sequin-tls.py", "--serve"]}


class Failure(Exception):
    pass


def require(condition, code):
    if not condition:
        raise Failure(code)


def checked_bytes(path):
    require(path.resolve() == path and path.is_file(), "INPUT_PATH")
    with path.open("rb") as source:
        raw = source.read(65537)
    require(len(raw) <= 65536, "INPUT_SIZE")
    return raw


def validate_sources(files):
    require(files == [ROOT / name for name in COMPOSE_SOURCES], "COMPOSE_SOURCE_PATHS")
    for path, expected in zip(files, COMPOSE_SOURCES.values()):
        require(hashlib.sha256(checked_bytes(path)).hexdigest() == expected, "COMPOSE_SOURCE_NOT_REVIEWED")


def fixture_environment(service):
    env = dict(CLOUD)
    if service == "postgres":
        env.update(POSTGRES_ENV)
    elif service == "sequin":
        env.update(METADATA_ENV, SELF_HOSTED="1", SEQUIN_TELEMETRY_DISABLED="true", LOG_LEVEL="info",
            CONFIG_FILE_PATH="/run/sequin/sequin.yaml", ERL_FLAGS="+S 2:2",
            PG_HOSTNAME="127.0.0.1", PG_PORT="15432", PG_SSL="false", PG_URL="")
    return env


def image_id(value):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", value):
        raise argparse.ArgumentTypeError("full local sha256 image ID required (no tag/digest reference)")
    return value


def capture(argv, directory, seconds, body=None):
    """Bound stdout/stderr in memory while reading, not after communicate()."""
    require(seconds > 0, "DEADLINE")
    data = (body or "").encode()
    require(len(data) <= LIMIT, "INPUT_LIMIT")
    end, output = time.monotonic() + seconds, bytearray()
    try:
        with subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT, env={"PATH": "/usr/bin:/bin", "HOME": str(directory)},
                start_new_session=True) as proc:
            try:
                with selectors.DefaultSelector() as selector:
                    os.set_blocking(proc.stdout.fileno(), False)
                    os.set_blocking(proc.stdin.fileno(), False)
                    selector.register(proc.stdout, selectors.EVENT_READ)
                    if data:
                        selector.register(proc.stdin, selectors.EVENT_WRITE)
                    else:
                        proc.stdin.close()
                    while selector.get_map():
                        remaining = end - time.monotonic()
                        require(remaining > 0, "COMMAND_UNCONFIRMED")
                        for key, _ in selector.select(min(remaining, .2)):
                            if key.fileobj is proc.stdin:
                                try:
                                    data = data[os.write(proc.stdin.fileno(), data[:4096]):]
                                except BrokenPipeError:
                                    data = b""
                                if not data:
                                    selector.unregister(proc.stdin)
                                    proc.stdin.close()
                            else:
                                chunk = os.read(proc.stdout.fileno(), 16384)
                                output.extend(chunk)
                                require(len(output) <= LIMIT, "OUTPUT_LIMIT_UNCONFIRMED")
                                if not chunk:
                                    selector.unregister(proc.stdout)
                    code = proc.wait(timeout=max(.01, end - time.monotonic()))
                    return code, output.decode("utf-8", "replace")
            except BaseException:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=2)
                raise
    except (OSError, subprocess.TimeoutExpired):
        raise Failure("COMMAND_UNCONFIRMED") from None


def trust_rejected(log, listener, reason):
    # stunnel connection IDs tie accepted client, upstream connection and verify error.
    # A global startup/config error or unrelated listener must not satisfy this gate.
    connections = {}
    for line in log.splitlines():
        match = re.search(r"LOG\d+\[([^]]+)\]:\s*(.*)", line)
        if match and match[1] not in {"ui", "main"}:
            connections.setdefault(match[1], []).append(match[2].lower())
    for lines in connections.values():
        text = "\n".join(lines)
        if (f"service [{listener}] accepted connection from 127.0.0.1:" not in text
                or not re.search(r"s_connect: connected \S+:5432\b", text)):
            continue
        if reason == "ca" and ("certificate verify failed" in text or "certificate verification failed" in text
                or "certificate verify error" in text) and any(word in text for word in
                ("unable to get", "self-signed", "unknown ca", "unable to verify")):
            return True
        if reason == "hostname" and any(word in text for word in
                ("subject checks failed", "subject check failed", "hostname mismatch", "host name mismatch")):
            return True
        if reason == "no-ssl" and "postgresql server rejected tls" in lines:
            return True
    return False


def source_failure_events(log, database_id, replication_id):
    """Pinned Sequin/DBConnection signatures; SQL PIDs still need runtime attribution."""
    sql_pids, wal = set(), False
    for line in log.splitlines():
        # Postgrex omits endpoint text after TCP acceptance followed by handshake closure.
        match = re.search(r"\bPostgrex\.Protocol \(#PID(<0\.\d+\.\d+>)\) failed to connect: "
            r"\*\* \(DBConnection\.ConnectionError\) tcp recv \(idle\): closed(?:\s|$)", line)
        if match:
            sql_pids.add(match[1])
        if re.search(r"\[SlotProducer\] replication connect failed: tcp recv \(idle\): closed(?:\s|$)", line):
            wal |= all(re.search(r"(?:^|\s)" + key + "=" + re.escape(value) + r"(?:\s|$)", line)
                for key, value in (("database_id", database_id), ("replication_id", replication_id)))
    return sql_pids, wal


def source_config():
    return dict(account=dict(name="tls-test"), api_tokens=[dict(name="test", token="tls-fixture-token")],
        databases=[dict(name="source", hostname="127.0.0.1", port=15433, ssl=False,
            username="tls_source", password=SECRET, database="tls_source", pool_size=3,
            slot=dict(name="tls_slot", create_if_not_exists=False),
            publication=dict(name="tls_pub", create_if_not_exists=False))],
        http_endpoints=[dict(name="webhook", url="http://webhook:8081/cdc")],
        sinks=[dict(name="tls_sink", database="source", source=dict(include_tables=["public.tls_events"]),
            actions=["insert"], batch_size=1, initial_backfill=False, load_shedding_policy="pause_on_full",
            destination=dict(type="webhook", http_endpoint="webhook", batch=False))])


def stunnel_config(listener=None, reason=None):
    require(listener in (None, "metadata", "source") and reason in (None, "ca", "hostname"), "FAULT_INPUT")
    text = "foreground = yes\npid =\ndebug = info\nclient = yes\nsslVersionMin = TLSv1.2\n"
    for name, port in (("metadata", 15432), ("source", 15433)):
        ca = "wrong.pem" if (listener, reason) == (name, "ca") else "root.pem"
        host = "not-postgres.invalid" if (listener, reason) == (name, "hostname") else "postgres"
        text += (f"\n[{name}]\naccept = 127.0.0.1:{port}\nconnect = postgres:5432\nprotocol = pgsql\n"
                 f"CAfile = /run/aura/stunnel/{ca}\nverifyChain = yes\ncheckHost = {host}\nsni = postgres\n")
    return text


def owned_container(item, project, token):
    labels = item.get("Config", {}).get("Labels") or {}
    name = labels.get("com.docker.compose.service")
    require(name in SERVICES and labels.get(OWNER) == token
        and labels.get("com.docker.compose.project") == project
        and item.get("Name") == f"/{project}-{name}-1"
        and re.fullmatch(r"[0-9a-f]{64}", item.get("Id", "")), "CONTAINER_OWNERSHIP")
    return name


def owned_network(item, project, token, ids):
    labels = item.get("Labels") or {}
    require(item.get("Name") == project + "_backend" and item.get("Internal") is True
        and item.get("Driver") == "bridge" and item.get("Scope") == "local"
        and item.get("Options") in (None, {}) and item.get("IPAM", {}).get("Driver") == "default"
        and item.get("IPAM", {}).get("Options") in (None, {})
        and labels.get(OWNER) == token and labels.get("com.docker.compose.project") == project
        and labels.get("com.docker.compose.network") == "backend", "NETWORK_OWNERSHIP")
    require(set(item.get("Containers", {})) <= set(ids), "NETWORK_FOREIGN_MEMBER")


def binds(directory):
    return {"postgres": {str(directory / "postgres"): "/etc/postgresql"},
        "redis": {str(directory / "redis.conf"): "/usr/local/etc/redis/redis.conf"},
        "sequin": {str(directory / "sequin.yaml"): "/run/sequin/sequin.yaml"},
        TLS_SERVICE: {str(directory / TLS_SERVICE): "/run/aura/stunnel"},
        "webhook": {str(ROOT / "deploy/tests"): "/smoke"}}


def validate_model(model, directory, project, token, images):
    require(set(model) <= {"name", "services", "networks", "volumes", "x-platform", "x-cloud", "x-test"}, "MODEL_FIELDS")
    require(SERVICES <= model["services"].keys() <= SERVICES | {"opensearch"}, "MODEL_SERVICES")
    for name, svc in model["services"].items():
        require(set(svc) <= SERVICE_FIELDS | ({"profiles", "ulimits"} if name == "opensearch" else set()), "MODEL_SERVICE_FIELDS")
    require(not model.get("volumes") and set(model["networks"]) == {"backend"}, "MODEL_EXTRA_RESOURCES")
    network = model["networks"]["backend"]
    require(set(network) <= {"name", "internal", "labels", "ipam", "driver"}
        and network.get("driver") in (None, "bridge"), "MODEL_NETWORK_FIELDS")
    require(network.get("internal") is True and network.get("name") == project + "_backend"
        and not network.get("external") and not network.get("driver_opts")
        and not network.get("ipam") and network.get("labels", {}).get(OWNER) == token, "MODEL_NETWORK")
    for name in SERVICES:
        svc = model["services"][name]
        require(svc["image"] == images[name] and svc.get("pull_policy") == "never"
            and svc.get("labels", {}).get(OWNER) == token, "MODEL_IMAGE_OWNER")
        require(svc.get("logging") == LOGGING and svc.get("security_opt") == SECURITY, "MODEL_LOGGING_SECURITY")
        if name == "sequin":
            require(svc.get("entrypoint") == SEQUIN_ENTRYPOINT and svc.get("command") in (None, []), "SEQUIN_PG_URL_WRAPPER")
            require(svc.get("network_mode") == "service:" + TLS_SERVICE and not svc.get("networks"), "SEQUIN_NAMESPACE")
        else:
            require(svc.get("entrypoint") is None and svc.get("command") == COMMANDS.get(name), "MODEL_START_COMMAND")
            require(set(svc.get("networks", {})) == {"backend"} and not svc.get("network_mode"), "MODEL_NAMESPACE")
        mounts = svc.get("volumes", [])
        require(len(mounts) == len(binds(directory)[name]) and all(m.get("type") == "bind"
            and m.get("read_only") is True and m.get("bind", {}).get("create_host_path") is False
            for m in mounts) and {m["source"]: m["target"] for m in mounts} == binds(directory)[name], "MODEL_BINDS")
        require(svc.get("restart") == ("no" if name == "webhook" else "unless-stopped"), "MODEL_RESTART")
        require(svc.get("environment", {}) == fixture_environment(name), "MODEL_FIXTURE_ENV")
    tls = model["services"][TLS_SERVICE]
    require(tls.get("user") in ("10001", "10001:10001") and tls.get("read_only") is True
        and tls.get("cap_drop") == ["ALL"] and not tls.get("tmpfs"), "TLS_HARDENING")
    require("sequin" in tls["networks"]["backend"].get("aliases", []), "TLS_DNS_ALIAS")
    env = model["services"]["sequin"]["environment"]
    require(all(env.get(k) == v for k, v in dict(PG_HOSTNAME="127.0.0.1", PG_PORT="15432",
        PG_SSL="false", PG_URL="").items()), "METADATA_TLS_ROUTE")


class Probe:
    def __init__(self, directory, tls_image):
        global r3
        spec = importlib.util.spec_from_file_location("tls_r3", Path(__file__).with_name("smoke-compose.py"))
        r3 = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(r3)
        image_id(tls_image)
        self.directory = directory
        self.token = uuid.uuid4().hex
        self.project = "aura-sequin-tls-" + self.token
        self.deadline = time.monotonic() + 540
        self.uncertain = False
        self.ids = {}
        self.images = {name: r3.IMAGES[{"webhook": "helper"}.get(name, name)]
                       for name in SERVICES - {TLS_SERVICE}}
        self.images[TLS_SERVICE] = tls_image
        self.files = [ROOT / name for name in COMPOSE_SOURCES]
        self.env_inputs = {}
        self.image_envs = {}
        self.model = None
        (directory / "docker").mkdir(mode=0o700)
        (directory / "ownership.json").write_text(json.dumps(dict(project=self.project, label=OWNER, owner=self.token)))
        (directory / "ownership.json").chmod(0o600)
        print("FIXTURES " + str(directory), flush=True)
        print("PROJECT " + self.project, flush=True)

    def call(self, *args, seconds=20, body=None, check=True, mutating=None):
        if mutating is None:
            mutating = not (args[0] in {"exec", "logs"} or args[:2] in {
                ("container", "ls"), ("container", "inspect"), ("network", "ls"), ("network", "inspect"),
                ("volume", "ls"), ("image", "inspect")})
        try:
            code, out = capture(["/usr/bin/docker", "--host", "unix:///var/run/docker.sock", "--config",
                str(self.directory / "docker"), *args], self.directory,
                min(seconds, self.deadline - time.monotonic()), body)
        except BaseException:
            self.uncertain = True
            raise
        # A failed daemon mutation may have partially applied. Never infer safety from its text.
        if code and mutating:
            self.uncertain = True
        require(not check or code == 0, "DOCKER_FAILED_" + args[0])
        return out if check else (code, out)

    def compose(self, *args, seconds=25):
        validate_sources(self.files)
        require(set(self.env_inputs) == {"compose.env", "postgres.env", "sequin.env"}, "ENV_INPUTS_UNSET")
        for name, expected in self.env_inputs.items():
            require(checked_bytes(self.directory / name) == expected, "ENV_INPUT_CHANGED")
        files = [arg for path in self.files for arg in ("-f", str(path))]
        return self.call("compose", "--project-name", self.project, "--env-file",
            str(self.directory / "compose.env"), *files, *args, seconds=seconds, mutating=args[0] != "config")

    def write(self, name, text):
        # Replace public synthetic fixtures atomically; existing files are 0444.
        r3.protected_write(self.directory, name + ".new", text)
        os.replace(self.directory / (name + ".new"), self.directory / name)

    def prepare(self):
        validate_sources(self.files)
        for name, image in self.images.items():
            item = json.loads(self.call("image", "inspect", image))[0]
            require(item["Id"] == image, "IMAGE_IDENTITY")
            env = dict(entry.split("=", 1) for entry in item["Config"].get("Env", []))
            self.image_envs[name] = env
            require(all(k in CLOUD for k in env if k.startswith(CLOUD_PREFIXES)), "IMAGE_CLOUD_DEFAULT")
            if name == TLS_SERVICE:
                require(item["Config"].get("Entrypoint") == ["/usr/local/bin/stunnel", "/run/aura/stunnel/stunnel.conf"]
                    and not item["Config"].get("Cmd") and item["Config"].get("User") in ("10001", "10001:10001"), "TLS_IMAGE_CONTRACT")
        self.write("postgres.env", r3.env_text(POSTGRES_ENV))
        self.write("postgres/postgresql.conf", "listen_addresses='*'\nwal_level=logical\nmax_replication_slots=4\nmax_wal_senders=4\nssl=off\nlog_connections=on\nhba_file='/etc/postgresql/pg_hba.conf'\n")
        # Allow plaintext at the server: ssl=off case must catch client fallback, not rely on HBA rejection.
        self.write("postgres/pg_hba.conf", "local all all trust\nhost all all all scram-sha-256\nhost replication all all scram-sha-256\n")
        (self.directory / "postgres").chmod(0o755)
        self.write("redis.conf", "bind 0.0.0.0\nprotected-mode no\nappendonly no\nsave \"\"\n")
        self.write("sequin.env", r3.env_text(METADATA_ENV))
        self.write("sequin.yaml", json.dumps(source_config()))
        self.write(TLS_SERVICE + "/stunnel.conf", stunnel_config())
        (self.directory / TLS_SERVICE).chmod(0o755)
        self.certificates()
        env = {k.upper() + "_IMAGE": v for k, v in r3.IMAGES.items()}
        env.update(SEQUIN_POSTGRES_TLS_IMAGE=self.images[TLS_SERVICE], AURA_CONFIG_DIR=str(self.directory),
            AURA_NETWORK=self.project + "_backend", AURA_NETWORK_INTERNAL="true", TLS_TEST_OWNER=self.token)
        self.write("compose.env", r3.env_text(env))
        self.env_inputs = {name: r3.env_text(values).encode() for name, values in
            (("compose.env", env), ("postgres.env", POSTGRES_ENV), ("sequin.env", METADATA_ENV))}
        self.model = json.loads(self.compose("config", "--format", "json"))
        validate_model(self.model, self.directory, self.project, self.token, self.images)

    def certificates(self):
        def openssl(*args):
            code, _ = capture(["/usr/bin/openssl", *args], self.directory,
                min(15, self.deadline - time.monotonic()))
            require(code == 0, "CERT_FIXTURE_FAILED")
        for name in ("root", "wrong"):
            openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=tls-" + name,
                "-addext", "basicConstraints=critical,CA:TRUE", "-keyout", str(self.directory / (name + ".key")),
                "-out", str(self.directory / TLS_SERVICE / (name + ".pem")))
        openssl("req", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=postgres", "-keyout",
            str(self.directory / "server.key"), "-out", str(self.directory / "server.csr"))
        self.write("server.ext", "subjectAltName=DNS:postgres\nextendedKeyUsage=serverAuth\n")
        openssl("x509", "-req", "-in", str(self.directory / "server.csr"), "-CA",
            str(self.directory / TLS_SERVICE / "root.pem"), "-CAkey", str(self.directory / "root.key"),
            "-set_serial", "1", "-days", "1", "-extfile", str(self.directory / "server.ext"), "-out", str(self.directory / "server.pem"))
        for path in (self.directory / TLS_SERVICE).glob("*.pem"):
            path.chmod(0o444)

    def resources(self):
        found = self.call("container", "ls", "-aq", "--no-trunc", "--filter", "label=com.docker.compose.project=" + self.project).split()
        items = json.loads(self.call("container", "inspect", *found)) if found else []
        ids = {}
        for item in items:
            name = owned_container(item, self.project, self.token)
            require(name not in ids and item["Image"] == self.images[name], "RUNTIME_IDENTITY")
            if name == "sequin":
                require(item["Config"].get("Entrypoint") == SEQUIN_ENTRYPOINT
                    and item["Config"].get("Cmd") in (None, []), "RUNTIME_SEQUIN_PG_URL_WRAPPER")
            ids[name] = item["Id"]
            host = item["HostConfig"]
            require(not host.get("PortBindings") and not host.get("Privileged") and not host.get("CapAdd")
                and not host.get("Devices"), "RUNTIME_ISOLATION")
            require(host.get("SecurityOpt") == SECURITY and host.get("LogConfig") ==
                dict(Type=LOGGING["driver"], Config=LOGGING["options"]), "RUNTIME_LOGGING_SECURITY")
            mounts = [m for m in item["Mounts"] if m["Type"] != "tmpfs"]
            require(len(mounts) == len(binds(self.directory)[name]) and all(m["Type"] == "bind" and not m["RW"] for m in mounts)
                and {m["Source"]: m["Destination"] for m in mounts} == binds(self.directory)[name], "RUNTIME_MOUNTS")
            env = dict(e.split("=", 1) for e in item["Config"].get("Env", []))
            require(env == self.image_envs[name] | fixture_environment(name), "RUNTIME_FIXTURE_ENV")
            if name == TLS_SERVICE:
                require(host.get("ReadonlyRootfs") is True and host.get("CapDrop") == ["ALL"]
                    and not host.get("Tmpfs") and item["Config"].get("User") in ("10001", "10001:10001")
                    and item["Config"].get("Entrypoint") == ["/usr/local/bin/stunnel", "/run/aura/stunnel/stunnel.conf"], "RUNTIME_TLS_HARDENING")
        for item in items:
            name = owned_container(item, self.project, self.token)
            mode = item["HostConfig"]["NetworkMode"]
            require(mode == ("container:" + ids[TLS_SERVICE] if name == "sequin" else self.project + "_backend"), "RUNTIME_NAMESPACE")
            require(set(item["NetworkSettings"]["Networks"]) <= {self.project + "_backend"}, "RUNTIME_EXTRA_NETWORK")
        networks = self.call("network", "ls", "-q", "--no-trunc", "--filter", "label=com.docker.compose.project=" + self.project).split()
        require(len(networks) <= 1, "EXTRA_NETWORK")
        for net in networks:
            owned_network(json.loads(self.call("network", "inspect", net))[0], self.project, self.token, ids.values())
        require(not self.call("volume", "ls", "-q", "--filter", "label=com.docker.compose.project=" + self.project).strip(), "UNEXPECTED_VOLUME")
        self.ids = ids
        return networks

    def up(self, *services):
        require(set(services) <= SERVICES, "START_SCOPE")
        require(json.loads(self.compose("config", "--format", "json")) == self.model, "MODEL_CHANGED")
        self.compose("up", "-d", "--no-deps", "--no-build", "--pull", "never", *services)
        self.resources()

    def sql(self, database, query):
        return self.call("exec", "-i", self.ids["postgres"], "psql", "-X", "-U", "postgres", "-d", database,
            "-v", "ON_ERROR_STOP=1", "-At", body="SET statement_timeout='4s';\n" + query, seconds=8).removeprefix("SET\n").strip()

    def wait(self, check, seconds, code):
        end = min(self.deadline, time.monotonic() + seconds)
        while time.monotonic() < end:
            if check():
                return
            time.sleep(1)
        raise Failure(code)

    def logs(self, service, since):
        return self.call("logs", "--since", since, "--tail", "1200", self.ids[service])

    def python(self, code, *args):
        return self.call("exec", self.ids["webhook"], "python3", "-c", code, *args, seconds=10).strip()

    def state(self):
        return json.loads(self.python("import urllib.request; print(urllib.request.urlopen('http://127.0.0.1:8081/state',timeout=3).read(8192).decode())"))

    def health(self):
        return self.python("import http.client\nc=http.client.HTTPConnection('sequin',7376,timeout=3)\ntry:\n c.request('GET','/health'); r=c.getresponse(); print(r.status)\nexcept OSError: print(0)\nfinally: c.close()") == "200"

    def tls_sessions(self):
        return json.loads(self.sql("postgres", "SELECT coalesce(json_agg(t),'[]') FROM (SELECT a.usename,a.backend_type,s.ssl,s.version FROM pg_stat_activity a JOIN pg_stat_ssl s USING(pid) WHERE a.usename IN ('tls_metadata','tls_source')) t;"))

    def healthy(self):
        rows = self.tls_sessions()
        require(all(r["ssl"] and r["version"] in ("TLSv1.2", "TLSv1.3") for r in rows), "PLAINTEXT_SESSION")
        expected = {("tls_metadata", "client backend"), ("tls_source", "client backend"), ("tls_source", "walsender")}
        return (expected <= {(r["usename"], r["backend_type"]) for r in rows} and self.health()
            and self.sql("tls_source", "SELECT active FROM pg_replication_slots WHERE slot_name='tls_slot';") == "t")

    def slot(self):
        return self.sql("tls_source", "SELECT slot_name||':'||plugin||':'||database FROM pg_replication_slots WHERE slot_name='tls_slot';")

    def delivered(self, event):
        state = self.state()
        require(not state["invalid"] and 0 not in state["ids"], "WEBHOOK_OR_BACKFILL")
        return event in state["ids"]

    def reload(self, listener=None, reason=None):
        self.write(TLS_SERVICE + "/stunnel.conf", stunnel_config(listener, reason))
        since = datetime.datetime.now(datetime.timezone.utc).isoformat()
        self.call("kill", "--signal", "HUP", self.ids[TLS_SERVICE])
        self.wait(lambda: "Configuration successful" in self.logs(TLS_SERVICE, since), 10, "STUNNEL_RELOAD")
        return since

    def disconnect(self, role):
        require(role in ("tls_source", "tls_metadata"), "ROLE_SCOPE")
        self.sql("postgres", f"SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename='{role}';")

    def source_sql_failed(self, pids):
        # Stock release RPC reads only DBConnection state; never emit opts (credentials).
        require(len(pids) <= 16 and all(re.fullmatch(r"<0\.\d+\.\d+>", p) for p in pids), "SQL_PID_INPUT")
        if not pids:
            return False
        code = """ok = Enum.any?(PIDS, fn text ->
          try do
            {_, s} = :sys.get_state(:erlang.list_to_pid(String.to_charlist(text)), 500)
            s.mod == Postgrex.Protocol and is_pid(s.pool) and
              Enum.all?([hostname: "127.0.0.1", port: 15433, database: "tls_source",
                username: "tls_source", ssl: false], fn {k, v} -> Keyword.get(s.opts, k) == v end)
          rescue
            _ -> false
          catch
            _, _ -> false
          end
        end)
        IO.puts(if ok, do: "TLS_SOURCE_SQL_CONFIRMED", else: "TLS_SOURCE_SQL_UNCONFIRMED")
        """.replace("PIDS", json.dumps(sorted(pids)))
        output = self.call("exec", self.ids["sequin"], "sequin-server", "rpc", code, seconds=15)
        return output.strip() == "TLS_SOURCE_SQL_CONFIRMED"

    def restore(self, event):
        self.reload()
        self.wait(self.healthy, 55, "RESTORE_SQL_WAL_TLS")
        self.wait(lambda: self.delivered(event), 35, "COMMITTED_DELIVERY")
        require(self.slot() == "tls_slot:pgoutput:tls_source", "SLOT_CHANGED")
        require(self.sql("tls_metadata", "SELECT count(*) FROM sequin_config.backfills;") == "0", "BACKFILL_CREATED")
        require(self.sql("tls_metadata", "SELECT id FROM sequin_config.sink_consumers WHERE name='tls_sink';") == self.sink_id, "SINK_CHANGED")

    def run(self):
        self.prepare()
        self.up("postgres", "redis", "webhook")
        self.wait(lambda: self.call("exec", self.ids["postgres"], "pg_isready", "-h", "127.0.0.1", "-U", "postgres", check=False)[0] == 0, 40, "POSTGRES_START")
        for name in ("server.key", "server.pem"):
            self.call("exec", "-i", "--user", "postgres", self.ids["postgres"], "sh", "-c",
                "umask 077; cat > /var/lib/postgresql/data/" + name, body=(self.directory / name).read_text())
        for key, value in (("ssl_cert_file", "/var/lib/postgresql/data/server.pem"),
                ("ssl_key_file", "/var/lib/postgresql/data/server.key"), ("ssl", "on")):
            self.sql("postgres", f"ALTER SYSTEM SET {key}='{value}';")
        self.sql("postgres", "SELECT pg_reload_conf();")
        self.wait(lambda: self.sql("postgres", "SHOW ssl;") == "on", 10, "POSTGRES_SSL_ON")
        self.sql("postgres", f"CREATE ROLE tls_metadata LOGIN PASSWORD '{SECRET}'; CREATE ROLE tls_source LOGIN REPLICATION PASSWORD '{SECRET}';")
        for name in ("tls_metadata", "tls_source"):
            self.sql("postgres", f"CREATE DATABASE {name} OWNER {name};")
        self.sql("tls_source", "CREATE TABLE tls_events(id integer PRIMARY KEY, marker text NOT NULL); INSERT INTO tls_events VALUES(0,'before-slot'); GRANT SELECT ON tls_events TO tls_source; CREATE PUBLICATION tls_pub FOR TABLE tls_events; SELECT pg_create_logical_replication_slot('tls_slot','pgoutput');")
        started = datetime.datetime.now(datetime.timezone.utc).isoformat()
        self.up(TLS_SERVICE)
        self.wait(lambda: "Configuration successful" in self.logs(TLS_SERVICE, started), 10, "STUNNEL_START")
        # Cold metadata negatives: actual release entrypoint must not migrate through failed trust.
        for reason in ("ca", "hostname"):
            since = self.reload("metadata", reason)
            self.up("sequin")
            self.wait(lambda: trust_rejected(self.logs(TLS_SERVICE, since), "metadata", reason), 45, "METADATA_TRUST_ATTEMPT_" + reason)
            require(not self.tls_sessions(), "BAD_METADATA_CONNECTED")
            require(self.sql("tls_metadata", "SELECT count(*) FROM pg_namespace WHERE nspname='sequin_config';") == "0", "BAD_METADATA_MIGRATED")
            self.call("stop", "--time", "5", self.ids["sequin"], seconds=10)
            require(not json.loads(self.call("container", "inspect", self.ids["sequin"]))[0]["State"]["Running"], "SEQUIN_STOP_UNCONFIRMED")
            print("PASS metadata " + reason + " rejected before migration", flush=True)
        self.reload()
        self.up("sequin")
        self.wait(self.healthy, 100, "GOOD_METADATA_SOURCE_SQL_WAL")
        schemas = self.sql("tls_metadata", "SELECT schemaname FROM pg_tables WHERE tablename='schema_migrations' ORDER BY schemaname;").splitlines()
        require(schemas and all(re.fullmatch(r"[a-z_]+", s) for s in schemas), "METADATA_MIGRATION_TABLES")
        require(all(int(self.sql("tls_metadata", f"SELECT count(*) FROM {s}.schema_migrations;")) > 0
                    for s in schemas), "METADATA_MIGRATIONS")
        self.database_id, self.replication_id = self.sql("tls_metadata", "SELECT d.id||':'||r.id FROM sequin_config.postgres_databases d JOIN sequin_config.postgres_replication_slots r ON r.postgres_database_id=d.id WHERE d.name='source' AND d.hostname='127.0.0.1' AND d.port=15433 AND r.slot_name='tls_slot';").split(":")
        require(all(re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", v)
            for v in (self.database_id, self.replication_id)), "SOURCE_IDENTITY")
        self.sink_id = self.sql("tls_metadata", "SELECT id FROM sequin_config.sink_consumers WHERE name='tls_sink';")
        require(re.fullmatch(r"[0-9a-f-]{36}", self.sink_id), "REAL_SINK")
        self.sql("tls_source", "BEGIN; INSERT INTO tls_events VALUES(1,'committed'); COMMIT;")
        self.wait(lambda: self.delivered(1), 35, "FIRST_DELIVERY")
        print("PASS real metadata migrations, source SQL+WAL pg_stat_ssl, committed delivery", flush=True)
        # Webhook is a separate backend container. DNS/API must work before testing refused loopback ports.
        require(self.health(), "BACKEND_DNS_CONTROL")
        require(self.python("import errno,socket\nfor p in (15432,15433):\n s=socket.socket(); s.settimeout(2)\n try:\n  assert s.connect_ex(('sequin',p)) == errno.ECONNREFUSED\n finally: s.close()\nprint('isolated')") == "isolated", "PLAINTEXT_PORT_EXPOSED")
        print("PASS loopback plaintext ports refused from separate backend container", flush=True)
        for event, reason in enumerate(("ca", "hostname", "no-ssl"), start=2):
            if reason == "no-ssl":
                self.sql("postgres", "ALTER SYSTEM SET ssl='off';")
                self.sql("postgres", "SELECT pg_reload_conf();")
                self.wait(lambda: self.sql("postgres", "SHOW ssl;") == "off", 10, "POSTGRES_SSL_OFF")
                since = self.reload()
            else:
                since = self.reload("source", reason)
            self.disconnect("tls_source")
            self.sql("tls_source", f"BEGIN; INSERT INTO tls_events VALUES({event},'committed'); COMMIT;")
            evidence = {}
            def rejected():
                rows = self.tls_sessions()
                require(all(r["ssl"] for r in rows), "PLAINTEXT_FALLBACK")
                metadata = any(r["usename"] == "tls_metadata" for r in rows)
                source = any(r["usename"] == "tls_source" for r in rows)
                require(not self.delivered(event), "DELIVERY_DURING_FAILED_TRUST")
                # Also catch short-lived plaintext/SQL connections missed by pg_stat_ssl polling.
                require("connection authorized: user=tls_source" not in self.logs("postgres", since), "SOURCE_AUTHORIZED_DURING_FAULT")
                pids, wal = source_failure_events(self.logs("sequin", since), self.database_id, self.replication_id)
                evidence.update(metadata_tls=metadata, source_absent=not source, health=self.health(),
                    trust_rejected=trust_rejected(self.logs(TLS_SERVICE, since), "source", reason),
                    wal_failure=wal, sql_failure=self.source_sql_failed(pids),
                    slot_inactive=self.sql("tls_source", "SELECT active FROM pg_replication_slots WHERE slot_name='tls_slot';") == "f")
                return all(evidence.values())
            try:
                self.wait(rejected, 55, "SOURCE_SQL_WAL_REJECTION_EVIDENCE_" + reason)
            except Failure:
                print("EVIDENCE " + json.dumps(evidence, sort_keys=True), flush=True)
                raise
            if reason == "no-ssl":
                self.sql("postgres", "ALTER SYSTEM SET ssl='on';")
                self.sql("postgres", "SELECT pg_reload_conf();")
                self.wait(lambda: self.sql("postgres", "SHOW ssl;") == "on", 10, "POSTGRES_SSL_RESTORED")
            self.restore(event)
            print("PASS source " + reason + ": SQL/WAL attempts rejected, metadata healthy, same slot reconnect, no backfill", flush=True)
        self.resources()

    def cleanup(self):
        require(not self.uncertain, "UNKNOWN_OUTCOME_RETAINED")
        self.deadline = time.monotonic() + 60
        try:
            networks = self.resources()
            for name in ("sequin", TLS_SERVICE, "webhook", "redis", "postgres"):
                if name in self.ids:
                    identifier = self.ids[name]
                    item = json.loads(self.call("container", "inspect", identifier))[0]
                    require(owned_container(item, self.project, self.token) == name, "CLEANUP_OWNER")
                    self.call("rm", "--force", identifier, seconds=10)
                    require(not self.call("container", "ls", "-aq", "--no-trunc", "--filter", "id=" + identifier).strip(), "CLEANUP_UNCONFIRMED")
            for net in networks:
                owned_network(json.loads(self.call("network", "inspect", net))[0], self.project, self.token, [])
                self.call("network", "rm", net)
                require(not self.call("network", "ls", "-q", "--filter", "id=" + net).strip(), "NETWORK_CLEANUP_UNCONFIRMED")
            require(not self.resources(), "RESOURCES_REMAIN")
        except BaseException:
            self.uncertain = True
            raise
        shutil.rmtree(self.directory)


class Webhook(http.server.BaseHTTPRequestHandler):
    ids = set()
    invalid = 0

    def log_message(self, *_args):
        pass

    def setup(self):
        super().setup()
        self.connection.settimeout(3)

    def reply(self, status, value):
        raw = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        self.reply(200 if self.path == "/state" else 404, dict(ids=sorted(self.ids), invalid=self.invalid))

    def do_POST(self):
        try:
            size = int(self.headers.get("Content-Length", "0"))
            require(self.path == "/cdc" and not self.headers.get("Transfer-Encoding") and 0 < size <= 16384, "BODY")
            body = json.loads(self.rfile.read(size))
            # Native batch:false event envelope. Never retain/print arbitrary request bodies.
            record = body["record"]
            event = record["id"]
            require(body["action"] == "insert" and type(event) is int and 0 <= event <= 4
                and record["marker"] == ("before-slot" if event == 0 else "committed")
                and body["metadata"]["table_schema"] == "public"
                and body["metadata"]["table_name"] == "tls_events", "EVENT")
            self.ids.add(event)
            self.reply(200, {})
        except (Failure, KeyError, TypeError, ValueError, OSError):
            Webhook.invalid = min(1000, Webhook.invalid + 1)
            try:
                self.reply(400, {})
            except OSError:
                pass


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tls-image", required=True, type=image_id)
    args = parser.parse_args(argv)
    os.umask(0o077)
    directory = Path(tempfile.mkdtemp(prefix="aura-sequin-tls-"))
    probe = Probe(directory, args.tls_image)
    def interrupted(_signum, _frame):
        probe.uncertain = True
        raise Failure("INTERRUPTED_RETAINED")
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupted)
    passed = False
    try:
        probe.run()
        passed = True
    except Failure as error:
        print("FAIL " + str(error), flush=True)
    except Exception:
        # No traceback: library exceptions can include passwords/response bodies.
        print("FAIL UNEXPECTED_INTERNAL_ERROR", flush=True)
    try:
        probe.cleanup()
        print("CLEANUP confirmed", flush=True)
    except BaseException:
        print("RETAINED " + str(directory) + "; ownership.json identifies scope; manual review required", flush=True)
        passed = False
    if passed:
        print("PASS isolated Sequin TLS (not production/worker-custody acceptance)", flush=True)
    return 0 if passed else 1


if __name__ == "__main__":
    if sys.argv[1:] == ["--serve"]:
        class Server(http.server.HTTPServer):
            def handle_error(self, *_args):
                Webhook.invalid = min(1000, Webhook.invalid + 1)
        Server(("0.0.0.0", 8081), Webhook).serve_forever()
    else:
        sys.exit(main())
