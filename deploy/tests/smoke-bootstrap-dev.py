#!/usr/bin/env python3
"""Real isolated fresh-dev Compose acceptance (not live-dev approval).

Requires --image sha256:<local ID> --source-sha <actual local commit> and cached
PG16/pg_ttl_index3 below, Docker Compose with raw env_file support, and openssl.
No pulls/builds, published ports, provider credentials or host Docker config.
600s work + 90s cleanup. Unknown Docker/application outcomes retain the private
fixture and ownership.json for manual inspection; never retry or prune.
Logical pg_dump snapshots cover schemas/data/history and cluster globals, not
physical/WAL equality, TTL health, full DDL drift or cryptographic provenance.
The source label is checked; embedded SQL checksums must match checkout AND commit.
"""
import argparse
import datetime
import hashlib
import importlib.util
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import sys
import tempfile
import time
import uuid

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("bootstrap_capture", Path(__file__).with_name("smoke-sequin-tls.py"))
helpers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helpers)
capture, require, Failure, image_id = helpers.capture, helpers.require, helpers.Failure, helpers.image_id
ROOT = Path(__file__).resolve().parents[2]
COMPOSE = ROOT / "deploy/compose/compose.bootstrap-dev.yml"
PG_IMAGE = "sha256:1ef4f65fa354b5771def2872dc765c5cafb5c3f1e56ce1d394a6ad4af33279be"
OWNER = "org.aura-historia.bootstrap-dev-test"
TARGETS = ("business", "crawler")
KEYS = dict(business="BUSINESS_DATABASE_URL", crawler="LOCAL_DB_URL")
ENTRYPOINT = ["/usr/local/bin/bootstrap-dev"]
CA = "/run/aura/postgres-ca.pem"
# Frozen review pin, checked BEFORE Compose can follow include/extends/env_file.
COMPOSE_SHA256 = "eb81ac913880e32d47812200957064b28829197c88040d0b1a3e8366bf54d8f7"
LOGGING = dict(driver="local", options={"max-size": "1m", "max-file": "1", "compress": "false"})
FIELDS = {"image", "pull_policy", "restart", "read_only", "user", "cap_drop", "security_opt",
    "networks", "mem_limit", "cpus", "pids_limit", "stop_grace_period", "command", "environment",
    "volumes", "logging", "labels", "container_name", "entrypoint"}
RUNTIME_CHECK = 'if command -v docker >/dev/null; then exit 1; fi; id -u | grep -qx 10001; test ! -e /var/run/docker.sock; test ! -e /run/docker.sock; echo RUNTIME_NO_DOCKER'


def source_sha(value):
    if not re.fullmatch(r"[0-9a-f]{40}", value) or len(set(value)) == 1:
        raise argparse.ArgumentTypeError("actual 40 lowercase hex local source commit required")
    return value


def source_guard():
    require(hashlib.sha256(helpers.checked_bytes(COMPOSE)).hexdigest() == COMPOSE_SHA256, "COMPOSE_SOURCE_NOT_REVIEWED")


def env_text(values):
    return "".join(f"{key}={value}\n" for key, value in values.items())


def logical_dump(raw):
    # Recent PG16 security releases randomize only these paired psql guard lines.
    return "\n".join(line for line in raw.splitlines()
        if not re.fullmatch(r"\\(?:un)?restrict [A-Za-z0-9]+", line))


def tls_evidence(raw, target):
    sessions = [line for line in raw.splitlines() if "connection authorized:" in line
        and "application_name=crawler-bootstrap-dev" in line]
    databases = re.findall(r"connection authorized: user=(?:postgres|smoke_readonly) "
        r"database=(smoke_business|smoke_crawler) application_name=crawler-bootstrap-dev "
        r"SSL enabled \(protocol=TLSv1\.[23], cipher=[A-Za-z0-9_-]+, bits=\d+\)", raw)
    return bool(databases) and len(databases) == len(sessions) and set(databases) == {"smoke_" + target}


def connection_attempts(raw, peer):
    """Only messages after TCP receipt from this owned one-shot, on the same PG PID."""
    attempts = {}
    for line in raw.splitlines():
        match = re.fullmatch(r".* \[(\d+)\] (LOG|FATAL|DETAIL|ERROR):\s+(.*)", line)
        if not match:
            continue
        pid, level, message = match.groups()
        if level == "LOG" and re.fullmatch(r"connection received: host=" + re.escape(peer) + r" port=\d+", message):
            attempts[pid] = []
        elif pid in attempts:
            attempts[pid].append(level + ": " + message)
    return list(attempts.values())


