#!/usr/bin/env python3
"""Opt-in stock OpenSearch rehearsal. Runtime acceptance requires an actual pass.

Run only after integrator review: python3 deploy/tests/smoke-opensearch.py
  --reviewed-run [--hostname opensearch] [--wrong-hostname wrong-opensearch]
  [--port 9200] [--cluster-name aura-stock-fixture]
No pulls, builds, packages, published ports, PG, cloud, or real credentials.
Stock 3.1.0 is unmaintained: compatibility test ONLY, never live readiness.
1200s work + 90s success cleanup. ANY failed/unknown run retains exact resources
and private ownership.json/evidence.json; no automatic retry or failure cleanup.
Inspect retained IDs before separately authorized exact cleanup. The next run
refuses retained fixtures/resources. Only one rehearsal may run at a time.

Exercises the checked-in platform service, native securityadmin Compose, and
actual operator script copied with ONLY its exact assets. Helper runs as root
with no capabilities to satisfy the operator's root-owned0600 key check; this
is not an application UID10001 or runtime-client CA acceptance test.
Snapshots cover application mappings/settings/aliases/documents+versions,
cluster settings, search pipelines, native users/roles/mappings (not physical
files, audit/compliance, all plugin state, or proof of no transient writes).
Request tracing separately checks operator verify/rejections issue GETs only.
Synthetic direct REST hybrid/PIT are not Rust/CDC/full application acceptance.
Restart means ordinary container stop/start, same data, NOT host/daemon reboot.

DOX handoff: integrator owns deploy/tests/README.md and deploy/AGENTS.md updates;
this task's write scope is only this harness, its unit tests, and tiny fixture.
"""
import argparse
import base64
import contextlib
import fcntl
import hashlib
import http.client
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import ssl
import sys
import tempfile
import time
import uuid

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
OWNER = "org.aura-historia.opensearch-test"
STOCK = "sha256:0dd81b2051dc9ccd9e466596aa66b7764b55184886eeccd36c1cf17bdf5ed27d"
PYTHON = "sha256:adbdfc3fab194e4291b7e5db1eaa1dfa997ea8a51e5a50229bec60124374deb8"
BASELINE = "02e842f23aaa5ccb54c225d6b1307cbc1df5f175"
LIMIT = 1024 * 1024
USERS = ("aura_reader", "aura_product_projector", "aura_filter_projector", "aura_percolator", "aura_cron")
INDICES = ("product-listings", "user_search_filters")
PIPELINE = "/_search/pipeline/hybrid-search-pipeline"
ML_INDEX = ".plugins-ml-config"
ML_HEALTH = "/_cluster/health/" + ML_INDEX + "?level=indices&wait_for_status=yellow&timeout=60s&cluster_manager_timeout=5s&wait_for_active_shards=1"
TOOL = "/usr/share/opensearch/plugins/opensearch-security/tools/"
LOGGING = {"driver": "local", "options": {"max-size": "1m", "max-file": "1", "compress": "false"}}
SECURITY = ["no-new-privileges:true"]
# Reviewed bytes, never learn expected hashes from the current checkout at run time.
PINS = {
    "deploy/compose/compose.platform.yml": "08c96e0d3b459612bf3092a0930fa6f860b4ef2366a41891ec40baaa40e7a31b",
    "deploy/compose/compose.opensearch-admin.yml": "17bab5c38b1d9525b29c50bfe8ef7d84f01f222d30157aa4f68a01d6e886d0ae",
    "deploy/tests/opensearch.fixture.yml": "73a5a36a4911543237831ed99a779bf35f17af95f1347e4672889b9a10097f04",
    "deploy/tests/smoke-sequin-tls.py": "b56b77ca6c4400f416102ffeea0a6d66ccd61095820317627f72c78ae6a73573",
    "deploy/bin/opensearch": "4044e8506a3224e01b8c325bfe24a42ca503e3990c9ef616e172824515acf9be",
    "deploy/compose/opensearch/opensearch.yml": "8c391cce6ff68df743e0708fcfb91e98501b5d4497e06705190ffd699c1e30ee",
    "deploy/compose/opensearch/security/action_groups.yml": "efa55912b14bf8be310cf219932d5d8163e50f62a42db268708e3ec4d9ed06de",
    "deploy/compose/opensearch/security/audit.yml": "8ed2cd75f419cddd6146b22f69951e6c4ba9cf7618071179f03bd05a17ae691c",
    "deploy/compose/opensearch/security/config.yml": "04d7bd05058ba4c86ddda920dd2071db888df9cd99a098f72a565b597c40ce86",
    "deploy/compose/opensearch/security/nodes_dn.yml": "bb2ffd89a0df95d851a4f94477e4cb215eff15b7cced9e69392d95494d16386c",
    "deploy/compose/opensearch/security/roles.yml": "77807711811e1c37a8a1c8f971dbfc50d9d001aeefa4b21237944c8ebba14724",
    "deploy/compose/opensearch/security/roles_mapping.yml": "2638798e1b9d7aef8f6620895b470c66c549e52ffdd01636759c2f2f006cf201",
    "deploy/compose/opensearch/security/tenants.yml": "77acc138aef0d4128a5d5a4e66310ccfe5cc3ebb5d14a59bc58aba335015f98f",
    "opensearch/analysis/english_synonyms.txt": "df9ee6f9210a35d432d31be87ac2be2f18d23929c3578acbc538400f4c5a85b1",
    "opensearch/analysis/french_synonyms.txt": "b571303dd38a63d7714cad4a5d45c188ea9936dba97a103e1e9386fba8b2556e",
    "opensearch/analysis/german_synonyms.txt": "1946ee4a83055f81b24e20b19a2cda1d32468e513987d81f06a9228a8b71f334",
    "opensearch/analysis/italian_synonyms.txt": "642b99c285c25097ed4823367ac354a2ed681c72d27e2512633bb2fa27ef35e6",
    "opensearch/analysis/spanish_synonyms.txt": "d546b97027be2881f83b5a83af60c9000a1071c654d4ec4209d9b32b5608a9f4",
    "opensearch/hybrid-search-pipeline.json": "a1cda77697797ab4790a56729eca1b7eb29984e327aef407552ec848bcf05b83",
    "opensearch/mappings/product_listings.json": "b75fe70793fa7563c88fc5608fd6a8a288eb2e799aba2066f450871b93c655b5",
    "opensearch/mappings/user_search_filters.json": "b8996a4cf4c50c06965414e3936b13b7b49bb3c59c4633ec4f293113f7bcbdfc",
}


class Failure(Exception):
    pass


def require(condition, code):
    if not condition:
        raise Failure(code)


def image_pins_guard():
    require(all(re.fullmatch(r"sha256:[0-9a-f]{64}", image) for image in (STOCK, PYTHON)), "MALFORMED_IMAGE_PIN")


def digest(value):
    return hashlib.sha256(value).hexdigest()


def checked_bytes(path):
    require(path.resolve() == path and path.is_file(), "SOURCE_PATH")
    with path.open("rb") as stream:
        raw = stream.read(LIMIT + 1)
    require(len(raw) <= LIMIT, "SOURCE_LIMIT")
    return raw


def source_guard(root=ROOT):
    snapshot = {}
    for name, sha in PINS.items():
        raw = checked_bytes(root / name)
        require(digest(raw) == sha, "SOURCE_NOT_REVIEWED:" + name)
        snapshot[name] = raw
    return snapshot


def load_capture():
    # Reuse bounded subprocess I/O only; no Sequin/PG machinery runs or is mounted.
    snapshot = source_guard()
    name = "deploy/tests/smoke-sequin-tls.py"
    spec = importlib.util.spec_from_file_location("stock_capture", ROOT / name)
    module = importlib.util.module_from_spec(spec)
    exec(compile(snapshot[name], str(ROOT / name), "exec"), module.__dict__)
    return module.capture


def hostname(value):
    if not re.fullmatch(r"[a-z][a-z0-9-]{0,50}[a-z0-9]", value):
        raise argparse.ArgumentTypeError("short lowercase fixture DNS label required")
    return value


def error_type(response):
    error = response.get("body", {}).get("error")
    return error.get("type") if isinstance(error, dict) else None


def denial(response):
    # Authentication failures, missing indices, parse errors, and timeouts are NOT authorization proof.
    return response.get("status") == 403 and error_type(response) == "security_exception"


def bulk_denials(response, targets):
    body = response.get("body")
    if response.get("status") != 200 or not isinstance(body, dict) or body.get("errors") is not True:
        return False
    items = body.get("items")
    if not isinstance(items, list) or len(items) != len(targets) or not targets:
        return False
    for item, (operation, index, identifier) in zip(items, targets):
        if not isinstance(item, dict) or set(item) != {operation}:
            return False
        result = item[operation]
        if (not isinstance(result, dict) or result.get("_index") != index or result.get("_id") != identifier
                or not denial({"status": result.get("status"), "body": result})):
            return False
    return True


