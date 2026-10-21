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
        self.tools = {}

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
            self.evidence.append({"check": label, "status": response.code,
                                  "headers": evidence_headers(response.headers)})
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
            message = json.loads(payload)
        except (ValueError, UnicodeError):
            raise SmokeFailure(f"{label}: expected bounded JSON") from None
        require(isinstance(message, dict) and message.get("jsonrpc") == "2.0"
                and type(message.get("id")) is int and message["id"] == 1
                and "error" not in message and isinstance(message.get("result"), dict),
                f"{label}: invalid or unsuccessful JSON-RPC response")
        return result_headers, message["result"]

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
        require(discovered.get("supportedVersions") == [CURRENT, *LEGACY],
                "current discovery drops supported revisions")
        _, listed = self.rpc("anonymous catalogue", "tools/list", {})
        self.tools = verify_catalogue(listed)
        _, result = self.rpc("anonymous public operation", "tools/call",
                             {"name": "getPublicOverview", "arguments": {}})
        verify_tool_result("public overview", result, self.tools["getPublicOverview"]["outputSchema"])
        for version in LEGACY:
            _, initialized = self.rpc(f"initialize {version}", "initialize", {
                "protocolVersion": version, "capabilities": {},
                "clientInfo": {"name": "generic-stage-smoke", "version": "1"}
            }, version=version)
            require(initialized.get("protocolVersion") == version,
                    "unexpected negotiated revision")
            self.rpc(f"initialized {version}", "notifications/initialized", {}, 202,
                     version=version, notification=True)
            _, legacy = self.rpc(f"catalogue {version}", "tools/list", {}, version=version)
            require(verify_catalogue(legacy) == self.tools, "legacy catalogue differs")

    def verify_http_boundary(self):
        challenge, _ = self.rpc("protected missing bearer", "tools/call",
                                {"name": "getUserAccount", "arguments": {}}, 401)
        metadata = f'resource_metadata="{self.origin}/.well-known/oauth-protected-resource/v1"'
        require(challenge.get("WWW-Authenticate", "").startswith("Bearer ")
                and metadata in challenge["WWW-Authenticate"]
                and 'scope="users:read"' in challenge["WWW-Authenticate"], "wrong resource/scope challenge")
        invalid, _ = self.rpc("invalid bearer on public operation", "tools/call",
                              {"name": "getPublicOverview", "arguments": {}}, 401,
                              token="deliberately-invalid-smoke-credential")
        require(metadata in invalid.get("WWW-Authenticate", "")
                and 'error="invalid_token"' in invalid["WWW-Authenticate"], "wrong invalid-token challenge")
        allowed, _ = self.rpc("allowed Origin", "tools/list", {},
                              headers={"Origin": "https://stage.aura-historia.com"})
        require(allowed.get("Access-Control-Allow-Origin") == "https://stage.aura-historia.com",
                "allowed POST drops allow-origin")
        exposed = {value.strip().lower() for value in allowed.get("Access-Control-Expose-Headers", "").split(",")}
        require({"www-authenticate", "x-request-id", "x-correlation-id", "content-type"} <= exposed,
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
        require({"authorization", "content-type", "mcp-protocol-version", "mcp-method", "mcp-name"} <= allowed_headers,
                "preflight drops MCP/auth headers")
        allowed_methods = {value.strip() for value in allowed.get("Access-Control-Allow-Methods", "").split(",")}
        require("POST" in allowed_methods, "preflight drops POST method")
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
            verify_tool_result("delegated account read", account,
                               self.tools["getUserAccount"]["outputSchema"])
        return {"protocols": [CURRENT, *LEGACY], "sdk": "rmcp 3.6.0",
                "checks": self.evidence, "authenticated_read_executed": bool(token)}


def require(condition, message):
    if not condition:
        raise SmokeFailure(message)


def evidence_headers(headers):
    """Persist only known public values; a broken front door may reflect credentials."""
    allowed = {
        "Cache-Control": {"no-store"}, "Allow": {"POST"},
        "Access-Control-Allow-Origin": {"https://stage.aura-historia.com"},
        "MCP-Protocol-Version": {CURRENT, *LEGACY}, "Retry-After": {"1"},
    }
    safe = {}
    for key, values in allowed.items():
        if key in headers:
            safe[key] = headers[key] if headers[key] in values else "[unexpected value omitted]"
    header_names = {"authorization", "content-type", "accept", "mcp-protocol-version",
                    "mcp-method", "mcp-name", "mcp-session-id", "last-event-id",
                    "www-authenticate", "x-request-id", "x-correlation-id"}
    for key in ("Access-Control-Allow-Headers", "Access-Control-Expose-Headers"):
        if key in headers:
            names = {name.strip().lower() for name in headers[key].split(",")}
            safe[key] = ", ".join(sorted(names)) if names <= header_names else "[unexpected value omitted]"
    if "Access-Control-Allow-Methods" in headers:
        methods = {method.strip() for method in headers["Access-Control-Allow-Methods"].split(",")}
        safe["Access-Control-Allow-Methods"] = ", ".join(sorted(methods)) if methods <= {
            "GET", "HEAD", "POST", "OPTIONS"} else "[unexpected value omitted]"
    if "WWW-Authenticate" in headers:
        safe["WWW-Authenticate"] = "[present; value omitted]"
    return safe


def manifest_operations():
    directory = pathlib.Path(__file__).resolve().parents[1] / "src/aura-historia-mcp"
    operations = {}
    for filename in ("public-operations.json", "authenticated-operations.json"):
        manifest = json.loads((directory / filename).read_text())
        for operation in manifest["operations"]:
            require(operation["name"] == operation["operationId"], "manifest operation ID mismatch")
            require(operation["name"] not in operations, "manifests overlap")
            operations[operation["name"]] = {
                key: operation.get(key, manifest.get(key))
                for key in ("access", "requiredScopes", "annotations")
            }
    return operations


def verify_catalogue(result):
    tools = result.get("tools")
    require(isinstance(tools, list) and all(isinstance(tool, dict)
            and isinstance(tool.get("name"), str) for tool in tools), "invalid tool catalogue")
    actual = {tool["name"]: tool for tool in tools}
    expected = manifest_operations()
    require(actual.keys() == expected.keys() and len(tools) == len(expected),
            "stage catalogue differs from this release's disjoint manifests")
    require(not result.get("nextCursor"), "unexpected catalogue continuation")
    for name, tool in actual.items():
        metadata = tool.get("_meta", {})
        policy = expected[name]
        require(metadata.get("aura.historia/operationId") == name
                and metadata.get("aura.historia/access") == policy["access"]
                and metadata.get("aura.historia/requiredScopes") == policy["requiredScopes"]
                and tool.get("annotations") == policy["annotations"], "tool policy differs from manifest")
        require(all(isinstance(tool.get(key), dict) and bool(tool[key])
                    for key in ("inputSchema", "outputSchema")), "missing tool schema")
    return actual


def verify_tool_result(label, result, schema):
    """Check framing and declared required members; Rust acceptance validates full schemas."""
    structured = result.get("structuredContent")
    content = result.get("content")
    require(result.get("isError", False) is False and isinstance(structured, dict)
            and isinstance(content, list) and len(content) == 1
            and isinstance(content[0], dict) and content[0].get("type") == "text"
            and isinstance(content[0].get("text"), str), f"{label}: incomplete tool result")
    try:
        text = json.loads(content[0]["text"])
    except ValueError:
        raise SmokeFailure(f"{label}: invalid tool text result") from None
    require(text == structured, f"{label}: tool representations differ")
    required = schema.get("required")
    require(isinstance(required, list) and bool(required) and all(key in structured for key in required),
            f"{label}: tool result drops schema-required members")


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
                  ["git", "rev-parse", "HEAD"], text=True,
                  cwd=pathlib.Path(__file__).resolve().parents[1]).strip()}
    def deadline_exceeded(signum, frame):
        raise SmokeFailure("smoke wall-clock deadline exceeded")

    signal.signal(signal.SIGALRM, deadline_exceeded)
    signal.setitimer(signal.ITIMER_REAL, MAX_SECONDS)
    try:
        report.update(smoke.run(token))
        report["outcome"] = "passed"
    except SmokeFailure as error:
        report.update({"outcome": "failed", "failure": str(error), "checks": smoke.evidence})
    except (ValueError, KeyError, TypeError, AttributeError):
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
