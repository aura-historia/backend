#!/usr/bin/env python3
"""R3 test-only idle provider; no Compose, Docker, forwarding, or paid operations.

Run only `python3 compose-provider.py serve`, with this file and smoke-images.py
mounted read-only as siblings. Integrator must use an internal Docker network,
no published ports/host networking, separate application namespaces, no real
credentials, and fresh empty R2 databases with the existing idle-only role grants.
This server cannot enforce those container/network constraints itself.

fixture_environment accepts R2 kinds: api, worker (explicit scope), cron, crawler.
fixture_mounts returns required read-only application credential destinations;
materialize fixture_adc() there. No PostgreSQL CA is needed for STAGE=test/disable.
All credentials are public synthetic fixtures, never suitable outside isolation.
COMMIT_SHA preserves R2 image identity, not the R3 implementation baseline.

GET /_state returns R2-compatible flat counters plus <scope>.<event> counters.
Events: attributes_source_attempt/complete, attributes_dlq_attempt/complete,
receive_attempt/complete, forbidden_attempt, active, unexpected. Global attempts
precede body I/O; scope attribution starts only once a known QueueUrl is decoded.
Completions follow successful response writes, not proof the client consumed them.
Any nonzero unexpected counter fails acceptance; no reset/fault endpoint exists.
ADC/JWKS retain R2's non-forwarding parse/startup-only behavior, not real auth.
No active work, queue custody, provider/TLS, or actual Compose acceptance claimed.
"""
import argparse
import collections
import importlib.util
import http.server
import json
from pathlib import Path
import threading
import time


spec = importlib.util.spec_from_file_location("r2_smoke", Path(__file__).with_name("smoke-images.py"))
r2 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r2)

ENDPOINT = "http://provider:18090"
BIND = ("0.0.0.0", 18090)
ADC_PATH = "/run/aura/google-adc.json"
SHA = r2.SHA
SCOPES = {
    "product-listing-opensearch": 60,
    "search-filter-projection": 60,
    "search-filter-percolator": 300,
    "search-filter-match-notification": 60,
    "watchlist-notification": 60,
    "product-content-assessment": 60,
    "product-embedding": 300,
    "product-translation": 300,
    "product-listing-normalization": 300,
    "notification-delivery": 360,
}
GOOGLE_SCOPES = {"search-filter-percolator", "product-embedding", "product-translation"}
SEARCH_SCOPES = {"product-listing-opensearch", "search-filter-projection", "search-filter-percolator"}
QUEUES = {
    f"{ENDPOINT}/000000000000/aura-worker-{scope}{'-dlq' if dlq else ''}-test": (scope, dlq)
    for scope in SCOPES for dlq in (False, True)
}


def attributes(scope, dlq):
    r2.require(scope in SCOPES and type(dlq) is bool, "QUEUE_IDENTITY")
    result = r2.attributes(scope, dlq)
    result["Attributes"]["VisibilityTimeout"] = str(SCOPES[scope])
    return result


def fixture_environment(kind, scope=None):
    r2.require(kind in r2.BINS, "FIXTURE_KIND")
    r2.require(scope in SCOPES if kind == "worker" else scope is None, "FIXTURE_SCOPE")
    env = r2.environment(kind, scope)
    env.update(COMMIT_SHA=SHA, POSTGRES_HOST="postgres")
    for key, value in env.items():
        if value.startswith(r2.ENDPOINT):
            env[key] = ENDPOINT + value[len(r2.ENDPOINT):]
    env["OPENSEARCH_ENDPOINT_URL"] = "http://opensearch:9200"
    env["GOOGLE_APPLICATION_CREDENTIALS"] = ADC_PATH
    if kind == "api":
        env["AURA_HISTORIA_API_BIND_ADDR"] = "0.0.0.0:8080"
        env.pop("VERTEX_AI_MODEL")  # API composes embeddings, not Gemini.
    elif kind == "worker":
        env["AURA_HISTORIA_WORKER_HEALTH_BIND_ADDR"] = "0.0.0.0:8081"
        if scope not in SEARCH_SCOPES:
            env.pop("OPENSEARCH_ENDPOINT_URL")
        if scope not in GOOGLE_SCOPES:
            for key in ("VERTEX_AI_PROJECT_ID", "VERTEX_AI_LOCATION", "VERTEX_AI_MODEL",
                        "GOOGLE_APPLICATION_CREDENTIALS"):
                env.pop(key)
        elif scope == "product-embedding":
            env.pop("VERTEX_AI_MODEL")
        if scope == "notification-delivery":
            env.update(S3_BUCKET_NAME_TEMPLATES="smoke-templates",
                       NOTIFICATION_EMAIL_FROM="no-reply@example.test",
                       NOTIFICATION_EMAIL_REPLY_TO="contact@example.test",
                       AWS_ENDPOINT_URL_S3=ENDPOINT, AWS_ENDPOINT_URL_SESV2=ENDPOINT)
    elif kind == "crawler":
        for key in ("LOCAL_DB_URL", "BUSINESS_DATABASE_URL"):
            env[key] = env[key].replace("@127.0.0.1:", "@postgres:")
        env.pop("OPENSEARCH_ENDPOINT_URL")
    return env


