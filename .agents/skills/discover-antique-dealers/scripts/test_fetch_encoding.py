"""Exercise decoding through actual public HTTP fetches and cached page evidence."""

import codecs
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import tempfile
from threading import Thread
import unittest

from fetch import fetch_page


class EncodingHandler(BaseHTTPRequestHandler):
    fixtures = {}

    def do_GET(self):
        encoding, content = self.fixtures[self.path]
        self.send_response(200)
        self.send_header("Content-Type", "text/html" + (
            "; charset=" + encoding if encoding else ""))
        self.send_header("Content-Length", str(len(content)))
        self.end_headers()
        self.wfile.write(content)

    def log_message(self, *args):
        pass


class FetchEncodingTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), EncodingHandler)
        cls.thread = Thread(target=cls.server.serve_forever,
                            kwargs={"poll_interval": .01}, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join()

    def fetch(self, encoding, content):
        EncodingHandler.fixtures["/page"] = (encoding, content)
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            result = fetch_page("http://127.0.0.1:" + str(
                self.server.server_port) + "/page", cache)
            pages = list(cache.glob("*.json"))
            cached = json.loads(pages[0].read_text()) if pages else None
        return result, cached

    def test_valid_page_declaration_survives_unknown_http_label(self):
        result, cached = self.fetch("iso-8559-15", (
            '<meta charset="utf8"><main>Rungestraße · Bücher 20 €</main>'
        ).encode("utf-8"))
        self.assertNotIn("error", result)
        self.assertEqual("Rungestraße · Bücher 20 €", cached["mainText"])
        self.assertEqual("utf-8", cached["encoding"])
        self.assertEqual(0, cached["decodingReplacements"])

    def test_transport_and_bom_precedence_preserves_actual_cached_text(self):
        fixtures = [
            ("windows-1252", b'<meta charset="utf-8"><main>\xc4 20 \x80</main>'),
            ("iso-8559-15", b'<meta charset="iso-8859-1"><main>\xc4 20 \x80</main>'),
            ("iso-8859-1", codecs.BOM_UTF8 + '<main>Ä 20 €</main>'.encode("utf-8")),
            ("iso-8859-1", codecs.BOM_UTF16_LE + '<main>Ä 20 €</main>'.encode("utf-16-le")),
            ("iso-8859-1", codecs.BOM_UTF16_BE + '<main>Ä 20 €</main>'.encode("utf-16-be")),
            (None, '<main>Ä 20 €</main>'.encode("utf-8")),
        ]
        for encoding, body in fixtures:
            with self.subTest(encoding=encoding, prefix=body[:8]):
                result, cached = self.fetch(encoding, body)
                self.assertNotIn("error", result)
                self.assertEqual("Ä 20 €", cached["mainText"])
                self.assertEqual(0, cached["decodingReplacements"])

    def test_unknown_declarations_do_not_produce_guessed_evidence(self):
        for encoding, body in [
            ("unknown-http", b'<meta charset="unknown-page"><main>Evidence</main>'),
            ("unknown-http", b'<main>Evidence</main>'),
            (None, b'<meta charset="unknown-page"><main>Evidence</main>'),
        ]:
            with self.subTest(encoding=encoding):
                result, cached = self.fetch(encoding, body)
                self.assertIn("error", result)
                self.assertIsNone(cached)


if __name__ == "__main__":
    unittest.main()