def rejection_evidence(raw, peer, reason):
    attempts = connection_attempts(raw, peer)
    if not attempts or any("connection authenticated:" in m or "connection authorized:" in m for a in attempts for m in a):
        return False
    patterns = {
        "wrong-ca": r"LOG: could not accept SSL connection: (?:tlsv1|sslv3) alert unknown ca",
        "wrong-hostname": r"LOG: could not accept SSL connection: (?:tlsv1|sslv3) alert bad certificate",
        "wrong-password": r'FATAL: password authentication failed for user "postgres"',
    }
    if reason == "plaintext":
        # Scoped policy evidence, not an invented server TLS error: SSL is disabled,
        # authenticated plaintext control passes, TCP arrives, client exits before
        # the shared five-second connect deadline without attempting authentication.
        return not any(m.startswith(("FATAL:", "ERROR:")) or "SSL connection:" in m for a in attempts for m in a)
    return reason in patterns and all(any(re.fullmatch(patterns[reason], m) for m in a) for a in attempts)


def fixture_peers(network):
    configs = network.get("IPAM", {}).get("Config", [])
    require(len(configs) == 1 and set(configs[0]) <= {"Subnet", "Gateway"}, "FIXTURE_SUBNET")
    subnet = ipaddress.ip_network(configs[0]["Subnet"])
    require(subnet.version == 4 and subnet.num_addresses >= 8, "FIXTURE_SUBNET")
    peers = {target: str(subnet[-2 - index]) for index, target in enumerate(TARGETS)}
    require(configs[0].get("Gateway") not in peers.values(), "FIXTURE_PEER_GATEWAY")
    return peers


def validate_model(model, directory, project, token, image, environments, commands, ca_value, peers, shell_target=None):
    require(set(model) <= {"name", "services", "networks", "x-bootstrap"}
        and model.get("name") == project, "MODEL_FIELDS")
    network = {"name": project, "external": True}
    require(model.get("networks") in ({"backend": network}, {"backend": network | {"ipam": {}}}), "MODEL_NETWORK")
    require(set(model.get("services", {})) == {"bootstrap-" + t for t in TARGETS}, "MODEL_SERVICES")
    for target in TARGETS:
        svc = model["services"]["bootstrap-" + target]
        require(set(svc) <= FIELDS, "MODEL_UNSAFE_FIELD")
        require(svc.get("entrypoint") == (["/bin/sh"] if target == shell_target else None), "MODEL_ENTRYPOINT")
        if target == shell_target:
            require(commands[target] == ["-ec", RUNTIME_CHECK], "MODEL_SHELL_CHECK")
        require(svc.get("image") == image and svc.get("pull_policy") == "never"
            and svc.get("restart") == "no" and svc.get("command") == commands[target], "MODEL_COMMAND")
        require(svc.get("user") == "10001:10001" and svc.get("read_only") is True
            and svc.get("cap_drop") == ["ALL"] and svc.get("security_opt") == ["no-new-privileges:true"], "MODEL_SECURITY")
        require(svc.get("mem_limit") in (536870912, "536870912") and float(svc.get("cpus", 0)) == 1
            and svc.get("pids_limit") == 64 and svc.get("stop_grace_period") in ("1m40s", "100s")
            and svc.get("logging") == LOGGING, "MODEL_BOUNDS")
        require(svc.get("networks") == {"backend": {"ipv4_address": peers[target]}}, "MODEL_NAMESPACE")
        expected_env = environments[target] | {"POSTGRES_SSL_ROOT_CERT": ca_value}
        require(svc.get("environment") == expected_env or
            (ca_value is None and svc.get("environment") == environments[target]), "MODEL_ENV")
        require(svc.get("labels") == {OWNER: token} and svc.get("container_name") == project + "-" + target, "MODEL_OWNER")
        require(svc.get("volumes") == [dict(type="bind", source=str(directory / "init/postgres-ca.pem"),
            target=CA, read_only=True, bind=dict(create_host_path=False))], "MODEL_MOUNT")


