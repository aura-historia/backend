#!/usr/bin/env python3
"""Initialize a fresh production volume; later invocations only verify it."""

import argparse
import base64
import io
import json
import os
from pathlib import Path
import secrets
import shutil
import ssl
import subprocess
import tarfile
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent
PRIVATE = ROOT / ".private"
OPERATOR = PRIVATE / "operator"
NODE = PRIVATE / "node"
COMPOSE = ["docker", "compose", "--file", str(ROOT / "compose.yaml")]
VOLUMES = ["aura-historia-opensearch-prod-" + name for name in ["data", "certificates", "gateway"]]
LOCAL = "https://localhost:19201"
PUBLIC = "https://opensearch.aura-historia.com"


def run(args, **kwargs):
    result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kwargs)
    if result.returncode:
        # Some upstream tools include supplied configuration in errors.
        raise RuntimeError(f"{args[0]} operation failed (exit {result.returncode}); inspect locally without exposing secrets")
    return result.stdout


def private_json(path, value):
    with open(path, "w", opener=lambda name, flags: os.open(name, flags, 0o600)) as file:
        json.dump(value, file, indent=2)
        file.write("\n")


def create_certificates():
    if (OPERATOR / "root-ca.pem").exists():
        for directory, names in [(OPERATOR, ["root-ca.pem", "root-ca-key.pem", "admin.pem", "admin-key.pem"]),
                                 (NODE, ["root-ca.pem", "node.pem", "node-key.pem"])]:
            if not all((directory / name).is_file() for name in names):
                raise RuntimeError("Incomplete existing private certificate state; repair it before retrying")
        return
    openssl = ["openssl"]
    run(openssl + ["req", "-x509", "-newkey", "rsa:3072", "-nodes", "-sha256", "-days", "3650",
                   "-subj", "/CN=Aura Historia OpenSearch production CA", "-keyout", str(OPERATOR / "root-ca-key.pem"),
                   "-out", str(OPERATOR / "root-ca.pem")])
    for name, directory, subject, extensions in [
        ("node", NODE, "/CN=opensearch", "subjectAltName=DNS:opensearch,DNS:localhost,IP:127.0.0.1\nextendedKeyUsage=serverAuth,clientAuth\n"),
        ("admin", OPERATOR, "/CN=aura-opensearch-prod-admin", "extendedKeyUsage=clientAuth\n"),
    ]:
        run(openssl + ["req", "-new", "-newkey", "rsa:3072", "-nodes", "-subj", subject,
                       "-keyout", str(directory / (name + "-key.pem")), "-out", str(directory / (name + ".csr"))])
        extension_file = directory / (name + ".ext")
        extension_file.write_text(extensions)
        run(openssl + ["x509", "-req", "-in", str(directory / (name + ".csr")), "-CA", str(OPERATOR / "root-ca.pem"),
                       "-CAkey", str(OPERATOR / "root-ca-key.pem"), "-CAcreateserial", "-sha256", "-days", "825",
                       "-extfile", str(extension_file), "-out", str(directory / (name + ".pem"))])
    shutil.copyfile(OPERATOR / "root-ca.pem", NODE / "root-ca.pem")
    (OPERATOR / "root-ca.pem").chmod(0o644)


def create_volumes(image, instance_id):
    for volume in VOLUMES:
        existing = subprocess.run(["docker", "volume", "inspect", volume], capture_output=True)
        if existing.returncode == 0:
            labels = json.loads(existing.stdout)[0].get("Labels") or {}
            if labels.get("aura.bootstrap-instance") != instance_id:
                raise RuntimeError(f"Refusing to adopt an existing volume: {volume}")
        else:
            run(["docker", "volume", "create", "--label", "aura.bootstrap-instance=" + instance_id, volume])
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w") as tar:
        for path in NODE.glob("*.pem"):
            entry = tar.gettarinfo(str(path), arcname=path.name)
            entry.uid = entry.gid = 1000
            entry.uname = entry.gname = "opensearch"
            entry.mode = 0o600 if "key" in path.name else 0o644
            with path.open("rb") as file:
                tar.addfile(entry, file)
    run(["docker", "run", "--rm", "-i", "--user", "0:0", "--entrypoint", "tar",
         "--mount", "type=volume,src=" + VOLUMES[1] + ",dst=/certificates", image,
         "-xpf", "-", "-C", "/certificates"], input=archive.getvalue())