def security_api_denial(response):
    if not isinstance(response, dict) or response.get("status") != 403:
        return False
    body = response.get("body")
    if not isinstance(body, dict) or body.get("status") != "FORBIDDEN":
        return False
    message = body.get("message")
    prefix = "No permission to access REST API: "
    return isinstance(message, str) and message.startswith(prefix) and bool(message[len(prefix):].strip())


def autocreate_policy(response):
    return (response.get("status") == 404 and error_type(response) == "index_not_found_exception"
        and "action.auto_create_index" in response["body"]["error"].get("reason", ""))


def pipeline_absence(response):
    # Stock3.1.0 GET search-pipeline routes return an empty object, not a typed error.
    return response.get("status") == 404 and response.get("body") == {}


def ml_index_ready(response, cluster):
    if not isinstance(response, dict) or response.get("status") != 200:
        return False
    body = response.get("body")
    if (not isinstance(body, dict) or body.get("cluster_name") != cluster
            or body.get("timed_out") is not False or body.get("status") not in ("yellow", "green")):
        return False
    indices = body.get("indices")
    if not isinstance(indices, dict) or set(indices) != {ML_INDEX}:
        return False
    index = indices[ML_INDEX]
    return (isinstance(index, dict) and index.get("status") in ("yellow", "green")
        and all(type(index.get(key)) is int and index[key] == 1
            for key in ("number_of_shards", "active_primary_shards")))


def safe_result(response):
    value = {k: response[k] for k in ("status", "tls", "transport", "classification") if k in response}
    kind = error_type(response)
    if kind and re.fullmatch(r"[a-z_]{1,80}", kind):
        value["error_type"] = kind
    error = response.get("body", {}).get("error", {})
    if isinstance(error, dict):
        value["actions"] = sorted(set(re.findall(r"(?:indices|cluster):[a-zA-Z0-9_/*-]+(?:\[[a-z]+\])?", str(error.get("reason", "")))))
    return value


def plaintext_rejection(raw, peer, port):
    # Netty's channel header and following exception often span separate lines.
    # Never borrow another channel's TLS error or accept a generic disconnect.
    events = re.split(r"(?m)(?=^\[\d{4}-\d\d-\d\d[T ])", raw)
    remote = r"remoteAddress=/" + re.escape(peer) + r":\d+\b"
    local = r"localAddress=/[0-9.]+:" + str(port) + r"\b"
    return any(re.search(remote, event) and re.search(local, event)
        and ("not an SSL/TLS record" in event or "plaintext http traffic on an https channel" in event.lower())
        for event in events)


def securityadmin_rejection(raw, identity):
    text = raw.lower()
    if identity == "node":
        return ("seems to be a node certificate" in text or "is not an admin user" in text
            or ("seems you use a node certificate" in text and "not an admin certificate" in text))
    return identity == "unregistered" and "is not an admin user" in text


def client_request(case):
    """Executed only inside the isolated helper; response bodies stay in bounded private capture."""
    host, port = case["host"], case["port"]
    headers = {"Content-Type": "application/json"}
    if "ndjson" in case:
        require("body" not in case and isinstance(case["ndjson"], str)
            and case["ndjson"].endswith("\n"), "NDJSON_INPUT")
        body = case["ndjson"].encode()
        headers["Content-Type"] = "application/x-ndjson"
    else:
        body = json.dumps(case["body"]).encode() if "body" in case else None
    require(body is None or len(body) <= LIMIT, "HTTP_REQUEST_LIMIT")
    if case.get("user"):
        passwords = json.loads(Path("/client/users.json").read_text())
        password = "wrong-synthetic-password" if case.get("wrong_password") else passwords[case["user"]]
        headers["Authorization"] = "Basic " + base64.b64encode((case["user"] + ":" + password).encode()).decode()
    if case.get("plain"):
        connection = http.client.HTTPConnection(host, port, timeout=8)
    else:
        context = ssl.create_default_context(cafile="/client/" + case.get("ca", "root-ca.pem"))
        if case.get("cert"):
            context.load_cert_chain("/client/" + case["cert"] + ".pem", "/client/" + case["cert"] + "-key.pem")
        timeout = 75 if (case.get("method", "GET") == "GET" and case.get("path") == ML_HEALTH
            and body is None and case.get("cert") == "admin" and not case.get("user")) else 8
        connection = http.client.HTTPSConnection(host, port, context=context, timeout=timeout)
    try:
        connection.request(case.get("method", "GET"), case.get("path", "/"), body=body, headers=headers)
        response = connection.getresponse()
        raw = response.read(LIMIT + 1)
        require(len(raw) <= LIMIT, "HTTP_RESPONSE_LIMIT")
        try:
            parsed = json.loads(raw)
        except ValueError:
            parsed = {"non_json_sha256": digest(raw)}
        result = {"status": response.status, "body": parsed}
        if response.status == 503 and raw.strip() == b"OpenSearch Security not initialized.":
            result["classification"] = "security-not-initialized"
        return result
    except ConnectionRefusedError:
        return {"transport": "connection_refused"}
    except ssl.SSLCertVerificationError as error:
        return {"tls": error.verify_code}
    except (http.client.RemoteDisconnected, ConnectionResetError):
        return {"transport": "peer_closed"}
    finally:
        connection.close()


def client_operator(case):
    # Invoke actual checked-in main and Client, with only request tracing added.
    import importlib.machinery
    loader = importlib.machinery.SourceFileLoader("stock_operator", str(ROOT / "deploy/bin/opensearch"))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    os.environ.clear()
    os.environ.update(STAGE="dev", OPENSEARCH_ENDPOINT_URL=f'https://{case["host"]}:{case["port"]}',
        OPENSEARCH_CLUSTER_NAME=case["cluster"], OPENSEARCH_SSL_ROOT_CERT="/client/root-ca.pem",
        OPENSEARCH_ADMIN_CERT="/client/admin.pem", OPENSEARCH_ADMIN_KEY="/client/admin-key.pem")
    trace, output = [], io.StringIO()
    original = module.Client.request
    def traced(self, method, path, *args, **kwargs):
        trace.append([method, path])
        return original(self, method, path, *args, **kwargs)
    module.Client.request = traced
    with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
        code = module.main([case["operator"]])
    return {"code": code, "trace": trace, "output": output.getvalue()}


def client_main():
    def expired(_sig, _frame):
        raise Failure("CLIENT_DEADLINE")
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(100)
    try:
        raw = sys.stdin.buffer.read(LIMIT + 1)
        require(len(raw) <= LIMIT, "CLIENT_INPUT_LIMIT")
        case = json.loads(raw)
        result = client_operator(case) if "operator" in case else client_request(case)
        encoded = json.dumps(result)
        require(len(encoded.encode()) <= LIMIT, "CLIENT_OUTPUT_LIMIT")
        print(encoded)
        return 0
    except Exception:
        # Includes timeouts: never turn ambiguous transport failure into negative evidence.
        print('{"unknown":true}')
        return 1


def bind(source, target):
    return {"type": "bind", "source": str(source), "target": target, "read_only": True,
        "bind": {"create_host_path": False}}


def validate_model(model, service, expected, project, volume=None):
    require(set(model) <= {"name", "services", "networks", "volumes", "x-platform"}
        and model.get("name") == project, "MODEL_FIELDS")
    network = model.get("networks", {}).get("backend", {})
    require(set(model.get("networks", {})) == {"backend"}
        and network in ({"name": project, "external": True}, {"name": project, "external": True, "ipam": {}}), "MODEL_NETWORK")
    services = model.get("services", {})
    require(set(services) <= {service, "postgres", "redis", "sequin"}, "MODEL_SERVICES")
    for name, svc in services.items():
        if name != service:
            require(svc.get("profiles") == ["excluded-from-opensearch-test"] and not svc.get("volumes")
                and not svc.get("env_file"), "MODEL_EXCLUDED_SERVICE")
    observed = dict(services.get(service) or {})
    # Compose emits null for inherited image entrypoint/Cmd; [] would override them.
    for field in ("entrypoint", "command"):
        if observed.get(field) is None:
            observed.pop(field, None)
    require(observed == expected, "MODEL_SERVICE_DRIFT")
    volumes = model.get("volumes", {})
    require(volumes == ({"opensearch-data": {"name": volume, "external": True}} if volume else {}), "MODEL_VOLUMES")


