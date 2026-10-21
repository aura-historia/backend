"""Local HTTP checks for the optional smoke command; never contacts stage."""
import importlib.util
import json
import pathlib
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

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
        names = smoke.manifest_names()
        self.assertIn("getPublicOverview", names)
        self.assertIn("getUserAccount", names)
        self.assertNotIn("protectedFixture", names)


if __name__ == "__main__":
    unittest.main()
