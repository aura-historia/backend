"""Local HTTP checks for the optional smoke command; never contacts stage."""
import importlib.util
import json
import pathlib
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("mcp_smoke", pathlib.Path(__file__).parents[1] / "mcp-smoke.py")
smoke = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(smoke)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "/must-not-follow")
        else:
            self.send_response(200)
        if self.path != "/cacheable":
            self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(b"x" * (smoke.MAX_BYTES + 1) if self.path == "/large" else b"{}")

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.headers.get("Origin") == "null":
            self.send_response(403)
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            return
        self.send_response(401)
        self.send_header("Cache-Control", "no-store")
        self.send_header("WWW-Authenticate", 'Bearer error="invalid_token"')
        self.end_headers()
        self.wfile.write(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32000}}).encode())


class SmokeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.origin = f"http://127.0.0.1:{cls.server.server_port}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join()

    def test_rejects_redirect_without_sending_credentials_to_another_location(self):
        runner = smoke.Smoke(self.origin)
        with self.assertRaisesRegex(smoke.SmokeFailure, "unexpected HTTP status"):
            runner.request("redirect", "/redirect", 200, headers={"Authorization": "Bearer private-fixture"})
        self.assertEqual([302], [check["status"] for check in runner.evidence])

    def test_keeps_http_auth_failure_and_safe_headers_without_token_or_response_body(self):
        runner = smoke.Smoke(self.origin)
        headers, _ = runner.rpc("invalid bearer", "tools/list", {}, 401, token="private-fixture")
        self.assertIn("invalid_token", headers["WWW-Authenticate"])
        self.assertNotIn("private-fixture", json.dumps(runner.evidence))
        self.assertNotIn("jsonrpc", json.dumps(runner.evidence))

    def test_rejects_cacheable_response(self):
        with self.assertRaisesRegex(smoke.SmokeFailure, "missing no-store"):
            smoke.Smoke(self.origin).request("cache", "/cacheable", 200)

    def test_accepts_empty_http_origin_denial_without_requiring_json_rpc_framing(self):
        runner = smoke.Smoke(self.origin)
        _, body = runner.rpc("null Origin", "tools/list", {}, 403, headers={"Origin": "null"})
        self.assertIsNone(body)
        self.assertEqual(403, runner.evidence[0]["status"])

    def test_rejects_oversized_response(self):
        with self.assertRaisesRegex(smoke.SmokeFailure, "response exceeds"):
            smoke.Smoke(self.origin).request("large", "/large", 200)

    def test_enforces_request_and_elapsed_time_budgets(self):
        runner = smoke.Smoke(self.origin)
        runner.evidence = [{}] * smoke.MAX_REQUESTS
        with self.assertRaisesRegex(smoke.SmokeFailure, "budget exhausted"):
            runner.request("over request budget", "/", 200)
        runner = smoke.Smoke(self.origin)
        runner.started -= smoke.MAX_SECONDS
        with self.assertRaisesRegex(smoke.SmokeFailure, "budget exhausted"):
            runner.request("over time budget", "/", 200)

    def test_reads_real_disjoint_manifests(self):
        names = smoke.manifest_operations()
        self.assertIn("getPublicOverview", names)
        self.assertIn("getUserAccount", names)
        self.assertEqual(38, len(names))
        self.assertNotIn(smoke.REMOVED_OPERATION, names)
        self.assertNotIn("submitProductListingDelivery", names)
        self.assertNotIn("protectedFixture", names)

    def test_rejects_incomplete_or_unsuccessful_json_rpc_even_with_http_200(self):
        runner = smoke.Smoke(self.origin)
        for body in ({}, [], {"jsonrpc": "2.0", "id": 1},
                     {"jsonrpc": "2.0", "id": True, "result": {}},
                     {"jsonrpc": "2.0", "id": 2, "result": {}},
                     {"jsonrpc": "2.0", "id": 1, "result": [], "error": {}},
                     {"jsonrpc": "2.0", "id": 1, "result": {}, "error": {}}):
            with self.subTest(body=body), patch.object(runner, "request", return_value=({}, json.dumps(body).encode())):
                with self.assertRaisesRegex(smoke.SmokeFailure, "JSON-RPC"):
                    runner.rpc("incomplete", "tools/list", {})

    def test_accepts_framed_json_rpc_result_and_rejects_discovery_without_revisions(self):
        runner = smoke.Smoke(self.origin)
        body = {"jsonrpc": "2.0", "id": 1, "result": {"supportedVersions": [smoke.CURRENT, *smoke.LEGACY]}}
        with patch.object(runner, "request", return_value=({}, json.dumps(body).encode())):
            _, result = runner.rpc("discovery", "server/discover", {})
        self.assertEqual(body["result"], result)
        with patch.object(runner, "request"), patch.object(runner, "rpc", return_value=({}, {})):
            with self.assertRaisesRegex(smoke.SmokeFailure, "supported revisions"):
                runner.verify_protocols()

    def test_requires_matching_complete_tool_representations_using_advertised_members(self):
        # A protocol-only schema probe; business fields remain owned by shared Rust DTOs.
        schema = {"required": ["syntheticMember"]}
        complete = {"structuredContent": {"syntheticMember": 1}, "content": [
            {"type": "text", "text": '{"syntheticMember":1}'}]}
        smoke.verify_tool_result("selected read", complete, schema)
        for result in ({}, {"isError": True},
                       {"structuredContent": {}, "content": [{"type": "text", "text": "{}"}]},
                       {**complete, "content": [{"type": "text", "text": "{}"}]}):
            with self.subTest(result=result), self.assertRaises(smoke.SmokeFailure):
                smoke.verify_tool_result("selected read", result, schema)

    def test_never_records_reflected_credentials_in_response_headers(self):
        token = "private-fixture"
        headers = {key: token for key in (
            "Cache-Control", "Allow", "Access-Control-Allow-Origin", "MCP-Protocol-Version",
            "Retry-After", "Access-Control-Allow-Headers", "Access-Control-Expose-Headers")}
        headers["Access-Control-Allow-Methods"] = token
        headers["WWW-Authenticate"] = f'Bearer error_description="{token}"'
        rendered = json.dumps(smoke.evidence_headers(headers))
        self.assertNotIn(token, rendered)
        self.assertIn("WWW-Authenticate", rendered)

    def test_checks_legacy_catalogue_duplicates_and_manifest_policies(self):
        # No second catalogue: construct metadata from the committed manifests.
        tools = [{"name": name, "inputSchema": {"type": "object"},
                  "outputSchema": {"type": "object"}, "annotations": policy["annotations"],
                  "_meta": {"aura.historia/operationId": name,
                            "aura.historia/access": policy["access"],
                            "aura.historia/requiredScopes": policy["requiredScopes"]}}
                 for name, policy in smoke.manifest_operations().items()]
        smoke.verify_catalogue({"tools": tools})
        with self.assertRaisesRegex(smoke.SmokeFailure, "catalogue differs"):
            smoke.verify_catalogue({"tools": tools + [tools[0]]})
        tools[0]["_meta"]["aura.historia/requiredScopes"] = ["invented:scope"]
        with self.assertRaisesRegex(smoke.SmokeFailure, "policy differs"):
            smoke.verify_catalogue({"tools": tools})

    def test_reads_single_catalogue_page_and_rejects_any_continuation(self):
        runner = smoke.Smoke(self.origin)
        page = {"tools": [{"name": name} for name in sorted(smoke.manifest_operations())]}
        with patch.object(runner, "rpc", return_value=({}, page)) as rpc, \
                patch.object(smoke, "verify_catalogue", return_value="complete") as verify:
            self.assertEqual("complete", runner.catalogue("local catalogue"))
            self.assertEqual(1, rpc.call_count)
            self.assertEqual(38, len(verify.call_args.args[0]["tools"]))
        with patch.object(runner, "rpc", return_value=({}, {**page, "nextCursor": "more"})):
            with self.assertRaisesRegex(smoke.SmokeFailure, "page bound"):
                runner.catalogue("local catalogue")

    def test_requires_removed_partner_operation_to_fail_as_unknown_tool(self):
        runner = smoke.Smoke(self.origin)
        unknown = {"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": "Unknown tool"}}
        with patch.object(runner, "request", return_value=({}, json.dumps(unknown).encode())) as request:
            runner.verify_removed_operation()
            self.assertEqual(smoke.REMOVED_OPERATION, request.call_args.kwargs["headers"]["Mcp-Name"])
        for body in ({"jsonrpc": "2.0", "id": 1, "result": {"content": []}},
                     {"jsonrpc": "2.0", "id": 1, "error": {"code": -32603}}):
            with self.subTest(body=body), \
                    patch.object(runner, "request", return_value=({}, json.dumps(body).encode())):
                with self.assertRaisesRegex(smoke.SmokeFailure, "callable"):
                    runner.verify_removed_operation()

    def test_checks_exact_auth_challenges_and_browser_preflight_admission(self):
        runner = smoke.Smoke(self.origin)
        metadata = f'resource_metadata="{self.origin}/.well-known/oauth-protected-resource/v1"'
        protected = {"WWW-Authenticate": f'Bearer {metadata}, scope="users:read"'}
        invalid = {"WWW-Authenticate": f'Bearer {metadata}, error="invalid_token"'}
        cors = {"Access-Control-Allow-Origin": "https://stage.aura-historia.com",
                "Access-Control-Expose-Headers": "www-authenticate,x-request-id,x-correlation-id,content-type",
                "Access-Control-Allow-Headers": "authorization,content-type,mcp-protocol-version,mcp-method,mcp-name",
                "Access-Control-Allow-Methods": "GET,HEAD,POST,OPTIONS"}

        def rpc(label, *args, **kwargs):
            headers = {"protected missing bearer": protected, "invalid bearer on public operation": invalid,
                       "allowed Origin": cors}.get(label, {})
            return headers, None

        def request(label, *args, **kwargs):
            return (cors if label == "allowed preflight" else {"Allow": "POST"}), None

        with patch.object(runner, "rpc", side_effect=rpc), patch.object(runner, "request", side_effect=request):
            runner.verify_http_boundary()
            protected["WWW-Authenticate"] = 'Bearer resource_metadata="https://foreign.example"'
            with self.assertRaisesRegex(smoke.SmokeFailure, "resource/scope challenge"):
                runner.verify_http_boundary()
            protected["WWW-Authenticate"] = f'Bearer {metadata}, scope="users:read"'
            cors["Access-Control-Allow-Headers"] = "authorization,mcp-protocol-version,mcp-method,mcp-name"
            with self.assertRaisesRegex(smoke.SmokeFailure, "preflight drops"):
                runner.verify_http_boundary()


if __name__ == "__main__":
    unittest.main()