def owned_network(item, project, token, identifiers):
    require(item.get("Name") == project and item.get("Labels") == {OWNER: token}
        and item.get("Internal") is True and item.get("Driver") == "bridge" and item.get("Scope") == "local"
        and not item.get("Options") and not item.get("EnableIPv6")
        and item.get("IPAM", {}).get("Driver") == "default" and not item.get("IPAM", {}).get("Options")
        and set(item.get("Containers", {})) <= set(identifiers), "NETWORK_OWNERSHIP")


def owned_volume(item, name, token):
    require(item.get("Name") == name and item.get("Labels") == {OWNER: token}
        and item.get("Driver") == "local" and item.get("Scope") == "local" and not item.get("Options"), "VOLUME_OWNERSHIP")


def runtime_guard(item, spec, image, project, token, name):
    cfg, host = item["Config"], item["HostConfig"]
    require(item.get("Name") == "/" + name and cfg.get("Labels", {}).get(OWNER) == token
        and item.get("Image") == spec["image"] == image["Id"], "CONTAINER_OWNERSHIP")
    require(cfg.get("User") == spec["user"]
        and cfg.get("Entrypoint") == spec.get("entrypoint", image["Config"].get("Entrypoint"))
        and cfg.get("Cmd") == spec.get("command", image["Config"].get("Cmd")), "RUNTIME_COMMAND")
    env = dict(e.split("=", 1) for e in image["Config"].get("Env", [])) | spec.get("environment", {})
    require(dict(e.split("=", 1) for e in cfg.get("Env", [])) == env, "RUNTIME_ENV")
    require(not host.get("Privileged") and not host.get("PortBindings") and not host.get("PublishAllPorts")
        and not host.get("Devices") and not host.get("DeviceRequests") and not host.get("VolumesFrom")
        and (host.get("Binds") or []) in ([], [m["source"] + ":" + m["target"] + (":ro" if m.get("read_only") else ":rw")
            for m in spec.get("volumes", []) if m["type"] == "volume"])
        and not host.get("ExtraHosts") and not host.get("Dns")
        and not host.get("Sysctls") and host.get("PidMode") in (None, "")
        and host.get("IpcMode") == "private" and host.get("SecurityOpt") == SECURITY
        and [cap.removeprefix("CAP_") for cap in (host.get("CapAdd") or [])] == spec.get("cap_add", [])
        and [cap.removeprefix("CAP_") for cap in (host.get("CapDrop") or [])] == spec.get("cap_drop", [])
        and host.get("ReadonlyRootfs") == spec.get("read_only", False)
        and host.get("Memory") == spec["mem_limit"] and host.get("PidsLimit") == spec["pids_limit"]
        and host.get("NanoCpus") == int(float(spec["cpus"]) * 1_000_000_000)
        and host.get("RestartPolicy", {}).get("Name") == spec["restart"]
        and host.get("LogConfig") == {"Type": "local", "Config": LOGGING["options"]}, "RUNTIME_ISOLATION")
    mode = spec.get("network_mode", project)
    require(host.get("NetworkMode") == mode, "RUNTIME_NETWORK_MODE")
    networks = item["NetworkSettings"]["Networks"]
    require(set(networks) == {mode}, "RUNTIME_NETWORKS")
    if mode == project:
        expected_aliases = (spec.get("networks", {}).get("backend") or {}).get("aliases", [])
        require(set(networks[project].get("Aliases") or []) <= set(expected_aliases) | {name, "opensearch", "opensearch-admin"}
            and set(expected_aliases) <= set(networks[project].get("Aliases") or []), "RUNTIME_ALIASES")
    expected = {(m["type"], m["source"], m["target"], not m.get("read_only", False)) for m in spec.get("volumes", [])}
    actual = {(m["Type"], m.get("Name") if m["Type"] == "volume" else m.get("Source"), m["Destination"], m["RW"])
        for m in item["Mounts"] if m["Type"] != "tmpfs"}
    require(actual == expected and len(actual) == len(spec.get("volumes", []))
        and len([m for m in item["Mounts"] if m["Type"] != "tmpfs"]) == len(actual)
        and all(m.get("Propagation") in (None, "", "rprivate") for m in item["Mounts"]), "RUNTIME_MOUNTS")
    expected_tmp = dict(value.split(":", 1) for value in spec.get("tmpfs", []))
    require((host.get("Tmpfs") or {}) == expected_tmp, "RUNTIME_TMPFS")