def fixture_mounts(kind, scope=None):
    """Only ADC-using apps need a provider fixture file; no CA or script mounts."""
    env = fixture_environment(kind, scope)
    return (ADC_PATH,) if "GOOGLE_APPLICATION_CREDENTIALS" in env else ()


def fixture_adc():
    return dict(type="authorized_user", client_id="smoke-client", client_secret=r2.SECRET,
                refresh_token=r2.SECRET, token_uri=ENDPOINT + "/token")


class Provider(r2.Provider):
    counts = collections.Counter()
    lock = threading.Lock()

    def handle_request(self):
        operation = self.headers.get("x-amz-target", "")
        if not operation and (self.command, self.path) in {
            ("GET", "/_state"), ("GET", "/jwks"), ("POST", "/token")
        }:
            if self.command == "GET" and (self.headers.get("Content-Length", "0") != "0"
                                          or self.headers.get("Transfer-Encoding")):
                self.send_error(400)
                return
            return super().handle_request()

        event = {"AmazonSQS.GetQueueAttributes": "attributes",
                 "AmazonSQS.ReceiveMessage": "receive"}.get(operation, "forbidden")
        self.count(event + "_attempt")
        scope = None
        responding = False
        try:
            r2.require(not self.headers.get("Transfer-Encoding"), "BODY_ENCODING")
            size = int(self.headers.get("Content-Length", "0"))
            r2.require(0 < size <= 16384, "BODY_SIZE")
            raw = self.rfile.read(size)
            r2.require(len(raw) == size, "BODY_SIZE")
            data = json.loads(raw)
            r2.require(isinstance(data, dict) and isinstance(data.get("QueueUrl"), str)
                       and data["QueueUrl"] in QUEUES, "QUEUE_IDENTITY")
            scope, dlq = QUEUES[data["QueueUrl"]]
            scoped_event = ("attributes_dlq" if dlq else "attributes_source") if event == "attributes" else event
            self.count(f"{scope}.{scoped_event}_attempt")
            r2.require(self.command == "POST" and self.path == "/", "HTTP_OPERATION")
            if event == "attributes":
                r2.require(data == dict(QueueUrl=data["QueueUrl"], AttributeNames=["All"]), "ATTRIBUTES_REQUEST")
                responding = True
                self.reply(200, attributes(scope, dlq))
                self.count("attributes_complete")
                self.count(f"{scope}.{scoped_event}_complete")
            else:
                r2.require(event == "receive" and not dlq, "SQS_OPERATION")
                r2.require(data == dict(QueueUrl=data["QueueUrl"], WaitTimeSeconds=20,
                    MaxNumberOfMessages=1, VisibilityTimeout=SCOPES[scope],
                    MessageSystemAttributeNames=r2.SYSTEM_ATTRIBUTES)
                    and all(type(data[key]) is int for key in
                            ("WaitTimeSeconds", "MaxNumberOfMessages", "VisibilityTimeout")), "RECEIVE_REQUEST")
                self.count("active")
                self.count(f"{scope}.active")
                try:
                    time.sleep(20)
                    responding = True
                    self.reply(200, {})
                    self.count("receive_complete")
                    self.count(f"{scope}.receive_complete")
                finally:
                    self.count("active", -1)
                    self.count(f"{scope}.active", -1)
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            # Cancellation after an accepted request is not a completed poll.
            if not responding:
                self.count("unexpected")
                if scope:
                    self.count(f"{scope}.unexpected")
            self.close_connection = True
        except Exception:
            self.count("unexpected")
            if scope:
                self.count(f"{scope}.unexpected")
            self.close_connection = True

    do_GET = do_POST = do_PUT = do_CONNECT = do_DELETE = do_HEAD = handle_request


class ProviderServer(http.server.ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 32

    def __init__(self, address, handler):
        self.slots = threading.BoundedSemaphore(32)
        super().__init__(address, handler)

    def process_request(self, request, address):
        if not self.slots.acquire(blocking=False):
            Provider.count("unexpected")
            self.shutdown_request(request)
            return
        super().process_request(request, address)

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            self.slots.release()

    def handle_error(self, *_args):
        Provider.count("unexpected")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("command", choices=("serve",))
    parser.parse_args(argv)
    with ProviderServer(BIND, Provider) as server:
        server.serve_forever()


if __name__ == "__main__":
    main()