def prepare_security(image):
    directory = PRIVATE / "security"
    if directory.exists():
        return
    container = run(["docker", "create", image]).decode().strip()
    try:
        run(["docker", "cp", container + ":/usr/share/opensearch/config/opensearch-security", str(directory)])
    finally:
        run(["docker", "rm", container])
    private_json(directory / "internal_users.yml", {"_meta": {"type": "internalusers", "config_version": 2}})
    curl = PRIVATE / "admin-curl"
    curl.write_text(f'cacert = "{OPERATOR / "root-ca.pem"}"\ncert = "{OPERATOR / "admin.pem"}"\nkey = "{OPERATOR / "admin-key.pem"}"\n')


def client_context(admin=False):
    context = ssl.create_default_context(cafile=str(OPERATOR / "root-ca.pem"))
    if admin:
        context.load_cert_chain(str(OPERATOR / "admin.pem"), str(OPERATOR / "admin-key.pem"))
    return context


def request(path, method="GET", body=None, credentials=None, public=False):
    headers = {"Content-Type": "application/json"}
    if credentials:
        value = credentials["username"] + ":" + credentials["password"]
        headers["Authorization"] = "Basic " + base64.b64encode(value.encode()).decode()
    req = urllib.request.Request((PUBLIC if public else LOCAL) + path,
                                 data=None if body is None else json.dumps(body).encode(), method=method, headers=headers)
    context = ssl.create_default_context() if public else client_context(admin=credentials is None)
    try:
        with urllib.request.urlopen(req, context=context, timeout=20) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, None


def require(result, expected=200):
    status, body = result
    allowed = expected if isinstance(expected, tuple) else (expected,)
    if status not in allowed:
        raise RuntimeError(f"Unexpected OpenSearch HTTP status {status}; expected {expected}")
    return body


def initialize(credentials):
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        try:
            status, _ = request("/_plugins/_security/api/roles")
            if status in (500, 503):
                run(COMPOSE + ["--profile", "bootstrap", "run", "--rm", "--no-deps", "securityadmin"])
                require(request("/_plugins/_security/api/roles"))
                break
            if status == 200:
                break
            raise RuntimeError(f"Unexpected bootstrap state: HTTP {status}")
        except (urllib.error.URLError, TimeoutError):
            time.sleep(5)
    else:
        raise RuntimeError("OpenSearch did not become reachable within three minutes")
    require(request("/"))
    for role, credential in credentials.items():
        role_name = "aura_prod_" + role.replace("-", "_")
        manifest = json.loads((ROOT.parent / "roles" / (role + ".json")).read_text())
        require(request("/_plugins/_security/api/roles/" + role_name, "PUT", manifest), (200, 201))
        require(request("/_plugins/_security/api/internalusers/" + credential["username"], "PUT", {"password": credential["password"]}), (200, 201))
        require(request("/_plugins/_security/api/rolesmapping/" + role_name, "PUT", {"users": [credential["username"]], "backend_roles": [], "hosts": []}), (200, 201))
    pipeline = json.loads((ROOT.parent / "hybrid-search-pipeline.json").read_text())
    require(request("/_search/pipeline/hybrid-search-pipeline", "PUT", pipeline))
    status, _ = request("/product-listings")
    if status != 404:
        raise RuntimeError("Product index already exists; refusing fresh-install initialization")
    mapping = json.loads((ROOT.parent / "mappings/product_listings.json").read_text())
    mapping["settings"].update(number_of_shards=1, number_of_replicas=0)
    require(request("/product-listings", "PUT", mapping))
    # This non-business probe runs before the gateway is started and adds no mapping fields.
    require(request("/product-listings/_doc/__bootstrap_probe__", "PUT", {}, credentials["product-projector"]), 201)
    require(request("/product-listings/_doc/__bootstrap_probe__", "DELETE"))
    require(request("/product-listings/_refresh", "POST"))
    if require(request("/product-listings/_count"))["count"] != 0:
        raise RuntimeError("Fresh product index is not empty")


