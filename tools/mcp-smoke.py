#!/usr/bin/env python3
"""Bounded, read-only generic MCP stage front-door verification (Python standard library)."""

import argparse
import datetime
import json
import pathlib
import re
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request

CURRENT = "2026-07-28"
LEGACY = ("2025-11-25", "2025-06-18", "2025-03-26")
MCP_ORIGIN = "https://mcp.stage.aura-historia.com"
ISSUER = "https://api.stage.aura-historia.com"
MAX_BYTES = 2 * 1024 * 1024
MAX_REQUESTS = 40
MAX_SECONDS = 90
MIN_REQUEST_INTERVAL = 0.3


class SmokeFailure(Exception):
    pass


class NoRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Smoke:
    def __init__(self, origin=MCP_ORIGIN, issuer=ISSUER):
        self.origin = origin
        self.issuer = issuer
        self.opener = urllib.request.build_opener(NoRedirects)
        self.started = time.monotonic()
        self.last_request = 0
        self.evidence = []

    def request(self, label, path, expected, *, method="GET", headers=None, body=None,
                origin=None, no_store=True):
        remaining = MAX_SECONDS - (time.monotonic() - self.started)
        if remaining <= 0 or len(self.evidence) >= MAX_REQUESTS:
            raise SmokeFailure("smoke request/time budget exhausted")
        delay = max(0, self.last_request + MIN_REQUEST_INTERVAL - time.monotonic())
        if remaining <= delay:
            raise SmokeFailure("smoke request/time budget exhausted")
        time.sleep(delay)
        self.last_request = time.monotonic()
        remaining = MAX_SECONDS - (self.last_request - self.started)
        request = urllib.request.Request((origin or self.origin) + path, data=body,
                                         headers=headers or {}, method=method)
        try:
            response = self.opener.open(request, timeout=min(10, remaining))
        except urllib.error.HTTPError as error:
            response = error
        except (urllib.error.URLError, TimeoutError, OSError):
            raise SmokeFailure(f"{label}: network request failed") from None
        with response:
            payload = response.read(MAX_BYTES + 1)
            if len(payload) > MAX_BYTES:
                raise SmokeFailure(f"{label}: response exceeds the MCP bound")
            safe_headers = {key: response.headers[key] for key in (
                "Cache-Control", "Allow", "Access-Control-Allow-Origin",
                "Access-Control-Allow-Headers", "Access-Control-Expose-Headers",
                "WWW-Authenticate", "MCP-Protocol-Version", "Retry-After"
            ) if key in response.headers}
            self.evidence.append({"check": label, "status": response.code,
                                  "headers": safe_headers})
            statuses = expected if isinstance(expected, tuple) else (expected,)
            require(response.code in statuses, f"{label}: unexpected HTTP status")
            if no_store:
                require(response.headers.get("Cache-Control") == "no-store",
                        f"{label}: missing no-store")
            require(not response.headers.get("Mcp-Session-Id"),
                    f"{label}: unexpected session")
            return response.headers, payload

    def rpc(self, label, method, params, expected=200, *, version=CURRENT, token=None,
            headers=None, notification=False):
        params = dict(params)
        if version == CURRENT:
            params["_meta"] = {
                "io.modelcontextprotocol/protocolVersion": CURRENT,
                "io.modelcontextprotocol/clientInfo": {"name": "generic-stage-smoke", "version": "1"},
                "io.modelcontextprotocol/clientCapabilities": {},
            }
        routing = {"Content-Type": "application/json",
                   "Accept": "application/json, text/event-stream",
                   "MCP-Protocol-Version": version, "Mcp-Method": method}
        if method == "tools/call":
            routing["Mcp-Name"] = params["name"]
        if token:
            routing["Authorization"] = f"Bearer {token}"
        routing.update(headers or {})
        message = {"jsonrpc": "2.0", "method": method, "params": params}
        if not notification:
            message["id"] = 1
        result_headers, payload = self.request(label, "/v1", expected, method="POST",
                                               headers=routing, body=json.dumps(message).encode())
        if notification:
            require(not payload, f"{label}: notification response must be empty")
            return result_headers, None
        if expected != 200:
            return result_headers, None
        try:
            return result_headers, json.loads(payload)
        except (ValueError, UnicodeError):
            raise SmokeFailure(f"{label}: expected bounded JSON") from None

    def verify_discovery(self):
        for path in ("/.well-known/oauth-protected-resource", "/.well-known/oauth-protected-resource/v1"):
            _, body = self.request(f"metadata {path}", path, 200)
            metadata = json.loads(body)
            require(metadata["resource"] == self.origin + "/v1", "wrong MCP resource")
            require(metadata["authorization_servers"] == [self.issuer], "wrong Aura issuer")
        _, body = self.request("authorization-server metadata", "/.well-known/oauth-authorization-server",
                               200, origin=self.issuer)
        metadata = json.loads(body)
        require(metadata["issuer"] == self.issuer, "wrong discovered issuer")
        require(metadata["token_endpoint"] == self.issuer + "/v1/oauth/token", "wrong token endpoint")
        require(metadata["authorization_endpoint"] == "https://stage.aura-historia.com/en/oauth/authorize",
                "wrong browser consent endpoint")
        require(metadata["code_challenge_methods_supported"] == ["S256"], "wrong PKCE metadata")
        require(metadata["token_endpoint_auth_methods_supported"] == ["client_secret_post"],
                "wrong fixed-client authentication")
        require("registration_endpoint" not in metadata, "unexpected registration claim")

    def verify_protocols(self):
        for path in ("/health", "/ready"):
            for method in ("GET", "HEAD"):
                self.request(f"probe {method} {path}", path, 204, method=method)
        _, discovered = self.rpc("current discovery", "server/discover", {})
        require("error" not in discovered, "current discovery failed")
        _, listed = self.rpc("anonymous catalogue", "tools/list", {})
        tools = listed.get("result", {}).get("tools", [])
        expected = manifest_names()
        require({tool["name"] for tool in tools} == expected and len(tools) == len(expected),
                "stage catalogue differs from this release's disjoint manifests")
        require(not listed["result"].get("nextCursor"), "unexpected catalogue continuation")
        _, result = self.rpc("anonymous public operation", "tools/call",
                             {"name": "getPublicOverview", "arguments": {}})
        require("error" not in result and not result["result"].get("isError"),
                "public overview failed")
        for version in LEGACY:
            _, initialized = self.rpc(f"initialize {version}", "initialize", {
                "protocolVersion": version, "capabilities": {},
                "clientInfo": {"name": "generic-stage-smoke", "version": "1"}
            }, version=version)
            require(initialized.get("result", {}).get("protocolVersion") == version,
                    "unexpected negotiated revision")
            self.rpc(f"initialized {version}", "notifications/initialized", {}, 202,
                     version=version, notification=True)
            _, legacy = self.rpc(f"catalogue {version}", "tools/list", {}, version=version)
            require({tool["name"] for tool in legacy.get("result", {}).get("tools", [])} == expected,
                    "legacy catalogue differs")

    def verify_http_boundary(self):
        challenge, _ = self.rpc("protected missing bearer", "tools/call",
                                {"name": "getUserAccount", "arguments": {}}, 401)
        require("resource_metadata=" in challenge.get("WWW-Authenticate", ""), "missing resource challenge")
        self.rpc("invalid bearer on public operation", "tools/call",
                 {"name": "getPublicOverview", "arguments": {}}, 401,
                 token="deliberately-invalid-smoke-credential")
        allowed, _ = self.rpc("allowed Origin", "tools/list", {},
                              headers={"Origin": "https://stage.aura-historia.com"})
        require(allowed.get("Access-Control-Allow-Origin") == "https://stage.aura-historia.com",
                "allowed POST drops allow-origin")
        exposed = {value.strip().lower() for value in allowed.get("Access-Control-Expose-Headers", "").split(",")}
        require({"www-authenticate", "x-request-id", "x-correlation-id"} <= exposed,
                "allowed POST drops exposed challenge/trace headers")
        for origin in ("https://unapproved.example", "null"):
            self.rpc(f"denied Origin {origin}", "tools/list", {}, 403, headers={"Origin": origin})
        allowed, _ = self.request("allowed preflight", "/v1", 204, method="OPTIONS", headers={
            "Origin": "https://stage.aura-historia.com", "Access-Control-Request-Method": "POST",
            "Access-Control-Request-Headers": "authorization,content-type,mcp-protocol-version,mcp-method,mcp-name"
        })
        require(allowed.get("Access-Control-Allow-Origin") == "https://stage.aura-historia.com",
                "wrong allow-origin")
        allowed_headers = {value.strip().lower() for value in allowed.get("Access-Control-Allow-Headers", "").split(",")}
        require({"authorization", "mcp-protocol-version", "mcp-method", "mcp-name"} <= allowed_headers,
                "preflight drops MCP/auth headers")
        denied, _ = self.request("denied preflight", "/v1", (204, 403), method="OPTIONS", headers={
            "Origin": "https://unapproved.example", "Access-Control-Request-Method": "POST",
            "Access-Control-Request-Headers": "authorization,mcp-protocol-version"
        })
        require(not denied.get("Access-Control-Allow-Origin"), "denied preflight grants allow-origin")
        for method in ("GET", "HEAD", "DELETE"):
            headers, _ = self.request(f"method {method}", "/v1", 405, method=method)
            require(headers.get("Allow") == "POST", "wrong allowed method")
        self.rpc("routing header mismatch", "tools/list", {}, 400, headers={"Mcp-Method": "ping"})
        self.rpc("unsupported revision", "tools/list", {}, 400, version="1900-01-01")

    def run(self, token=None):
        self.verify_discovery()
        self.verify_protocols()
        self.verify_http_boundary()
        if token:
            _, account = self.rpc("delegated account read", "tools/call",
                                  {"name": "getUserAccount", "arguments": {}}, token=token)
            require("error" not in account and not account["result"].get("isError"),
                    "delegated account read failed")
        return {"protocols": [CURRENT, *LEGACY], "sdk": "rmcp 3.6.0",
                "checks": self.evidence, "authenticated_read_executed": bool(token)}