class Probe:
    def __init__(self, directory, args, capture):
        self.directory, self.args, self.capture = directory, args, capture
        self.token = uuid.uuid4().hex
        self.project = "aura-opensearch-" + self.token
        self.volume, self.network = self.project + "-data", None
        self.deadline = time.monotonic() + 1200
        self.ids, self.specs, self.images, self.frozen, self.events = {}, {}, {}, {}, []
        self.generated = {}
        self.pending = None
        for name in ("docker", "stage", "client-tree", "secrets", "pki"):
            (directory / name).mkdir(mode=0o700)
        self.journal()
        print("FIXTURES " + str(directory) + " PROJECT " + self.project, flush=True)

    def write(self, name, content, mode=0o600):
        path = self.directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(path.name + ".new")
        temporary.write_bytes(content if isinstance(content, bytes) else content.encode())
        temporary.chmod(mode)
        os.replace(temporary, path)
        if name.startswith("secrets/"):
            self.generated[name.removeprefix("secrets/")] = digest(content if isinstance(content, bytes) else content.encode())
        return path

    def journal(self):
        self.write("ownership.json", json.dumps({"project": self.project, "label": OWNER, "token": self.token,
            "network": self.network, "volume": self.volume, "containers": self.ids, "pending_container": self.pending,
            "deadline_seconds": 1200, "baseline": BASELINE}, indent=2))
        self.write("evidence.json", json.dumps(self.events, indent=2))

    def event(self, check, **facts):
        self.events.append({"check": check, **facts})
        self.journal()
        print("CHECK " + check, flush=True)

    def call(self, *args, seconds=30, body=None, check=True):
        code, raw = self.capture(["/usr/bin/docker", "--host", "unix:///var/run/docker.sock", "--config",
            str(self.directory / "docker"), *args], self.directory, min(seconds, self.deadline - time.monotonic()), body)
        require(not check or code == 0, "DOCKER_COMMAND_FAILED")
        return raw if check else (code, raw)

    def inspect(self, kind, name):
        code, raw = self.call(kind, "inspect", name, check=False)
        if code and kind == "image" and name in (STOCK, PYTHON) and "no such image:" in raw.lower():
            raise Failure("CACHED_IMAGE_MISSING:" + name)
        require(code == 0, "DOCKER_INSPECT_FAILED")
        return json.loads(raw)[0]

    def guard(self):
        for path, sha in self.frozen.items():
            require(digest(checked_bytes(path)) == sha, "FROZEN_INPUT_CHANGED")
        if self.network:
            item = self.inspect("network", self.network)
            require(item.get("Id") == self.network, "NETWORK_ID")
            owned_network(item, self.project, self.token, self.ids.values())
            owned_volume(self.inspect("volume", self.volume), self.volume, self.token)
            consumers = self.call("container", "ls", "-aq", "--no-trunc", "--filter", "volume=" + self.volume).split()
            require(set(consumers) <= set(self.ids.values()), "VOLUME_FOREIGN_CONSUMER")

    def remember(self, name, identifier, spec):
        require(re.fullmatch(r"[0-9a-f]{64}", identifier), "CONTAINER_ID_UNKNOWN")
        self.ids[name], self.specs[name] = identifier, spec
        self.pending = None
        self.journal()
        self.runtime(name)
        return identifier

    def runtime(self, name):
        spec = self.specs[name]
        item = self.inspect("container", self.ids[name])
        require(item.get("Id") == self.ids[name], "CONTAINER_ID_CHANGED")
        runtime_guard(item, spec, self.images[spec["image"]], self.project, self.token, self.project + "-" + name)
        self.guard()
        return item

    def plain_spec(self, image=PYTHON, command=None, mounts=None, offline=False):
        return dict(image=image, user="0:0", entrypoint=["python3"], command=command or [],
            restart="no", read_only=True, cap_drop=["ALL"], security_opt=SECURITY,
            mem_limit=536870912, cpus="1", pids_limit=128, environment={}, logging=LOGGING,
            volumes=mounts or [], network_mode="none" if offline else self.project)

    def create_plain(self, name, spec):
        self.guard()
        argv = ["create", "--pull=never", "--name", self.project + "-" + name, "--label", OWNER + "=" + self.token,
            "--network", spec["network_mode"], "--user", spec["user"], "--read-only", "--cap-drop=ALL",
            "--security-opt=no-new-privileges:true", "--memory", str(spec["mem_limit"]), "--cpus=1", "--pids-limit=128",
            "--restart=no", "--log-driver=local", "--log-opt=max-size=1m", "--log-opt=max-file=1", "--log-opt=compress=false",
            "--entrypoint", spec["entrypoint"][0]]
        for cap in spec.get("cap_add", []):
            argv += ["--cap-add", cap]
        for mount in spec["volumes"]:
            argv += ["--mount", f'type=bind,src={mount["source"]},dst={mount["target"]}' + (",readonly" if mount.get("read_only") else "")]
        for value in spec.get("tmpfs", []):
            argv += ["--tmpfs", value]
        if spec.get("environment"):
            # All env values are generated synthetic inputs; never credentials in argv.
            path = self.write("tool.env", "".join(k + "=" + v + "\n" for k, v in spec["environment"].items()))
            argv += ["--env-file", str(path)]
        self.pending = self.project + "-" + name
        self.journal()
        identifier = self.call(*argv, spec["image"], *spec["command"]).strip()
        return self.remember(name, identifier, spec)

    def finish(self, name, seconds=100):
        self.runtime(name)
        code, raw = self.call("start", "--attach", self.ids[name], seconds=seconds, check=False)
        item = self.runtime(name)
        state = item["State"]
        if (code and state["Status"] == "created" and not state["Running"] and state.get("Pid") == 0
                and state.get("ExitCode") == 126 and state.get("StartedAt") == "0001-01-01T00:00:00Z"
                and "permission denied" in state.get("Error", "").lower()):
            self.event("oneshot-exec-permission-denied", container=name, exit=126, pid=0,
                error_sha256=digest(state["Error"].encode()))
            raise Failure("ONESHOT_EXEC_PERMISSION_DENIED")
        require(item["State"]["Status"] == "exited" and not item["State"].get("OOMKilled")
            and not item["State"].get("Error") and item["State"]["ExitCode"] == code, "ONESHOT_OUTCOME_UNKNOWN")
        return code, raw

    def retire(self, name):
        item = self.runtime(name)
        if item["State"]["Running"]:
            self.call("stop", "--time", "15", self.ids[name], seconds=25)
            require(not self.runtime(name)["State"]["Running"], "STOP_UNCONFIRMED")
        identifier = self.ids[name]
        self.call("rm", identifier)
        require(not self.call("container", "ls", "-aq", "--no-trunc", "--filter", "id=" + identifier).strip(), "REMOVE_UNCONFIRMED")
        del self.ids[name]
        del self.specs[name]
        self.journal()

    def stage_sources(self, snapshot):
        require(set(snapshot) == set(PINS), "SNAPSHOT_FILES")
        for name, raw in snapshot.items():
            require(digest(raw) == PINS[name], "SNAPSHOT_CHANGED")
            path = self.write("stage/" + name, raw, 0o444)
            self.frozen[path] = PINS[name]
            if name == "deploy/bin/opensearch" or name.startswith("opensearch/mappings/") or name == "opensearch/hybrid-search-pipeline.json":
                copied = self.write("client-tree/" + name, raw, 0o444)
                self.frozen[copied] = PINS[name]

    def native_inputs(self):
        stage = self.directory / "stage"
        source_guard(stage)
        for name in PINS:
            if name.startswith("deploy/compose/opensearch/security/"):
                raw = checked_bytes(stage / name)
                require(digest(raw) == PINS[name], "SNAPSHOT_CHANGED")
                self.write("secrets/admin/security/" + Path(name).name, raw)
        name = "deploy/compose/opensearch/opensearch.yml"
        raw = checked_bytes(stage / name)
        require(digest(raw) == PINS[name], "SNAPSHOT_CHANGED")
        config = raw.decode()
        require(config.count("cluster.name: aura-dev-search") == 1, "NODE_CONFIG_TEMPLATE")
        config = config.replace("cluster.name: aura-dev-search", "cluster.name: " + self.args.cluster_name)
        self.write("secrets/opensearch.yml", config + f"\nhttp.port: {self.args.port}\n", 0o444)

    def freeze_generated(self):
        root = self.directory / "secrets"
        paths = list(root.rglob("*"))
        require(not any(p.is_symlink() for p in paths), "GENERATED_SYMLINK")
        files = {str(p.relative_to(root)): p for p in paths if p.is_file()}
        require(set(files) == set(self.generated), "GENERATED_FILES_CHANGED")
        for name, path in files.items():
            require(digest(checked_bytes(path)) == self.generated[name], "GENERATED_INPUT_CHANGED")
            path.chmod(0o400)
        manifest = self.write("generated-inputs.json", json.dumps(self.generated, sort_keys=True), 0o444)
        self.frozen[manifest] = digest(checked_bytes(manifest))

    def prepare(self):
        image_pins_guard()
        snapshot = source_guard()
        require(not self.call("container", "ls", "-q").strip(), "OTHER_RUNNING_CONTAINERS")
        # No second environment, including retained stopped one-shots/volumes.
        for kind in ("container", "network", "volume"):
            require(not self.call(kind, "ls", "-q", *( ["-a"] if kind == "container" else []),
                "--filter", "label=" + OWNER).strip(), "PRIOR_RESOURCES_RETAINED")
        for image in (STOCK, PYTHON):
            item = self.inspect("image", image)
            require(item["Id"] == image and not item["Config"].get("Volumes")
                and not item["Config"].get("Healthcheck"), "IMAGE_CONTRACT")
            env = dict(e.split("=", 1) for e in item["Config"].get("Env", []))
            require(not any(k.startswith(("AWS_", "GOOGLE_", "GCP_", "AZURE_")) or
                any(s in k for s in ("PASSWORD", "TOKEN", "CREDENTIAL", "PROXY")) for k in env), "IMAGE_AMBIENT_SECRET_ENV")
            self.images[image] = item
        require(self.images[STOCK]["Config"].get("User") in ("1000", "1000:1000", "opensearch"), "STOCK_NONROOT")
        self.stage_sources(snapshot)
        copied = self.write("client-tree/deploy/tests/smoke-opensearch.py", checked_bytes(Path(__file__).resolve()), 0o444)
        self.frozen[copied] = digest(copied.read_bytes())
        # Public assets only; mounted root helper does not need access to host parent0700.
        for path in (self.directory / "client-tree").rglob("*"):
            if path.is_dir():
                path.chmod(0o755)
        (self.directory / "client-tree").chmod(0o755)
        (self.directory / "stage/opensearch/analysis").chmod(0o755)
        self.certificates()
        self.native_inputs()
        self.passwords = {user: secrets.token_hex(24) for user in USERS}
        hashes = {}
        for user in USERS:
            spec = self.plain_spec(STOCK, ["-env", "FIXTURE_PASSWORD"], offline=True)
            spec["entrypoint"] = [TOOL + "hash.sh"]
            spec["user"] = "1000:1000"  # Stock tools are UID/GID1000 mode0750.
            spec["environment"] = {"FIXTURE_PASSWORD": self.passwords[user], "JAVA_TOOL_OPTIONS": "-Xms64m -Xmx256m -XX:ActiveProcessorCount=1"}
            spec["tmpfs"] = ["/tmp:rw,nosuid,nodev,noexec,size=64m,mode=1777"]
            self.create_plain("hash", spec)
            code, raw = self.finish("hash", 45)
            found = re.findall(r"(?m)^\$2[aby]\$\d\d\$[./A-Za-z0-9]{53}$", raw)
            require(code == 0 and len(found) == 1 and self.passwords[user] not in raw, "STOCK_HASH_FAILED")
            hashes[user] = found[0]
            self.retire("hash")
        require(len(set(hashes.values())) == len(USERS), "HASH_NOT_DISTINCT")
        users = {"_meta": {"type": "internalusers", "config_version": 2}}
        users.update({user: {"hash": hashes[user], "backend_roles": []} for user in USERS})
        self.write("secrets/admin/security/internal_users.yml", json.dumps(users))  # JSON is native YAML subset.
        self.write("secrets/client/users.json", json.dumps(self.passwords))
        self.permissions()
        env = dict(OPENSEARCH_IMAGE=STOCK, POSTGRES_IMAGE=STOCK, REDIS_IMAGE=STOCK, SEQUIN_IMAGE=STOCK,
            AURA_CONFIG_DIR=str(self.directory / "secrets"), AURA_OPENSEARCH_ADMIN_DIR=str(self.directory / "secrets/admin"),
            AURA_NETWORK=self.project, FIXTURE_VOLUME=self.volume, FIXTURE_OWNER=self.token,
            FIXTURE_HOST=self.args.hostname, FIXTURE_WRONG_HOST=self.args.wrong_hostname)
        path = self.write("compose.env", "".join(k + "=" + v + "\n" for k, v in env.items()))
        self.frozen[path] = digest(path.read_bytes())
        self.call("volume", "create", "--driver", "local", "--label", OWNER + "=" + self.token, self.volume)
        self.network = self.call("network", "create", "--internal", "--driver", "bridge", "--label", OWNER + "=" + self.token, self.project).strip()
        require(re.fullmatch(r"[0-9a-f]{64}", self.network), "NETWORK_ID_UNKNOWN")
        self.journal()
        self.guard()
        spec = self.plain_spec(command=["-c", "import time; time.sleep(1200)"], mounts=[
            bind(self.directory / "client-tree", "/repo"), bind(self.directory / "secrets/client", "/client")])
        self.create_plain("client", spec)
        self.call("start", self.ids["client"])
        self.runtime("client")
        self.event("isolation-prepared", stock=STOCK, helper=PYTHON, sources=PINS)

    def certificates(self):
        def openssl(*args):
            code, _ = self.capture(["/usr/bin/openssl", *args], self.directory, min(20, self.deadline - time.monotonic()))
            require(code == 0, "CERTIFICATE_FIXTURE_FAILED")
        pki = self.directory / "pki"
        for ca in ("root", "wrong"):
            openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=" + ca,
                "-addext", "basicConstraints=critical,CA:TRUE", "-keyout", str(pki / (ca + ".key")), "-out", str(pki / (ca + ".pem")))
        for serial, (name, subject) in enumerate((("node", "/O=Aura Historia/OU=Nodes/CN=opensearch"),
                ("admin", "/O=Aura Historia/OU=Operators/CN=opensearch-admin"),
                ("unregistered", "/O=Aura Historia/OU=Operators/CN=unregistered")), 1):
            openssl("req", "-newkey", "rsa:2048", "-nodes", "-subj", subject,
                "-keyout", str(pki / (name + "-key.pem")), "-out", str(pki / (name + ".csr")))
            extension = "basicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=clientAuth"
            if name == "node":
                extension += ",serverAuth\nsubjectAltName=DNS:opensearch,DNS:" + self.args.hostname
            ext = self.write("pki/leaf.ext", extension + "\n")
            openssl("x509", "-req", "-in", str(pki / (name + ".csr")), "-CA", str(pki / "root.pem"),
                "-CAkey", str(pki / "root.key"), "-set_serial", str(serial), "-days", "1", "-extfile", str(ext),
                "-out", str(pki / (name + ".pem")))
        for folder, leaves in (("opensearch-certs", ("node",)), ("admin", ("admin", "node", "unregistered")),
                ("client", ("admin", "node", "unregistered"))):
            self.write("secrets/" + folder + "/root-ca.pem", (pki / "root.pem").read_bytes(), 0o444)
            for leaf in leaves:
                for suffix in (".pem", "-key.pem"):
                    self.write("secrets/" + folder + "/" + leaf + suffix, (pki / (leaf + suffix)).read_bytes())
        self.write("secrets/client/wrong-ca.pem", (pki / "wrong.pem").read_bytes(), 0o444)

    def permissions(self, restore=False):
        # The only writable bind is this generated fixture tree, never repo/home/socket.
        root = self.directory / "secrets"
        if not restore:
            self.freeze_generated()
            for path in [root, *root.rglob("*")]:
                if path.is_dir():
                    path.chmod(0o755)
        uid, gid = (os.getuid(), os.getgid()) if restore else (0, 0)
        # Walk top-down: restore must reopen private UID1000 directories before descent.
        script = ("import os,pathlib; r=pathlib.Path('/fixture'); "
            f"os.chown(r,{uid},{gid}); os.chmod(r,0o755)\n"
            "for directory,names,files in os.walk(r):\n"
            " for name in names+files:\n"
            f"  p=pathlib.Path(directory)/name; os.chown(p,{uid},{gid}); os.chmod(p,0o755 if p.is_dir() else 0o600)\n")
        if not restore:
            # Recheck after chown makes private files readable, before native UID1000 handoff.
            # Manifest has hashes only; never credentials in command arguments/output.
            script += ("import hashlib,json; paths=list(r.rglob('*')); "
                "assert not any(p.is_symlink() for p in paths); "
                "assert {str(p.relative_to(r)):hashlib.sha256(p.read_bytes()).hexdigest() for p in paths if p.is_file()} == json.loads(pathlib.Path('/expected.json').read_text())")
            script += ("; n=r/'opensearch-certs'; "
                "[(os.chmod(p,0o600 if p.name.endswith('-key.pem') else 0o444),os.chown(p,1000,1000)) for p in n.iterdir()]; "
                "a=r/'admin'; [(os.chmod(p,0o700 if p.is_dir() else 0o600),os.chown(p,1000,1000)) for p in [*sorted(a.rglob('*'),key=lambda p:len(p.parts),reverse=True),a]]; "
                                "os.chmod(r/'opensearch.yml',0o444); os.chmod(r/'client',0o700)")
        mount = bind(root, "/fixture")
        mount["read_only"] = False
        mounts = [mount] if restore else [mount, bind(self.directory / "generated-inputs.json", "/expected.json")]
        spec = self.plain_spec(command=["-c", script], mounts=mounts, offline=True)
        spec["cap_add"] = ["CHOWN", "FOWNER"]
        self.create_plain("permissions", spec)
        require(self.finish("permissions", 20)[0] == 0, "FIXTURE_OWNERSHIP_FAILED")
        self.retire("permissions")

    def compose_create(self, service, command=None):
        self.guard()
        stage = self.directory / "stage"
        common = dict(image=STOCK, pull_policy="never", security_opt=SECURITY, logging=LOGGING,
            labels={OWNER: self.token}, container_name=self.project + "-" + service)
        overlay = {"services": {service: {"container_name": common["container_name"], "labels": common["labels"]}}}
        if service == "opensearch":
            files = [stage / "deploy/compose/compose.platform.yml", stage / "deploy/tests/opensearch.fixture.yml"]
            expected = common | dict(restart="unless-stopped", user="1000:1000", stop_grace_period="1m",
                mem_limit="3221225472", cpus=2.0, pids_limit=512,
                ulimits={"nofile": {"soft": 65536, "hard": 65536}},
                networks={"backend": {"aliases": [self.args.hostname, self.args.wrong_hostname]}},
                environment={"DISABLE_INSTALL_DEMO_CONFIG": "true", "OPENSEARCH_JAVA_OPTS": "-Xms1g -Xmx1g"},
                volumes=[{"type": "volume", "source": "opensearch-data", "target": "/usr/share/opensearch/data", "volume": {}},
                    bind(self.directory / "secrets/opensearch.yml", "/usr/share/opensearch/config/opensearch.yml"),
                    bind(self.directory / "secrets/opensearch-certs", "/usr/share/opensearch/config/certs"),
                    bind(stage / "opensearch/analysis", "/usr/share/opensearch/config/analysis")])
            volume = self.volume
        else:
            require(service == "opensearch-admin" and command and "-nhnv" not in command, "ADMIN_COMMAND")
            files = [stage / "deploy/compose/compose.opensearch-admin.yml"]
            overlay["services"][service]["command"] = command
            expected = common | dict(profiles=["security-setup"], restart="no", user="1000:1000", read_only=True,
                cap_drop=["ALL"], networks={"backend": None}, mem_limit="1073741824", cpus=1.0, pids_limit=128,
                working_dir="/tmp", tmpfs=["/tmp:rw,nosuid,nodev,noexec,size=64m,mode=1777"],
                entrypoint=[TOOL + "securityadmin.sh"], command=command,
                environment={"JAVA_TOOL_OPTIONS": "-Xms64m -Xmx256m -XX:ActiveProcessorCount=1"},
                volumes=[bind(self.directory / "secrets/admin", "/operator")])
            volume = None
        case = self.write("case.json", json.dumps(overlay))
        frozen_case = checked_bytes(case)
        argv = ["compose", "--project-name", self.project, "--env-file", str(self.directory / "compose.env"),
            "--profile", "security-setup"]
        for path in [*files, case]:
            argv += ["-f", str(path)]
        raw = self.call(*argv, "config", "--format", "json")
        model = json.loads(raw)
        # Normalize only Compose's numeric/duration/empty-network representations.
        for svc in model.get("services", {}).values():
            if "mem_limit" in svc:
                svc["mem_limit"] = str(svc["mem_limit"])
            if svc.get("stop_grace_period") in ("60s", "1m0s"):
                svc["stop_grace_period"] = "1m"
            if svc.get("networks") == {"backend": {}}:
                svc["networks"] = {"backend": None}
        validate_model(model, service, expected, self.project, volume)
        require(checked_bytes(case) == frozen_case, "COMPOSE_OVERLAY_CHANGED")
        self.guard()
        self.pending = common["container_name"]
        self.journal()
        # Exact model validation excludes depends_on for both selected services.
        self.call(*argv, "create", "--pull", "never", "--no-build", service, seconds=60)
        identifier = self.inspect("container", common["container_name"])["Id"]
        runtime = dict(expected)
        runtime["mem_limit"] = int(runtime["mem_limit"])
        if service == "opensearch":
            runtime["volumes"] = [dict(m, source=self.volume) if m["type"] == "volume" else m for m in runtime["volumes"]]
        return self.remember(service, identifier, runtime)

    def client(self, case):
        self.runtime("client")
        case = {"host": self.args.hostname, "port": self.args.port, **case}
        code, raw = self.call("exec", "-i", self.ids["client"], "python3", "/repo/deploy/tests/smoke-opensearch.py", "--client",
            body=json.dumps(case), seconds=105, check=False)
        require(code == 0, "CLIENT_OUTCOME_UNKNOWN")
        result = json.loads(raw)
        require(not result.get("unknown"), "CLIENT_OUTCOME_UNKNOWN")
        return result

    def request(self, method, path, body=None, user=None, expected=(200,), cert="admin", **options):
        case = {"method": method, "path": path, "user": user, "cert": None if user else cert, **options}
        if body is not None:
            case["body"] = body
        result = self.client(case)
        if result.get("status") not in expected:
            self.event("unexpected-http", method=method, path=path, user=user or cert, result=safe_result(result))
            raise Failure("HTTP_ASSERTION_RETAINED")
        return result

    def target(self, response=None):
        response = self.client({"cert": "admin"}) if response is None else response
        self.event("trusted-admin-root", result=safe_result(response))
        require(response.get("status") == 200, "TARGET_ROOT_NOT_200_RETAINED")
        result = response["body"]
        require(result.get("cluster_name") == self.args.cluster_name and result.get("version", {}).get("number") == "3.1.0"
            and result["version"].get("distribution") == "opensearch", "TARGET_IDENTITY")

    def wait_node(self):
        end = min(self.deadline, time.monotonic() + 180)
        while time.monotonic() < end:
            require(self.runtime("opensearch")["State"]["Running"], "NODE_EXITED")
            # Only a confirmed refusal before the listener starts may be retried.
            # HTTP503, TLS failures and unknown client/command outcomes stop immediately.
            response = self.client({"cert": "admin"})
            if response.get("transport") == "connection_refused":
                time.sleep(2)
                continue
            self.target(response)
            return
        raise Failure("NODE_START_UNCONFIRMED")

    def admin(self, identity="admin", offline=False):
        command = ["-cd", "/operator/security", "-vc", "7"] if offline else ["-h", self.args.hostname, "-p", str(self.args.port),
            "-cn", self.args.cluster_name, "-cacert", "/operator/root-ca.pem", "-cert", "/operator/" + identity + ".pem",
            "-key", "/operator/" + identity + "-key.pem", "-cd", "/operator/security", "-ff"]
        if not offline:
            self.target()  # -cn does not enforce identity in stock3.1.0.
        self.compose_create("opensearch-admin", command)
        code, raw = self.finish("opensearch-admin", 120)
        self.event("securityadmin-" + ("offline" if offline else identity), exit=code, output_sha256=digest(raw.encode()))
        if identity == "admin":
            require(code == 0 and not any(marker in raw for marker in ("ERR:", "FAIL:", "ERROR")), "SECURITYADMIN_FAILED_RETAINED")
        else:
            require(code != 0 and securityadmin_rejection(raw, identity), "ADMIN_REJECTION_UNATTRIBUTED")
        self.retire("opensearch-admin")

    def operator(self, mode, success):
        result = self.client({"operator": mode, "cluster": self.args.cluster_name})
        require(result["code"] == (0 if success else 1), "OPERATOR_RESULT_RETAINED")
        if not success:
            require("outcome=no-writes-attempted" in result["output"], "OPERATOR_UNKNOWN_WRITE_RETAINED")
        if mode == "verify" or not success:
            require(result["trace"] and all(method == "GET" for method, _ in result["trace"]), "OPERATOR_WROTE")
        self.event("operator-" + mode, success=success, trace=result["trace"])

    def ml_config_ready(self):
        # Native named-index wait, not global ML quiescence or a new snapshot baseline.
        response = self.client({"method": "GET", "path": ML_HEALTH, "cert": "admin"})
        self.write("ml-config-health.json", json.dumps(response))  # Private bounded response for failed-gate diagnosis.
        ready = ml_index_ready(response, self.args.cluster_name)
        self.event("ml-config-index-readiness", ready=ready,
            response_sha256=digest(json.dumps(response, sort_keys=True).encode()))
        require(ready, "ML_CONFIG_INDEX_NOT_READY")

    def query_insights(self):
        expected = {"search.insights.top_queries." + metric + ".enabled": "false"
            for metric in ("latency", "cpu", "memory")}
        expected.update({"search.insights.top_queries.exporter.type": "none", "search.query.metrics.enabled": "false"})
        settings = self.request("GET", "/_cluster/settings?include_defaults=true&flat_settings=true")["body"]
        effective = settings.get("defaults", {}) | settings.get("persistent", {}) | settings.get("transient", {})
        require(all(effective.get(key) == value for key, value in expected.items()), "QUERY_INSIGHTS_NOT_DISABLED")
        indices = self.request("GET", "/_cat/indices?format=json&h=index&expand_wildcards=all")["body"]
        require(isinstance(indices, list) and all(isinstance(row, dict) and isinstance(row.get("index"), str)
            for row in indices), "QUERY_INSIGHTS_INVENTORY_UNKNOWN")
        require(not any(row["index"].startswith("top_queries-") for row in indices), "QUERY_INSIGHTS_INDEX_APPEARED")
        self.event("query-insights-disabled-no-export-index", settings=expected)

    def snapshot(self):
        state = {}
        for index in (*INDICES, "fixture-cross-index", "fixture-autocreate"):
            response = self.request("GET", "/" + index + "?flat_settings=true", expected=(200, 404))
            if response["status"] == 404:
                require(error_type(response) == "index_not_found_exception", "SNAPSHOT_ABSENCE")
                state[index] = None
                continue
            docs = self.request("GET", "/" + index + "/_search?version=true&seq_no_primary_term=true",
                {"size": 100, "track_total_hits": True, "query": {"match_all": {}}})["body"]
            require(not docs.get("timed_out") and docs["_shards"]["failed"] == 0
                and docs["hits"]["total"]["value"] == len(docs["hits"]["hits"]) <= 100, "SNAPSHOT_INCOMPLETE")
            state[index] = {"metadata": response["body"], "docs": sorted(
                [{k: hit[k] for k in ("_id", "_source", "_version", "_seq_no", "_primary_term")} for hit in docs["hits"]["hits"]],
                key=lambda hit: hit["_id"])}
        for path in ("/_cluster/settings?flat_settings=true", "/_search/pipeline", "/_alias",
                "/_plugins/_security/api/internalusers", "/_plugins/_security/api/roles", "/_plugins/_security/api/rolesmapping"):
            if path == "/_search/pipeline":
                response = self.request("GET", path, expected=(200, 404))
                require(response["status"] == 200 or pipeline_absence(response), "PIPELINE_ABSENCE_UNCONFIRMED")
                state[path] = response["body"]
            else:
                state[path] = self.request("GET", path)["body"]
        sha = digest(json.dumps(state, sort_keys=True).encode())
        # Fixed snapshot section names only; never expose security values or document fields.
        self.event("snapshot", snapshot_sha256=sha, field_sha256={
            field: digest(json.dumps(value, sort_keys=True).encode()) for field, value in state.items()})
        self.query_insights()  # Extra assertion; never filter or normalize the full snapshot.
        return sha

    def unchanged(self, check, action):
        before = self.snapshot()
        action()
        after = self.snapshot()
        require(before == after, "STATE_CHANGED:" + check)
        self.event(check, snapshot_sha256=after)

    def trust_tests(self):
        def negatives():
            self.target()
            for name, options, accepted in (("wrong-ca", {"ca": "wrong-ca.pem"}, {18, 19, 20, 21}),
                    ("wrong-hostname", {"host": self.args.wrong_hostname}, {62, 64})):
                response = self.client({"cert": "admin", **options})
                require(response.get("tls") in accepted, "TLS_REJECTION_UNATTRIBUTED:" + name)
                self.event(name, result=safe_result(response))
                self.target()
            self.request("GET", "/product-listings/_doc/product-0", user="aura_reader")
            wrong = self.request("GET", "/product-listings/_doc/product-0", user="aura_reader", wrong_password=True, expected=(401,))
            self.event("wrong-password", result=safe_result(wrong))
            self.request("GET", "/product-listings/_doc/product-0", user="aura_reader")
            # No credential is sent over plaintext. Require a node-specific TLS handler log,
            # bracketed by verified HTTPS, not just a client timeout/connection reset.
            stamp = self.call("exec", self.ids["client"], "python3", "-c",
                "import datetime; print(datetime.datetime.now(datetime.timezone.utc).isoformat())").strip()
            response = self.client({"plain": True})
            require(response.get("transport") == "peer_closed", "PLAINTEXT_NOT_REJECTED")
            raw = self.call("logs", "--since", stamp, "--tail", "100", self.ids["opensearch"])
            peer = self.runtime("client")["NetworkSettings"]["Networks"][self.project]["IPAddress"]
            require(plaintext_rejection(raw, peer, self.args.port), "PLAINTEXT_REJECTION_UNATTRIBUTED")
            self.event("plaintext", peer=peer, node_log_sha256=digest(raw.encode()))
            self.target()
            for identity in ("unregistered", "node"):
                self.admin(identity)
                self.target()
        self.unchanged("trust-negatives-unchanged", negatives)

    def grants(self):
        # Root must work for every selected runtime role; collect gaps without widening grants.
        gaps = []
        for user in USERS:
            result = self.client({"user": user})
            self.event("role-root-" + user, result=safe_result(result))
            if result.get("status") != 200:
                gaps.append(user + ":root")
        for index, user, id_field in ((INDICES[0], USERS[1], "productListingId"), (INDICES[1], USERS[2], "userSearchFilterId")):
            for number in range(3):
                identifier = ("product-" if index == INDICES[0] else "filter-") + str(number)
                doc = {id_field: identifier, "projectionDeleted": False}
                if index == INDICES[0]:
                    doc.update(titleEn="antique chair", embedding=[1.0, number / 10] + [0.0] * 766)
                else:
                    doc.update(query={"match": {"titleEn": "antique chair"}})
                response = self.client({"user": user, "method": "PUT", "path": f"/{index}/_doc/{identifier}?version=10&version_type=external&refresh=true", "body": doc})
                self.event("role-write-" + user, result=safe_result(response))
                if response.get("status") not in (200, 201):
                    gaps.append(user + ":write")
                    break
                self.request("GET", f"/{index}/_doc/{identifier}", user=user)
            for reader in (USERS[0], user, USERS[4] if index == INDICES[0] else USERS[3]):
                response = self.client({"user": reader, "method": "POST", "path": f"/{index}/_search", "body": {"query": {"match_all": {}}}})
                self.event("role-search-" + reader, result=safe_result(response))
                if response.get("status") != 200 or response.get("body", {}).get("_shards", {}).get("failed") != 0:
                    gaps.append(reader + ":search")
                if reader != USERS[4]:
                    identifier = "product-0" if index == INDICES[0] else "filter-0"
                    response = self.client({"user": reader, "path": f"/{index}/_doc/{identifier}"})
                    self.event("role-get-" + reader, result=safe_result(response))
                    if response.get("status") != 200 or response.get("body", {}).get("found") is not True:
                        gaps.append(reader + ":get")
        require(not gaps, "ROLE_GRANT_GAPS:" + ",".join(gaps))
        self.event("core-grants-passed")

    def denied_changes(self):
        self.request("PUT", "/fixture-cross-index", {"mappings": {"properties": {"name": {"type": "keyword"}}}})
        self.request("PUT", "/fixture-cross-index/_doc/sentinel?refresh=true", {"name": "preserve"}, expected=(201,))
        def negatives():
            for user in USERS:
                index = INDICES[1] if user in (USERS[2], USERS[3]) else INDICES[0]
                cases = [("auto-create", "PUT", "/fixture-autocreate/_doc/x", {"name": "forbidden"}),
                                    ("index-create", "PUT", "/fixture-autocreate", {"mappings": {"properties": {"name": {"type": "keyword"}}}}),
                    ("index-delete", "DELETE", "/" + index, None),
                    ("document-delete", "DELETE", "/" + index + "/_doc/" + ("product-0" if index == INDICES[0] else "filter-0"), None),
                    ("mapping", "PUT", "/" + index + "/_mapping", {"properties": {"forbidden": {"type": "keyword"}}}),
                    ("settings", "PUT", "/" + index + "/_settings", {"index": {"number_of_replicas": 2}}),
                    ("alias", "POST", "/_aliases", {"actions": [{"add": {"index": index, "alias": "forbidden-alias"}}]}),
                    ("pipeline", "PUT", PIPELINE, {"description": "forbidden", "phase_results_processors": []}),
                    ("security", "PUT", "/_plugins/_security/api/roles/forbidden", {"cluster_permissions": ["*"]}),
                    ("cross-index-write", "PUT", "/fixture-cross-index/_doc/sentinel", {"name": "forbidden"}),
                    ("cross-index-read", "GET", "/fixture-cross-index/_doc/sentinel", None)]
                if user == USERS[4]:
                    cases.append(("search-only-get", "GET", "/product-listings/_doc/product-0", None))
                if user == USERS[1]:
                    cases.append(("other-projector", "PUT", "/user_search_filters/_doc/filter-0", {"projectionDeleted": True}))
                if user == USERS[2]:
                    cases.append(("other-projector", "PUT", "/product-listings/_doc/product-0", {"projectionDeleted": True}))
                if user in (USERS[0], USERS[3], USERS[4]):
                    cases.append(("readonly-write", "PUT", "/" + index + "/_doc/forbidden", {"projectionDeleted": True}))
                for label, method, path, body in cases:
                    case = {"user": user, "method": method, "path": path}
                    if body is not None:
                        case["body"] = body
                    result = self.client(case)
                    self.write("last-denial-response.json", json.dumps({"user": user, "method": method, "path": path, "response": result}))
                    self.event("denied-" + user + "-" + label, result=safe_result(result))
                    # auto_create_index=false can reject before authorization: attribute as
                    # cluster policy, not a role denial, with index absence in snapshot.
                    policy = label == "auto-create" and autocreate_policy(result)
                    # Stock's management-disabled branch omits identity; prior auth and full snapshots bind this check.
                    rest_denial = (label == "security" and method == "PUT" and path == "/_plugins/_security/api/roles/forbidden"
                        and security_api_denial(result))
                    require(denial(result) or policy or rest_denial, "LEAST_PRIVILEGE_FAILED_RETAINED")
                if user in (USERS[1], USERS[2]):
                    other = INDICES[1] if index == INDICES[0] else INDICES[0]
                    targets = [("delete", index, "product-0" if index == INDICES[0] else "filter-0"),
                        ("index", other, "product-0" if other == INDICES[0] else "filter-0"),
                        ("index", "fixture-cross-index", "sentinel")]
                    lines = []
                    for operation, target, identifier in targets:
                        lines.append({operation: {"_index": target, "_id": identifier}})
                        if operation == "index":
                            lines.append({"projectionDeleted": True} if target in INDICES else {"name": "forbidden"})
                    result = self.client({"user": user, "method": "POST", "path": "/_bulk?refresh=true",
                        "ndjson": "".join(json.dumps(line) + "\n" for line in lines)})
                    self.write("last-denial-response.json", json.dumps({"user": user, "method": "POST", "path": "/_bulk?refresh=true", "response": result}))
                    self.event("bulk-envelope-" + user, result=safe_result(result))
                    require(bulk_denials(result, targets), "BULK_ITEM_DENIAL_FAILED_RETAINED")
                    self.event("bulk-all-items-denied-" + user, items=[safe_result({"status": item[operation]["status"],
                        "body": item[operation]}) for item, (operation, _, _) in zip(result["body"]["items"], targets)])
        self.unchanged("role-denials-unchanged", negatives)

    def tombstones(self, seed=True):
        for index, user in zip(INDICES, (USERS[1], USERS[2])):
            path = f"/{index}/_doc/fence"
            if seed:
                require(self.request("PUT", "/" + index + "/_settings", {"index": {"gc_deletes": "1s"}})["body"].get("acknowledged") is True,
                    "GC_SETTING_UNCONFIRMED")
                observed = self.request("GET", "/" + index + "/_settings?flat_settings=true")["body"]
                require(observed[index]["settings"].get("index.gc_deletes") == "1s", "GC_SETTING_NOT_APPLIED")
                self.request("PUT", path + "?version=10&version_type=external&refresh=true", {"projectionDeleted": False}, user=user, expected=(201,))
                self.request("PUT", path + "?version=20&version_type=external&refresh=true", {"projectionDeleted": True}, user=user)
        if seed:
            time.sleep(2)  # Beyond configured physical-delete GC; tombstones are never DELETEd.
        def conflicts():
            for index, user in zip(INDICES, (USERS[1], USERS[2])):
                path = f"/{index}/_doc/fence"
                for version in (19, 20):
                    result = self.request("PUT", path + f"?version={version}&version_type=external", {"projectionDeleted": False}, user=user, expected=(409,))
                    require(error_type(result) == "version_conflict_engine_exception", "FENCE_CONFLICT_TYPE")
                doc = self.request("GET", path, user=user)["body"]
                require(doc["_version"] == 20 and doc["_source"] == {"projectionDeleted": True}, "FENCE_CONTENT")
        self.unchanged("tombstones-after-restart" if not seed else "tombstones-beyond-gc", conflicts)

    def searches(self):
        live = {"bool": {"must_not": [{"term": {"projectionDeleted": True}}]}}
        hybrid = {"size": 3, "query": {"hybrid": {"queries": [
            {"bool": {"must": [{"match": {"titleEn": "antique chair"}}], "filter": [live]}},
            {"knn": {"embedding": {"vector": [1.0] + [0.0] * 767, "k": 3, "filter": live}}}]}}}
        result = self.request("POST", "/product-listings/_search?search_pipeline=hybrid-search-pipeline", hybrid, user=USERS[0])["body"]
        require(not result.get("timed_out") and result["_shards"]["failed"] == 0
            and {h["_id"] for h in result["hits"]["hits"]} == {"product-0", "product-1", "product-2"}, "HYBRID_RESULTS")
        pit = self.request("POST", "/user_search_filters/_search/point_in_time?keep_alive=1m", user=USERS[3])["body"]["pit_id"]
        query = {"size": 1, "pit": {"id": pit, "keep_alive": "1m"}, "sort": [{"userSearchFilterId": "asc"}],
            "query": {"bool": {"filter": [live, {"percolate": {"field": "query", "document": {"titleEn": "antique chair"}}}]}}}
        found = []
        for _ in range(4):
            result = self.request("POST", "/_search", query, user=USERS[3])["body"]
            require(not result.get("timed_out") and result["_shards"]["failed"] == 0, "PIT_INCOMPLETE")
            hits = result["hits"]["hits"]
            if not hits:
                break
            found.extend(h["_id"] for h in hits)
            query["search_after"] = hits[-1]["sort"]
        require(found == ["filter-0", "filter-1", "filter-2"], "PIT_PERCOLATION_RESULTS")
        result = self.request("DELETE", "/_search/point_in_time", {"pit_id": [pit]}, user=USERS[3])["body"]
        require(result.get("pits") and all(p.get("successful") is True for p in result["pits"]), "PIT_CLOSE")
        self.event("synthetic-768-hybrid-and-multipage-pit")

    def run(self):
        self.prepare()
        self.admin(offline=True)
        self.compose_create("opensearch")
        self.call("start", self.ids["opensearch"])
        self.wait_node()
        self.admin()
        self.target()
        self.ml_config_ready()
        self.unchanged("missing-verify-unchanged", lambda: self.operator("verify", False))
        # Partial-state refusal BEFORE real fresh init. Only this known synthetic
        # pipeline is removed, explicitly, after a successful unchanged assertion.
        pipeline = json.loads(checked_bytes(self.directory / "stage/opensearch/hybrid-search-pipeline.json"))
        require(self.request("PUT", PIPELINE, pipeline)["body"].get("acknowledged") is True, "PARTIAL_FIXTURE_CREATE_UNCONFIRMED")
        self.unchanged("partial-init-refused-unchanged", lambda: self.operator("initialize-fresh", False))
        require(self.request("DELETE", PIPELINE)["body"].get("acknowledged") is True, "PARTIAL_FIXTURE_REMOVE_UNCONFIRMED")
        require(pipeline_absence(self.request("GET", PIPELINE, expected=(404,))), "PARTIAL_FIXTURE_REMOVE")
        self.operator("initialize-fresh", True)
        self.unchanged("verify-unchanged", lambda: self.operator("verify", True))
        self.unchanged("repeat-init-refused-unchanged", lambda: self.operator("initialize-fresh", False))
        self.grants()
        self.denied_changes()
        self.tombstones()
        self.unchanged("populated-verify-unchanged", lambda: self.operator("verify", True))
        self.unchanged("populated-repeat-init-refused", lambda: self.operator("initialize-fresh", False))
        self.unchanged("searches-unchanged", self.searches)
        self.trust_tests()
        before = self.snapshot()
        identifier = self.ids["opensearch"]
        started = self.runtime("opensearch")["State"]["StartedAt"]
        self.call("stop", "--time", "60", identifier, seconds=70)
        require(not self.runtime("opensearch")["State"]["Running"], "NODE_STOP_UNCONFIRMED")
        self.call("start", identifier)
        self.wait_node()
        require(self.runtime("opensearch")["State"]["StartedAt"] != started, "NODE_NOT_RESTARTED")
        self.ml_config_ready()
        self.operator("verify", True)
        require(before == self.snapshot(), "RESTART_STATE_CHANGED")
        for user in USERS:
            self.request("GET", "/", user=user)
        self.tombstones(seed=False)
        self.event("same-container-data-users-restart", snapshot_sha256=before)

    def cleanup(self):
        # Called ONLY after every assertion passed; no generic failure cleanup path.
        self.deadline = time.monotonic() + 90
        self.guard()
        found = self.call("container", "ls", "-aq", "--no-trunc", "--filter", "label=" + OWNER + "=" + self.token).split()
        require(set(found) == set(self.ids.values()), "UNRECORDED_CONTAINER_RETAINED")
        require(self.call("network", "ls", "-q", "--no-trunc", "--filter", "label=" + OWNER + "=" + self.token).split() == [self.network], "UNRECORDED_NETWORK_RETAINED")
        require(self.call("volume", "ls", "-q", "--filter", "label=" + OWNER + "=" + self.token).split() == [self.volume], "UNRECORDED_VOLUME_RETAINED")
        for name in list(self.ids):
            self.retire(name)
        self.permissions(restore=True)
        self.guard()
        self.call("network", "rm", self.network)
        self.call("volume", "rm", self.volume)
        for kind in ("container", "network", "volume"):
            require(not self.call(kind, "ls", "-q", *(["-a"] if kind == "container" else []),
                "--filter", "label=" + OWNER + "=" + self.token).strip(), "CLEANUP_UNCONFIRMED")
        print("EVIDENCE " + json.dumps(self.events, sort_keys=True), flush=True)
        shutil.rmtree(self.directory)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--reviewed-run", action="store_true", help="explicit integrator pre-review approval required")
    parser.add_argument("--hostname", type=hostname, default="opensearch")
    parser.add_argument("--wrong-hostname", type=hostname, default="wrong-opensearch")
    parser.add_argument("--cluster-name", type=hostname, default="aura-stock-fixture")
    parser.add_argument("--port", type=int, default=9200)
    args = parser.parse_args(argv)
    require(args.reviewed_run, "INTEGRATOR_PRE_REVIEW_REQUIRED")
    require(1024 <= args.port <= 65535 and args.port != 9300 and args.hostname != args.wrong_hostname
        and args.wrong_hostname != "opensearch" and "opensearch-admin" not in (args.hostname, args.wrong_hostname), "FIXTURE_ENDPOINT_INPUT")
    # Lock this existing file read-only: no global lock/config file or broad host writes.
    with Path(__file__).open("rb") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        require(not list(Path("/tmp").glob("aura-opensearch-*")), "RETAINED_FIXTURE_REVIEW_REQUIRED")
        capture = load_capture()
        os.umask(0o077)
        directory = Path(tempfile.mkdtemp(prefix="aura-opensearch-", dir="/tmp"))
        probe = Probe(directory, args, capture)
        def interrupted(_sig, _frame):
            raise Failure("INTERRUPTED_OR_DEADLINE_RETAINED")
        previous = {sig: signal.signal(sig, interrupted) for sig in (signal.SIGALRM, signal.SIGINT, signal.SIGTERM)}
        signal.alarm(1200)
        try:
            probe.run()
            signal.alarm(90)
            probe.cleanup()
            print("PASS isolated stock rehearsal; no live/runtime-client/CDC/reboot acceptance", flush=True)
            return 0
        except Exception as error:
            # Only our static assertion codes are printable, never raw subprocess exceptions.
            print("FAIL " + (str(error) if isinstance(error, Failure) else "UNKNOWN_OUTCOME"), flush=True)
            print("RETAINED " + str(directory) + "; inspect ownership.json and evidence.json; no blind cleanup/retry", flush=True)
            return 1
        finally:
            signal.alarm(0)
            for sig, handler in previous.items():
                signal.signal(sig, handler)


if __name__ == "__main__":
    if sys.argv[1:] == ["--client"]:
        sys.exit(client_main())
    try:
        sys.exit(main())
    except (Failure, BlockingIOError) as error:
        print("REFUSED " + (str(error) if isinstance(error, Failure) else "REHEARSAL_LOCKED"), file=sys.stderr)
        sys.exit(1)