def verify(credentials, public=False):
    require(request("/product-listings/_search", "POST", {"size": 0}, credentials["reader"], public))
    require(request("/product-listings/_search", "POST", {"size": 0}, credentials["product-projector"], public), 403)
    require(request("/product-listings/_doc/__denied_probe__", "PUT", {}, credentials["reader"], public), 403)
    if public:
        require(request("/_plugins/_security/api/internalusers", public=True), 403)
        require(request("/product-listings/_search", "POST", {"size": 0}, public=True), 401)
        bad = {"username": credentials["reader"]["username"], "password": secrets.token_urlsafe(32)}
        require(request("/product-listings/_search", "POST", {"size": 0}, bad, True), 401)
    else:
        health = require(request("/_cluster/health"))
        if health["status"] != "green" or health["number_of_nodes"] != 1:
            raise RuntimeError("Production single-node cluster is not green")
    print(("Public" if public else "Local") + " TLS, scoped reader/projector and administration-denial checks passed", flush=True)


def publish_ssm(credentials):
    account = json.loads(run(["aws", "sts", "get-caller-identity", "--output", "json"]))["Account"]
    if account != "550663032924":
        raise RuntimeError("Refusing to publish production configuration to an unexpected AWS account")
    values = {"endpoint-url": PUBLIC}
    for role, credential in credentials.items():
        values.update({role + "/" + key: value for key, value in credential.items()})
    for suffix, value in values.items():
        name = "/opensearch/prod/" + suffix
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json") as file:
            json.dump({"Name": name, "Type": "String", "Value": value, "Overwrite": True}, file)
            file.flush()
            run(["aws", "ssm", "put-parameter", "--region", "eu-central-1", "--cli-input-json", "file://" + file.name])
        print("Configured " + name, flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify-public", action="store_true")
    parser.add_argument("--publish-ssm", action="store_true")
    args = parser.parse_args()
    if args.publish_ssm and not args.verify_public:
        parser.error("--publish-ssm requires successful --verify-public checks")
    os.umask(0o077)
    for directory in [PRIVATE, OPERATOR, NODE]:
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    config = json.loads(run(COMPOSE + ["config", "--format", "json"]))
    image = config["services"]["opensearch"]["image"]
    state_path = PRIVATE / "state.json"
    if state_path.exists():
        state = json.loads(state_path.read_text())
    else:
        if args.verify_public:
            raise RuntimeError("Initialize the fresh private node before verifying public access")
        if shutil.disk_usage("/var/lib/docker").free < 100 * 1024**3:
            raise RuntimeError("Fresh production setup requires at least 100 GiB free Docker storage")
        available_kib = next(int(line.split()[1]) for line in Path("/proc/meminfo").read_text().splitlines()
                             if line.startswith("MemAvailable:"))
        if available_kib * 1024 < int(config["services"]["opensearch"]["mem_limit"]) + 8 * 1024**3:
            raise RuntimeError("Available RAM must cover the container memory limit plus 8 GiB for the host")
        if int(Path("/proc/sys/vm/max_map_count").read_text()) < 262144:
            raise RuntimeError("OpenSearch requires vm.max_map_count >= 262144")
        state = {"instance": secrets.token_hex(16), "complete": False}
        private_json(state_path, state)
    create_certificates()
    credential_path = PRIVATE / "runtime-credentials.json"
    if not credential_path.exists():
        private_json(credential_path, {role: {"username": "aura_prod_" + role.replace("-", "_"), "password": secrets.token_urlsafe(48)}
                                      for role in ["reader", "product-projector"]})
    credentials = json.loads(credential_path.read_text())
    if not state["complete"]:
        create_volumes(image, state["instance"])
        prepare_security(image)
        run(COMPOSE + ["up", "-d", "opensearch"])
        initialize(credentials)
        verify(credentials)
        state["complete"] = True
        private_json(state_path, state)
    else:
        verify(credentials)
    if args.verify_public:
        verify(credentials, public=True)
        observation = {"target": "prod", "targetHost": PUBLIC, "index": "product-listings", "alias": "product-listings",
                       "mapping": require(request("/product-listings/_mapping")), "aliases": require(request("/product-listings/_alias")),
                       "observedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
        private_json(PRIVATE / "observation.json", observation)
    if args.publish_ssm:
        publish_ssm(credentials)


if __name__ == "__main__":
    main()