def require(condition, message):
    if not condition:
        raise SmokeFailure(message)


def manifest_names():
    directory = pathlib.Path(__file__).resolve().parents[1] / "src/aura-historia-mcp"
    names = set()
    for filename in ("public-operations.json", "authenticated-operations.json"):
        for operation in json.loads((directory / filename).read_text())["operations"]:
            require(operation["name"] == operation["operationId"], "manifest operation ID mismatch")
            require(operation["name"] not in names, "manifests overlap")
            names.add(operation["name"])
    return names


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release-commit", required=True, help="exact deployed 40-character commit SHA")
    parser.add_argument("--token-file", type=pathlib.Path, help="optional stage users:read token; never printed")
    parser.add_argument("--output", type=pathlib.Path, help="safe JSON evidence file (default stdout)")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.release_commit):
        parser.error("--release-commit must be an exact lowercase commit SHA")
    token = args.token_file.read_text().strip() if args.token_file else None
    if token is not None and (not token or any(character.isspace() for character in token)):
        parser.error("token file must contain one bearer value")
    smoke = Smoke()
    report = {"started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "release_commit": args.release_commit, "origin": MCP_ORIGIN,
              "evidence_kind": "live-stage", "runner_commit": subprocess.check_output(
                  ["git", "rev-parse", "HEAD"], text=True).strip()}
    def deadline_exceeded(signum, frame):
        raise SmokeFailure("smoke wall-clock deadline exceeded")

    signal.signal(signal.SIGALRM, deadline_exceeded)
    signal.setitimer(signal.ITIMER_REAL, MAX_SECONDS)
    try:
        report.update(smoke.run(token))
        report["outcome"] = "passed"
    except SmokeFailure as error:
        report.update({"outcome": "failed", "failure": str(error), "checks": smoke.evidence})
    except (ValueError, KeyError):
        report.update({"outcome": "failed", "failure": "unexpected discovery or protocol JSON",
                       "checks": smoke.evidence})
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    rendered = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.write_text(rendered)
    else:
        sys.stdout.write(rendered)
    return 0 if report["outcome"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