class Probe:
    def __init__(self, directory, image, sha):
        self.directory, self.image, self.sha = directory, image, sha
        self.token = uuid.uuid4().hex
        self.project = "aura-bootstrap-dev-" + self.token
        self.deadline = time.monotonic() + 600
        self.uncertain, self.network = False, None
        self.ids, self.recorded, self.inputs, self.peers = {}, {}, {}, {}
        self.password = secrets.token_hex(24)
        self.image_env = {}
        for name in ("docker", "init", "postgres"):
            (directory / name).mkdir(mode=0o700)
        self.journal()
        print("FIXTURES " + str(directory), flush=True)
        print("PROJECT " + self.project, flush=True)

    def write(self, name, text, mode=0o600):
        path = self.directory / name
        replacement = path.with_name(path.name + ".new")
        replacement.write_text(text)
        replacement.chmod(mode)
        os.replace(replacement, path)
        return path

    def journal(self):
        self.write("ownership.json", json.dumps(dict(project=self.project, label=OWNER, owner=self.token,
            network=self.network, containers=self.recorded, active=self.ids), indent=2))

    def call(self, *args, seconds=20, body=None, check=True, mutation=False):
        try:
            code, out = capture(["/usr/bin/docker", "--host", "unix:///var/run/docker.sock", "--config",
                str(self.directory / "docker"), *args], self.directory,
                min(seconds, self.deadline - time.monotonic()), body)
        except BaseException:
            self.uncertain = True
            raise
        if code and mutation:
            self.uncertain = True
        require(not check or code == 0, "DOCKER_COMMAND_FAILED")
        return out if check else (code, out)

    def compose(self, *args, mutation=False):
        source_guard()
        for name, expected in self.inputs.items():
            require(helpers.checked_bytes(self.directory / name) == expected, "INPUT_CHANGED")
        return self.call("compose", "--project-name", self.project, "--env-file", str(self.directory / "compose.env"),
            "-f", str(COMPOSE), "-f", str(self.directory / "case.json"), *args, mutation=mutation)

    def inspect(self, kind, identifier):
        return json.loads(self.call(kind, "inspect", identifier))[0]

    def remember(self, name, identifier):
        require(re.fullmatch(r"[0-9a-f]{64}", identifier), "RESOURCE_ID_UNKNOWN")
        self.ids[name] = identifier
        self.recorded[identifier] = name
        self.journal()
        return identifier

    def owned(self, identifier):
        item = self.inspect("container", identifier)
        require(item.get("Id") == identifier and item.get("Config", {}).get("Labels", {}).get(OWNER) == self.token
            and item.get("Name") == "/" + self.project + "-" + self.recorded[identifier], "CONTAINER_OWNERSHIP")
        return item

    def network_guard(self):
        item = self.inspect("network", self.network)
        require(item.get("Id") == self.network and item.get("Name") == self.project
            and item.get("Labels") == {OWNER: self.token} and item.get("Internal") is True
            and item.get("Driver") == "bridge" and item.get("Scope") == "local"
            and not item.get("Options") and not item.get("EnableIPv6")
            and item.get("IPAM", {}).get("Driver") == "default"
            and not item.get("IPAM", {}).get("Options")
            and set(item.get("Containers", {})) <= set(self.ids.values()), "NETWORK_OWNERSHIP")

    def runtime(self, identifier, target, environment, command, entrypoint=ENTRYPOINT):
        item = self.owned(identifier)
        cfg, host = item["Config"], item["HostConfig"]
        require(item["Image"] == self.image and cfg.get("User") == "10001:10001"
            and cfg.get("Entrypoint") == entrypoint and cfg.get("Cmd") == command, "RUNTIME_IMAGE")
        expected = self.image_env | environment
        # Docker can preserve an explicitly unset variable as a bare key.
        actual = dict(e.split("=", 1) if "=" in e else (e, None) for e in cfg.get("Env", []))
        require(actual == expected or actual == {k: v for k, v in expected.items() if v is not None}, "RUNTIME_ENV")
        require(not host.get("Privileged") and not host.get("PortBindings") and not host.get("PublishAllPorts")
            and not host.get("CapAdd") and not host.get("Devices") and not host.get("Binds")
            and not host.get("VolumesFrom") and not host.get("Tmpfs")
            and host.get("ReadonlyRootfs") is True and host.get("CapDrop") == ["ALL"]
            and host.get("SecurityOpt") == ["no-new-privileges:true"]
            and host.get("NetworkMode") == self.project
            and host.get("PidMode") in ("", None) and host.get("IpcMode") == "private"
            and host.get("Memory") == 536870912 and host.get("PidsLimit") == 64, "RUNTIME_ISOLATION")
        networks = item["NetworkSettings"]["Networks"]
        require(set(networks) == {self.project} and networks[self.project].get("IPAMConfig", {}).get("IPv4Address") == self.peers[target], "RUNTIME_NETWORK")
        mounts = item["Mounts"]
        require(len(mounts) == 1 and mounts[0].get("Type") == "bind" and not mounts[0].get("RW")
            and mounts[0].get("Source") == str(self.directory / "init/postgres-ca.pem")
            and mounts[0].get("Destination") == CA, "RUNTIME_MOUNTS")
        require(cfg.get("Labels", {}).get("com.docker.compose.service") == "bootstrap-" + target
            and cfg.get("Labels", {}).get("com.docker.compose.project") == self.project, "RUNTIME_COMPOSE")
        self.network_guard()

    def prepare(self):
        source_guard()
        code, out = capture(["/usr/bin/git", "--no-pager", "-C", str(ROOT), "cat-file", "-t", self.sha], self.directory, 5)
        require(code == 0 and out.strip() == "commit", "SOURCE_NOT_LOCAL_COMMIT")
        self.histories = {}
        for target, folder in (("business", "migrations"), ("crawler", "src/crawler/migrations")):
            paths = sorted((ROOT / folder).glob("*.sql"))
            require(len(paths) == (1 if target == "business" else 6), "SOURCE_MIGRATION_COUNT")
            rows = []
            for path in paths:
                raw = path.read_bytes()
                code, committed = capture(["/usr/bin/git", "--no-pager", "-C", str(ROOT), "show",
                    self.sha + ":" + str(path.relative_to(ROOT))], self.directory, 5)
                require(code == 0 and committed.encode() == raw, "SOURCE_SQL_MISMATCH")
                rows.append(f"{path.name.split('_')[0]}:{hashlib.sha384(raw).hexdigest()}:true")
            self.histories[target] = "\n".join(rows)
        image = self.inspect("image", self.image)
        cfg = image["Config"]
        require(image["Id"] == self.image and cfg.get("User") == "10001:10001"
            and cfg.get("Entrypoint") == ENTRYPOINT and cfg.get("Cmd") == ["--help"]
            and not cfg.get("Volumes") and not cfg.get("ExposedPorts") and not cfg.get("Healthcheck")
            and cfg.get("Labels", {}).get("org.opencontainers.image.revision") == self.sha
            and cfg.get("Labels", {}).get("org.opencontainers.image.source") == "https://github.com/aura-historia/backend", "IMAGE_CONTRACT")
        self.image_env = dict(e.split("=", 1) for e in cfg.get("Env", []))
        require(set(self.image_env) == {"PATH", "HOME", "COMMIT_SHA"}
            and self.image_env["COMMIT_SHA"] == self.sha and self.image_env["HOME"] == "/home/aura"
            and self.image_env["PATH"] == "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", "IMAGE_ENV")
        pg_image = self.inspect("image", PG_IMAGE)
        require(pg_image["Id"] == PG_IMAGE and set(pg_image["Config"].get("Volumes") or {}) <=
            {"/var/lib/postgresql/data"}, "PG_IMAGE_ID_OR_VOLUMES")
        self.certificates()
        self.write("compose.env", env_text(dict(BOOTSTRAP_DEV_IMAGE=self.image,
            AURA_DEV_INIT_CONFIG_DIR=str(self.directory / "init"), AURA_NETWORK=self.project)))
        self.write("postgres.env", env_text(dict(POSTGRES_PASSWORD=self.password, POSTGRES_USER="postgres", POSTGRES_DB="postgres")))
        self.write("postgres/pg_hba.conf", "local all all trust\nhost all all all scram-sha-256\n", 0o444)
        self.write("postgres/postgresql.conf", "listen_addresses='*'\nshared_preload_libraries='pg_ttl_index'\n"
            "ssl=on\nssl_cert_file='/fixture/server.pem'\nssl_key_file='/tmp/server.key'\n"
            "hba_file='/fixture/pg_hba.conf'\nlog_connections=on\nlog_statement=none\n"
            "log_min_error_statement=panic\nlog_line_prefix='%m [%p] '\nmax_connections=30\n", 0o444)
        (self.directory / "postgres").chmod(0o755)
        self.network = self.call("network", "create", "--internal", "--driver", "bridge", "--label", OWNER + "=" + self.token,
            self.project, mutation=True).strip()
        require(re.fullmatch(r"[0-9a-f]{64}", self.network), "NETWORK_ID_UNKNOWN")
        self.journal()
        self.network_guard()
        # Reserve high usable addresses, away from the PG fixture's dynamic lease.
        # Docker clears dynamic endpoint/hosts metadata on exit; pin attribution first.
        self.peers = fixture_peers(self.inspect("network", self.network))
        pg = self.call("create", "--pull=never", "--name", self.project + "-postgres", "--label", OWNER + "=" + self.token,
            "--network", self.project, "--network-alias", "postgres", "--network-alias", "wrong-host",
            "--memory=1g", "--memory-swap=1g", "--cpus=2", "--pids-limit=256", "--security-opt=no-new-privileges",
            "--log-driver=local", "--log-opt=max-size=1m", "--log-opt=max-file=1", "--log-opt=compress=false",
            "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=32m,mode=1777",
            "--tmpfs=/var/run/postgresql:rw,nosuid,nodev,noexec,size=16m,mode=1777",
            "--tmpfs=/var/lib/postgresql/data:rw,nosuid,nodev,noexec,size=512m",
            "--mount", f"type=bind,src={self.directory / 'postgres'},dst=/fixture,readonly",
            "--env-file", str(self.directory / "postgres.env"), "--entrypoint=/bin/sh", PG_IMAGE, "-ec",
            "cp /fixture/server.key /tmp/server.key; chown postgres:postgres /tmp/server.key; chmod 600 /tmp/server.key; "
            "exec /usr/local/bin/docker-entrypoint.sh postgres -c config_file=/fixture/postgresql.conf", mutation=True).strip()
        self.remember("postgres", pg)
        item = self.owned(pg)
        host = item["HostConfig"]
        require(item["Image"] == PG_IMAGE and host.get("NetworkMode") == self.project
            and not host.get("PortBindings") and not host.get("PublishAllPorts") and not host.get("Privileged")
            and not host.get("Devices") and not host.get("CapAdd")
            and host.get("Memory") == 1073741824 and host.get("MemorySwap") == 1073741824
            and host.get("Tmpfs") == {
                "/tmp": "rw,nosuid,nodev,noexec,size=32m,mode=1777",
                "/var/run/postgresql": "rw,nosuid,nodev,noexec,size=16m,mode=1777",
                "/var/lib/postgresql/data": "rw,nosuid,nodev,noexec,size=512m"}, "PG_RUNTIME_ISOLATION")
        mounts = [m for m in item["Mounts"] if m["Type"] != "tmpfs"]
        require(len(mounts) == 1 and mounts[0]["Type"] == "bind" and not mounts[0]["RW"]
            and mounts[0]["Source"] == str(self.directory / "postgres")
            and mounts[0]["Destination"] == "/fixture", "PG_RUNTIME_MOUNTS")
        self.call("start", pg, mutation=True)
        self.wait(lambda: self.call("exec", pg, "pg_isready", "-h", "postgres", "-U", "postgres", check=False)[0] == 0, 45, "PG_START")
        require(160000 <= int(self.sql("postgres", "SHOW server_version_num;")) < 170000, "PG16")
        for target in TARGETS:
            self.sql("postgres", f"CREATE DATABASE smoke_{target} TEMPLATE template0;")
        # Do not start the optional TTL worker: its last_run writes would invalidate custody snapshots.
        self.sql("smoke_business", "CREATE EXTENSION pg_ttl_index WITH SCHEMA public;")
        require(self.sql("smoke_business", "SELECT extversion FROM pg_extension WHERE extname='pg_ttl_index';") == "3.0.0", "TTL3")

    def certificates(self):
        def openssl(*args):
            code, _ = capture(["/usr/bin/openssl", *args], self.directory, min(15, self.deadline - time.monotonic()))
            require(code == 0, "CERT_FIXTURE_FAILED")
        for name in ("root", "wrong"):
            openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=" + name,
                "-addext", "basicConstraints=critical,CA:TRUE", "-keyout", str(self.directory / (name + ".key")),
                "-out", str(self.directory / (name + ".pem")))
        openssl("req", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=postgres", "-keyout",
            str(self.directory / "postgres/server.key"), "-out", str(self.directory / "server.csr"))
        self.write("server.ext", "subjectAltName=DNS:postgres\nextendedKeyUsage=serverAuth\n")
        openssl("x509", "-req", "-in", str(self.directory / "server.csr"), "-CA", str(self.directory / "root.pem"),
            "-CAkey", str(self.directory / "root.key"), "-set_serial", "1", "-days", "1", "-extfile",
            str(self.directory / "server.ext"), "-out", str(self.directory / "postgres/server.pem"))
        for name in ("server.key", "server.pem"):
            (self.directory / "postgres" / name).chmod(0o444)

    def sql(self, database, query):
        return self.call("exec", "-i", self.ids["postgres"], "psql", "-XqAt", "-U", "postgres", "-d", database,
            "-v", "ON_ERROR_STOP=1", body="SET statement_timeout='5s';\n" + query, seconds=10).strip()

    def wait(self, predicate, seconds, code):
        end = min(self.deadline, time.monotonic() + seconds)
        while time.monotonic() < end:
            if predicate():
                return
            time.sleep(.2)
        raise Failure(code)

    def snapshot(self):
        require(self.sql("postgres", "SELECT count(*) FROM pg_stat_activity WHERE application_name='crawler-bootstrap-dev';") == "0", "BOOTSTRAP_SESSION_REMAINS")
        result = {}
        for database in ("postgres", "smoke_business", "smoke_crawler"):
            raw = self.call("exec", self.ids["postgres"], "pg_dump", "-U", "postgres", "-d", database, seconds=15)
            normalized = logical_dump(raw)
            if database == "smoke_business":
                # pg_dump may omit extension-owned table data unless registered as extconfig.
                normalized += self.sql(database, "SELECT row_to_json(t) FROM public.ttl_index_table t ORDER BY schema_name,table_name,column_name;")
            result[database] = hashlib.sha256(normalized.encode()).hexdigest()
        raw = self.call("exec", self.ids["postgres"], "pg_dumpall", "-U", "postgres", "--globals-only", seconds=15)
        result["globals"] = hashlib.sha256(logical_dump(raw).encode()).hexdigest()
        databases = self.sql("postgres", "SELECT datname,datdba,encoding,datcollate,datctype,datistemplate,datallowconn,datconnlimit,dattablespace,datacl FROM pg_database ORDER BY datname;")
        result["databases"] = hashlib.sha256(databases.encode()).hexdigest()
        return result

    def environment(self, target, user="postgres"):
        # Poisoned other URL proves it need not be valid, not that getenv never reads it.
        # Unselected DB effects and actual authenticated sessions are separate checks.
        return dict(STAGE="dev", POSTGRES_SSL_MODE="verify-full", **{
            KEYS[target]: f"postgres://{user}:{self.password}@postgres:5432/smoke_{target}",
            KEYS[TARGETS[1 - TARGETS.index(target)]]: "invalid-unselected-url"})

    def attempt(self, target, action, expected, changes=None, ca="root", ca_value=CA, runtime_check=False, reason=None):
        before = self.snapshot()
        if reason == "plaintext":
            require(self.sql("postgres", "SHOW ssl;") == "off", "PLAINTEXT_CONFIG")
        prior_logs = self.call("logs", self.ids["postgres"])
        environments = {t: self.environment(t, "smoke_readonly" if action == "--verify" else "postgres") for t in TARGETS}
        for key, value in (changes or {}).items():
            if value is None:
                environments[target].pop(key, None)
            else:
                environments[target][key] = value
        self.write("init/postgres-ca.pem", (self.directory / (ca + ".pem")).read_text() if ca != "invalid" else "invalid CA\n", 0o444)
        commands = {t: ["--help"] for t in TARGETS}
        commands[target] = ["-ec", RUNTIME_CHECK] if runtime_check else ([action, target] if action else ["--help"])
        overlay = {"services": {"bootstrap-" + t: dict(container_name=self.project + "-" + t,
            labels={OWNER: self.token}, environment={"POSTGRES_SSL_ROOT_CERT": ca_value},
            networks={"backend": {"ipv4_address": self.peers[t]}}) for t in TARGETS}}
        if action or runtime_check:
            overlay["services"]["bootstrap-" + target]["command"] = commands[target]
        if runtime_check:
            overlay["services"]["bootstrap-" + target]["entrypoint"] = ["/bin/sh"]
        for t in TARGETS:
            self.write("init/" + t + ".env", env_text(environments[t]))
        self.write("case.json", json.dumps(overlay))
        self.inputs = {name: (self.directory / name).read_bytes() for name in
            ("compose.env", "case.json", "init/business.env", "init/crawler.env", "init/postgres-ca.pem")}
        model = json.loads(self.compose("config", "--format", "json"))
        validate_model(model, self.directory, self.project, self.token, self.image, environments, commands, ca_value,
            self.peers, target if runtime_check else None)
        self.compose("create", "--no-build", "--pull", "never", "bootstrap-" + target, mutation=True)
        identifier = self.inspect("container", self.project + "-" + target)["Id"]
        self.remember(target, identifier)
        self.runtime(identifier, target, environments[target] | {"POSTGRES_SSL_ROOT_CERT": ca_value}, commands[target],
            ["/bin/sh"] if runtime_check else ENTRYPOINT)
        self.call("start", identifier, mutation=True)
        self.wait(lambda: self.owned(identifier)["State"]["Status"] == "exited", 100, "APP_EXIT_UNKNOWN")
        item = self.owned(identifier)
        state = item["State"]
        out = self.call("logs", identifier).strip()
        after = self.snapshot()
        self.write("last-snapshot.json", json.dumps(dict(target=target, action=action, reason=reason, before=before, after=after), indent=2))
        if expected[0] != 0 or not action or action == "--verify":
            require(after == before, "REJECTION_OR_READONLY_MUTATED_DATABASE")
        else:
            require(all(after[k] == v for k, v in before.items() if k != "smoke_" + target), "UNSELECTED_DATABASE_MUTATED")
        require(state["ExitCode"] == expected[0] and
            (out.startswith("HELP\nbootstrap-dev ") if expected[1] == "HELP" else out == expected[1]), "APP_RESULT_MISMATCH")
        require(not state.get("OOMKilled") and not state.get("Error"), "APP_EXIT_UNKNOWN")
        if reason or (expected[0] == 0 and action) or expected[1] == "NOT_FRESH":
            logs = self.call("logs", self.ids["postgres"])
            require(logs.startswith(prior_logs), "CASE_LOG_WINDOW_LOST")
            delta = logs[len(prior_logs):]
            if reason:
                if reason == "plaintext":
                    duration = datetime.datetime.fromisoformat(state["FinishedAt"].replace("Z", "+00:00")) - datetime.datetime.fromisoformat(state["StartedAt"].replace("Z", "+00:00"))
                    require(self.sql("postgres", "SHOW ssl;") == "off" and 0 <= duration.total_seconds() < 4, "PLAINTEXT_TIMEOUT_OR_CONFIG")
                require(rejection_evidence(delta, self.peers[target], reason), "CASE_REJECTION_EVIDENCE_MISSING_" + reason)
            else:
                attempts = connection_attempts(delta, self.peers[target])
                require(tls_evidence(delta, target) and attempts and all(tls_evidence("\n".join(a), target) for a in attempts), "TLS_SESSION_EVIDENCE_MISSING")
        print("PASS " + target + " " + (action or "default-help") + " " + (reason or expected[1]), flush=True)
        self.remove(target)

    def remove(self, name):
        require(not self.uncertain, "UNKNOWN_OUTCOME_RETAINED")
        identifier = self.ids[name]
        self.owned(identifier)
        self.call("rm", "--force", identifier, mutation=True)
        require(not self.call("container", "ls", "-aq", "--no-trunc", "--filter", "id=" + identifier).strip(), "REMOVAL_UNCONFIRMED")
        del self.ids[name]
        self.journal()

    def run(self):
        self.prepare()
        self.attempt("business", None, (0, "RUNTIME_NO_DOCKER"), runtime_check=True)
        for target in TARGETS:
            other = TARGETS[1 - TARGETS.index(target)]
            self.attempt(target, None, (0, "HELP"))
            cases = [({"STAGE": None}, "root", CA, (3, "CONFIG_ERROR")),
                ({"STAGE": "prod"}, "root", CA, (3, "UNSUPPORTED_STAGE")),
                ({}, "root", None, (3, "CONFIG_ERROR")),
                ({}, "root", "/run/aura/missing-ca.pem", (3, "CONFIG_ERROR")),
                ({}, "invalid", CA, (3, "CONFIG_ERROR")),
                ({"POSTGRES_SSL_MODE": "disable"}, "root", CA, (3, "CONFIG_ERROR")),
                ({KEYS[target]: None, KEYS[other]: self.environment(other)[KEYS[other]]}, "root", CA, (3, "CONFIG_ERROR"))]
            for changes, ca, ca_value, expected in cases:
                self.attempt(target, "--initialize-fresh", expected, changes, ca, ca_value)
            self.attempt(target, "--initialize-fresh", (0, "INITIALIZED_" + target.upper()))
            history = self.sql("smoke_" + target, "SELECT version||':'||encode(checksum,'hex')||':'||success::text FROM public._sqlx_migrations ORDER BY version;")
            require(history == self.histories[target], "SOURCE_SQLX_HISTORY")
            self.attempt(target, "--initialize-fresh", (4, "NOT_FRESH"))
            for reason, changes, ca in (
                ("wrong-hostname", {KEYS[target]: self.environment(target)[KEYS[target]].replace("@postgres:", "@wrong-host:")}, "root"),
                ("wrong-ca", {}, "wrong"),
                ("wrong-password", {KEYS[target]: self.environment(target)[KEYS[target]].replace(self.password, "wrong-password")}, "root")):
                self.attempt(target, "--initialize-fresh", (5, "DEPENDENCY_FAILED"), changes, ca, reason=reason)
                # Same action/target, valid credentials: authenticated TLS NOT_FRESH is
                # the control on each side of every single-fault transport rejection.
                self.attempt(target, "--initialize-fresh", (4, "NOT_FRESH"))
            self.plaintext(True)
            self.attempt(target, "--initialize-fresh", (5, "DEPENDENCY_FAILED"), reason="plaintext")
            self.plaintext(False)
            self.attempt(target, "--initialize-fresh", (4, "NOT_FRESH"))
            print("PASS " + target + " invalid-unselected-config tolerated; unselected DB snapshots unchanged; selected TLS sessions attributed", flush=True)
        self.sql("postgres", f"CREATE ROLE smoke_readonly LOGIN PASSWORD '{self.password}' NOSUPERUSER NOCREATEDB NOCREATEROLE; ALTER ROLE smoke_readonly SET default_transaction_read_only=on;")
        for target in TARGETS:
            self.sql("smoke_" + target, "GRANT USAGE ON SCHEMA public TO smoke_readonly; GRANT SELECT ON ALL TABLES IN SCHEMA public TO smoke_readonly;")
            self.attempt(target, "--verify", (0, "VERIFIED_" + target.upper()))
        self.plaintext(True)
        for target in TARGETS:
            self.attempt(target, "--verify", (5, "VERIFICATION_FAILED"), reason="plaintext")
        self.plaintext(False)
        for target in TARGETS:
            self.attempt(target, "--verify", (0, "VERIFIED_" + target.upper()))

    def plaintext(self, enabled):
        setting = "off" if enabled else "on"
        self.sql("postgres", f"ALTER SYSTEM SET ssl={setting}; SELECT pg_reload_conf();")
        self.wait(lambda: self.sql("postgres", "SHOW ssl;") == setting, 10, "TLS_FIXTURE_RELOAD")
        if enabled:
            # HBA accepts authenticated plaintext; client policy must cause the refusal.
            require(self.call("exec", "-e", "PGPASSWORD=" + self.password, "-e", "PGSSLMODE=disable", self.ids["postgres"],
                "psql", "-XqAt", "-h", "postgres", "-U", "postgres", "-d", "smoke_business", "-c",
                "SELECT NOT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid();").strip() == "t", "PLAINTEXT_CONTROL")

    def cleanup(self):
        require(not self.uncertain, "UNKNOWN_OUTCOME_RETAINED")
        self.deadline = time.monotonic() + 90
        try:
            found = self.call("container", "ls", "-aq", "--no-trunc", "--filter", "label=" + OWNER + "=" + self.token).split()
            require(set(found) == set(self.ids.values()), "UNRECORDED_CONTAINER_RETAINED")
            for identifier in found:
                self.owned(identifier)
            nets = self.call("network", "ls", "-q", "--no-trunc", "--filter", "label=" + OWNER + "=" + self.token).split()
            require(nets == ([self.network] if self.network else []), "UNRECORDED_NETWORK_RETAINED")
            if self.network:
                self.network_guard()
            for name in list(self.ids):
                self.remove(name)
            if self.network:
                self.network_guard()
                self.call("network", "rm", self.network, mutation=True)
                require(not self.call("network", "ls", "-q", "--filter", "id=" + self.network).strip(), "NETWORK_REMOVAL_UNCONFIRMED")
        except BaseException:
            self.uncertain = True
            raise
        shutil.rmtree(self.directory)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, type=image_id)
    parser.add_argument("--source-sha", required=True, type=source_sha)
    args = parser.parse_args(argv)
    os.umask(0o077)
    directory = Path(tempfile.mkdtemp(prefix="aura-bootstrap-dev-"))
    probe = Probe(directory, args.image, args.source_sha)
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
        # One fence covers ALL acceptance checks, including post-init SQLx histories
        # outside attempt(). No failed run may erase its effects or evidence.
        probe.uncertain = True
        print("FAIL " + str(error), flush=True)
    except Exception:
        print("FAIL INTERNAL_ERROR", flush=True)
        probe.uncertain = True
    try:
        probe.cleanup()
        print("CLEANUP confirmed", flush=True)
    except BaseException:
        print("RETAINED " + str(directory) + "; inspect ownership.json; manual review required", flush=True)
        passed = False
    if passed:
        print("PASS isolated fresh-dev initialization; not live deployment/provenance acceptance", flush=True)
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
